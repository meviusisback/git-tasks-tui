//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

use crate::actions::{Action, MergeMethod, RepoSlug, argv_for};
use crate::actions_ui::{Confirmation, Executed, confirmation_for, execute};
use crate::agent::{
    AgentChoice, AgentId, AgentPlan, AgentRecord, AgentStatus, PlanInputs, plan_agent,
};
use crate::binaries::Binaries;
use crate::fetch::{FetchError, FetchOutcome};
use crate::gh::CheckedArgv;
use crate::model::Item;
use crate::model::ItemKind;
use crate::sanitize::sanitize;

use std::collections::BTreeSet;
use std::collections::HashMap;

/// Which pane has focus. Left column holds `Repos` (top) and `List`
/// (bottom); right column holds `Detail` (top) and `Agent` (bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Repos,
    List,
    Detail,
    Agent,
}

/// One open tab: the issue/PR opened with `Enter`.
/// The title is resolved live from `items`, so the tab stays valid across
/// refreshes even when the title changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub repo: String,
    pub number: u64,
}

/// One entry of the tabs strip: the label, the kind (for colour) and whether
/// it is the active one (for emphasis), plus whether a worktree exists for
/// its branch (the mauve marker, like the list and the detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub label: String,
    pub kind: Option<crate::model::ItemKind>,
    pub active: bool,
    pub has_worktree: bool,
    /// A live agent session runs in this tab: the strip marks it, so the
    /// user sees which tab keeps working while another is on screen.
    pub has_agent: bool,
}

/// The repo filter as a type, not a string: either everything or one valid
/// slug. There is no representation for "one invalid slug", so no check has
/// to remember to refuse it — the same lesson as the worktree paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoFilter {
    All,
    One(RepoSlug),
}

/// Which states the list shows. Open-only by default — the fetch asks
/// GitHub for closed history only when the user picks Closed or All on
/// a single repo, since every extra state multiplies the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateFilter {
    #[default]
    Open,
    Closed,
    All,
}

impl StateFilter {
    fn next(self) -> Self {
        match self {
            StateFilter::Open => StateFilter::Closed,
            StateFilter::Closed => StateFilter::All,
            StateFilter::All => StateFilter::Open,
        }
    }

    fn label(self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all states",
        }
    }
}

/// One row of the repos pane: the "everything" selector or one valid slug.
/// The list is typed end to end — `set_repos` takes slugs, never strings —
/// so no `parse` (and no `expect`) is left on the cursor path, and no magic
/// string means "all".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoRow {
    All,
    Repo(RepoSlug),
}

impl RepoRow {
    pub fn text(&self) -> &str {
        match self {
            RepoRow::All => "— all repos —",
            RepoRow::Repo(slug) => slug.as_str(),
        }
    }
}

/// "1 item", "2 items", "1 repo", "3 repos": counts are born right, so no
/// message can grow a "1 items" it never re-reads.
pub fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// The keys the app cares about, independent of the terminal backend.
/// `Shift*` move focus between the four boxes following the screen layout —
/// independent of `Tab`, which cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Backspace,
    ShiftUp,
    ShiftDown,
    ShiftLeft,
    ShiftRight,
    /// Ctrl-C always quits, whatever mode we are in.
    CtrlC,
}

/// The first action the program offers, as a **parameter**: when the product
/// decision changes (merge commit, rebase), it changes here and nowhere else.
const FIRST_ACTION_METHOD: MergeMethod = MergeMethod::Squash;
/// S9: a destructive bit needs a second confirmation, so branch deletion stays
/// out of the first version whatever the method is.
const FIRST_ACTION_DELETES_BRANCH: bool = false;

/// The confirmation and the argv travel together: what is shown is what runs.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub action: Action,
    pub argv: CheckedArgv,
}

/// What `y` will run when the confirmation dialog is open. One dialog, one
/// pending — an action and an agent launch can never wait together, so the
/// key that confirms cannot run the wrong one.
#[derive(Debug, Clone)]
pub enum Pending {
    Action(PendingAction),
    Agent(AgentPlan, std::path::PathBuf),
    /// `Enter` on a worktree row, confirmed with `y`: reopen that agent's
    /// previous session there, sending nothing. `n` goes back to choosing:
    /// with the preset agent (the `g` flow) it launches there, without one
    /// (the `W` flow) it opens the agent menu for that tree.
    Resume {
        plan: AgentPlan,
        path: std::path::PathBuf,
        preset: Option<crate::agent::AgentChoice>,
    },
    /// `d` on a worktree row, confirmed with `y`: the loop deletes it.
    Delete(crate::worktree::WorktreeDelete),
    /// A merged PR, confirmed with `y`: the loop deletes its local trees
    /// and head branch. Asked while the merge is fresh, never assumed.
    Cleanup(crate::worktree::CleanupOrder),
    /// No local clone for the repo, confirmed with `y`: the loop clones it
    /// into the agreed directory, then grows the tree the user asked for.
    Clone {
        repo: String,
        path: std::path::PathBuf,
        /// Where to continue when the clone lands: the repo tree flow, or
        /// the agent menu the user asked for.
        then: CloneThen,
    },
    /// `Enter` on a repo with no issues or PRs, confirmed with `y`: open the
    /// new-branch flow for that repo instead of an empty list.
    NewBranch {
        repo: String,
    },
}

/// Everything that can change the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Key(Key),
    Tick,
}

/// A confirmation dialog. The payload is the question key, resolved to text by
/// the UI layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Confirm(String),
    RepoMenu(RepoMenu),
    ActionMenu(ActionMenu),
    /// `g`: which agent runs the selected PR. Rows are the installed
    /// binaries; the plan waits inside until a row is confirmed.
    AgentMenu(AgentMenu),
    /// After the agent is picked: the repo's existing worktrees. `Enter`
    /// works on one, `n` names a new parallel one, `r` renames, `d` deletes.
    Worktrees(WorktreeMenu),
    /// The name of a worktree to create — or the new name of one to rename.
    /// Prepopulated with the suggestion (head branch, or the old name), so
    /// confirming untouched keeps the obvious choice.
    NameInput(NameInput),
    /// A startup notice (broken config): information, not a question. `Esc`
    /// dismisses it, `q` quits through it; nothing else acts while it is up.
    Notice(String),
    /// The diff overlay: read-only, `Esc` (or `Enter`) closes it, `↑`/`↓`
    /// scroll, `q` quits through it like the notice. No payload — the text
    /// lives in the diff fields below, keyed by `diff_for`.
    Diff,
    /// The settings menu: one row per toggleable setting, `↑`/`↓` move,
    /// `Enter`/`Space` flip, `Esc` closes, `q` quits through. Flipping sets
    /// `config_save_requested` — the loop writes `config.toml`, the key
    /// handler never touches the disk itself.
    Settings(SettingsMenu),
}

/// One toggleable setting. The menu grows by adding variants here, not by
/// special-casing the draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    SandboxAgents,
    FontSize,
    FontFamily,
}

impl Setting {
    /// The menu row title.
    pub fn title(&self) -> &'static str {
        match self {
            Setting::SandboxAgents => "sandboxed agents",
            Setting::FontSize => "terminal font size",
            Setting::FontFamily => "terminal font",
        }
    }

    /// What the toggle does, shown under the rows for the selected one.
    pub fn description(&self) -> &'static str {
        match self {
            Setting::SandboxAgents => {
                "launch through the `<agent>-sandboxed` wrappers (omp YOLO, hermes Smart, pi contained)"
            }
            Setting::FontSize => {
                "whole-terminal glyph size: Small 9px · Medium 11px · Large 14px. Restart the terminal to apply."
            }
            Setting::FontFamily => {
                "whole-terminal typeface, from the installed monospace fonts. Needs a Nerd Font for the glyphs, otherwise the markers fall back. Restart the terminal to apply."
            }
        }
    }

    /// The current value, read from the app at draw and toggle time.
    pub fn state_text(&self, app: &App) -> String {
        match self {
            Setting::SandboxAgents => {
                if app.sandbox_agents {
                    "on".to_string()
                } else {
                    "off".to_string()
                }
            }
            Setting::FontSize => app.font_size.name().to_string(),
            Setting::FontFamily => app.font_family.clone(),
        }
    }
}

/// The settings menu: every toggleable setting, in a fixed order, cursor
/// on top. Rows are data (`Setting`), so the draw never decides what
/// exists — adding a setting is a variant plus its three strings above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsMenu {
    pub cursor: usize,
    pub rows: Vec<Setting>,
}

/// The `g` menu: one row per installed agent binary, cursor, and the
/// validated plan the pick applies to. `place` is set when the tree was
/// already chosen (the `W` worktree menu): confirming an agent then
/// launches straight there instead of asking for a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMenu {
    pub rows: Vec<crate::agent::AgentChoice>,
    pub cursor: usize,
    pub plan: AgentPlan,
    pub place: Option<std::path::PathBuf>,
}

/// Where a launched agent works: repo, branch and tree path. The detail
/// pane renders this while no issue/PR is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContext {
    pub repo: String,
    pub branch: String,
    pub path: std::path::PathBuf,
}

/// The worktree picker for one repo: existing trees plus the plan the
/// choice applies to. `agent` is `Some` inside the `g` flow and `None` when
/// opened directly with `W`: using a tree then leads to the agent menu
/// first, while rename/delete work either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeMenu {
    pub agent: Option<crate::agent::AgentChoice>,
    pub plan: AgentPlan,
    pub rows: Vec<crate::worktree::WorktreeRow>,
    pub cursor: usize,
}

/// Where a background clone continues when it lands: the repo tree flow
/// (`W` grows `<repo>-dev`), or the agent menu the user asked for (`g`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneThen {
    GrowTree,
    AgentMenu,
    RepoSessions,
}

/// What the name being typed is for: a fresh worktree (agent + plan wait
/// for it) or a rename (the old tree waits for its new name). `agent` is
/// `None` when the prompt was opened from the standalone worktree menu:
/// confirming then leads to the agent menu instead of launching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamePurpose {
    Create {
        agent: Option<crate::agent::AgentChoice>,
        plan: AgentPlan,
    },
    Rename {
        order: RenameDraft,
    },
}

/// A rename before the new name is known: everything but the new branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDraft {
    pub clone: std::path::PathBuf,
    pub slug: crate::actions::RepoSlug,
    pub old_branch: crate::worktree::BranchName,
    pub old_path: std::path::PathBuf,
}

/// The name prompt: purpose plus the value being edited (append-only, like
/// the search box — `Enter` confirms, `Esc` cancels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInput {
    pub purpose: NamePurpose,
    pub value: String,
}

/// One row of the actions menu: the fully specified choice. No strings cross
/// this boundary — a row IS the typed parameters the action runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionPick {
    Merge(MergeMethod, bool),
    Close,
}

impl ActionPick {
    pub fn text(&self) -> &str {
        match self {
            ActionPick::Merge(MergeMethod::Squash, false) => "merge — squash",
            ActionPick::Merge(MergeMethod::Squash, true) => "merge — squash + delete branch",
            ActionPick::Merge(MergeMethod::Merge, false) => "merge — merge",
            ActionPick::Merge(MergeMethod::Merge, true) => "merge — merge + delete branch",
            ActionPick::Merge(MergeMethod::Rebase, false) => "merge — rebase",
            ActionPick::Merge(MergeMethod::Rebase, true) => "merge — rebase + delete branch",
            ActionPick::Close => "close issue",
        }
    }
}

/// The single constructor: a menu pick becomes the action it runs. `a`
/// (fast merge with defaults) goes through here too, so "merge with
/// defaults" lives in one place no matter which key asked for it.
pub fn action_for_pick(repo: RepoSlug, number: u64, pick: &ActionPick) -> Action {
    match pick {
        ActionPick::Merge(method, delete_branch) => Action::Merge {
            repo,
            number,
            method: *method,
            delete_branch: *delete_branch,
        },
        ActionPick::Close => Action::Close {
            repo,
            number,
            reason: None,
            comment: None,
        },
    }
}

/// The `m` single-select: one row per action the current item allows.
/// `Space`/arrows choose, `Enter` confirms, `Esc` cancels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMenu {
    pub rows: Vec<ActionPick>,
    pub cursor: usize,
}

/// The `R` multi-select: which of the configured repos the list shows.
/// `Space` toggles the row, `Enter` applies, `Esc` cancels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMenu {
    pub rows: Vec<RepoSlug>,
    pub cursor: usize,
    pub marked: BTreeSet<usize>,
}

