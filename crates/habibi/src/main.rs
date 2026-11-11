mod defaults;
mod inspect;
mod login;
mod program_status;
mod tui;
mod update;
use anyhow::{Context, Result, bail, ensure};
use libhabibi::{
    CancellationToken, Provider, Session, SessionConfig,
    auth::{AuthStore, Login},
};
use std::{io::IsTerminal, path::PathBuf};

const HELP: &str = "habibi — a native Rust coding agent (experimental)\n\nUsage:\n  habibi [--provider codex|claude|ollama] [--model ID] [--cwd DIR] [--skill-dir DIR] [--no-sticky-skills]\n  habibi [--ollama-url http://127.0.0.1:11434]\n  habibi models [codex|claude|ollama] [--ollama-url URL]\n  habibi --resume SESSION.jsonl\n  habibi login codex|claude\n  habibi logout codex|claude\n\nSubscriptions use direct OAuth/model requests, never provider agent subprocesses.\nAdapters are experimental and not guaranteed provider-approved.\nTools ask by default; /settings offers explicit YOLO auto-approval. Shell commands are NOT sandboxed.\n\nTUI: Enter submit/steer; Alt-Enter follow-up; Ctrl-Enter/Ctrl-J newline; PgUp/PgDn scroll; Esc interrupt; Ctrl-C cancel or quit.\nCommands: /inspect, /compact, /models, /model, /settings, /update, /quit, /help. Subscriptions require login. Ollama requires a running server and an installed tool-capable model.\n";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        println!("habibi {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "models") {
        return list_models(&args[1..]).await;
    }
    let auth = AuthStore::new(base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/auth"));
    if args.first().is_some_and(|a| a == "login" || a == "logout") {
        ensure!(args.len() == 2, "Usage: habibi {} codex|claude", args[0]);
        let provider: Provider = args[1].parse()?;
        ensure!(
            provider != Provider::Ollama,
            "Ollama does not require login or logout; start `ollama serve`"
        );
        if args[0] == "logout" {
            auth.logout(provider).await?;
            println!("Logged out of {provider}");
            return Ok(());
        }
        eprintln!("Experimental direct subscription login for {provider}. No API-billed fallback.");
        if provider == Provider::Claude {
            eprintln!(
                "Claude access uses Claude Code-specific OAuth/request identity; third-party support is not established. Review provider terms before proceeding."
            );
        }
        let login = Login::begin(provider).await?;
        println!(
            "Open this URL in your browser:\n\n{}\n\nWaiting up to five minutes for the browser callback or a privately pasted callback URL. Ctrl-C to cancel.",
            login.authorization_url()
        );
        let mut status = program_status::Reporter::terminal();
        status.report(program_status::Status::Auth);
        let cancel = CancellationToken::new();
        let (result, cancelled) = tokio::select! {
            result=login.complete_with_callback(&auth,cancel,login::read_callback()) => {
                let cancelled = result.as_ref().err().is_some_and(|error| error.chain().any(|cause| cause.to_string() == "Login cancelled"));
                (result, cancelled)
            },
            result=tokio::signal::ctrl_c()=>{ (result.map_err(anyhow::Error::from).and_then(|()| Err(anyhow::anyhow!("Login cancelled"))), true) },
        };
        status.report(if cancelled {
            program_status::Status::Idle
        } else if result.is_ok() {
            program_status::Status::Done
        } else {
            program_status::Status::Error
        });
        result?;
        println!("Logged in to {provider}");
        return Ok(());
    }
    let options = Options::parse(&args)?;
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "The TUI requires a terminal. Use libhabibi for headless integration."
    );
    let (session, path) = if let Some(path) = options.resume.clone() {
        (Session::resume(&path, auth)?, path)
    } else {
        let defaults = defaults::Defaults::load(&defaults::path()?)?;
        let mut config = options.config(&defaults);
        if config.provider == Provider::Ollama
            && options.ollama_url.is_none()
            && let Ok(host) = std::env::var("OLLAMA_HOST")
        {
            config.ollama_url = host;
        }
        let mut session = Session::new(config, auth)?;
        let path = base_dir("XDG_DATA_HOME", ".local/share")?
            .join(format!("habibi/sessions/{}.jsonl", session.id()));
        session.persist(&path)?;
        (session, path)
    };
    tui::run(session, path).await
}

async fn list_models(args: &[String]) -> Result<()> {
    let mut args = args.iter();
    let provider = args.next().map(|s| s.parse::<Provider>()).transpose()?;
    let mut url = None;
    if let Some(flag) = args.next() {
        ensure!(
            flag == "--ollama-url",
            "Usage: habibi models [codex|claude|ollama] [--ollama-url URL]"
        );
        url = Some(
            args.next()
                .context("Missing value for --ollama-url")?
                .clone(),
        );
    }
    ensure!(args.next().is_none(), "Too many arguments for models");
    let providers = provider
        .map(|p| vec![p])
        .unwrap_or_else(|| vec![Provider::Codex, Provider::Claude]);
    for provider in providers {
        println!("{provider}:");
        if provider == Provider::Ollama {
            let defaults = defaults::Defaults::load(&defaults::path()?)?;
            let mut config = SessionConfig::new(provider, ".");
            defaults.apply(&mut config);
            let url = url
                .clone()
                .or_else(|| std::env::var("OLLAMA_HOST").ok())
                .unwrap_or(config.ollama_url);
            let models = libhabibi::ollama::Client::new(&url)?.models().await?;
            if models.is_empty() {
                println!(
                    "  No installed models. Install a tool-capable model with `ollama pull MODEL`."
                );
            }
            for model in models {
                println!("  {}", model.escape_debug());
            }
        } else {
            for model in provider.models() {
                println!("  {model}");
            }
            println!("  Availability depends on your subscription. Custom IDs: --model ID");
        }
    }
    Ok(())
}

