//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

use crate::actions::{Action, MergeMethod, RepoSlug, argv_for};
use crate::actions_ui::{Confirmation, Executed, confirmation_for, execute};
use crate::agent::{AgentPlan, AgentRecord, plan_agent};
use crate::binaries::Binaries;
use crate::fetch::FetchOutcome;
use crate::gh::CheckedArgv;
use crate::model::Item;
use crate::sanitize::sanitize;

use std::collections::BTreeSet;

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Repos,
    List,
    Detail,
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
    /// A startup notice (broken config): information, not a question. `Esc`
    /// dismisses it, `q` quits through it; nothing else acts while it is up.
    Notice(String),
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
            detail: String::new(),
            detail_for: None,
            detail_requested: None,
            open_requested: None,
            show_help: false,
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
                    "{} marked — Space mark · Enter apply · Esc cancel",
                    menu.marked.len()
                );
            }
            Some(Modal::ActionMenu(_)) => {
                return String::from("↑/↓ choose · Enter confirm · Esc cancel");
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
                "q quit · ? help · ←/→ panes · ↑/↓ move · Enter open · R repos · m actions",
            )
        } else if self.pane == Pane::Repos {
            String::from("q quit · ? help · Enter drill in · ↑/↓ move")
        } else if self.repo_filter == RepoFilter::All {
            String::from(
                "q quit · ? help · ←/→ panes · ↑/↓ move · Enter open · R repos · m actions",
            )
        } else {
            String::from(
                "q quit · ? help · Esc back · ←/→ panes · ↑/↓ move · Enter open · R repos · m actions",
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
    pub fn request_open(&mut self) {
        let Some((url, number)) = self
            .selected_item()
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
        let Some(item) = self.selected_item() else {
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

    /// `Enter` in the actions menu: the picked row becomes the action and
    /// goes through the same confirmation as `a` — one dialog, one argv.
    fn apply_action_pick(&mut self) {
        if let Some(Modal::ActionMenu(menu)) = self.modal.take() {
            let Some(item) = self.selected_item() else {
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
    pub fn request_detail(&mut self) {
        let Some(item) = self.selected_item() else {
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
    /// is written and nothing is spawned by pressing `g`.
    pub fn request_agent(&mut self) {
        let Some(item) = self.selected_item() else {
            self.status = "nothing selected".to_string();
            return;
        };
        match plan_agent(
            &item.repo,
            item.number,
            &item.url,
            item.kind,
            item.head_ref_name.as_deref(),
            &self.agent_profile,
            &self.clone_roots,
        ) {
            Ok(plan) => self.agent_requested = Some(plan),
            Err(reason) => self.status = reason,
        }
    }

    /// The record card shows for the selected item when it is that item's
    /// record and the card was not dismissed for it. A record glued to a PR
    /// that is not on screen would be a lie, so anything else shows detail.
    pub fn visible_record(&self) -> Option<&AgentRecord> {
        let record = self.agent_record.as_ref()?;
        let item = self.selected_item()?;
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

    /// Build the first action for the selected row, show what it would run, and
    /// keep the pair (confirmation, argv) so `y` executes exactly that.
    fn request_action(&mut self) {
        let Some(item) = self.selected_item() else {
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
            Msg::Key(key) => self.on_key(key),
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
            // `x` forgets the last agent record; there is no process left
            // to stop by the time the TUI is back.
            Key::Char('x') => self.close_agent_record(),
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
                if let Some(item) = self.selected_item() {
                    self.agent_dismissed_for = Some((item.repo.clone(), item.number));
                }
            }
            Key::Esc => match self.pane {
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
                if self.pane == Pane::Detail {
                    self.pane = Pane::List;
                }
            }
            Key::Right => {
                if self.pane != Pane::Detail {
                    self.pane = Pane::Detail;
                }
            }
            Key::Tab => self.pane = next_pane(self.pane, true),
            Key::BackTab => self.pane = next_pane(self.pane, false),
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
        }
    }
}

fn next_pane(pane: Pane, forward: bool) -> Pane {
    // Order: list → repos → detail. The repos pane comes first on purpose: it is
    // how you narrow the list, so it must be one keypress away from it.
    match (pane, forward) {
        (Pane::List, true) | (Pane::Detail, false) => Pane::Repos,
        (Pane::Repos, true) | (Pane::List, false) => Pane::Detail,
        (Pane::Detail, true) | (Pane::Repos, false) => Pane::List,
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
