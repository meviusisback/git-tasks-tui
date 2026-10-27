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
    /// The open trees behind the shown issue/PR, if any: branch, path, who
    /// ran there last, live now or not. Rendered as a WORKTREES section.
    pub trees: Vec<crate::app::ItemTree>,
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

/// The worktree marker text, everywhere the same: the folder-tree glyph
/// with a Nerd Font, `◉` without (both end with their own space). Mauve at
/// every site, via `theme::worktree_marker`.
fn worktree_marker_text() -> &'static str {
    let marker = icons::worktree();
    if marker.is_empty() { "◉ " } else { marker }
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
    "g          agent + worktree for the selected PR",
    "W          worktrees: use one on a PR · create one on a repo",
    "/          filter the visible list · r refresh",
    "a          first action on the selected item",
    "m          actions menu (list only): method and branch deletion are a choice",
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

/// Which box a click lands in, if any. `agent_shown` mirrors the draw:
/// when the active tab rents no agent box, the whole lower right is
/// detail, so a click there focuses detail instead of a hidden pane.
pub fn pane_at(g: &Grid, x: u16, y: u16, agent_shown: bool) -> Option<crate::app::Pane> {
    let inside = |r: &Rect| {
        x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
    };
    if inside(&g.repos) {
        Some(crate::app::Pane::Repos)
    } else if inside(&g.list) {
        Some(crate::app::Pane::List)
    } else if inside(&g.detail) || inside(&g.tabs) {
        Some(crate::app::Pane::Detail)
    } else if inside(&g.agent) {
        Some(if agent_shown {
            crate::app::Pane::Agent
        } else {
            crate::app::Pane::Detail
        })
    } else {
        None
    }
}

/// Left width for a vertical-divider drag at `x`, in percent — `None`
/// when the body is too small or the cursor would leave a sliver box.
/// Only the cross divider moves; single boxes are never sized directly.
pub fn col_pct_for(body: Rect, x: u16) -> Option<u16> {
    let left = x.saturating_sub(body.x);
    if body.width > 30 && left >= 12 && left <= body.width.saturating_sub(20) {
        Some((left as u32 * 100 / body.width as u32) as u16)
    } else {
        None
    }
}

/// Top height for a horizontal-divider drag at `y`, in percent — same
/// contract as [`col_pct_for`].
pub fn row_pct_for(body: Rect, y: u16) -> Option<u16> {
    let top = y.saturating_sub(body.y);
    if body.height > 11 && top >= 5 && top <= body.height.saturating_sub(6) {
        Some((top as u32 * 100 / body.height as u32) as u16)
    } else {
        None
    }
}

/// The cross: every box from one pair of splits, so draw, pty sizing and
/// mouse dragging can never disagree. Only the two dividers move (percent
/// of the body); single boxes are sized, never dragged.
pub struct Grid {
    pub repos: Rect,
    pub list: Rect,
    pub tabs: Rect,
    pub detail: Rect,
    pub detail_full: Rect,
    pub agent: Rect,
    pub status: Rect,
    /// x of the right column's left edge: the vertical divider grabs the
    /// two border cells ending here.
    pub div_x: u16,
    /// y of the bottom boxes' top edge: the horizontal divider grabs the
    /// two border rows ending here.
    pub div_y: u16,
    /// Body rows (everything above the status line): drags clamp to these,
    /// never onto the status line.
    pub body: Rect,
}

/// Clamp a split percent to the usable range: no sliver boxes, no
/// off-screen dividers. Out-of-range values (a hand-edited state file)
/// land back inside instead of breaking the layout.
fn clamp_pct(pct: u16) -> u16 {
    pct.clamp(15, 85)
}

