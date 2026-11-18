use anyhow::{Context, Result, bail, ensure};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, EnableMouseCapture,
        Event as TerminalEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use libhabibi::{CancellationToken, Event, Message, Preferences, Session, ToolCall};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use unicode_width::UnicodeWidthChar;

const TRANSCRIPT_LIMIT: usize = 256 * 1024;
type ResumeTarget = (Session, PathBuf, crate::control::Control);
type Job = JoinHandle<(Session, Result<()>, mpsc::Receiver<String>)>;

struct Screen;
impl Screen {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        Ok(guard)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            LeaveAlternateScreen,
            event::DisableMouseCapture,
            crossterm::cursor::Show
        );
    }
}

/// A draft: changes reach the session and user defaults only after explicit Apply.
#[derive(Clone)]
struct SettingsDialog {
    fields: Vec<String>,
    selected: usize,
    model_only: bool,
    default_model: String,
    model_choices: Vec<String>,
    provider: String,
    error: String,
    preferences: Preferences,
    confirm_yolo: bool,
    sticky_skills: bool,
}
impl SettingsDialog {
    fn from_config(c: &libhabibi::SessionConfig, model_only: bool) -> Self {
        Self {
            fields: vec![
                c.model.clone(),
                c.max_turns.to_string(),
                c.max_context_bytes.to_string(),
                c.auto_compact_percent.to_string(),
                serde_json::to_string(&c.skill_dirs).expect("skill paths"),
            ],
            selected: 0,
            model_only,
            default_model: c.provider.default_model().into(),
            model_choices: c.provider.models().iter().map(|s| (*s).into()).collect(),
            provider: c.provider.to_string(),
            error: String::new(),
            preferences: c.preferences.clone(),
            confirm_yolo: false,
            sticky_skills: c.sticky_skills,
        }
    }
    fn count(&self) -> usize {
        if self.model_only { 1 } else { 14 }
    }
    fn values(&self) -> Result<(String, usize, usize, u8)> {
        let values = (
            self.fields[0].clone(),
            self.fields[1]
                .parse()
                .context("Max turns must be a non-negative integer (0 = unlimited)")?,
            self.fields[2]
                .parse()
                .context("Context bytes must be a positive integer")?,
            self.fields[3]
                .parse()
                .context("Auto-compaction must be 0–100 (0 disables it)")?,
        );
        anyhow::ensure!(
            !values.0.trim().is_empty()
                && values.0.len() <= 256
                && !values.0.chars().any(char::is_control),
            "Model must be 1–256 characters without control characters"
        );
        anyhow::ensure!(values.2 > 0, "Context budget must be positive");
        anyhow::ensure!(values.3 <= 100, "Auto-compaction must be 0–100");
        Ok(values)
    }
    fn key(&mut self, key: KeyEvent) {
        self.error.clear();
        self.confirm_yolo = false;
        if self.selected == 0
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('n' | 'p'))
            && !self.model_choices.is_empty()
        {
            let next = key.code == KeyCode::Char('n');
            let index = self.model_choices.iter().position(|m| *m == self.fields[0]);
            let index = match index {
                Some(i) if next => (i + 1) % self.model_choices.len(),
                Some(i) => (i + self.model_choices.len() - 1) % self.model_choices.len(),
                None if next => 0,
                None => self.model_choices.len() - 1,
            };
            self.fields[0] = self.model_choices[index].clone();
            return;
        }
        if matches!(self.selected, 12 | 13)
            && matches!(
                key.code,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            )
        {
            let value = if self.selected == 12 {
                &mut self.preferences.mcp_enabled
            } else {
                &mut self.preferences.codemode_enabled
            };
            *value = !*value;
            return;
        }
        if self.selected == 10
            && matches!(
                key.code,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            )
        {
            self.sticky_skills = !self.sticky_skills;
            return;
        }
        if (3..9).contains(&self.selected)
            && matches!(
                key.code,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            )
        {
            let value = match self.selected {
                3 => &mut self.preferences.yolo,
                4 => &mut self.preferences.status_activity,
                5 => &mut self.preferences.status_model,
                6 => &mut self.preferences.status_ram,
                7 => &mut self.preferences.status_context,
                _ => &mut self.preferences.status_hints,
            };
            *value = !*value;
            return;
        }
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % self.count(),
            KeyCode::BackTab | KeyCode::Up => {
                self.selected = (self.selected + self.count() - 1) % self.count()
            }
            KeyCode::Backspace
                if (self.selected < 3 || self.selected == 9 || self.selected == 11) =>
            {
                self.fields[if self.selected == 9 {
                    3
                } else if self.selected == 11 {
                    4
                } else {
                    self.selected
                }]
                .pop();
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && (self.selected < 3 || self.selected == 9 || self.selected == 11) =>
            {
                self.fields[if self.selected == 9 {
                    3
                } else if self.selected == 11 {
                    4
                } else {
                    self.selected
                }]
                .clear()
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.selected == 0 =>
            {
                self.fields[0] = self.default_model.clone()
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && (self.selected < 3 || self.selected == 9 || self.selected == 11)
                    && self.fields[if self.selected == 9 {
                        3
                    } else if self.selected == 11 {
                        4
                    } else {
                        self.selected
                    }]
                    .len()
                        < 256 =>
            {
                self.fields[if self.selected == 9 {
                    3
                } else if self.selected == 11 {
                    4
                } else {
                    self.selected
                }]
                .push(c);
            }
            _ => {}
        }
    }
    fn draw(&self, frame: &mut ratatui::Frame) {
        let screen = frame.area();
        let width = screen.width.min(76);
        let height = screen.height.min(27);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        if self.confirm_yolo {
            frame.render_widget(
                Paragraph::new("YOLO auto-approves ALL tools in this AND FUTURE new sessions, in any directory.\nShell commands run unsandboxed with your filesystem/network access.\nEdits and commands can destroy data.\n\nEnter again to confirm. Esc cancels without saving.")
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL).title(" Confirm YOLO — dangerous ")),
                area,
            );
            return;
        }
        let labels = [
            "Model ID",
            "Max turns / prompt (0=unlimited)",
            "Context budget (bytes, not tokens)",
        ];
        let mut lines = vec![
            Line::raw(format!(
                "Provider: {} (fixed for this session)",
                self.provider
            )),
            Line::raw(""),
        ];
        for (i, label) in labels.iter().enumerate().take(self.count()) {
            lines.push(Line::styled(
                format!(
                    "{} {}: {}",
                    if self.selected == i { "▸" } else { " " },
                    label,
                    visible(&self.fields[i])
                ),
                Style::default().fg(if self.selected == i {
                    Color::Cyan
                } else {
                    Color::White
                }),
            ));
        }
        if !self.model_only {
            let prefs = &self.preferences;
            for (i, (label, value)) in [
                (
                    "Tool permissions",
                    if prefs.yolo {
                        "YOLO — auto-approve ALL tools"
                    } else {
                        "Ask every time"
                    },
                ),
                (
                    "Status: activity",
                    if prefs.status_activity { "on" } else { "off" },
                ),
                (
                    "Status: model",
                    if prefs.status_model { "on" } else { "off" },
                ),
                (
                    "Status: RAM (process RSS)",
                    if prefs.status_ram { "on" } else { "off" },
                ),
                (
                    "Status: context bytes",
                    if prefs.status_context { "on" } else { "off" },
                ),
                (
                    "Status: keyboard hints",
                    if prefs.status_hints { "on" } else { "off" },
                ),
            ]
            .iter()
            .enumerate()
            {
                lines.push(Line::styled(
                    format!(
                        "{} {label}: {value}",
                        if self.selected == i + 3 { "▸" } else { " " }
                    ),
                    Style::default().fg(if self.selected == i + 3 {
                        Color::Cyan
                    } else {
                        Color::White
                    }),
                ));
            }
        }
        if !self.model_only {
            lines.push(Line::styled(
                format!(
                    "{} Auto-compact % (0=off): {}",
                    if self.selected == 9 { "▸" } else { " " },
                    visible(&self.fields[3])
                ),
                Style::default().fg(if self.selected == 9 {
                    Color::Cyan
                } else {
                    Color::White
                }),
            ));
        }
        if !self.model_only {
            for (index, label, value) in [
                (
                    10,
                    "Sticky skills",
                    if self.sticky_skills {
                        "on".to_string()
                    } else {
                        "off".to_string()
                    },
                ),
                (
                    11,
                    "Skill directories (JSON list)",
                    visible(&self.fields[4]),
                ),
                (
                    12,
                    "MCP (reconnect on next prompt)",
                    if self.preferences.mcp_enabled {
                        "on"
                    } else {
                        "off"
                    }
                    .into(),
                ),
                (
                    13,
                    "Code mode",
                    if self.preferences.codemode_enabled {
                        "on"
                    } else {
                        "off"
                    }
                    .into(),
                ),
            ] {
                lines.push(Line::styled(
                    format!(
                        "{} {label}: {value}",
                        if self.selected == index { "▸" } else { " " }
                    ),
                    Style::default().fg(if self.selected == index {
                        Color::Cyan
                    } else {
                        Color::White
                    }),
                ));
            }
        }
        lines.extend([
            Line::raw(""),
            Line::raw("Type to edit · Ctrl-N/P models · Ctrl-U clear · Ctrl-D bundled default"),
            Line::raw("Tab/↑/↓ select · Space/←/→ toggle · Enter apply · Esc cancel"),
            Line::styled(
                if self.confirm_yolo {
                    "YOLO: unsandboxed commands run without asking! Enter again to confirm."
                } else {
                    ""
                },
                Style::default().fg(Color::Red),
            ),
            Line::raw("Model and permissions also default for new sessions everywhere."),
            Line::styled(visible(&self.error), Style::default().fg(Color::Red)),
        ]);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((
                    self.selected
                        .saturating_sub(area.height.saturating_sub(10) as usize)
                        as u16,
                    0,
                ))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(if self.model_only {
                            " Model "
                        } else {
                            " Settings — model & permissions become user defaults "
                        }),
                ),
            area,
        );
    }
}

