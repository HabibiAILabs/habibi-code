//! Native Ollama chat adapter. No OAuth, API-key fallback, model pulls, or implicit discovery.
use crate::{Event, Message, ToolCall, auth, provider::Completion, session::emit, tools};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: url::Url,
}

impl Client {
    pub fn new(base_url: &str) -> Result<Self> {
        let base = url::Url::parse(&format!("{}/", base_url.trim_end_matches('/')))
            .context("Invalid Ollama URL; use http://host:11434")?;
        ensure!(
            matches!(base.scheme(), "http" | "https")
                && base.host_str().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Ollama URL must be HTTP(S), without credentials, query, or fragment"
        );
        Ok(Self {
            http: auth::http_client()?,
            base,
        })
    }

    /// Explicitly list installed model names. Does not load or download models.
    pub async fn models(&self) -> Result<Vec<String>> {
        let response = self
            .http
            .get(self.base.join("api/tags")?)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("Could not reach Ollama; start `ollama serve` or check the Ollama URL")?;
        ensure!(
            response.status().is_success(),
            "Ollama model discovery failed ({})",
            response.status()
        );
        let bytes = bounded_body(response, 4 * 1024 * 1024).await?;
        let body: Value = serde_json::from_slice(&bytes).context("Invalid Ollama model list")?;
        let mut models: Vec<String> = body["models"]
            .as_array()
            .context("Missing Ollama models")?
            .iter()
            .map(|m| {
                m["name"]
                    .as_str()
                    .map(str::to_owned)
                    .context("Missing Ollama model name")
            })
            .collect::<Result<_>>()?;
        models.sort();
        models.dedup();
        Ok(models)
    }

    pub(crate) async fn complete(
        &self,
        model: &str,
        system: &str,
        history: &[Message],
        events: &mpsc::Sender<Event>,
        summary: bool,
    ) -> Result<Completion> {
        let response = self
            .http
            .post(self.base.join("api/chat")?)
            .timeout(Duration::from_secs(300))
            .json(&request_body(model, system, history, summary))
            .send()
            .await
            .context("Ollama request failed; start `ollama serve` or check the Ollama URL")?;
        ensure!(
            response.status().is_success(),
            "Ollama rejected the request ({}). Check that the model is installed and supports tools; Habibi never pulls models automatically.",
            response.status()
        );
        consume(response, events).await
    }
}

async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Ollama response exceeds safety limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn request_body(model: &str, system: &str, history: &[Message], summary: bool) -> Value {
    let mut messages = vec![json!({"role":"system","content":system})];
    for message in history {
        match message {
            Message::User { text } => messages.push(json!({"role":"user","content":text})),
            Message::Assistant { native, .. } => messages.extend(native.iter().cloned()),
            Message::ToolResult {
                call_id, output, ..
            } => {
                let name = history
                    .iter()
                    .find_map(|m| match m {
                        Message::Assistant { tool_calls, .. } => tool_calls
                            .iter()
                            .find(|c| c.id == *call_id)
                            .map(|c| c.name.as_str()),
                        _ => None,
                    })
                    .unwrap_or("");
                messages.push(json!({"role":"tool","tool_name":name,"content":output}));
            }
        }
    }
    let mut body = json!({"model":model,"messages":messages,"stream":true});
    if !summary {
        body["tools"] = json!(
            tools::definitions()
                .iter()
                .map(|d| json!({"type":"function","function":{
                    "name":d.name,"description":d.description,"parameters":d.schema
                }}))
                .collect::<Vec<_>>()
        );
    }
    body
}

#[derive(Default)]
struct State {
    pending: Vec<u8>,
    total: usize,
    done: bool,
    text: String,
    thinking: String,
    calls: Vec<ToolCall>,
    native_calls: Vec<Value>,
}

