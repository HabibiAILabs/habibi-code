//! Experimental account OAuth adapters. Credentials never pass through terminal events.
use crate::Provider;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

const CODEX_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CLAUDE_CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Clone)]
pub struct AuthStore {
    root: PathBuf,
}

struct AuthLock(File);
impl Drop for AuthLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

// Deliberately no Debug: never accidentally format credentials into a log.
#[derive(Serialize, Deserialize)]
pub(crate) struct Credentials {
    pub access_token: String,
    refresh_token: String,
    expires_at: u64,
    pub account_id: Option<String>,
}

pub struct Login {
    provider: Provider,
    listener: TcpListener,
    url: Url,
    redirect: String,
    verifier: String,
    state: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn client_id(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => CODEX_CLIENT,
        Provider::Claude => CLAUDE_CLIENT,
        Provider::Ollama => unreachable!("Ollama does not use OAuth"),
    }
}
fn token_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "https://auth.openai.com/oauth/token",
        Provider::Claude => "https://platform.claude.com/v1/oauth/token",
        Provider::Ollama => unreachable!("Ollama does not use OAuth"),
    }
}

pub(crate) fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("habibi/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

impl AuthStore {
    /// No files or directories are created until authentication is used.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn path(&self, provider: Provider) -> PathBuf {
        self.root.join(format!("{}.json", provider.name()))
    }
    pub fn is_logged_in(&self, provider: Provider) -> bool {
        self.path(provider).is_file()
    }

    async fn lock(&self, provider: Provider) -> Result<AuthLock> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            private_dir(&root)?;
            let file = private_open(&root.join(format!("{}.lock", provider.name())), false)?;
            file.lock()?;
            Ok(AuthLock(file))
        })
        .await?
    }

    pub async fn logout(&self, provider: Provider) -> Result<()> {
        ensure!(
            provider != Provider::Ollama,
            "Ollama does not require login or logout"
        );
        let _lock = self.lock(provider).await?;
        match fs::remove_file(self.path(provider)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) async fn credentials(&self, provider: Provider) -> Result<Credentials> {
        ensure!(
            provider != Provider::Ollama,
            "Ollama does not use OAuth credentials"
        );
        // Serialize refresh-token rotation across Habibi processes.
        let _lock = self.lock(provider).await?;
        let path = self.path(provider);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options
            .open(path)
            .with_context(|| format!("Run `habibi login {}` first", provider.name()))?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= 64 * 1024,
            "Invalid credential file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                file.metadata()?.permissions().mode() & 0o077 == 0,
                "Credential file must not be accessible by group or others"
            );
        }
        let mut credentials: Credentials = serde_json::from_reader(file)
            .map_err(|_| anyhow::anyhow!("Invalid credential file; log in again"))?;
        if credentials.expires_at <= now() + 120 {
            let payload = json!({
                "grant_type": "refresh_token", "client_id": client_id(provider),
                "refresh_token": credentials.refresh_token,
            });
            let response = token_request(provider, payload).await?;
            credentials = parse_tokens(provider, response, Some(&credentials.refresh_token))?;
            self.save(provider, &credentials)?;
        }
        Ok(credentials)
    }

    #[cfg(test)]
    pub(crate) fn seed_test_credentials(&self, provider: Provider) -> Result<()> {
        self.save(
            provider,
            &Credentials {
                access_token: "fake-test-token".into(),
                refresh_token: "fake-refresh".into(),
                expires_at: now() + 3600,
                account_id: Some("fake-account".into()),
            },
        )
    }

    fn save(&self, provider: Provider, credentials: &Credentials) -> Result<()> {
        private_dir(&self.root)?;
        let temporary = self.root.join(format!(".{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_open(&temporary, true)?;
            serde_json::to_writer(&mut file, credentials)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path(provider))?;
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

impl Login {
    /// Bind loopback before giving the caller an authorization URL.
    pub async fn begin(provider: Provider) -> Result<Self> {
        ensure!(
            provider != Provider::Ollama,
            "Ollama does not require login; start `ollama serve`"
        );
        let port = match provider {
            Provider::Codex => 1455,
            Provider::Claude => 53692,
            Provider::Ollama => unreachable!("checked above"),
        };
        let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => listener,
            Err(_) if provider == Provider::Claude => {
                TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?
            }
            Err(e) => return Err(e).context("OAuth callback port 1455 is occupied"),
        };
        let path = match provider {
            Provider::Codex => "/auth/callback",
            Provider::Claude => "/callback",
            Provider::Ollama => unreachable!("checked above"),
        };
        let redirect = format!("http://localhost:{}{path}", listener.local_addr()?.port());
        let verifier = random();
        let state = random();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let authorize = match provider {
            Provider::Codex => "https://auth.openai.com/oauth/authorize",
            Provider::Claude => "https://claude.ai/oauth/authorize",
            Provider::Ollama => unreachable!("checked above"),
        };
        let scope = match provider {
            Provider::Ollama => unreachable!("checked above"),
            Provider::Codex => "openid profile email offline_access",
            Provider::Claude => {
                "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
            }
        };
        let mut url = Url::parse(authorize)?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", client_id(provider)),
            ("redirect_uri", &redirect),
            ("scope", scope),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ]);
        match provider {
            Provider::Ollama => unreachable!("checked above"),
            Provider::Codex => {
                url.query_pairs_mut().extend_pairs([
                    ("id_token_add_organizations", "true"),
                    ("codex_cli_simplified_flow", "true"),
                    ("originator", "habibi"),
                ]);
            }
            Provider::Claude => {
                url.query_pairs_mut().append_pair("code", "true");
            }
        }
        Ok(Self {
            provider,
            listener,
            url,
            redirect,
            verifier,
            state,
        })
    }

    pub fn authorization_url(&self) -> &str {
        self.url.as_str()
    }

    /// Wait at most five minutes. OAuth codes and tokens are never returned to the UI.
    pub async fn complete(self, store: &AuthStore, cancel: CancellationToken) -> Result<()> {
        self.complete_with_callback(store, cancel, std::future::pending())
            .await
    }

    /// Race the loopback callback against host-provided private input. Accepts the full
    /// redirect URL or `code#state`; bare codes are rejected to retain state validation.
    /// The host must keep this input out of logs and ordinary chat/session history.
    pub async fn complete_with_callback(
        self,
        store: &AuthStore,
        cancel: CancellationToken,
        input: impl std::future::Future<Output = Result<String>>,
    ) -> Result<()> {
        let finish = async {
            let code = self.receive_code(input).await?;
            self.exchange_code(store, code).await
        };
        tokio::select! {
            _ = cancel.cancelled() => bail!("Login cancelled"),
            result = tokio::time::timeout(Duration::from_secs(300), finish) => {
                result.context("Login timed out")?
            }
        }
    }

    async fn receive_code(
        &self,
        input: impl std::future::Future<Output = Result<String>>,
    ) -> Result<String> {
        tokio::select! {
            code = self.wait_for_callback() => code,
            input = input => pasted_callback_code(&input?, &self.redirect, &self.state),
        }
    }

    async fn wait_for_callback(&self) -> Result<String> {
        let expected_path = Url::parse(&self.redirect)?.path().to_owned();
        loop {
            let (mut socket, _) = self.listener.accept().await?;
            let mut request = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(3), async {
                while request.len() < 8192 && !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut bytes = [0u8; 512];
                    let n = socket.read(&mut bytes).await?;
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&bytes[..n]);
                }
                Ok::<_, std::io::Error>(())
            })
            .await;
            if !matches!(read, Ok(Ok(()))) {
                continue;
            }
            let request = String::from_utf8_lossy(&request);
            let code = callback_code(&request, &expected_path, &self.state);
            let response = if code.is_ok() {
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nCallback received. Return to Habibi."
            } else {
                "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nInvalid OAuth callback."
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                socket.write_all(response.as_bytes()),
            )
            .await;
            if let Ok(code) = code {
                return Ok(code);
            }
        }
    }

    async fn exchange_code(&self, store: &AuthStore, code: String) -> Result<()> {
        let mut payload = json!({
            "grant_type": "authorization_code", "client_id": client_id(self.provider),
            "code": code, "code_verifier": self.verifier, "redirect_uri": self.redirect,
        });
        if self.provider == Provider::Claude {
            payload["state"] = json!(self.state);
        }
        let tokens = parse_tokens(
            self.provider,
            token_request(self.provider, payload).await?,
            None,
        )?;
        let _lock = store.lock(self.provider).await?;
        store.save(self.provider, &tokens)
    }
}