#[derive(Default)]
enum Activity {
    #[default]
    Thinking,
    Responding,
    Tool(String),
    Compacting,
}
impl Activity {
    fn label(&self) -> String {
        match self {
            Self::Thinking => "Thinking".into(),
            Self::Responding => "Responding".into(),
            Self::Tool(name) => format!("Executing tool call: {}", one_line(name, 40)),
            Self::Compacting => "Compacting context".into(),
        }
    }
}

/// A fixed-width track with a ball that bounces at both ends.
fn activity_ball(frame: u8) -> String {
    let phase = frame as usize % 10;
    let position = if phase <= 5 { phase } else { 10 - phase };
    (0..6)
        .map(|index| if index == position { '●' } else { '·' })
        .collect()
}

struct View {
    lines: VecDeque<String>,
    bytes: usize,
    input: String,
    skills: Vec<(String, String)>,
    completion_index: usize,
    completion_dismissed: bool,
    scroll: u16,
    pending: Option<(ToolCall, oneshot::Sender<bool>)>,
    status: String,
    title: String,
    dialog: Option<SettingsDialog>,
    mcp_manager: Option<crate::mcp_ui::Manager>,
    inspector: Option<crate::inspect::Inspector>,
    resume_picker: Option<crate::resume_ui::Picker>,
    inspection: (Vec<(String, String)>, Vec<Message>),
    preferences: Preferences,
    model: String,
    context_used: usize,
    context_limit: usize,
    ram: Option<usize>,
    yolo_active: bool,
    busy: bool,
    activity: Activity,
    animation_frame: u8,
    input_cursor: usize,
    prompt_history: Vec<String>,
    history_index: Option<usize>,
    history_draft: (String, usize),
    details: Option<String>,
    details_scroll: u16,
    transcript_width: u16,
    followups: VecDeque<String>,
    steering_count: usize,
    update_available: Option<String>,
}
impl View {
    fn reset_completion(&mut self) {
        self.completion_index = 0;
        self.completion_dismissed = false;
    }
    fn suggestions(&self) -> Vec<Suggestion> {
        if self.completion_dismissed
            || self.dialog.is_some()
            || self.mcp_manager.is_some()
            || self.inspector.is_some()
            || self.resume_picker.is_some()
            || self.details.is_some()
            || self.pending.is_some()
        {
            return Vec::new();
        }
        slash_matches(&self.input, &self.skills)
    }
    fn completion_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let matches = self.suggestions();
        if matches.is_empty() {
            return false;
        }
        self.completion_index = self.completion_index.min(matches.len() - 1);
        match key.code {
            KeyCode::Down => self.completion_index = (self.completion_index + 1) % matches.len(),
            KeyCode::Up | KeyCode::BackTab => {
                self.completion_index = (self.completion_index + matches.len() - 1) % matches.len()
            }
            KeyCode::Tab | KeyCode::Enter
                if key.code == KeyCode::Tab
                    || self.input != matches[self.completion_index].command =>
            {
                self.input = format!("{} ", matches[self.completion_index].command);
                self.input_cursor = self.input.len();
                self.reset_completion();
            }
            KeyCode::Esc if !self.busy => self.completion_dismissed = true,
            _ => return false,
        }
        true
    }

    fn append(&mut self, text: &str) {
        let before = if self.scroll > 0 {
            self.rendered_rows()
        } else {
            0
        };
        let text = visible(text);
        for (i, line) in text.split('\n').enumerate() {
            if i == 0 && !self.lines.is_empty() {
                self.lines.back_mut().unwrap().push_str(line);
                self.bytes += line.len();
            } else {
                self.lines.push_back(line.into());
                self.bytes += line.len() + 1;
            }
        }
        // Bound both byte size and line count; Paragraph's scroll offset is u16.
        while self.bytes > TRANSCRIPT_LIMIT || self.lines.len() > 4000 {
            let Some(line) = self.lines.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(line.len() + 1);
        }
        if self.scroll > 0 {
            self.scroll = self
                .scroll
                .saturating_add(self.rendered_rows().saturating_sub(before));
        }
    }
    fn rendered_rows(&self) -> u16 {
        Paragraph::new(format_transcript(&self.lines))
            .wrap(Wrap { trim: false })
            .line_count(self.transcript_width.max(1))
            .min(u16::MAX as usize) as u16
    }
    fn insert(&mut self, text: &str) {
        if self.input.len() + text.len() <= 64 * 1024 {
            self.input.insert_str(self.input_cursor, text);
            self.input_cursor += text.len();
        }
    }
    fn backspace(&mut self) {
        if let Some((i, _)) = self.input[..self.input_cursor].char_indices().next_back() {
            self.input.drain(i..self.input_cursor);
            self.input_cursor = i;
        }
    }
    fn clear_draft(&mut self) -> bool {
        if self.input.is_empty() {
            return false;
        }
        self.input.clear();
        self.input_cursor = 0;
        self.history_index = None;
        self.history_draft = (String::new(), 0);
        self.reset_completion();
        true
    }
    fn remember_prompt(&mut self, prompt: &str) {
        if !prompt.trim().is_empty() && self.prompt_history.last().is_none_or(|last| last != prompt)
        {
            self.prompt_history.push(prompt.to_owned());
            if self.prompt_history.len() > 100 {
                self.prompt_history.remove(0);
            }
        }
        self.history_index = None;
        self.history_draft = (String::new(), 0);
    }
    fn recall_prompt(&mut self, older: bool) {
        if self.prompt_history.is_empty() {
            return;
        }
        let index = match (self.history_index, older) {
            (None, false) => return,
            (None, true) => {
                self.history_draft = (self.input.clone(), self.input_cursor);
                self.prompt_history.len() - 1
            }
            (Some(index), true) => index.saturating_sub(1),
            (Some(index), false) => index + 1,
        };
        if index == self.prompt_history.len() {
            (self.input, self.input_cursor) = self.history_draft.clone();
            self.history_index = None;
        } else {
            self.input = self.prompt_history[index].clone();
            self.input_cursor = self.input.len();
            self.history_index = Some(index);
        }
        self.reset_completion();
        // Recalled slash commands must remain browsable with Up/Down.
        self.completion_dismissed = true;
    }
    fn word_left(&self) -> usize {
        let prefix = &self.input[..self.input_cursor];
        let trimmed = prefix.trim_end_matches(char::is_whitespace);
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8())
    }
    fn word_right(&self) -> usize {
        let suffix = &self.input[self.input_cursor..];
        let trimmed = suffix.trim_start_matches(char::is_whitespace);
        self.input_cursor + suffix.len() - trimmed.len()
            + trimmed.find(char::is_whitespace).unwrap_or(trimmed.len())
    }
    fn editor_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || ctrl => {
                self.recall_prompt(true)
            }
            KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || ctrl => {
                self.recall_prompt(false)
            }
            KeyCode::Left if ctrl || alt => self.input_cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.input_cursor = self.word_right(),
            KeyCode::Char('b') if alt => self.input_cursor = self.word_left(),
            KeyCode::Char('f') if alt => self.input_cursor = self.word_right(),
            KeyCode::Left | KeyCode::Char('b') if key.code == KeyCode::Left || ctrl => {
                if let Some((i, _)) = self.input[..self.input_cursor].char_indices().next_back() {
                    self.input_cursor = i;
                }
            }
            KeyCode::Right | KeyCode::Char('f') if key.code == KeyCode::Right || ctrl => {
                if let Some(c) = self.input[self.input_cursor..].chars().next() {
                    self.input_cursor += c.len_utf8();
                }
            }
            KeyCode::Home | KeyCode::Char('a') if key.code == KeyCode::Home || ctrl => {
                self.input_cursor = self.input[..self.input_cursor]
                    .rfind('\n')
                    .map_or(0, |i| i + 1);
            }
            KeyCode::End | KeyCode::Char('e') if key.code == KeyCode::End || ctrl => {
                self.input_cursor += self.input[self.input_cursor..]
                    .find('\n')
                    .unwrap_or(self.input.len() - self.input_cursor);
            }
            KeyCode::Backspace if ctrl || alt => {
                let start = self.word_left();
                self.input.drain(start..self.input_cursor);
                self.input_cursor = start;
            }
            KeyCode::Char('w') if ctrl => {
                let start = self.word_left();
                self.input.drain(start..self.input_cursor);
                self.input_cursor = start;
            }
            KeyCode::Delete if ctrl || alt => {
                self.input.drain(self.input_cursor..self.word_right());
            }
            KeyCode::Char('d') if alt => {
                self.input.drain(self.input_cursor..self.word_right());
            }
            KeyCode::Backspace | KeyCode::Char('h') if key.code == KeyCode::Backspace || ctrl => {
                self.backspace()
            }
            KeyCode::Delete | KeyCode::Char('d') if key.code == KeyCode::Delete || ctrl => {
                if let Some(c) = self.input[self.input_cursor..].chars().next() {
                    self.input
                        .drain(self.input_cursor..self.input_cursor + c.len_utf8());
                }
            }
            KeyCode::Char('k') if ctrl => {
                let end = self.input[self.input_cursor..]
                    .find('\n')
                    .map_or(self.input.len(), |i| self.input_cursor + i);
                let end = if end == self.input_cursor && end < self.input.len() {
                    end + 1
                } else {
                    end
                };
                self.input.drain(self.input_cursor..end);
            }
            KeyCode::Char('u') if ctrl => {
                self.clear_draft();
            }
            _ => return false,
        }
        true
    }
    fn enqueue(&mut self, followup: bool, sender: Option<&mpsc::Sender<String>>) -> bool {
        let prompt = self.input.clone();
        let queued = if followup {
            if self.followups.len() < 32 {
                self.followups.push_back(prompt);
                true
            } else {
                false
            }
        } else {
            sender.is_some_and(|sender| sender.try_send(prompt).is_ok())
        };
        if queued {
            if !followup {
                self.steering_count += 1;
            }
            self.remember_prompt(&self.input.clone());
            self.input.clear();
            self.input_cursor = 0;
        }
        queued
    }
    fn next_queued(
        &mut self,
        unconsumed: &mut mpsc::Receiver<String>,
        discard: bool,
    ) -> Option<String> {
        let mut remaining = VecDeque::new();
        while let Ok(text) = unconsumed.try_recv() {
            remaining.push_back(text);
        }
        remaining.append(&mut self.followups);
        self.steering_count = 0;
        if discard {
            if !remaining.is_empty() {
                self.message(&format!(
                    "Cleared {} queued messages after interruption/error.",
                    remaining.len()
                ));
            }
            None
        } else {
            let next = remaining.pop_front();
            self.followups = remaining;
            next
        }
    }
    fn message(&mut self, text: &str) {
        self.append(&format!("\n\n{text}\n"));
    }
    fn event(&mut self, event: Event) {
        match event {
            Event::ProviderRetry {
                retry,
                max_retries,
                delay_secs,
            } => {
                self.status = format!(
                    "Provider retry {retry}/{max_retries} in {delay_secs}s · Esc/Ctrl-C cancel"
                );
                self.message(&format!(
                    "Temporary provider failure · retry {retry}/{max_retries} in {delay_secs}s"
                ));
            }
            Event::Inspection {
                definitions,
                history,
            } => {
                self.inspection = (definitions, history);
            }
            Event::RunStarted => {
                self.status = "Thinking · Esc/Ctrl-C cancel".into();
                self.busy = true;
                self.activity = Activity::Thinking;
                self.animation_frame = 0;
                self.message("Habibi");
            }
            Event::SteeringApplied(text) => {
                self.steering_count = self.steering_count.saturating_sub(1);
                self.message(&format!("You · steering\n{text}\n\nHabibi"));
            }
            Event::ContextUsage { used, limit } => {
                self.context_used = used;
                self.context_limit = limit;
            }
            Event::CompactionStarted => {
                self.activity = Activity::Compacting;
                self.status = "Compacting context…".into();
                self.message("Compacting context…");
            }
            Event::Compacted { before, after } => {
                self.activity = Activity::Thinking;
                self.message(&format!("Context compacted: {before} → {after} bytes. Original history retained in the journal."));
            }
            Event::TextDelta(text) => {
                self.activity = Activity::Responding;
                self.append(&text);
            }
            Event::Permission { call, reply } => {
                if self.yolo_active {
                    self.message(&format!("  › {} · auto-approved", tool_summary(&call)));
                    let _ = reply.send(true);
                    return;
                }
                self.message(&format!(
                    "  ? {} · approval required (F2 inspect)",
                    tool_summary(&call)
                ));
                self.status =
                    "Approval required · Ctrl-Y allow / Ctrl-N deny · F2 inspect · Esc interrupt"
                        .into();
                self.pending = Some((call, reply));
            }
            Event::ToolStarted(call) => {
                self.activity = Activity::Tool(call.name.clone());
                self.status = format!("Running {} · Esc/Ctrl-C cancel", call.name);
                self.message(&format!("  › {}", tool_summary(&call)));
            }
            Event::ToolFinished {
                output, is_error, ..
            } => {
                self.message(&result_summary(&output, is_error));
                if self.pending.is_none() {
                    self.activity = Activity::Thinking;
                    self.status = "Thinking · Esc/Ctrl-C cancel".into();
                }
            }
            Event::RunFinished { .. } => {
                self.busy = false;
            }
        }
    }
    fn prompt_title(&self) -> String {
        if self.pending.is_some() {
            "Approval required · Ctrl-Y/Ctrl-N · F2 inspect".into()
        } else if self.status == "Cancelling…" {
            "Cancelling…".into()
        } else if self.busy {
            format!(
                " {} {} ",
                activity_ball(self.animation_frame),
                self.activity.label()
            )
        } else {
            "Prompt · Enter submit · Alt-Enter follow-up".into()
        }
    }

    fn status_bar(&self) -> String {
        let mut parts = Vec::new();
        // Safety indicators cannot be hidden by customization.
        if self.yolo_active {
            parts.push("YOLO: ALL tools auto-approved".into());
        }
        if self.pending.is_some() || self.status == "Cancelling…" {
            parts.push(self.status.clone());
        } else if self.busy {
            parts.push("Esc/Ctrl-C cancel".into());
        } else if self.preferences.status_activity {
            parts.push("Ready".into());
        }
        if self.steering_count > 0 || !self.followups.is_empty() {
            parts.push(format!(
                "Queued: {} steering / {} follow-ups",
                self.steering_count,
                self.followups.len()
            ));
        }
        if let Some(version) = &self.update_available {
            parts.push(format!("Update {version} available · /update"));
        }
        if self.preferences.status_model {
            parts.push(visible(&self.model));
        }
        if self.preferences.status_ram {
            parts.push(
                self.ram
                    .map(|bytes| format!("RAM {:.1} MiB", bytes as f64 / 1048576.0))
                    .unwrap_or_else(|| "RAM n/a".into()),
            );
        }
        if self.preferences.status_context {
            let percent = self.context_used as f64 / self.context_limit.max(1) as f64 * 100.0;
            parts.push(format!(
                "Context {:.1}% ({}/{} B)",
                percent, self.context_used, self.context_limit
            ));
        }
        if self.preferences.status_hints {
            parts.push("Enter steer · Alt-Enter follow-up · Ctrl-Enter newline · Esc stop".into());
        }
        parts.join(" · ")
    }

    fn draw<B: ratatui::backend::Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        terminal.draw(|frame| {
            let area = frame.area();
            let (input_lines, cursor_row, cursor_col) = wrap_input(&self.input, self.input_cursor, area.width.saturating_sub(2).max(1) as usize);
            let input_height = (input_lines.len() as u16).saturating_add(2).clamp(3, (area.height / 2).clamp(3, 8));
            let suggestions = self.suggestions();
            let suggestion_rows = suggestions.len().min(area.height.saturating_sub(input_height + 5) as usize);
            let sections = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(suggestion_rows as u16),
                Constraint::Length(input_height),
                Constraint::Length(2),
            ])
            .split(area);
            let text = format_transcript(&self.lines);
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            let inner_width = sections[0].width.saturating_sub(2);
            self.transcript_width = inner_width;
            let height = sections[0].height.saturating_sub(2) as usize;
            let bottom = paragraph
                .line_count(inner_width)
                .saturating_sub(height)
                .min(u16::MAX as usize) as u16;
            self.scroll = self.scroll.min(bottom);
            frame.render_widget(
                paragraph
                    .scroll((bottom.saturating_sub(self.scroll), 0))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(self.title.as_str()),
                    ),
                sections[0],
            );
            let selected = self.completion_index.min(suggestions.len().saturating_sub(1));
            let start = selected.saturating_sub(suggestion_rows.saturating_sub(1));
            frame.render_widget(Paragraph::new(suggestions.iter().enumerate().skip(start).take(suggestion_rows).map(|(i, entry)| {
                Line::styled(format!("{} {}  {}", if i == selected { "▸" } else { " " }, entry.command, one_line(&entry.description, 100)),
                    if i == selected { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) })
            }).collect::<Vec<_>>()), sections[1]);
            let input_rows = sections[2].height.saturating_sub(2) as usize;
            let input_scroll = cursor_row.saturating_sub(input_rows.saturating_sub(1));
            frame.render_widget(
                Paragraph::new(input_lines.iter().map(|line| Line::raw(line.clone())).collect::<Vec<_>>())
                    .scroll((input_scroll.min(u16::MAX as usize) as u16, 0))
                    .block(Block::default().borders(Borders::ALL)
                        .border_style(Style::default().fg(if self.pending.is_some() { Color::Yellow } else if self.busy { Color::Cyan } else { Color::Reset }))
                        .title(self.prompt_title())),
                sections[2],
            );
            frame.render_widget(
                Paragraph::new(self.status_bar())
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(if self.yolo_active {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })),
                sections[3],
            );
            if self.dialog.is_none() && self.mcp_manager.is_none() && self.inspector.is_none() && self.resume_picker.is_none() && self.details.is_none() && sections[2].width > 2 && sections[2].height > 2 {
                frame.set_cursor_position((
                    sections[2].x + 1 + cursor_col as u16,
                    sections[2].y + 1 + cursor_row.saturating_sub(input_scroll) as u16,
                ));
            }
            if let Some(details) = &self.details {
                let max_scroll = Paragraph::new(details.as_str()).wrap(Wrap { trim: false })
                    .line_count(area.width.saturating_sub(2)).saturating_sub(area.height.saturating_sub(2) as usize);
                self.details_scroll = self.details_scroll.min(max_scroll.min(u16::MAX as usize) as u16);
                frame.render_widget(Clear, area);
                frame.render_widget(Paragraph::new(details.as_str()).wrap(Wrap { trim: false })
                    .scroll((self.details_scroll, 0)).block(Block::default().borders(Borders::ALL)
                    .title(" Proposed tool arguments · PgUp/PgDn scroll · F2 close · Esc interrupt ")), area);
            }
            if let Some(manager) = &self.mcp_manager { manager.draw(frame); }
            if let Some(dialog) = &self.dialog {
                dialog.draw(frame);
            }
            if let Some(inspector) = &mut self.inspector {
                inspector.draw(frame);
            }
            if let Some(picker) = &mut self.resume_picker { picker.draw(frame); }
        })?;
        Ok(())
    }
}

