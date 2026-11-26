//! OSC 7577 Program Resume Protocol (revision 0.5).
//! Only the TUI reports. Tokens never enter journals, diagnostics or child environments.
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Token(String);
impl Token {
    pub fn parse(value: Option<String>) -> Option<Self> {
        value
            .filter(|s| {
                (22..=64).contains(&s.len())
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .map(Self)
    }
}

// Positional arguments, not interpolated shell source: spaces, quotes, $(), etc.
// remain literal paths. cd failing must not start Habibi in another directory.
const SCRIPT: &str = "cd \"$1\" && exec habibi --resume \"$2\"";
const MAX_SEQUENCE_BYTES: usize = 2048;

fn signed(token: &Token, body: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(body.as_bytes());
    hash.update(b":");
    hash.update(token.0.as_bytes());
    format!("\x1b]7577;{body}:mac={:x}\x1b\\", hash.finalize())
}

fn resume_sequence(
    token: &Token,
    pid: u32,
    time: u64,
    cwd: &Path,
    journal: &Path,
) -> Option<String> {
    if !cwd.is_absolute() || !journal.is_absolute() {
        return None;
    }
    let argv = [
        "/bin/sh",
        "-c",
        SCRIPT,
        "habibi-resume",
        cwd.to_str()?,
        journal.to_str()?,
    ];
    if argv.iter().any(|arg| arg.chars().any(char::is_control)) {
        return None;
    }
    let args = argv
        .iter()
        .map(|arg| STANDARD.encode(arg))
        .collect::<Vec<_>>()
        .join(",");
    let sequence = signed(token, &format!("pid={pid}:time={time}:argv={args}"));
    (sequence.len() <= MAX_SEQUENCE_BYTES).then_some(sequence)
}

pub(crate) struct Reporter<W: Write> {
    writer: W,
    token: Option<Token>,
}
impl Reporter<io::Stdout> {
    pub fn terminal(token: Option<Token>) -> Self {
        let enabled = io::stdout().is_terminal()
            && std::env::var_os("HABIBI_NO_PROGRAM_RESUME").is_none_or(|v| v != "1");
        Self {
            writer: io::stdout(),
            token: token.filter(|_| enabled),
        }
    }
}
impl<W: Write> Reporter<W> {
    pub fn report(&mut self, cwd: &Path, journal: &Path) {
        let Some(token) = &self.token else { return };
        let Some(time) = now() else { return };
        let sequence = cwd
            .canonicalize()
            .ok()
            .zip(journal.canonicalize().ok())
            .and_then(|(cwd, journal)| {
                resume_sequence(token, std::process::id(), time, &cwd, &journal)
            })
            // Clear an older session if the new command cannot be represented safely.
            .unwrap_or_else(|| {
                signed(
                    token,
                    &format!("pid={}:time={time}:clear=1", std::process::id()),
                )
            });
        self.write(&sequence);
    }

    /// Explicit user quit only. Never clear in Drop, on terminal loss, or signals.
    pub fn clear(&mut self) {
        let Some(token) = &self.token else { return };
        let Some(time) = now() else { return };
        let sequence = signed(
            token,
            &format!("pid={}:time={time}:clear=1", std::process::id()),
        );
        self.write(&sequence);
    }

    fn write(&mut self, sequence: &str) {
        if self
            .writer
            .write_all(sequence.as_bytes())
            .and_then(|()| self.writer.flush())
            .is_err()
        {
            self.token = None;
        }
    }
}
fn now() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|t| t.as_secs())
        .filter(|t| *t > 0)
}

pub(crate) struct ContinueSignal {
    #[cfg(unix)]
    signal: Option<tokio::signal::unix::Signal>,
}
impl ContinueSignal {
    pub fn new() -> Self {
        Self {
            #[cfg(unix)]
            signal: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
                libc::SIGCONT,
            ))
            .ok(),
        }
    }
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        if let Some(signal) = &mut self.signal {
            signal.recv().await;
            return;
        }
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token() -> Token {
        Token::parse(Some("a".repeat(22))).unwrap()
    }

    #[test]
    fn validates_token_and_matches_protocol_hash_example() {
        for value in ["", "short", &"a".repeat(65), &"!".repeat(22)] {
            assert!(Token::parse(Some(value.into())).is_none());
        }
        // Published spec test vector deliberately uses a shorter example token.
        let token = Token("dummy-token-replace-me".into());
        assert_eq!(
            signed(&token, "pid=74877:time=1795543200:clear=1"),
            "\x1b]7577;pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93\x1b\\"
        );
    }

    #[test]
    fn paths_are_separate_arguments_and_invalid_commands_are_not_reported() {
        let cwd = Path::new("/tmp/project ' $(echo danger); space");
        let journal = Path::new("/tmp/session ' \" $.jsonl");
        let report = resume_sequence(&token(), 123, 456, cwd, journal).unwrap();
        let argv = report
            .split(":argv=")
            .nth(1)
            .unwrap()
            .split(":mac=")
            .next()
            .unwrap();
        let decoded: Vec<String> = argv
            .split(',')
            .map(|a| String::from_utf8(STANDARD.decode(a).unwrap()).unwrap())
            .collect();
        assert_eq!(
            decoded,
            [
                "/bin/sh",
                "-c",
                SCRIPT,
                "habibi-resume",
                cwd.to_str().unwrap(),
                journal.to_str().unwrap()
            ]
        );
        assert!(!report.contains(&"a".repeat(22)));
        for bad in [
            "relative",
            "/tmp/new\nline",
            "/tmp/\u{85}",
            &format!("/{}", "x".repeat(3000)),
        ] {
            assert!(resume_sequence(&token(), 123, 456, Path::new(bad), journal).is_none());
        }
    }

    #[test]
    fn reporter_updates_session_clears_invalid_target_and_is_disabled_without_token() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.jsonl");
        let two = dir.path().join("two.jsonl");
        std::fs::write(&one, "session").unwrap();
        std::fs::write(&two, "session").unwrap();
        let mut reporter = Reporter {
            writer: Vec::new(),
            token: Some(token()),
        };
        reporter.report(dir.path(), &one);
        reporter.report(dir.path(), &two);
        reporter.report(dir.path(), &dir.path().join("missing"));
        reporter.clear();
        let output = String::from_utf8(reporter.writer).unwrap();
        assert_eq!(output.matches("\x1b]7577;").count(), 4);
        assert_eq!(output.matches(":clear=1:").count(), 2);
        assert!(output.contains(&STANDARD.encode(one.canonicalize().unwrap().to_str().unwrap())));
        assert!(output.contains(&STANDARD.encode(two.canonicalize().unwrap().to_str().unwrap())));
        let mut disabled = Reporter {
            writer: Vec::new(),
            token: None,
        };
        disabled.report(dir.path(), &one);
        disabled.clear();
        assert!(disabled.writer.is_empty());
    }
}
