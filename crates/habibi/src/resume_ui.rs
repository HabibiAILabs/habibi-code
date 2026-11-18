//! Read-only session discovery. Opening the picker never locks or repairs journals.
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};
use tokio::sync::oneshot;

const WINDOW: u64 = 256 * 1024;
const MAX_SESSIONS: usize = 1000;
struct Entry {
    path: PathBuf,
    id: String,
    model: String,
    modified: SystemTime,
    first: String,
    last: String,
    partial: bool,
    current: bool,
}
fn snippet(text: &str) -> String {
    let mut value: String = crate::tui::visible(text).chars().take(3000).collect();
    if text.chars().count() > 3000 {
        value.push_str("\n[preview truncated]");
    }
    value
}
fn messages(bytes: &[u8]) -> impl Iterator<Item = Value> + '_ {
    // Ignore incomplete records at either window boundary, including a torn tail.
    bytes.split_inclusive(|b| *b == b'\n').filter_map(|line| {
        if line.last() != Some(&b'\n') {
            return None;
        }
        let value: Value = serde_json::from_slice(line).ok()?;
        (value["record"] == "message").then_some(value)
    })
}
fn read_entry(path: PathBuf, workspace: &Path, current: &Path) -> Result<Option<Entry>> {
    // Normalize both sides, including macOS /var -> /private/var aliases.
    let workspace = workspace.canonicalize()?;
    let current = current
        .canonicalize()
        .unwrap_or_else(|_| current.to_path_buf());
    let path = path.canonicalize()?;
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Not a journal file");
    let mut header = Vec::new();
    BufReader::new((&mut file).take(WINDOW)).read_until(b'\n', &mut header)?;
    ensure!(
        header.last() == Some(&b'\n'),
        "Incomplete or oversized header"
    );
    let header: Value = serde_json::from_slice(&header)?;
    ensure!(
        header["record"] == "header" && header["version"] == 1,
        "Unsupported header"
    );
    let saved = Path::new(
        header["config"]["workspace"]
            .as_str()
            .context("Missing workspace")?,
    );
    if saved.canonicalize().unwrap_or_else(|_| saved.to_path_buf()) != workspace {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut head = Vec::new();
    (&mut file).take(WINDOW).read_to_end(&mut head)?;
    let first = messages(&head)
        .find(|r| r["message"]["type"] == "user")
        .and_then(|r| r["message"]["text"].as_str().map(snippet))
        .unwrap_or_else(|| "No user message in preview window.".into());
    let mut tail = Vec::new();
    if metadata.len() > WINDOW {
        file.seek(SeekFrom::Start(metadata.len().saturating_sub(WINDOW)))?;
        (&mut file).take(WINDOW).read_to_end(&mut tail)?;
    }
    let last = messages(if tail.is_empty() { &head } else { &tail })
        .last()
        .map(|r| {
            let message = &r["message"];
            let role = message["type"].as_str().unwrap_or("message");
            let text = message["text"]
                .as_str()
                .or_else(|| message["output"].as_str())
                .unwrap_or("[Tool call; no text]");
            format!("{role}\n{}", snippet(text))
        })
        .unwrap_or_else(|| "No message in preview window.".into());
    Ok(Some(Entry {
        current: path == current,
        path,
        id: header["id"]
            .as_str()
            .context("Missing session ID")?
            .to_owned(),
        model: format!(
            "{} / {}",
            header["config"]["provider"].as_str().unwrap_or("?"),
            header["config"]["model"].as_str().unwrap_or("?")
        ),
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        first,
        last,
        partial: metadata.len() > WINDOW,
    }))
}
fn discover(
    dirs: &[PathBuf],
    workspace: &Path,
    current: &Path,
    cancelled: &AtomicBool,
) -> Result<(Vec<Entry>, usize)> {
    let workspace = workspace.canonicalize()?;
    let current = current
        .canonicalize()
        .unwrap_or_else(|_| current.to_path_buf());
    let mut paths = Vec::new();
    for dir in dirs {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok((Vec::new(), 0));
                    }
                    if entry.path().extension().is_some_and(|s| s == "jsonl")
                        && entry.file_type().is_ok_and(|t| t.is_file())
                    {
                        paths.push((
                            entry
                                .metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(SystemTime::UNIX_EPOCH),
                            entry.path(),
                        ));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    paths.sort_by(|a, b| b.cmp(a));
    paths.dedup_by(|a, b| a.1 == b.1);
    let mut skipped = paths.len().saturating_sub(MAX_SESSIONS);
    let mut entries = Vec::new();
    for (_, path) in paths.into_iter().take(MAX_SESSIONS) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        match read_entry(path, &workspace, &current) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(_) => skipped += 1,
        }
    }
    Ok((entries, skipped))
}

