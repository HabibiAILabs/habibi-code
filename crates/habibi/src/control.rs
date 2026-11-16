//! Private, per-process request/reply transport. Disconnect never stops the agent.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    sync::{mpsc, oneshot},
    task::JoinSet,
};

pub struct Request {
    pub value: Value,
    pub reply: Reply,
}
pub struct Reply {
    sender: oneshot::Sender<Value>,
    delivered: oneshot::Receiver<()>,
}
impl Reply {
    pub async fn send(self, value: Value) {
        if self.sender.send(value).is_ok() {
            // Ensure shutdown replies reach the socket before transport teardown.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), self.delivered).await;
        }
    }
}
pub struct Control {
    pub path: PathBuf,
    pub requests: mpsc::Receiver<Request>,
    task: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}
impl Drop for Control {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Control {
    pub fn bind(session: &str, journal: &std::path::Path) -> Result<Self> {
        let root = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .prefix(&format!("habibi-{}-", std::process::id()))
            .tempdir_in(root)?;
        // Set restrictive permissions at directory creation, before binding.
        let path = directory.path().join("control.sock");
        let listener = UnixListener::bind(&path).context("Bind local control socket")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::write(
            directory.path().join("instance.json"),
            serde_json::to_vec(
                &json!({"pid":std::process::id(),"session_id":session,"journal":journal,"socket":path}),
            )?,
        )?;
        let (tx, requests) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if clients.len() < 16 => {
                        let Ok((socket, _)) = accepted else { break; };
                        let tx = tx.clone();
                        clients.spawn(async move {
                            let (read, mut write) = socket.into_split();
                            let mut read = BufReader::new(read);
                            loop {
                                let mut line = Vec::new();
                                let n = (&mut read).take(1024 * 1024 + 1).read_until(b'\n', &mut line).await?;
                                if n == 0 { break; }
                                ensure!(n <= 1024 * 1024, "Request exceeds 1 MiB");
                                let mut acknowledgement = None;
                                let response = match serde_json::from_slice::<Value>(&line) {
                                    Ok(value) => {
                                        let id = value.get("id").cloned().unwrap_or(Value::Null);
                                        if !id.is_string() && !id.is_number() {
                                            json!({"id":id,"error":{"message":"Request needs a string or numeric id"}})
                                        } else {
                                            let (sender, result) = oneshot::channel();
                                            let (ack, delivered) = oneshot::channel();
                                            acknowledgement = Some(ack);
                                            let reply = Reply { sender, delivered };
                                            if tx.send(Request { value, reply }).await.is_err() { break; }
                                            match result.await { Ok(value) => value, Err(_) => break }
                                        }
                                    }
                                    Err(e) => json!({"id":null,"error":{"message":e.to_string()}}),
                                };
                                let mut bytes = serde_json::to_vec(&response)?;
                                bytes.push(b'\n');
                                write.write_all(&bytes).await?;
                                if let Some(ack) = acknowledgement { let _ = ack.send(()); }
                            }
                            Ok::<_, anyhow::Error>(())
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Ok(Self {
            path,
            requests,
            task,
            _directory: directory,
        })
    }
}
pub fn response(id: Value, result: Result<Value>) -> Value {
    match result {
        Ok(result) => json!({"id":id,"result":result}),
        Err(error) => json!({"id":id,"error":{"message":format!("{error:#}")}}),
    }
}
pub fn text(params: &Value) -> Result<String> {
    let text = params["text"]
        .as_str()
        .context("params.text must be a string")?;
    ensure!(
        !text.trim().is_empty() && text.len() <= 524288,
        "Text must contain 1–524288 bytes"
    );
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn private_bounded_request_reply_and_cleanup() {
        let mut control = Control::bind("test", std::path::Path::new("journal")).unwrap();
        let path = control.path.clone();
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let client = tokio::spawn(async move {
            let mut socket = UnixStream::connect(path).await.unwrap();
            socket
                .write_all(b"bad json\n{\"id\":1,\"method\":\"status\"}\n")
                .await
                .unwrap();
            let mut lines = BufReader::new(socket).lines();
            let error: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert!(error["error"].is_object());
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["id"], 1);
        });
        let request = control.requests.recv().await.unwrap();
        assert_eq!(request.value["method"], "status");
        request
            .reply
            .send(response(json!(1), Ok(json!({"busy":false}))))
            .await;
        client.await.unwrap();
        // Disconnect does not close the listener or the agent's request channel.
        let mut socket = UnixStream::connect(&control.path).await.unwrap();
        socket
            .write_all(&vec![b'x'; 1024 * 1024 + 1])
            .await
            .unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        let path = control.path.clone();
        drop(control);
        assert!(!path.exists());
    }
}