fn callback_code(request: &str, expected_path: &str, expected_state: &str) -> Result<String> {
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    ensure!(parts.next() == Some("GET"), "Not a GET callback");
    let target = parts.next().context("Missing callback target")?;
    ensure!(
        target.starts_with('/') && !target.starts_with("//"),
        "Invalid callback target"
    );
    let url = Url::parse(&format!("http://localhost{target}"))?;
    ensure!(url.path() == expected_path, "Wrong callback path");
    code_from_url(&url, expected_state)
}

fn pasted_callback_code(input: &str, redirect: &str, expected_state: &str) -> Result<String> {
    let input = input.trim();
    ensure!(
        input.len() <= 8192 && !input.chars().any(|c| c.is_whitespace() || c.is_control()),
        "Invalid callback input"
    );
    if input.starts_with("http://") || input.starts_with("https://") {
        // Do not include URL parser errors: they may contain the authorization code.
        let url = Url::parse(input).map_err(|_| anyhow::anyhow!("Invalid callback URL"))?;
        let expected = Url::parse(redirect)?;
        ensure!(
            url.origin() == expected.origin()
                && url.path() == expected.path()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "Callback URL does not match this login's redirect address"
        );
        code_from_url(&url, expected_state)
    } else {
        let (code, state) = input.split_once('#').context(
            "Paste the full callback URL or code#state; a bare code cannot verify OAuth state",
        )?;
        ensure!(
            !code.is_empty() && !state.is_empty() && !state.contains('#'),
            "Invalid code#state input"
        );
        ensure!(state == expected_state, "OAuth state mismatch");
        Ok(code.to_owned())
    }
}