pub(crate) enum Action {
    None,
    Close,
    Open(PathBuf),
}
type ScanResult = Result<(Vec<Entry>, usize)>;
pub(crate) struct Picker {
    entries: Vec<Entry>,
    selected: usize,
    query: String,
    scroll: u16,
    workspace: PathBuf,
    loading: Option<oneshot::Receiver<ScanResult>>,
    cancelled: Arc<AtomicBool>,
    pub(crate) error: String,
    skipped: usize,
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
impl Picker {
    pub(crate) fn open(workspace: &Path, current: &Path) -> Result<Self> {
        let root = crate::base_dir("XDG_DATA_HOME", ".local/share")?.join("habibi/sessions");
        let mut dirs = vec![root];
        if let Some(parent) = current.parent() {
            dirs.push(parent.to_path_buf());
        }
        let mut dirs: Vec<_> = dirs
            .into_iter()
            .map(|p| p.canonicalize().unwrap_or(p))
            .collect();
        dirs.sort();
        dirs.dedup();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = oneshot::channel();
        let (workspace_copy, current, stop) = (
            workspace.to_path_buf(),
            current.to_path_buf(),
            cancelled.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(discover(&dirs, &workspace_copy, &current, &stop));
        });
        Ok(Self {
            entries: Vec::new(),
            selected: 0,
            query: String::new(),
            scroll: 0,
            workspace: workspace.to_path_buf(),
            loading: Some(rx),
            cancelled,
            error: String::new(),
            skipped: 0,
        })
    }
    pub(crate) fn poll(&mut self) -> bool {
        let Some(rx) = &mut self.loading else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok((entries, skipped))) => {
                self.entries = entries;
                self.skipped = skipped;
            }
            Ok(Err(e)) => self.error = format!("Cannot list sessions: {e:#}"),
            Err(oneshot::error::TryRecvError::Empty) => return false,
            Err(_) => self.error = "Session scan stopped.".into(),
        }
        self.loading = None;
        true
    }
    fn matches(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                format!("{} {} {}", e.id, e.model, e.first)
                    .to_lowercase()
                    .contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }
    pub(crate) fn step(&mut self, down: bool) {
        let len = self.matches().len();
        self.selected = if down {
            (self.selected + 1).min(len.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        self.scroll = 0;
    }
    pub(crate) fn key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Close;
            }
            KeyCode::Up => self.step(false),
            KeyCode::Down => self.step(true),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Enter => {
                if let Some(i) = self.matches().get(self.selected) {
                    let entry = &self.entries[*i];
                    return if entry.current {
                        Action::Close
                    } else {
                        Action::Open(entry.path.clone())
                    };
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.scroll = 0;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.query.len() < 256 =>
            {
                self.query.push(c);
                self.selected = 0;
                self.scroll = 0;
            }
            _ => {}
        }
        Action::None
    }
    pub(crate) fn draw(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        let rows = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(format!(
                "Folder: {}\nSearch: {}",
                crate::tui::visible(&self.workspace.display().to_string()),
                self.query
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Resume session · current folder "),
            ),
            rows[0],
        );
        let panels = if area.width >= 80 {
            Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
                .split(rows[1])
        } else {
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(rows[1])
        };
        let matches = self.matches();
        let items: Vec<_> = matches
            .iter()
            .map(|i| {
                let e = &self.entries[*i];
                let age = e.modified.elapsed().unwrap_or_default().as_secs() / 60;
                let title = e.first.lines().next().unwrap_or("Empty session");
                ListItem::new(vec![
                    Line::from(Span::styled(
                        format!("{}{}", if e.current { "● " } else { "  " }, title),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(format!("  {age}m ago · {}", crate::tui::visible(&e.model))),
                ])
            })
            .collect();
        let mut state =
            ListState::default().with_selected((!matches.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Sessions ({}) · newest first ", matches.len())),
                )
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White)),
            panels[0],
            &mut state,
        );
        let preview = if let Some(i) = matches.get(self.selected) {
            let e = &self.entries[*i];
            format!(
                "{}\n{}{}\n\nFIRST USER MESSAGE\n{}\n\nLAST MESSAGE\n{}\n\n{}",
                e.id,
                crate::tui::visible(&e.model),
                if e.current { " · CURRENT" } else { "" },
                e.first,
                e.last,
                if e.partial {
                    "Bounded preview: first/last 256 KiB only; some messages may be unavailable."
                } else {
                    "Preview only. Saved model and permissions are restored on resume."
                }
            )
        } else if self.loading.is_some() {
            "Loading sessions…".into()
        } else {
            "No sessions match this folder/search.".into()
        };
        let paragraph = Paragraph::new(preview).wrap(Wrap { trim: false });
        self.scroll = self.scroll.min(
            paragraph
                .line_count(panels[1].width.saturating_sub(2).max(1))
                .saturating_sub(panels[1].height.saturating_sub(2) as usize)
                .min(u16::MAX as usize) as u16,
        );
        frame.render_widget(
            paragraph
                .scroll((self.scroll, 0))
                .block(Block::default().borders(Borders::ALL).title(" Preview ")),
            panels[1],
        );
        let footer = if !self.error.is_empty() {
            crate::tui::visible(&self.error)
        } else {
            format!(
                "↑/↓ or wheel: select · type: search · PgUp/PgDn: preview · Enter: resume · Esc: close\n{} unreadable/unsupported/over-limit files skipped. Current session stays saved.",
                self.skipped
            )
        };
        frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), rows[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libhabibi::{Provider, Session, SessionConfig, auth::AuthStore};
    use serde_json::json;
    use std::io::Write;
    fn journal(root: &Path, workspace: &Path, name: &str) -> PathBuf {
        let mut config = SessionConfig::new(Provider::Codex, workspace);
        config.discover_skills = false;
        let mut session = Session::new(config, AuthStore::new(root.join("auth"))).unwrap();
        let path = root.join(format!("{name}.jsonl"));
        session.persist(&path).unwrap();
        drop(session);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for record in [
            json!({"record":"message","message":{"type":"user","text":"first request 🦀"}}),
            json!({"record":"compaction","summary":{"type":"user","text":"summary"}}),
            json!({"record":"message","message":{"type":"assistant","text":"last reply", "tool_calls":[],"native":[]}}),
        ] {
            writeln!(file, "{record}").unwrap();
        }
        path
    }
    #[test]
    fn discovery_filters_workspace_and_preserves_original_messages() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let a = journal(temp.path(), temp.path(), "a");
        journal(temp.path(), other.path(), "b");
        std::fs::write(temp.path().join("bad.jsonl"), b"garbage\n").unwrap();
        std::os::unix::fs::symlink(&a, temp.path().join("link.jsonl")).unwrap();
        let before = std::fs::read(&a).unwrap();
        let (entries, skipped) = discover(
            &[temp.path().to_path_buf()],
            temp.path(),
            &a,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(skipped, 1);
        assert!(entries[0].current);
        assert_eq!(entries[0].first, "first request 🦀");
        assert_eq!(entries[0].last, "assistant\nlast reply");
        assert_eq!(std::fs::read(&a).unwrap(), before);
        let auth = AuthStore::new(temp.path().join("auth"));
        let resumed = Session::resume(&a, auth.clone()).unwrap();
        assert!(
            Session::resume(&a, auth).is_err(),
            "active journal must remain locked"
        );
        // Browsing can still preview locked sessions and never modifies them.
        assert!(read_entry(a.clone(), temp.path(), &a).unwrap().is_some());
        drop(resumed);
    }
    #[test]
    fn workspace_and_current_session_match_through_directory_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let path = journal(temp.path(), temp.path(), "alias-test");
        let links = tempfile::tempdir().unwrap();
        let alias = links.path().join("workspace");
        std::os::unix::fs::symlink(temp.path(), &alias).unwrap();
        let alias_path = alias.join(path.file_name().unwrap());
        let entry = read_entry(alias_path.clone(), &alias, &path)
            .unwrap()
            .unwrap();
        assert!(entry.current);
        let (entries, _) = discover(
            std::slice::from_ref(&alias),
            &alias,
            &alias_path,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].current);
    }

    #[test]
    fn preview_windows_handle_large_records_torn_tail_and_controls() {
        let temp = tempfile::tempdir().unwrap();
        let path = journal(temp.path(), temp.path(), "large");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({"record":"tool_audit","message":{"output":"x".repeat(WINDOW as usize * 2)}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"record":"message","message":{"type":"user","text":"latest\u{1b}[31m"}})
        )
        .unwrap();
        write!(file, "{{\"record\":").unwrap();
        let e = read_entry(path.clone(), temp.path(), &path)
            .unwrap()
            .unwrap();
        assert!(e.partial);
        assert_eq!(e.first, "first request 🦀");
        assert_eq!(e.last, "user\nlatest[31m");
    }
    #[tokio::test]
    async fn picker_search_navigation_and_responsive_rendering() {
        let temp = tempfile::tempdir().unwrap();
        let path = journal(temp.path(), temp.path(), "picker");
        let entry = read_entry(path.clone(), temp.path(), &path)
            .unwrap()
            .unwrap();
        let mut picker = Picker {
            entries: vec![entry],
            selected: 0,
            query: String::new(),
            scroll: 0,
            workspace: temp.path().to_path_buf(),
            loading: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            error: String::new(),
            skipped: 0,
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(matches!(picker.key(key(KeyCode::Enter)), Action::Close));
        picker.entries[0].current = false;
        assert!(
            matches!(picker.key(key(KeyCode::Enter)), Action::Open(p) if p == path.canonicalize().unwrap())
        );
        picker.key(key(KeyCode::Char('z')));
        assert!(picker.matches().is_empty());
        assert!(matches!(picker.key(key(KeyCode::Enter)), Action::None));
        picker.key(key(KeyCode::Backspace));
        picker.key(key(KeyCode::Down));
        assert_eq!(picker.selected, 0);
        for (width, height) in [(120, 30), (60, 20), (12, 4), (1, 1)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| picker.draw(f)).unwrap();
        }
        assert!(matches!(picker.key(key(KeyCode::Esc)), Action::Close));
    }
}