pub fn grid(area: Rect, col_pct: u16, row_pct: u16) -> Grid {
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let status_h = 1.min(inner.height);
    let body_h = inner.height.saturating_sub(status_h);
    let body = Rect::new(inner.x, inner.y, inner.width, body_h);
    let status = Rect::new(
        inner.x,
        inner.y.saturating_add(body_h),
        inner.width,
        status_h,
    );
    // Pixel floors keep every box usable on small screens: the percents
    // only divide what is left after the minimums.
    let max_left = body.width.saturating_sub(10);
    let left_w = if max_left <= 10 || body.width == 0 {
        body.width / 2
    } else {
        let want = body.width as u32 * clamp_pct(col_pct) as u32 / 100;
        (want as u16).clamp(10, max_left)
    };
    let max_top = body_h.saturating_sub(4);
    let top_h = if max_top <= 3 || body_h == 0 {
        body_h / 2
    } else {
        let want = body_h as u32 * clamp_pct(row_pct) as u32 / 100;
        (want as u16).clamp(3, max_top)
    };
    let div_x = body.x.saturating_add(left_w);
    let div_y = body.y.saturating_add(top_h);
    let right_w = body.width.saturating_sub(left_w);
    let bottom_h = body_h.saturating_sub(top_h);
    Grid {
        repos: Rect::new(body.x, body.y, left_w, top_h),
        list: Rect::new(body.x, div_y, left_w, bottom_h),
        tabs: Rect::new(div_x, body.y, right_w, 1.min(body_h)),
        detail: Rect::new(
            div_x,
            body.y.saturating_add(1),
            right_w,
            top_h.saturating_sub(1),
        ),
        detail_full: Rect::new(
            div_x,
            body.y.saturating_add(1),
            right_w,
            body_h.saturating_sub(1),
        ),
        agent: Rect::new(div_x, div_y, right_w, bottom_h),
        status,
        div_x,
        div_y,
        body,
    }
}

/// The one-line tabs strip: the top row of the right column. Draw renders
/// the strip here and mouse clicks are mapped here — one function, so the
/// two can never disagree by a row.
pub fn tabs_row(area: Rect, col_pct: u16) -> Rect {
    grid(area, col_pct, 38).tabs
}

/// The outer agent box when shown, from the full frame area. Main uses the
/// same function to size the pty before drawing, so the terminal and its
/// box never disagree by a row.
pub fn agent_box(area: Rect, col_pct: u16, row_pct: u16) -> Rect {
    grid(area, col_pct, row_pct).agent
}

