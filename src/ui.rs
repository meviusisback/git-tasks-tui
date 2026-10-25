//! Rendering: one frame from [`App`] + view data.
//!
//! Everything displayed passes through [`crate::sanitize::sanitize`] first:
//! this is the boundary S13 talks about. Every colour comes from
//! [`crate::theme`]; no colour literal may appear here.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::{App, Pane};
use crate::markdown::{self as md};
use crate::sanitize::sanitize;
use crate::theme;

/// What the UI needs to draw a frame. Owned strings keep the borrow simple.
#[derive(Default)]
pub struct ViewModel {
    pub repos: Vec<String>,
    pub items: Vec<String>,
    pub detail: String,
}

/// The keys that work today. Also the source of the `?` overlay.
const HELP_NOW: &[&str] = &[
    "← / →      move between the left column and the right column",
    "↑ / ↓      move the selection in the left column",
    "Enter      open the detail box",
    "Esc        back one level: overlay → detail → list (never quits)",
    "R          repo selection menu",
    "o          open the selected item in the browser",
    "/          filter the visible list",
    "r          refresh",
    "a          first action on the selected item",
    "?          this keymap",
    "q          quit (with a confirmation if the agent is running)",
];

/// Keys the overlay would lie about if they were listed above: nothing
/// happens yet, so they get their own section with the stage that brings
/// them, instead of reading as available today (X-1).
const HELP_NEXT: &[&str] = &[
    "m          actions menu (stage 14)",
    "w          worktree menu (stage 15)",
    "s          agent sessions (stage 16)",
];

pub fn draw(frame: &mut Frame, app: &App, view: &ViewModel) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" gtasks ")
        .border_style(theme::border());
    let inner = outer.inner(frame.area());
    frame.render_widget(outer, frame.area());

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(rows[0]);

    let (list, mut list_state) = left_list(app, view);
    frame.render_stateful_widget(list, columns[0], &mut list_state);

    frame.render_widget(detail(app, view, columns[1].width), columns[1]);

    // Two panels, so the status cannot be pushed off the right edge by the
    // legend. The legend keeps the left side and truncates there: the recovery
    // keys (`q`, `?`) are first, so what is lost is the least important part.
    match app.status_text() {
        Some(status) => {
            let bottom = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
                .split(rows[1]);
            frame.render_widget(legend(app), bottom[0]);
            frame.render_widget(status_line(&status), bottom[1]);
        }
        None => frame.render_widget(legend(app), rows[1]),
    }

    if app.show_help {
        draw_help(frame, frame.area());
    }

    if let Some(crate::app::Modal::RepoMenu(menu)) = &app.modal {
        draw_repo_menu(frame, frame.area(), menu);
    }
}

/// Build what the frame needs from the current state: the repo column, the
/// visible items (search applied) and the detail of the selected one.
pub fn view_for(app: &App) -> ViewModel {
    let visible = app.visible_items();
    let items = visible
        .iter()
        .map(|item| format!("#{} {}", item.number, item.title))
        .collect();
    // Structured fields from the item, always; the fetched body below them
    // only when it belongs to this item. A missing field is omitted, never
    // blank — and a missing body says how to fetch it instead of leaving a
    // blank zone ("Enter — body").
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let detail = visible
        .get(app.list_cursor)
        .map(|item| {
            let mut text = item.detail_fields(now).join("\n");
            let key = (item.repo.clone(), item.number);
            if app.detail_for.as_ref() == Some(&key) && !app.detail.trim().is_empty() {
                text.push_str("\n\n");
                text.push_str(&app.detail);
            } else if app.detail_requested.as_ref() != Some(&key) {
                text.push_str("\n\nEnter — body");
            }
            text
        })
        .unwrap_or_default();
    ViewModel {
        repos: app.repos.iter().map(|row| row.text().to_string()).collect(),
        items,
        detail,
    }
}

/// Rows of the left column: one `▸ repo (n)` row per repo, then the items.
/// The highlight follows the focused pane; the right column never highlights.
fn left_rows(view: &ViewModel, counts: &[usize]) -> Vec<String> {
    let mut rows = Vec::with_capacity(view.repos.len() + view.items.len());
    for (repo, count) in view
        .repos
        .iter()
        .zip(counts.iter().chain(std::iter::repeat(&0)))
    {
        rows.push(format!("▸ {repo} ({count})"));
    }
    rows.extend(view.items.iter().cloned());
    rows
}

fn left_list(app: &App, view: &ViewModel) -> (List<'static>, ListState) {
    let counts: Vec<usize> = view
        .repos
        .iter()
        .map(|repo| app.items.iter().filter(|item| &item.repo == repo).count())
        .collect();
    let rows = left_rows(view, &counts);
    let selected = match app.pane {
        Pane::Repos => Some(app.repos_cursor.min(rows.len().saturating_sub(1))),
        Pane::List => {
            let idx = view.repos.len() + app.list_cursor;
            if view.items.is_empty() {
                None
            } else {
                Some(idx.min(rows.len().saturating_sub(1)))
            }
        }
        Pane::Detail => None,
    };
    let focused = app.pane != Pane::Detail;
    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|entry| ListItem::new(sanitize(entry.as_str())))
        .collect();
    let widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" list ")
                .border_style(if focused {
                    theme::border_focused()
                } else {
                    theme::border()
                })
                .title_style(theme::title(focused)),
        )
        .highlight_style(theme::highlight());
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
                        md::Style::Plain => theme::overlay_text(),
                        md::Style::Bold => theme::overlay_text().add_modifier(Modifier::BOLD),
                        md::Style::Italic => theme::overlay_text().add_modifier(Modifier::ITALIC),
                        md::Style::Code => theme::overlay_text().add_modifier(Modifier::DIM),
                    };
                    ratatui::text::Span::styled(span.text, style)
                })
                .collect();
            ratatui::text::Line::from(spans)
        })
        .collect();
    let focused = app.pane == Pane::Detail;
    Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" detail ")
                .border_style(if focused {
                    theme::border_focused()
                } else {
                    theme::border()
                })
                .title_style(theme::title(focused)),
        )
        .scroll((app.detail_scroll, 0))
}

fn legend(app: &App) -> Paragraph<'static> {
    Paragraph::new(sanitize(app.legend().as_str())).style(theme::legend())
}

fn status_line(status: &str) -> Paragraph<'static> {
    // A visible separator: without it the truncated legend runs into the
    // status ("nothing to clonothing selected").
    Paragraph::new(format!("│ {status}")).style(theme::status())
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let text: Vec<ratatui::text::Line<'static>> = HELP_NOW
        .iter()
        .map(|line| ratatui::text::Line::from(sanitize(line)))
        .chain(std::iter::once(ratatui::text::Line::from("")))
        .chain(std::iter::once(ratatui::text::Line::from(sanitize(
            "coming in the next stages:",
        ))))
        .chain(
            HELP_NEXT
                .iter()
                .map(|line| ratatui::text::Line::from(sanitize(line))),
        )
        .collect();
    let help = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" keys — any key closes ")
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .style(theme::overlay_text());
    frame.render_widget(help, area);
}

fn draw_repo_menu(frame: &mut Frame, area: Rect, menu: &crate::app::RepoMenu) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mark = if menu.marked.contains(&i) {
                "[x]"
            } else {
                "[ ]"
            };
            ListItem::new(sanitize(&format!("{mark} {}", row.as_str())))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" repos — Space mark · Enter apply · Esc cancel ")
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .highlight_style(theme::highlight());
    let mut state = ListState::default().with_selected(Some(menu.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}
