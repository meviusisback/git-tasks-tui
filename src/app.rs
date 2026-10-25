//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

use crate::actions::{Action, MergeMethod, RepoSlug, argv_for};
use crate::actions_ui::{Confirmation, Executed, confirmation_for, execute};
use crate::agent::{
    AgentChoice, AgentId, AgentPlan, AgentRecord, AgentStatus, PlanInputs, plan_agent,
};
use crate::binaries::Binaries;
use crate::fetch::FetchOutcome;
use crate::gh::CheckedArgv;
use crate::model::Item;
use crate::sanitize::sanitize;

use std::collections::BTreeSet;

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
/// it is the active one (for emphasis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub label: String,
    pub kind: Option<crate::model::ItemKind>,
    pub active: bool,
}

/// The repo filter as a type, not a string: either everything or one valid
/// slug. There is no representation for "one invalid slug", so no check has
/// to remember to refuse it — the same lesson as the worktree paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoFilter {
    All,
    One(RepoSlug),
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
    /// `d` on a worktree row, confirmed with `y`: the loop deletes it.
    Delete(crate::worktree::WorktreeDelete),
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
}

/// The `g` menu: one row per installed agent binary, cursor, and the
/// validated plan the pick applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMenu {
    pub rows: Vec<crate::agent::AgentChoice>,
    pub cursor: usize,
    pub plan: AgentPlan,
}

/// The worktree picker for one repo: existing trees plus the agent and plan
/// the choice applies to. The current item's branch (if any tree matches it)
/// is marked at draw time by comparing branches — no stored flag to go stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeMenu {
    pub agent: crate::agent::AgentChoice,
    pub plan: AgentPlan,
    pub rows: Vec<crate::worktree::WorktreeRow>,
    pub cursor: usize,
}

