//! Task 8: turn a markdown body into styled spans, wrapped to a width.
//!
//! Kept deliberately small: the detail pane needs headings, emphasis, inline
//! code, code blocks and links — not a full CommonMark renderer. Every piece of
//! text goes through [`crate::sanitize::sanitize`] before it becomes a span, so
//! a hostile PR body cannot emit escape sequences.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::sanitize::sanitize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Plain,
    Bold,
    Italic,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    /// A plain span. Text is sanitised here too: a public constructor that
    /// skipped it would become the way hostile text reaches the renderer.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: sanitize(&text.into()),
            style: Style::Plain,
        }
    }
}

/// A rendered line: one or more spans sharing the row.
pub type Line = Vec<Span>;

/// Render markdown to lines no wider than `width` columns.
pub fn render(markdown: &str, width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut builder = Builder::default();
    let parser = Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH);

    for event in parser {
        match event {
            Event::Start(tag) => builder.start(tag),
            Event::End(tag) => builder.end(tag),
            Event::Text(text) => builder.text(&text),
            Event::Code(text) => builder.code(&text),
            Event::SoftBreak | Event::HardBreak => builder.break_line(),
            Event::Rule => builder.rule(),
            _ => {}
        }
    }
    builder.finish(width)
}

#[derive(Default)]
struct Builder {
    lines: Vec<Line>,
    current: Line,
    bold: usize,
    italic: usize,
    in_code_block: bool,
    heading: bool,
    pending_link: Option<String>,
}

impl Builder {
    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::CodeBlock(_) => {
                self.flush();
                self.in_code_block = true;
            }
            Tag::Link { dest_url, .. } => self.pending_link = Some(dest_url.to_string()),
            Tag::Item => self.flush(),
            Tag::Heading { .. } => {
                self.flush();
                self.heading = true;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.flush();
                self.in_code_block = false;
            }
            TagEnd::Link => {
                if let Some(url) = self.pending_link.take() {
                    self.current.push(Span {
                        text: format!(" ({url})"),
                        style: Style::Plain,
                    });
                }
            }
            TagEnd::Paragraph | TagEnd::Item | TagEnd::BlockQuote(_) => self.flush(),
            TagEnd::Heading(_) => {
                self.heading = false;
                self.flush();
            }
            _ => {}
        }
    }

    fn style(&self) -> Style {
        if self.heading {
            return Style::Bold;
        }
        match (self.bold, self.italic) {
            (0, 0) => Style::Plain,
            (0, _) => Style::Italic,
            _ => Style::Bold,
        }
    }

    fn text(&mut self, text: &str) {
        let style = if self.in_code_block {
            Style::Code
        } else {
            self.style()
        };
        // A code block arrives as one event with embedded newlines: each of
        // them is a line of its own.
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.flush();
            }
            let clean = sanitize(part);
            if clean.is_empty() {
                continue;
            }
            self.current.push(Span { text: clean, style });
        }
    }

    fn code(&mut self, text: &str) {
        self.current.push(Span {
            text: sanitize(text),
            style: Style::Code,
        });
    }

    fn break_line(&mut self) {
        self.flush();
    }

    fn rule(&mut self) {
        self.flush();
        self.lines.push(vec![Span::plain("─".repeat(20))]);
    }

    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        // Wrapping happens once, in `finish`: wrapping twice would re-break
        // lines that already fit.
        let spans = std::mem::take(&mut self.current);
        self.lines.push(spans);
    }

    fn finish(mut self, width: usize) -> Vec<Line> {
        self.flush();
        self.lines
            .into_iter()
            .flat_map(|line| wrap(line, width))
            .collect()
    }
}

/// Split spans into lines of at most `width` columns, breaking on spaces and
/// only splitting inside a word when it is longer than the line.
fn wrap(spans: Line, width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Line = Vec::new();
    let mut used = 0usize;

    for span in spans {
        for piece in span.text.split_inclusive(' ') {
            // Count the trailing space too: a line that only fits because the
            // space was ignored would still be one column too wide.
            let visible = piece.chars().count();
            if used > 0 && used + visible > width {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            if visible > width {
                // A single word longer than the line: hard-split it.
                let mut chunk = String::new();
                for ch in piece.chars() {
                    if chunk.chars().count() == width {
                        current.push(Span {
                            text: std::mem::take(&mut chunk),
                            style: span.style,
                        });
                        lines.push(std::mem::take(&mut current));
                        used = 0;
                    }
                    chunk.push(ch);
                }
                if !chunk.is_empty() {
                    used += chunk.chars().count();
                    current.push(Span {
                        text: chunk,
                        style: span.style,
                    });
                }
                continue;
            }
            used += visible;
            current.push(Span {
                text: piece.to_string(),
                style: span.style,
            });
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    // Trailing spaces at a line break are invisible but count as columns.
    for line in &mut lines {
        while let Some(last) = line.last_mut() {
            let trimmed = last.text.trim_end().to_string();
            if trimmed.is_empty() {
                line.pop();
            } else {
                last.text = trimmed;
                break;
            }
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

/// Plain text of a rendered line, for tests and for the status line.
pub fn line_text(line: &Line) -> String {
    line.iter().map(|span| span.text.as_str()).collect()
}