impl State {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>> {
        self.total += chunk.len();
        ensure!(
            self.total <= 32 * 1024 * 1024,
            "Ollama response exceeds 32 MiB safety limit"
        );
        self.pending.extend_from_slice(chunk);
        let mut deltas = Vec::new();
        let mut consumed = 0;
        while let Some(end) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + end;
            let line = self.pending[consumed..end].to_vec();
            if !line.iter().all(u8::is_ascii_whitespace)
                && let Some(text) = self
                    .accept(serde_json::from_slice(&line).context("Invalid Ollama stream event")?)?
            {
                deltas.push(text);
            }
            consumed = end + 1;
        }
        self.pending.drain(..consumed);
        ensure!(
            self.pending.len() <= 8 * 1024 * 1024,
            "Ollama stream frame exceeds 8 MiB"
        );
        Ok(deltas)
    }

    fn accept(&mut self, event: Value) -> Result<Option<String>> {
        ensure!(!self.done, "Ollama sent data after completion");
        if event.get("error").is_some() {
            bail!("Ollama reported an error; partial response was not committed");
        }
        let message = &event["message"];
        let text = message["content"].as_str().unwrap_or_default();
        self.text.push_str(text);
        self.thinking
            .push_str(message["thinking"].as_str().unwrap_or_default());
        if let Some(calls) = message["tool_calls"].as_array() {
            for call in calls {
                let name = call["function"]["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .context("Missing Ollama tool name")?;
                let arguments = call["function"]["arguments"].clone();
                ensure!(
                    arguments.is_object(),
                    "Ollama tool arguments must be a JSON object"
                );
                // Native Ollama does not require a tool-call ID; give the agent journal a stable one.
                let id = format!("ollama-{}", uuid::Uuid::new_v4());
                self.calls.push(ToolCall {
                    id,
                    name: name.into(),
                    arguments,
                });
                self.native_calls.push(call.clone());
            }
        }
        if event["done"] == true {
            let reason = event["done_reason"].as_str().unwrap_or("stop");
            ensure!(
                reason == "stop",
                "Ollama stopped with {reason}; no tools from this response were executed"
            );
            self.done = true;
        }
        Ok((!text.is_empty()).then(|| text.to_owned()))
    }

    fn finish(self) -> Result<Completion> {
        ensure!(
            self.done,
            "Ollama disconnected before completion; partial response was not committed"
        );
        ensure!(
            !self.text.is_empty() || !self.calls.is_empty(),
            "Ollama returned no text or tool calls"
        );
        let mut native = json!({"role":"assistant","content":self.text});
        if !self.thinking.is_empty() {
            native["thinking"] = json!(self.thinking);
        }
        if !self.native_calls.is_empty() {
            native["tool_calls"] = json!(self.native_calls);
        }
        Ok(Completion {
            native: vec![native],
            text: self.text,
            calls: self.calls,
        })
    }
}

async fn consume(response: reqwest::Response, events: &mpsc::Sender<Event>) -> Result<Completion> {
    let mut stream = response.bytes_stream();
    let mut state = State::default();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(90), stream.next())
        .await
        .context("Ollama stream idle for 90 seconds")?
    {
        for text in state.push(&chunk?)? {
            emit(events, Event::TextDelta(text)).await?;
        }
        if state.done {
            break;
        }
    }
    // Some servers omit the newline on the final JSON object.
    if !state.done && !state.pending.is_empty() {
        let event = serde_json::from_slice(&state.pending).context("Truncated Ollama stream")?;
        if let Some(text) = state.accept(event)? {
            emit(events, Event::TextDelta(text)).await?;
        }
    }
    state.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_chunks_tools_thinking_and_history_roundtrip() {
        let input = concat!(
            "{\"message\":{\"content\":\"héllo\",\"thinking\":\"private\"},\"done\":false}\n",
            "{\"message\":{\"tool_calls\":[{\"function\":{\"name\":\"read\",\"arguments\":{\"path\":\"a\"}}}]},\"done\":false}\n",
            "{\"message\":{},\"done\":true,\"done_reason\":\"stop\"}\n"
        );
        for size in 1..input.len() {
            let mut state = State::default();
            let mut text = String::new();
            for chunk in input.as_bytes().chunks(size) {
                for delta in state.push(chunk).unwrap() {
                    text.push_str(&delta);
                }
            }
            let result = state.finish().unwrap();
            assert_eq!(text, "héllo");
            assert_eq!(result.native[0]["thinking"], "private");
            let history = vec![
                Message::Assistant {
                    text: result.text,
                    native: result.native,
                    tool_calls: result.calls.clone(),
                },
                Message::ToolResult {
                    call_id: result.calls[0].id.clone(),
                    output: "found".into(),
                    is_error: false,
                },
            ];
            let body = request_body("test", "system", &history, false);
            assert_eq!(
                body["messages"][1]["tool_calls"][0]["function"]["arguments"]["path"],
                "a"
            );
            assert_eq!(body["messages"][2]["tool_name"], "read");
            assert_eq!(body["messages"][2]["content"], "found");
            assert!(
                request_body("test", "system", &history, true)
                    .get("tools")
                    .is_none()
            );
        }
    }

    #[test]
    fn rejects_errors_truncation_and_bad_arguments() {
        assert!(State::default().finish().is_err());
        for event in [
            json!({"error":"failure"}),
            json!({"done":true,"done_reason":"length"}),
            json!({"message":{"tool_calls":[{"function":{"name":"read","arguments":"bad"}}]}}),
        ] {
            assert!(State::default().accept(event).is_err());
        }
        assert!(Client::new("file:///tmp").is_err());
        assert!(Client::new("http://user:secret@localhost:11434").is_err());
        assert!(Client::new(DEFAULT_URL).is_ok());
    }
}