pub async fn run(
    mut session: Session,
    mut path: PathBuf,
    auth: libhabibi::auth::AuthStore,
) -> Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        drop(Screen);
        previous(info)
    }));
    let mut control = crate::control::Control::bind(&session.id().to_string(), &path)?;
    while let Some((next, next_path, next_control)) =
        run_session(session, path, &auth, control).await?
    {
        session = next;
        path = next_path;
        control = next_control;
    }
    Ok(())
}

async fn run_session(
    session: Session,
    path: PathBuf,
    auth: &libhabibi::auth::AuthStore,
    mut control: crate::control::Control,
) -> Result<Option<ResumeTarget>> {
    let session_id = session.id().to_string();
    let mut next_session = None;
    let _screen = Screen::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut program_status = crate::program_status::Reporter::terminal();
    program_status.report(crate::program_status::Status::Idle);
    let mut view = View {
        lines: VecDeque::new(),
        bytes: 0,
        input: String::new(),
        skills: Vec::new(),
        completion_index: 0,
        completion_dismissed: false,
        scroll: 0,
        pending: None,
        status: "Enter submit · PgUp/PgDn scroll · Ctrl-C quit · /help".into(),
        dialog: None,
        mcp_manager: None,
        inspector: None,
        resume_picker: None,
        inspection: Default::default(),
        preferences: session.config().preferences.clone(),
        model: format!("{} / {}", session.config().provider, session.config().model),
        context_used: session.context_bytes(),
        context_limit: session.config().max_context_bytes,
        ram: process_ram(),
        yolo_active: session.config().preferences.yolo,
        busy: false,
        activity: Activity::default(),
        animation_frame: 0,
        input_cursor: 0,
        prompt_history: Vec::new(),
        history_index: None,
        history_draft: (String::new(), 0),
        details: None,
        details_scroll: 0,
        transcript_width: 80,
        followups: VecDeque::new(),
        steering_count: 0,
        update_available: None,
        title: visible(&format!(
            " habibi · {} · {} ",
            session.config().provider,
            session.config().model
        )),
    };
    view.message(&format!("Workspace: {}\nSession: {}\nShell commands are not sandboxed. /settings controls tool permissions; /models lists model choices.",session.config().workspace.display(),path.display()));
    view.message(&format!("Control socket: {}", control.path.display()));
    if session.config().provider == libhabibi::Provider::Ollama {
        view.message(&format!(
            "Ollama: {} (no login required). Use an installed tool-capable model.",
            session.config().ollama_url
        ));
    } else {
        view.message(&format!(
            "Subscription adapters are experimental. Login outside the TUI: habibi login {}",
            session.config().provider
        ));
    }
    for message in session.messages() {
        match message {
            Message::User { text } => {
                view.remember_prompt(text);
                view.message(&format!("You\n{text}"));
            }
            Message::Assistant {
                text, tool_calls, ..
            } => {
                if !text.is_empty() {
                    view.message(&format!("Habibi\n{text}"));
                }
                for call in tool_calls {
                    view.message(&format!("  › {}", tool_summary(call)));
                }
            }
            Message::ToolResult {
                output, is_error, ..
            } => view.message(&result_summary(output, *is_error)),
        }
    }
    if session.config().preferences.yolo {
        view.message("Saved YOLO mode restored: ALL tools auto-approved. Shell commands are not sandboxed. Disable in /settings.");
    }
    view.inspection = (session.inspect_context(), session.tool_history().to_vec());
    let mut current_config = session.config().clone();
    let mut deferred_settings: Option<SettingsDialog> = None;
    view.skills = session.skill_catalog();
    let mut session = Some(session);
    let mut job: Option<Job> = None;
    let mut cancellation: Option<CancellationToken> = None;
    let mut steering_sender: Option<mpsc::Sender<String>> = None;
    let mut interrupted = false;
    let (tx, mut rx) = mpsc::channel(256);
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut last_ram = Instant::now();
    let mut last_animation = Instant::now();
    let cache = crate::base_dir("XDG_CACHE_HOME", ".cache")
        .ok()
        .map(|base| base.join("habibi/update.json"));
    let mut update_check = Some(tokio::spawn(crate::update::check(cache)));
    let mut model_check: Option<JoinHandle<Result<Vec<String>>>> = None;
    let mut installed_models = Vec::new();
    let outcome=async {
        loop {
            // Render at most once per frame, not once per token or tool-output chunk.
            tokio::select! {
                Some(request) = control.requests.recv() => {
                    use serde_json::json;
                    let id = request.value["id"].clone();
                    let mut shutdown = false;
                    let result = (|| -> Result<serde_json::Value> {
                        let method = request.value["method"].as_str().context("Missing method")?;
                        let params = &request.value["params"];
                        match method {
                            "status" => return Ok(json!({"session_id":session_id,"busy":job.is_some(),"stopping":interrupted,"queued":view.followups.len(),"pending_approvals":view.pending.iter().map(|(call,_)| &call.id).collect::<Vec<_>>(),"permission":view.pending.as_ref().map(|(call,_)|call),"socket":control.path})),
                            "prompt" | "follow_up" | "steer" => {
                                ensure!(view.resume_picker.is_none(), "Session picker open; close it before sending a prompt");
                                ensure!(!view.mcp_manager.as_ref().is_some_and(|m| m.authenticating()), "MCP authentication in progress; finish or cancel it first");
                                let text = crate::control::text(params)?;
                                ensure!(!interrupted, "Run is stopping; wait for idle");
                                if method == "steer" {
                                    steering_sender.as_ref().context("No active run to steer")?.try_send(text).context("Steering queue full or run ending")?;
                                    view.steering_count += 1;
                                } else if job.is_some() {
                                    ensure!(method != "prompt", "Agent busy; use steer or follow_up");
                                    ensure!(view.followups.len() < 32, "Follow-up queue full");
                                    view.followups.push_back(text);
                                } else {
                                    let running = session.take().context("Session unavailable")?;
                                    view.message(&format!("You · socket\n{text}"));
                                    let (next_job, cancel, sender) = start_job(running, text, tx.clone());
                                    job = Some(next_job); cancellation = Some(cancel); steering_sender = Some(sender);
                                }
                            }
                            "approve" => {
                                let call_id = params["call_id"].as_str().context("Missing call_id")?;
                                let allow = params["allow"].as_bool().context("allow must be boolean")?;
                                ensure!(view.pending.as_ref().is_some_and(|(call,_)|call.id == call_id), "Unknown or expired approval");
                                let (_, reply) = view.pending.take().unwrap();
                                reply.send(allow).map_err(|_|anyhow::anyhow!("Approval expired"))?;
                                view.details = None;
                                program_status.report(crate::program_status::Status::Thinking);
                            }
                            "stop" | "shutdown" => {
                                if let Some(cancel) = &cancellation { cancel.cancel(); interrupted = true; view.status = "Cancelling…".into(); }
                                view.followups.clear(); view.pending = None; view.details = None;
                                shutdown = method == "shutdown";
                            }
                            _ => bail!("Unknown method: {method}"),
                        }
                        Ok(json!({"accepted":true}))
                    })();
                    request.reply.send(crate::control::response(id, result)).await;
                    dirty = true;
                    if shutdown { break; }
                }
                _=tick.tick()=>{
                    if let Some(picker) = &mut view.resume_picker { dirty |= picker.poll(); }
                    if let Some(manager) = &mut view.mcp_manager {
                        let (changed, reconnect) = manager.poll_auth();
                        dirty |= changed;
                        if reconnect && let Some(session) = session.as_mut() { session.disconnect_mcp(); }
                    }

                    if view.busy && view.pending.is_none() && view.status != "Cancelling…" && last_animation.elapsed() >= Duration::from_millis(100) {
                        view.animation_frame = (view.animation_frame + 1) % 10;
                        last_animation = Instant::now();
                        dirty = true;
                    }
                    if last_ram.elapsed() >= Duration::from_secs(1) {
                        if view.preferences.status_ram { view.ram = process_ram(); dirty = true; }
                        last_ram = Instant::now();
                    }
                    if job.as_ref().is_some_and(|job|job.is_finished()) {
                        // Drain the completed run's events before displaying/starting another run.
                        while let Ok(event) = rx.try_recv() { if !interrupted { program_status.event(&event, view.yolo_active); } view.event(event); }
                        let (returned,result,mut unconsumed)=job.take().unwrap().await.context("Agent task panicked")?;
                        view.context_used = returned.context_bytes();
                        session=Some(returned);cancellation=None;steering_sender=None;view.pending=None;view.details=None;view.busy=false;
                        if let Some(dialog) = deferred_settings.take()
                            && let Err(error) = apply_settings(&mut view, session.as_mut().unwrap(), &dialog) {
                            view.message(&format!("Queued settings failed: {error:#}"));
                        }
                        current_config = session.as_ref().unwrap().config().clone();
                        view.skills = session.as_ref().unwrap().skill_catalog();
                        view.inspection = (session.as_ref().unwrap().inspect_context(), session.as_ref().unwrap().tool_history().to_vec());
                        if let Err(error)=&result {view.message(&format!("Stopped: {error:#}"));}
                        program_status.report(if interrupted { crate::program_status::Status::Idle } else if result.is_err() { crate::program_status::Status::Error } else { crate::program_status::Status::Done });
                        if let Some(prompt) = view.next_queued(&mut unconsumed, interrupted || result.is_err()) {
                            view.message(&format!("You · follow-up\n{prompt}"));
                            let running = session.take().context("Session unavailable")?;
                            let (next_job, cancel, sender) = start_job(running, prompt, tx.clone());
                            job = Some(next_job); cancellation = Some(cancel); steering_sender = Some(sender);
                        }
                        interrupted = false;
                        view.status="Ready · Enter submit · Ctrl-C quit".into();dirty=true;
                    }
                    if dirty {view.draw(&mut terminal)?;dirty=false;}
                }
                update = async { update_check.as_mut().unwrap().await }, if update_check.is_some() => {
                    view.update_available = update.ok().flatten(); update_check = None; dirty = true;
                }
                models = async { model_check.as_mut().unwrap().await }, if model_check.is_some() => {
                    model_check = None;
                    match models {
                        Ok(Ok(models)) => {
                            installed_models = models;
                            if let Some(dialog) = &mut view.dialog { dialog.model_choices = installed_models.clone(); }
                            if installed_models.is_empty() {
                                view.message("Ollama has no installed models. Install a tool-capable model with `ollama pull MODEL`.");
                            } else {
                                view.message(&format!("Installed Ollama models (tool support varies):\n{}\n/model: Ctrl-N/P selects a model; Enter saves it.", installed_models.join("\n")));
                            }
                        }
                        Ok(Err(error)) => view.message(&format!("Ollama model discovery failed: {error:#}")),
                        Err(error) => view.message(&format!("Model discovery task failed: {error}")),
                    }
                    dirty = true;
                }
                Some(event)=rx.recv()=>{if !interrupted { program_status.event(&event, view.yolo_active); } view.event(event);dirty=true;}
                input=keys.next()=>{
                    let Some(input)=input else {break};
                    match input? {
                        TerminalEvent::Resize(..)=>dirty=true,
                        TerminalEvent::Mouse(mouse) => {
                            match mouse.kind {
                                MouseEventKind::ScrollUp if view.resume_picker.is_some() => view.resume_picker.as_mut().unwrap().step(false),
                                MouseEventKind::ScrollDown if view.resume_picker.is_some() => view.resume_picker.as_mut().unwrap().step(true),
                                MouseEventKind::ScrollUp if view.inspector.is_some() => view.inspector.as_mut().unwrap().scroll(false),
                                MouseEventKind::ScrollDown if view.inspector.is_some() => view.inspector.as_mut().unwrap().scroll(true),
                                MouseEventKind::ScrollUp if view.details.is_some() => view.details_scroll=view.details_scroll.saturating_sub(3),
                                MouseEventKind::ScrollDown if view.details.is_some() => view.details_scroll=view.details_scroll.saturating_add(3),
                                MouseEventKind::ScrollUp => view.scroll=view.scroll.saturating_add(3),
                                MouseEventKind::ScrollDown => view.scroll=view.scroll.saturating_sub(3),
                                _ => {}
                            }
                            dirty=true;
                        }
                        TerminalEvent::Paste(text)=>{
                            view.reset_completion();
                            program_status.acknowledge();
                            if view.inspector.is_some() || view.resume_picker.is_some() { continue; }
                            if let Some(manager) = &mut view.mcp_manager { manager.paste(&text); dirty = true; continue; }
                            if let Some(dialog) = &mut view.dialog {
                                if (3..9).contains(&dialog.selected) || dialog.selected == 10 || dialog.selected >= 12 { continue; }
                                dialog.confirm_yolo = false;
                                let field = &mut dialog.fields[if dialog.selected == 9 { 3 } else if dialog.selected == 11 { 4 } else { dialog.selected }];
                                let text = visible(&text).replace('\n', " ");
                                if field.len() + text.len() <= 256 { field.push_str(&text); }
                            } else { view.insert(&visible(&text.replace("\r\n", "\n").replace('\r', "\n"))); }
                            dirty=true;
                        }
                        TerminalEvent::Key(key) if key.kind!=KeyEventKind::Release=>{
                            program_status.acknowledge();
                            dirty=true;
                            if let Some(picker) = &mut view.resume_picker {
                                match picker.key(key) {
                                    crate::resume_ui::Action::None => {},
                                    crate::resume_ui::Action::Close => view.resume_picker = None,
                                    crate::resume_ui::Action::Open(next_path) => {
                                        let result = Session::resume(&next_path, auth.clone());
                                        match result {
                                            Ok(next) if next.config().workspace == current_config.workspace => {
                                                // Bind before dropping the old session, so a socket error leaves it intact.
                                                match crate::control::Control::bind(&next.id().to_string(), &next_path) {
                                                    Ok(next_control) => { next_session = Some((next, next_path, next_control)); break; },
                                                    Err(error) => picker.error = format!("Cannot resume: {error:#}"),
                                                }
                                            }
                                            Ok(_) => picker.error = "Session workspace changed; reopen the picker.".into(),
                                            Err(error) => picker.error = format!("Cannot resume: {error:#}"),
                                        }
                                    }
                                }
                                continue;
                            }
                            if let Some(inspector) = &mut view.inspector {
                                if inspector.key(key) { view.inspector = None; }
                                continue;
                            }
                            if key.code==KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                                if let Some(manager) = &mut view.mcp_manager && manager.authenticating() {
                                    manager.key(crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                                    continue;
                                }

                                if view.dialog.is_none() && view.mcp_manager.is_none() && view.clear_draft() { continue; }
                                if let Some(cancel)=&cancellation {cancel.cancel();interrupted=true;program_status.report(crate::program_status::Status::Idle);view.followups.clear();view.pending=None;view.details=None;view.status="Cancelling…".into();}else{break}
                                continue
                            }
                            if let Some(manager) = &mut view.mcp_manager {
                                match manager.key(key) {
                                    crate::mcp_ui::Action::None => {},
                                    crate::mcp_ui::Action::Close => view.mcp_manager = None,
                                    crate::mcp_ui::Action::Saved => {
                                        view.mcp_manager = None;
                                        session.as_mut().context("Session unavailable")?.disconnect_mcp();
                                        view.message("MCP configuration saved. Connections closed; enabled servers reconnect on the next prompt.");
                                    }
                                }
                                continue;
                            }
                            if view.dialog.is_some() {
                                match key.code {
                                    KeyCode::Esc => view.dialog = None,
                                    KeyCode::Enter => {
                                        let dialog = view.dialog.as_mut().unwrap();
                                        if dialog.preferences.yolo && !view.yolo_active && !dialog.confirm_yolo {
                                            dialog.confirm_yolo = true;
                                            continue;
                                        }
                                        if job.is_some() {
                                            if let Err(error) = dialog.values().and_then(|_| serde_json::from_str::<Vec<PathBuf>>(&dialog.fields[4]).map_err(Into::into)) {
                                                dialog.error = format!("{error:#}");
                                            } else {
                                                deferred_settings = view.dialog.take();
                                                view.message("Settings queued; they apply after this run, before the next follow-up. Current tool permissions are unchanged.");
                                            }
                                        } else {
                                            let dialog = view.dialog.take().unwrap();
                                            match apply_settings(&mut view, session.as_mut().context("Session unavailable")?, &dialog) {
                                                Ok(()) => current_config = session.as_ref().unwrap().config().clone(),
                                                Err(error) => { let mut dialog = dialog; dialog.error = format!("{error:#}"); view.dialog = Some(dialog); }
                                            }
                                        }
                                    }
                                    _ => view.dialog.as_mut().unwrap().key(key),
                                }
                                continue;
                            }
                            if view.completion_key(key) { continue; }
                            if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::CONTROL) && view.details.is_none()
                                && let Some(prompt) = skill_prompt(&view.input, &view.skills) {
                                view.input = prompt; view.input_cursor = view.input.len();
                            }
                            if !matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab) { view.reset_completion(); }
                            match key.code {
                                KeyCode::F(2) if view.pending.is_some()=>{
                                    if view.details.take().is_none() {
                                        let call = &view.pending.as_ref().unwrap().0;
                                        let args=serde_json::to_string_pretty(&call.arguments).unwrap_or_default();
                                        view.details=Some(format!("{}\n{}", call.name, truncate(&args,24*1024)));
                                        view.details_scroll=0;
                                    }
                                }
                                KeyCode::PageUp if view.details.is_some()=>view.details_scroll=view.details_scroll.saturating_sub(10),
                                KeyCode::PageDown if view.details.is_some()=>view.details_scroll=view.details_scroll.saturating_add(10),
                                KeyCode::PageUp=>view.scroll=view.scroll.saturating_add(10),
                                KeyCode::PageDown=>view.scroll=view.scroll.saturating_sub(10),
                                KeyCode::Esc=>{
                                    if let Some(cancel)=&cancellation { cancel.cancel(); interrupted=true;program_status.report(crate::program_status::Status::Idle); }
                                    view.followups.clear();view.pending=None;view.details=None;view.status="Cancelling…".into();
                                }
                                KeyCode::Char('y'|'n') if view.pending.is_some() && (key.modifiers.contains(KeyModifiers::CONTROL) || view.details.is_some())=>{
                                    let (_,reply)=view.pending.take().unwrap();let _=reply.send(key.code==KeyCode::Char('y'));program_status.report(crate::program_status::Status::Thinking);
                                    view.details=None;view.status="Thinking…".into();
                                }
                                _ if view.details.is_some()=>{},
                                _ if view.editor_key(key)=>{},
                                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL)=>view.insert("\n"),
                                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL)=>view.insert("\n"),
                                KeyCode::Char(c) if !c.is_control() && !key.modifiers.intersects(KeyModifiers::CONTROL|KeyModifiers::ALT)=>view.insert(&c.to_string()),
                                KeyCode::Enter if job.is_some() && !view.input.trim().starts_with('/')=>{
                                    if !view.input.trim().is_empty() {
                                        if interrupted { view.message("Wait for cancellation to finish before submitting."); continue; }
                                        if !view.enqueue(key.modifiers.contains(KeyModifiers::ALT), steering_sender.as_ref()) {
                                            view.message("Queue full or run ending; draft kept. Try again shortly.");
                                        }
                                    }
                                }
                                KeyCode::Enter=>{
                                    let prompt=std::mem::take(&mut view.input);view.input_cursor=0;
                                    view.remember_prompt(&prompt);
                                    match prompt.trim() {
                                        ""=>{},
                                        "/quit"|"/exit" if job.is_none()=>break,
                                       "/quit"|"/exit"|"/compact" if job.is_some() => view.message("This command requires an idle agent. Esc interrupts the run."),
                                        "/update" => view.message("Install/upgrade (review the script before running):\ncurl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh\nReleases: https://github.com/habibiailabs/habibi-code/releases"),
                                        "/compact" => {
                                            let running = session.take().context("Session unavailable")?;
                                            let (next_job, cancel, sender) = start_job(running, "/compact".into(), tx.clone());
                                            job = Some(next_job); cancellation = Some(cancel); steering_sender = Some(sender); interrupted = false;
                                        }
                                        "/resume" if job.is_some() => view.message("Resume requires an idle agent. Esc interrupts the run."),
                                        "/resume" => match crate::resume_ui::Picker::open(&current_config.workspace, &path) {
                                            Ok(picker) => view.resume_picker = Some(picker),
                                            Err(error) => view.message(&format!("Cannot list sessions: {error:#}")),
                                        },
                                        "/mcp" if job.is_some() => view.message("MCP configuration requires an idle agent. Stop the run first."),
                                        "/mcp" => match crate::mcp_ui::Manager::open() {
                                            Ok(manager) => view.mcp_manager = Some(manager),
                                            Err(error) => view.message(&format!("Cannot open MCP manager: {error:#}")),
                                        },
                                        "/inspect" => {
                                            view.inspector = Some(crate::inspect::Inspector::from_snapshot(view.inspection.0.clone(), &view.inspection.1));
                                           if job.is_some() { view.message("Inspector: latest context-boundary snapshot; the agent continues running. Close and reopen to refresh."); }
                                        }
                                        "/models" | "/model" | "/settings" => {
                                            let provider = current_config.provider;
                                            if prompt.trim() != "/models" {
                                                let mut dialog = deferred_settings.clone().unwrap_or_else(|| SettingsDialog::from_config(&current_config, prompt.trim() == "/model"));
                                                dialog.model_only = prompt.trim() == "/model";
                                                dialog.selected = 0;
                                                if provider == libhabibi::Provider::Ollama { dialog.model_choices = installed_models.clone(); }
                                                view.dialog = Some(dialog);
                                            }
                                            if provider == libhabibi::Provider::Ollama && prompt.trim() != "/settings" && model_check.is_none() {
                                                let url = current_config.ollama_url.clone();
                                                model_check = Some(tokio::spawn(async move { libhabibi::ollama::Client::new(&url)?.models().await }));
                                                view.message("Discovering installed Ollama models…");
                                            } else if prompt.trim() == "/models" && provider != libhabibi::Provider::Ollama {
                                                view.message(&format!("Bundled {provider} subscription models (account availability varies):\n{}\n/model: Ctrl-N/P selects a model, or type any model ID; Enter saves it.", provider.models().join("\n")));
                                            }
                                        }
                                        "/help"=>view.message("/resume: browse sessions for this folder · /mcp: add/configure/disable/remove MCP servers · /inspect: context and tool-call inspector · /compact: summarize context · /models: list models · /model: choose or edit model ID · /settings: model, limits, status bar, tool permissions · /update: upgrade instructions · /quit: exit\nEnter: submit/steer · Alt-Enter: follow-up · Ctrl-Enter/Ctrl-J: newline\n↑/↓: select suggestion · Tab: complete command or skill · /skill:name [request]: invoke a skill · PgUp/PgDn or mouse wheel: scroll · Shift-drag: terminal selection · Ctrl-U: clear input · Ctrl-A/E: line start/end · Alt-B/F: word left/right · Ctrl-W: delete word · Ctrl-K: delete to line end · Up/Down: prompt history (when completion is closed) · F2: inspect approval\nCommands work during execution; settings apply after the run. /compact and /quit require idle. Ctrl-C clears a nonempty prompt first; otherwise cancels or quits. Esc cancels; completed edits are not undone.\nAsk mode: Ctrl-Y allows once, Ctrl-N denies (or y/n in F2 inspector). YOLO auto-approves all tools. Neither mode is a sandbox.\nLogin: subscriptions use habibi login codex|claude outside the TUI. Ollama needs no login.\nResume: /resume opens the folder-scoped session picker; CLI: habibi --resume <path>."),
                                        command if command.starts_with('/') => view.message("Unknown command. Available: /resume, /mcp, /inspect, /compact, /models, /model, /settings, /update, /help, /quit, /exit."),
                                        _=>{
                                            view.scroll=0;view.message(&format!("You\n{prompt}"));
                                            let running=session.take().context("Session unavailable")?;
                                            let (next_job,cancel,sender)=start_job(running,prompt,tx.clone());
                                            job=Some(next_job);cancellation=Some(cancel);steering_sender=Some(sender);interrupted=false;
                                        }
                                    }
                                }
                                _=>{}
                            }
                        }
                        _=>{}
                    }
                }
            }
        }
        Ok::<_,anyhow::Error>(())
    }.await;
    if outcome.is_err() {
        program_status.report(crate::program_status::Status::Error);
    }
    if let Some(check) = model_check {
        check.abort();
    }
    if let Some(check) = update_check {
        check.abort();
    }
    if let Some(cancel) = cancellation {
        cancel.cancel();
    }
    drop(rx);
    view.pending = None;
    if let Some(job) = job {
        let _ = job.await;
    }
    outcome?;
    Ok(next_session)
}