#[derive(Debug, Clone)]
pub struct App {
    pub pane: Pane,
    pub query: String,
    pub repos_cursor: usize,
    pub repos_len: usize,
    pub list_cursor: usize,
    pub list_len: usize,
    pub detail_scroll: u16,
    pub modal: Option<Modal>,
    /// True while `/` search input is active: keys go to the query.
    pub searching: bool,
    pub quit: bool,
    pub status: String,
    /// Data from the last successful refresh, in repo order.
    pub items: Vec<Item>,
    /// Tabbed issues/PRs the OPEN-only fetch dropped (closed): the detail
    /// keeps showing them while their tab lives. Never listed, never
    /// counted — the list only ever shows what the fetch returned.
    pub retained: Vec<Item>,
    /// The repos the user configured, plus a leading "all repos" entry, so a
    /// repo with no open items is still visible and selectable.
    pub repos: Vec<RepoRow>,
    /// Every configured repo, even when the list shows a subset: the rows
    /// the `R` menu offers. Set once at startup, never filtered.
    pub all_repos: Vec<RepoSlug>,
    /// The repo selected in the repos pane; `All` means "all".
    pub repo_filter: RepoFilter,
    /// Closed history needs an explicit ask: `f` cycles open → closed →
    /// all states, but only on a single repo — all repos stay open-only.
    pub state_filter: StateFilter,
    /// `Enter` in the repo menu: the checked rows, for the loop to save to
    /// `state.json`. The live list is already updated; this is only the file.
    pub selection_applied: Option<Vec<String>>,
    /// A fetch is in flight: `r` must not start a second one.
    pub loading: bool,
    /// Repos whose last fetch failed.
    pub failed: usize,
    /// Set by the key handler, consumed by the event loop that owns the thread.
    pub refresh_requested: bool,
    /// The tab's own item, asked straight from GitHub: when the OPEN-only
    /// fetch never saw it (closed before startup), the detail fetches it
    /// on demand instead of falling back to the cursor row. Set while the
    /// ask is in flight — and after a miss, so a gone item asks once.
    pub item_requested: Option<(String, u64)>,
    /// `R` also re-reads the repo list from GitHub, behind the open menu.
    pub repo_refresh_requested: bool,
    /// A repo refresh found new repos: the config file still needs them.
    pub repos_save_requested: bool,
    /// Resolved once at startup; `None` in tests that do not execute.
    pub binaries: Option<Binaries>,
    /// Limit for a write started from the UI.
    pub write_timeout: std::time::Duration,
    /// Markdown body of the selected item.
    pub detail: String,
    /// Which item `detail` belongs to, so the same body is not fetched twice.
    pub detail_for: Option<(String, u64)>,
    /// Set by the key handler: the loop fetches this body.
    pub detail_requested: Option<(String, u64)>,
    /// Unified diff of the selected PR, capped at `DIFF_MAX_LINES`.
    pub diff_text: String,
    /// How many lines the diff really has (before the cap).
    pub diff_total: usize,
    /// True when `diff_text` holds only the head of a longer diff.
    pub diff_truncated: bool,
    /// Which item `diff_text` belongs to, so the same diff is not fetched twice.
    pub diff_for: Option<(String, u64)>,
    /// Set by the key handler: the loop fetches this diff.
    pub diff_requested: Option<(String, u64)>,
    /// How far down the diff overlay scrolled (lines).
    pub diff_scroll: usize,
    /// Set by `o`: the loop launches the browser.
    pub open_requested: Option<String>,
    /// The `?` overlay with the full keymap is on screen.
    pub show_help: bool,
    /// Open tabs (`Enter` on a row opens one) and the active tab.
    /// Bookmarks over the issues being worked: the active tab owns the
    /// detail, and the agent acts on the active tab when set, otherwise
    /// on the row under the list cursor.
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    /// Column/row split of the cross, in percent (left width, top height).
    /// Dragged with the mouse on the dividers, saved to `state.json`:
    /// a restart keeps the user's geometry. Only the two dividers move —
    /// never a single box.
    pub col_pct: u16,
    pub row_pct: u16,
    /// A divider drag in progress: which line the mouse holds. Transient —
    /// never persisted, never drawn.
    pub drag: Option<DragLine>,
    /// One tab's agent side, keyed by `(repo, number)`: everything the loop
    /// reads or writes for the box in that tab. The session itself lives in
    /// the loop (it owns threads); this is the state the screen renders.
    /// Sessions die with the app, so this map never touches `state.json`.
    pub tab_agents: HashMap<(String, u64), TabAgent>,
    /// The agent last launched in each worktree, by tree path. Recorded on
    /// every launch, persisted in the session file: reopening the tree
    /// offers that agent's previous session instead of a fresh start.
    pub last_agents: HashMap<String, crate::agent::LastAgent>,
    /// True when session fields changed since the last save: the loop
    /// persists `state.json` so a temporary exit keeps tabs, filter and
    /// cursor positions.
    pub session_dirty: bool,
    /// Profile passed to `hermes -p` by the agent handoff; validated at load.
    pub agent_profile: String,
    /// Mirror of `Config::sandbox_agents`: launch through the
    /// `<agent>-sandboxed` wrappers with non-interactive approvals. Off by
    /// default; set from the config at startup, never flipped at runtime.
    pub sandbox_agents: bool,
    /// Set when a settings toggle flips: the loop writes `config.toml`.
    /// The key handler never touches the disk itself.
    pub config_save_requested: bool,
    /// Set with `config_save_requested` by the font toggles only: the loop
    /// also rewrites the Foot `font=` line. Kept separate so a sandbox-only
    /// toggle never touches the terminal config.
    pub font_save_requested: bool,
    /// Terminal font size (whole terminal — a TUI cannot resize its glyphs).
    pub font_size: crate::config::FontSize,
    /// Terminal typeface, from the installed monospace fonts.
    pub font_family: String,
    /// Installed monospace families, discovered once at startup: the font
    /// toggle cycles through these.
    pub fonts: Vec<String>,
    /// Where to look for local clones, in order (`<root>/<repo-name>`); empty refuses.
    /// A list, not a path: clones really do live in more than one directory, and one path
    /// cannot say so without making the other directory invisible.
    pub clone_roots: Vec<std::path::PathBuf>,
    /// True when the config on disk could not be used and the app is running on the demo fallback.
    /// The notice says it once; this is how the status line can keep saying it, because a decision
    /// taken once must not become a thing forgotten: after `Esc` the list holds a repository the
    /// user never chose, and nothing else on screen remembers why.
    pub config_fallback: bool,
    /// Set by `g`, consumed by the loop that owns the terminal: validate
    /// here, hand over there, like every other outbox.
    pub agent_requested: Option<AgentPlan>,
    /// Set by `y` on the create question, consumed by the loop: the plan
    /// plus the worktree the user agreed to create.
    pub agent_go: Option<(AgentPlan, std::path::PathBuf)>,
    /// Set by `y` on the resume question, consumed by the loop: reopen the
    /// recorded agent's previous session in that tree, sending nothing.
    pub agent_resume: Option<(AgentPlan, std::path::PathBuf)>,
    /// Set by `Enter` on a worktree row: the plan plus the tree's real path.
    /// The loop launches there without re-deriving the path from the branch:
    /// a rename outside the app leaves the directory out of sync with the
    /// branch name, and the derived path would point at a directory that
    /// was never created.
    pub agent_place: Option<(AgentPlan, std::path::PathBuf)>,
    /// Set by `Enter` on the rename prompt, consumed by the loop: rename
    /// the branch, then move the tree. No confirm — nothing is lost.
    pub rename_go: Option<crate::worktree::WorktreeRename>,
    /// `y` on the delete question, consumed by the loop.
    pub delete_go: Option<crate::worktree::WorktreeDelete>,
    /// Set by `y` on the merge cleanup question, consumed by the loop: the
    /// merged PR's local trees plus its head branch.
    pub cleanup_go: Option<crate::worktree::CleanupOrder>,
    /// Set by `y` on the clone question, consumed by the loop: the repo plus
    /// the directory the user agreed to clone it into, plus where to
    /// continue when it lands. The loop clones in a thread (network can
    /// take minutes) and continues the flow that asked when done.
    pub clone_go: Option<(String, std::path::PathBuf, CloneThen)>,
    /// Where worktrees live (configured root, or the default). The list
    /// icons and the worktree picker read it; `None` (unresolvable home)
    /// means no icons, and the picker explains instead of guessing.
    pub worktree_root: Option<std::path::PathBuf>,
    /// What `y` runs: an action or an agent launch, never both.
    pub pending: Option<Pending>,
}

/// Which divider of the cross the mouse holds: the vertical line between
/// the columns or the horizontal line between the top and bottom boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragLine {
    Cols,
    Rows,
}

/// One open worktree behind an issue/PR, for the detail box: where the
/// work lives, who ran an agent there last, and whether one is live now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTree {
    pub branch: String,
    pub path: std::path::PathBuf,
    pub agent: Option<String>,
    pub live: bool,
}

/// One tab's agent side: the screen the box renders, whether its session
/// is alive, the finished run (the card stays after the exit), what the
/// agent works on (shown in the detail while nothing is selected), and
/// the draft the loop is still typing. The session itself lives in the
/// loop; this map entry is its shadow.
#[derive(Debug, Clone, Default)]
pub struct TabAgent {
    pub running: bool,
    pub screen: crate::embedded::AgentScreen,
    pub scroll: u16,
    pub record: Option<AgentRecord>,
    pub dismissed_for: Option<(String, u64)>,
    pub context: Option<AgentContext>,
    pub draft: Option<String>,
}

/// Periodic-refresh door: true when `refresh_s` seconds passed since `last`
/// and nothing else wants the fetch lane. `refresh_s == 0` means off.
/// The loop evaluates this every tick; the single-flight spawn stays the only
/// place a fetch starts, so auto and manual refresh share the same door.
pub fn auto_refresh_due(
    last: std::time::Instant,
    now: std::time::Instant,
    refresh_s: u64,
    busy: bool,
) -> bool {
    refresh_s > 0 && !busy && now.duration_since(last).as_secs() >= refresh_s
}

impl Default for App {
    fn default() -> Self {
        Self {
            pane: Pane::List,
            query: String::new(),
            repos_cursor: 0,
            repos_len: 0,
            list_cursor: 0,
            list_len: 0,
            detail_scroll: 0,
            modal: None,
            searching: false,
            quit: false,
            status: String::new(),
            items: Vec::new(),
            retained: Vec::new(),
            repos: Vec::new(),
            all_repos: Vec::new(),
            repo_filter: RepoFilter::All,
            state_filter: StateFilter::Open,
            selection_applied: None,
            loading: false,
            failed: 0,
            refresh_requested: false,
            item_requested: None,
            repo_refresh_requested: false,
            repos_save_requested: false,
            binaries: None,
            write_timeout: std::time::Duration::from_secs(30),
            pending: None,
            agent_profile: String::from("default"),
            sandbox_agents: false,
            config_save_requested: false,
            font_save_requested: false,
            font_size: crate::config::FontSize::default(),
            font_family: String::from("monospace"),
            fonts: Vec::new(),
            clone_roots: Vec::new(),
            config_fallback: false,
            tab_agents: HashMap::new(),
            last_agents: HashMap::new(),
            agent_requested: None,
            agent_go: None,
            agent_resume: None,
            agent_place: None,
            rename_go: None,
            delete_go: None,
            cleanup_go: None,
            clone_go: None,
            worktree_root: None,
            detail: String::new(),
            detail_for: None,
            detail_requested: None,
            diff_text: String::new(),
            diff_total: 0,
            diff_truncated: false,
            diff_for: None,
            diff_requested: None,
            diff_scroll: 0,
            open_requested: None,
            show_help: false,
            tabs: Vec::new(),
            active_tab: None,
            col_pct: 30,
            row_pct: 38,
            drag: None,
            session_dirty: false,
        }
    }
}

impl App {
    /// Tell the app how many rows exist, so the cursors can clamp.
    pub fn set_lengths(&mut self, repos: usize, list: usize) {
        self.repos_len = repos;
        self.list_len = list;
        self.repos_cursor = self.repos_cursor.min(repos.saturating_sub(1));
        self.list_cursor = self.list_cursor.min(list.saturating_sub(1));
    }

    /// Take the result of a fetch. Two facts keep the cursor honest: a refresh
    /// keeps the repo order, and a new list resets the cursor (see
    /// `set_repos`). Together they mean no index survives a reorder — so
    /// `RepoFilter` is the only representation of "which repo", and there is
    /// no second copy to fall out of sync.
    pub fn apply_outcome(&mut self, outcome: FetchOutcome) {
        let FetchOutcome { items, failures } = outcome;
        // Tabs outlive the open list. The fetch is OPEN-only, so a closed
        // issue/PR vanishes from it — but its tab keeps showing it: the
        // content, the body and the worktrees. Repos that failed to fetch
        // retain nothing: unknown is not closed.
        let mut retained: Vec<Item> = self
            .retained
            .iter()
            .filter(|item| {
                self.tabs
                    .iter()
                    .any(|t| t.repo == item.repo && t.number == item.number)
                    && !items
                        .iter()
                        .any(|i| i.repo == item.repo && i.number == item.number)
            })
            .cloned()
            .collect();
        for tab in &self.tabs {
            let gone = !items
                .iter()
                .any(|i| i.repo == tab.repo && i.number == tab.number);
            let failed = failures.iter().any(|(repo, _)| *repo == tab.repo);
            let kept = retained
                .iter()
                .any(|i| i.repo == tab.repo && i.number == tab.number);
            if gone
                && !failed
                && !kept
                && let Some(old) = self
                    .items
                    .iter()
                    .find(|i| i.repo == tab.repo && i.number == tab.number)
            {
                retained.push(old.clone());
            }
        }
        self.retained = retained;
        self.items = items;
        self.failed = failures.len();
        self.loading = false;
        // A fresh fetch is a fresh chance: a tab item the last round
        // could not load asks again instead of staying silent.
        self.item_requested = None;

        // Keep the configured repos: a repo with no open items must still be
        // visible, otherwise it looks like it was never configured. Slugs
        // that do not parse are left out of the list (their items still show
        // under "all"): the fetch boundary is untrusted input.
        if self.repos.is_empty() {
            for item in &self.items {
                let listed = self.repos.iter().any(|row| match row {
                    RepoRow::Repo(slug) => slug.as_str() == item.repo,
                    RepoRow::All => false,
                });
                if !listed {
                    if let Ok(slug) = RepoSlug::parse(&item.repo) {
                        self.repos.push(RepoRow::Repo(slug));
                    }
                }
            }
        }
        self.set_lengths(self.repos.len(), self.visible_items().len());
        // A fresh list means a fresh selection to describe: start its body
        // at once, quietly — Enter stays the loud, explicit ask.
        self.preload_detail();
        self.refresh_status();
    }

    /// The single writer of the list summary: every path that changes what the
    /// list shows (fetch, filter, menu) ends here, so the line always
    /// describes the current view — never the previous one. Transient event
    /// lines (saved, failed, loading) belong to their event, not here.
    pub fn refresh_status(&mut self) {
        let shown = self.visible_items().len();
        // Row 0 is the "all repos" selector, not a repository: do not count it.
        let configured = self.repos.len().saturating_sub(1);
        let items = plural(self.items.len(), "item", "items");
        let repos = plural(configured, "repo", "repos");
        self.status = match &self.repo_filter {
            RepoFilter::One(slug) => {
                let stated = match self.state_filter {
                    StateFilter::Open => String::new(),
                    filter => format!(" · {}", filter.label()),
                };
                format!("{} in {slug}{stated}", plural(shown, "item", "items"))
            }
            RepoFilter::All => match self.failed {
                0 => format!("{items} from {repos}"),
                failed => format!(
                    "{items} from {repos} · {}",
                    plural(failed, "failed", "failed")
                ),
            },
        };
    }

    /// Install the configured repos. Row 0 is "all repos", so the user can get
    /// back to the unfiltered list without leaving the pane.
    pub fn set_repos(&mut self, repos: Vec<RepoSlug>) {
        self.repos = std::iter::once(RepoRow::All)
            .chain(repos.into_iter().map(RepoRow::Repo))
            .collect();
        self.repos_cursor = 0;
        self.repo_filter = RepoFilter::All;
        self.set_lengths(self.repos.len(), self.visible_items().len());
        // A new list means a new context: a body fetched for the previous
        // one would say "this is the item you are looking at" about an item
        // that may be gone — same discipline as `apply_repo_filter`.
        self.detail.clear();
        self.detail_for = None;
        self.detail_requested = None;
    }

    /// Row 0 means "everything"; any other row filters the list to that repo.
    /// The row already is the type — no parse, no expect, no magic string.
    fn apply_repo_filter(&mut self) {
        self.repo_filter = match self.repos.get(self.repos_cursor) {
            Some(RepoRow::Repo(slug)) => RepoFilter::One(slug.clone()),
            // Row 0, or a cursor past the end after the list shrank.
            _ => RepoFilter::All,
        };
        self.list_cursor = 0;
        self.set_lengths(self.repos.len(), self.visible_items().len());
        // The detail belonged to the previous list: showing it here would say
        // "this is the item you are looking at" about an item that is gone.
        self.detail.clear();
        self.detail_for = None;
        self.detail_requested = None;
        // ... but the new first row starts loading at once: the description
        // is on its way before anyone presses Enter.
        self.preload_detail();
        self.refresh_status();
    }

