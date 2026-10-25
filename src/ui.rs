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
    /// One per item: a worktree directory with a `.git` entry exists for
    /// the item's branch. The marker in the issues list.
    pub item_worktrees: Vec<bool>,
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
    "Enter      open a tab + open the detail box · d diff",
    "t / T      next / previous tab",
    "x / X      close the active tab, then the agent record",
    "w          focus the agent pane (worktree + agent instructions)",
    "Esc        back one level: overlay → agent → detail → list (never quits)",
    "R          repo selection menu (Space mark · a all · n none)",
    "o          open the selected item in the browser",
    "g / W      agent + worktree for the PR · worktrees alone",
    "/          filter the visible list",
    "r          refresh",
    "a          first action on the selected item",
    "m          actions menu: method and branch deletion are a choice",
    "?          this keymap · s settings",
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
/// `w` used to live here, then `R`, then `s` (now the settings menu): each
/// left the day its menu landed. Empty today — the header below says so
/// instead of dangling.
pub const PLANNED: &[(char, &str)] = &[];

/// The right column (detail + agent) from the full frame area: outer
/// border, status row and the left 30% removed. Draw and mouse clicks share
/// it, so a click can never target a row the boxes are not on.
pub fn right_column(area: Rect) -> Rect {
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(rows[0]);
    columns[1]
}

/// The one-line tabs strip: the top row of the right column. Draw renders
/// the strip here and mouse clicks are mapped here — one function, so the
/// two can never disagree by a row.
pub fn tabs_row(area: Rect) -> Rect {
    let right = right_column(area);
    Rect::new(right.x, right.y, right.width, 1)
}