/// Current RSS, not peak usage or child-process memory. No subprocesses.
fn process_ram() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))?
            .split_whitespace()
            .next()?
            .parse::<usize>()
            .ok()?;
        kb.checked_mul(1024)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Strip terminal-control characters from model output, tool output, file paths, and paste.
pub(crate) fn visible(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' => Some(c),
            '\t' => Some(' '),
            _ if c.is_control() => None,
            _ => Some(c),
        })
        .collect()
}
fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1
    }
    format!(
        "{}\n[preview truncated; full content is not shown]",
        &text[..end]
    )
}
const SLASH_COMMANDS: &[&str] = &[
    "/mcp",
    "/inspect",
    "/settings",
    "/models",
    "/model",
    "/compact",
    "/help",
    "/update",
    "/quit",
    "/exit",
    "/resume",
];

#[derive(Debug)]
struct Suggestion {
    command: String,
    description: String,
}

fn slash_matches(input: &str, skills: &[(String, String)]) -> Vec<Suggestion> {
    if !input.starts_with('/') || input.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let prefix = input.to_ascii_lowercase();
    let descriptions = [
        "Manage MCP servers",
        "Inspect context and tool calls",
        "Configure session settings",
        "List models",
        "Choose a model",
        "Summarize context",
        "Show help",
        "Upgrade instructions",
        "Quit",
        "Quit",
        "Browse sessions for this folder",
    ];
    let mut matches: Vec<_> = SLASH_COMMANDS
        .iter()
        .zip(descriptions)
        .filter(|(command, _)| command.starts_with(&prefix))
        .map(|(command, description)| Suggestion {
            command: (*command).into(),
            description: description.into(),
        })
        .collect();
    matches.extend(
        skills
            .iter()
            .filter(|(name, _)| {
                format!("/skill:{name}").starts_with(&prefix)
                    || name.starts_with(prefix.trim_start_matches('/'))
            })
            .map(|(name, description)| Suggestion {
                command: format!("/skill:{name}"),
                description: format!("Skill · {description}"),
            }),
    );
    // Exact matches first, then stable built-in/catalog order. Never show >5.
    matches.sort_by_key(|entry| entry.command != prefix);
    matches.truncate(5);
    matches
}

