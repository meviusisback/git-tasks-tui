//! Rendering: one frame from [`App`] + view data.
//!
//! Everything displayed passes through [`crate::sanitize::sanitize`] first:
//! this is the boundary S13 talks about. Every colour comes from
//! [`crate::theme`]; no colour literal may appear here.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::{App, Pane};
use crate::icons;
use crate::markdown::{self as md};
use crate::model::ItemKind;
use crate::sanitize::sanitize;
use crate::theme;

/// What the UI needs to draw a frame. Owned strings keep the borrow simple.
/// `item_kinds` runs parallel to `items`: the kind colours the number without
/// changing the text any test reads.
#[derive(Default)]
pub struct ViewModel {
    pub repos: Vec<String>,
    pub items: Vec<String>,
    pub item_kinds: Vec<Option<ItemKind>>,
    pub detail: String,
    pub tabs: Vec<String>,
    pub active_tab: Option<usize>,
    pub agent: String,
    /// Last parsed screen of the embedded agent, row by row. Empty while
    /// idle: the panel is hidden then, so there is nothing to show.
    pub agent_screen: crate::embedded::AgentScreen,
}

/// The number colour by kind: green PRs, yellow issues. Only the number —
/// titles stay plain so the rows scan as text, not confetti.
fn kind_style(kind: Option<ItemKind>) -> Style {
    match kind {
        Some(ItemKind::Pr) => Style::default().fg(theme::OK),
        Some(ItemKind::Issue) => Style::default().fg(theme::WARN),
        None => Style::default(),
    }
}

/// Pane title with an icon when the host has a Nerd Font, plain otherwise.
fn pane_title(name: &str, icon: &str) -> String {
    if icon.is_empty() {
        format!(" {name} ")
    } else {
        format!(" {icon}{name} ")
    }
}

/// The keys that work today. Also the source of the `?` overlay.
const HELP_NOW: &[&str] = &[
    "← / →      move between the columns · wheel scrolls the agent box",
    "Shift + ←/↑/↓/→  move between the four boxes",
    "↑ / ↓      move the selection, or scroll detail/agent",
    "Enter      open a tab + open the detail box",
    "t / T      next / previous tab",
    "x / X      close the active tab, then the agent record",
    "w          focus the agent pane (worktree + agent instructions)",
    "Esc        back one level: overlay → agent → detail → list (never quits)",
    "R          repo selection menu (Space mark · a all · n none)",
    "o          open the selected item in the browser",
    "g          hand the selected PR to an agent (runs inside the agent box)",
    "/          filter the visible list",
    "r          refresh",
    "a          first action on the selected item",
    "m          actions menu: method and branch deletion are a choice",
    "?          this keymap",
    "q          quit",
    "Ctrl-C     quit — or interrupt the agent when it has focus",
];

/// Keys the overlay would lie about if they were listed above: nothing
/// happens yet, so they get their own section with the stage that brings
/// them, instead of reading as available today (X-1).
///
/// One list, two readers: the overlay formats from `PLANNED` and the test
/// iterates over `PLANNED` — so a newly promised key is watched from the
/// day it is promised, not from the day someone remembers the test.
/// `pub` (not `pub(crate)`) because the second reader is an integration
/// test, outside the crate.
///
/// `w` used to live here; it now focuses the agent pane, so only `s`
/// (sessions) is still promised.
pub const PLANNED: &[(char, &str)] = &[('s', "agent sessions (stage 16)")];

/// The outer agent box when shown, from the full frame area. Main uses the
/// same function to size the pty before drawing, so the terminal and its
/// box never disagree by a row.
pub fn agent_box(area: Rect) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(rows[0]);
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(columns[1]);
    right_rows[1]
}

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

    // Left column: repos on top, issues/PRs below. The filter follows the
    // repos cursor, so moving above re-lists below live.
    let left_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(columns[0]);
    let (repos_widget, mut repos_state) = repos_list(app, view);
    frame.render_stateful_widget(repos_widget, left_rows[0], &mut repos_state);
    let (issues_widget, mut issues_state) = issues_list(app, view);
    frame.render_stateful_widget(issues_widget, left_rows[1], &mut issues_state);

    // Right column: tabs + detail on top, agent/worktree at the bottom —
    // but only while an agent is being arranged or is running, or while the
    // pane has focus (so `w`/`Tab` still reach the instructions when idle).
    // Idle and unfocused screens give the whole column to the detail.
    if app.agent_active() || app.pane == Pane::Agent {
        let right_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(columns[1]);
        // The tabs are one line, not a box: at 80x12 every row counts, and a
        // bordered strip would eat the detail it labels.
        let detail_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(right_rows[0]);
        frame.render_widget(tabs_bar(app, view), detail_rows[0]);
        frame.render_widget(detail(app, view, detail_rows[1].width), detail_rows[1]);
        frame.render_widget(agent_panel(app, view, right_rows[1].width), right_rows[1]);
    } else {
        let detail_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(columns[1]);
        frame.render_widget(tabs_bar(app, view), detail_rows[0]);
        frame.render_widget(detail(app, view, detail_rows[1].width), detail_rows[1]);
    }

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
    if let Some(crate::app::Modal::ActionMenu(menu)) = &app.modal {
        draw_action_menu(frame, frame.area(), menu);
    }
    if let Some(crate::app::Modal::Notice(notice)) = &app.modal {
        draw_notice(frame, frame.area(), notice);
    }
}