/// The outer agent box when shown, from the full frame area. Main uses the
/// same function to size the pty before drawing, so the terminal and its
/// box never disagree by a row.
pub fn agent_box(area: Rect) -> Rect {
    let right = right_column(area);
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(right);
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
    // Geometry comes from `right_column`/`tabs_row`, the same functions the
    // mouse click mapping uses.
    let right = right_column(frame.area());
    let tabs = tabs_row(frame.area());
    let detail_below_tabs = Rect::new(
        right.x,
        tabs.y.saturating_add(1),
        right.width,
        right.height.saturating_sub(1),
    );
    if app.agent_active() || app.pane == Pane::Agent {
        let right_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(right);
        // The tabs are one line, not a box: at 80x12 every row counts, and a
        // bordered strip would eat the detail it labels.
        frame.render_widget(tabs_bar(app, view), tabs);
        let detail_area = Rect::new(
            right.x,
            tabs.y.saturating_add(1),
            right.width,
            right_rows[0].height.saturating_sub(1),
        );
        frame.render_widget(detail(app, view, detail_area.width), detail_area);
        frame.render_widget(agent_panel(app, view, right_rows[1].width), right_rows[1]);
    } else {
        frame.render_widget(tabs_bar(app, view), tabs);
        frame.render_widget(
            detail(app, view, detail_below_tabs.width),
            detail_below_tabs,
        );
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
    if let Some(crate::app::Modal::AgentMenu(menu)) = &app.modal {
        draw_agent_menu(frame, frame.area(), menu, app);
    }
    if let Some(crate::app::Modal::Settings(menu)) = &app.modal {
        draw_settings(frame, frame.area(), menu, app);
    }
    if let Some(crate::app::Modal::Worktrees(menu)) = &app.modal {
        draw_worktrees_menu(frame, frame.area(), menu);
    }
    if let Some(crate::app::Modal::NameInput(input)) = &app.modal {
        draw_name_input(frame, frame.area(), input);
    }
    if matches!(app.modal, Some(crate::app::Modal::Diff)) {
        draw_diff(frame, frame.area(), app);
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
    let item_worktrees = visible
        .iter()
        .map(|item| app.has_worktree(&item.repo, item.head_ref_name.as_deref()))
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
        item_worktrees,
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
    let icon = icons::repo();
    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|entry| {
            // The icon gets the directory colour; the name stays plain —
            // one coloured glyph per row, never a coloured sentence.
            match entry.strip_prefix(icon) {
                Some(rest) => ListItem::new(Line::from(vec![
                    Span::styled(icon, theme::repo_icon()),
                    Span::raw(sanitize(rest)),
                ])),
                None => ListItem::new(sanitize(entry.as_str())),
            }
        })
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
/// repo above — no `R` menu needed for the common case. The number carries the
/// kind colour (green PR, yellow issue); the title stays plain. A worktree
/// marker (`⎇` with a Nerd Font, `◉` without) in front of the number says a
/// worktree directory exists for the item's branch: `g` then works on it.
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
        .zip(view.item_worktrees.iter().chain(std::iter::repeat(&false)))
        .map(|((entry, kind), has_wt)| {
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
            if *has_wt {
                let marker = icons::worktree();
                let marker = if marker.is_empty() { "◉ " } else { marker };
                spans.insert(0, Span::styled(marker, theme::worktree_marker()));
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

/// One visual chunk of the tabs strip: text, style, and the tab it belongs
/// to (`None` for separators). Both the render and the mouse-click mapping
/// walk this same list, so a click can never land on a tab the drawing did
/// not show.
struct TabChunk {
    text: String,
    style: Style,
    tab: Option<usize>,
}

fn tab_chunks(app: &App) -> Vec<TabChunk> {
    let mut chunks: Vec<TabChunk> = Vec::new();
    for (i, info) in app.tab_infos().iter().enumerate() {
        if i > 0 {
            chunks.push(TabChunk {
                text: "  │  ".to_string(),
                style: Style::default(),
                tab: None,
            });
        }
        let mark = if info.active { "●" } else { "○" };
        let mark_style = if info.active {
            Style::default()
                .fg(theme::FOCUSED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::SUBTEXT)
        };
        chunks.push(TabChunk {
            text: mark.to_string(),
            style: mark_style,
            tab: Some(i),
        });
        chunks.push(TabChunk {
            text: " ".to_string(),
            style: Style::default(),
            tab: Some(i),
        });
        let icon = match info.kind {
            Some(ItemKind::Pr) => icons::kind_pr(),
            Some(ItemKind::Issue) => icons::kind_issue(),
            None => "",
        };
        if !icon.is_empty() {
            chunks.push(TabChunk {
                text: icon.to_string(),
                style: kind_style(info.kind),
                tab: Some(i),
            });
        }
        let (number, title) = split_number_title(&info.label);
        chunks.push(TabChunk {
            text: sanitize(&number),
            style: kind_style(info.kind),
            tab: Some(i),
        });
        if !title.is_empty() {
            chunks.push(TabChunk {
                text: " ".to_string(),
                style: Style::default(),
                tab: Some(i),
            });
            chunks.push(TabChunk {
                text: sanitize(&title),
                style: Style::default(),
                tab: Some(i),
            });
        }
    }
    chunks
}

/// Which tab sits under a mouse click, if any. Pure over the app (no
/// terminal needed), so tests pin the mapping: separators and the empty
/// strip map to `None` — a click between tabs switches nothing.
pub fn tab_at(app: &App, area: Rect, x: u16, y: u16) -> Option<usize> {
    use unicode_width::UnicodeWidthStr;
    let row = tabs_row(area);
    if y != row.y || x < row.x {
        return None;
    }
    let end = row.x.saturating_add(row.width);
    if x >= end {
        return None;
    }
    let mut cursor = row.x;
    for chunk in tab_chunks(app) {
        let width = chunk.text.width() as u16;
        if x >= cursor && x < cursor.saturating_add(width) {
            return chunk.tab;
        }
        cursor = cursor.saturating_add(width);
        if cursor >= end {
            break;
        }
    }
    None
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
    let spans: Vec<Span<'static>> = tab_chunks(app)
        .into_iter()
        .map(|chunk| Span::styled(chunk.text, chunk.style))
        .collect();
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
        .chain(if PLANNED.is_empty() {
            vec![ratatui::text::Line::from(sanitize("  (nothing promised)"))]
        } else {
            PLANNED
                .iter()
                .map(|(key, what)| {
                    ratatui::text::Line::from(sanitize(&format!("{key}          {what}")))
                })
                .collect()
        })
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

/// The `g` menu: one row per installed agent binary, hermes carrying its
/// profile. Same centered shape as the actions menu, so all pickers read
/// the same.
fn draw_agent_menu(
    frame: &mut Frame,
    area: Rect,
    menu: &crate::app::AgentMenu,
    app: &crate::app::App,
) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mark = if i == menu.cursor { "▸" } else { " " };
            // Sandboxed launches are YOLO inside containment: the suffix
            // says so on the row itself, before anything is confirmed.
            let label = if app.sandbox_agents {
                format!("{} · sandboxed", row.label(&menu.plan.profile))
            } else {
                row.label(&menu.plan.profile)
            };
            ListItem::new(sanitize(&format!("{mark} {label}")))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" agent — ↑/↓ choose · Enter confirm · Esc cancel ")
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .highlight_style(theme::highlight());
    let mut state = ListState::default().with_selected(Some(menu.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The worktree picker: one row per existing tree (branch — path, the
/// current item's branch marked), chosen with `Enter`, managed with
/// `n`/`r`/`d`.
fn draw_worktrees_menu(frame: &mut Frame, area: Rect, menu: &crate::app::WorktreeMenu) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mark = if i == menu.cursor { "▸" } else { " " };
            let current = if row.branch.as_str() == menu.plan.branch {
                " · this PR"
            } else {
                ""
            };
            ListItem::new(sanitize(&format!(
                "{mark} {} — {}{current}",
                row.branch,
                row.path.display()
            )))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " worktrees for {} — Enter use · n new · r rename · d delete · Esc cancel ",
                    menu.plan.repo,
                ))
                .border_style(theme::overlay())
                .title_style(theme::overlay()),
        )
        .highlight_style(theme::highlight());
    let mut state = ListState::default().with_selected(Some(menu.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The diff overlay: the PR's unified diff, one screenful at the scroll
/// offset, additions green, removals red, hunk headers bright. Long lines
/// are clipped by the widget (code reads better unwrapped than rewrapped).
/// A truncation note leads when the fetch kept only the head.
fn draw_diff(frame: &mut Frame, area: Rect, app: &crate::app::App) {
    let area = centered_rect(88, 88, area);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " diff {} — ↑/↓ scroll · Esc close ",
            app.diff_for
                .as_ref()
                .map(|(repo, number)| format!("{repo}#{number}"))
                .unwrap_or_default(),
        ))
        .border_style(theme::overlay())
        .title_style(theme::overlay())
        .style(theme::overlay_text());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut rows: Vec<Line<'static>> = Vec::new();
    if app.diff_truncated {
        rows.push(Line::from(Span::styled(
            format!(
                "…showing the first {} of {} lines…",
                crate::app::App::DIFF_MAX_LINES,
                app.diff_total
            ),
            Style::default().fg(theme::SUBTEXT),
        )));
    }
    rows.extend(app.diff_text.lines().skip(app.diff_scroll).map(|line| {
        let text = sanitize(line);
        let style = if line.starts_with('+') && !line.starts_with("+++") {
            Style::default().fg(theme::OK)
        } else if line.starts_with('-') && !line.starts_with("---") {
            Style::default().fg(theme::ALERT)
        } else if line.starts_with("@@") {
            Style::default()
                .fg(theme::FOCUSED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(Span::styled(text, style))
    }));
    let visible = Paragraph::new(rows);
    frame.render_widget(visible, inner);
}

/// The settings menu: one row per toggle with its state, the selected
/// row's description underneath. Same centered shape as the other pickers,
/// so all menus read the same.
fn draw_settings(
    frame: &mut Frame,
    area: Rect,
    menu: &crate::app::SettingsMenu,
    app: &crate::app::App,
) {
    let area = centered_rect(70, 80, area);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(i, setting)| {
            let mark = if i == menu.cursor { "▸" } else { " " };
            let state = if setting.is_on(app) { "[on]" } else { "[off]" };
            ListItem::new(sanitize(&format!("{mark} {} {state}", setting.title())))
        })
        .collect();
    let rows = items.len().max(1);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" settings "))
        .highlight_style(theme::highlight());
    frame.render_widget(list, area);
    if let Some(selected) = menu.rows.get(menu.cursor) {
        let note = Paragraph::new(sanitize(selected.description()))
            .style(Style::default().fg(theme::SUBTEXT));
        let note_area = Rect::new(
            area.x.saturating_add(2),
            area.y.saturating_add(rows as u16).saturating_add(2),
            area.width.saturating_sub(4),
            2,
        );
        frame.render_widget(note, note_area);
    }
}

/// The name prompt: create or rename, with the suggestion prepopulated so
/// confirming untouched keeps the obvious choice. Append-only editing, like
/// the search box.
fn draw_name_input(frame: &mut Frame, area: Rect, input: &crate::app::NameInput) {
    let area = centered_rect(70, 25, area);
    frame.render_widget(Clear, area);
    let title = match input.purpose {
        crate::app::NamePurpose::Create { .. } => {
            " name the worktree — Enter confirm · Esc cancel "
        }
        crate::app::NamePurpose::Rename { .. } => {
            " rename the worktree — Enter confirm · Esc cancel "
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(theme::overlay())
        .title_style(theme::overlay())
        .style(theme::overlay_text());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let value = Paragraph::new(ratatui::text::Line::from(sanitize(&input.value)))
        .style(theme::overlay_text());
    frame.render_widget(value, inner);
}
