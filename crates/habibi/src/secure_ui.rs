//! Local-only private UI. Never send keystrokes, rendered values or clipboard errors
//! through the transcript/event/control channels.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use libhabibi::{
    CancellationToken,
    secure::{PrivateReply, PrivateRequest, Vault},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::{
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, task::JoinHandle};

pub struct Panel {
    vault: Vault,
    epoch: CancellationToken,
    browser: bool,
    selected: usize,
    dialog: Option<PrivateRequest>,
    input: String,
    revealed: bool,
    shown: bool,
    copied: bool,
    confirm: Option<char>,
    deadline: Option<Instant>,
    copy: Option<JoinHandle<bool>>,
    status: &'static str,
}
impl Panel {
    pub fn browser(vault: Vault) -> Self {
        Self {
            epoch: vault.epoch(),
            vault,
            browser: true,
            selected: 0,
            dialog: None,
            input: String::new(),
            revealed: false,
            shown: false,
            copied: false,
            confirm: None,
            deadline: None,
            copy: None,
            status: "",
        }
    }
    pub fn request(vault: Vault, request: PrivateRequest) -> Self {
        let mut panel = Self::browser(vault);
        panel.browser = false;
        panel.open(request);
        panel
    }
    pub fn local(vault: Vault, name: &str) -> Option<Self> {
        let value = vault.local_value(name)?;
        let (reply, _) = tokio::sync::oneshot::channel();
        let request = PrivateRequest {
            name: name.into(),
            reason: String::new(),
            value: Some(value),
            cancel: vault.epoch(),
            reply,
        };
        Some(Self::request(vault, request))
    }
    fn open(&mut self, request: PrivateRequest) {
        self.deadline = request
            .value
            .as_ref()
            .map(|_| Instant::now() + Duration::from_secs(30));
        self.dialog = Some(request);
        self.input.clear();
        self.revealed = false;
        self.shown = false;
        self.copied = false;
        self.confirm = None;
        self.status = "";
    }
    fn close_dialog(&mut self, submit: bool) {
        if let Some(job) = self.copy.take() {
            job.abort();
        }
        if let Some(request) = self.dialog.take() {
            let value = if submit && request.value.is_none() {
                Some(std::mem::take(&mut self.input))
            } else {
                None
            };
            let _ = request.reply.send(PrivateReply {
                value,
                shown: self.shown,
                copied: self.copied,
            });
        }
        self.input.clear();
        self.confirm = None;
        self.deadline = None;
        self.status = "";
    }
    /// True means dismiss the panel. The UI polls even while the model runs.
    pub fn poll(&mut self) -> bool {
        if self.epoch.is_cancelled() {
            self.close_dialog(false);
            return true;
        }
        if self
            .dialog
            .as_ref()
            .is_some_and(|d| d.cancel.is_cancelled())
            || self.deadline.is_some_and(|d| Instant::now() >= d)
        {
            self.close_dialog(false);
            return !self.browser;
        }
        !self.browser && self.dialog.is_none()
    }
    pub async fn copy_result(&mut self) {
        if self.copy.as_ref().is_some_and(|j| j.is_finished()) {
            let copied = self.copy.take().unwrap().await.unwrap_or(false);
            self.copied |= copied;
            self.status = if copied {
                "Copied. Clipboard/history is NOT automatically cleared."
            } else {
                "Clipboard unavailable or copy failed (no fallback)."
            };
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.dialog.as_ref().is_some_and(|d| d.value.is_none())
            && !text.contains(['\0', '\r', '\n'])
            && self.input.len() + text.len() <= 1_048_576
        {
            self.input.push_str(text);
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if self.poll() {
            return true;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.close_dialog(false);
            return true;
        }
        if let Some(request) = &self.dialog {
            if request.value.is_none() {
                match key.code {
                    KeyCode::Esc => {
                        self.close_dialog(false);
                        return !self.browser;
                    }
                    KeyCode::Enter if !self.input.is_empty() => {
                        self.close_dialog(true);
                        return !self.browser;
                    }
                    KeyCode::Backspace => {
                        self.input.pop();
                    }
                    KeyCode::Char('u') if ctrl => self.input.clear(),
                    KeyCode::Char(c)
                        if !c.is_control()
                            && !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        self.paste(&c.to_string())
                    }
                    _ => {}
                }
                return false;
            }
            if let Some(action) = self.confirm {
                match key.code {
                    KeyCode::Char('y' | 'Y') => {
                        self.confirm = None;
                        if action == 'r' {
                            self.revealed = true;
                            self.shown = true;
                        }
                        if action == 'c' && self.copy.is_none() {
                            let value = request.value.as_ref().unwrap().clone();
                            self.copy = Some(tokio::spawn(copy_clipboard(value)));
                            self.status = "Copying…";
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => self.confirm = None,
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => {
                        self.close_dialog(false);
                        return !self.browser;
                    }
                    KeyCode::Char('r' | 'R') => {
                        if self.revealed {
                            self.revealed = false;
                        } else {
                            self.confirm = Some('r');
                        }
                    }
                    KeyCode::Char('c' | 'C') if self.copy.is_none() => self.confirm = Some('c'),
                    _ => {}
                }
            }
        } else {
            let names = self.vault.names();
            self.selected = self.selected.min(names.len().saturating_sub(1));
            if self.confirm == Some('d') {
                match key.code {
                    KeyCode::Char('y' | 'Y') => {
                        if let Some(name) = names.get(self.selected) {
                            self.vault.clear(Some(name));
                            self.epoch = self.vault.epoch();
                        }
                        self.confirm = None;
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => self.confirm = None,
                    _ => {}
                }
                return false;
            }
            match key.code {
                KeyCode::Esc => return true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(names.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Delete | KeyCode::Char('d' | 'D') if !names.is_empty() => {
                    self.confirm = Some('d')
                }
                KeyCode::Enter | KeyCode::Char('r' | 'R') => {
                    if let Some(name) = names.get(self.selected)
                        && let Some(value) = self.vault.local_value(name)
                    {
                        let (reply, _) = tokio::sync::oneshot::channel();
                        self.open(PrivateRequest {
                            name: name.clone(),
                            reason: String::new(),
                            value: Some(value),
                            cancel: self.epoch.clone(),
                            reply,
                        });
                    }
                }
                _ => {}
            }
        }
        false
    }
    pub fn draw(&self, frame: &mut Frame) {
        let bounds = frame.area();
        let w = bounds.width.saturating_sub(2).min(78);
        let h = bounds.height.saturating_sub(2).min(22);
        let area = Rect::new((bounds.width - w) / 2, (bounds.height - h) / 2, w, h);
        let body = if let Some(request) = &self.dialog {
            let content = match &request.value {
                None => "*".repeat(self.input.chars().count().min(4096)),
                Some(value) if self.revealed => safe_display(value),
                Some(_) => "[masked]".into(),
            };
            let help = if request.value.is_none() {
                "Enter submit · Esc/Ctrl-C cancel · single line only"
            } else {
                "R reveal/hide · C copy · Enter/Esc close · auto-close 30s"
            };
            format!(
                "{}\n{}\n{}\n{}\n\n{}",
                safe_display(&request.name),
                help,
                self.status,
                safe_display(&request.reason),
                content
            )
        } else {
            let names = self.vault.names();
            let rows = usize::from(h.saturating_sub(7)).max(1);
            let start = self.selected.saturating_sub(rows - 1);
            let list = names
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, n)| format!("{} {}", if i == self.selected { "›" } else { " " }, n))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n\n↑/↓ or j/k select · Enter/R open · D delete · Esc close",
                if names.is_empty() {
                    "No secrets stored."
                } else {
                    &list
                }
            )
        };
        let confirmation = match self.confirm {
            Some('r') => "\nReveal on this terminal? Y confirm · N/Esc cancel",
            Some('c') => "\nCopy to system clipboard? Y confirm · N/Esc cancel",
            Some('d') => "\nForget selected secret? Y confirm · N/Esc cancel",
            _ => "",
        };
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!("{confirmation}\n{body}"))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Private secrets · local only ")
                        .border_style(Style::default().fg(Color::Cyan)),
                ),
            area,
        );
    }
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.close_dialog(false);
    }
}
fn safe_display(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars().take(2048) {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    if value.chars().count() > 2048 {
        out.push_str("… [display truncated]");
    }
    out
}
async fn copy_clipboard(value: String) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        tokio::process::Command::new("/usr/bin/pbcopy")
    } else if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tokio::process::Command::new("wl-copy")
    } else if cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_some() {
        // Select an installed X11 helper; never use a shell or OSC 52.
        let has_xclip = std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|p| p.join("xclip").is_file()));
        if has_xclip {
            let mut c = tokio::process::Command::new("xclip");
            c.args(["-selection", "clipboard"]);
            c
        } else {
            let mut c = tokio::process::Command::new("xsel");
            c.args(["--clipboard", "--input"]);
            c
        }
    } else {
        return false;
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let operation = async {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(value.as_bytes()).await.ok()?;
        drop(stdin);
        Some(child.wait().await.ok()?.success())
    };
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn request(
        value: Option<&str>,
    ) -> (PrivateRequest, tokio::sync::oneshot::Receiver<PrivateReply>) {
        let (reply, rx) = tokio::sync::oneshot::channel();
        (
            PrivateRequest {
                name: "TOKEN".into(),
                reason: "Synthetic test".into(),
                value: value.map(str::to_owned),
                cancel: CancellationToken::new(),
                reply,
            },
            rx,
        )
    }
    fn render(panel: &Panel) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 26)).unwrap();
        terminal.draw(|frame| panel.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
    }
    #[test]
    fn entry_masks_paste_and_rejects_multiline() {
        let (req, mut rx) = request(None);
        let mut panel = Panel::request(Vault::default(), req);
        panel.paste("synthetic-private");
        panel.paste("invalid\nline");
        let screen = render(&panel);
        assert!(!screen.contains("synthetic-private"));
        assert!(screen.contains("*****************"));
        panel.key(key(KeyCode::Backspace));
        panel.key(key(KeyCode::Char('e')));
        assert!(panel.key(key(KeyCode::Enter)));
        assert_eq!(
            rx.try_recv().unwrap().value.as_deref(),
            Some("synthetic-private")
        );
    }
    #[test]
    fn reveal_requires_confirmation_and_returns_only_status() {
        let (req, mut rx) = request(Some("synthetic-private"));
        let mut panel = Panel::request(Vault::default(), req);
        assert!(!render(&panel).contains("synthetic-private"));
        panel.key(key(KeyCode::Char('r')));
        assert!(!render(&panel).contains("synthetic-private"));
        assert!(render(&panel).contains("Y confirm"));
        panel.key(key(KeyCode::Esc));
        assert!(!panel.revealed);
        panel.key(key(KeyCode::Char('r')));
        panel.key(key(KeyCode::Char('y')));
        assert!(render(&panel).contains("synthetic-private"));
        panel.key(key(KeyCode::Char('r')));
        assert!(!render(&panel).contains("synthetic-private"));
        panel.key(key(KeyCode::Char('c')));
        assert_eq!(panel.confirm, Some('c'));
        assert!(panel.copy.is_none());
        panel.key(key(KeyCode::Esc));
        assert!(panel.copy.is_none());
        assert!(panel.key(key(KeyCode::Enter)));
        let reply = rx.try_recv().unwrap();
        assert!(reply.shown);
        assert!(!reply.copied);
        assert!(reply.value.is_none());
    }
    #[test]
    fn timeout_and_clear_close_even_during_confirmation() {
        let (req, mut rx) = request(Some("synthetic-private"));
        let vault = Vault::default();
        let mut panel = Panel::request(vault.clone(), req);
        panel.key(key(KeyCode::Char('r')));
        panel.deadline = Some(Instant::now() - Duration::from_secs(1));
        assert!(panel.poll());
        assert!(rx.try_recv().unwrap().value.is_none());
        let (req, mut rx) = request(None);
        let mut panel = Panel::request(vault.clone(), req);
        panel.paste("synthetic-private");
        vault.clear(None);
        assert!(panel.poll());
        assert!(rx.try_recv().unwrap().value.is_none());
    }
    #[test]
    fn display_escapes_controls_and_bounds_large_values() {
        assert_eq!(safe_display("a\x1b[31m\n\0"), "a\\u{1b}[31m\\n\\u{0}");
        assert!(safe_display(&"a".repeat(10000)).len() < 2100);
        let (req, _) = request(Some(&"a".repeat(10000)));
        let mut panel = Panel::request(Vault::default(), req);
        panel.key(key(KeyCode::Char('r')));
        panel.key(key(KeyCode::Char('y')));
        panel.key(key(KeyCode::Char('c')));
        assert!(render(&panel).contains("Copy to system clipboard?"));
    }
    #[test]
    fn cancelled_queued_request_never_accepts_input() {
        let (req, mut rx) = request(None);
        req.cancel.cancel();
        let mut panel = Panel::request(Vault::default(), req);
        panel.paste("synthetic-private");
        assert!(panel.key(key(KeyCode::Enter)));
        assert!(rx.try_recv().unwrap().value.is_none());
    }
}
