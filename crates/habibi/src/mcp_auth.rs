//! Explicit user-initiated MCP authentication; never called by model tools.
use anyhow::{Context, Result, bail, ensure};
use libhabibi::{
    CancellationToken,
    mcp_oauth::{Login, LoginOptions, Store},
};
use std::path::PathBuf;

pub fn root() -> Result<PathBuf> {
    Ok(crate::base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/mcp-auth"))
}
pub fn store(url: &str) -> Result<Store> {
    Store::new(root()?, url)
}

/// Browser opening is best effort. Always display the URL as a fallback.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if let Ok(mut child) = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

pub async fn cli(args: &[String]) -> Result<()> {
    ensure!(
        args.len() >= 2,
        "Usage: habibi mcp login|status|logout SERVER [--client-id ID] [--scope SCOPE] [--callback-port PORT]"
    );
    let action = &args[0];
    ensure!(
        matches!(action.as_str(), "login" | "logout" | "status"),
        "Expected login, logout or status"
    );
    let mut options = LoginOptions::default();
    let mut flags = args[2..].iter();
    while let Some(flag) = flags.next() {
        ensure!(action == "login", "Options are only valid for login");
        let value = flags.next().context("Missing option value")?;
        match flag.as_str() {
            "--client-id" => options.client_id = Some(value.clone()),
            "--scope" => options.scopes.push(value.clone()),
            "--callback-port" => {
                options.callback_port = value.parse().context("Invalid callback port")?
            }
            _ => bail!("Unknown MCP login option: {flag}"),
        }
    }
    let path = crate::base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/mcp.json");
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .context("Cannot read user MCP configuration")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "MCP configuration exceeds 1 MiB"
    );
    let config: libhabibi::mcp::Config = serde_json::from_slice(&bytes)?;
    let server = config.servers.get(&args[1]).context("Unknown MCP server")?;
    let libhabibi::mcp::Server::Http(server) = server else {
        bail!("OAuth requires an HTTP MCP server")
    };
    let store = store(&server.url)?;
    match action.as_str() {
        "status" => println!(
            "{}: {}{}",
            args[1],
            if store.has_credentials()? {
                "OAuth credentials saved (not a live validity check)"
            } else {
                "No saved OAuth credentials"
            },
            if server.bearer_token_env.is_some() {
                "; bearer environment variable takes precedence"
            } else {
                ""
            }
        ),
        "logout" => {
            store.logout().await?;
            println!(
                "Removed local OAuth credentials for {}. Provider grants are not revoked.",
                args[1]
            );
        }
        _ => {
            ensure!(
                server.bearer_token_env.is_none(),
                "Clear bearer_token_env to use OAuth"
            );
            tokio::select! {
                result = async {
                    let login = Login::begin(store, &server.url, options).await?;
                    println!("Open this URL in your browser:\n\n{}\n\nWaiting for callback. Paste the full callback URL if using a remote browser. Ctrl-C cancels.", login.authorization_url());
                    open_browser(login.authorization_url());
                    login.complete(CancellationToken::new(), crate::login::read_mcp_callback()).await
                } => result?,
                _ = tokio::signal::ctrl_c() => bail!("Login cancelled"),
            }
            println!("Logged in to {}", args[1]);
        }
    }
    Ok(())
}