/// What the name being typed is for: a fresh worktree (agent + plan wait
/// for it) or a rename (the old tree waits for its new name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamePurpose {
    Create {
        agent: crate::agent::AgentChoice,
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
    /// The repos the user configured, plus a leading "all repos" entry, so a
    /// repo with no open items is still visible and selectable.
    pub repos: Vec<RepoRow>,
    /// Every configured repo, even when the list shows a subset: the rows
    /// the `R` menu offers. Set once at startup, never filtered.
    pub all_repos: Vec<RepoSlug>,
    /// The repo selected in the repos pane; `All` means "all".
    pub repo_filter: RepoFilter,
    /// `Enter` in the repo menu: the checked rows, for the loop to save to
    /// `state.json`. The live list is already updated; this is only the file.
    pub selection_applied: Option<Vec<String>>,
    /// A fetch is in flight: `r` must not start a second one.
    pub loading: bool,
    /// Repos whose last fetch failed.
    pub failed: usize,
    /// Set by the key handler, consumed by the event loop that owns the thread.
    pub refresh_requested: bool,
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
    /// Set by `o`: the loop launches the browser.
    pub open_requested: Option<String>,
    /// The `?` overlay with the full keymap is on screen.
    pub show_help: bool,
    /// Open tabs (`Enter` on a row opens one) and the active tab.
    /// The detail and the agent act on the active tab when set, otherwise
    /// on the row under the list cursor.
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    /// Scroll offset of the agent pane.
    pub agent_scroll: u16,
    /// True when session fields changed since the last save: the loop
    /// persists `state.json` so a temporary exit keeps tabs, filter and
    /// cursor positions.
    pub session_dirty: bool,
    /// Profile passed to `hermes -p` by the agent handoff; validated at load.
    pub agent_profile: String,
    /// Where to look for local clones, in order (`<root>/<repo-name>`); empty refuses.
    /// A list, not a path: clones really do live in more than one directory, and one path
    /// cannot say so without making the other directory invisible.
    pub clone_roots: Vec<std::path::PathBuf>,
    /// True when the config on disk could not be used and the app is running on the demo fallback.
    /// The notice says it once; this is how the status line can keep saying it, because a decision
    /// taken once must not become a thing forgotten: after `Esc` the list holds a repository the
    /// user never chose, and nothing else on screen remembers why.
    pub config_fallback: bool,
    /// The last agent run, if any. It stays after the exit: `Esc` hides the
    /// card, `x` deletes the record, a new run replaces it.
    pub agent_record: Option<AgentRecord>,
    /// The item the record card was dismissed for: the card shows again when
    /// that item is reselected, and stays hidden while it is selected.
    pub agent_dismissed_for: Option<(String, u64)>,
    /// Set by `g`, consumed by the loop that owns the terminal: validate
    /// here, hand over there, like every other outbox.
    pub agent_requested: Option<AgentPlan>,
    /// Set by `y` on the create question, consumed by the loop: the plan
    /// plus the worktree the user agreed to create.
    pub agent_go: Option<(AgentPlan, std::path::PathBuf)>,
    /// Set by `Enter` on the rename prompt, consumed by the loop: rename
    /// the branch, then move the tree. No confirm — nothing is lost.
    pub rename_go: Option<crate::worktree::WorktreeRename>,
    /// Set by `y` on the delete question, consumed by the loop.
    pub delete_go: Option<crate::worktree::WorktreeDelete>,
    /// Where worktrees live (configured root, or the default). The list
    /// icons and the worktree picker read it; `None` (unresolvable home)
    /// means no icons, and the picker explains instead of guessing.
    pub worktree_root: Option<std::path::PathBuf>,
    /// True while an embedded agent session is alive in the box. Set by the
    /// loop on spawn, cleared on exit: the loop owns the child, `App` only
    /// carries what the screen needs (flag + last parsed screen).
    pub agent_running: bool,
    /// Last parsed screen of the embedded session, row by row. Empty while
    /// idle: the box is hidden then, not blank.
    pub agent_screen: crate::embedded::AgentScreen,
    /// What `y` runs: an action or an agent launch, never both.
    pub pending: Option<Pending>,
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
            repos: Vec::new(),
            all_repos: Vec::new(),
            repo_filter: RepoFilter::All,
            selection_applied: None,
            loading: false,
            failed: 0,
            refresh_requested: false,
            binaries: None,
            write_timeout: std::time::Duration::from_secs(30),
            pending: None,
            agent_profile: String::from("default"),
            clone_roots: Vec::new(),
            config_fallback: false,
            agent_record: None,
            agent_dismissed_for: None,
            agent_requested: None,
            agent_go: None,
            rename_go: None,
            delete_go: None,
            worktree_root: None,
            agent_running: false,
            agent_screen: Vec::new(),
            detail: String::new(),
            detail_for: None,
            detail_requested: None,
            open_requested: None,
            show_help: false,
            tabs: Vec::new(),
            active_tab: None,
            agent_scroll: 0,
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
        self.items = items;
        self.failed = failures.len();
        self.loading = false;

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
                format!("{} in {slug}", plural(shown, "item", "items"))
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
        if self.query.trim().is_empty() {
            return by_repo;
        }
        let needle = self.query.to_lowercase();
        by_repo
            .into_iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&needle)
                    || item.number.to_string().contains(&needle)
            })
            .collect()
    }

    /// The bottom legend: the keys available right now, never the full keymap
    /// (that lives behind `?`). Untrusted text (query, status) is sanitised.
    pub fn legend(&self) -> String {
        match &self.modal {
            Some(Modal::Confirm(question)) => {
                return format!("{}  —  y confirm · n/Esc cancel", sanitize(question));
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
                "q quit · ? help · ←/→ panes · t/T tabs · x close · m actions · g agent · Enter body",
            )
        } else if self.pane == Pane::Agent {
            // While the agent runs, every key but Esc is agent input —
            // including Ctrl-C, which interrupts it. The legend must not
            // promise keys that type instead.
            if self.agent_running {
                String::from("Esc/Tab panes · Ctrl-C interrupts · wheel scrolls · type to agent")
            } else {
                String::from(
                    "q quit · ? help · ←/→ panes · ↑/↓ scroll · g launch agent · x close record",
                )
            }
        } else if self.pane == Pane::Repos {
            String::from("q quit · ? help · ↑/↓ move · Enter drill in · R repos · t/T · x close")
        } else if self.repo_filter == RepoFilter::All {
            String::from(
                "q quit · ? help · ←/→ panes · ↑/↓ move · Tab repos · Enter open tab · t/T tabs · x close · R repos · m actions · g agent",
            )
        } else {
            String::from(
                "q quit · ? help · Esc back · ←/→ panes · ↑/↓ move · Tab repos · Enter open tab · t/T tabs · x close · m actions · g agent",
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
    /// list (everything, when the state file never picked a subset).
    pub fn open_repo_menu(&mut self) {
        if self.all_repos.is_empty() {
            self.status = "no repos configured".to_string();
            return;
        }
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

    /// `Enter` in the repo menu: the checked rows become the selection. An
    /// empty check means "everything", like an empty state file. The list
    /// updates from the items already in memory; the loop saves the file.
    fn apply_selection(&mut self, selected: Vec<RepoSlug>) {
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
        self.set_repos(shown);
    }

    /// The item under the cursor, if any.
    pub fn selected_item(&self) -> Option<&Item> {
        self.visible_items().get(self.list_cursor).copied()
    }

    /// The item the right column acts on: the active tab when set and still
    /// present, otherwise the row under the list cursor. Tabs survive a
    /// filter change, so the detail never follows a row that disappeared.
    pub fn effective_item(&self) -> Option<&Item> {
        if let Some(idx) = self.active_tab
            && let Some(tab) = self.tabs.get(idx)
            && let Some(found) = self
                .items
                .iter()
                .find(|i| i.repo == tab.repo && i.number == tab.number)
        {
            return Some(found);
        }
        self.selected_item()
    }

    /// Open a tab for the row under the cursor, or activate it when open.
    /// `Enter` in the list opens; the detail and the left column follow it.
    /// `Enter` on the already-active tab changes nothing: the cached body
    /// is reused instead of being cleared and fetched again.
    pub fn open_tab_for_selected(&mut self) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
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
        self.session_dirty = true;
        self.sync_left_to_tab(&item.repo, item.number);
        // Load the body for the newly active tab when missing.
        let key = (item.repo.clone(), item.number);
        if self.detail_for.as_ref() != Some(&key) && self.detail_requested.as_ref() != Some(&key) {
            self.detail_requested = Some(key);
            self.status = format!("loading #{}…", item.number);
        }
    }

    /// Activate tab `idx` (clamped): the detail follows it and loads its
    /// body when missing.
    pub fn activate_tab(&mut self, idx: usize) {
        if self.tabs.is_empty() {
            self.active_tab = None;
            return;
        }
        let idx = idx.min(self.tabs.len() - 1);
        self.active_tab = Some(idx);
        self.session_dirty = true;
        if let Some(tab) = self.tabs.get(idx).cloned() {
            // The left column follows the tab, then the body loads for it.
            self.sync_left_to_tab(&tab.repo, tab.number);
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
                    self.status = format!("loading #{}…", item.number);
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
                }
            })
            .collect()
    }

    /// The left column populates from the tab: repos cursor onto the tab's
    /// repo (filtering the list to it) and list cursor onto the tab's row.
    /// Called when a tab opens and whenever the active tab changes, so both
    /// columns always show the same task.
    fn sync_left_to_tab(&mut self, repo: &str, number: u64) {
        if let Some(pos) = self.repos.iter().position(|row| match row {
            RepoRow::Repo(slug) => slug.as_str() == repo,
            RepoRow::All => false,
        }) {
            self.repos_cursor = pos;
            self.apply_repo_filter();
        }
        if let Some(index) = self
            .visible_items()
            .iter()
            .position(|item| item.repo == repo && item.number == number)
        {
            self.list_cursor = index;
        }
    }

    /// The agent pane shows only while an agent is being arranged or is
    /// running. A finished run stays in the detail as a card; the pane
    /// itself is for work in progress, so idle screens keep the detail tall.
    pub fn agent_active(&self) -> bool {
        self.agent_requested.is_some()
            || self.agent_go.is_some()
            || self.agent_running
            || matches!(
                &self.agent_record,
                Some(record) if record.status == AgentStatus::Running
            )
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

    /// `Enter` on a worktree row: work on it — the plan takes the row's
    /// branch and goes to the loop, which launches straight into the
    /// existing tree (no creation question for what is already there).
    fn use_worktree_row(&mut self) {
        if let Some(Modal::Worktrees(menu)) = self.modal.take() {
            let Some(row) = menu.rows.get(menu.cursor).cloned() else {
                self.modal = Some(Modal::Worktrees(menu));
                return;
            };
            let mut plan = menu.plan;
            plan.branch = row.branch.as_str().to_string();
            plan.agent = menu.agent;
            self.agent_requested = Some(plan);
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
                        plan.agent = agent;
                        // The loop decides: existing tree launches straight
                        // in, missing tree gets the creation question.
                        self.agent_requested = Some(plan);
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

    /// `g` asks for an agent handoff on the selected row. Validation happens
    /// here; the terminal, git and the seed file belong to the loop, which
    /// consumes `agent_requested` (outbox, like `detail_requested`). Nothing
    /// is written and nothing is spawned by pressing `g`: it opens the agent
    /// menu, and the menu leads to the worktree choice.
    /// Follows the active tab when one is open.
    pub fn request_agent(&mut self) {
        if self.agent_running {
            self.status = "an agent is already running in the box below".to_string();
            return;
        }
        let Some(item) = self.effective_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let hermes = match self.binaries.as_ref().and_then(|b| b.hermes.clone()) {
            Some(hermes) => AgentChoice {
                id: AgentId::Hermes,
                program: hermes,
            },
            None => AgentChoice {
                id: AgentId::Hermes,
                // Resolved again at launch; a missing binary then fails with
                // its own message instead of refusing the menu now.
                program: std::path::PathBuf::from("hermes"),
            },
        };
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
        }));
    }

    /// An agent row was confirmed: with existing worktrees the user picks
    /// one (or manages them); without, they name the new one directly.
    fn pick_agent(&mut self, agent: AgentChoice, plan: AgentPlan) {
        let rows = self.repo_worktrees(&plan);
        if rows.is_empty() {
            self.modal = Some(Modal::NameInput(NameInput {
                value: plan.branch.clone(),
                purpose: NamePurpose::Create { agent, plan },
            }));
        } else {
            self.modal = Some(Modal::Worktrees(WorktreeMenu {
                agent,
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

    /// The record card shows for the selected item when it is that item's
    /// record and the card was not dismissed for it. A record glued to a PR
    /// that is not on screen would be a lie, so anything else shows detail.
    /// Follows the active tab when one is open.
    pub fn visible_record(&self) -> Option<&AgentRecord> {
        let record = self.agent_record.as_ref()?;
        let item = self.effective_item()?;
        if record.repo == item.repo && record.number == item.number {
            let key = (item.repo.clone(), item.number);
            if self.agent_dismissed_for.as_ref() != Some(&key) {
                return Some(record);
            }
        }
        None
    }

    /// `x` deletes the single record. After the exit there is nothing to
    /// stop, so closing only forgets — it never claims to kill anything.
    pub fn close_agent_record(&mut self) {
        if let Some(record) = self.agent_record.take() {
            self.agent_dismissed_for = None;
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
                        out.push("issue · no branch — g refuses, m closes".to_string());
                    }
                }
            }
            None => out.push("task · nothing selected".to_string()),
        }
        if let Some(record) = self.visible_record() {
            out.push(String::new());
            out.push(record.card());
        } else if let Some(record) = &self.agent_record {
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
                            self.apply_selection(selected);
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
                                self.pick_agent(row, menu.plan);
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
            match key {
                Key::Esc | Key::Char('n') => self.modal = None,
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
                            self.status = describe(outcome);
                        }
                        // A worktree write needs the loop (git, seed file,
                        // terminal): `y` only moves the agreed plan there.
                        Some(Pending::Agent(plan, path)) => {
                            self.agent_go = Some((plan, path));
                        }
                        // A delete is a write too (to the clone, via the
                        // loop): `y` moves the agreed order there.
                        Some(Pending::Delete(order)) => {
                            self.delete_go = Some(order);
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
                Key::Char(c) => self.query.push(c),
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
            // terminal, so this only validates and asks).
            Key::Char('g') => self.request_agent(),
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
                    self.agent_dismissed_for = Some((item.repo.clone(), item.number));
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
            // arrow did not point.
            Key::ShiftRight => match self.pane {
                Pane::Repos => self.pane = Pane::Detail,
                Pane::List => self.pane = Pane::Agent,
                Pane::Detail | Pane::Agent => {}
            },
            Key::ShiftLeft => match self.pane {
                Pane::Detail => self.pane = Pane::Repos,
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
                Pane::Detail => self.pane = Pane::Agent,
                Pane::List | Pane::Agent => {}
            },
            Key::Right => {
                // Right column top is the detail; from anywhere on the left,
                // go to the detail.
                if matches!(self.pane, Pane::Repos | Pane::List) {
                    self.pane = Pane::Detail;
                }
            }
            Key::Tab => self.pane = next_pane(self.pane, true, self.agent_active()),
            Key::BackTab => self.pane = next_pane(self.pane, false, self.agent_active()),
            Key::Up => self.move_cursor(-1),
            Key::Down => self.move_cursor(1),
            Key::Backspace => {}
            Key::Char(_) => {}
            // In the repos pane Enter drills in: the filter is already live
            // (it follows the cursor), so reaching the list shows the repo's
            // items.
            Key::Enter if self.pane == Pane::Repos => {
                self.pane = Pane::List;
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
            Pane::List => self.list_cursor = step(self.list_cursor, delta, self.list_len),
            Pane::Detail => {
                let next = self.detail_scroll as i32 + delta;
                self.detail_scroll = next.max(0) as u16;
            }
            Pane::Agent => {
                let next = self.agent_scroll as i32 + delta;
                self.agent_scroll = next.max(0) as u16;
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
