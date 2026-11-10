use anyhow::{Context, Result, bail, ensure};
use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use std::io::{self, IsTerminal, Write};

struct PrivateInput;
impl Drop for PrivateInput {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
        eprintln!();
    }
}

/// Never echo the callback or read it through ordinary line-buffered terminal input.
/// Dropping this future (local callback, timeout, or cancellation) restores the terminal.
pub async fn read_callback() -> Result<String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return std::future::pending().await;
    }
    enable_raw_mode()?;
    let _private = PrivateInput;
    execute!(io::stdout(), EnableBracketedPaste)?;
    eprint!(
        "Remote browser? Paste the full callback URL (or code#state), then Enter. Input hidden: "
    );
    io::stderr().flush()?;
    let mut events = EventStream::new();
    let mut input = String::new();
    loop {
        match events.next().await.context("Login input closed")?? {
            Event::Paste(text) => {
                ensure!(
                    input.len() + text.len() <= 8192,
                    "Callback input exceeds 8 KiB"
                );
                input.push_str(&text);
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => bail!("Login cancelled"),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    bail!("Login cancelled")
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear()
                }
                KeyCode::Enter if !input.trim().is_empty() => return Ok(input),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    ensure!(
                        input.len() + c.len_utf8() <= 8192,
                        "Callback input exceeds 8 KiB"
                    );
                    input.push(c);
                }
                _ => {}
            },
            _ => {}
        }
    }
}
