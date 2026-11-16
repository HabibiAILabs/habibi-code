//! Local, parent-owned JSON Lines RPC. Stdout is exclusively protocol output.
use anyhow::{Context, Result, bail, ensure};
use libhabibi::{CancellationToken, Event, Session};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

const MAX_INPUT: usize = 1024 * 1024;
const MAX_QUEUE: usize = 32;
type Job = tokio::task::JoinHandle<(Session, Result<()>, mpsc::Receiver<String>)>;

pub async fn run(
    session: Session,
    path: PathBuf,
    prompt: Option<String>,
    rpc: bool,
    yolo: bool,
) -> Result<()> {
    let (input_tx, input) = mpsc::channel(32);
    if rpc {
        // A detached OS thread avoids Tokio's uncancellable stdin blocking task
        // keeping the runtime alive after an RPC shutdown request.
        std::thread::spawn(move || {
            use std::io::{BufRead, Read};
            let mut stdin = std::io::stdin().lock();
            loop {
                let mut line = Vec::new();
                match (&mut stdin)
                    .take((MAX_INPUT + 1) as u64)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(_) if line.len() <= MAX_INPUT => {
                        if input_tx.blocking_send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        let _ = input_tx.blocking_send(Err("RPC line exceeds 1 MiB".into()));
                        break;
                    }
                    Err(e) => {
                        let _ = input_tx.blocking_send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });
    } else {
        drop(input_tx);
    }
    serve(
        session,
        path,
        prompt,
        rpc,
        yolo,
        input,
        &mut tokio::io::stdout(),
    )
    .await
}

async fn send(output: &mut (impl AsyncWrite + Unpin), value: Value) -> Result<()> {
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    session: Session,
    path: PathBuf,
    initial: Option<String>,
    rpc: bool,
    yolo: bool,
    mut input: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
    output: &mut (impl AsyncWrite + Unpin),
) -> Result<()> {
    let id = session.id().to_string();
    let mut control = crate::control::Control::bind(&id, &path)?;
    eprintln!("Control socket: {}", control.path.display());
    if rpc {
        send(output, json!({"event":"ready","version":1,"session_id":id,"journal":path,"socket":control.path,"tools":if yolo {"auto_approve"} else {"ask"}})).await?;
    } else {
        eprintln!("Session: {}", path.display());
    }
    let mut session = Some(session);
    let (events_tx, mut events) = mpsc::channel(64);
    let mut job: Option<Job> = None;
    let mut cancel = CancellationToken::new();
    let mut steering: Option<mpsc::Sender<String>> = None;
    let mut queue = VecDeque::new();
    if let Some(prompt) = initial {
        queue.push_back(prompt);
    }
    let mut approvals: BTreeMap<String, oneshot::Sender<bool>> = BTreeMap::new();
    let mut closing = false;
    let mut failed = None;
    let result = async {
        loop {
            if job.is_none() {
                if closing { break; }
                if let Some(prompt) = queue.pop_front() {
                    let mut running = session.take().context("Session unavailable")?;
                    let tx = events_tx.clone();
                    cancel = CancellationToken::new();
                    let token = cancel.clone();
                    let (sender, mut receiver) = mpsc::channel(32);
                    steering = Some(sender);
                    job = Some(tokio::spawn(async move {
                        let result = async {
                            tokio::select! {
                                result = crate::connect_mcp(&mut running) => result?,
                                _ = token.cancelled() => bail!("Cancelled"),
                            }
                            running.prompt_with_steering(prompt, tx, token, &mut receiver).await
                        }.await;
                        (running, result, receiver)
                    }));
                } else if !rpc { break; }
            }
            tokio::select! {
                biased;
                event = events.recv() => {
                    let Some(event) = event else { break; };
                    match event {
                        Event::Permission { call, reply } => {
                            if yolo { let _ = reply.send(true); }
                            else if rpc {
                                let call_id = call.id.clone();
                                approvals.insert(call_id, reply);
                                send(output, json!({"event":"permission","call":call})).await?;
                            } else { eprintln!("Denied tool {} (use RPC approvals or explicitly pass --yolo)", call.name); let _ = reply.send(false); }
                        }
                        Event::TextDelta(text) if !rpc => { output.write_all(text.as_bytes()).await?; output.flush().await?; }
                        event if rpc => send(output, event_json(event)).await?,
                        _ => {}
                    }
                }
                completed = async { job.as_mut().unwrap().await }, if job.is_some() => {
                    job = None;
                    let (returned, result, mut receiver) = completed.context("Agent task failed")?;
                    session = Some(returned);
                    steering = None;
                    approvals.clear();
                    // Unconsumed steering is not silently lost at a run boundary.
                    let mut pending = Vec::new();
                    while let Ok(text) = receiver.try_recv() { pending.push(text); }
                    if result.is_err() { queue.clear(); }
                    if result.is_ok() && !cancel.is_cancelled() && !closing { for text in pending.into_iter().rev() { queue.push_front(text); } }
                    if rpc { send(output, json!({"event":"idle","cancelled":cancel.is_cancelled(),"error":result.as_ref().err().map(|e|format!("{e:#}")),"queued":queue.len()})).await?; }
                    else { output.write_all(b"\n").await?; }
                    if let Err(error) = result { failed = Some(error); }
                }
                (request, socket_reply) = async {
                    tokio::select! {
                        request = input.recv(), if rpc => (request, None),
                        Some(request) = control.requests.recv() => (Some(Ok(serde_json::to_vec(&request.value).expect("JSON value"))), Some(request.reply)),
                    }
                }, if !closing => {
                    let Some(request) = request else { closing = true; queue.clear(); approvals.clear(); cancel.cancel(); continue; };
                    let bytes = match request { Ok(bytes) => bytes, Err(error) => { send(output,json!({"id":null,"error":{"message":error}})).await?; closing=true; queue.clear(); approvals.clear(); cancel.cancel(); continue; } };
                    let request: Value = match serde_json::from_slice(&bytes) {
                        Ok(value) => value,
                        Err(error) => { send(output,json!({"id":null,"error":{"message":error.to_string()}})).await?; continue; }
                    };
                    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
                    let response = (|| -> Result<Value> {
                        ensure!(request_id.is_string() || request_id.is_number(), "Request needs a string or numeric id");
                        let method = request["method"].as_str().context("Missing method")?;
                        let params = &request["params"];
                        match method {
                            "status" => Ok(json!({"session_id":id,"busy":job.is_some(),"stopping":cancel.is_cancelled() && job.is_some(),"queued":queue.len(),"pending_approvals":approvals.keys().collect::<Vec<_>>(),"socket":control.path})),
                            "prompt" | "follow_up" | "steer" => {
                                let text = params["text"].as_str().context("params.text must be a string")?;
                                ensure!(!text.trim().is_empty() && text.len() <= MAX_INPUT / 2, "Text must contain 1–524288 bytes");
                                ensure!(!cancel.is_cancelled() || job.is_none(), "Run is stopping; wait for idle");
                                if method == "steer" {
                                    steering.as_ref().context("No active run to steer")?.try_send(text.to_owned()).context("Steering queue full or run ending")?;
                                } else {
                                    ensure!(method != "prompt" || job.is_none(), "Agent busy; use steer or follow_up");
                                    ensure!(queue.len() < MAX_QUEUE, "Follow-up queue full");
                                    queue.push_back(text.to_owned());
                                }
                                Ok(json!({"accepted":true}))
                            }
                            "approve" => {
                                let call_id = params["call_id"].as_str().context("Missing call_id")?;
                                let allow = params["allow"].as_bool().context("allow must be boolean")?;
                                approvals.remove(call_id).context("Unknown or expired approval")?.send(allow).map_err(|_| anyhow::anyhow!("Approval expired"))?;
                                Ok(json!({"accepted":true}))
                            }
                            "stop" | "shutdown" => {
                                cancel.cancel(); queue.clear(); approvals.clear();
                                if method == "shutdown" { closing = true; }
                                Ok(json!({"accepted":true}))
                            }
                            _ => bail!("Unknown method: {method}"),
                        }
                    })();
                    let response = crate::control::response(request_id, response);
                    if let Some(reply) = socket_reply { reply.send(response).await; }
                    else { send(output, response).await?; }
                }
                _ = tokio::signal::ctrl_c(), if !closing => { closing=true; queue.clear(); approvals.clear(); cancel.cancel(); }
            }
        }
        if !rpc && let Some(error) = failed { return Err(error); }
        Ok(())
    }.await;
    // Broken output/transport must not abandon an agent with a live mutation.
    cancel.cancel();
    approvals.clear();
    if let Some(mut job) = job {
        loop {
            tokio::select! { _ = &mut job => break, event = events.recv() => { if event.is_none() { break; } } }
        }
    }
    result
}

fn event_json(event: Event) -> Value {
    match event {
        Event::RunStarted => json!({"event":"run_started"}),
        Event::ProviderRetry {
            retry,
            max_retries,
            delay_secs,
        } => {
            json!({"event":"provider_retry","retry":retry,"max_retries":max_retries,"delay_secs":delay_secs})
        }
        Event::Inspection {
            definitions,
            history,
        } => json!({"event":"inspection","definitions":definitions,"history":history}),
        Event::SteeringApplied(text) => json!({"event":"steering_applied","text":text}),
        Event::CompactionStarted => json!({"event":"compaction_started"}),
        Event::Compacted { before, after } => {
            json!({"event":"compacted","before":before,"after":after})
        }
        Event::ContextUsage { used, limit } => {
            json!({"event":"context_usage","used":used,"limit":limit})
        }
        Event::TextDelta(text) => json!({"event":"text_delta","text":text}),
        Event::ToolStarted(call) => json!({"event":"tool_started","call":call}),
        Event::ToolFinished {
            call_id,
            output,
            is_error,
        } => json!({"event":"tool_finished","call_id":call_id,"output":output,"is_error":is_error}),
        Event::RunFinished { cancelled, error } => {
            json!({"event":"run_finished","cancelled":cancelled,"error":error})
        }
        Event::Permission { .. } => unreachable!("permissions handled separately"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libhabibi::{
        Provider, SessionConfig, Tool, ToolContext, ToolDefinition, ToolFuture, ToolOutput,
        auth::AuthStore,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, DuplexStream, Lines};

    async fn next(lines: &mut Lines<BufReader<DuplexStream>>) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("RPC closed unexpectedly");
        serde_json::from_str(&line).unwrap()
    }
    async fn until(lines: &mut Lines<BufReader<DuplexStream>>, event: &str) -> Value {
        loop {
            let value = next(lines).await;
            if value["event"] == event {
                return value;
            }
        }
    }
    async fn command(
        tx: &mpsc::Sender<std::result::Result<Vec<u8>, String>>,
        method: &str,
        params: Value,
    ) {
        tx.send(Ok(serde_json::to_vec(
            &json!({"id":method,"method":method,"params":params}),
        )
        .unwrap()))
            .await
            .unwrap();
    }
    struct Counter(Arc<AtomicUsize>);
    impl Tool for Counter {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "counter".into(),
                description: "test".into(),
                schema: json!({"type":"object"}),
            }
        }
        fn execute<'a>(&'a self, _: Value, _: ToolContext<'a>) -> ToolFuture<'a> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ToolOutput::text("counted"))
            })
        }
    }
    async fn fixture(
        responses: Vec<Value>,
    ) -> (
        tempfile::TempDir,
        Session,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<Vec<Value>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let (end, length) = loop {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&buf[..n]);
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..i]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        break (i + 4, length);
                    }
                };
                while data.len() < end + length {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&buf[..n]);
                }
                requests.push(serde_json::from_slice(&data[end..end + length]).unwrap());
                let body = format!("{response}\n");
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let dir = tempfile::tempdir().unwrap();
        let mut config = SessionConfig::new(Provider::Ollama, dir.path());
        config.discover_skills = false;
        config.preferences.mcp_enabled = false;
        config.ollama_url = url;
        let mut session = Session::new(config, AuthStore::new(dir.path().join("auth"))).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        session
            .register_tool(Arc::new(Counter(count.clone())))
            .unwrap();
        session.persist(dir.path().join("session.jsonl")).unwrap();
        (dir, session, count, server)
    }
    fn tool_response() -> Value {
        json!({"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"counter","arguments":{}}}]},"done":true})
    }
    fn text_response() -> Value {
        json!({"message":{"role":"assistant","content":"hello"},"done":true})
    }

    #[tokio::test]
    async fn socket_controls_same_session_and_shutdown_is_acknowledged() {
        let (dir, session, _, server) = fixture(vec![]).await;
        let (_tx, rx) = mpsc::channel(32);
        let (mut output, client) = tokio::io::duplex(256 * 1024);
        let path = dir.path().join("session.jsonl");
        let task =
            tokio::spawn(
                async move { serve(session, path, None, true, false, rx, &mut output).await },
            );
        let ready = next(&mut BufReader::new(client).lines()).await;
        let path = ready["socket"].as_str().unwrap();
        let mut socket = tokio::net::UnixStream::connect(path).await.unwrap();
        socket
            .write_all(b"{\"id\":1,\"method\":\"status\"}\n{\"id\":2,\"method\":\"shutdown\"}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(socket).lines();
        let status: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(status["result"]["session_id"], ready["session_id"]);
        let shutdown: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(shutdown["result"]["accepted"], true);
        task.await.unwrap().unwrap();
        assert!(!std::path::Path::new(path).exists());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rpc_approval_steering_followup_and_shutdown() {
        let (dir, session, count, server) =
            fixture(vec![tool_response(), text_response(), text_response()]).await;
        let (tx, rx) = mpsc::channel(32);
        let (mut output, client) = tokio::io::duplex(256 * 1024);
        let path = dir.path().join("session.jsonl");
        let task =
            tokio::spawn(
                async move { serve(session, path, None, true, false, rx, &mut output).await },
            );
        let mut lines = BufReader::new(client).lines();
        assert_eq!(next(&mut lines).await["event"], "ready");
        command(&tx, "status", json!({})).await;
        assert_eq!(next(&mut lines).await["result"]["busy"], false);
        tx.send(Ok(b"not json".to_vec())).await.unwrap();
        assert!(next(&mut lines).await["error"].is_object());
        command(&tx, "approve", json!({"call_id":"missing","allow":true})).await;
        assert!(next(&mut lines).await["error"].is_object());
        command(&tx, "prompt", json!({"text":"first"})).await;
        let permission = until(&mut lines, "permission").await;
        command(&tx, "follow_up", json!({"text":"later"})).await;
        command(&tx, "steer", json!({"text":"correction"})).await;
        command(
            &tx,
            "approve",
            json!({"call_id":permission["call"]["id"],"allow":true}),
        )
        .await;
        assert_eq!(
            until(&mut lines, "steering_applied").await["text"],
            "correction"
        );
        until(&mut lines, "idle").await;
        until(&mut lines, "idle").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        command(&tx, "shutdown", json!({})).await;
        assert_eq!(next(&mut lines).await["id"], "shutdown");
        task.await.unwrap().unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["content"] == "correction")
        );
        assert!(
            requests[2]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["content"] == "later")
        );
    }

    #[tokio::test]
    async fn stop_or_disconnect_denies_pending_approval_and_drains() {
        for disconnect in [false, true] {
            let (dir, session, count, server) = fixture(vec![tool_response()]).await;
            let (tx, rx) = mpsc::channel(32);
            let (mut output, client) = tokio::io::duplex(256 * 1024);
            let path = dir.path().join("session.jsonl");
            let task = tokio::spawn(async move {
                serve(
                    session,
                    path,
                    Some("first".into()),
                    true,
                    false,
                    rx,
                    &mut output,
                )
                .await
            });
            let mut lines = BufReader::new(client).lines();
            until(&mut lines, "permission").await;
            if disconnect {
                drop(tx);
            } else {
                command(&tx, "follow_up", json!({"text":"must not run"})).await;
                command(&tx, "stop", json!({})).await;
                let idle = until(&mut lines, "idle").await;
                assert_eq!(idle["cancelled"], true);
                assert_eq!(idle["queued"], 0);
                command(&tx, "shutdown", json!({})).await;
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(count.load(Ordering::SeqCst), 0);
            server.await.unwrap();
            let resumed = Session::resume(
                dir.path().join("session.jsonl"),
                AuthStore::new(dir.path().join("auth")),
            )
            .unwrap();
            assert!(
                resumed
                    .messages()
                    .iter()
                    .any(|m| matches!(m, libhabibi::Message::ToolResult { is_error: true, .. }))
            );
        }
    }

    #[tokio::test]
    async fn broken_output_cancels_and_releases_session_lock() {
        let (dir, session, count, server) = fixture(vec![tool_response()]).await;
        let (tx, rx) = mpsc::channel(32);
        let (mut output, client) = tokio::io::duplex(256 * 1024);
        let path = dir.path().join("session.jsonl");
        let task = tokio::spawn(async move {
            serve(
                session,
                path,
                Some("first".into()),
                true,
                false,
                rx,
                &mut output,
            )
            .await
        });
        let mut lines = BufReader::new(client).lines();
        until(&mut lines, "permission").await;
        drop(lines);
        command(&tx, "status", json!({})).await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        server.await.unwrap();
        Session::resume(
            dir.path().join("session.jsonl"),
            AuthStore::new(dir.path().join("auth")),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn one_shot_requires_explicit_yolo_even_with_saved_yolo() {
        for yolo in [false, true] {
            let (dir, mut session, count, server) =
                fixture(vec![tool_response(), text_response()]).await;
            let c = session.config().clone();
            let mut preferences = c.preferences.clone();
            preferences.yolo = true;
            session
                .update_settings_with_skills(
                    c.model,
                    c.max_turns,
                    c.max_context_bytes,
                    preferences,
                    c.auto_compact_percent,
                    c.sticky_skills,
                    c.skill_dirs,
                )
                .unwrap();
            let (_tx, rx) = mpsc::channel(1);
            let mut output = Vec::new();
            serve(
                session,
                dir.path().join("session.jsonl"),
                Some("first".into()),
                false,
                yolo,
                rx,
                &mut output,
            )
            .await
            .unwrap();
            assert_eq!(String::from_utf8(output).unwrap(), "hello\n");
            assert_eq!(count.load(Ordering::SeqCst), usize::from(yolo));
            server.await.unwrap();
        }
    }
}