fn code_from_url(url: &Url, expected_state: &str) -> Result<String> {
    let values: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = values.iter().filter(|(k, _)| k == "state").collect();
    ensure!(
        states.len() == 1 && states[0].1 == expected_state,
        "OAuth state mismatch"
    );
    let codes: Vec<_> = values.iter().filter(|(k, _)| k == "code").collect();
    ensure!(
        codes.len() == 1 && !codes[0].1.is_empty(),
        "Missing OAuth code"
    );
    Ok(codes[0].1.to_string())
}

async fn token_request(provider: Provider, payload: Value) -> Result<Value> {
    let request = http_client()?
        .post(token_url(provider))
        .timeout(Duration::from_secs(30));
    let request = match provider {
        Provider::Codex => request.form(
            &payload
                .as_object()
                .context("Invalid token request")?
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default()))
                .collect::<Vec<_>>(),
        ),
        Provider::Claude => request.json(&payload),
        Provider::Ollama => bail!("Ollama does not use OAuth"),
    };
    let response = request.send().await.context("OAuth request failed")?;
    ensure!(
        response.status().is_success(),
        "OAuth request rejected ({}); log in again if refreshing",
        response.status()
    );
    response.json().await.context("Invalid OAuth response")
}

fn parse_tokens(
    provider: Provider,
    response: Value,
    previous_refresh: Option<&str>,
) -> Result<Credentials> {
    let access_token = response["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("OAuth response missing access token")?
        .to_owned();
    let refresh_token = response["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or(previous_refresh)
        .context("OAuth response missing refresh token")?
        .to_owned();
    let expires = response["expires_in"]
        .as_u64()
        .filter(|n| *n > 0)
        .context("OAuth response missing expiration")?;
    let account_id = if provider == Provider::Codex {
        let payload = access_token
            .split('.')
            .nth(1)
            .context("Invalid account token")?;
        let claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .context("Invalid account claims")?,
        )?;
        // Routing metadata only, not locally trusted authorization claims.
        Some(
            claims["https://api.openai.com/auth"]["chatgpt_account_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("Token missing ChatGPT account ID")?
                .to_owned(),
        )
    } else {
        None
    };
    Ok(Credentials {
        access_token,
        refresh_token,
        expires_at: now().saturating_add(expires),
        account_id,
    })
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Storage path must be a directory, not a symlink"
    );
    Ok(())
}

pub(crate) fn private_open(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_callbacks_require_matching_state_and_redirect() {
        let redirect = "http://localhost:1455/auth/callback";
        assert_eq!(
            pasted_callback_code(
                "http://localhost:1455/auth/callback?code=a%2Bb&state=expected",
                redirect,
                "expected"
            )
            .unwrap(),
            "a+b"
        );
        assert_eq!(
            pasted_callback_code("  test-code#expected\n", redirect, "expected").unwrap(),
            "test-code"
        );
        for invalid in [
            "bare-code",
            "test-code#wrong",
            "#expected",
            "test-code#expected#extra",
            "http://localhost:1455/auth/callback?code=secret&state=wrong",
            "http://localhost:1455/auth/callback?code=secret&state=expected&state=expected",
            "http://localhost:1455/auth/callback?code=secret&code=other&state=expected",
            "http://localhost:1455/other?code=secret&state=expected",
            "http://localhost:1456/auth/callback?code=secret&state=expected",
            "https://evil.example/auth/callback?code=secret&state=expected",
            "http://user@localhost:1455/auth/callback?code=secret&state=expected",
            "http://localhost:1455/auth/callback?code=secret&state=expected#fragment",
            "test\ncode#expected",
        ] {
            let error = pasted_callback_code(invalid, redirect, "expected")
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
        }
    }

    async fn login_fixture() -> Login {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect = format!(
            "http://localhost:{}/callback",
            listener.local_addr().unwrap().port()
        );
        Login {
            provider: Provider::Claude,
            listener,
            url: Url::parse("https://example.invalid/authorize").unwrap(),
            redirect,
            verifier: "test-verifier".into(),
            state: "expected".into(),
        }
    }

    #[tokio::test]
    async fn manual_callback_can_complete_without_a_browser_connection() {
        let login = login_fixture().await;
        let input = format!("{}?code=test-code&state=expected", login.redirect);
        assert_eq!(
            login
                .receive_code(std::future::ready(Ok(input)))
                .await
                .unwrap(),
            "test-code"
        );
    }

    #[tokio::test]
    async fn loopback_still_completes_while_manual_input_is_pending() {
        let login = login_fixture().await;
        let address = login.listener.local_addr().unwrap();
        let browser = async {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(b"GET /callback?code=local-code&state=expected HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
        };
        let (code, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(login.receive_code(std::future::pending()), browser)
        })
        .await
        .unwrap();
        assert_eq!(code.unwrap(), "local-code");
    }

    #[test]
    fn callback_checks_path_state_and_duplicates() {
        assert_eq!(
            callback_code(
                "GET /callback?code=abc&state=expected HTTP/1.1\r\n",
                "/callback",
                "expected"
            )
            .unwrap(),
            "abc"
        );
        for target in [
            "/callback?code=x&state=wrong",
            "/other?code=x&state=expected",
            "/callback?code=x",
            "/callback?code=x&state=expected&state=expected",
            "/callback?code=x&code=y&state=expected",
        ] {
            assert!(
                callback_code(&format!("GET {target} HTTP/1.1"), "/callback", "expected").is_err()
            );
        }
    }
    #[test]
    fn pkce_matches_rfc7636() {
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(
                b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
            )),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
    #[test]
    fn credentials_are_private_and_atomic() {
        let temp = tempfile::tempdir().unwrap();
        let store = AuthStore::new(temp.path().join("auth"));
        let creds = Credentials {
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            expires_at: now() + 3600,
            account_id: None,
        };
        store.save(Provider::Claude, &creds).unwrap();
        assert!(store.is_logged_in(Provider::Claude));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path(Provider::Claude))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(parse_tokens(Provider::Claude, json!({"access_token":"x"}), None).is_err());
    }
}