/// Build what the frame needs from the current state: the repo column, the
/// visible items (search applied), the detail of the active tab (or the
/// selected row when no tab is open), the tab labels and the agent pane.
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
    //
    // The detail follows the active tab when one is open, otherwise the row
    // under the list cursor — so a tab keeps its issue/PR while the list
    // moves underneath it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The record card replaces the detail for its own item: the last agent
    // run is shown where the detail would be, never beside a PR it is not for.
    let record_card = app.visible_record().map(|record| record.card());
    let detail = app
        .effective_item()
        .map(|item| {
            if let Some(card) = &record_card {
                return card.clone();
            }
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
    let _ = visible;
    let item_kinds = app
        .visible_items()
        .iter()
        .map(|item| Some(item.kind))
        .collect();
    ViewModel {
        repos: app.repos.iter().map(|row| row.text().to_string()).collect(),
        items,
        item_kinds,
        detail,
        tabs: app.tab_labels(),
        active_tab: app.active_tab,
        agent: app.agent_panel_text(),
        agent_screen: app.agent_screen.clone(),
    }
}

/// Left column, top: the repos. `▸ repo (n)` rows with per-repo counts —
/// a folder icon when the host has a Nerd Font.
fn repos_list(app: &App, view: &ViewModel) -> (List<'static>, ListState) {
    let counts: Vec<usize> = view
        .repos
        .iter()
        .map(|repo| app.items.iter().filter(|item| &item.repo == repo).count())
        .collect();
    let rows: Vec<String> = view
        .repos
        .iter()
        .zip(counts.iter().chain(std::iter::repeat(&0)))
        .map(|(repo, count)| format!("{}{repo} ({count})", icons::repo()))
        .collect();
    let selected = if rows.is_empty() {
        None
    } else {
        Some(app.repos_cursor.min(rows.len().saturating_sub(1)))
    };
    let highlight = app.pane == Pane::Repos;
    let focused = highlight;
    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|entry| ListItem::new(sanitize(entry.as_str())))
        .collect();
    let widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(pane_title("repos", ""))
                .border_style(if focused {
                    theme::border_focused()
                } else {
                    theme::border()
                })
                .title_style(theme::title(focused)),
        )
        .highlight_style(theme::highlight());
    let state = ListState::default().with_selected(highlight.then_some(selected).flatten());
    (widget, state)
}

/// Split a `"#12 title"` row into its number and title parts. Rows built
/// without a title keep the whole text as the number.
fn split_number_title(entry: &str) -> (String, String) {
    match entry.split_once(' ') {
        Some((number, title)) => (number.to_string(), title.to_string()),
        None => (entry.to_string(), String::new()),
    }
}

/// Left column, bottom: the issues/PRs of the repo under the cursor.
/// The repo filter follows the repos cursor, so this box always shows the
/// repo above — no `R` menu needed for the common case. The number carries
/// the kind colour (green PR, yellow issue); the title stays plain.
fn issues_list(app: &App, view: &ViewModel) -> (List<'static>, ListState) {
    let selected = if view.items.is_empty() {
        None
    } else {
        Some(app.list_cursor.min(view.items.len().saturating_sub(1)))
    };
    let highlight = app.pane == Pane::List;
    let focused = highlight;
    let items: Vec<ListItem> = view
        .items
        .iter()
        .zip(view.item_kinds.iter().chain(std::iter::repeat(&None)))
        .map(|(entry, kind)| {
            let (number, title) = split_number_title(entry);
            let icon = match kind {
                Some(ItemKind::Pr) => icons::kind_pr(),
                Some(ItemKind::Issue) => icons::kind_issue(),
                None => "",
            };
            let mut spans = vec![
                Span::styled(icon, kind_style(*kind)),
                Span::styled(sanitize(&number), kind_style(*kind)),
                Span::raw(" "),
                Span::raw(sanitize(&title)),
            ];
            if icon.is_empty() {
                spans.remove(0);
            }
            ListItem::new(Line::from(spans))
        })
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
    let state = ListState::default().with_selected(highlight.then_some(selected).flatten());
    (widget, state)
}