    /// The transient line the loop sets after the menu applies: what just
    /// happened (cleared vs narrowed), counted from the same list the panel
    /// draws. Lives here — not in main — so tests can reach it: a sentence
    /// composed where it cannot be tested rots unnoticed.
    pub fn selection_summary(&self, cleared: bool) -> String {
        let shown = plural(self.repos.len().saturating_sub(1), "repo", "repos");
        if cleared {
            format!("selection cleared — showing all {shown}")
        } else {
            format!("selection saved — {shown}")
        }
    }

    /// Items matching the search box. Case-insensitive on the title and the
    /// number, so `/42` finds `#42`.
    pub fn visible_items(&self) -> Vec<&Item> {
        let by_repo: Vec<&Item> = match &self.repo_filter {
            RepoFilter::One(slug) => self
                .items
                .iter()
                .filter(|i| i.repo == slug.as_str())
                .collect(),
            RepoFilter::All => self.items.iter().collect(),
        };
        let by_state: Vec<&Item> = match self.state_filter {
            StateFilter::Open => by_repo.into_iter().filter(|i| i.state == "OPEN").collect(),
            StateFilter::Closed => by_repo.into_iter().filter(|i| i.state != "OPEN").collect(),
            StateFilter::All => by_repo,
        };
        let searched: Vec<&Item> = if self.query.trim().is_empty() {
            by_state
        } else {
            let needle = self.query.to_lowercase();
            by_state
                .into_iter()
                .filter(|item| {
                    item.title.to_lowercase().contains(&needle)
                        || item.number.to_string().contains(&needle)
                })
                .collect()
        };
        // Closed history reads oldest-first: numeric order, like an archive.
        if self.state_filter == StateFilter::Open {
            searched
        } else {
            let mut ordered = searched;
            ordered.sort_by_key(|i| i.number);
            ordered
        }
    }

    /// The bottom legend: the keys available right now, never the full keymap
    /// (that lives behind `?`). Untrusted text (query, status) is sanitised.
    pub fn legend(&self) -> String {
        match &self.modal {
            Some(Modal::Confirm(_)) => {
                // The question lives in its own box now; the legend keeps
                // only the keys, so the two never disagree about the words.
                return String::from("y confirm · n/Esc cancel");
            }
            Some(Modal::RepoMenu(menu)) => {
                return format!(
                    "{} marked — Space mark · a all · n none · Enter apply · Esc cancel",
                    menu.marked.len()
                );
            }
            Some(Modal::ActionMenu(_)) => {
                return String::from("↑/↓ choose · Enter confirm · Esc cancel");
            }
            Some(Modal::AgentMenu(_)) => {
                return String::from("↑/↓ choose · Enter confirm · Esc cancel");
            }
            Some(Modal::Worktrees(_)) => {
                return String::from("Enter use · n new · r rename · d delete · Esc cancel");
            }
            Some(Modal::NameInput(_)) => {
                return String::from("type the name · Enter confirm · Esc cancel");
            }
            Some(Modal::Diff) => {
                return String::from("↑/↓ scroll · Enter/Esc close · q quit");
            }
            Some(Modal::Settings(_)) => {
                return String::from("↑/↓ choose · Enter/Space toggle · Esc close · q quit");
            }
            Some(Modal::Notice(_)) => {
                // The box above says what is wrong, in full. The row only
                // keeps the remedy: repeating the message here would truncate
                // it mid-word and add nothing.
                return String::from("Esc dismiss · q quit");
            }
            None => {}
        }
        if self.searching {
            return format!(
                "/{query} — Esc done · Enter keep",
                query = sanitize(&self.query)
            );
        }
        // The legend names what the keys do *here*. It stays short on
        // purpose: the status has its own panel (`status_text`), and a long
        // legend in this one row would push every message off the edge.
        let text = if self.pane == Pane::Detail {
            String::from(
                "q quit · ? help · ←/→ panes · t/T tabs · x close · m actions · g agent · W trees · d diff · Enter body · s settings",
            )
        } else if self.pane == Pane::Agent {
            // While the agent runs, every key but Esc is agent input —
            // including Ctrl-C, which interrupts it. The legend must not
            // promise keys that type instead.
            if self.active_agent().is_some_and(|a| a.running) {
                String::from(
                    "Shift+arrows move · Ctrl-C interrupts · wheel scrolls · type to agent",
                )
            } else {
                String::from(
                    "q quit · ? help · ←/→ panes · ↑/↓ scroll · g launch agent · x close record",
                )
            }
        } else if self.pane == Pane::Repos {
            String::from(
                "q quit · ? help · ↑/↓ move · Enter drill in · R repos · t/T · W worktree · x close",
            )
        } else if self.repo_filter == RepoFilter::All {
            String::from(
                "q quit · ? help · ←/→ panes · ↑/↓ move · Tab repos · Enter open tab · t/T tabs · x close · R repos · m actions · g agent · W trees · d diff · s settings",
            )
        } else {
            String::from(
                "q quit · ? help · Esc back · ←/→ panes · ↑/↓ move · Tab repos · Enter open tab · t/T tabs · x close · m actions · g agent · W trees · d diff · f state · s settings",
            )
        };
        // The marker is *prefixed*, not appended, and the reason is measured: appended, the key
        // hints fill the row and push it past the right edge - at 100 columns the legend already
        // ends at `R repos` and the marker simply was not on screen. What must never be lost goes
        // first. It also does not live in `status`, which every event rewrites (loading, saved,
        // failed): a persistent fact there would be gone at the next keystroke.
        if self.config_fallback {
            return format!("config: demo fallback · {text}");
        }
        text
    }

    /// The status message, rendered in its own panel.
    ///
    /// It must NOT be a suffix of `legend()`: that string goes into a single
    /// row, so a long legend pushes the status past the right edge and the
    /// message disappears at 80–100 columns — exactly where "nothing selected"
    /// and, later, "it may have been applied" have to be readable.
    pub fn status_text(&self) -> Option<String> {
        if self.status.is_empty() {
            None
        } else {
            Some(sanitize(&self.status))
        }
    }

    /// Ask the loop to open the selected item, after checking its URL (S10).
    /// Follows the active tab when one is open.
    pub fn request_open(&mut self) {
        let Some((url, number)) = self
            .effective_item()
            .map(|item| (item.url.clone(), item.number))
        else {
            self.status = "nothing selected".to_string();
            return;
        };
        match crate::actions::validate_url(&url) {
            Ok(url) => {
                self.open_requested = Some(url.to_string());
                self.status = format!("opening #{number}…");
            }
            Err(err) => self.status = format!("cannot open: {err}"),
        }
    }

    /// `R`: pick which repos the list shows. The menu starts from the live
    /// list (everything, when the state file never picked a subset) — and a
    /// `gh repo list` runs behind it, so repos created outside the app join
    /// the rows instead of needing a restart.
    pub fn open_repo_menu(&mut self) {
        if self.all_repos.is_empty() {
            self.status = "no repos configured".to_string();
            return;
        }
        // The refresh runs behind the menu; the rows merge in on arrival.
        self.repo_refresh_requested = true;
        self.status = "updating the repo list…".to_string();
        // The live list, minus the "all repos" row, is the current pick.
        let live: Vec<&RepoSlug> = self
            .repos
            .iter()
            .filter_map(|row| match row {
                RepoRow::Repo(slug) => Some(slug),
                RepoRow::All => None,
            })
            .collect();
        let marked: BTreeSet<usize> = self
            .all_repos
            .iter()
            .enumerate()
            .filter(|(_, slug)| live.contains(slug))
            .map(|(i, _)| i)
            .collect();
        self.modal = Some(Modal::RepoMenu(RepoMenu {
            rows: self.all_repos.clone(),
            cursor: 0,
            marked,
        }));
    }

    fn menu_step(&mut self, delta: i32) {
        if let Some(Modal::RepoMenu(menu)) = self.modal.as_mut() {
            let len = menu.rows.len();
            menu.cursor = step(menu.cursor, delta, len);
        }
    }

    fn menu_toggle(&mut self) {
        if let Some(Modal::RepoMenu(menu)) = self.modal.as_mut() {
            if menu.cursor < menu.rows.len() && !menu.marked.remove(&menu.cursor) {
                menu.marked.insert(menu.cursor);
            }
        }
    }

    /// `a` in the repo menu: mark every repo.
    fn menu_mark_all(&mut self) {
        if let Some(Modal::RepoMenu(menu)) = self.modal.as_mut() {
            menu.marked = (0..menu.rows.len()).collect();
        }
    }

    /// `n` in the repo menu: unmark every repo (empty means "everything").
    fn menu_mark_none(&mut self) {
        if let Some(Modal::RepoMenu(menu)) = self.modal.as_mut() {
            menu.marked.clear();
        }
    }

    /// The repos a fetch reads: the saved selection, or everything when the
    /// selection is empty. Invalid slugs are skipped, never trusted: the state
    /// file is user-editable and a typo must not break the fetch.
    pub fn fetch_scope(all: &[RepoSlug], selected: &[String]) -> Vec<RepoSlug> {
        if selected.is_empty() {
            return all.to_vec();
        }
        selected
            .iter()
            .filter_map(|s| match RepoSlug::parse(s) {
                Ok(slug) => Some(slug),
                Err(_) => {
                    tracing::warn!(
                        slug = s.as_str(),
                        "state file holds an invalid slug, ignored"
                    );
                    None
                }
            })
            .collect()
    }

    /// `Enter` in the repo menu: the checked rows become the selection. An
    /// empty check means "everything", like an empty state file. The list
    /// drops the deselected repos at once, then re-reads from the server;
    /// the loop saves the file.
    pub fn apply_repo_selection(&mut self, selected: Vec<RepoSlug>) {
        // A new list resets the cursor (see `set_repos`), so no row survives
        // a reorder — that reset is why no `last_repo` identity is needed.
        let shown = if selected.is_empty() {
            self.all_repos.clone()
        } else {
            selected.clone()
        };
        self.selection_applied = Some(
            selected
                .iter()
                .map(|slug| slug.as_str().to_string())
                .collect(),
        );
        self.modal = None;
        self.set_repos(shown.clone());
        // The items in memory are from the previous selection: drop the
        // deselected repos at once, so the list never shows what the user
        // just unchecked. The refresh below re-reads from the server.
        self.items
            .retain(|i| shown.iter().any(|s| s.as_str() == i.repo));
        self.set_lengths(self.repos.len(), self.visible_items().len());
        self.loading = true;
        self.refresh_requested = true;
    }

    /// Merge a fresh `gh repo list` into the menu rows: new slugs append in
    /// server order, the configured order never reshuffles. An open repo
    /// menu rebuilds around the same marked slugs (not rows), so a refresh
    /// landing mid-pick never steals the cursor. Returns the new slugs —
    /// the loop saves those to the config file.
    pub fn merge_repo_list(&mut self, slugs: Vec<RepoSlug>) -> Vec<RepoSlug> {
        let fresh: Vec<RepoSlug> = slugs
            .into_iter()
            .filter(|slug| !self.all_repos.contains(slug))
            .collect();
        if fresh.is_empty() {
            self.status = "the repo list is up to date".to_string();
            return fresh;
        }
        self.all_repos.extend(fresh.iter().cloned());
        if let Some(Modal::RepoMenu(menu)) = self.modal.as_mut() {
            let marked: Vec<RepoSlug> = menu
                .marked
                .iter()
                .filter_map(|i| menu.rows.get(*i).cloned())
                .collect();
            menu.rows.clone_from(&self.all_repos);
            menu.cursor = menu.cursor.min(menu.rows.len().saturating_sub(1));
            menu.marked = self
                .all_repos
                .iter()
                .enumerate()
                .filter(|(_, slug)| marked.contains(slug))
                .map(|(i, _)| i)
                .collect();
        }
        self.repos_save_requested = true;
        let n = fresh.len();
        self.status = format!(
            "{} new {} — pick it with Space",
            n,
            if n == 1 { "repo" } else { "repos" }
        );
        fresh
    }

    /// Click a list row: the cursor jumps there. Same side effects as the
    /// keyboard move (finished card dismissed, body preloaded), so mouse
    /// and keys never disagree about what the row means.
    pub fn click_list_row(&mut self, idx: usize) {
        if idx >= self.visible_items().len() {
            return;
        }
        self.list_cursor = idx;
        self.dismiss_finished_card();
        self.preload_detail();
        self.session_dirty = true;
    }

    /// Click a repos row: same as moving there — the filter follows it.
    pub fn click_repos_row(&mut self, idx: usize) {
        if idx >= self.repos.len() {
            return;
        }
        self.repos_cursor = idx;
        self.apply_repo_filter();
        self.session_dirty = true;
    }

    /// The item under the cursor, if any.
    pub fn selected_item(&self) -> Option<&Item> {
        self.visible_items().get(self.list_cursor).copied()
    }

    /// The item the right column acts on. An open tab owns the detail:
    /// switching to one (`t`, click, `Enter`) shows its issue — even
    /// across a repo filter, even closed (the fetch is OPEN-only, so a
    /// vanished tab item is read from the retained copy until its tab
    /// closes). Scrolling never drags the detail off a tab. Without tabs
    /// the detail follows the row under the list cursor, loading as it
    /// moves.
    pub fn effective_item(&self) -> Option<&Item> {
        if let Some(tab) = self.active_tab.and_then(|idx| self.tabs.get(idx)) {
            if let Some(found) = self
                .items
                .iter()
                .find(|i| i.repo == tab.repo && i.number == tab.number)
            {
                return Some(found);
            }
            if let Some(kept) = self
                .retained
                .iter()
                .find(|i| i.repo == tab.repo && i.number == tab.number)
            {
                return Some(kept);
            }
        }
        self.selected_item()
    }

