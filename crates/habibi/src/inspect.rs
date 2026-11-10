//! Read-only, session-local context and tool-call inspector.
use crossterm::event::{KeyCode, KeyEvent};
use libhabibi::{Message, Session};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::collections::HashMap;

struct Entry {
    label: String,
    text: String,
    latest: bool,
}
pub(crate) struct Inspector {
    context: Vec<Entry>,
    calls: Vec<Entry>,
    tab: usize,
    all: bool,
    selected: [usize; 2],
    scroll: u16,
    query: String,
    searching: bool,
}
impl Inspector {
    pub(crate) fn new(session: &Session) -> Self {
        Self::from_snapshot(session.inspect_context(), session.tool_history())
    }
    fn from_snapshot(definitions: Vec<(String, String)>, history: &[Message]) -> Self {
        let boundary = history
            .iter()
            .rposition(|m| matches!(m, Message::User { .. }))
            .unwrap_or(0);
        let results: HashMap<_, _> = history
            .iter()
            .filter_map(|m| match m {
                Message::ToolResult {
                    call_id,
                    output,
                    is_error,
                } => Some((call_id.as_str(), (output.as_str(), *is_error))),
                _ => None,
            })
            .collect();
        let schemas = definitions
            .iter()
            .find(|(name, _)| name == "Tool definitions")
            .and_then(|(_, text)| serde_json::from_str::<serde_json::Value>(text).ok());
        let mut calls = Vec::new();
        for (index, message) in history.iter().enumerate() {
            if let Message::Assistant { tool_calls, .. } = message {
                for call in tool_calls {
                    let (status, output) = match results.get(call.id.as_str()) {
                        Some((output, true)) => ("ERROR / DENIED", *output),
                        Some((output, false)) => ("OK", *output),
                        None => ("PENDING / INTERRUPTED", "No result recorded."),
                    };
                    let args = serde_json::to_string_pretty(&call.arguments).unwrap_or_default();
                    let definition = schemas
                        .as_ref()
                        .and_then(|v| {
                            v.as_array()
                                .and_then(|a| a.iter().find(|v| v["name"] == call.name).cloned())
                        })
                        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                        .unwrap_or_default();
                    calls.push(Entry {
                        label: format!("{} [{}] {}", call.name, status, call.id),
                        text: format!("Tool: {}\nCall ID: {}\nStatus: {status}\n\nARGUMENTS\n{args}\n\nRESULT ({} bytes)\n{output}\n\nCURRENT TOOL DEFINITION\n{definition}", call.name, call.id, output.len()),
                        latest: index >= boundary,
                    });
                }
            }
        }
        calls.reverse(); // Most recent first.
        Self {
            context: definitions
                .into_iter()
                .map(|(label, text)| Entry {
                    label,
                    text,
                    latest: true,
                })
                .collect(),
            calls,
            tab: 0,
            all: false,
            selected: [0, 0],
            scroll: 0,
            query: String::new(),
            searching: false,
        }
    }
    fn entries(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        let source = if self.tab == 0 {
            &self.context
        } else {
            &self.calls
        };
        source
            .iter()
            .filter(|entry| {
                (self.tab == 0 || self.all || entry.latest)
                    && (query.is_empty()
                        || entry.label.to_lowercase().contains(&query)
                        || entry.text.to_lowercase().contains(&query))
            })
            .collect()
    }
    /// Returns true when the modal should close. Never approves or executes tools.
    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {
        if self.searching {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) if !c.is_control() && self.query.len() < 256 => self.query.push(c),
                _ => {}
            }
            self.selected[self.tab] = 0;
            self.scroll = 0;
            return false;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                self.tab = 1 - self.tab;
                self.scroll = 0;
                self.query.clear();
            }
            KeyCode::Char('1') | KeyCode::Char('2') => {
                self.tab = if key.code == KeyCode::Char('1') { 0 } else { 1 };
                self.scroll = 0;
                self.query.clear();
            }
            KeyCode::Char('a') if self.tab == 1 => {
                self.all = !self.all;
                self.selected[1] = 0;
                self.scroll = 0;
            }
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('c') => {
                self.query.clear();
                self.selected[self.tab] = 0;
                self.scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected[self.tab] = self.selected[self.tab].saturating_sub(1);
                self.scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected[self.tab] =
                    (self.selected[self.tab] + 1).min(self.entries().len().saturating_sub(1));
                self.scroll = 0;
            }
            KeyCode::Home => {
                self.selected[self.tab] = 0;
                self.scroll = 0;
            }
            KeyCode::End => {
                self.selected[self.tab] = self.entries().len().saturating_sub(1);
                self.scroll = 0;
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            _ => {}
        }
        false
    }
    pub(crate) fn scroll(&mut self, down: bool) {
        self.scroll = if down {
            self.scroll.saturating_add(3)
        } else {
            self.scroll.saturating_sub(3)
        };
    }
    pub(crate) fn draw(&mut self, frame: &mut ratatui::Frame) {
        let screen = frame.area();
        let area = Rect::new(
            screen.x + 1.min(screen.width / 2),
            screen.y + 1.min(screen.height / 2),
            screen.width.saturating_sub(2),
            screen.height.saturating_sub(2),
        );
        frame.render_widget(Clear, area);
        let title = if self.tab == 0 {
            " /inspect · [1 Context]   2 Tool calls "
        } else {
            " /inspect · 1 Context   [2 Tool calls] "
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);
        let scope = if self.tab == 0 {
            "Active context"
        } else if self.all {
            "All session tool calls"
        } else {
            "Tool calls since latest user message"
        };
        let header = format!(
            "{scope} · {} entries\n{} /{}",
            self.entries().len(),
            if self.searching { "Search:" } else { "Filter:" },
            crate::tui::visible(&self.query)
        );
        frame.render_widget(Paragraph::new(header), rows[0]);
        let columns = Layout::horizontal([Constraint::Percentage(33), Constraint::Percentage(67)])
            .split(rows[1]);
        let entries = self.entries();
        let selected = self.selected[self.tab].min(entries.len().saturating_sub(1));
        let items: Vec<_> = entries
            .iter()
            .map(|e| ListItem::new(crate::tui::visible(&e.label)))
            .collect();
        let mut state = ListState::default().with_selected(if entries.is_empty() {
            None
        } else {
            Some(selected)
        });
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(Style::default().fg(Color::Cyan))
                .highlight_symbol("▸ ")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Entries · ↑/↓ "),
                ),
            columns[0],
            &mut state,
        );
        let text = entries
            .get(selected)
            .map(|e| crate::tui::visible(&e.text))
            .unwrap_or_else(|| {
                "No matching entries. Press a for all calls, or c to clear search.".into()
            });
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(columns[1].width.saturating_sub(2).max(1))
            .saturating_sub(columns[1].height.saturating_sub(2) as usize);
        let scroll = self.scroll.min(max_scroll.min(u16::MAX as usize) as u16);
        frame.render_widget(
            paragraph.scroll((scroll, 0)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Details · PgUp/PgDn / mouse scroll "),
            ),
            columns[1],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(
                    "Tab/1/2 tabs · ↑/↓ j/k select · a latest/all calls · / search · c clear",
                ),
                Line::raw(
                    "PgUp/PgDn scroll details · Esc/q close · Read-only; byte counts, not tokens",
                ),
            ]),
            rows[2],
        );
        self.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use libhabibi::ToolCall;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn fixture() -> Inspector {
        let call = |id: &str| Message::Assistant {
            text: "not exposed".into(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: "read".into(),
                arguments: json!({"path": "hello.rs"}),
            }],
            native: vec![],
        };
        Inspector::from_snapshot(
            vec![("Overview".into(), "Budget usage: 50 bytes\n".repeat(100))],
            &[
                Message::User {
                    text: "first".into(),
                },
                call("old"),
                Message::ToolResult {
                    call_id: "old".into(),
                    output: "old output".into(),
                    is_error: false,
                },
                Message::User {
                    text: "latest".into(),
                },
                call("new"),
                Message::ToolResult {
                    call_id: "new".into(),
                    output: "Denied by user\u{1b}[2J".into(),
                    is_error: true,
                },
                call("pending"),
            ],
        )
    }
    fn key(inspector: &mut Inspector, code: KeyCode) -> bool {
        inspector.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    #[test]
    fn tabs_scope_search_and_results() {
        let mut inspector = fixture();
        assert_eq!(inspector.tab, 0);
        assert_eq!(inspector.entries().len(), 1);
        key(&mut inspector, KeyCode::Tab);
        assert_eq!(inspector.entries().len(), 2);
        assert!(inspector.entries()[0].text.contains("PENDING"));
        assert!(inspector.entries()[1].text.contains("ERROR / DENIED"));
        assert!(inspector.entries()[1].text.contains("hello.rs"));
        key(&mut inspector, KeyCode::Char('a'));
        assert_eq!(inspector.entries().len(), 3);
        key(&mut inspector, KeyCode::Char('/'));
        for c in "old output".chars() {
            key(&mut inspector, KeyCode::Char(c));
        }
        key(&mut inspector, KeyCode::Enter);
        assert_eq!(inspector.entries().len(), 1);
        assert!(inspector.entries()[0].text.contains("Status: OK"));
        key(&mut inspector, KeyCode::Char('c'));
        key(&mut inspector, KeyCode::End);
        assert_eq!(inspector.selected[1], 2);
        assert!(key(&mut inspector, KeyCode::Esc));
    }
    #[test]
    fn rendering_handles_small_screens_empty_search_and_controls() {
        let mut inspector = fixture();
        for size in [(100, 30), (36, 12), (4, 3), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            terminal.draw(|frame| inspector.draw(frame)).unwrap();
            key(&mut inspector, KeyCode::Tab);
            key(&mut inspector, KeyCode::Down);
            terminal.draw(|frame| inspector.draw(frame)).unwrap();
            key(&mut inspector, KeyCode::PageDown);
            terminal.draw(|frame| inspector.draw(frame)).unwrap();
            inspector.query = "no matches".into();
            terminal.draw(|frame| inspector.draw(frame)).unwrap();
            inspector.query.clear();
        }
    }
}