/// Right column, top strip: the open tabs on a single line, so small
/// terminals keep their detail rows. The active tab is bold, the rest dim;
/// the number carries the kind colour (green PR, yellow issue). No prefix,
/// no keys — the legend below owns those.
fn tabs_bar(app: &App, view: &ViewModel) -> Paragraph<'static> {
    if view.tabs.is_empty() {
        let style = theme::title(app.pane == Pane::Detail);
        return Paragraph::new("no tabs — Enter opens one").style(style);
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, info) in app.tab_infos().iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  │  "));
        }
        let mark = if info.active { "●" } else { "○" };
        let mark_style = if info.active {
            Style::default()
                .fg(theme::FOCUSED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::SUBTEXT)
        };
        let icon = match info.kind {
            Some(ItemKind::Pr) => icons::kind_pr(),
            Some(ItemKind::Issue) => icons::kind_issue(),
            None => "",
        };
        spans.push(Span::styled(mark.to_string(), mark_style));
        spans.push(Span::raw(" "));
        if !icon.is_empty() {
            spans.push(Span::styled(icon.to_string(), kind_style(info.kind)));
        }
        let (number, title) = split_number_title(&info.label);
        spans.push(Span::styled(sanitize(&number), kind_style(info.kind)));
        if !title.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::raw(sanitize(&title)));
        }
    }
    Paragraph::new(Line::from(spans))
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

/// Right column, bottom: the live agent terminal while it runs, otherwise
/// the worktree + agent instructions. The live screen is the parsed pty
/// grid, cell by cell; untrusted bytes were already neutralised by the
/// parser, and every cell is sanitised again at this boundary.
fn agent_panel(app: &App, view: &ViewModel, width: u16) -> Paragraph<'static> {
    let text: Vec<ratatui::text::Line<'static>> = if view.agent_screen.is_empty() {
        agent_instructions(&view.agent, width)
    } else {
        view.agent_screen
            .iter()
            .map(|row| {
                let spans: Vec<ratatui::text::Span<'static>> = row
                    .iter()
                    .map(|cell| {
                        let style = theme::agent_cell(cell.fg, cell.bg, cell.bold, cell.inverse);
                        ratatui::text::Span::styled(sanitize(&cell.text), style)
                    })
                    .collect();
                ratatui::text::Line::from(spans)
            })
            .collect()
    };
    let focused = app.pane == Pane::Agent;
    Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(pane_title("agent", icons::agent()))
                .border_style(if focused {
                    theme::border_focused()
                } else {
                    theme::border()
                })
                .title_style(theme::title(focused)),
        )
        .scroll((app.agent_scroll, 0))
}

/// The idle agent box: the worktree + agent instructions, rendered from the
/// same markdown pipeline as the detail so bold stays bold.
fn agent_instructions(text: &str, width: u16) -> Vec<ratatui::text::Line<'static>> {
    let lines = md::render(text, width.saturating_sub(2).max(1) as usize);
    lines
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
        .collect()
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
    let area = centered_rect(75, 92, area);
    frame.render_widget(Clear, area);
    let text: Vec<ratatui::text::Line<'static>> = HELP_NOW
        .iter()
        .map(|line| ratatui::text::Line::from(sanitize(line)))
        .chain(std::iter::once(ratatui::text::Line::from("")))
        .chain(std::iter::once(ratatui::text::Line::from(sanitize(
            "coming in the next stages:",
        ))))
        .chain(PLANNED.iter().map(|(key, what)| {
            ratatui::text::Line::from(sanitize(&format!("{key}          {what}")))
        }))
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
                .title(" repos — Space mark · a all · n none · Enter apply · Esc cancel ")
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .highlight_style(theme::highlight());
    let mut state = ListState::default().with_selected(Some(menu.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_action_menu(frame: &mut Frame, area: Rect, menu: &crate::app::ActionMenu) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mark = if i == menu.cursor { "▸" } else { " " };
            ListItem::new(sanitize(&format!("{mark} {}", row.text())))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" action — ↑/↓ choose · Enter confirm · Esc cancel ")
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .highlight_style(theme::highlight());
    let mut state = ListState::default().with_selected(Some(menu.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

/// A startup blocker: the config on disk cannot be used and the app runs on
/// the demo fallback. A centered box, not just the legend row — at 80 columns
/// a long path would push the remedy ("Esc dismiss") off the edge, which is
/// exactly the silence this notice replaces.
fn draw_notice(frame: &mut Frame, area: Rect, notice: &str) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" config problem ")
        .border_style(theme::alert())
        .title_style(theme::alert())
        .style(theme::overlay_text());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The remedy is anchored to the bottom of the box instead of being the message's last line:
    // a centered box protects the remedy from a long *path*, but a long *message* pushes it out
    // of the box vertically — the same silence, rotated ninety degrees. A TOML parse error
    // repeats the offending line, so the longest message this program can produce is ~1000
    // characters; at 80x24 the box holds 17 lines of 54 columns, and the remedy was the line
    // that fell off.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let message = Paragraph::new(ratatui::text::Line::from(sanitize(notice)))
        .style(theme::overlay_text())
        .wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(message, rows[0]);
    let remedy = Paragraph::new(ratatui::text::Line::from(sanitize("Esc dismiss · q quit")))
        .style(theme::overlay_text());
    frame.render_widget(remedy, rows[1]);
}
