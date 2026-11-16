//! User-owned MCP config editor. Saving never starts a server; next prompt reconnects.
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
use unicode_width::UnicodeWidthStr;

pub struct Manager {
    path: PathBuf,
    original: Option<Vec<u8>>,
    servers: BTreeMap<String, Value>,
    selected: usize,
    edit: Option<Editor>,
    error: String,
    confirm: bool,
    auth: Option<AuthDialog>,
}
enum AuthEvent {
    Url(String),
    Finished(Result<()>),
}
struct AuthDialog {
    task: tokio::task::JoinHandle<()>,
    events: tokio::sync::mpsc::UnboundedReceiver<AuthEvent>,
    callback: Option<tokio::sync::oneshot::Sender<String>>,
    input: Input,
    url: String,
}
impl Drop for AuthDialog {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Default)]
struct Input {
    value: String,
    cursor: usize,
}
impl Input {
    fn new(value: String) -> Self {
        let cursor = value.len();
        Self { value, cursor }
    }
    fn insert(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if self.value.len() + text.len() <= 64 * 1024 {
            self.value.insert_str(self.cursor, &text);
            self.cursor += text.len();
        }
    }
    fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => {
                self.cursor = self.value[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            }
            KeyCode::Right => {
                if let Some(c) = self.value[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            KeyCode::Backspace => {
                if let Some((i, _)) = self.value[..self.cursor].char_indices().next_back() {
                    self.value.drain(i..self.cursor);
                    self.cursor = i;
                }
            }
            KeyCode::Delete => {
                if let Some(c) = self.value[self.cursor..].chars().next() {
                    self.value.drain(self.cursor..self.cursor + c.len_utf8());
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&c.to_string())
            }
            _ => {}
        }
    }
    // Horizontal scrolling keeps the caret inside its box, even for wide text.
    fn display(&self, width: usize) -> (String, usize) {
        let mut start = self.cursor;
        let mut caret = 0;
        for (offset, c) in self.value[..self.cursor].char_indices().rev() {
            let columns = clean(&c.to_string()).width();
            if caret + columns >= width.max(1) {
                break;
            }
            caret += columns;
            start = offset;
        }
        (clean(&self.value[start..]), caret)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Transport,
    Enabled,
    Command,
    Arg(usize),
    EnvKey(usize),
    EnvSource(usize),
    Url,
    Token,
}
struct Editor {
    old_name: Option<String>,
    name: Input,
    http: bool,
    enabled: bool,
    command: Input,
    args: Vec<Input>,
    env: Vec<(Input, Input)>,
    url: Input,
    token: Input,
    selected: usize,
}
impl Editor {
    fn new() -> Self {
        Self {
            old_name: None,
            name: Input::default(),
            http: false,
            enabled: true,
            command: Input::default(),
            args: vec![],
            env: vec![],
            url: Input::default(),
            token: Input::default(),
            selected: 0,
        }
    }
    fn from_server(name: String, value: Value) -> Result<Self> {
        let mut edit = Self::new();
        edit.old_name = Some(name.clone());
        edit.name = Input::new(name);
        match serde_json::from_value::<libhabibi::mcp::Server>(value)? {
            libhabibi::mcp::Server::Stdio(s) => {
                edit.enabled = !s.disabled;
                edit.command = Input::new(s.command);
                edit.args = s.args.into_iter().map(Input::new).collect();
                edit.env = s
                    .env_from
                    .into_iter()
                    .map(|(k, v)| (Input::new(k), Input::new(v)))
                    .collect();
            }
            libhabibi::mcp::Server::Http(s) => {
                edit.http = true;
                edit.enabled = !s.disabled;
                edit.url = Input::new(s.url);
                edit.token = Input::new(s.bearer_token_env.unwrap_or_default());
            }
        }
        Ok(edit)
    }
    fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Name, Field::Transport, Field::Enabled];
        if self.http {
            fields.extend([Field::Url, Field::Token]);
        } else {
            fields.push(Field::Command);
            fields.extend((0..self.args.len()).map(Field::Arg));
            for i in 0..self.env.len() {
                fields.extend([Field::EnvKey(i), Field::EnvSource(i)]);
            }
        }
        fields
    }
    fn input(&self, field: Field) -> Option<&Input> {
        match field {
            Field::Name => Some(&self.name),
            Field::Command => Some(&self.command),
            Field::Arg(i) => self.args.get(i),
            Field::EnvKey(i) => self.env.get(i).map(|p| &p.0),
            Field::EnvSource(i) => self.env.get(i).map(|p| &p.1),
            Field::Url => Some(&self.url),
            Field::Token => Some(&self.token),
            _ => None,
        }
    }
    fn input_mut(&mut self, field: Field) -> Option<&mut Input> {
        match field {
            Field::Name => Some(&mut self.name),
            Field::Command => Some(&mut self.command),
            Field::Arg(i) => self.args.get_mut(i),
            Field::EnvKey(i) => self.env.get_mut(i).map(|p| &mut p.0),
            Field::EnvSource(i) => self.env.get_mut(i).map(|p| &mut p.1),
            Field::Url => Some(&mut self.url),
            Field::Token => Some(&mut self.token),
            _ => None,
        }
    }
    fn value(&self) -> Result<Value> {
        use libhabibi::mcp::{HttpServer, Server, StdioServer};
        let server = if self.http {
            let token = self.token.value.trim();
            ensure!(
                token.is_empty() || env_name(token),
                "Token environment variable must be a variable name, not a secret value"
            );
            Server::Http(HttpServer {
                disabled: !self.enabled,
                url: self.url.value.trim().into(),
                bearer_token_env: if token.is_empty() {
                    None
                } else {
                    Some(token.into())
                },
            })
        } else {
            let mut env_from = BTreeMap::new();
            for (target, source) in &self.env {
                let (target, source) = (target.value.trim(), source.value.trim());
                ensure!(
                    env_name(target) && env_name(source),
                    "Each environment mapping needs a child variable name and a source variable name (letters, digits, underscores; no leading digit)"
                );
                ensure!(
                    env_from.insert(target.into(), source.into()).is_none(),
                    "Duplicate child environment variable: {target}"
                );
            }
            Server::Stdio(StdioServer {
                disabled: !self.enabled,
                command: self.command.value.clone(),
                args: self.args.iter().map(|a| a.value.clone()).collect(),
                env_from,
            })
        };
        Ok(serde_json::to_value(server)?)
    }
    fn key(&mut self, key: KeyEvent) {
        let fields = self.fields();
        let field = fields[self.selected];
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % fields.len(),
            KeyCode::BackTab | KeyCode::Up => {
                self.selected = (self.selected + fields.len() - 1) % fields.len()
            }
            KeyCode::F(2) => {
                self.http = !self.http;
                self.selected = 1;
            }
            KeyCode::F(3) if !self.http && self.args.len() < 256 => {
                self.args.push(Input::default());
                self.selected = 3 + self.args.len();
            }
            KeyCode::F(4) if !self.http && self.env.len() < 128 => {
                self.env.push((Input::default(), Input::default()));
                self.selected = 4 + self.args.len() + 2 * (self.env.len() - 1);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match field {
                    Field::Arg(i) => {
                        self.args.remove(i);
                    }
                    Field::EnvKey(i) | Field::EnvSource(i) => {
                        self.env.remove(i);
                    }
                    _ => {}
                }
                self.selected = self.selected.min(self.fields().len() - 1);
            }
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
                if matches!(field, Field::Transport | Field::Enabled) =>
            {
                if field == Field::Transport {
                    self.http = !self.http;
                } else {
                    self.enabled = !self.enabled;
                }
            }
            _ => {
                if let Some(input) = self.input_mut(field) {
                    input.key(key);
                }
            }
        }
    }
}
fn env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn clean(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
            .chars()
            .collect::<Vec<_>>()
        })
        .collect()
}
pub enum Action {
    None,
    Close,
    Saved,
}

