mod inspect;
mod login;
mod tui;
mod update;
use anyhow::{Context, Result, bail, ensure};
use libhabibi::{
    CancellationToken, Provider, Session, SessionConfig,
    auth::{AuthStore, Login},
};
use std::{io::IsTerminal, path::PathBuf};

const HELP: &str = "habibi — a native Rust coding agent (experimental)\n\nUsage:\n  habibi [--provider codex|claude] [--model ID] [--cwd DIR] [--skill-dir DIR] [--no-sticky-skills]\n  habibi --resume SESSION.jsonl\n  habibi login codex|claude\n  habibi logout codex|claude\n\nSubscriptions use direct OAuth/model requests, never provider agent subprocesses.\nAdapters are experimental and not guaranteed provider-approved.\nTools ask by default; /settings offers explicit YOLO auto-approval. Shell commands are NOT sandboxed.\n\nTUI: Enter submit/steer; Alt-Enter follow-up; Ctrl-Enter/Ctrl-J newline; PgUp/PgDn scroll; Esc interrupt; Ctrl-C cancel or quit.\nCommands: /inspect, /compact, /model, /settings, /update, /quit, /help. Use login before requesting a model response.\n";

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
    let auth = AuthStore::new(base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/auth"));
    if args.first().is_some_and(|a| a == "login" || a == "logout") {
        ensure!(args.len() == 2, "Usage: habibi {} codex|claude", args[0]);
        let provider: Provider = args[1].parse()?;
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
        let cancel = CancellationToken::new();
        tokio::select! {
            result=login.complete_with_callback(&auth,cancel,login::read_callback())=>result?,
            result=tokio::signal::ctrl_c()=>{result?;bail!("Login cancelled")},
        }
        println!("Logged in to {provider}");
        return Ok(());
    }
    let options = Options::parse(&args)?;
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "The TUI requires a terminal. Use libhabibi for headless integration."
    );
    let (session, path) = if let Some(path) = options.resume {
        (Session::resume(&path, auth)?, path)
    } else {
        let mut config = SessionConfig::new(options.provider, options.cwd);
        if let Some(model) = options.model {
            config.model = model
        }
        config.sticky_skills = options.sticky_skills;
        if !options.skill_dirs.is_empty() {
            config.skill_dirs = options.skill_dirs;
        }
        let mut session = Session::new(config, auth)?;
        let path = base_dir("XDG_DATA_HOME", ".local/share")?
            .join(format!("habibi/sessions/{}.jsonl", session.id()));
        session.persist(&path)?;
        (session, path)
    };
    tui::run(session, path).await
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
    model: Option<String>,
    cwd: PathBuf,
    resume: Option<PathBuf>,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            sticky_skills: true,
            skill_dirs: Vec::new(),
            provider: Provider::Codex,
            model: None,
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
                "--provider" | "--model" | "--cwd" | "--resume" | "--skill-dir" => args
                    .next()
                    .with_context(|| format!("Missing value for {flag}"))?,
                _ => bail!("Unknown argument {flag}; use --help"),
            };
            match flag.as_str() {
                "--provider" => {
                    options.provider = value.parse()?;
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
