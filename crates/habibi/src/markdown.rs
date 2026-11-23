//! Streaming event parser: no retained AST, grammar database, or rendered-history cache.
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct Renderer {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    width: usize,
    column: usize,
    indent: usize,
    quote: usize,
    code: bool,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    links: Vec<String>,
    item_widths: Vec<usize>,
}
impl Renderer {
    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }
    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }
    fn flush(&mut self) {
        if !self.spans.is_empty() {
            self.lines.push(Line::from(std::mem::take(&mut self.spans)));
        }
        self.column = 0;
    }
    fn blank(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }
    fn prefix(&mut self) {
        if self.column == 0 {
            let prefix = format!("{}{}", "│ ".repeat(self.quote), " ".repeat(self.indent));
            // Always leave room for content, even in deeply nested narrow layouts.
            let prefix: String = prefix.chars().take(self.width.saturating_sub(2)).collect();
            self.column = prefix.width();
            if !prefix.is_empty() {
                self.spans
                    .push(Span::styled(prefix, Style::default().fg(Color::DarkGray)));
            }
        }
    }
    fn text(&mut self, text: &str, style: Style) {
        for token in text.split_inclusive(char::is_whitespace) {
            let word_width = token.trim_end().width();
            if !self.code
                && self.column > self.indent + self.quote * 2
                && self.column + word_width > self.width
            {
                self.flush();
            }
            for c in token.chars() {
                if c == '\n' {
                    if self.code && self.spans.is_empty() {
                        self.lines.push(Line::default());
                    }
                    self.flush();
                    continue;
                }
                self.prefix();
                let w = c.width().unwrap_or(0);
                if self.column + w > self.width {
                    self.flush();
                    self.prefix();
                }
                if !self.code && c.is_whitespace() && self.column == self.indent + self.quote * 2 {
                    continue;
                }
                if let Some(last) = self.spans.last_mut().filter(|s| s.style == style) {
                    last.content.to_mut().push(c);
                } else {
                    self.spans.push(Span::styled(c.to_string(), style));
                }
                self.column += w;
            }
        }
    }
}

pub fn render(source: &str, width: usize) -> Vec<Line<'static>> {
    let mut r = Renderer {
        lines: Vec::new(),
        spans: Vec::new(),
        width: width.max(2),
        column: 0,
        indent: 0,
        quote: 0,
        code: false,
        styles: vec![Style::default()],
        lists: Vec::new(),
        links: Vec::new(),
        item_widths: Vec::new(),
    };
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES;
    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => {
                    r.blank();
                    r.push_style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::Strong => r.push_style(Style::default().add_modifier(Modifier::BOLD)),
                Tag::Emphasis => r.push_style(Style::default().add_modifier(Modifier::ITALIC)),
                Tag::Strikethrough => {
                    r.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT))
                }
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    r.links.push(dest_url.into_string());
                    r.push_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
                Tag::BlockQuote(_) => {
                    r.flush();
                    r.quote += 1;
                }
                Tag::List(start) => {
                    r.flush();
                    r.lists.push(start);
                }
                Tag::Item => {
                    r.flush();
                    let marker = match r.lists.last_mut() {
                        Some(Some(n)) => {
                            let marker = format!("{n}. ");
                            *n = n.saturating_add(1);
                            marker
                        }
                        _ => "• ".into(),
                    };
                    r.text(&marker, Style::default().fg(Color::Cyan));
                    r.item_widths.push(marker.width());
                    r.indent += marker.width();
                }
                Tag::CodeBlock(kind) => {
                    r.blank();
                    let language = match kind {
                        CodeBlockKind::Fenced(s) => s.into_string(),
                        _ => String::new(),
                    };
                    r.text(
                        &format!("┌ {language}"),
                        Style::default().fg(Color::DarkGray),
                    );
                    r.flush();
                    r.code = true;
                    r.push_style(Style::default().fg(Color::Gray).bg(Color::Rgb(30, 30, 30)));
                }
                Tag::Table(_) => r.blank(),
                Tag::TableHead => {
                    r.flush();
                    r.push_style(Style::default().add_modifier(Modifier::BOLD));
                }
                Tag::TableRow => r.flush(),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    r.styles.pop();
                    r.blank();
                }
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                    r.styles.pop();
                }
                TagEnd::Link | TagEnd::Image => {
                    r.styles.pop();
                    if let Some(url) = r.links.pop() {
                        r.text(&format!(" ({url})"), Style::default().fg(Color::DarkGray));
                    }
                }
                TagEnd::Paragraph => {
                    if r.lists.is_empty() {
                        r.blank();
                    } else {
                        r.flush();
                    }
                }
                TagEnd::BlockQuote(_) => {
                    r.flush();
                    r.quote = r.quote.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    r.flush();
                    r.lists.pop();
                    if r.lists.is_empty() {
                        r.blank();
                    }
                }
                TagEnd::Item => {
                    r.flush();
                    r.indent = r.indent.saturating_sub(r.item_widths.pop().unwrap_or(0));
                }
                TagEnd::CodeBlock => {
                    r.flush();
                    r.code = false;
                    r.styles.pop();
                    r.text("└", Style::default().fg(Color::DarkGray));
                    r.blank();
                }
                TagEnd::TableCell => r.text(" │ ", Style::default().fg(Color::DarkGray)),
                TagEnd::TableHead => {
                    r.flush();
                    r.styles.pop();
                }
                TagEnd::TableRow => r.flush(),
                TagEnd::Table => r.blank(),
                _ => {}
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                r.text(&text, r.style())
            }
            Event::Code(text) => r.text(&text, r.style().fg(Color::Yellow)),
            Event::SoftBreak => r.text(" ", r.style()),
            Event::HardBreak => {
                r.flush();
            }
            Event::Rule => {
                r.blank();
                r.text(
                    &"─".repeat(r.width.min(40)),
                    Style::default().fg(Color::DarkGray),
                );
                r.blank();
            }
            Event::TaskListMarker(checked) => r.text(
                if checked { "☑ " } else { "☐ " },
                Style::default().fg(Color::Cyan),
            ),
            _ => {}
        }
    }
    r.flush();
    while r.lines.last().is_some_and(|line| line.spans.is_empty()) {
        r.lines.pop();
    }
    r.lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_styles_links_and_literal_code() {
        let lines = render(
            "## Title\n\n**bold _nested_** [site](https://example.com)\n\n```rust\n**literal**\n```",
            80,
        );
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Title"));
        assert!(text.contains("site (https://example.com)"));
        assert!(text.contains("**literal**"));
        assert!(lines.iter().flat_map(|l| &l.spans).any(|s| {
            s.content.contains("nested")
                && s.style
                    .add_modifier
                    .contains(Modifier::BOLD | Modifier::ITALIC)
        }));
    }
    #[test]
    fn wraps_lists_with_hanging_indent() {
        let lines = render("- one two three four five six", 12);
        assert_eq!(lines[0].to_string(), "• one two ");
        assert!(lines[1].to_string().starts_with("  three"));
        assert!(lines.iter().all(|l| l.width() <= 12));
    }
}
