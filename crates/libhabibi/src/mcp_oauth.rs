//! Explicit MCP OAuth login and endpoint-bound private credential storage.
//! No browser, terminal, or process-global path discovery in the library.
use crate::{
    CancellationToken,
    auth::{private_dir, private_open},
};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    CredentialRefreshGuard, CredentialStore, StoredCredentials,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    key: String,
    issuer: Option<String>,
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
impl Store {
    /// Credentials are scoped to the canonical full MCP resource URL, not its
    /// display name or origin. Renaming a server cannot redirect its tokens.
    pub fn new(root: impl Into<PathBuf>, resource: &str) -> Result<Self> {
        crate::mcp::validate_url(resource)?;
        let url = Url::parse(resource)?;
        Ok(Self {
            root: root.into(),
            key: format!("{:x}", Sha256::digest(url.as_str().as_bytes())),
            issuer: None,
        })
    }
    fn path(&self) -> PathBuf {
        self.root.join(format!("{}.json", self.key))
    }
    fn directory(&self) -> Result<()> {
        private_dir(&self.root)?;
        check_private(&fs::metadata(&self.root)?)
    }
    async fn lock(&self) -> Result<Lock> {
        self.directory()?;
        let path = self.root.join(format!("{}.lock", self.key));
        // Nonblocking acquisition: cancellation never leaves a blocking worker
        // waiting forever for another process's browser login.
        loop {
            let file = private_open(&path, false)?;
            check_private(&file.metadata()?)?;
            ensure!(file.metadata()?.is_file(), "Invalid MCP credential lock");
            match file.try_lock() {
                Ok(()) => return Ok(Lock(file)),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn read(&self) -> Result<Option<StoredCredentials>> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir(),
                    "MCP credential root must be a directory, not a symlink"
                );
                check_private(&metadata)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(self.path()) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let metadata = file.metadata()?;
        check_private(&metadata)?;
        ensure!(
            metadata.is_file() && metadata.len() <= 128 * 1024,
            "Invalid MCP credential file"
        );
        Ok(Some(
            serde_json::from_reader(std::io::Read::take(file, 128 * 1024 + 1))
                .map_err(|_| anyhow::anyhow!("Invalid MCP credential file; log in again"))?,
        ))
    }
    fn write(&self, credentials: &StoredCredentials) -> Result<()> {
        let bytes = serde_json::to_vec(credentials)?;
        ensure!(
            bytes.len() <= 128 * 1024,
            "MCP credentials exceed storage limit"
        );
        self.directory()?;
        let temporary = self.root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_open(&temporary, true)?;
            std::io::Write::write_all(&mut file, &bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path())?;
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    fn remove(&self) -> Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => {
                File::open(&self.root)?.sync_all()?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn has_credentials(&self) -> Result<bool> {
        Ok(self.read()?.is_some_and(|c| c.token_response.is_some()))
    }
    /// Local deletion only, not provider-side revocation. Live clients consult
    /// this store for tokens; already in-flight requests may still complete.
    pub async fn logout(&self) -> Result<()> {
        let _lock = self.lock().await?;
        self.remove()
    }
    pub(crate) async fn manager(&self, resource: &str) -> Result<AuthorizationManager> {
        ensure!(
            Store::new(&self.root, resource)?.key == self.key,
            "OAuth store/resource mismatch"
        );
        let _lock = self.lock().await?;
        let (mut manager, issuer) = discover(resource).await?;
        let mut bound = self.clone();
        bound.issuer = Some(issuer);
        manager.set_credential_store(bound);
        ensure!(
            manager.initialize_from_store().await.map_err(safe_error)?,
            "MCP OAuth login required; run `habibi mcp login SERVER`"
        );
        Ok(manager)
    }
}
fn check_private(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.mode() & 0o077 == 0 && metadata.uid() == unsafe { libc::geteuid() },
            "MCP credential storage must be owned by this user and private (0700 directory / 0600 file)"
        );
    }
    Ok(())
}
fn store_error(_: anyhow::Error) -> AuthError {
    AuthError::CredentialStoreError(
        "Cannot read or update private MCP credentials; check storage permissions".into(),
    )
}
#[async_trait]
impl CredentialStore for Store {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let value = self.read().map_err(store_error)?;
        // Check every read, not just initialization: another process may log
        // into a different issuer while this manager is still alive.
        if let (Some(expected), Some(stored)) = (&self.issuer, &value)
            && stored.issuer.as_ref() != Some(expected)
        {
            return Err(AuthError::AuthorizationRequired);
        }
        Ok(value)
    }
    async fn save(&self, value: StoredCredentials) -> std::result::Result<(), AuthError> {
        self.write(&value).map_err(store_error)
    }
    async fn clear(&self) -> std::result::Result<(), AuthError> {
        self.remove().map_err(store_error)
    }
    async fn acquire_refresh_guard(
        &self,
    ) -> std::result::Result<Option<CredentialRefreshGuard>, AuthError> {
        Ok(Some(CredentialRefreshGuard::new(
            self.lock().await.map_err(store_error)?,
        )))
    }
}