    /// Open a tab for the row under the cursor, or activate it when open.
    /// `Enter` in the list opens; the detail and the left column follow it.
    /// `Enter` on the already-active tab changes nothing: the cached body
    /// is reused instead of being cleared and fetched again.
    pub fn open_tab_for_selected(&mut self) {
        self.dismiss_finished_card();
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        // Enter on a row means "work on this": the tab opens and focus
        // moves to the right column, on the tab.
        self.pane = Pane::Detail;
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.repo == item.repo && t.number == item.number)
        {
            if Some(idx) == self.active_tab {
                self.request_detail();
                return;
            }
            self.active_tab = Some(idx);
        } else {
            self.tabs.push(Tab {
                repo: item.repo.clone(),
                number: item.number,
            });
            self.active_tab = Some(self.tabs.len() - 1);
        }
        // The new tab may carry a finished card: selecting it shows the
        // item again, like every other move.
        self.dismiss_finished_card();
        self.session_dirty = true;
        // The left column is the user's: opening a tab never reshapes it.
        // Load the body for the newly active tab when missing. Opening a tab
        // is explicit, so the status says so even when a quiet preload got
        // there first — same upgrade as `request_detail`.
        let key = (item.repo.clone(), item.number);
        if self.detail_for.as_ref() != Some(&key) {
            if self.detail_requested.as_ref() != Some(&key) {
                self.detail_requested = Some(key);
            }
            self.status = format!("loading #{}…", item.number);
        }
    }

    /// Activate tab `idx` (clamped): the tab owns the detail, so it shows
    /// the tab's issue and loads the body when missing. The agent box
    /// follows the tab the same way. The left column is left alone:
    /// navigating tabs must not reshuffle the repo selection (an
    /// "all repos" view stays "all repos").
    pub fn activate_tab(&mut self, idx: usize) {
        self.dismiss_finished_card();
        if self.tabs.is_empty() {
            self.active_tab = None;
            return;
        }
        let idx = idx.min(self.tabs.len() - 1);
        self.active_tab = Some(idx);
        self.session_dirty = true;
        // The detail shows the tab's issue: load its body when missing.
        if let Some(tab) = self.tabs.get(idx).cloned() {
            if let Some(item) = self
                .items
                .iter()
                .find(|i| i.repo == tab.repo && i.number == tab.number)
                .cloned()
            {
                let key = (item.repo.clone(), item.number);
                if self.detail_for.as_ref() != Some(&key)
                    && self.detail_requested.as_ref() != Some(&key)
                {
                    self.detail_requested = Some(key);
                }
            }
        }
    }

    pub fn next_tab(&mut self) {
        let Some(cur) = self.active_tab else {
            if !self.tabs.is_empty() {
                self.activate_tab(0);
            }
            return;
        };
        if self.tabs.is_empty() {
            return;
        }
        self.activate_tab((cur + 1) % self.tabs.len());
    }

    pub fn prev_tab(&mut self) {
        let Some(cur) = self.active_tab else {
            if !self.tabs.is_empty() {
                self.activate_tab(0);
            }
            return;
        };
        if self.tabs.is_empty() {
            return;
        }
        let prev = if cur == 0 {
            self.tabs.len() - 1
        } else {
            cur - 1
        };
        self.activate_tab(prev);
    }

    /// Close the active tab. Returns true when a tab was closed.
    pub fn close_active_tab(&mut self) -> bool {
        let Some(idx) = self.active_tab else {
            return false;
        };
        if idx >= self.tabs.len() {
            self.active_tab = None;
            return false;
        }
        let closed = self.tabs.remove(idx);
        // The closed tab's agent side goes with it: the loop kills a
        // session whose tab is gone, so no agent outlives its box.
        self.tab_agents
            .remove(&(closed.repo.clone(), closed.number));
        // A body belonging to the closed tab must not linger under the next
        // tab's fields.
        if self.detail_for.as_ref() == Some(&(closed.repo.clone(), closed.number)) {
            self.detail.clear();
            self.detail_for = None;
        }
        if self.detail_requested.as_ref() == Some(&(closed.repo.clone(), closed.number)) {
            self.detail_requested = None;
        }
        if self.tabs.is_empty() {
            self.active_tab = None;
        } else {
            let next = idx.min(self.tabs.len() - 1);
            self.active_tab = Some(next);
            // Load the newly active tab's body when missing.
            if let Some(tab) = self.tabs.get(next).cloned()
                && let Some(item) = self
                    .items
                    .iter()
                    .find(|i| i.repo == tab.repo && i.number == tab.number)
                    .cloned()
            {
                let key = (item.repo.clone(), item.number);
                if self.detail_for.as_ref() != Some(&key)
                    && self.detail_requested.as_ref() != Some(&key)
                {
                    self.detail_requested = Some(key);
                }
            }
        }
        self.session_dirty = true;
        true
    }

    /// One-line label per tab, with the live title when the item is loaded.
    pub fn tab_labels(&self) -> Vec<String> {
        self.tab_infos()
            .into_iter()
            .map(|info| info.label)
            .collect()
    }

    /// One entry per open tab for the tabs strip: the label, the kind (for
    /// colour) and whether it is the active one (for emphasis).
    pub fn tab_infos(&self) -> Vec<TabInfo> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let found = self
                    .items
                    .iter()
                    .find(|item| item.repo == tab.repo && item.number == tab.number);
                let label = match found {
                    Some(item) => {
                        let short: String = item.title.chars().take(28).collect();
                        format!("#{} {short}", tab.number)
                    }
                    None => format!("#{} {}", tab.number, tab.repo),
                };
                TabInfo {
                    label,
                    kind: found.map(|item| item.kind),
                    active: Some(i) == self.active_tab,
                    has_worktree: found.is_some_and(|item| {
                        self.has_worktree(&item.repo, item.head_ref_name.as_deref())
                    }),
                    has_agent: self
                        .tab_agents
                        .get(&(tab.repo.clone(), tab.number))
                        .is_some_and(|agent| agent.running),
                }
            })
            .collect()
    }

    /// The agent pane shows only while an agent is being arranged or is
    /// running. A finished run stays in the detail as a card; the pane
    /// itself is for work in progress, so idle screens keep the detail tall.
    pub fn agent_active(&self) -> bool {
        self.agent_requested.is_some()
            || self.agent_go.is_some()
            || self.agent_resume.is_some()
            || self.tab_agents.values().any(|a| {
                a.running || matches!(&a.record, Some(r) if r.status == AgentStatus::Running)
            })
    }

    /// The agent box belongs to the active tab, not the app: it shows while
    /// the tab's agent operates (a live session) or its finished output is
    /// still on screen (an undismissed record). Any other tab gives the
    /// whole right column to the detail — an idle tab never rents a box.
    /// (`w` focuses the pane explicitly, which the draw keeps regardless.)
    pub fn agent_box_shown(&self) -> bool {
        let Some(agent) = self.active_agent() else {
            return false;
        };
        if agent.running {
            return true;
        }
        agent.record.is_some() && agent.dismissed_for.is_none()
    }

    /// The tab's key in `TabAgent`: agents live per tab, so switching
    /// tabs switches boxes.
    pub fn tab_key(repo: &str, number: u64) -> (String, u64) {
        (repo.to_string(), number)
    }

    /// This tab's agent side, if the tab ever launched one.
    pub fn tab_agent(&self, key: &(String, u64)) -> Option<&TabAgent> {
        self.tab_agents.get(key)
    }

    /// The active tab's key, when a tab is open.
    pub fn active_key(&self) -> Option<(String, u64)> {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .map(|tab| (tab.repo.clone(), tab.number))
    }

    /// The active tab's agent side, if it ever launched one.
    pub fn active_agent(&self) -> Option<&TabAgent> {
        self.active_key().and_then(|key| self.tab_agents.get(&key))
    }

    /// The active tab's agent side, mutably.
    pub fn active_agent_mut(&mut self) -> Option<&mut TabAgent> {
        let key = self.active_key()?;
        self.tab_agents.get_mut(&key)
    }

    /// Remember the agent launched in this tree, so reopening it offers the
    /// session back. Recorded on every launch, resume included.
    pub fn remember_worktree_agent(&mut self, plan: &AgentPlan, path: &std::path::Path) {
        self.last_agents.insert(
            path.display().to_string(),
            crate::agent::LastAgent {
                agent: plan.agent.id.name().to_string(),
                profile: plan.profile.clone(),
            },
        );
        self.session_dirty = true;
    }

    /// A deleted tree has no session to reopen: drop its entry so a later
    /// tree at the same path starts without a stale offer.
    pub fn forget_worktree(&mut self, path: &std::path::Path) {
        if self
            .last_agents
            .remove(&path.display().to_string())
            .is_some()
        {
            self.session_dirty = true;
        }
    }

    /// A renamed tree keeps its session: the entry follows the directory.
    pub fn move_worktree(&mut self, old: &std::path::Path, new: &std::path::Path) {
        if let Some(last) = self.last_agents.remove(&old.display().to_string()) {
            self.last_agents.insert(new.display().to_string(), last);
            self.session_dirty = true;
        }
    }

    /// Copy the session fields into `state` so the loop can persist them.
    pub fn capture_session(&self, state: &mut crate::state::State) {
        state.pane = match self.pane {
            Pane::Repos => "repos".to_string(),
            Pane::List => "list".to_string(),
            Pane::Detail => "detail".to_string(),
            Pane::Agent => "agent".to_string(),
        };
        state.repos_cursor = self.repos_cursor;
        state.list_cursor = self.list_cursor;
        state.query = self.query.clone();
        state.repo_filter = match &self.repo_filter {
            RepoFilter::All => None,
            RepoFilter::One(slug) => Some(slug.as_str().to_string()),
        };
        state.tabs = self
            .tabs
            .iter()
            .map(|t| crate::state::TabState {
                repo: t.repo.clone(),
                number: t.number,
            })
            .collect();
        state.active_tab = self.active_tab;
        state.col_pct = self.col_pct;
        state.row_pct = self.row_pct;
        state.last_agents = self.last_agents.clone();
    }

    /// Restore pane, cursors, filter, query and tabs from a saved session.
    /// Unknown panes fall back to `list`; unknown slugs fall back to `all`;
    /// out-of-range cursors clamp — a stale session never breaks startup.
    pub fn restore_session(&mut self, state: &crate::state::State) {
        self.pane = match state.pane.as_str() {
            "repos" => Pane::Repos,
            "detail" => Pane::Detail,
            "agent" => Pane::Agent,
            _ => Pane::List,
        };
        self.query = state.query.clone();
        if let Some(slug_text) = state.repo_filter.as_ref().filter(|s| !s.is_empty()) {
            if let Ok(slug) = RepoSlug::parse(slug_text) {
                // Only keep the filter when the repo is still configured.
                let known = self.repos.iter().any(|row| match row {
                    RepoRow::Repo(s) => s == &slug,
                    RepoRow::All => false,
                });
                if known {
                    self.repo_filter = RepoFilter::One(slug.clone());
                    if let Some(pos) = self.repos.iter().position(|row| match row {
                        RepoRow::Repo(s) => s == &slug,
                        RepoRow::All => false,
                    }) {
                        self.repos_cursor = pos;
                    }
                }
            }
        }
        // A saved cursor without a saved filter still moves home.
        if matches!(self.repo_filter, RepoFilter::All) {
            self.repos_cursor = state.repos_cursor.min(self.repos.len().saturating_sub(1));
        }
        self.list_cursor = state.list_cursor;
        // The cross geometry comes back too, clamped like a fresh drag:
        // a shrunken terminal must not restore an unusable split.
        self.col_pct = state.col_pct.clamp(15, 85);
        self.row_pct = state.row_pct.clamp(15, 85);
        self.tabs = state
            .tabs
            .iter()
            .filter_map(|t| {
                if RepoSlug::parse(&t.repo).is_ok() {
                    Some(Tab {
                        repo: t.repo.clone(),
                        number: t.number,
                    })
                } else {
                    None
                }
            })
            .collect();
        self.active_tab = state.active_tab.filter(|idx| *idx < self.tabs.len());
        self.last_agents = state.last_agents.clone();
        self.set_lengths(self.repos.len(), self.visible_items().len());
        self.refresh_status();
    }

    /// One row of the actions menu: the fully specified choice. No strings
    /// cross this boundary — a row IS the typed parameters the action runs
    /// with, so the menu cannot promise what the type cannot build.
    pub fn action_menu_rows(kind: crate::model::ItemKind) -> Vec<ActionPick> {
        match kind {
            crate::model::ItemKind::Pr => [
                (MergeMethod::Squash, false),
                (MergeMethod::Merge, false),
                (MergeMethod::Rebase, false),
                (MergeMethod::Squash, true),
                (MergeMethod::Merge, true),
                (MergeMethod::Rebase, true),
            ]
            .into_iter()
            .map(|(method, delete)| ActionPick::Merge(method, delete))
            .collect(),
            // Issues are closed, never merged — the menu offers exactly that.
            crate::model::ItemKind::Issue => vec![ActionPick::Close],
        }
    }

    /// `m` opens the menu on the selected item. Issues offer only closing;
    /// anything without a parseable repo cannot be acted on at all.
    pub fn open_action_menu(&mut self) {
        // The actions belong to the list box: from anywhere else (detail,
        // agent, repos) the key is a mistake, not a shortcut.
        if self.pane != Pane::List {
            self.status = "move to the list first".to_string();
            return;
        }
        let Some(item) = self.effective_item() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if RepoSlug::parse(&item.repo).is_err() {
            self.status = format!("cannot act on {}", item.repo);
            return;
        }
        self.modal = Some(Modal::ActionMenu(ActionMenu {
            rows: Self::action_menu_rows(item.kind),
            cursor: 0,
        }));
    }

    fn menu_step_action(&mut self, delta: i32) {
        if let Some(Modal::ActionMenu(menu)) = self.modal.as_mut() {
            let len = menu.rows.len();
            menu.cursor = step(menu.cursor, delta, len);
        }
    }

    /// Cursor moves for the agent and worktree menus: same wrap as every
    /// other menu (first after last), so `Up` on the first row is not dead.
    fn menu_step_agent(&mut self, delta: i32) {
        if let Some(Modal::AgentMenu(menu)) = self.modal.as_mut() {
            let len = menu.rows.len();
            menu.cursor = step(menu.cursor, delta, len);
        }
    }

    fn menu_step_worktree(&mut self, delta: i32) {
        if let Some(Modal::Worktrees(menu)) = self.modal.as_mut() {
            let len = menu.rows.len();
            menu.cursor = step(menu.cursor, delta, len);
        }
    }

    /// `Enter` on a worktree row: work on it. Inside the `g` flow the agent
    /// is already picked and the loop launches on the row's path; opened
    /// directly with `W`, the agent menu comes first with the tree preset.
    /// The path travels with the choice instead of being re-derived from
    /// the branch: after an outside rename the two no longer match.
    fn use_worktree_row(&mut self) {
        if let Some(Modal::Worktrees(menu)) = self.modal.take() {
            let Some(row) = menu.rows.get(menu.cursor).cloned() else {
                self.modal = Some(Modal::Worktrees(menu));
                return;
            };
            // The tree remembers its agent: offer the previous session
            // back before any agent question — `y` reopens it with
            // nothing sent, `n` goes back to choosing. A missing binary
            // skips the offer silently and chooses as usual.
            if let Some(last) = self
                .last_agents
                .get(&row.path.display().to_string())
                .cloned()
            {
                if let Some(choice) = self
                    .agent_choices()
                    .into_iter()
                    .find(|c| c.id.name() == last.agent)
                {
                    let mut plan = menu.plan;
                    plan.branch = row.branch.as_str().to_string();
                    plan.agent = choice;
                    plan.profile = last.profile.clone();
                    let question = format!(
                        "reopen the last {} session here ({})? (y = resume, nothing sent · n = choose agent)",
                        last.agent,
                        row.path.display()
                    );
                    self.modal = Some(Modal::Confirm(question));
                    self.pending = Some(Pending::Resume {
                        plan,
                        path: row.path,
                        preset: menu.agent,
                    });
                    return;
                }
            }
            match menu.agent {
                Some(agent) => {
                    let mut plan = menu.plan;
                    plan.branch = row.branch.as_str().to_string();
                    plan.agent = agent;
                    self.agent_place = Some((plan, row.path));
                }
                None => {
                    let rows = self.agent_choices();
                    if rows.is_empty() {
                        self.status =
                            "no agent binaries found (hermes, pi, omp, opencode, agy)".to_string();
                        return;
                    }
                    let mut plan = menu.plan;
                    plan.branch = row.branch.as_str().to_string();
                    let place = row.path;
                    self.modal = Some(Modal::AgentMenu(AgentMenu {
                        rows,
                        cursor: 0,
                        plan,
                        place: Some(place),
                    }));
                }
            }
        }
    }

    /// `n` in the worktree menu: name a new parallel tree. The suggestion
    /// is the head branch — confirming untouched names it the usual way.
    fn name_new_worktree(&mut self) {
        if let Some(Modal::Worktrees(menu)) = self.modal.take() {
            let suggestion = menu.plan.branch.clone();
            self.modal = Some(Modal::NameInput(NameInput {
                value: suggestion,
                purpose: NamePurpose::Create {
                    agent: menu.agent,
                    plan: menu.plan,
                },
            }));
        }
    }

    /// `r` in the worktree menu: rename the row's tree. The prompt opens on
    /// the old name; confirming renames the branch, then moves the tree.
    fn rename_worktree_row(&mut self) {
        if let Some(Modal::Worktrees(menu)) = self.modal.take() {
            let Some(row) = menu.rows.get(menu.cursor).cloned() else {
                self.modal = Some(Modal::Worktrees(menu));
                return;
            };
            let Ok(slug) = crate::actions::RepoSlug::parse(&menu.plan.repo) else {
                self.status = format!("cannot rename in {}", menu.plan.repo);
                self.modal = Some(Modal::Worktrees(menu));
                return;
            };
            let value = row.branch.as_str().to_string();
            self.modal = Some(Modal::NameInput(NameInput {
                value,
                purpose: NamePurpose::Rename {
                    order: RenameDraft {
                        clone: menu.plan.clone.clone(),
                        slug,
                        old_branch: row.branch,
                        old_path: row.path,
                    },
                },
            }));
        }
    }

    /// `d` in the worktree menu: delete the row's tree after the usual
    /// confirmation. The branch is kept — deleting history is separate.
    fn delete_worktree_row(&mut self) {
        if let Some(Modal::Worktrees(menu)) = self.modal.take() {
            let Some(row) = menu.rows.get(menu.cursor).cloned() else {
                self.modal = Some(Modal::Worktrees(menu));
                return;
            };
            let order = crate::worktree::WorktreeDelete {
                clone: menu.plan.clone.clone(),
                branch: row.branch,
                path: row.path,
            };
            self.pending = Some(Pending::Delete(order.clone()));
            self.modal = Some(Modal::Confirm(format!(
                "delete worktree {}? the branch {} stays",
                order.path.display(),
                order.branch
            )));
        }
    }

    /// `Enter` in the name prompt. An invalid name keeps the prompt open
    /// with the reason — retyping beats reopening.
    fn confirm_name_input(&mut self) {
        let Some(Modal::NameInput(input)) = self.modal.take() else {
            return;
        };
        match input.purpose {
            NamePurpose::Create { agent, mut plan } => {
                let name = input.value.trim();
                match crate::worktree::BranchName::parse(name) {
                    Ok(branch) => {
                        plan.branch = branch.as_str().to_string();
                        match agent {
                            // The `g` flow: the loop decides — existing
                            // tree launches straight in, missing tree gets
                            // the creation question.
                            Some(agent) => {
                                plan.agent = agent;
                                self.agent_requested = Some(plan);
                            }
                            // The standalone menu: the agent menu comes
                            // next, with this tree's path preset. The loop
                            // then launches or asks, same as above.
                            None => {
                                let root = match self.worktree_root.clone() {
                                    Some(root) => root,
                                    None => {
                                        self.status =
                                            "no worktree root — cannot place the name".to_string();
                                        return;
                                    }
                                };
                                let slug = match crate::actions::RepoSlug::parse(&plan.repo) {
                                    Ok(slug) => slug,
                                    Err(_) => {
                                        self.status =
                                            format!("cannot place a tree in {}", plan.repo);
                                        return;
                                    }
                                };
                                match crate::worktree::resolve_in(
                                    &root,
                                    &plan.clone,
                                    &slug,
                                    &branch,
                                ) {
                                    Ok(place) => {
                                        let rows = self.agent_choices();
                                        if rows.is_empty() {
                                            self.status = "no agent binaries found (hermes, pi, omp, opencode, agy)".to_string();
                                            return;
                                        }
                                        self.modal = Some(Modal::AgentMenu(AgentMenu {
                                            rows,
                                            cursor: 0,
                                            plan,
                                            place: Some(place),
                                        }));
                                    }
                                    Err(err) => {
                                        self.status = format!("bad worktree name: {err:#}");
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        self.status = format!("bad worktree name: {err:#}");
                        self.modal = Some(Modal::NameInput(NameInput {
                            purpose: NamePurpose::Create { agent, plan },
                            value: input.value,
                        }));
                    }
                }
            }
            NamePurpose::Rename { order } => {
                let name = input.value.trim();
                match crate::worktree::BranchName::parse(name) {
                    Ok(new_branch) => {
                        if new_branch == order.old_branch {
                            self.status = "same name — nothing to rename".to_string();
                            return;
                        }
                        let root = match self.worktree_root.clone() {
                            Some(root) => root,
                            None => {
                                self.status =
                                    "no worktree root — cannot place the rename".to_string();
                                return;
                            }
                        };
                        match crate::worktree::resolve_in(
                            &root,
                            &order.clone,
                            &order.slug,
                            &new_branch,
                        ) {
                            Ok(new_path) => {
                                if new_path.exists() {
                                    self.status = format!("already exists: {}", new_path.display());
                                    return;
                                }
                                self.rename_go = Some(crate::worktree::WorktreeRename {
                                    clone: order.clone,
                                    old_branch: order.old_branch,
                                    new_branch,
                                    old_path: order.old_path,
                                    new_path,
                                });
                            }
                            Err(err) => {
                                self.status = format!("bad worktree name: {err:#}");
                            }
                        }
                    }
                    Err(err) => {
                        self.status = format!("bad worktree name: {err:#}");
                        self.modal = Some(Modal::NameInput(NameInput {
                            purpose: NamePurpose::Rename { order },
                            value: input.value,
                        }));
                    }
                }
            }
        }
    }

    /// `Enter` in the actions menu: the picked row becomes the action and
    /// goes through the same confirmation as `a` — one dialog, one argv.
    fn apply_action_pick(&mut self) {
        if let Some(Modal::ActionMenu(menu)) = self.modal.take() {
            let Some(item) = self.effective_item().cloned() else {
                return;
            };
            let Some(pick) = menu.rows.get(menu.cursor) else {
                return;
            };
            let Ok(slug) = RepoSlug::parse(&item.repo) else {
                self.status = format!("cannot act on {}", item.repo);
                return;
            };
            self.confirm_action(action_for_pick(slug, item.number, pick));
        }
    }

    /// The quiet request: like `request_detail` but without the status line.
    /// Cursor moves, filter changes and fresh fetches call this, so the
    /// body is on its way before anyone presses Enter; Enter keeps the loud
    /// one because an explicit ask deserves a visible answer.
    pub fn preload_detail(&mut self) {
        let Some(item) = self.effective_item().cloned() else {
            return;
        };
        let key = (item.repo.clone(), item.number);
        if self.detail_for.as_ref() == Some(&key) {
            return;
        }
        if self.detail_requested.as_ref() == Some(&key) {
            return;
        }
        self.detail_requested = Some(key);
    }

    /// Ask for the selected item's body, unless it is already loaded.
    /// Follows the active tab when one is open.
    pub fn request_detail(&mut self) {
        let Some(item) = self.effective_item().cloned() else {
            return;
        };
        let key = (item.repo.clone(), item.number);
        let number = item.number;
        if self.detail_for.as_ref() == Some(&key) {
            return;
        }
        if self.detail_requested.as_ref() == Some(&key) {
            // Already on its way (preloaded): an explicit Enter still gets
            // the visible answer.
            self.status = format!("loading #{number}…");
            return;
        }
        self.detail_requested = Some(key);
        self.status = format!("loading #{number}…");
    }

    /// Store a body that arrived for `(repo, number)`; a late answer for a
    /// previous selection is dropped instead of overwriting the current one.
    pub fn apply_detail(&mut self, repo: &str, number: u64, body: String) {
        let key = (repo.to_string(), number);
        // A late answer for a previous selection must not overwrite the one the
        // user is looking at now.
        if let Some(pending) = &self.detail_requested {
            if pending != &key {
                return;
            }
        }
        self.detail_requested = None;
        self.detail = body;
        self.detail_for = Some(key);
        self.detail_scroll = 0;
        // The "loading #N…" line referred to exactly this body: once it is
        // here, leaving it would say "still loading" about a loaded screen.
        // Anything else the status says belongs to someone else.
        if self.status == format!("loading #{number}…") {
            self.status.clear();
        }
    }

    /// How many diff lines the overlay keeps: a huge diff must not fill
    /// memory, and nobody reads ten thousand lines in a box anyway.
    pub const DIFF_MAX_LINES: usize = 200;

    /// `d` on the selected row: show the PR's diff in an overlay. Fetched
    /// once per item (like the body) and cached; reopening the same PR
    /// shows the cache instead of hitting the network again. Issues have
    /// no diff and are refused with the reason.
    /// Follows the active tab when one is open.
    pub fn request_diff(&mut self) {
        let Some(item) = self.effective_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != crate::model::ItemKind::Pr {
            self.status = "diffs are for pull requests — issues have no diff".to_string();
            return;
        }
        let key = (item.repo.clone(), item.number);
        self.diff_scroll = 0;
        if self.diff_for.as_ref() == Some(&key) {
            self.modal = Some(Modal::Diff);
        } else {
            self.diff_text = "loading the diff…".to_string();
            self.diff_total = 0;
            self.diff_truncated = false;
            self.diff_for = None;
            self.diff_requested = Some(key);
            self.modal = Some(Modal::Diff);
        }
    }

    /// Store a diff that arrived for `(repo, number)`; a late answer for a
    /// previous selection is dropped instead of overwriting the current
    /// one. The text is capped at `DIFF_MAX_LINES` with the total kept, so
    /// the overlay can say how much it hides.
    pub fn apply_diff(&mut self, repo: &str, number: u64, result: Result<String, FetchError>) {
        let key = (repo.to_string(), number);
        if let Some(pending) = &self.diff_requested {
            if pending != &key {
                return;
            }
        }
        self.diff_requested = None;
        match result {
            Ok(text) => {
                let lines: Vec<&str> = text.lines().collect();
                let total = lines.len();
                if lines.is_empty() {
                    self.diff_text = "(no changes)".to_string();
                    self.diff_total = 0;
                    self.diff_truncated = false;
                } else {
                    let kept: Vec<&str> = lines.into_iter().take(Self::DIFF_MAX_LINES).collect();
                    self.diff_truncated = total > kept.len();
                    self.diff_total = total;
                    self.diff_text = kept.join("\n");
                }
                self.diff_for = Some(key);
            }
            Err(err) => {
                self.diff_for = None;
                self.diff_text = format!("diff failed: {err}");
                self.diff_total = 0;
                self.diff_truncated = false;
            }
        }
        self.diff_scroll = 0;
    }

    /// `g` asks for an agent handoff on the selected row. Validation happens
    /// here; the terminal, git and the seed file belong to the loop, which
    /// consumes `agent_requested` (outbox, like `detail_requested`). Nothing
    /// is written and nothing is spawned by pressing `g`: it opens the agent
    /// menu, and the menu leads to the worktree choice — unless a previous
    /// session for this issue/PR offers itself first.
    /// Follows the active tab when one is open.
    ///
    /// The single previous session behind this issue/PR, if exactly one
    /// open tree carries a recorded agent: `g` offers it before any menu
    /// — y reopens with nothing sent, n chooses as usual. Zero or several:
    /// the menus disambiguate, as before.
    fn resume_candidate(
        &self,
        item: &crate::model::Item,
    ) -> Option<(AgentPlan, std::path::PathBuf)> {
        let root = self.worktree_root.as_deref()?;
        let slug = crate::actions::RepoSlug::parse(&item.repo).ok()?;
        let branch = item
            .head_ref_name
            .clone()
            .unwrap_or_else(|| format!("issue-{}", item.number));
        let want = crate::worktree::BranchName::parse(&branch).ok()?;
        let mut found = Vec::new();
        for path in crate::worktree::list_worktrees(root, &slug) {
            let Some(here) = crate::worktree::worktree_branch(&path) else {
                continue;
            };
            if here == want && self.last_agents.contains_key(&path.display().to_string()) {
                found.push(path);
            }
        }
        let [path] = found.as_slice() else {
            return None;
        };
        let last = self.last_agents.get(&path.display().to_string())?;
        let choice = self
            .agent_choices()
            .into_iter()
            .find(|c| c.id.name() == last.agent)?;
        let clone = crate::agent::find_clone(&self.clone_roots, &item.repo).ok()?;
        let plan = AgentPlan {
            repo: item.repo.clone(),
            number: item.number,
            url: item.url.clone(),
            branch: want.as_str().to_string(),
            profile: last.profile.clone(),
            agent: choice,
            clone,
        };
        Some((plan, path.clone()))
    }

    /// Follows the active tab when one is open.
    pub fn request_agent(&mut self) {
        let Some(item) = self.effective_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        // One agent per tab: a second `g` on the same tab refuses instead
        // of orphaning the first. Other tabs keep running untouched.
        let key = Self::tab_key(&item.repo, item.number);
        if self.tab_agents.get(&key).is_some_and(|a| a.running) {
            self.status = "an agent is already running in this tab".to_string();
            return;
        }
        // No local clone: offer to clone it, then continue here — the same
        // question `W` asks, asked up front instead of erroring.
        if crate::agent::find_clone(&self.clone_roots, &item.repo).is_err() {
            self.offer_clone(&item.repo, CloneThen::AgentMenu);
            return;
        }
        // A previous session for this exact issue/PR: offer it before any
        // menu — the app already knows who ran here.
        if let Some((plan, path)) = self.resume_candidate(&item) {
            let agent = plan.agent.id.name();
            self.modal = Some(Modal::Confirm(format!(
                "reopen the last {agent} session here ({})? (y = resume, nothing sent · n = choose agent)",
                path.display()
            )));
            self.pending = Some(Pending::Resume {
                plan,
                path,
                preset: None,
            });
            return;
        }
        let hermes = self.default_hermes();
        match plan_agent(
            &item.repo,
            item.number,
            &item.url,
            item.kind,
            item.head_ref_name.as_deref(),
            PlanInputs {
                profile: &self.agent_profile,
                roots: &self.clone_roots,
                default_agent: hermes,
            },
        ) {
            Ok(plan) => self.open_agent_menu(plan),
            Err(reason) => self.status = reason,
        }
    }

    /// The agent every flow starts with until a row is confirmed: the
    /// resolved hermes binary, or the bare name (resolved again at launch —
    /// a missing binary then fails with its own message instead of refusing
    /// the menu now).
    fn default_hermes(&self) -> AgentChoice {
        match self.binaries.as_ref().and_then(|b| b.hermes.clone()) {
            Some(hermes) => AgentChoice {
                id: AgentId::Hermes,
                program: hermes,
            },
            None => AgentChoice {
                id: AgentId::Hermes,
                program: std::path::PathBuf::from("hermes"),
            },
        }
    }

    /// The installed agent binaries as menu rows, hermes first. Resolved now
    /// (not at startup), so an install mid-session just appears.
    fn agent_choices(&self) -> Vec<AgentChoice> {
        let mut rows = Vec::new();
        if let Some(hermes) = self.binaries.as_ref().and_then(|b| b.hermes.clone()) {
            rows.push(AgentChoice {
                id: AgentId::Hermes,
                program: hermes,
            });
        }
        for id in AgentId::all() {
            if id == AgentId::Hermes {
                continue;
            }
            if let Ok(program) = crate::binaries::resolve(id.binary()) {
                rows.push(AgentChoice { id, program });
            }
        }
        rows
    }

    /// `g` validated: show the agent menu, or say why there is nothing to show.
    fn open_agent_menu(&mut self, plan: AgentPlan) {
        let rows = self.agent_choices();
        if rows.is_empty() {
            self.status = "no agent binaries found (hermes, pi, omp, opencode, agy)".to_string();
            return;
        }
        self.modal = Some(Modal::AgentMenu(AgentMenu {
            rows,
            cursor: 0,
            plan,
            place: None,
        }));
    }

    /// `s`: the settings menu — every toggleable setting, cursor on top.
    /// Always available: settings are app-wide, not per-row.
    pub fn open_settings(&mut self) {
        self.modal = Some(Modal::Settings(SettingsMenu {
            cursor: 0,
            rows: vec![
                Setting::SandboxAgents,
                Setting::FontSize,
                Setting::FontFamily,
            ],
        }));
    }

    /// Flip the setting under the cursor. The menu stays open on the new
    /// state; the loop persists it (`config_save_requested`, plus the Foot
    /// `font=` line for the font rows).
    fn toggle_setting(&mut self) {
        let Some(Modal::Settings(menu)) = &self.modal else {
            return;
        };
        let Some(setting) = menu.rows.get(menu.cursor).copied() else {
            return;
        };
        match setting {
            Setting::SandboxAgents => {
                self.sandbox_agents = !self.sandbox_agents;
                self.status = if self.sandbox_agents {
                    "sandboxed agents on".to_string()
                } else {
                    "sandboxed agents off".to_string()
                };
            }
            Setting::FontSize => {
                self.font_size = self.font_size.next();
                self.status = format!("terminal font size: {}", self.font_size.name());
                self.font_save_requested = true;
            }
            Setting::FontFamily => {
                if self.fonts.is_empty() {
                    self.status = "no monospace fonts found".to_string();
                    return;
                }
                let pos = self
                    .fonts
                    .iter()
                    .position(|f| f == &self.font_family)
                    .map(|i| (i + 1) % self.fonts.len())
                    .unwrap_or(0);
                self.font_family = self.fonts[pos].clone();
                self.status = format!("terminal font: {}", self.font_family);
                self.font_save_requested = true;
            }
        }
        self.config_save_requested = true;
    }

    /// `W` with the repos pane focused: a new worktree for the repo under
    /// the cursor. The branch is suggested (`<repo>-dev`, editable), the
    /// agent menu follows with the tree preset — confirming an agent asks
    /// the creation question and launches, `Esc` aborts the whole thing.
    /// `All` is refused: a tree belongs to one repo.
    pub fn open_repo_worktree(&mut self) {
        let Some(row) = self.repos.get(self.repos_cursor).cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let RepoRow::Repo(slug) = row else {
            self.status = "pick one repo first — All is not a place".to_string();
            return;
        };
        self.open_repo_worktree_for(slug.as_str());
    }

    /// No local clone: offer to clone it, then continue (`then`) — the repo
    /// tree flow, or the agent menu the user asked for. One question, one
    /// background clone, shared by both flows.
    fn offer_clone(&mut self, repo: &str, then: CloneThen) {
        let Some(root) = self.clone_roots.first().cloned() else {
            self.status =
                "no local clone and no clone_roots configured — set one first".to_string();
            return;
        };
        let name = repo.split('/').next_back().unwrap_or(repo);
        let path = root.join(name);
        if path.exists() {
            self.status = format!(
                "no local clone for {repo} ({} exists but has no .git) — clone it by hand, then try again",
                path.display()
            );
            return;
        }
        self.modal = Some(Modal::Confirm(format!(
            "no local clone for {repo} — clone it into {}?",
            path.display()
        )));
        self.pending = Some(Pending::Clone {
            repo: repo.to_string(),
            path,
            then,
        });
    }

    /// The same flow for a named repo: used by the cursor above, by `W` on
    /// an empty list filtered to one repo, and by the empty-repo suggestion.
    /// Without a local clone there is nothing to branch from, so instead of
    /// erroring the app offers to clone first — `gh repo clone` into the
    /// first clone root — and continues the flow when the clone lands.
    pub fn open_repo_worktree_for(&mut self, repo: &str) {
        let slug = match RepoSlug::parse(repo) {
            Ok(slug) => slug,
            Err(_) => {
                self.status = format!("cannot grow a tree: invalid repo {repo}");
                return;
            }
        };
        let repo = slug.as_str();
        let clone = match crate::agent::find_clone(&self.clone_roots, repo) {
            Ok(clone) => clone,
            Err(_) => {
                self.offer_clone(repo, CloneThen::GrowTree);
                return;
            }
        };
        let name = repo.split('/').next_back().unwrap_or(repo);
        let suggestion = format!("{name}-dev");
        // A repo-level tree works on no PR: number 0 with the repo URL, so
        // the seed names the repo instead of inventing a PR number.
        let plan = AgentPlan {
            repo: repo.to_string(),
            number: 0,
            url: format!("https://github.com/{repo}"),
            branch: suggestion.clone(),
            profile: self.agent_profile.clone(),
            agent: self.default_hermes(),
            clone,
        };
        self.modal = Some(Modal::NameInput(NameInput {
            value: suggestion,
            purpose: NamePurpose::Create { agent: None, plan },
        }));
    }

    /// A merged PR leaves its local trees and head branch behind: while the
    /// merge is fresh, offer to delete what is no longer useful. Silent when
    /// nothing local remains — a PR merged elsewhere asks nothing.
    fn offer_merge_cleanup(&mut self, action: &Action) {
        let Action::Merge { repo, number, .. } = action else {
            return;
        };
        let Some(item) = self
            .items
            .iter()
            .find(|i| i.repo == repo.as_str() && i.number == *number)
            .cloned()
        else {
            return;
        };
        let Some(head) = item.head_ref_name.as_deref() else {
            return;
        };
        let Ok(branch) = crate::worktree::BranchName::parse(head) else {
            return;
        };
        let Some(binaries) = self.binaries.clone() else {
            return;
        };
        // The plan resolves the clone; the rows list the trees — the same
        // doors as the worktree menu, no special cases for cleanup. A plan
        // that cannot be built (no clone, no root) skips silently and keeps
        // the merge status line.
        let plan = match crate::agent::plan_agent(
            &item.repo,
            item.number,
            &item.url,
            item.kind,
            item.head_ref_name.as_deref(),
            crate::agent::PlanInputs {
                profile: &self.agent_profile,
                roots: &self.clone_roots,
                default_agent: self.default_hermes(),
            },
        ) {
            Ok(plan) => plan,
            Err(_) => return,
        };
        let trees: Vec<std::path::PathBuf> = self
            .repo_worktrees(&plan)
            .into_iter()
            .filter(|row| row.branch == branch)
            .map(|row| row.path)
            .collect();
        let branch_here =
            crate::worktree::has_branch(&binaries, &plan.clone, &branch, self.write_timeout);
        if trees.is_empty() && !branch_here {
            return;
        }
        let what = match (trees.len(), branch_here) {
            (0, _) => format!("local branch '{branch}'"),
            (1, false) => format!("tree {}", trees[0].display()),
            (1, true) => format!("tree {} and local branch '{branch}'", trees[0].display()),
            (n, false) => format!("{n} trees"),
            (n, true) => format!("{n} trees and local branch '{branch}'"),
        };
        let question = format!("merged #{number} — delete {what}?");
        self.modal = Some(Modal::Confirm(question));
        self.pending = Some(Pending::Cleanup(crate::worktree::CleanupOrder {
            repo: item.repo,
            number: item.number,
            branch,
            clone: plan.clone,
            trees,
        }));
    }

    /// `W` on the selected row: the worktree menu directly, without going
    /// through the agent menu first. Managing trees (rename, delete, or
    /// picking where to work) is not launching an agent.
    pub fn open_worktree_menu(&mut self) {
        // No agent_running refusal: managing trees is not launching one.
        // Launching a second agent stays refused, and the loop owns that.
        let Some(item) = self.effective_item().cloned() else {
            // Empty list, `W` with nowhere to point: when the filter names
            // one repo, grow a tree for it — same as `W` on that repo.
            // On "all repos" there is no single repo to grow: say so.
            // (Cloned first: the flow borrows mutably.)
            let filtered = match &self.repo_filter {
                RepoFilter::One(slug) => Some(slug.as_str().to_string()),
                RepoFilter::All => None,
            };
            match filtered {
                Some(repo) => {
                    self.open_repo_worktree_for(&repo);
                }
                None => {
                    self.status =
                        "empty list — filter to one repo first (Enter on it), then W grows a tree"
                            .to_string();
                }
            }
            return;
        };
        let hermes = self.default_hermes();
        // The agent inside is a placeholder until a row is confirmed: the
        // menu replaces it with the pick (or never reads it, for r/d).
        match plan_agent(
            &item.repo,
            item.number,
            &item.url,
            item.kind,
            item.head_ref_name.as_deref(),
            PlanInputs {
                profile: &self.agent_profile,
                roots: &self.clone_roots,
                default_agent: hermes,
            },
        ) {
            Ok(plan) => {
                let rows = self.repo_worktrees(&plan);
                self.modal = Some(Modal::Worktrees(WorktreeMenu {
                    agent: None,
                    plan,
                    rows,
                    cursor: 0,
                }));
            }
            Err(reason) => self.status = reason,
        }
    }

    /// `g` in the repos pane: every agent and past session on the repo
    /// under the cursor, open or closed — one contextual menu to reopen
    /// them. Rows come from disk, so closed issues/PRs show up exactly
    /// like open ones; `Enter` reopens (the recorded session first, with
    /// nothing sent) or picks an agent for a fresh run.
    pub fn open_repo_sessions_for(&mut self, repo: &str) {
        let slug = match RepoSlug::parse(repo) {
            Ok(slug) => slug,
            Err(_) => {
                self.status = format!("cannot list sessions: invalid repo {repo}");
                return;
            }
        };
        let repo = slug.as_str();
        let clone = match crate::agent::find_clone(&self.clone_roots, repo) {
            Ok(clone) => clone,
            Err(_) => {
                self.offer_clone(repo, CloneThen::RepoSessions);
                return;
            }
        };
        let name = repo.split('/').next_back().unwrap_or(repo);
        // A repo-level tree works on no PR: number 0 with the repo URL, so
        // the seed names the repo instead of inventing a PR number — the
        // same shape `W` grows. Empty branch: no row may claim "this PR".
        let plan = AgentPlan {
            repo: repo.to_string(),
            number: 0,
            url: format!("https://github.com/{repo}"),
            branch: String::new(),
            profile: self.agent_profile.clone(),
            agent: self.default_hermes(),
            clone,
        };
        let rows = self.repo_worktrees(&plan);
        if rows.is_empty() {
            // Nothing to reopen: fall through to naming a fresh tree, the
            // same suggestion `W` grows.
            let mut plan = plan;
            plan.branch = format!("{name}-dev");
            self.modal = Some(Modal::NameInput(NameInput {
                value: plan.branch.clone(),
                purpose: NamePurpose::Create { agent: None, plan },
            }));
            return;
        }
        self.modal = Some(Modal::Worktrees(WorktreeMenu {
            agent: None,
            plan,
            rows,
            cursor: 0,
        }));
    }

    /// True while any tab's agent runs in `path`: the sessions menu marks
    /// the live tree, so reopening does not double a running agent.
    pub fn tree_live(&self, path: &std::path::Path) -> bool {
        self.tab_agents.values().any(|agent| {
            agent.running
                && agent
                    .record
                    .as_ref()
                    .is_some_and(|record| record.worktree == path)
        })
    }

    /// The issue/PR a worktree branch belongs to, if the app knows it: an
    /// open item by head branch, a retained (closed, tabbed) one the same
    /// way, or the number straight from an `issue-N` branch. Closed work
    /// with no tab stays anonymous — the fetch never saw it.
    pub fn issue_ref_for_branch(&self, branch: &str) -> Option<u64> {
        if let Some(number) = branch
            .strip_prefix("issue-")
            .and_then(|rest| rest.parse::<u64>().ok())
        {
            return Some(number);
        }
        self.items
            .iter()
            .chain(self.retained.iter())
            .find(|i| i.head_ref_name.as_deref() == Some(branch))
            .map(|i| i.number)
    }

    /// An agent row was confirmed: with existing worktrees the user picks
    /// one (or manages them); without, they name the new one directly.
    fn pick_agent(&mut self, agent: AgentChoice, plan: AgentPlan) {
        let rows = self.repo_worktrees(&plan);
        if rows.is_empty() {
            self.modal = Some(Modal::NameInput(NameInput {
                value: plan.branch.clone(),
                purpose: NamePurpose::Create {
                    agent: Some(agent),
                    plan,
                },
            }));
        } else {
            self.modal = Some(Modal::Worktrees(WorktreeMenu {
                agent: Some(agent),
                plan,
                rows,
                cursor: 0,
            }));
        }
    }

    /// The repo's worktrees as menu rows: every directory with a `.git`
    /// file, branch read from the tree itself. Detached or unreadable trees
    /// are skipped — the agent cannot be placed on them reliably.
    fn repo_worktrees(&self, plan: &AgentPlan) -> Vec<crate::worktree::WorktreeRow> {
        let (Some(root), Ok(slug)) = (
            self.worktree_root.as_deref(),
            crate::actions::RepoSlug::parse(&plan.repo),
        ) else {
            return Vec::new();
        };
        crate::worktree::list_worktrees(root, &slug)
            .iter()
            .filter_map(|path| {
                let branch = crate::worktree::worktree_branch(path)?;
                Some(crate::worktree::WorktreeRow {
                    branch,
                    path: path.clone(),
                })
            })
            .collect()
    }

    /// True when a worktree directory with a `.git` entry exists for the
    /// item's branch: the marker in the issues list. Pure existence — the
    /// loop re-validates everything before launching.
    pub fn has_worktree(&self, repo: &str, branch: Option<&str>) -> bool {
        let (Some(root), Some(branch), Ok(slug)) = (
            self.worktree_root.as_deref(),
            branch,
            crate::actions::RepoSlug::parse(repo),
        ) else {
            return false;
        };
        let Ok(branch) = crate::worktree::BranchName::parse(branch) else {
            return false;
        };
        // `resolve_in` ignores the clone (the slug already scopes the path);
        // the empty path is a placeholder, not a lookup.
        let Ok(target) =
            crate::worktree::resolve_in(root, std::path::Path::new(""), &slug, &branch)
        else {
            return false;
        };
        std::fs::symlink_metadata(target.join(".git")).is_ok()
    }

    /// The repo's open trees for one issue/PR: branch, path, who ran an
    /// agent there last and whether one is live inside right now. For the
    /// detail box, so the work behind an issue is visible where it is read.
    pub fn item_trees(&self, repo: &str, branch: &str) -> Vec<ItemTree> {
        let (Some(root), Ok(slug), Ok(want)) = (
            self.worktree_root.as_deref(),
            crate::actions::RepoSlug::parse(repo),
            crate::worktree::BranchName::parse(branch),
        ) else {
            return Vec::new();
        };
        let mut out: Vec<ItemTree> = crate::worktree::list_worktrees(root, &slug)
            .into_iter()
            .filter_map(|path| {
                let found = crate::worktree::worktree_branch(&path)?;
                (found == want).then_some((found, path))
            })
            .map(|(found, path)| {
                let agent = self
                    .last_agents
                    .get(&path.display().to_string())
                    .map(|last| last.agent.clone());
                let live = self.tab_agents.values().any(|tab| {
                    tab.running
                        && tab
                            .record
                            .as_ref()
                            .is_some_and(|record| record.worktree == path)
                });
                ItemTree {
                    branch: found.as_str().to_string(),
                    path,
                    agent,
                    live,
                }
            })
            .collect();
        out.sort_by(|a, b| a.branch.cmp(&b.branch).then(a.path.cmp(&b.path)));
        out
    }

    /// The record card shows for the active tab when it is that tab's record
    /// and the card was not dismissed for it. A record glued to a tab that
    /// is not on screen would be a lie, so anything else shows detail —
    /// and so would a card for a row the list no longer shows.
    pub fn visible_record(&self) -> Option<&AgentRecord> {
        let key = self.active_key()?;
        let agent = self.tab_agents.get(&key)?;
        let record = agent.record.as_ref()?;
        if (record.repo.clone(), record.number) != key {
            return None;
        }
        if agent.dismissed_for.as_ref() == Some(&key) {
            return None;
        }
        let item = self.effective_item()?;
        if item.repo == record.repo && item.number == record.number {
            return Some(record);
        }
        None
    }

    /// A finished run's card yields to the detail as soon as the user moves:
    /// after the close, arrows/tabs/clicks show the item again, not the
    /// agent's last frame. A running session keeps its card — it is still
    /// the news. `x` deletes the record entirely, this only hides the card
    /// (reselecting after a new run shows the new one).
    fn dismiss_finished_card(&mut self) {
        let Some(key) = self.active_key() else {
            return;
        };
        let Some(agent) = self.tab_agents.get_mut(&key) else {
            return;
        };
        let finished = matches!(
            agent.record.as_ref().map(|r| &r.status),
            Some(AgentStatus::Finished { .. })
        );
        if finished {
            if let Some(record) = agent.record.as_ref() {
                let key = (record.repo.clone(), record.number);
                agent.dismissed_for = Some(key);
            }
        }
    }
    /// `x` deletes the active tab's record. After the exit there is nothing to
    /// stop, so closing only forgets — it never claims to kill anything.
    pub fn close_agent_record(&mut self) {
        let Some(key) = self.active_key() else {
            self.status = "no agent record to close".to_string();
            return;
        };
        let take = self
            .tab_agents
            .get_mut(&key)
            .and_then(|agent| agent.record.take());
        if let Some(record) = take {
            if let Some(agent) = self.tab_agents.get_mut(&key) {
                agent.dismissed_for = None;
            }
            self.status = format!(
                "closed the agent record for {}#{}",
                record.repo, record.number
            );
        } else {
            self.status = "no agent record to close".to_string();
        }
    }

    /// `x`/`X`: close the active tab first, then the agent record.
    /// Tabs are the newest layer, so they close first — once no tab is
    /// left, `x` forgets the record exactly as before.
    pub fn close_tab_or_record(&mut self) {
        if self.close_active_tab() {
            if let Some(idx) = self.active_tab {
                let label = self.tab_labels().get(idx).cloned().unwrap_or_default();
                self.status = format!("closed tab — now on {label}");
            } else {
                self.status = "closed tab — no tabs left".to_string();
            }
            return;
        }
        self.close_agent_record();
    }

    /// The kind of `(repo, number)` on the current list, for body fetches:
    /// issues need `gh issue view`, PRs need `gh pr view`. The detail
    /// request only carries repo+number, so the kind is read back here —
    /// a hardcoded `Pr` asks GitHub for a pull request with an issue's
    /// number and the body never loads. Unknown items keep the old `Pr`
    /// guess: the fetch then runs and fails honestly.
    pub fn item_kind(&self, repo: &str, number: u64) -> ItemKind {
        self.items
            .iter()
            .chain(self.retained.iter())
            .find(|item| item.repo == repo && item.number == number)
            .map(|item| item.kind)
            .unwrap_or(ItemKind::Pr)
    }

    /// Bottom-right pane: what to do next, always visible.
    /// The keys come first, so a short box still shows them; the record
    /// card lives here too, below the task it belongs to.
    pub fn agent_panel_text(&self) -> String {
        let mut out = Vec::new();
        match self.effective_item() {
            Some(item) => {
                out.push(format!(
                    "task · {}#{} {}",
                    item.repo, item.number, item.title
                ));
                match item.kind {
                    crate::model::ItemKind::Pr => {
                        let branch = item.head_ref_name.as_deref().unwrap_or("?");
                        out.push(format!("branch · {branch}"));
                    }
                    crate::model::ItemKind::Issue => {
                        out.push(format!(
                            "issue · new branch issue-{} — g works, m closes",
                            item.number
                        ));
                    }
                }
            }
            None => out.push("task · nothing selected".to_string()),
        }
        if let Some(record) = self.visible_record() {
            out.push(String::new());
            out.push(record.card());
        } else if let Some(record) = self.active_agent().and_then(|a| a.record.as_ref()) {
            out.push(String::new());
            out.push(format!(
                "last agent · {} · {}#{} · {}",
                record.agent,
                record.repo,
                record.number,
                record.status.summary()
            ));
        } else {
            out.push("agent · no run yet".to_string());
        }
        out.push(String::new());
        out.push(format!("profile · {}", self.agent_profile));
        let roots = if self.clone_roots.is_empty() {
            "no clone_root — g will refuse".to_string()
        } else {
            self.clone_roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push(format!("clones · {roots}"));
        out.push(String::new());
        out.push("g chooses the agent + worktree, then launches".to_string());
        out.join("\n")
    }

    /// Build the first action for the selected row, show what it would run, and
    /// keep the pair (confirmation, argv) so `y` executes exactly that.
    /// Follows the active tab when one is open.
    fn request_action(&mut self) {
        let Some(item) = self.effective_item().cloned() else {
            self.status = "nothing selected".into();
            return;
        };
        // An issue has no verb here: saying nothing would be indistinguishable
        // from a broken key, so say why (review, X-2 point 2).
        if item.kind != crate::model::ItemKind::Pr {
            self.status = "no action for this item — issues are not merged".into();
            return;
        }
        let number = item.number;
        // The item knows its repo; the cursor in the repos pane does not.
        let repo = item.repo.clone();
        let Ok(slug) = RepoSlug::parse(&repo) else {
            self.status = format!("cannot act on {repo}");
            return;
        };
        let action = action_for_pick(
            slug,
            number,
            &ActionPick::Merge(FIRST_ACTION_METHOD, FIRST_ACTION_DELETES_BRANCH),
        );
        self.confirm_action(action);
    }

    /// Build the confirmation for an action: the sentence plus the exact
    /// argv, shown before anything runs. `a` and the `m` menu meet here —
    /// one dialog, one argv, whichever key asked.
    fn confirm_action(&mut self, action: Action) {
        let Some(binaries) = self.binaries.clone() else {
            self.status = "no binaries resolved".into();
            return;
        };
        match argv_for(&binaries, &action) {
            Ok(argv) => {
                let Confirmation {
                    summary,
                    argv: shown,
                } = confirmation_for(&action, &argv);
                self.status = shown.join(" ");
                self.modal = Some(Modal::Confirm(summary));
                self.pending = Some(Pending::Action(PendingAction { action, argv }));
            }
            Err(err) => self.status = format!("cannot build the action: {err}"),
        }
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => {}
            Msg::Key(key) => {
                self.on_key(key);
                if key != Key::CtrlC {
                    self.session_dirty = true;
                }
            }
        }
    }

    fn on_key(&mut self, key: Key) {
        if key == Key::CtrlC {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            // Route by dialog first: a key means different things per
            // dialog, so the match is on the modal, then on the key.
            if matches!(self.modal, Some(Modal::RepoMenu(_))) {
                match key {
                    Key::Esc => self.modal = None,
                    Key::Up => self.menu_step(-1),
                    Key::Down => self.menu_step(1),
                    Key::Char(' ') => self.menu_toggle(),
                    Key::Char('a') => self.menu_mark_all(),
                    Key::Char('n') => self.menu_mark_none(),
                    Key::Enter => {
                        if let Some(Modal::RepoMenu(menu)) = self.modal.take() {
                            let selected: Vec<RepoSlug> = menu
                                .marked
                                .iter()
                                .filter_map(|i| menu.rows.get(*i).cloned())
                                .collect();
                            self.apply_repo_selection(selected);
                        }
                    }
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::ActionMenu(_))) {
                match key {
                    Key::Esc => self.modal = None,
                    Key::Up => self.menu_step_action(-1),
                    Key::Down | Key::Char(' ') => self.menu_step_action(1),
                    Key::Enter => self.apply_action_pick(),
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::AgentMenu(_))) {
                match key {
                    Key::Esc => self.modal = None,
                    Key::Up => self.menu_step_agent(-1),
                    Key::Down => self.menu_step_agent(1),
                    Key::Enter => {
                        if let Some(Modal::AgentMenu(menu)) = self.modal.take() {
                            if let Some(row) = menu.rows.get(menu.cursor).cloned() {
                                match menu.place {
                                    // The tree was already chosen in the
                                    // worktree menu: launch straight there.
                                    Some(place) => {
                                        let mut plan = menu.plan;
                                        plan.agent = row;
                                        self.agent_place = Some((plan, place));
                                    }
                                    // The `g` flow: the tree choice comes next.
                                    None => self.pick_agent(row, menu.plan),
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::Worktrees(_))) {
                match key {
                    Key::Esc => self.modal = None,
                    Key::Up => self.menu_step_worktree(-1),
                    Key::Down => self.menu_step_worktree(1),
                    Key::Enter => self.use_worktree_row(),
                    Key::Char('n') => self.name_new_worktree(),
                    Key::Char('r') => self.rename_worktree_row(),
                    Key::Char('d') => self.delete_worktree_row(),
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::NameInput(_))) {
                match key {
                    Key::Esc => self.modal = None,
                    Key::Enter => self.confirm_name_input(),
                    Key::Backspace => {
                        if let Some(Modal::NameInput(mut input)) = self.modal.take() {
                            input.value.pop();
                            self.modal = Some(Modal::NameInput(input));
                        }
                    }
                    Key::Char(c) => {
                        if let Some(Modal::NameInput(mut input)) = self.modal.take() {
                            input.value.push(c);
                            self.modal = Some(Modal::NameInput(input));
                        }
                    }
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::Notice(_))) {
                // Information, not a question: `Esc` puts it away, `q` quits
                // through it, everything else waits its turn. Quitting must
                // always work, even on top of bad news.
                match key {
                    Key::Esc => self.modal = None,
                    Key::Char('q') => self.quit = true,
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::Diff)) {
                // Read-only overlay: `Esc`/`Enter` put it away, `↑`/`↓`
                // scroll, `q` quits through it like the notice.
                match key {
                    Key::Esc | Key::Enter => self.modal = None,
                    Key::Up => self.diff_scroll = self.diff_scroll.saturating_sub(1),
                    Key::Down => self.diff_scroll = self.diff_scroll.saturating_add(1),
                    Key::Char('q') => self.quit = true,
                    _ => {}
                }
                return;
            }
            if matches!(self.modal, Some(Modal::Settings(_))) {
                // One row per setting: `↑`/`↓` move, `Enter`/`Space` flip
                // (the menu stays open on the new state), `Esc` closes,
                // `q` quits through like every overlay.
                match key {
                    Key::Esc => self.modal = None,
                    Key::Up => {
                        if let Some(Modal::Settings(menu)) = &mut self.modal {
                            menu.cursor = menu.cursor.saturating_sub(1);
                        }
                    }
                    Key::Down => {
                        if let Some(Modal::Settings(menu)) = &mut self.modal {
                            menu.cursor = menu
                                .cursor
                                .saturating_add(1)
                                .min(menu.rows.len().saturating_sub(1));
                        }
                    }
                    Key::Enter | Key::Char(' ') => self.toggle_setting(),
                    Key::Char('q') => self.quit = true,
                    _ => {}
                }
                return;
            }
            match key {
                Key::Esc => self.modal = None,
                Key::Char('n') => {
                    self.modal = None;
                    // Declining the resume offer goes back to choosing —
                    // with the preset agent (the `g` flow) it launches
                    // there, without one (the `W` flow) it opens the agent
                    // menu for that tree. Any other question's `n` just
                    // closes, as before.
                    if let Some(pending) = self.pending.take() {
                        match pending {
                            Pending::Resume {
                                mut plan,
                                path,
                                preset,
                            } => match preset {
                                Some(agent) => {
                                    plan.agent = agent;
                                    self.agent_place = Some((plan, path));
                                }
                                None => {
                                    let rows = self.agent_choices();
                                    if rows.is_empty() {
                                        self.status = "no agent binaries found (hermes, pi, omp, opencode, agy)".to_string();
                                    } else {
                                        plan.profile = self.agent_profile.clone();
                                        self.modal = Some(Modal::AgentMenu(AgentMenu {
                                            rows,
                                            cursor: 0,
                                            plan,
                                            place: Some(path),
                                        }));
                                    }
                                }
                            },
                            other => self.pending = Some(other),
                        }
                    }
                }
                Key::Char('y') => {
                    self.modal = None;
                    match self.pending.take() {
                        // X-1: the message comes from the type, never invented.
                        // `Unknown` says "it may have been applied", which is the
                        // only honest sentence when a write times out.
                        Some(Pending::Action(pending)) => {
                            let outcome = execute(&pending.argv, self.write_timeout);
                            // The transport touched the server when the
                            // process ran (whatever it answered) or may have
                            // run (timeout): re-read instead of trusting the
                            // return code. A refusal or a spawn failure never
                            // reached the server, so there is nothing to read.
                            if matches!(&outcome, Executed::Finished(_) | Executed::Unknown(_)) {
                                self.refresh_requested = true;
                            }
                            let merged = matches!(
                                &outcome,
                                Executed::Finished(status) if status.success()
                            ) && matches!(pending.action, Action::Merge { .. });
                            self.status = describe(outcome);
                            // A merge that landed leaves its local trees and
                            // head branch behind: while it is fresh, offer to
                            // delete what is no longer useful. Anything else
                            // keeps its status line, no question asked.
                            if merged {
                                self.offer_merge_cleanup(&pending.action);
                            }
                        }
                        // A worktree write needs the loop (git, seed file,
                        // terminal): `y` only moves the agreed plan there.
                        Some(Pending::Agent(plan, path)) => {
                            self.agent_go = Some((plan, path));
                        }
                        // A merged PR leaves its local trees and head branch
                        // behind: `y` hands the cleanup to the loop.
                        Some(Pending::Cleanup(order)) => {
                            self.cleanup_go = Some(order);
                        }
                        // Reopen the recorded session: the loop resumes it
                        // with the agent's continue flag, sending nothing.
                        Some(Pending::Resume { plan, path, .. }) => {
                            self.agent_resume = Some((plan, path));
                        }
                        // A delete is a write too (to the clone, via the
                        // loop): `y` moves the agreed order there.
                        Some(Pending::Delete(order)) => {
                            self.delete_go = Some(order);
                        }
                        // A clone is a write too (network, minutes): `y`
                        // moves the agreed repo+directory there; the loop
                        // clones in a thread and continues the flow that
                        // asked (`then`) when it lands.
                        Some(Pending::Clone { repo, path, then }) => {
                            self.clone_go = Some((repo, path, then));
                        }
                        // A fresh branch for an empty repo: no write yet, so
                        // `y` opens the flow directly — the creation question
                        // still asks before anything touches the disk.
                        Some(Pending::NewBranch { repo }) => {
                            self.open_repo_worktree_for(&repo);
                        }
                        // Nothing was asked: never claim success for it.
                        None => self.status = "nothing to run".into(),
                    }
                }
                _ => {}
            }
            return;
        }

        // While searching, printable keys belong to the query — including `q`.
        if self.searching {
            match key {
                Key::Esc => {
                    self.searching = false;
                    self.query.clear();
                }
                Key::Enter => self.searching = false,
                Key::Backspace => {
                    self.query.pop();
                }
                Key::Char(c) => {
                    self.query.push(c);
                }
                Key::Up
                | Key::Down
                | Key::Tab
                | Key::BackTab
                | Key::Left
                | Key::Right
                | Key::ShiftUp
                | Key::ShiftDown
                | Key::ShiftLeft
                | Key::ShiftRight
                | Key::CtrlC => {}
            }
            return;
        }

        // The help overlay eats every key except the quit chord: any other key
        // just closes it and is otherwise ignored.
        if self.show_help && key != Key::CtrlC {
            self.show_help = false;
            return;
        }

        match key {
            // `a` asks to run the first action on the selected row.
            Key::Char('a') => self.request_action(),
            // `m` opens the same actions as a menu, so the method and the
            // branch deletion are a choice instead of a constant.
            Key::Char('m') => self.open_action_menu(),
            // `g` hands the selected PR to an agent (the loop owns the
            // terminal, so this only validates and asks). From the repos
            // pane it lists the repo's sessions instead: every agent and
            // past run on its worktrees, open or closed.
            Key::Char('g') => {
                if self.pane == Pane::Repos {
                    match self.repos.get(self.repos_cursor).cloned() {
                        Some(RepoRow::Repo(slug)) => self.open_repo_sessions_for(slug.as_str()),
                        _ => self.status = "pick one repo first — All is not a session".to_string(),
                    }
                } else {
                    self.request_agent()
                }
            }
            // `W` manages worktrees without launching anything: list, use,
            // rename, delete — the agent menu only appears when a tree is
            // picked for work.
            Key::Char('W') => {
                // The repos pane grows trees: same key, the repo under the
                // cursor instead of the selected issue/PR.
                if self.pane == Pane::Repos {
                    self.open_repo_worktree();
                } else {
                    self.open_worktree_menu();
                }
            }
            // `x`/`X` close the active tab first, then the agent record.
            // Tabs are the newest layer; without tabs this is the old `x`.
            Key::Char('x') | Key::Char('X') => self.close_tab_or_record(),
            // `t`/`T` cycle tabs, like a browser. Without tabs it says so
            // instead of staying silent.
            Key::Char('t') => {
                if self.tabs.is_empty() {
                    self.status = "no tabs — Enter opens one".to_string();
                } else {
                    self.next_tab();
                }
            }
            Key::Char('T') => {
                if self.tabs.is_empty() {
                    self.status = "no tabs — Enter opens one".to_string();
                } else {
                    self.prev_tab();
                }
            }
            // `w` focuses the agent pane, where the worktree instructions
            // live. The full worktree menu is still planned (`s` with it);
            // this key only moves focus and names the next step.
            Key::Char('w') => {
                self.pane = Pane::Agent;
                if let Some(item) = self.effective_item().cloned() {
                    self.status = format!(
                        "agent pane for {}#{} — g chooses the agent and worktree",
                        item.repo, item.number
                    );
                } else {
                    self.status = "agent pane — select an item first".to_string();
                }
            }
            Key::Char('/') => self.searching = true,
            // `R` picks which repos the list shows (multi-select menu).
            Key::Char('R') => self.open_repo_menu(),
            // `o` opens the selected item in the browser, if the URL is safe.
            Key::Char('o') => self.request_open(),
            // One refresh at a time: a second `r` while loading is ignored.
            Key::Char('r') if !self.loading => {
                self.loading = true;
                self.refresh_requested = true;
                self.status = "refreshing…".to_string();
            }
            // `f` cycles the state filter — open, closed, all — but only
            // on a single repo: closed history for all repos would
            // multiply the fetch by every repo. The refetch carries the
            // closed states; the list shows them oldest-first.
            Key::Char('f') if !self.loading => {
                let RepoFilter::One(_) = self.repo_filter else {
                    self.status =
                        "pick a single repo first — closed history is per-repo".to_string();
                    return;
                };
                self.state_filter = self.state_filter.next();
                self.status = format!("showing {}", self.state_filter.label());
                self.loading = true;
                self.refresh_requested = true;
            }
            // `d` shows the selected PR's diff in an overlay (fetched once,
            // then cached). With a running agent focused this key types
            // into the agent instead — it never reaches here.
            Key::Char('d') => self.request_diff(),
            // `s` opens the settings menu (app-wide toggles like sandboxed
            // agents). With a running agent focused this key types into
            // the agent instead — it never reaches here.
            Key::Char('s') => self.open_settings(),
            // Esc climbs one level and stops at the first: it never quits.
            // The match is exhaustive on purpose, so a new pane is a compile
            // error here instead of silently inheriting an exit.
            // A visible record card closes first, from any pane: the card
            // lives in the right column, which both panes share.
            Key::Esc if self.visible_record().is_some() => {
                // First Esc closes the record card and shows the detail; the
                // record itself stays (a new run replaces it, `x` deletes it).
                // Reselecting the item shows the card again.
                if let Some(item) = self.effective_item().cloned() {
                    if let Some(agent) = self.active_agent_mut() {
                        agent.dismissed_for = Some((item.repo, item.number));
                    }
                }
            }
            Key::Esc => match self.pane {
                Pane::Agent => self.pane = Pane::Detail,
                Pane::Detail => self.pane = Pane::List,
                // Drilled in (a repo filter is active): back to the repos
                // pane. The cursor never moved while drilled in, so the row
                // it left is still selected by construction — nothing needs
                // remembering, neither an index nor a slug.
                Pane::List if matches!(self.repo_filter, RepoFilter::One(_)) => {
                    self.pane = Pane::Repos;
                }
                // Second Esc, in the repos pane with a filter: clear it and
                // go home to row 0. The filter follows the cursor, so moving
                // the cursor is enough — `apply_repo_filter` resets the list
                // cursor, the stale detail and the status line too.
                Pane::Repos if matches!(self.repo_filter, RepoFilter::One(_)) => {
                    self.repos_cursor = 0;
                    self.apply_repo_filter();
                }
                Pane::List | Pane::Repos => {
                    // Nothing to close: say it as a status. In the legend it was
                    // 24 characters that pushed every real message off the edge.
                    self.status = "Esc — nothing to close".to_string();
                }
            },
            Key::Char('q') => self.quit = true,
            Key::Char('?') => self.show_help = true,
            Key::Left => {
                // Left column is repos + list; from anywhere on the right,
                // go back to the list.
                if matches!(self.pane, Pane::Detail | Pane::Agent) {
                    self.pane = Pane::List;
                }
            }
            // Shift+arrows move between boxes following the screen layout:
            // left column is repos over list, right column detail over agent.
            // At an edge nothing moves — a wrap would land somewhere the
            // arrow did not point. Right from the list goes to the detail
            // (the PR box on the right), never into the agent box: the agent
            // is reached from the detail, or with Tab/`w` when it is live.
            Key::ShiftRight => match self.pane {
                Pane::Repos => self.pane = Pane::Detail,
                Pane::List => self.pane = Pane::Detail,
                Pane::Detail | Pane::Agent => {}
            },
            Key::ShiftLeft => match self.pane {
                Pane::Detail => self.pane = Pane::List,
                Pane::Agent => self.pane = Pane::List,
                Pane::Repos | Pane::List => {}
            },
            Key::ShiftUp => match self.pane {
                Pane::List => self.pane = Pane::Repos,
                Pane::Agent => self.pane = Pane::Detail,
                Pane::Repos | Pane::Detail => {}
            },
            Key::ShiftDown => match self.pane {
                Pane::Repos => self.pane = Pane::List,
                Pane::Detail if self.agent_box_shown() => self.pane = Pane::Agent,
                Pane::Detail | Pane::List | Pane::Agent => {}
            },
            Key::Right => {
                // Right column top is the detail; from anywhere on the left,
                // go to the detail.
                if matches!(self.pane, Pane::Repos | Pane::List) {
                    self.pane = Pane::Detail;
                }
            }
            Key::Tab => self.pane = next_pane(self.pane, true, self.agent_box_shown()),
            Key::BackTab => self.pane = next_pane(self.pane, false, self.agent_box_shown()),
            Key::Up => self.move_cursor(-1),
            Key::Down => self.move_cursor(1),
            Key::Backspace => {}
            Key::Char(_) => {}
            // In the repos pane Enter drills in: the filter is already live
            // (it follows the cursor), so reaching the list shows the repo's
            // items. A repo with no issues or PRs shows an empty list — that
            // is a starting point, not a dead end: offer a fresh branch.
            Key::Enter if self.pane == Pane::Repos => {
                self.pane = Pane::List;
                if self.query.trim().is_empty()
                    && let RepoFilter::One(slug) = &self.repo_filter
                    && self.visible_items().is_empty()
                {
                    let repo = slug.as_str().to_string();
                    self.modal = Some(Modal::Confirm(format!(
                        "{repo} has no open issues or PRs — start a new branch?"
                    )));
                    self.pending = Some(Pending::NewBranch { repo });
                }
            }
            // In the list Enter opens a tab for the row; the detail follows
            // the active tab from then on.
            Key::Enter if self.pane == Pane::List => {
                self.open_tab_for_selected();
            }
            Key::Enter => self.request_detail(),
            // Handled at the top; repeated here because the match must be total.
            Key::CtrlC => {}
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        match self.pane {
            Pane::Repos => {
                self.repos_cursor = step(self.repos_cursor, delta, self.repos_len);
                self.apply_repo_filter();
            }
            Pane::List => {
                self.list_cursor = step(self.list_cursor, delta, self.list_len);
                self.dismiss_finished_card();
                // Landing on a row starts its body: by the time the user
                // reads the fields, the description is already on its way.
                self.preload_detail();
            }
            Pane::Detail => {
                let next = self.detail_scroll as i32 + delta;
                self.detail_scroll = next.max(0) as u16;
            }
            Pane::Agent => {
                if let Some(agent) = self.active_agent_mut() {
                    let next = agent.scroll as i32 + delta;
                    agent.scroll = next.max(0) as u16;
                }
            }
        }
    }
}

fn next_pane(pane: Pane, forward: bool, with_agent: bool) -> Pane {
    // Order: list → repos → detail, plus the agent pane only while there is
    // something in it (an agent being arranged or running). Cycling into an
    // empty instructions box is a surprise, not a shortcut — `w` reaches the
    // instructions on purpose when idle. The repos pane comes first on
    // purpose: it is how you narrow the list, so it must be one keypress
    // away from it.
    match (pane, forward, with_agent) {
        (Pane::List, true, _) => Pane::Repos,
        (Pane::Repos, true, _) => Pane::Detail,
        (Pane::Detail, true, true) => Pane::Agent,
        (Pane::Detail, true, false) => Pane::List,
        (Pane::Agent, true, _) => Pane::List,
        (Pane::List, false, true) => Pane::Agent,
        (Pane::List, false, false) => Pane::Detail,
        (Pane::Agent, false, _) => Pane::Detail,
        (Pane::Detail, false, _) => Pane::Repos,
        (Pane::Repos, false, _) => Pane::List,
    }
}

fn step(cursor: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = cursor as i32 + delta;
    next.clamp(0, len as i32 - 1) as usize
}

/// One sentence per outcome, taken from the type: the UI never guesses what
/// happened to a write.
fn describe(outcome: Executed) -> String {
    // The sentence reports the REQUEST, never the server's state: `done`
    // says the process finished, and the refresh the caller triggers says
    // when the screen shows what the server confirms. A non-zero exit is a
    // failure with the code visible — never "done".
    match outcome {
        Executed::Finished(status) if status.success() => {
            format!("done ({status}) — re-reading…")
        }
        Executed::Finished(status) => format!("failed ({status}) — re-reading…"),
        Executed::Refused(why) => format!("refused: {why}"),
        Executed::SpawnFailed(why) => format!("cannot run: {why}"),
        Executed::Unknown(why) => {
            format!("unknown — it may have been applied: {why} — re-reading…")
        }
    }
}

#[cfg(test)]
mod x1_tests {
    use super::*;

    /// X-1: a key that runs nothing must not report success.
    #[test]
    fn the_confirm_key_does_not_claim_an_action_it_did_not_run() {
        let mut app = App {
            modal: Some(Modal::Confirm("merge PR #7?".into())),
            ..App::default()
        };
        app.update(Msg::Key(Key::Char('y')));
        assert!(
            !app.status.contains("confirmed"),
            "y must not claim success: {:?}",
            app.status
        );
        assert!(
            app.status.contains("nothing to run"),
            "it says what happened: {:?}",
            app.status
        );
    }
}