pub fn draw(frame: &mut Frame, app: &App, view: &ViewModel) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" gtasks ")
        .border_style(theme::border());
    frame.render_widget(outer, frame.area());

    // One cross for every box: the same grid feeds draw, pty sizing and
    // the mouse, so a divider grab and a box edge can never disagree.
    let g = grid(frame.area(), app.col_pct, app.row_pct);

    // Left column: repos on top, issues/PRs below. The filter follows the
    // repos cursor, so moving above re-lists below live.
    let (repos_widget, mut repos_state) = repos_list(app, view);
    frame.render_stateful_widget(repos_widget, g.repos, &mut repos_state);
    let (issues_widget, mut issues_state) = issues_list(app, view);
    frame.render_stateful_widget(issues_widget, g.list, &mut issues_state);

    // Right column: tabs + detail on top, agent/worktree at the bottom —
    // but only while the active tab's agent operates or its finished
    // output is still on screen (or the pane has focus, so `w` still
    // reaches the instructions when idle). Idle tabs give the whole
    // column to the detail.
    if app.agent_box_shown() || app.pane == Pane::Agent {
        // The tabs are one line, not a box: at 80x12 every row counts, and a
        // bordered strip would eat the detail it labels.
        frame.render_widget(tabs_bar(app, view), g.tabs);
        frame.render_widget(detail(app, view, g.detail.width), g.detail);
        if let Some(area) = worktrees_box_rect(g.detail, view) {
            frame.render_widget(worktrees_box(view), area);
        }
        frame.render_widget(agent_panel(app, view, g.agent.width), g.agent);
    } else {
        frame.render_widget(tabs_bar(app, view), g.tabs);
        frame.render_widget(detail(app, view, g.detail_full.width), g.detail_full);
        if let Some(area) = worktrees_box_rect(g.detail_full, view) {
            frame.render_widget(worktrees_box(view), area);
        }
    }

    // Two panels, so the status cannot be pushed off the right edge by the
    // legend. The legend keeps the left side and truncates there: the recovery
    // keys (`q`, `?`) are first, so what is lost is the least important part.
    match app.status_text() {
        Some(status) => {
            let bottom = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
                .split(g.status);
            frame.render_widget(legend(app), bottom[0]);
            frame.render_widget(status_line(&status), bottom[1]);
        }
        None => frame.render_widget(legend(app), g.status),
    }

    if app.show_help {
        draw_help(frame, frame.area());
    }

    if let Some(crate::app::Modal::Confirm(question)) = &app.modal {
        draw_confirm(frame, frame.area(), question);
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
    // The body only: the meta table renders straight from the item in
    // `detail()`, so a stale body can never glue itself under new fields.
    // (A finished record's card renders straight from the record there —
    // the view never carries it, so it can never leak under item fields.)
    let detail = app
        .effective_item()
        .map(|item| {
            let key = (item.repo.clone(), item.number);
            if app.detail_for.as_ref() == Some(&key) {
                app.detail.clone()
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    let _ = visible;
    let item_kinds = app
        .visible_items()
        .iter()
        .map(|item| Some(item.kind))
        .collect();
    // The open trees behind the shown issue/PR: its head branch, or the
    // fresh branch issues grow (`issue-N`), so the work is visible where
    // the issue is read.
    let trees = app
        .effective_item()
        .map(|item| {
            let branch = item
                .head_ref_name
                .clone()
                .unwrap_or_else(|| format!("issue-{}", item.number));
            app.item_trees(&item.repo, &branch)
        })
        .unwrap_or_default();
    ViewModel {
        repos: app.repos.iter().map(|row| row.text().to_string()).collect(),
        items,
        item_worktrees,
        item_kinds,
        detail,
        trees,
        tabs: app.tab_labels(),
        active_tab: app.active_tab,
        agent: app.agent_panel_text(),
        agent_screen: app
            .active_agent()
            .map(|agent| agent.screen.clone())
            .unwrap_or_default(),
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
/// marker (mauve folder-tree with a Nerd Font, `◉` without) plus a gap in
/// front of the number says a worktree directory exists for the item's
/// branch: `g` then works on it.
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
            // Titles read bold: the number carries the kind colour, the
            // title carries the weight — at a glance rows scan as titles
            // first, numbers second. (Glyph size itself is the terminal's,
            // not the app's: no box can be bigger than the terminal font.)
            let mut spans = vec![
                Span::styled(icon, kind_style(*kind)),
                Span::styled(sanitize(&number), kind_style(*kind)),
                Span::raw(" "),
                Span::styled(
                    sanitize(&title),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            if icon.is_empty() {
                spans.remove(0);
            }
            if *has_wt {
                // Marker, then a real gap: the marker must not touch the
                // kind icon — at a glance the two merge into one glyph.
                spans.insert(0, Span::raw(" "));
                spans.insert(
                    0,
                    Span::styled(worktree_marker_text(), theme::worktree_marker()),
                );
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
        if info.has_worktree {
            // Same order as the list and the detail: marker, gap, kind.
            chunks.push(TabChunk {
                text: worktree_marker_text().to_string(),
                style: theme::worktree_marker(),
                tab: Some(i),
            });
            chunks.push(TabChunk {
                text: " ".to_string(),
                style: Style::default(),
                tab: Some(i),
            });
        }
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
        if info.has_agent {
            // A live session in this tab: the mark says which tab keeps
            // working while another is on screen.
            chunks.push(TabChunk {
                text: " ".to_string(),
                style: Style::default(),
                tab: Some(i),
            });
            chunks.push(TabChunk {
                text: "◉".to_string(),
                style: Style::default().fg(theme::FOCUSED),
                tab: Some(i),
            });
        }
    }
    // The active tab stands out with a full background: at a glance the
    // eye lands on the bookmark being read, not on the row of dots.
    if let Some(active) = app.active_tab {
        for chunk in &mut chunks {
            if chunk.tab == Some(active) {
                chunk.style = chunk
                    .style
                    .bg(theme::SELECTED_BG)
                    .add_modifier(Modifier::BOLD);
            }
        }
    }
    chunks
}

/// Which tab sits under a mouse click, if any. Pure over the app (no
/// terminal needed), so tests pin the mapping: separators and the empty
/// strip map to `None` — a click between tabs switches nothing.
pub fn tab_at(app: &App, area: Rect, x: u16, y: u16) -> Option<usize> {
    use unicode_width::UnicodeWidthStr;
    let row = grid(area, app.col_pct, app.row_pct).tabs;
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
/// terminals keep their detail rows. The active tab carries a full
/// background, the rest stay dim; the number carries the kind colour
/// (green PR, yellow issue). No prefix, no keys — the legend owns those.
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

/// Markdown to spans: the shared mapping for bodies and record cards.
fn md_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    // Markdown is rendered at the column width, so the wrapping is real: the
    // paragraph must not wrap again.
    let lines = md::render(text, width.saturating_sub(2).max(1) as usize);
    lines
        .into_iter()
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .into_iter()
                .map(|span| {
                    let style = match span.style {
                        md::Style::Plain => theme::overlay_text(),
                        md::Style::Bold => theme::overlay_text().add_modifier(Modifier::BOLD),
                        md::Style::Italic => theme::overlay_text().add_modifier(Modifier::ITALIC),
                        md::Style::Code => theme::overlay_text().add_modifier(Modifier::DIM),
                    };
                    Span::styled(span.text, style)
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// The state pill: a coloured dot plus the state, the colour saying open,
/// draft, merged or closed before the word is read.
fn state_pill(item: &crate::model::Item) -> (String, Style) {
    let bold =
        |color: ratatui::style::Color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    if item.is_draft == Some(true) {
        return ("● DRAFT".to_string(), bold(theme::WARN));
    }
    match item.state.as_str() {
        "OPEN" => ("● OPEN".to_string(), bold(theme::OK)),
        "MERGED" => ("● MERGED".to_string(), bold(theme::MAUVE)),
        "CLOSED" => ("● CLOSED".to_string(), bold(theme::ALERT)),
        other => (format!("● {}", sanitize(other)), bold(theme::SUBTEXT)),
    }
}

/// One tone of field value to its colour: verdicts read as colours first.
fn field_style(tone: Option<crate::model::FieldTone>) -> Style {
    match tone {
        None => theme::overlay_text(),
        Some(crate::model::FieldTone::Good) => Style::default().fg(theme::OK),
        Some(crate::model::FieldTone::Bad) => Style::default().fg(theme::ALERT),
        Some(crate::model::FieldTone::Warn) => Style::default().fg(theme::WARN),
        Some(crate::model::FieldTone::Accent) => Style::default().fg(theme::FOCUSED),
    }
}

/// The detail pane: title with the kind-coloured number, a state pill, a
/// meta table (muted labels, plain values, verdicts coloured), then a
/// separate DESCRIPTION section with the body. Structured lines, not
/// Markdown: the pane owns every span, so the table aligns and the
/// description can never glue itself to the fields. The agent record card
/// keeps the old whole-pane path — it replaces the detail, it is not a row
/// of it.
/// The open trees behind the shown issue/PR float in their own box,
/// top-right inside the detail. `worktrees_box_rect` says where, `draw`
/// renders it after the detail so it sits on top; the mouse still maps
/// the whole area to the detail. `None` when there is nothing to show
/// or the detail is too narrow to host it.
pub fn worktrees_box_rect(detail_area: Rect, view: &ViewModel) -> Option<Rect> {
    if view.trees.is_empty() || detail_area.width < 30 || detail_area.height < 5 {
        return None;
    }
    let mut inner: usize = "worktrees".len();
    for tree in &view.trees {
        let head = tree_head_text(tree);
        inner = inner.max(head.len());
        inner = inner.max(tree.path.display().to_string().len() + 2);
    }
    let width = (inner + 4).min(detail_area.width as usize) as u16;
    let height = (view.trees.len() as u16) * 2 + 2;
    if height + 1 >= detail_area.height {
        return None;
    }
    Some(Rect::new(
        detail_area.x + detail_area.width.saturating_sub(width),
        detail_area.y,
        width,
        height,
    ))
}

/// One head line per tree, without the marker glyph (it leads the box
/// rows at render time): `branch · agent · running`.
fn tree_head_text(tree: &crate::app::ItemTree) -> String {
    let mut head = tree.branch.clone();
    match &tree.agent {
        Some(agent) => head.push_str(&format!(" · {agent}")),
        None => head.push_str(" · no agent yet"),
    }
    if tree.live {
        head.push_str(" · running");
    }
    head
}

fn worktrees_box(view: &ViewModel) -> Paragraph<'static> {
    let mut lines = Vec::new();
    for tree in &view.trees {
        lines.push(Line::from(vec![
            Span::styled(worktree_marker_text(), theme::worktree_marker()),
            Span::styled(
                tree_head_text(tree),
                Style::default()
                    .fg(theme::FOCUSED)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {}", tree.path.display()),
            Style::default().fg(theme::SUBTEXT),
        )));
    }
    Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" worktrees ")
            .border_style(theme::border()),
    )
}

fn detail(app: &App, view: &ViewModel, width: u16) -> Paragraph<'static> {
    let focused = app.pane == Pane::Detail;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" detail ")
        .border_style(if focused {
            theme::border_focused()
        } else {
            theme::border()
        })
        .title_style(theme::title(focused));
    // A finished record's card is the whole pane, like before. A running
    // session keeps the classic item box up top — the agent's screen is
    // the box below, never the detail.
    if let Some(record) = app
        .visible_record()
        .filter(|r| matches!(r.status, crate::agent::AgentStatus::Finished { .. }))
    {
        return Paragraph::new(md_lines(&record.card(), width))
            .block(block)
            .scroll((app.detail_scroll, 0));
    }
    let Some(item) = app.effective_item() else {
        // No issue/PR under the cursor — but the agent may still have a
        // tree: show where it works instead of a blank pane. Repos with
        // no rows (a fresh branch, a W-grown tree) would otherwise leave
        // the whole right column above the box empty.
        if let Some(ctx) = app.active_agent().and_then(|agent| agent.context.as_ref()) {
            let lines = vec![
                Line::from(Span::styled(
                    "working on",
                    theme::overlay_text().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::raw(ctx.repo.clone())),
                Line::from(Span::raw(format!(
                    "{} @ {}",
                    ctx.path.display(),
                    ctx.branch
                ))),
            ];
            return Paragraph::new(lines).block(block);
        }
        // Still nothing, but a tab names an issue the visible list does
        // not have (a stale fetch, a just-created PR, a repo filter):
        // name it instead of a blank pane — the tab keeps pointing there.
        if let Some(tab) = app.active_tab.and_then(|idx| app.tabs.get(idx)) {
            let known = app
                .visible_items()
                .iter()
                .any(|i| i.repo == tab.repo && i.number == tab.number);
            if !known {
                let lines = vec![
                    Line::from(Span::styled(
                        format!("#{} {}", tab.number, tab.repo),
                        theme::title(false),
                    )),
                    Line::from(Span::raw("not in the current list")),
                    Line::from(Span::raw("r refreshes — the tab keeps pointing here")),
                ];
                return Paragraph::new(lines).block(block);
            }
        }
        return Paragraph::new(Vec::<Line<'static>>::new()).block(block);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut fields: Vec<crate::model::DetailField> = Vec::new();
    for row in item.detail_rows(now) {
        if let crate::model::DetailRow::Field(field) = row {
            fields.push(field);
        }
    }
    let label_width = fields.iter().map(|f| f.label.len()).max().unwrap_or(0);
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Title: the number in the kind colour, everything bold.
    let kind = Some(item.kind);
    let icon = match kind {
        Some(crate::model::ItemKind::Pr) => icons::kind_pr(),
        Some(crate::model::ItemKind::Issue) => icons::kind_issue(),
        None => "",
    };
    let number_style = kind_style(kind).add_modifier(Modifier::BOLD);
    let mut title = Vec::new();
    if app.has_worktree(&item.repo, item.head_ref_name.as_deref()) {
        // Same marker as the list and the tabs: one glance says "a tree
        // exists", wherever the eye lands.
        title.push(Span::styled(
            worktree_marker_text(),
            theme::worktree_marker(),
        ));
        title.push(Span::raw(" "));
    }
    title.push(Span::styled(icon, number_style));
    title.push(Span::styled(
        format!("#{} {}", item.number, sanitize(&item.title)),
        theme::overlay_text().add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(title));
    let (pill, pill_style) = state_pill(item);
    lines.push(Line::from(Span::styled(pill, pill_style)));
    for field in &fields {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:label_width$} : ", field.label),
                Style::default().fg(theme::SUBTEXT),
            ),
            Span::styled(sanitize(&field.value), field_style(field.tone)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "DESCRIPTION",
        Style::default()
            .fg(theme::FOCUSED)
            .add_modifier(Modifier::BOLD),
    )));
    if !view.detail.trim().is_empty() {
        lines.extend(md_lines(&view.detail, width));
    } else {
        let key = (item.repo.clone(), item.number);
        let hint = if app.detail_requested.as_ref() == Some(&key) {
            "loading the body…"
        } else {
            "Enter — body"
        };
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(theme::SUBTEXT),
        )));
    }
    Paragraph::new(lines)
        .block(block)
        .scroll((app.detail_scroll, 0))
        .wrap(ratatui::widgets::Wrap { trim: true })
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
    let scroll = app.active_agent().map(|agent| agent.scroll).unwrap_or(0);
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
        .scroll((scroll, 0))
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

/// A yes/no question in the middle of the screen — delete a worktree, clone
/// a repo, start a branch. It used to print in the bottom legend, where a
/// long path pushed the keys off the edge and a short one was easy to
/// answer without reading. The box wraps the question and pins `y`/`n` to
/// its own bottom row, so the choice is always under the words.
fn draw_confirm(frame: &mut Frame, area: Rect, question: &str) {
    let area = centered_rect(70, 40, area);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" confirm ")
        .border_style(theme::border_focused())
        .title_style(theme::border_focused())
        .style(theme::overlay_text());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let message = Paragraph::new(ratatui::text::Line::from(sanitize(question)))
        .style(theme::overlay_text())
        .wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(message, rows[0]);
    let remedy = Paragraph::new(ratatui::text::Line::from(sanitize(
        "y confirm · n/Esc cancel",
    )))
    .style(theme::overlay_text());
    frame.render_widget(remedy, rows[1]);
}

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
            let state = setting.state_text(app);
            ListItem::new(sanitize(&format!("{mark} {} [{state}]", setting.title())))
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