fn base_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(value) = std::env::var_os(variable).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(value);
        ensure!(path.is_absolute(), "{variable} must be an absolute path");
        return Ok(path);
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is unset; configure XDG paths")?)
            .join(fallback),
    )
}

struct Options {
    sticky_skills: bool,
    skill_dirs: Vec<PathBuf>,
    provider: Provider,
    provider_explicit: bool,
    model: Option<String>,
    ollama_url: Option<String>,
    cwd: PathBuf,
    resume: Option<PathBuf>,
}
impl Options {
    fn config(&self, defaults: &defaults::Defaults) -> SessionConfig {
        let provider = if self.provider_explicit {
            self.provider
        } else {
            defaults.provider.unwrap_or(self.provider)
        };
        let mut config = SessionConfig::new(provider, self.cwd.clone());
        defaults.apply(&mut config);
        if let Some(model) = &self.model {
            config.model = model.clone();
        }
        if let Some(url) = &self.ollama_url {
            config.ollama_url = url.clone();
        }
        config.sticky_skills = self.sticky_skills;
        if !self.skill_dirs.is_empty() {
            config.skill_dirs = self.skill_dirs.clone();
        }
        config
    }
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            sticky_skills: true,
            skill_dirs: Vec::new(),
            provider: Provider::Codex,
            provider_explicit: false,
            model: None,
            ollama_url: None,
            cwd: std::env::current_dir()?,
            resume: None,
        };
        let mut changed = false;
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            if flag == "--sticky-skills" || flag == "--no-sticky-skills" {
                options.sticky_skills = flag == "--sticky-skills";
                changed = true;
                continue;
            }
            let value = match flag.as_str() {
                "--provider" | "--model" | "--cwd" | "--resume" | "--skill-dir"
                | "--ollama-url" => args
                    .next()
                    .with_context(|| format!("Missing value for {flag}"))?,
                _ => bail!("Unknown argument {flag}; use --help"),
            };
            match flag.as_str() {
                "--provider" => {
                    options.provider = value.parse()?;
                    options.provider_explicit = true;
                    changed = true;
                }
                "--ollama-url" => {
                    libhabibi::ollama::Client::new(value)?;
                    options.ollama_url = Some(value.clone());
                    changed = true;
                }
                "--model" => {
                    options.model = Some(value.clone());
                    changed = true;
                }
                "--cwd" => {
                    options.cwd = PathBuf::from(value);
                    changed = true;
                }
                "--skill-dir" => {
                    options.skill_dirs.push(PathBuf::from(value));
                    changed = true;
                }
                "--resume" => options.resume = Some(PathBuf::from(value)),
                _ => unreachable!(),
            }
        }
        ensure!(
            options.resume.is_none() || !changed,
            "--resume uses the saved provider, model, and workspace; do not combine overrides"
        );
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_defaults_and_explicit_provider_model_endpoint_precedence() {
        let mut defaults = defaults::Defaults::default();
        let mut saved = SessionConfig::new(Provider::Ollama, ".");
        saved.model = "installed:tag".into();
        saved.ollama_url = "http://localhost:12345".into();
        saved.preferences.yolo = true;
        defaults.remember(&saved, false);
        let options = Options::parse(&[]).unwrap();
        let config = options.config(&defaults);
        assert_eq!(config.provider, Provider::Ollama);
        assert_eq!(config.model, "installed:tag");
        assert_eq!(config.ollama_url, saved.ollama_url);
        assert!(config.preferences.yolo);
        let args = ["--provider", "claude", "--model", "custom-claude"].map(String::from);
        let config = Options::parse(&args).unwrap().config(&defaults);
        assert_eq!(config.provider, Provider::Claude);
        assert_eq!(config.model, "custom-claude");
        assert!(config.preferences.yolo);
        let args = ["--ollama-url", "http://localhost:54321"].map(String::from);
        let config = Options::parse(&args).unwrap().config(&defaults);
        assert_eq!(config.ollama_url, "http://localhost:54321");
        let args = [
            "--resume",
            "saved.jsonl",
            "--ollama-url",
            "http://localhost:54321",
        ]
        .map(String::from);
        assert!(Options::parse(&args).is_err());
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_arguments() {
        let parse =
            |s: &[&str]| Options::parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse(&["--skill-dir"]).is_err());
        assert!(parse(&["--resume", "session", "--sticky-skills"]).is_err());
        let skills = parse(&[
            "--sticky-skills",
            "--skill-dir",
            "one",
            "--skill-dir",
            "two",
        ])
        .unwrap();
        assert!(skills.sticky_skills);
        assert_eq!(
            skills.skill_dirs,
            vec![PathBuf::from("one"), PathBuf::from("two")]
        );
        assert!(parse(&["--provider"]).is_err());
        assert!(parse(&["--unknown", "x"]).is_err());
        assert!(parse(&["--resume", "session", "--model", "x"]).is_err());
        assert_eq!(
            parse(&["--provider", "claude"]).unwrap().provider,
            Provider::Claude
        );
    }
}