fn skill_prompt(input: &str, skills: &[(String, String)]) -> Option<String> {
    let rest = input.trim().strip_prefix("/skill:")?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..end];
    if !skills.iter().any(|(known, _)| known == name) {
        return None;
    }
    let request = rest[end..].trim();
    Some(format!(
        "Use the {name} skill for this request. Load it with load_skill before proceeding, unless it is already loaded.{}",
        if request.is_empty() {
            String::new()
        } else {
            format!("\n\n{request}")
        }
    ))
}

fn apply_settings(view: &mut View, session: &mut Session, dialog: &SettingsDialog) -> Result<()> {
    let (model, turns, bytes, percent) = dialog.values()?;
    let dirs = serde_json::from_str(&dialog.fields[4])
        .context("Skill directories must be a JSON array of paths")?;
    session.update_settings_with_skills(
        model,
        turns,
        bytes,
        dialog.preferences.clone(),
        percent,
        dialog.sticky_skills,
        dirs,
    )?;
    let config = session.config();
    view.title = visible(&format!(
        " habibi · {} · {} ",
        config.provider, config.model
    ));
    view.preferences = config.preferences.clone();
    view.skills = session.skill_catalog();
    view.reset_completion();
    view.yolo_active = config.preferences.yolo;
    view.context_limit = config.max_context_bytes;
    view.context_used = session.context_bytes();
    view.model = format!("{} / {}", config.provider, config.model);
    view.inspection = (session.inspect_context(), session.tool_history().to_vec());
    let saved = (|| -> Result<()> {
        let path = crate::defaults::path()?;
        let mut defaults = crate::defaults::Defaults::load(&path)?;
        defaults.remember(config, dialog.model_only);
        defaults.save(&path)
    })();
    match saved {
        Ok(()) => view.message("Session settings and user defaults saved. Defaults apply to new sessions in every directory."),
        Err(error) => view.message(&format!("Session settings saved, but user defaults were NOT saved: {error:#}")),
    }
    Ok(())
}