// Never surface raw OAuth response bodies: a provider can echo credentials in
// its error response. These errors go to the human, not the model transcript.
fn safe_error(error: AuthError) -> anyhow::Error {
    let message = match error {
        AuthError::PkceUnsupported => "MCP authorization server must support PKCE S256",
        AuthError::AuthorizationRequired | AuthError::TokenRefreshRejected(_) => {
            "MCP OAuth login required; run `habibi mcp login SERVER`"
        }
        AuthError::RegistrationFailed(_) => {
            "MCP OAuth client registration failed; if dynamic registration is unavailable, supply --client-id for a pre-registered public client"
        }
        AuthError::TokenExchangeFailed(_) => "MCP OAuth token exchange failed; retry login",
        AuthError::TokenRefreshFailed(_) => {
            "MCP OAuth token refresh failed; retry later or log in again"
        }
        AuthError::AuthorizationServerMismatch { .. }
        | AuthError::AuthorizationServerMissingIssuer { .. } => {
            "MCP OAuth issuer validation failed"
        }
        AuthError::CredentialStoreError(_) => {
            "MCP OAuth credential storage failed; check private storage permissions"
        }
        _ => {
            "MCP OAuth failed; check server authorization metadata, connectivity and client configuration"
        }
    };
    anyhow::anyhow!(message)
}
async fn discover(resource: &str) -> Result<(AuthorizationManager, String)> {
    crate::mcp::validate_url(resource)?;
    // The SDK creates its own DNS-filtered reqwest client. Its
    // rustls-no-provider feature needs a default; preserve a host's choice.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let mut manager = AuthorizationManager::new(resource)
        .await
        .map_err(safe_error)?;
    // Probe only the explicitly configured resource, without credentials or
    // redirects. Some servers advertise a nonstandard metadata URL only here.
    let response = crate::mcp::http_client()?
        .get(resource)
        .header("Accept", "application/json, text/event-stream")
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Cannot contact MCP server for OAuth discovery"))?;
    let challenge = response
        .headers()
        .get("www-authenticate")
        .and_then(|h| h.to_str().ok())
        .filter(|h| h.len() <= 8192)
        .map(str::to_owned);
    drop(response);
    let resolution = manager
        .resolve_metadata_from_challenge(challenge.as_deref())
        .await
        .map_err(safe_error)?;
    ensure!(
        resolution.source.is_discovered(),
        "MCP server did not publish OAuth metadata"
    );
    ensure!(
        resolution.metadata.issuer.is_some(),
        "MCP OAuth metadata must identify its issuer"
    );
    let issuer = resolution
        .metadata
        .issuer
        .clone()
        .context("Missing OAuth issuer")?;
    manager.set_metadata(resolution.metadata);
    Ok((manager, issuer))
}

