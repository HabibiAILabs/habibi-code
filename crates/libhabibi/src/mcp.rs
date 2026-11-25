//! Explicit host-managed MCP client (stdio and Streamable HTTP). No repository discovery.
use crate::{Session, Tool, ToolContext, ToolDefinition, ToolFuture, ToolOutput};
use anyhow::{Context, Result, ensure};
use rmcp::{
    RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
    },
    service::RunningService,
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, process::Stdio, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "mcpServers")]
    pub servers: BTreeMap<String, Server>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Server {
    Stdio(StdioServer),
    Http(HttpServer),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpServer {
    #[serde(default)]
    pub disabled: bool,
    pub url: String,
    #[serde(default)]
    pub bearer_token_env: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioServer {
    #[serde(default)]
    pub disabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Child variable name -> parent environment variable name. No secret values
    /// are stored in configuration or session journals.
    #[serde(default)]
    pub env_from: BTreeMap<String, String>,
}

/// Connecting executes user-configured programs with the user's privileges.
/// The caller must obtain trust for the configuration before calling this.
/// Tool calls still require ordinary session approval. Connections live as long
/// as their registered tools; dropping them closes the SDK service/child process.
pub async fn connect(session: &mut Session, config: Config) -> Result<usize> {
    connect_with_oauth_store(session, config, None).await
}

/// Opt in to endpoint-bound OAuth credentials stored at a caller-owned path.
pub async fn connect_with_oauth_store(
    session: &mut Session,
    config: Config,
    oauth_root: Option<&Path>,
) -> Result<usize> {
    ensure!(
        config.servers.len() <= 32,
        "At most 32 MCP servers are supported"
    );
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut metadata_bytes = 0usize;
    for (server, config) in config.servers {
        if match &config {
            Server::Stdio(s) => s.disabled,
            Server::Http(s) => s.disabled,
        } {
            continue;
        }
        ensure!(
            !server.is_empty()
                && server.len() <= 24
                && server
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "MCP server names must contain 1–24 ASCII letters, digits or hyphens"
        );
        let service = tokio::time::timeout(
            Duration::from_secs(30),
            start(&config, &session.config().workspace, oauth_root),
        )
        .await
        .with_context(|| format!("MCP server {server}: initialization timed out after 30 seconds"))?
        .with_context(|| format!("MCP server {server}: connection failed"))?;
        let service = Arc::new(service);
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let discovered = tokio::time::timeout(Duration::from_secs(30), async {
            let mut discovered = Vec::new();
            loop {
                let page = service
                    .list_tools(cursor.clone().map(|cursor| {
                        rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
                    }))
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "MCP tool discovery failed; check connection and authentication"
                        )
                    })?;
                for tool in page.tools {
                    let definition = tool_definition(&server, &tool, &mut metadata_bytes)?;
                    discovered.push((tool.name.to_string(), definition));
                }
                ensure!(
                    discovered.len() <= 256,
                    "MCP server exposes more than 256 tools"
                );
                cursor = page.next_cursor;
                if let Some(next) = &cursor {
                    ensure!(
                        seen.insert(next.clone()),
                        "MCP repeated a pagination cursor"
                    );
                } else {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(discovered)
        })
        .await
        .context("MCP tool discovery timed out")??;
        for (remote_name, definition) in discovered {
            tools.push(Arc::new(McpTool {
                definition,
                remote_name,
                service: service.clone(),
            }));
        }
    }
    let count = tools.len();
    session.register_tools(tools)?;
    Ok(count)
}

// Large APIs can legitimately expose verbose descriptions and nested schemas.
// Bound both each complete definition and the catalog across all servers; never
// truncate schema fields or silently omit tools. Validate as pages arrive.
const MAX_TOOL_METADATA_BYTES: usize = 1024 * 1024;
const MAX_CATALOG_METADATA_BYTES: usize = 4 * 1024 * 1024;

fn tool_definition(
    server: &str,
    tool: &rmcp::model::Tool,
    total: &mut usize,
) -> Result<ToolDefinition> {
    let definition = ToolDefinition {
        name: format!("mcp__{server}__{}", tool.name),
        description: format!(
            "MCP server {server} (untrusted description): {}",
            tool.description.as_deref().unwrap_or("")
        ),
        schema: Value::Object((*tool.input_schema).clone()),
    };
    let bytes = serde_json::to_vec(&definition)?.len();
    // Names are untrusted too: bound and escape diagnostics, never print payloads.
    let label: String = tool.name.chars().take(128).collect();
    ensure!(
        bytes <= MAX_TOOL_METADATA_BYTES,
        "MCP server {server}: tool {label:?} metadata is too large: {bytes} bytes (limit {MAX_TOOL_METADATA_BYTES}); reduce the server's tool metadata"
    );
    let next = total.saturating_add(bytes);
    ensure!(
        next <= MAX_CATALOG_METADATA_BYTES,
        "MCP server {server}: tool {label:?} would exceed the total MCP metadata limit: {next} bytes (limit {MAX_CATALOG_METADATA_BYTES}); disable unused servers or reduce their exposed tools"
    );
    *total = next;
    Ok(definition)
}

// `serve` uses initialize, not the newer server/discover lifecycle. Do not let
// SDK upgrades silently advertise a protocol that removed that handshake.
fn client_config() -> ClientConfig {
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("habibi", env!("CARGO_PKG_VERSION")),
    )
    .with_protocol_version(ProtocolVersion::V_2025_11_25)
}

