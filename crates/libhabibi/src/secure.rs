//! Memory-only secret workflows. Transcript hygiene, NOT isolation from tools.
//! Private UI messages deliberately do not implement Debug or Serialize.
use crate::{CancellationToken, Tool, ToolContext, ToolDefinition, ToolFuture, ToolOutput};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, oneshot},
};

const LIMIT: usize = 1_048_576;
#[derive(Clone, Default)]
pub struct Vault(Arc<Mutex<State>>);
#[derive(Default)]
struct State {
    values: BTreeMap<String, String>,
    epoch: CancellationToken,
    ui: Option<mpsc::Sender<PrivateRequest>>,
}
pub struct PrivateRequest {
    pub name: String,
    pub reason: String,
    pub value: Option<String>,
    pub cancel: CancellationToken,
    pub reply: oneshot::Sender<PrivateReply>,
}
#[derive(Default)]
pub struct PrivateReply {
    pub value: Option<String>,
    pub shown: bool,
    pub copied: bool,
}
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
impl Vault {
    pub fn attach_ui(&self) -> mpsc::Receiver<PrivateRequest> {
        let (tx, rx) = mpsc::channel(32);
        self.0.lock().unwrap().ui = Some(tx);
        rx
    }
    pub fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().values.keys().cloned().collect()
    }
    /// Trusted local UI only; never put this value in events, journals or RPC.
    pub fn local_value(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().values.get(name).cloned()
    }
    pub fn epoch(&self) -> CancellationToken {
        self.0.lock().unwrap().epoch.clone()
    }
    pub fn clear(&self, name: Option<&str>) {
        let mut state = self.0.lock().unwrap();
        state.epoch.cancel();
        state.epoch = CancellationToken::new();
        if let Some(name) = name {
            state.values.remove(name);
        } else {
            state.values.clear();
        }
    }
    pub(crate) fn tools(&self) -> Vec<Arc<dyn Tool>> {
        [
            "request_secret",
            "capture_secret",
            "show_secret",
            "secure_exec",
        ]
        .into_iter()
        .map(|name| {
            Arc::new(SecureTool {
                name,
                vault: self.clone(),
            }) as Arc<dyn Tool>
        })
        .collect()
    }
    async fn execute(
        &self,
        tool: &str,
        args: Value,
        context: ToolContext<'_>,
    ) -> Result<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("");
        if tool != "secure_exec" {
            ensure!(valid_name(name), "Invalid secret name");
        }
        let epoch = self.epoch();
        ensure!(!context.cancel.is_cancelled(), "Secret operation cancelled");
        if tool == "request_secret" || tool == "show_secret" {
            let value = self.local_value(name);
            if tool == "request_secret" && value.is_some() {
                return Ok(ToolOutput::text(
                    json!({"name":name,"available":true}).to_string(),
                ));
            }
            ensure!(
                tool != "show_secret" || value.is_some(),
                "Secret unavailable"
            );
            let ui = self.0.lock().unwrap().ui.clone();
            let Some(ui) = ui else {
                bail!(
                    "Private entry/reveal requires the interactive terminal; no chat or RPC fallback"
                );
            };
            let (reply, response) = oneshot::channel();
            let cancel = epoch.child_token();
            // Cancels an already displayed/queued dialog when the invocation ends.
            let _guard = cancel.clone().drop_guard();
            let operation = async {
                ui.send(PrivateRequest {
                    name: name.into(),
                    reason: args["reason"].as_str().unwrap_or("").into(),
                    value: if tool == "show_secret" { value } else { None },
                    cancel,
                    reply,
                })
                .await
                .map_err(|_| anyhow::anyhow!("Private UI unavailable"))?;
                response
                    .await
                    .map_err(|_| anyhow::anyhow!("Private dialog closed"))
            };
            let reply = tokio::select! {
                _ = context.cancel.cancelled() => bail!("Secret operation cancelled"),
                _ = epoch.cancelled() => bail!("Secret operation cancelled"),
                reply = operation => reply?,
            };
            let mut state = self.0.lock().unwrap();
            ensure!(
                !epoch.is_cancelled() && !context.cancel.is_cancelled(),
                "Secret operation cancelled"
            );
            if tool == "request_secret" {
                if let Some(value) = reply.value {
                    ensure!(
                        !value.is_empty()
                            && value.len() <= LIMIT
                            && !value.contains(['\0', '\r', '\n']),
                        "Invalid private input"
                    );
                    state.values.entry(name.into()).or_insert(value);
                }
                return Ok(ToolOutput::text(
                    json!({"name":name,"available":state.values.contains_key(name)}).to_string(),
                ));
            }
            return Ok(ToolOutput::text(
                json!({"name":name,"shown":reply.shown,"copied":reply.copied}).to_string(),
            ));
        }
        let capture = tool == "capture_secret";
        let (env, mut values) = {
            let state = self.0.lock().unwrap();
            ensure!(
                !capture || !state.values.contains_key(name),
                "Secret already exists; choose another name"
            );
            let mut env = BTreeMap::new();
            if let Some(names) = args["secrets"].as_array() {
                for name in names {
                    let name = name.as_str().unwrap_or("");
                    ensure!(valid_name(name), "Invalid secret name");
                    let Some(value) = state.values.get(name) else {
                        bail!("Requested secret unavailable");
                    };
                    env.insert(name.to_owned(), value.clone());
                }
            }
            (env, state.values.values().cloned().collect::<Vec<_>>())
        };
        let seconds = args["timeout"].as_u64().unwrap_or(60);
        ensure!(
            (1..=300).contains(&seconds),
            "Timeout must be 1–300 seconds"
        );
        let command = args["command"].as_str().unwrap_or("");
        ensure!(!command.is_empty(), "Command required");
        let (code, stdout, stderr) = run(
            command,
            context.workspace,
            env,
            &context.cancel,
            &epoch,
            seconds,
        )
        .await?;
        let mut state = self.0.lock().unwrap();
        ensure!(
            !epoch.is_cancelled() && !context.cancel.is_cancelled(),
            "Secret operation cancelled; output discarded"
        );
        if capture {
            ensure!(code == 0, "Capture failed; output discarded");
            let mut value = stdout;
            if args["trimNewline"].as_bool().unwrap_or(true) && value.ends_with('\n') {
                value.pop();
                if value.ends_with('\r') {
                    value.pop();
                }
            }
            ensure!(
                !value.is_empty() && !value.contains('\0'),
                "Capture empty or contains NUL; output discarded"
            );
            ensure!(
                !state.values.contains_key(name),
                "Secret already exists; output discarded"
            );
            state.values.insert(name.into(), value);
            Ok(ToolOutput::text(
                json!({"name":name,"available":true}).to_string(),
            ))
        } else {
            values.extend(state.values.values().cloned());
            let output = json!({"exitCode":code,"stdout":redact(&stdout, &values)?,"stderr":redact(&stderr, &values)?});
            Ok(ToolOutput {
                is_error: code != 0,
                ..ToolOutput::text(output.to_string())
            })
        }
    }
}
struct SecureTool {
    name: &'static str,
    vault: Vault,
}
impl Tool for SecureTool {
    fn definition(&self) -> ToolDefinition {
        let name = json!({"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"});
        let mut props = json!({"name": name});
        let (description, required) = match self.name {
            "request_secret" => {
                props["reason"] = json!({"type":"string"});
                (
                    "Request masked local secret entry. Returns availability only; existing names reused. Never ask for secrets in chat.",
                    vec!["name", "reason"],
                )
            }
            "show_secret" => (
                "Open a private local masked reveal/copy dialog (confirmation required). Returns status only. Requires interactive terminal.",
                vec!["name"],
            ),
            name => {
                if name == "secure_exec" {
                    props = json!({});
                }
                props["command"] = json!({"type":"string","minLength":1});
                props["secrets"] = json!({"type":"array","items":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"}});
                props["timeout"] = json!({"type":"integer","minimum":1,"maximum":300});
                if name == "capture_secret" {
                    props["trimNewline"] = json!({"type":"boolean"});
                    (
                        "Capture shell stdout into memory only. Never returns stdout/stderr, even on failure. No overwrite. Optional secrets inject named environment variables. One trailing newline removed by default. 60s default, 1 MiB combined output limit. Never embed secret literals in arguments.",
                        vec!["name", "command"],
                    )
                } else {
                    (
                        "Run shell with named secrets in child environment. Buffer and redact exact values of ALL stored secrets in stdout/stderr. 60s default, 1 MiB combined limit. Transcript hygiene, NOT a sandbox: commands can write/exfiltrate/encode secrets. Never embed literal secrets in arguments or use ordinary tools to read secret-bearing files. Requires normal tool approval.",
                        vec!["command", "secrets"],
                    )
                }
            }
        };
        ToolDefinition {
            name: self.name.into(),
            description: description.into(),
            schema: json!({"type":"object","properties":props,"required":required,"additionalProperties":false}),
        }
    }
    fn execute<'a>(&'a self, arguments: Value, context: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(self.vault.execute(self.name, arguments, context))
    }
}
fn redact(text: &str, values: &[String]) -> Result<String> {
    // Literal multi-pattern matching avoids regex escaping/engine limits and
    // repeated rescans of the entire buffer for each occurrence of a short value.
    let patterns: std::collections::BTreeSet<&str> = values
        .iter()
        .map(String::as_str)
        .filter(|v| !v.is_empty())
        .collect();
    ensure!(
        patterns.iter().map(|p| p.len()).sum::<usize>() <= 16 * LIMIT,
        "Redaction budget exceeded; output withheld. Clear unneeded secrets. Check command effects before retrying"
    );
    let matcher = aho_corasick::AhoCorasick::builder()
        .match_kind(aho_corasick::MatchKind::LeftmostLongest)
        .kind(Some(aho_corasick::AhoCorasickKind::NoncontiguousNFA))
        .build(patterns)
        .map_err(|_| {
            anyhow::anyhow!(
                "Redaction failed; output withheld. Check command effects before retrying"
            )
        })?;
    let mut output = String::new();
    let mut end = 0;
    for found in matcher.find_iter(text) {
        output.push_str(&text[end..found.start()]);
        output.push_str("[REDACTED]");
        end = found.end();
    }
    output.push_str(&text[end..]);
    Ok(output)
}
#[cfg(unix)]
struct Group(u32);
#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
async fn run(
    command: &str,
    cwd: &std::path::Path,
    env: BTreeMap<String, String>,
    cancel: &CancellationToken,
    epoch: &CancellationToken,
    seconds: u64,
) -> Result<(i32, String, String)> {
    ensure!(cfg!(unix), "Secure shell execution requires Unix");
    let mut builder = tokio::process::Command::new("/bin/sh");
    builder
        .args(["-c", command])
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    builder.process_group(0);
    let mut child = builder
        .spawn()
        .map_err(|_| anyhow::anyhow!("Could not start secure command"))?;
    #[cfg(unix)]
    let group = Group(child.id().unwrap());
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let operation = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut a = [0u8; 8192];
        let mut b = [0u8; 8192];
        let mut out_done = false;
        let mut err_done = false;
        let mut status = None;
        while !out_done || !err_done || status.is_none() {
            tokio::select! {
                s = child.wait(), if status.is_none() => {
                    status = Some(s.map_err(|_| anyhow::anyhow!("Secure command failed"))?);
                    #[cfg(unix)] unsafe { libc::kill(-(group.0 as i32), libc::SIGKILL); }
                },
                n = stdout.read(&mut a), if !out_done => { let n = n.map_err(|_| anyhow::anyhow!("Secure output read failed"))?; out_done = n == 0; out.extend_from_slice(&a[..n]); },
                n = stderr.read(&mut b), if !err_done => { let n = n.map_err(|_| anyhow::anyhow!("Secure output read failed"))?; err_done = n == 0; err.extend_from_slice(&b[..n]); },
            }
            ensure!(
                out.len() + err.len() <= LIMIT,
                "Secure output limit exceeded; output discarded"
            );
        }
        let code = status.unwrap().code().ok_or_else(|| {
            anyhow::anyhow!("Secure command terminated by signal; output discarded")
        })?;
        // Reject invalid UTF-8 rather than leak differently-decoded secret bytes.
        let out = String::from_utf8(out)
            .map_err(|_| anyhow::anyhow!("Invalid secure output encoding; output discarded"))?;
        let err = String::from_utf8(err)
            .map_err(|_| anyhow::anyhow!("Invalid secure output encoding; output discarded"))?;
        Ok((code, out, err))
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("Secure command cancelled; output discarded")),
        _ = epoch.cancelled() => Err(anyhow::anyhow!("Secure command cancelled; output discarded")),
        result = tokio::time::timeout(Duration::from_secs(seconds), operation) => result.unwrap_or_else(|_| Err(anyhow::anyhow!("Secure command timed out; output discarded"))),
    };
    #[cfg(unix)]
    drop(group);
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn call(vault: &Vault, tool: &str, args: Value) -> Result<ToolOutput> {
        vault
            .execute(
                tool,
                args,
                ToolContext {
                    workspace: std::path::Path::new("/tmp"),
                    cancel: CancellationToken::new(),
                },
            )
            .await
    }
    fn put(vault: &Vault, name: &str, value: &str) {
        vault
            .0
            .lock()
            .unwrap()
            .values
            .insert(name.into(), value.into());
    }
    #[test]
    fn names_and_non_recursive_longest_redaction() {
        for n in ["A", "_", "a_2"] {
            assert!(valid_name(n));
        }
        for n in ["", "1A", "A=B", "a-b", "é", "A\n"] {
            assert!(!valid_name(n));
        }
        let values = ["ab", "abc", "[REDACTED]", "♥", "a.*"].map(String::from);
        assert_eq!(
            redact("abc ab ♥ a.* xyz", &values).unwrap(),
            "[REDACTED] [REDACTED] [REDACTED] [REDACTED] xyz"
        );
    }
    #[test]
    fn redaction_is_bounded_and_handles_many_short_matches() {
        assert_eq!(redact("public", &[]).unwrap(), "public");
        let values = vec!["x".into(), "y".repeat(100_000)];
        let output = redact(&"x".repeat(100_000), &values).unwrap();
        assert_eq!(output, "[REDACTED]".repeat(100_000));
        assert!(redact("withheld", &["a".repeat(16 * LIMIT + 1)]).is_err());
    }
    #[test]
    fn schemas_register_and_reject_literal_value_arguments() {
        let vault = Vault::default();
        let mut registry = crate::registry::ToolRegistry::new().unwrap();
        registry.register_many(vault.tools()).unwrap();
        assert!(
            registry
                .validate(
                    "request_secret",
                    &json!({"name":"TOKEN","reason":"test","value":"must not be passed"})
                )
                .is_err()
        );
        assert!(
            registry
                .validate(
                    "secure_exec",
                    &json!({"command":"true","secrets":[],"timeout":301})
                )
                .is_err()
        );
    }
    #[tokio::test]
    async fn entry_is_private_and_reuses_names() {
        let vault = Vault::default();
        let mut ui = vault.attach_ui();
        let task = call(
            &vault,
            "request_secret",
            json!({"name":"TOKEN","reason":"synthetic test"}),
        );
        let local = async {
            let req = ui.recv().await.unwrap();
            assert!(req.value.is_none());
            req.reply
                .send(PrivateReply {
                    value: Some("synthetic-private".into()),
                    ..Default::default()
                })
                .ok();
        };
        let (result, ()) = tokio::join!(task, local);
        assert_eq!(
            result.unwrap().text,
            json!({"name":"TOKEN","available":true}).to_string()
        );
        assert_eq!(
            vault.local_value("TOKEN").as_deref(),
            Some("synthetic-private")
        );
        let result = call(
            &vault,
            "request_secret",
            json!({"name":"TOKEN","reason":"reuse"}),
        )
        .await
        .unwrap();
        assert!(!result.text.contains("synthetic-private"));
        assert!(ui.try_recv().is_err());
        let show = call(&vault, "show_secret", json!({"name":"TOKEN"}));
        let local = async {
            let req = ui.recv().await.unwrap();
            assert_eq!(req.value.as_deref(), Some("synthetic-private"));
            req.reply
                .send(PrivateReply {
                    shown: true,
                    copied: false,
                    value: None,
                })
                .ok();
        };
        let (result, ()) = tokio::join!(show, local);
        assert_eq!(
            result.unwrap().text,
            json!({"name":"TOKEN","shown":true,"copied":false}).to_string()
        );
    }
    #[tokio::test]
    async fn headless_fails_closed() {
        let vault = Vault::default();
        assert!(
            call(
                &vault,
                "request_secret",
                json!({"name":"TOKEN","reason":"test"})
            )
            .await
            .is_err()
        );
        put(&vault, "TOKEN", "synthetic-private");
        assert!(
            call(&vault, "show_secret", json!({"name":"TOKEN"}))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn clear_invalidates_pending_input_and_preserves_other_names() {
        let vault = Vault::default();
        put(&vault, "KEEP", "kept");
        let mut ui = vault.attach_ui();
        let task = call(
            &vault,
            "request_secret",
            json!({"name":"TOKEN","reason":"test"}),
        );
        let clear = async {
            let req = ui.recv().await.unwrap();
            vault.clear(Some("TOKEN"));
            assert!(req.cancel.is_cancelled());
            req.reply
                .send(PrivateReply {
                    value: Some("late-value".into()),
                    ..Default::default()
                })
                .ok();
        };
        let (result, ()) = tokio::join!(task, clear);
        assert!(result.is_err());
        assert_eq!(vault.names(), vec!["KEEP"]);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn capture_is_private_trimmed_and_never_overwrites() {
        let vault = Vault::default();
        let result=call(&vault,"capture_secret",json!({"name":"TOKEN","command":"printf 'synthetic-private\\r\\n'; printf 'private-stderr' >&2"})).await.unwrap();
        assert_eq!(
            result.text,
            json!({"name":"TOKEN","available":true}).to_string()
        );
        assert_eq!(
            vault.local_value("TOKEN").as_deref(),
            Some("synthetic-private")
        );
        assert!(
            call(
                &vault,
                "capture_secret",
                json!({"name":"TOKEN","command":"printf replaced"})
            )
            .await
            .is_err()
        );
        call(
            &vault,
            "capture_secret",
            json!({"name":"MULTI","command":"printf 'a\\n\\n'","trimNewline":false}),
        )
        .await
        .unwrap();
        assert_eq!(vault.local_value("MULTI").as_deref(), Some("a\n\n"));
        for command in [
            "printf synthetic-private; printf private-stderr >&2; exit 3",
            "true",
            "printf '\\0'",
            "printf '\\377'",
        ] {
            let error = call(
                &vault,
                "capture_secret",
                json!({"name":"FAIL","command":command}),
            )
            .await
            .err()
            .unwrap()
            .to_string();
            assert!(!error.contains("synthetic-private") && !error.contains("private-stderr"));
            assert!(vault.local_value("FAIL").is_none());
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn exec_redacts_all_values_and_does_not_modify_parent_environment() {
        let vault = Vault::default();
        put(&vault, "HABIBI_SECURE_TEST", "synthetic-private");
        put(&vault, "OTHER", "hidden-other");
        let before = std::env::var_os("HABIBI_SECURE_TEST");
        let result=call(&vault,"secure_exec",json!({"command":"printf '%s hidden-other' \"$HABIBI_SECURE_TEST\"; printf '%s' \"$HABIBI_SECURE_TEST\" >&2; exit 7","secrets":["HABIBI_SECURE_TEST"]})).await.unwrap();
        assert!(result.is_error);
        let output: Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(output["stdout"], "[REDACTED] [REDACTED]");
        assert_eq!(output["stderr"], "[REDACTED]");
        assert_eq!(output["exitCode"], 7);
        assert_eq!(std::env::var_os("HABIBI_SECURE_TEST"), before);
        let result=call(&vault,"capture_secret",json!({"name":"COPY","command":"printf '%s' \"$HABIBI_SECURE_TEST\"","secrets":["HABIBI_SECURE_TEST"]})).await.unwrap();
        assert!(!result.text.contains("synthetic-private"));
        assert_eq!(
            vault.local_value("COPY"),
            vault.local_value("HABIBI_SECURE_TEST")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn overflow_timeout_and_cancellation_discard_output() {
        let vault = Vault::default();
        for command in [
            "printf synthetic-private; head -c 1048577 /dev/zero",
            "printf synthetic-private; sleep 5",
        ] {
            let error = call(
                &vault,
                "capture_secret",
                json!({"name":"TOKEN","command":command,"timeout":1}),
            )
            .await
            .err()
            .unwrap()
            .to_string();
            assert!(!error.contains("synthetic-private"));
            assert!(vault.names().is_empty());
        }
        let cancel = CancellationToken::new();
        let work = vault.execute(
            "capture_secret",
            json!({"name":"TOKEN","command":"printf synthetic-private; sleep 10"}),
            ToolContext {
                workspace: std::path::Path::new("/tmp"),
                cancel: cancel.clone(),
            },
        );
        let stop = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(work, stop);
        assert!(result.is_err());
        assert!(vault.names().is_empty());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn clear_cancels_capture_before_commit() {
        let vault = Vault::default();
        let work = call(
            &vault,
            "capture_secret",
            json!({"name":"TOKEN","command":"sleep 1; printf late-value"}),
        );
        let clear = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            vault.clear(None);
        };
        let (result, ()) = tokio::join!(work, clear);
        assert!(result.is_err());
        assert!(vault.names().is_empty());
    }
}