#[derive(Default)]
pub struct LoginOptions {
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    /// Zero binds an available loopback port; pre-registered clients may need a fixed one.
    pub callback_port: u16,
}
pub struct Login {
    session: AuthorizationSession,
    listener: TcpListener,
    store: Store,
    _lock: Lock,
    state: String,
    issuer: String,
}
impl Login {
    pub async fn begin(store: Store, resource: &str, options: LoginOptions) -> Result<Self> {
        ensure!(
            Store::new(&store.root, resource)?.key == store.key,
            "OAuth store/resource mismatch"
        );
        let lock = store.lock().await?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, options.callback_port))
            .await
            .context("Cannot bind MCP OAuth loopback callback")?;
        let redirect = format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr()?.port()
        );
        let (mut manager, issuer) =
            tokio::time::timeout(Duration::from_secs(90), discover(resource))
                .await
                .context("OAuth discovery timed out")??;
        let scopes = if options.scopes.is_empty() {
            manager.select_initial_scopes(&[]).await
        } else {
            options.scopes
        };
        let client_id = if let Some(id) = options.client_id {
            id
        } else {
            let registration = manager
                .register_client(
                    "Habibi",
                    &redirect,
                    &scopes.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .await
                .map_err(safe_error)?;
            ensure!(
                registration.client_secret.is_none(),
                "MCP OAuth requires a public client; server returned a confidential registration. Supply --client-id for a pre-registered public client"
            );
            registration.client_id
        };
        let request = AuthorizationRequest::new(redirect)
            .with_client_name("Habibi")
            .with_preregistered_client(client_id)
            .with_scopes(scopes);
        // Keep registration and tokens in the SDK's in-memory store until the
        // complete login succeeds. A denied/cancelled login keeps old tokens.
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| safe_error(e))?;
        let state = Url::parse(session.get_authorization_url())?
            .query_pairs()
            .find(|(key, _)| key == "state")
            .context("OAuth state missing")?
            .1
            .into_owned();
        Ok(Self {
            session,
            listener,
            store,
            _lock: lock,
            state,
            issuer,
        })
    }
    pub fn authorization_url(&self) -> &str {
        self.session.get_authorization_url()
    }
    pub async fn complete(
        self,
        cancel: CancellationToken,
        input: impl std::future::Future<Output = Result<String>>,
    ) -> Result<()> {
        let finish = async {
            let callback = tokio::select! {
                callback = self.receive() => callback?,
                input = input => validate_callback(&input?, &self.session.redirect_uri, &self.state)?,
            };
            ensure!(!callback.denied, "MCP OAuth authorization was denied");
            self.session
                .handle_callback_url(&callback.url)
                .await
                .map_err(safe_error)?;
            // Persist the original discovery issuer and receipt time alongside
            // the SDK token so refresh expiry and issuer binding survive restarts.
            let (client_id, token) = self.session.get_credentials().await.map_err(safe_error)?;
            let token = token.context("OAuth token missing")?;
            let issuer = self.issuer.clone();
            use rmcp::transport::auth::OAuthTokenResponse;
            let _: &OAuthTokenResponse = &token;
            let scopes = self.session.auth_manager.get_current_scopes().await;
            let received = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            self.store.write(
                &StoredCredentials::new(client_id, Some(token), scopes, Some(received))
                    .with_issuer(Some(issuer)),
            )
        };
        tokio::select! {
            _ = cancel.cancelled() => bail!("Login cancelled"),
            result = tokio::time::timeout(Duration::from_secs(300), finish) => result.context("MCP OAuth login timed out")?,
        }
    }
    async fn receive(&self) -> Result<Callback> {
        loop {
            let (mut socket, _) = self.listener.accept().await?;
            let mut bytes = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(3), async {
                while bytes.len() < 8192 && !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut buf = [0; 512];
                    let n = socket.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                }
                Ok::<_, std::io::Error>(())
            })
            .await;
            if !matches!(read, Ok(Ok(()))) {
                continue;
            }
            let callback = (|| -> Result<Callback> {
                ensure!(bytes.len() <= 8192, "Callback too large");
                let request = std::str::from_utf8(&bytes)?;
                let mut parts = request
                    .lines()
                    .next()
                    .context("Empty callback")?
                    .split_whitespace();
                ensure!(parts.next() == Some("GET"), "Callback must be GET");
                let target = parts.next().context("Missing callback target")?;
                ensure!(target.starts_with("/callback?"), "Wrong callback path");
                let mut url = Url::parse(&self.session.redirect_uri)?;
                url.set_query(Some(target.split_once('?').unwrap().1));
                validate_callback(url.as_str(), &self.session.redirect_uri, &self.state)
            })();
            let response = if callback.is_ok() {
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nCallback received. Return to Habibi."
            } else {
                "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nInvalid OAuth callback."
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                socket.write_all(response.as_bytes()),
            )
            .await;
            if let Ok(callback) = callback {
                return Ok(callback);
            }
        }
    }
}
struct Callback {
    url: String,
    denied: bool,
}
fn validate_callback(raw: &str, redirect: &str, state: &str) -> Result<Callback> {
    let raw = raw.trim();
    ensure!(raw.len() <= 8192, "Invalid OAuth callback");
    let url = Url::parse(raw).map_err(|_| anyhow::anyhow!("Paste the full callback URL"))?;
    let expected = Url::parse(redirect)?;
    ensure!(
        url.origin() == expected.origin()
            && url.path() == expected.path()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Wrong OAuth callback URL"
    );
    let mut params = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        ensure!(
            params
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate OAuth callback parameter"
        );
    }
    ensure!(
        params.get("state").is_some_and(|v| v == state),
        "OAuth state mismatch"
    );
    let denied = params.contains_key("error");
    ensure!(
        denied != params.contains_key("code"),
        "OAuth callback needs either code or error"
    );
    ensure!(
        denied || params.get("code").is_some_and(|v| !v.is_empty()),
        "Missing OAuth code"
    );
    Ok(Callback {
        url: url.to_string(),
        denied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn callbacks_reject_wrong_origin_state_duplicates_and_fragments() {
        let redirect = "http://127.0.0.1:1234/callback";
        assert!(validate_callback(&format!("{redirect}?code=ok&state=s"), redirect, "s").is_ok());
        assert!(
            validate_callback(
                &format!("{redirect}?error=access_denied&state=s"),
                redirect,
                "s"
            )
            .unwrap()
            .denied
        );
        for value in [
            format!("{redirect}?code=ok&state=wrong"),
            format!("{redirect}?code=ok&state=s&state=s"),
            format!("{redirect}?code=ok&code=bad&state=s"),
            format!("{redirect}?code=ok&state=s#fragment"),
            format!("{redirect}?code=ok&error=denied&state=s"),
            format!("{redirect}?code=&state=s"),
            "http://127.0.0.1:5678/callback?code=ok&state=s".into(),
            "http://evil.example/callback?code=ok&state=s".into(),
        ] {
            assert!(validate_callback(&value, redirect, "s").is_err(), "{value}");
        }
    }

    struct Server {
        url: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Server {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let origin = url.clone();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let (header_end, length) = loop {
                        let mut buf = [0; 4096];
                        let n = socket.read(&mut buf).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&bytes[..end]);
                            let length = header
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            break (end + 4, length);
                        }
                    };
                    while bytes.len() < header_end + length {
                        let mut buf = [0; 4096];
                        let n = socket.read(&mut buf).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    let header = String::from_utf8_lossy(&bytes[..header_end]);
                    let path = header.split_whitespace().nth(1).unwrap();
                    let body = String::from_utf8_lossy(&bytes[header_end..]).into_owned();
                    recorded.lock().unwrap().push((path.into(), body.clone()));
                    let (status, payload) = match path {
                        "/mcp" if header.starts_with("POST ") => {
                            assert!(
                                header
                                    .to_ascii_lowercase()
                                    .contains("authorization: bearer refreshed-access")
                            );
                            let message: serde_json::Value = serde_json::from_str(&body).unwrap();
                            if message.get("id").is_none() {
                                ("202 Accepted", json!({}))
                            } else {
                                let result = match message["method"].as_str().unwrap() {
                                    "initialize" => {
                                        json!({"protocolVersion":message["params"]["protocolVersion"], "capabilities":{"tools":{}}, "serverInfo":{"name":"oauth-fixture","version":"1"}})
                                    }
                                    "tools/list" => json!({"tools":[]}),
                                    method => panic!("Unexpected MCP method {method}"),
                                };
                                (
                                    "200 OK",
                                    json!({"jsonrpc":"2.0", "id":message["id"], "result":result}),
                                )
                            }
                        }
                        "/mcp" => ("401 Unauthorized", json!({})),
                        "/.well-known/oauth-protected-resource/mcp" => (
                            "200 OK",
                            json!({"resource":format!("{origin}/mcp"), "authorization_servers":[origin], "scopes_supported":["tools"]}),
                        ),
                        "/.well-known/oauth-authorization-server" => (
                            "200 OK",
                            json!({"issuer":origin, "authorization_endpoint":format!("{origin}/authorize"), "token_endpoint":format!("{origin}/token"), "registration_endpoint":format!("{origin}/register"), "response_types_supported":["code"], "code_challenge_methods_supported":["S256"]}),
                        ),
                        "/register" => {
                            let request: serde_json::Value = serde_json::from_str(&body).unwrap();
                            assert_eq!(request["token_endpoint_auth_method"], "none");
                            (
                                "200 OK",
                                json!({"client_id":"public-client", "redirect_uris":request["redirect_uris"]}),
                            )
                        }
                        "/token" => (
                            "200 OK",
                            json!({"access_token":if body.contains("grant_type=refresh_token") { "refreshed-access" } else { "initial-access" }, "token_type":"Bearer", "refresh_token":"rotated-refresh", "expires_in":3600}),
                        ),
                        _ => ("404 Not Found", json!({})),
                    };
                    let body = payload.to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            });
            Self {
                url,
                requests,
                task,
            }
        }
    }

    #[tokio::test]
    async fn discovery_pkce_callback_persistence_refresh_and_logout() {
        use base64::Engine;
        let server = Server::start().await;
        let resource = format!("{}/mcp", server.url);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("auth"), &resource).unwrap();
        let login = Login::begin(store.clone(), &resource, LoginOptions::default())
            .await
            .unwrap();
        let auth = Url::parse(login.authorization_url()).unwrap();
        let params: BTreeMap<_, _> = auth
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["resource"], resource);
        assert!(!store.has_credentials().unwrap());
        let callback = format!(
            "{}?code=sample-code&state={}",
            params["redirect_uri"], params["state"]
        );
        let callback_client = crate::mcp::http_client().unwrap();
        let (result, response) = tokio::join!(
            login.complete(CancellationToken::new(), std::future::pending()),
            callback_client.get(&callback).send()
        );
        result.unwrap();
        assert!(response.unwrap().status().is_success());
        assert!(store.has_credentials().unwrap());
        let token_request = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|(path, _)| path == "/token")
            .unwrap()
            .1
            .clone();
        let form: BTreeMap<_, _> = url::form_urlencoded::parse(token_request.as_bytes()).collect();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(form["code_verifier"].as_bytes()));
        assert_eq!(challenge, params["code_challenge"]);
        // A normal MCP connection must refresh an expired saved token and
        // attach the replacement bearer to initialize and discovery requests.
        let mut expired = store.read().unwrap().unwrap();
        expired.token_received_at = Some(0);
        store.write(&expired).unwrap();
        let mut session_config = crate::SessionConfig::new(crate::Provider::Ollama, dir.path());
        session_config.discover_skills = false;
        let mut session = crate::Session::new(
            session_config,
            crate::auth::AuthStore::new(dir.path().join("provider-auth")),
        )
        .unwrap();
        let config =
            serde_json::from_value(json!({"mcpServers":{"test":{"url":resource}}})).unwrap();
        assert_eq!(
            crate::mcp::connect_with_oauth_store(&mut session, config, Some(&store.root))
                .await
                .unwrap(),
            0
        );
        let manager = store.manager(&resource).await.unwrap();
        let saved = serde_json::to_value(store.read().unwrap().unwrap()).unwrap();
        assert_eq!(saved["issuer"], server.url);
        assert_eq!(saved["token_response"]["access_token"], "refreshed-access");
        assert!(
            Store::new(
                dir.path().join("auth"),
                &format!("{}/different", server.url)
            )
            .unwrap()
            .read()
            .unwrap()
            .is_none()
        );
        let mut changed = store.read().unwrap().unwrap();
        changed.issuer = Some("https://different-issuer.example".into());
        store.write(&changed).unwrap();
        assert!(manager.get_access_token().await.is_err());
        assert!(manager.refresh_token().await.is_err());
        store.logout().await.unwrap();
        assert!(!store.has_credentials().unwrap());
        assert!(manager.refresh_token().await.is_err());
    }

    #[tokio::test]
    async fn cancelled_and_denied_login_preserve_credentials_and_release_lock() {
        let server = Server::start().await;
        let resource = format!("{}/mcp", server.url);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("auth"), &resource).unwrap();
        let original = StoredCredentials::new("old".into(), None, vec![], None);
        store.write(&original).unwrap();
        let bytes = fs::read(store.path()).unwrap();
        let login = Login::begin(
            store.clone(),
            &resource,
            LoginOptions {
                client_id: Some("public".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let callback = format!(
            "{}?error=access_denied&state={}",
            login.session.redirect_uri, login.state
        );
        assert!(
            login
                .complete(CancellationToken::new(), async { Ok(callback) })
                .await
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        let login = Login::begin(
            store.clone(),
            &resource,
            LoginOptions {
                client_id: Some("public".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            login
                .complete(cancel, std::future::pending())
                .await
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        let lock = store.lock().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), store.lock())
                .await
                .is_err()
        );
        drop(lock);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), store.lock())
                .await
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_storage_rejects_symlinks_and_world_readable_credentials() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("auth"), "https://example.com/mcp").unwrap();
        store
            .write(&StoredCredentials::new("public".into(), None, vec![], None))
            .unwrap();
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.read().is_err());
        fs::remove_file(store.path()).unwrap();
        symlink(dir.path().join("target"), store.path()).unwrap();
        assert!(store.read().is_err());
    }
}
