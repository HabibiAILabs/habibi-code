//! Best-effort release metadata check, never a provider request or auto-updater.
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LATEST: &str = "https://api.github.com/repos/habibiailabs/habibi-code/releases/latest";
const DAY: u64 = 24 * 60 * 60;

fn version(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let mut number = || {
        let part = parts.next()?;
        if part.is_empty()
            || !part.bytes().all(|c| c.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        part.parse::<u64>().ok()
    };
    let result = (number()?, number()?, number()?);
    if parts.next().is_some() {
        None
    } else {
        Some(result)
    }
}
fn newer(tag: &str, current: &str) -> bool {
    match (version(tag), version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn fetch_latest(endpoint: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("habibi/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let response = client
        .get(endpoint)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > 64 * 1024 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let release: Value = serde_json::from_slice(&body).ok()?;
    if release["draft"] != false || release["prerelease"] != false {
        return None;
    }
    let tag = release["tag_name"].as_str()?;
    version(tag)?;
    Some(tag.to_owned())
}

async fn cached(path: &Path) -> Option<Value> {
    if tokio::fs::metadata(path).await.ok()?.len() > 4096 {
        return None;
    }
    serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()
}
fn fresh(cache: &Value, time: u64) -> bool {
    cache["checked_at"]
        .as_u64()
        .is_some_and(|checked| time >= checked && time - checked < DAY)
}
async fn save_cache(path: &Path, latest: Option<&str>) {
    let Some(parent) = path.parent() else { return };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    let temporary = parent.join(format!(".update-{}.tmp", std::process::id()));
    let content = json!({"checked_at": now(), "latest": latest}).to_string();
    // Cache contains public release metadata only. Atomic replacement avoids torn JSON.
    let result = async {
        use tokio::io::AsyncWriteExt;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;
        tokio::fs::rename(&temporary, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temporary).await;
    }
}

pub async fn check(cache_path: Option<PathBuf>) -> Option<String> {
    if std::env::var_os("HABIBI_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    // Never hold up first paint or make --help/login/version perform a check.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let cache = if let Some(path) = &cache_path {
        cached(path).await
    } else {
        None
    };
    let latest = if let Some(cache) = cache.filter(|cache| fresh(cache, now())) {
        cache["latest"].as_str().map(str::to_owned)
    } else {
        let latest = fetch_latest(LATEST).await;
        if let Some(path) = &cache_path {
            save_cache(path, latest.as_deref()).await;
        }
        latest
    }?;
    newer(&latest, env!("CARGO_PKG_VERSION")).then_some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_versions_and_cache_age_are_checked() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("v0.10.0", "0.9.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
        for tag in [
            "v0.1.0-beta",
            "v01.2.3",
            "2.0",
            "2.0.0.1",
            "v2.0.0\n",
            "v2.0.0;evil",
            "999999999999999999999999.0.0",
        ] {
            assert!(!newer(tag, "0.1.0"));
        }
        assert!(fresh(&json!({"checked_at":100}), 101));
        assert!(!fresh(&json!({"checked_at":100}), 99));
        assert!(!fresh(&json!({"checked_at":100}), 100 + DAY));
    }
    #[tokio::test]
    async fn release_metadata_is_fetched_without_credentials() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let count = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]).to_lowercase();
            assert!(!request.contains("authorization:"));
            let body = r#"{"tag_name":"v0.2.0","draft":false,"prerelease":false}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        assert_eq!(
            fetch_latest(&format!("http://{address}")).await.as_deref(),
            Some("v0.2.0")
        );
        server.await.unwrap();
    }
}