async fn start(
    config: &Server,
    workspace: &Path,
    oauth_root: Option<&Path>,
) -> Result<RunningService<RoleClient, ClientConfig>> {
    match config {
        Server::Stdio(config) => start_stdio(config, workspace).await,
        Server::Http(config) => {
            use rmcp::transport::{
                StreamableHttpClientTransport,
                streamable_http_client::StreamableHttpClientTransportConfig,
            };
            validate_url(&config.url)?;
            let client = http_client()?;
            let mut transport_config =
                StreamableHttpClientTransportConfig::with_uri(config.url.clone());
            if let Some(variable) = &config.bearer_token_env {
                let token = std::env::var(variable).with_context(|| {
                    format!("Missing MCP token environment variable {variable}")
                })?;
                ensure!(!token.is_empty(), "MCP bearer token is empty");
                transport_config = transport_config.auth_header(token);
            }
            if config.bearer_token_env.is_none()
                && let Some(root) = oauth_root
            {
                let store = crate::mcp_oauth::Store::new(root, &config.url)?;
                if store.has_credentials()? {
                    let manager = store.manager(&config.url).await?;
                    let client = rmcp::transport::auth::AuthClient::new(client, manager);
                    let transport =
                        StreamableHttpClientTransport::with_client(client, transport_config);
                    return client_config()
                        .serve(transport)
                        .await
                        .map_err(handshake_error);
                }
            }
            let transport = StreamableHttpClientTransport::with_client(client, transport_config);
            client_config()
                .serve(transport)
                .await
                .map_err(handshake_error)
        }
    }
}

// Never format raw SDK errors: server responses and URLs can contain credentials.
fn handshake_error(error: rmcp::service::ClientInitializeError) -> anyhow::Error {
    handshake_error_ref(&error)
}

fn handshake_error_ref(error: &rmcp::service::ClientInitializeError) -> anyhow::Error {
    use rmcp::service::ClientInitializeError as E;
    use rmcp::transport::streamable_http_client::{
        AuthRequiredError, HttpStatusError, InsufficientScopeError,
    };
    let detail = match error {
        E::LegacyFallbackFailed { fallback, .. } => return handshake_error_ref(fallback),
        E::ConnectionClosed(_) => "server closed the connection; check the server command and logs",
        E::NoCompatibleProtocolVersion { .. } => {
            "client and server have no compatible MCP protocol version"
        }
        E::JsonRpcError(_) => {
            "server rejected MCP initialization; check server compatibility and logs"
        }
        E::TransportError { error, .. } => {
            let mut source = Some(error.error.as_ref() as &(dyn std::error::Error + 'static));
            while let Some(current) = source {
                if current.is::<AuthRequiredError>() {
                    return anyhow::anyhow!(
                        "MCP handshake failed: authorization required; use /mcp Authenticate for this server"
                    );
                }
                if current.is::<InsufficientScopeError>() {
                    return anyhow::anyhow!(
                        "MCP handshake failed: insufficient permissions; check the account and granted scopes"
                    );
                }
                if let Some(status) = current.downcast_ref::<HttpStatusError>() {
                    return anyhow::anyhow!("MCP handshake failed: HTTP {}", status.status);
                }
                if let Some(request) = current.downcast_ref::<reqwest_mcp::Error>() {
                    if let Some(status) = request.status() {
                        return anyhow::anyhow!("MCP handshake failed: HTTP {}", status.as_u16());
                    }
                    if request.is_timeout() {
                        return anyhow::anyhow!("MCP handshake failed: network request timed out");
                    }
                    if request.is_connect() {
                        return anyhow::anyhow!(
                            "MCP handshake failed: network connection failed; check DNS, TLS and server availability"
                        );
                    }
                }
                source = current.source();
            }
            "transport failed; check endpoint, server availability and authentication"
        }
        E::Cancelled => "connection attempt cancelled",
        _ => "invalid MCP initialization response; check endpoint and server compatibility",
    };
    anyhow::anyhow!("MCP handshake failed: {detail}")
}

pub(crate) fn http_client() -> Result<reqwest_mcp::Client> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    reqwest_mcp::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest_mcp::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(Into::into)
}

pub(crate) fn validate_url(raw: &str) -> Result<()> {
    let url = url::Url::parse(raw)?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            || host == "[::1]"
    });
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "MCP requires HTTPS except for loopback HTTP"
    );
    ensure!(
        url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "MCP URL must not contain credentials, query or fragment; use bearer_token_env"
    );
    Ok(())
}