fn start_job(
    mut session: Session,
    prompt: String,
    events: mpsc::Sender<Event>,
) -> (Job, CancellationToken, mpsc::Sender<String>) {
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let (sender, mut steering) = mpsc::channel(32);
    let job = tokio::spawn(async move {
        let connected = tokio::select! {
            result = crate::connect_mcp(&mut session) => result,
            _ = token.cancelled() => Err(anyhow::anyhow!("Cancelled")),
        };
        let result = if let Err(error) = connected {
            Err(error)
        } else if prompt == "/compact" {
            session.compact(events, token).await
        } else {
            session
                .prompt_with_steering(prompt, events, token, &mut steering)
                .await
        };
        (session, result, steering)
    });
    (job, cancel, sender)
}

fn one_line(text: &str, limit: usize) -> String {
    let text = visible(text).replace('\n', " ");
    let mut result: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        result.push('…');
    }
    result
}
fn tool_summary(call: &ToolCall) -> String {
    let target = call
        .arguments
        .get(if call.name == "bash" {
            "command"
        } else if call.name == "load_skill" {
            "name"
        } else {
            "path"
        })
        .and_then(|v| v.as_str())
        .unwrap_or("");
    format!("{} {}", one_line(&call.name, 32), one_line(target, 100))
}
fn result_summary(output: &str, is_error: bool) -> String {
    let first = output
        .lines()
        .find(|s| !s.trim().is_empty())
        .unwrap_or("(no output)");
    format!(
        "  {} {} · {} bytes",
        if is_error { "✗" } else { "✓" },
        one_line(first, 100),
        output.len()
    )
}
fn transcript_line(text: &str) -> Line<'static> {
    let style = if text == "You" || text.starts_with("You ·") {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if text == "Habibi" {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if text.starts_with("  ✗") {
        Style::default().fg(Color::Red)
    } else if text.starts_with("  ") {
        Style::default().fg(Color::DarkGray)
    } else if text.starts_with('#') {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let content = if text.starts_with('#') {
        text.trim_start_matches('#').trim_start()
    } else {
        text
    };
    Line::from(markdown_spans(content)).style(style)
}

fn markdown_spans(mut text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    loop {
        let next = [
            text.find("**").map(|i| (i, "**")),
            text.find('`').map(|i| (i, "`")),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(i, _)| *i);
        let Some((start, marker)) = next else {
            spans.push(Span::raw(text.to_owned()));
            break;
        };
        let after = &text[start + marker.len()..];
        let Some(end) = after.find(marker) else {
            spans.push(Span::raw(text.to_owned()));
            break;
        };
        spans.push(Span::raw(text[..start].to_owned()));
        spans.push(Span::styled(
            after[..end].to_owned(),
            if marker == "**" {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            },
        ));
        text = &after[end + marker.len()..];
    }
    spans
}

fn format_transcript(lines: &VecDeque<String>) -> Vec<Line<'static>> {
    let mut code = false;
    lines
        .iter()
        .map(|text| {
            if text.starts_with("```") {
                code = !code;
                Line::styled(
                    if code {
                        format!("┌ {}", text.trim_start_matches('`'))
                    } else {
                        "└".into()
                    },
                    Style::default().fg(Color::DarkGray),
                )
            } else if code {
                Line::styled(text.clone(), Style::default().fg(Color::Gray))
            } else {
                transcript_line(text)
            }
        })
        .collect()
}

/// Character-width wrapping shared by input height, rendering and cursor placement.
fn wrap_input(text: &str, cursor: usize, width: usize) -> (Vec<String>, usize, usize) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let (mut col, mut cursor_row, mut cursor_col) = (0, 0, 0);
    let mut soft_break = false;
    for (index, c) in text.char_indices() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0).min(width);
        if c != '\n' && col + w > width {
            lines.push(String::new());
            col = 0;
        }
        if index == cursor {
            cursor_row = lines.len() - 1;
            cursor_col = col;
        }
        if c == '\n' {
            if !soft_break {
                lines.push(String::new());
            }
            col = 0;
            soft_break = false;
        } else {
            soft_break = false;
            lines.last_mut().unwrap().push(c);
            col += w;
            if col == width {
                lines.push(String::new());
                col = 0;
                soft_break = true;
            }
        }
    }
    if cursor == text.len() {
        cursor_row = lines.len() - 1;
        cursor_col = col;
    }
    (lines, cursor_row, cursor_col)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_animation_bounces_and_does_not_duplicate_status() {
        assert_eq!(activity_ball(0), "●·····");
        assert_eq!(activity_ball(5), "·····●");
        assert_eq!(activity_ball(6), "····●·");
        assert_eq!(activity_ball(10), activity_ball(0));
        for frame in 0..10 {
            assert_eq!(
                activity_ball(frame).chars().filter(|c| *c == '●').count(),
                1
            );
        }
        let mut view = test_view();
        view.event(Event::RunStarted);
        assert!(view.prompt_title().contains("Thinking"));
        assert!(!view.status_bar().contains("Thinking"));
        assert!(view.status_bar().contains("cancel"));
        view.event(Event::TextDelta("hello".into()));
        assert!(view.prompt_title().contains("Responding"));
        view.event(Event::ToolStarted(ToolCall {
            id: "one".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command":"true"}),
        }));
        assert!(view.prompt_title().contains("Executing tool call: bash"));
        view.event(Event::ToolFinished {
            call_id: "one".into(),
            output: "ok".into(),
            is_error: false,
        });
        assert!(view.prompt_title().contains("Thinking"));
        view.event(Event::CompactionStarted);
        assert!(view.prompt_title().contains("Compacting"));
        view.status = "Cancelling…".into();
        assert_eq!(view.prompt_title(), "Cancelling…");
        view.event(Event::RunFinished {
            cancelled: true,
            error: None,
        });
        view.status = "Ready".into();
        assert!(view.prompt_title().starts_with("Prompt"));
    }

    #[test]
    fn settings_dialog_navigation_and_draft_validation() {
        let mut dialog = SettingsDialog {
            fields: vec![
                "custom-model".into(),
                "32".into(),
                "1048576".into(),
                "80".into(),
                "[\"~/.agents\",\".agents\"]".into(),
            ],
            selected: 0,
            model_only: false,
            default_model: "default-model".into(),
            model_choices: vec!["choice-a".into(), "choice-b".into()],
            provider: "codex".into(),
            error: String::new(),
            preferences: Preferences::default(),
            confirm_yolo: false,
            sticky_skills: true,
        };
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        dialog.key(key(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(dialog.fields[0], "choice-a");
        dialog.key(key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(dialog.fields[0], "choice-b");
        dialog.key(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(dialog.fields[0], "default-model");
        dialog.key(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(dialog.selected, 1);
        dialog.key(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(dialog.values().is_err());
        dialog.key(key(KeyCode::Char('8'), KeyModifiers::NONE));
        assert_eq!(dialog.values().unwrap().1, 8);
        dialog.key(key(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(dialog.selected, 0);
        dialog.selected = 3;
        dialog.key(key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(dialog.preferences.yolo);
        dialog.selected = 10;
        dialog.key(key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!dialog.sticky_skills);
        dialog.selected = 11;
        dialog.key(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(dialog.fields[4].is_empty());
        dialog.key(key(KeyCode::Char('['), KeyModifiers::NONE));
        dialog.key(key(KeyCode::Char(']'), KeyModifiers::NONE));
        assert_eq!(dialog.fields[4], "[]");
        dialog.selected = 6;
        dialog.key(key(KeyCode::Right, KeyModifiers::NONE));
        assert!(!dialog.preferences.status_ram);
        dialog.key(key(KeyCode::Backspace, KeyModifiers::NONE));
        dialog.selected = 0;
        dialog.model_only = true;
        dialog.key(key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(dialog.selected, 0);
    }

    #[test]
    fn slash_suggestions_match_only_command_drafts() {
        let names = |draft| {
            slash_matches(draft, &[])
                .into_iter()
                .map(|s| s.command)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("/ins"), vec!["/inspect"]);
        assert_eq!(names("/mod"), vec!["/models", "/model"]);
        assert_eq!(names("/"), SLASH_COMMANDS[..5]);
        assert_eq!(names("/model")[0], "/model");
        for draft in ["", "hello", "/unknown", "/model name", "/inspect\n"] {
            assert!(slash_matches(draft, &[]).is_empty(), "{draft:?}");
        }
    }

    #[test]
    fn skill_completion_selection_and_invocation() {
        let key = KeyEvent::new;
        let mut view = test_view();
        view.skills = vec![
            ("review".into(), "Review code".into()),
            ("release".into(), "Publish a release".into()),
        ];
        view.input = "/re".into();
        assert_eq!(
            view.suggestions()
                .iter()
                .map(|s| s.command.as_str())
                .collect::<Vec<_>>(),
            ["/resume", "/skill:review", "/skill:release"]
        );
        assert!(view.completion_key(key(KeyCode::Down, KeyModifiers::NONE)));
        assert!(view.completion_key(key(KeyCode::Down, KeyModifiers::NONE)));
        assert!(view.completion_key(key(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(view.input, "/skill:release ");
        assert!(view.suggestions().is_empty());
        view.insert("check the changelog");
        let prompt = skill_prompt(&view.input, &view.skills).unwrap();
        assert!(prompt.contains("release skill"));
        assert!(prompt.contains("load_skill"));
        assert!(prompt.ends_with("check the changelog"));
        assert!(skill_prompt("/skill:missing", &view.skills).is_none());
        assert!(skill_prompt("/settings", &view.skills).is_none());
        view.input = "/skill:re".into();
        assert_eq!(view.suggestions().len(), 2);
        assert!(view.completion_key(key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(view.suggestions().is_empty());
        view.reset_completion();
        view.busy = true;
        assert!(
            !view.completion_key(key(KeyCode::Esc, KeyModifiers::NONE)),
            "Esc must still cancel active work"
        );
        assert!(
            !view.completion_key(key(KeyCode::Enter, KeyModifiers::ALT)),
            "Alt-Enter must retain follow-up semantics"
        );
        view.input = "/mod".into();
        assert!(view.completion_key(key(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(view.input, "/models ");
        view.input = "/model".into();
        assert!(
            !view.completion_key(key(KeyCode::Enter, KeyModifiers::NONE)),
            "Exact command must submit"
        );
    }

    #[test]
    fn suggestions_render_as_five_rows_not_in_status_line() {
        use ratatui::backend::TestBackend;
        let mut view = test_view();
        view.input = "/".into();
        view.input_cursor = 1;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        view.draw(&mut terminal).unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..24)
            .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        for command in &SLASH_COMMANDS[..5] {
            assert_eq!(
                rows.iter().filter(|line| line.contains(command)).count(),
                if *command == "/model" { 2 } else { 1 }
            );
        }
        assert!(!rows[22..].join("").contains("/inspect"));
        assert!(!rows.join("").contains("Tab complete:"));
        for (width, height) in [(1, 1), (12, 5), (30, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            view.draw(&mut terminal).unwrap();
        }
        view.skills = vec![("demo".into(), "A skill\nwith \u{1b}controls".into())];
        view.input = "/skill:".into();
        view.input_cursor = view.input.len();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        view.draw(&mut terminal).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("/skill:demo"));
        assert!(!content.contains('\u{1b}'));
    }

    #[test]
    fn retry_notice_preserves_running_state_and_permissions() {
        let mut view = test_view();
        view.event(Event::RunStarted);
        view.event(Event::ProviderRetry {
            retry: 2,
            max_retries: 5,
            delay_secs: 4,
        });
        assert!(view.busy);
        assert!(!view.yolo_active);
        assert!(view.pending.is_none());
        assert!(
            view.lines
                .iter()
                .any(|line| line.contains("retry 2/5 in 4s"))
        );
    }

    #[test]
    fn inspection_snapshot_does_not_interrupt_or_approve() {
        let mut view = test_view();
        view.event(Event::RunStarted);
        view.event(Event::Inspection {
            definitions: vec![("System".into(), "snapshot".into())],
            history: vec![],
        });
        assert!(view.busy);
        assert!(view.pending.is_none());
        assert_eq!(view.inspection.0[0].1, "snapshot");
        let _inspector =
            crate::inspect::Inspector::from_snapshot(view.inspection.0.clone(), &view.inspection.1);
        assert!(view.busy);
    }

    #[test]
    fn prompt_history_restores_draft_and_does_not_mutate_entries() {
        let mut view = test_view();
        view.remember_prompt("first\nmessage");
        view.remember_prompt("/help");
        view.remember_prompt("/help");
        view.insert("draft 🦀");
        view.input_cursor = 5;
        view.recall_prompt(true);
        assert_eq!(view.input, "/help");
        assert!(view.suggestions().is_empty());
        view.insert(" edited");
        view.recall_prompt(true);
        assert_eq!(view.input, "first\nmessage");
        view.recall_prompt(true);
        assert_eq!(view.input, "first\nmessage");
        view.recall_prompt(false);
        assert_eq!(view.input, "/help");
        view.recall_prompt(false);
        assert_eq!((&*view.input, view.input_cursor), ("draft 🦀", 5));
        view.recall_prompt(false);
        assert_eq!(view.input, "draft 🦀");
        assert!(view.clear_draft());
        assert!(!view.clear_draft());
        assert_eq!(view.input_cursor, 0);
        for i in 0..120 {
            view.remember_prompt(&i.to_string());
        }
        assert_eq!(view.prompt_history.len(), 100);
    }

    #[test]
    fn terminal_editing_is_unicode_safe_and_line_aware() {
        let mut view = test_view();
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let alt = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        view.insert("héllo 世界\nnext 🦀");
        view.editor_key(ctrl('a'));
        assert_eq!(view.input_cursor, "héllo 世界\n".len());
        view.editor_key(ctrl('e'));
        assert_eq!(view.input_cursor, view.input.len());
        view.editor_key(alt('b'));
        assert_eq!(&view.input[view.input_cursor..], "🦀");
        view.editor_key(ctrl('d'));
        assert_eq!(view.input, "héllo 世界\nnext ");
        view.editor_key(ctrl('w'));
        assert_eq!(view.input, "héllo 世界\n");
        view.editor_key(ctrl('b'));
        view.editor_key(ctrl('k'));
        assert_eq!(view.input, "héllo 世界");
        view.editor_key(alt('b'));
        view.editor_key(alt('f'));
        assert_eq!(view.input_cursor, view.input.len());
        view.editor_key(ctrl('h'));
        assert_eq!(view.input, "héllo 世");
        view.editor_key(ctrl('a'));
        view.editor_key(alt('d'));
        assert_eq!(view.input, " 世");
        view.editor_key(ctrl('f'));
        view.editor_key(ctrl('k'));
        assert_eq!(view.input, " ");
        view.editor_key(ctrl('u'));
        assert!(view.input.is_empty());
    }

    fn test_view() -> View {
        View {
            lines: VecDeque::new(),
            bytes: 0,
            input: String::new(),
            skills: Vec::new(),
            completion_index: 0,
            completion_dismissed: false,
            scroll: 0,
            pending: None,
            status: "Ready".into(),
            title: String::new(),
            dialog: None,
            mcp_manager: None,
            inspector: None,
            resume_picker: None,
            inspection: Default::default(),
            preferences: Preferences::default(),
            model: "codex / custom".into(),
            context_used: 512,
            context_limit: 1024,
            ram: Some(5 * 1048576),
            yolo_active: false,
            busy: false,
            activity: Activity::default(),
            animation_frame: 0,
            input_cursor: 0,
            prompt_history: Vec::new(),
            history_index: None,
            history_draft: (String::new(), 0),
            details: None,
            details_scroll: 0,
            transcript_width: 80,
            followups: VecDeque::new(),
            steering_count: 0,
            update_available: None,
        }
    }

    #[test]
    fn status_elements_toggle_but_safety_stays_visible() {
        let mut view = test_view();
        let bar = view.status_bar();
        assert!(bar.contains("RAM 5.0 MiB"));
        assert!(bar.contains("Context 50.0% (512/1024 B)"));
        view.preferences = Preferences {
            status_activity: false,
            status_model: false,
            status_ram: false,
            status_context: false,
            status_hints: false,
            yolo: false,
            ..Preferences::default()
        };
        assert_eq!(view.status_bar(), "");
        view.yolo_active = true;
        assert!(view.status_bar().contains("YOLO"));
        view.busy = true;
        view.status = "Cancelling…".into();
        assert!(view.status_bar().contains("Cancelling"));
    }

    #[tokio::test]
    async fn yolo_auto_approves_but_ask_mode_waits_for_user() {
        let mut view = test_view();
        let call = ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command":"true"}),
        };
        let (reply, mut answer) = oneshot::channel();
        view.event(Event::Permission {
            call: call.clone(),
            reply,
        });
        assert!(answer.try_recv().is_err());
        assert!(view.pending.is_some());
        assert!(view.status_bar().contains("Approval required"));
        let (_, reply) = view.pending.take().unwrap();
        reply.send(false).unwrap();
        assert!(!answer.await.unwrap());
        view.yolo_active = true;
        let (reply, answer) = oneshot::channel();
        view.event(Event::Permission { call, reply });
        assert!(answer.await.unwrap());
        assert!(view.pending.is_none());
    }

    #[test]
    fn multiline_editor_wraps_and_edits_unicode_at_cursor() {
        let mut view = test_view();
        view.insert("first\n界");
        assert_eq!(view.input, "first\n界");
        view.backspace();
        assert_eq!(view.input, "first\n");
        view.input_cursor = 0;
        view.insert("é");
        assert_eq!(view.input, "éfirst\n");
        assert_eq!(view.input_cursor, 2);
        view.backspace();
        assert_eq!(view.input, "first\n");
        assert_eq!(
            wrap_input("abc\ndef", 7, 3),
            (vec!["abc".into(), "def".into(), "".into()], 2, 0)
        );
        assert_eq!(
            wrap_input("a\n\nb", 4, 10),
            (vec!["a".into(), "".into(), "b".into()], 2, 1)
        );
        assert_eq!(
            wrap_input("界b", 3, 2),
            (vec!["界".into(), "b".into()], 1, 0)
        );
    }

    #[test]
    fn summaries_are_bounded_and_scroll_stays_anchored() {
        let call = ToolCall {
            id: "1".into(),
            name: "edit".into(),
            arguments: serde_json::json!({"path":"src/main.rs", "new_text":"sensitive long content"}),
        };
        assert_eq!(tool_summary(&call), "edit src/main.rs");
        assert!(!tool_summary(&call).contains("sensitive"));
        assert!(result_summary(&"x".repeat(64000), false).len() < 150);
        let mut view = test_view();
        view.message("first\nsecond\nthird");
        view.scroll = 2;
        let before = view.scroll;
        view.append("\nnext\nmore");
        assert_eq!(view.scroll, before + 2);
    }

    #[tokio::test]
    async fn steering_and_followups_are_bounded_ordered_and_interruptible() {
        let mut view = test_view();
        let (sender, mut receiver) = mpsc::channel(1);
        view.insert("steer first");
        assert!(view.enqueue(false, Some(&sender)));
        view.insert("steer second");
        assert!(!view.enqueue(false, Some(&sender)));
        assert_eq!(view.input, "steer second");
        assert!(view.enqueue(true, Some(&sender)));
        view.insert("follow up");
        assert!(view.enqueue(true, Some(&sender)));
        assert_eq!(
            view.next_queued(&mut receiver, false).unwrap(),
            "steer first"
        );
        assert_eq!(
            view.next_queued(&mut receiver, false).unwrap(),
            "steer second"
        );
        assert_eq!(view.next_queued(&mut receiver, false).unwrap(), "follow up");
        for _ in 0..32 {
            view.insert("queued");
            assert!(view.enqueue(true, None));
        }
        view.insert("draft kept");
        assert!(!view.enqueue(true, None));
        assert_eq!(view.input, "draft kept");
        assert!(view.next_queued(&mut receiver, true).is_none());
        assert!(view.followups.is_empty());
    }

    #[test]
    fn markdown_formats_prose_but_preserves_code() {
        let lines = VecDeque::from([
            "## Heading".into(),
            "**bold** and `inline`".into(),
            "```rust".into(),
            "**literal**".into(),
            "```".into(),
        ]);
        let formatted = format_transcript(&lines);
        assert_eq!(formatted[0].to_string(), "Heading");
        assert_eq!(formatted[1].to_string(), "bold and inline");
        assert_eq!(formatted[3].to_string(), "**literal**");
    }

    #[test]
    fn terminal_output_and_unicode_are_safe() {
        assert_eq!(visible("hi\x1b[31m\x07\tthere"), "hi[31m there");
        assert_eq!(
            wrap_input("a界b", 5, 3),
            (vec!["a界".into(), "b".into()], 1, 1)
        );
        assert!(truncate("éé", 3).starts_with('é'));
    }
}