fn validate(servers: &BTreeMap<String, Value>) -> Result<()> {
    ensure!(servers.len() <= 32, "At most 32 servers");
    for (name, value) in servers {
        ensure!(
            !name.is_empty()
                && name.len() <= 24
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Names must contain 1–24 ASCII letters, digits or hyphens"
        );
        let server: libhabibi::mcp::Server = serde_json::from_value(value.clone()).context("Invalid server: use command/args/env_from or url/bearer_token_env, plus optional disabled")?;
        match server {
            libhabibi::mcp::Server::Stdio(s) => {
                ensure!(!s.command.trim().is_empty(), "Command cannot be empty")
            }
            libhabibi::mcp::Server::Http(s) => {
                let url = reqwest::Url::parse(&s.url)?;
                ensure!(
                    url.host().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.query().is_none()
                        && url.fragment().is_none(),
                    "URL cannot contain credentials, query or fragment; use the token environment field"
                );
                let local = url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host == "[::1]"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                ensure!(
                    url.scheme() == "https" || (url.scheme() == "http" && local),
                    "HTTP requires loopback; otherwise use HTTPS"
                );
            }
        }
    }
    Ok(())
}

impl Manager {
    pub fn open() -> Result<Self> {
        Self::load(crate::base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/mcp.json"))
    }
    fn load(path: PathBuf) -> Result<Self> {
        let original = match std::fs::read(&path) {
            Ok(bytes) => {
                ensure!(bytes.len() <= 1024 * 1024, "Config exceeds 1 MiB");
                Some(bytes)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let servers = match &original {
            Some(bytes) => {
                let config: libhabibi::mcp::Config = serde_json::from_slice(bytes)
                    .context("Invalid MCP file; fix JSON before opening manager")?;
                config
                    .servers
                    .into_iter()
                    .map(|(k, v)| Ok((k, serde_json::to_value(v)?)))
                    .collect::<Result<_>>()?
            }
            None => BTreeMap::new(),
        };
        Ok(Self {
            path,
            original,
            servers,
            selected: 0,
            edit: None,
            error: String::new(),
            confirm: false,
            auth: None,
        })
    }
    fn name(&self) -> Option<String> {
        self.servers.keys().nth(self.selected).cloned()
    }
    pub fn authenticating(&self) -> bool {
        self.auth.is_some()
    }
    pub fn paste(&mut self, text: &str) {
        if let Some(auth) = &mut self.auth {
            if auth.input.value.len() + text.len() <= 8192 {
                auth.input.insert(text);
            }
            return;
        }
        self.confirm = false;
        if let Some(edit) = &mut self.edit {
            let field = edit.fields()[edit.selected];
            if let Some(input) = edit.input_mut(field) {
                input.insert(text);
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if let Some(auth) = &mut self.auth {
            match key.code {
                KeyCode::Esc => {
                    self.auth = None;
                    self.error = "Authentication cancelled; previous credentials preserved".into();
                }
                KeyCode::Enter if !auth.input.value.is_empty() => {
                    if let Some(callback) = auth.callback.take() {
                        let _ = callback.send(std::mem::take(&mut auth.input.value));
                    }
                }
                _ => {
                    if auth.input.value.len() < 8192 {
                        auth.input.key(key);
                    }
                }
            }
            return Action::None;
        }
        self.error.clear();
        if let Some(edit) = &mut self.edit {
            match key.code {
                KeyCode::Esc => self.edit = None,
                KeyCode::Enter => {
                    let result = (|| -> Result<()> {
                        let name = edit.name.value.trim().to_owned();
                        ensure!(
                            edit.old_name.as_ref() == Some(&name)
                                || !self.servers.contains_key(&name),
                            "Server name already exists"
                        );
                        let value = edit.value()?;
                        let mut draft = self.servers.clone();
                        if let Some(old) = &edit.old_name {
                            draft.remove(old);
                        }
                        draft.insert(name.clone(), value);
                        validate(&draft)?;
                        self.selected = draft.keys().position(|k| *k == name).unwrap_or(0);
                        self.servers = draft;
                        Ok(())
                    })();
                    match result {
                        Ok(()) => self.edit = None,
                        Err(error) => self.error = format!("{error:#}"),
                    }
                }
                _ => edit.key(key),
            }
            return Action::None;
        }
        if self.confirm {
            self.confirm = false;
            if key.code == KeyCode::Enter {
                match self.save() {
                    Ok(()) => return Action::Saved,
                    Err(error) => self.error = format!("{error:#}"),
                }
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.servers.len().saturating_sub(1))
            }
            KeyCode::Char('n') => self.edit = Some(Editor::new()),
            KeyCode::Char('e') | KeyCode::Enter => {
                if let Some(name) = self.name() {
                    match Editor::from_server(name.clone(), self.servers[&name].clone()) {
                        Ok(edit) => self.edit = Some(edit),
                        Err(error) => self.error = format!("{error:#}"),
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(name) = self.name() {
                    self.servers.remove(&name);
                    self.selected = self.selected.min(self.servers.len().saturating_sub(1));
                }
            }
            KeyCode::Char(' ') => {
                if let Some(name) = self.name() {
                    let value = self.servers.get_mut(&name).unwrap();
                    let disabled = value["disabled"].as_bool().unwrap_or(false);
                    value["disabled"] = json!(!disabled);
                }
            }
            KeyCode::Char('s') => self.confirm = true,
            KeyCode::Char('a' | 'l') => {
                if let Err(error) = self.authenticate(key.code == KeyCode::Char('l')) {
                    self.error = format!("{error:#}");
                }
            }

            _ => {}
        }
        Action::None
    }
    fn authenticate(&mut self, logout: bool) -> Result<()> {
        let name = self.name().context("Select an HTTP server first")?;
        let current: libhabibi::mcp::Config = match &self.original {
            Some(bytes) => serde_json::from_slice(bytes)?,
            None => anyhow::bail!("Save the configuration before authenticating"),
        };
        let saved = current
            .servers
            .get(&name)
            .context("Save the server before authenticating")?;
        ensure!(
            serde_json::to_value(saved)? == self.servers[&name],
            "Save server changes before authenticating"
        );
        let libhabibi::mcp::Server::Http(server) = saved else {
            anyhow::bail!("OAuth is only available for HTTP servers")
        };
        ensure!(
            logout || server.bearer_token_env.is_none(),
            "Clear the bearer environment field and save before using OAuth"
        );
        let store = crate::mcp_auth::store(&server.url)?;
        let resource = server.url.clone();
        let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let (callback, input) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = async {
                if logout {
                    return store.logout().await;
                }
                let login =
                    libhabibi::mcp_oauth::Login::begin(store, &resource, Default::default())
                        .await?;
                let url = login.authorization_url().to_owned();
                let _ = events_tx.send(AuthEvent::Url(url.clone()));
                crate::mcp_auth::open_browser(&url);
                login
                    .complete(libhabibi::CancellationToken::new(), async {
                        input.await.context("Login cancelled")
                    })
                    .await
            }
            .await;
            let _ = events_tx.send(AuthEvent::Finished(result));
        });
        self.auth = Some(AuthDialog {
            task,
            events,
            callback: Some(callback),
            input: Input::default(),
            url: String::new(),
        });
        Ok(())
    }
    /// Poll without blocking the terminal. Successful changes require reconnect.
    pub fn poll_auth(&mut self) -> (bool, bool) {
        let mut changed = false;
        if let Some(auth) = &mut self.auth {
            while let Ok(event) = auth.events.try_recv() {
                changed = true;
                match event {
                    AuthEvent::Url(url) => auth.url = url,
                    AuthEvent::Finished(result) => {
                        let reconnect = result.is_ok();
                        self.error = match result { Ok(()) => "OAuth credentials updated. Reconnect on the next prompt. Logout only removes local credentials.".into(), Err(error) => format!("{error:#}") };
                        self.auth = None;
                        return (true, reconnect);
                    }
                }
            }
        }
        (changed, false)
    }
    fn save(&self) -> Result<()> {
        validate(&self.servers)?;
        let bytes = serde_json::to_vec_pretty(&json!({"mcpServers":self.servers}))?;
        ensure!(bytes.len() <= 1024 * 1024, "Config exceeds 1 MiB");
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        ensure!(
            current == self.original,
            "MCP config changed externally; close and reopen manager"
        );
        let parent = self.path.parent().context("Config path needs parent")?;
        std::fs::create_dir_all(parent)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temp = parent.join(format!(".mcp-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result
    }
    pub fn draw(&self, frame: &mut Frame) {
        let screen = frame.area();
        let width = screen.width.min(100);
        let height = screen.height.min(38);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        let border = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Line::styled(
                " MCP servers ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = border.inner(area);
        frame.render_widget(border, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Some(auth) = &self.auth {
            let lines = vec![
                Line::styled(
                    " MCP OAuth authentication",
                    Style::default().fg(Color::Cyan),
                ),
                Line::raw(""),
                Line::raw(if auth.url.is_empty() {
                    "Discovering authorization metadata / updating credentials…"
                } else {
                    "Browser opened if available. Otherwise open this link:"
                }),
                Line::raw(clean(&auth.url)),
                Line::raw(""),
                Line::raw("Waiting for the loopback callback (up to five minutes)."),
                Line::raw("Remote browser: paste the FULL callback URL, then Enter."),
                Line::raw(format!(
                    "Callback input hidden · {} characters",
                    auth.input.value.chars().count()
                )),
                Line::raw("Esc cancels. Credentials are saved only after successful login."),
                Line::raw("Pre-registered client? Use habibi mcp login SERVER --client-id ID."),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            return;
        }
        if self.confirm {
            frame.render_widget(Paragraph::new(vec![
                Line::styled(" Save and trust this configuration?", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Line::raw(""),
                Line::raw("Enabled local programs run UNSANDBOXED with your account privileges on the next prompt. Remote servers receive tool arguments."),
                Line::raw("This user-level configuration applies in all directories."),
                Line::raw(""),
                Line::styled("Enter: save & apply     Any other key: go back", Style::default().fg(Color::Cyan)),
            ]).wrap(Wrap { trim: false }), inner);
            return;
        }
        let footer_height = inner.height.min(if self.error.is_empty() { 3 } else { 5 });
        let body = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(footer_height),
        );
        let footer = Rect::new(inner.x, inner.y + body.height, inner.width, footer_height);
        if let Some(edit) = &self.edit {
            draw_editor(frame, body, edit);
        } else {
            self.draw_list(frame, body);
        }
        let hints = if self.edit.is_some() {
            "Tab/↑/↓ focus · ←/→ edit · Ctrl-U clear · F2 transport\nF3 +argument · F4 +env mapping · Ctrl-D remove row\nEnter accept draft · Esc discard edit"
        } else {
            "↑/↓ select · n add · Enter/e edit · Space toggle · d remove\na Authenticate · l logout · s save & apply · Esc discard changes\nChanges are drafts until saved. Reconnect on the next prompt."
        };
        let mut lines: Vec<Line> = hints
            .lines()
            .map(|l| Line::styled(l, Style::default().fg(Color::Cyan)))
            .collect();
        if !self.error.is_empty() {
            lines.insert(
                0,
                Line::styled(
                    clean(&self.error),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            );
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), footer);
    }
    fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::styled(
                format!("  {} configured servers", self.servers.len()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
        ];
        if self.servers.is_empty() {
            lines.push(Line::styled(
                "  No servers yet. Press n to add a local or remote MCP server.",
                Style::default().fg(Color::DarkGray),
            ));
        }
        let rows = area.height.saturating_sub(3).max(1) as usize;
        let start = self.selected.saturating_sub(rows - 1);
        for (i, (name, value)) in self.servers.iter().enumerate().skip(start).take(rows) {
            let enabled = !value["disabled"].as_bool().unwrap_or(false);
            let http = value.get("url").is_some();
            let selected = i == self.selected;
            let base = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, base.fg(Color::Cyan)),
                Span::styled(
                    if enabled { "● ON   " } else { "○ OFF  " },
                    base.fg(if enabled { Color::Green } else { Color::Yellow }),
                ),
                Span::styled(
                    format!("{}  ", clean(name)),
                    base.fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if http { "HTTP  " } else { "STDIO " },
                    base.fg(Color::Magenta),
                ),
                Span::styled(
                    if http {
                        if value["bearer_token_env"].is_string() {
                            "[bearer env] ".into()
                        } else {
                            match crate::mcp_auth::store(value["url"].as_str().unwrap_or(""))
                                .and_then(|s| s.has_credentials())
                            {
                                Ok(true) => "[OAuth saved] ".into(),
                                Ok(false) => "[no OAuth] ".into(),
                                Err(_) => "[OAuth storage error] ".into(),
                            }
                        }
                    } else {
                        String::new()
                    },
                    base.fg(Color::Yellow),
                ),
                Span::styled(
                    clean(
                        value[if http { "url" } else { "command" }]
                            .as_str()
                            .unwrap_or(""),
                    ),
                    base.fg(Color::Gray),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn field_label(field: Field) -> (String, &'static str) {
    match field {
        Field::Name => (
            "Server name".into(),
            "e.g. github (letters, digits, hyphens)",
        ),
        Field::Transport => ("Transport · Space or ←/→ to change".into(), ""),
        Field::Enabled => ("Enabled · Space or ←/→ to toggle".into(), ""),
        Field::Command => (
            "Executable · not a shell command".into(),
            "e.g. npx or /absolute/path/to/server",
        ),
        Field::Arg(i) => (
            format!("Argument {} · one exact argument, no shell quoting", i + 1),
            "Empty argument (remove row with Ctrl-D if unwanted)",
        ),
        Field::EnvKey(i) => (
            format!("Environment {} · child variable name", i + 1),
            "e.g. API_TOKEN",
        ),
        Field::EnvSource(i) => (
            format!("Environment {} · read from parent variable", i + 1),
            "e.g. MY_SERVER_TOKEN — variable name, NOT a secret",
        ),
        Field::Url => (
            "Server URL · Streamable HTTP".into(),
            "https://example.com/mcp",
        ),
        Field::Token => (
            "Bearer token · parent environment variable (optional)".into(),
            "Leave blank to use saved OAuth (list: a Authenticate), or no authentication",
        ),
    }
}
fn draw_editor(frame: &mut Frame, area: Rect, edit: &Editor) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            if edit.old_name.is_some() {
                " Edit server · JSON generated automatically"
            } else {
                " Add server · JSON generated automatically"
            },
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let fields = edit.fields();
    let capacity = (area.height.saturating_sub(1) / 3).max(1) as usize;
    let start = edit.selected.saturating_sub(capacity - 1);
    for (row, (index, field)) in fields
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let y = area.y.saturating_add(1 + row as u16 * 3);
        if y >= area.bottom() {
            break;
        }
        let rect = Rect::new(area.x, y, area.width, 3.min(area.bottom() - y));
        let focused = index == edit.selected;
        let (label, placeholder) = field_label(*field);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {}{} ", if focused { "▸ " } else { "" }, label))
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let inside = block.inner(rect);
        let (text, color, caret) = if let Some(input) = edit.input(*field) {
            let (text, caret) = input.display(inside.width as usize);
            (
                if input.value.is_empty() {
                    placeholder.into()
                } else {
                    text
                },
                if input.value.is_empty() {
                    Color::DarkGray
                } else {
                    Color::White
                },
                Some(caret),
            )
        } else if *field == Field::Transport {
            (
                if edit.http {
                    "◀  Remote · Streamable HTTP  ▶"
                } else {
                    "◀  Local process · stdio  ▶"
                }
                .into(),
                Color::Magenta,
                None,
            )
        } else {
            (
                if edit.enabled {
                    "● Enabled"
                } else {
                    "○ Disabled"
                }
                .into(),
                if edit.enabled {
                    Color::Green
                } else {
                    Color::Yellow
                },
                None,
            )
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(color))
                .block(block),
            rect,
        );
        if focused
            && inside.width > 0
            && inside.height > 0
            && let Some(caret) = caret
        {
            frame.set_cursor_position((inside.x + (caret as u16).min(inside.width - 1), inside.y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn auth_dialog_hides_callback_supports_cancel_and_reports_completion() {
        use ratatui::{Terminal, backend::TestBackend};
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::load(dir.path().join("mcp.json")).unwrap();
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let (callback, input) = tokio::sync::oneshot::channel();
        manager.auth = Some(AuthDialog {
            task: tokio::spawn(std::future::pending()),
            events,
            callback: Some(callback),
            input: Input::default(),
            url: "https://example.com/authorize".into(),
        });
        manager.paste("http://127.0.0.1/callback?code=SECRET&state=STATE");
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
        terminal.draw(|f| manager.draw(f)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("https://example.com/authorize"));
        assert!(!content.contains("SECRET"));
        manager.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(input.await.unwrap().contains("SECRET"));
        tx.send(AuthEvent::Finished(Ok(()))).unwrap();
        assert_eq!(manager.poll_auth(), (true, true));
        assert!(!manager.authenticating());
        let (_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(std::future::pending());
        let abort = task.abort_handle();
        manager.auth = Some(AuthDialog {
            task,
            events,
            callback: None,
            input: Input::default(),
            url: String::new(),
        });
        manager.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
        assert!(!manager.authenticating());
    }

    #[test]
    fn authentication_rejects_drafts_stdio_and_bearer_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let mut manager = Manager::load(path.clone()).unwrap();
        manager
            .servers
            .insert("remote".into(), json!({"url":"https://example.com/mcp"}));
        assert!(
            manager
                .authenticate(false)
                .unwrap_err()
                .to_string()
                .contains("Save")
        );
        for value in [
            json!({"command":"server"}),
            json!({"url":"https://example.com/mcp", "bearer_token_env":"TOKEN"}),
        ] {
            manager.servers.insert("remote".into(), value);
            manager.save().unwrap();
            manager = Manager::load(path.clone()).unwrap();
            assert!(manager.authenticate(false).is_err());
        }
    }

    #[test]
    fn roundtrip_disable_remove_and_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let mut manager = Manager::load(path.clone()).unwrap();
        manager.servers.insert(
            "example".into(),
            json!({"command":"example","disabled":true}),
        );
        manager.save().unwrap();
        let mut manager = Manager::load(path.clone()).unwrap();
        assert_eq!(manager.servers["example"]["disabled"], true);
        manager.servers.clear();
        manager.save().unwrap();
        assert!(Manager::load(path.clone()).unwrap().servers.is_empty());
        assert!(
            manager.save().is_err(),
            "stale snapshot must not overwrite newer file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn editor_keys_stage_changes_until_confirmed_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let mut manager = Manager::load(path.clone()).unwrap();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        manager.key(key(KeyCode::Char('n')));
        manager.paste("local");
        for _ in 0..3 {
            manager.key(key(KeyCode::Tab));
        }
        manager.paste("example");
        manager.key(key(KeyCode::F(3)));
        manager.paste("--stdio");
        manager.key(key(KeyCode::Enter));
        assert!(manager.edit.is_none());
        assert!(!path.exists());
        manager.key(key(KeyCode::Char(' ')));
        manager.key(key(KeyCode::Char('s')));
        assert!(!path.exists());
        assert!(matches!(manager.key(key(KeyCode::Enter)), Action::Saved));
        let manager = Manager::load(path).unwrap();
        assert_eq!(manager.servers["local"]["disabled"], true);
        assert_eq!(manager.servers["local"]["command"], "example");
        assert_eq!(manager.servers["local"]["args"], json!(["--stdio"]));
    }

    #[test]
    fn form_roundtrips_both_transports_and_preserves_argument_boundaries() {
        for value in [
            json!({"command":"/path with spaces/server", "args":["--stdio", "one argument with spaces", "", "quote\"and\\slash"], "env_from":{"API_TOKEN":"MY_TOKEN"}, "disabled":true}),
            json!({"url":"https://example.com/mcp", "bearer_token_env":"TOKEN", "disabled":false}),
        ] {
            let edit = Editor::from_server("example".into(), value.clone()).unwrap();
            assert_eq!(edit.value().unwrap(), value);
        }
        let mut edit = Editor::new();
        edit.command = Input::new("server".into());
        edit.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        edit.input_mut(edit.fields()[edit.selected])
            .unwrap()
            .insert("hello world");
        edit.key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
        edit.input_mut(edit.fields()[edit.selected])
            .unwrap()
            .insert("TOKEN");
        edit.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        edit.input_mut(edit.fields()[edit.selected])
            .unwrap()
            .insert("PARENT_TOKEN");
        assert_eq!(
            edit.value().unwrap()["env_from"],
            json!({"TOKEN":"PARENT_TOKEN"})
        );
        edit.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        edit.url = Input::new("https://example.com/mcp".into());
        assert!(edit.value().unwrap().get("command").is_none());
        assert!(edit.value().unwrap().get("env_from").is_none());
        edit.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(edit.value().unwrap()["args"], json!(["hello world"]));
        edit.selected = edit.fields().len() - 1;
        edit.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(edit.env.is_empty());
        edit.selected = 4;
        edit.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(edit.args.is_empty());
        assert!(edit.selected < edit.fields().len());
    }

    #[test]
    fn form_rejects_invalid_or_duplicate_variable_names() {
        let mut edit = Editor::new();
        edit.env.push((
            Input::new("TOKEN".into()),
            Input::new("not a variable".into()),
        ));
        assert!(edit.value().is_err());
        edit.env[0].1 = Input::new("PARENT_TOKEN".into());
        edit.env
            .push((Input::new("TOKEN".into()), Input::new("OTHER".into())));
        assert!(edit.value().is_err());
        edit.http = true;
        edit.token = Input::new("literal secret".into());
        assert!(edit.value().is_err());
    }

    #[test]
    fn unicode_input_supports_caret_editing_and_horizontal_scroll() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut input = Input::new("a界é".into());
        input.key(key(KeyCode::Left));
        input.insert("x");
        assert_eq!(input.value, "a界xé");
        input.key(key(KeyCode::Backspace));
        input.key(key(KeyCode::Delete));
        assert_eq!(input.value, "a界");
        input.key(key(KeyCode::Home));
        input.insert("first");
        input.key(key(KeyCode::End));
        let (visible, caret) = input.display(4);
        assert_eq!(visible, "a界");
        assert_eq!(caret, 3);
        input.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(input.value.is_empty());
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn colored_form_renders_boxes_and_scrolls_to_selected_field() {
        use ratatui::{Terminal, backend::TestBackend};
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::load(dir.path().join("mcp.json")).unwrap();
        manager.edit = Some(Editor::new());
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
        terminal.draw(|f| manager.draw(f)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for label in ["Server name", "Transport", "Enabled", "Executable"] {
            assert!(content.contains(label), "{label}");
        }
        assert!(!content.contains("Server JSON"));
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.fg == Color::Cyan)
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.fg == Color::Green)
        );
        for _ in 0..12 {
            manager.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        }
        terminal.draw(|f| manager.draw(f)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("Argument 12"));
        manager.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        terminal.draw(|f| manager.draw(f)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("Server URL"));
        assert!(content.contains("Bearer token"));
        for (w, h) in [(1, 1), (10, 4), (30, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| manager.draw(f)).unwrap();
        }
    }

    #[test]
    fn invalid_configs_rejected_without_starting_programs() {
        for value in [
            json!({"command":""}),
            json!({"url":"http://example.com/mcp"}),
            json!({"url":"https://user:secret@example.com"}),
            json!({"url":"https://example.com/mcp?token=secret"}),
            json!({"command":"test","secret":"literal"}),
        ] {
            assert!(validate(&BTreeMap::from([("test".into(), value)])).is_err());
        }
    }
}