async fn start_stdio(
    config: &StdioServer,
    workspace: &Path,
) -> Result<RunningService<RoleClient, ClientConfig>> {
    ensure!(!config.command.is_empty(), "Empty MCP command");
    let mut command = tokio::process::Command::new(&config.command);
    command
        .args(&config.args)
        .current_dir(workspace)
        .env_clear()
        .kill_on_drop(true);
    for name in ["PATH", "HOME", "USER", "TMPDIR", "TEMP", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (target, source) in &config.env_from {
        command.env(
            target,
            std::env::var_os(source)
                .with_context(|| format!("Missing MCP environment variable {source}"))?,
        );
    }
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()?;
    client_config()
        .serve(transport)
        .await
        .map_err(handshake_error)
}

struct McpTool {
    definition: ToolDefinition,
    remote_name: String,
    service: Arc<RunningService<RoleClient, ClientConfig>>,
}
impl Tool for McpTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    fn execute<'a>(&'a self, arguments: Value, context: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let request = CallToolRequestParams::new(self.remote_name.clone()).with_arguments(
                arguments
                    .as_object()
                    .context("MCP arguments must be an object")?
                    .clone(),
            );
            let result = tokio::select! {
                biased;
                _ = context.cancel.cancelled() => anyhow::bail!("MCP call cancelled; remote side effects may have completed"),
                result = tokio::time::timeout(Duration::from_secs(120), self.service.call_tool(request)) => result.context("MCP call timed out; remote side effects may have completed")?.map_err(|_| anyhow::anyhow!("MCP tool request failed; check connection and authentication. Remote side effects may have completed"))?,
            };
            let is_error = result.is_error.unwrap_or(false);
            let value = serde_json::to_value(result)?;
            ensure!(
                value.to_string().len() <= 1024 * 1024,
                "MCP result exceeds 1 MiB; request a smaller result"
            );
            Ok(ToolOutput {
                images: Vec::new(),
                text: String::new(),
                structured: Some(value),
                is_error,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_urls_require_secure_transport_and_explicit_credentials() {
        for url in [
            "https://example.com/mcp",
            "http://127.0.0.1:3000/mcp",
            "http://[::1]/mcp",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com/mcp",
            "file:///tmp/mcp",
            "https://user:secret@example.com/mcp",
            "https://example.com/mcp?token=secret",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn metadata_supports_large_tools_without_truncation_and_bounds_catalog() {
        let schema = serde_json::json!({"type":"object", "description":"s".repeat(80 * 1024)});
        let tool: rmcp::model::Tool = serde_json::from_value(serde_json::json!({
            "name":"large", "description":"d".repeat(20 * 1024), "inputSchema":schema
        }))
        .unwrap();
        let mut total = 0;
        let definition = tool_definition("datadog", &tool, &mut total).unwrap();
        assert_eq!(definition.schema, schema);
        assert!(definition.description.ends_with(&"d".repeat(20 * 1024)));
        assert_eq!(total, serde_json::to_vec(&definition).unwrap().len());
        let bytes = total;
        total = MAX_CATALOG_METADATA_BYTES - bytes;
        tool_definition("datadog", &tool, &mut total).unwrap();
        assert_eq!(total, MAX_CATALOG_METADATA_BYTES);
        let error = tool_definition("other", &tool, &mut total)
            .unwrap_err()
            .to_string();
        assert!(error.contains("total MCP metadata limit"));
        assert!(error.contains("other"));
        assert_eq!(total, MAX_CATALOG_METADATA_BYTES);
        let mut boundary = tool.clone();
        boundary.description = Some(
            "d".repeat(20 * 1024 + MAX_TOOL_METADATA_BYTES - bytes)
                .into(),
        );
        total = 0;
        tool_definition("datadog", &boundary, &mut total).unwrap();
        assert_eq!(total, MAX_TOOL_METADATA_BYTES);
        boundary.description.as_mut().unwrap().to_mut().push('x');
        total = 0;
        assert!(tool_definition("datadog", &boundary, &mut total).is_err());
        assert_eq!(total, 0);
        let mut huge = tool;
        huge.description = Some("SECRET".repeat(MAX_TOOL_METADATA_BYTES / 6 + 1).into());
        total = 0;
        let error = tool_definition("datadog", &huge, &mut total)
            .unwrap_err()
            .to_string();
        assert!(error.contains("datadog"));
        assert!(error.contains("large"));
        assert!(error.contains("bytes (limit"));
        assert!(!error.contains("SECRET"));
        assert_eq!(total, 0);
    }

    #[test]
    fn handshake_diagnostics_do_not_expose_server_payloads() {
        use rmcp::{service::ClientInitializeError as E, transport::DynamicTransportError};
        let closed = handshake_error(E::ConnectionClosed("SECRET".into())).to_string();
        assert!(closed.contains("server closed"));
        assert!(!closed.contains("SECRET"));
        let transport = E::TransportError {
            error: DynamicTransportError::from_parts(
                "test",
                std::any::TypeId::of::<()>(),
                Box::new(
                    rmcp::transport::streamable_http_client::HttpStatusError::new(
                        503,
                        Some("SECRET".into()),
                    ),
                ),
            ),
            context: "SECRET".into(),
        };
        let message = handshake_error(transport).to_string();
        assert!(message.contains("HTTP 503"));
        assert!(!message.contains("SECRET"));
        assert!(!message.contains("Authenticate"));
        let fallback = E::LegacyFallbackFailed {
            discover: Box::new(E::Cancelled),
            fallback: Box::new(E::ConnectionClosed("SECRET".into())),
        };
        assert_eq!(handshake_error(fallback).to_string(), closed);
    }

    #[tokio::test]
    async fn disabled_servers_are_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::SessionConfig::new(crate::Provider::Ollama, dir.path());
        config.discover_skills = false;
        let mut session =
            Session::new(config, crate::auth::AuthStore::new(dir.path().join("auth"))).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({"mcpServers": {
            "local": {"command":"/nonexistent/never-execute", "disabled":true},
            "remote": {"url":"https://nonexistent.invalid/mcp", "disabled":true}
        }}))
        .unwrap();
        assert_eq!(connect(&mut session, config).await.unwrap(), 0);
        assert!(
            !session
                .tool_definitions()
                .iter()
                .any(|d| d.name.starts_with("mcp__"))
        );
    }

    #[tokio::test]
    async fn streamable_http_handshake_discovery_and_call() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let header_end = loop {
                        let mut buf = [0; 1024];
                        let count = stream.read(&mut buf).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..count]);
                        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            break index + 4;
                        }
                        assert!(bytes.len() < 16384);
                    };
                    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
                    if !headers.starts_with("post ") {
                        stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        return;
                    }
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    while bytes.len() < header_end + length {
                        let mut buf = [0; 1024];
                        let count = stream.read(&mut buf).await.unwrap();
                        assert!(count > 0);
                        bytes.extend_from_slice(&buf[..count]);
                    }
                    let message: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    if message.get("id").is_none() {
                        stream.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        return;
                    }
                    let result = match message["method"].as_str().unwrap() {
                        "initialize" => {
                            assert_eq!(message["params"]["protocolVersion"], "2025-11-25");
                            assert_eq!(message["params"]["clientInfo"]["name"], "habibi");
                            serde_json::json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"http-fixture","version":"1"}})
                        }
                        "tools/list" => serde_json::json!({"tools":[]}),
                        "tools/call" => {
                            serde_json::json!({"content":[{"type":"text","text":"remote ok"}],"isError":false})
                        }
                        method => panic!("Unexpected method {method}"),
                    };
                    let body =
                        serde_json::json!({"jsonrpc":"2.0","id":message["id"],"result":result})
                            .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        let service = tokio::time::timeout(
            Duration::from_secs(5),
            start(
                &Server::Http(HttpServer {
                    url: format!("http://{addr}/mcp"),
                    bearer_token_env: None,
                    disabled: false,
                }),
                Path::new("."),
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(service.list_tools(None).await.unwrap().tools.is_empty());
        let result = service
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap()["content"][0]["text"],
            "remote ok"
        );
        service.cancel().await.unwrap();
        server.abort();
    }
}
