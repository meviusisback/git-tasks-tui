//! Rendering: one function from [`App`] + view data to a frame.
//!
//! Everything displayed passes through [`crate::sanitize::sanitize`] first:
//! this is the boundary S13 talks about.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, Modal, Pane};
use crate::markdown::{self as md};
use crate::sanitize::sanitize;

/// What the UI needs to draw a frame. Owned strings keep the borrow simple.
#[derive(Default)]
pub struct ViewModel {
    pub repos: Vec<String>,
    pub items: Vec<String>,
    pub detail: String,
}

pub fn draw(frame: &mut Frame, app: &App, view: &ViewModel) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(Paragraph::new(header(app)), outer[0]);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(38),
            Constraint::Min(20),
        ])
        .split(outer[1]);

    let (repos, mut repos_state) = list_widget(app, &view.repos, app.repos_cursor, Pane::Repos);
    frame.render_stateful_widget(repos, columns[0], &mut repos_state);

    let (items, mut items_state) = list_widget(app, &view.items, app.list_cursor, Pane::List);
    frame.render_stateful_widget(items, columns[1], &mut items_state);

    frame.render_widget(detail(app, view, columns[2].width), columns[2]);
    frame.render_widget(Paragraph::new(footer(app)), outer[2]);
}

/// Build what the frame needs from the current state: the repo column, the
/// visible items (search applied) and the detail of the selected one.
pub fn view_for(app: &App) -> ViewModel {
    let visible = app.visible_items();
    let items = visible
        .iter()
        .map(|item| format!("#{} {}", item.number, item.title))
        .collect();
    let detail = if app.detail.trim().is_empty() {
        visible
            .get(app.list_cursor)
            .map(|item| {
                format!(
                    "#{} {}\n\nstate: {}\nrepo: {}\nurl: {}",
                    item.number, item.title, item.state, item.repo, item.url
                )
            })
            .unwrap_or_default()
    } else {
        app.detail.clone()
    };
    ViewModel {
        repos: app.repos.clone(),
        items,
        detail,
    }
}

fn header(app: &App) -> String {
    let mut text = format!("gtasks — {:?}", app.pane);
    if app.searching {
        text.push_str(&format!("  /{}", sanitize(&app.query)));
    } else if !app.query.is_empty() {
        text.push_str(&format!("  [{}]", sanitize(&app.query)));
    }
    text
}

fn footer(app: &App) -> String {
    if let Some(Modal::Confirm(question)) = app.modal {
        return format!("{}  —  y confirm · n/Esc cancel", sanitize(question));
    }
    if app.status.is_empty() {
        "/ search · Tab pane · ↑↓ move · Enter open · q quit".to_string()
    } else {
        sanitize(&app.status)
    }
}

fn list_widget<'a>(
    app: &App,
    entries: &'a [String],
    cursor: usize,
    pane: Pane,
) -> (List<'a>, ListState) {
    let items: Vec<ListItem> = entries
        .iter()
        .map(|entry| ListItem::new(sanitize(entry)))
        .collect();
    let selected = if app.pane == pane && !entries.is_empty() {
        Some(cursor.min(entries.len() - 1))
    } else {
        None
    };
    let title = match pane {
        Pane::Repos => "repos",
        Pane::List => "items",
        Pane::Detail => "",
    };
    let widget = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    (widget, ListState::default().with_selected(selected))
}

fn detail(app: &App, view: &ViewModel, width: u16) -> Paragraph<'static> {
    // Markdown is rendered at the column width, so the wrapping is real: the
    // paragraph must not wrap again.
    let lines = md::render(&view.detail, width.saturating_sub(2).max(1) as usize);
    let text: Vec<ratatui::text::Line<'static>> = lines
        .into_iter()
        .map(|line| {
            let spans: Vec<ratatui::text::Span<'static>> = line
                .into_iter()
                .map(|span| {
                    let style = match span.style {
                        md::Style::Plain => Style::default(),
                        md::Style::Bold => Style::default().add_modifier(Modifier::BOLD),
                        md::Style::Italic => Style::default().add_modifier(Modifier::ITALIC),
                        md::Style::Code => Style::default().add_modifier(Modifier::DIM),
                    };
                    ratatui::text::Span::styled(span.text, style)
                })
                .collect();
            ratatui::text::Line::from(spans)
        })
        .collect();
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("detail"))
        .scroll((app.detail_scroll, 0))
}
