//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

use crate::actions::{Action, MergeMethod, RepoSlug, argv_for};
use crate::actions_ui::{Confirmation, Executed, confirmation_for, execute};
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
}

/// The `R` multi-select: which of the configured repos the list shows.
/// `Space` toggles the row, `Enter` applies, `Esc` cancels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMenu {
    pub rows: Vec<String>,
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
    pub repos: Vec<String>,
    /// Every configured repo, even when the list shows a subset: the rows
    /// the `R` menu offers. Set once at startup, never filtered.
    pub all_repos: Vec<String>,
    /// The repo selected in the repos pane; `All` means "all".
    pub repo_filter: RepoFilter,
    /// Where the drill-down came from, by identity: `Esc` in a filtered
    /// list restores the cursor to this repo's row, not to an index — the
    /// list may have changed while drilled in. `None` when never drilled in.
    pub last_repo: Option<RepoSlug>,
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
    /// The action awaiting `y`: confirmation and argv are the same object.
    pub pending_action: Option<PendingAction>,
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
            last_repo: None,
            selection_applied: None,
            loading: false,
            failed: 0,
            refresh_requested: false,
            binaries: None,
            write_timeout: std::time::Duration::from_secs(30),
            pending_action: None,
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

    /// Take the result of a fetch. Repo order is preserved, so the list does
    /// not reshuffle between refreshes.
    pub fn apply_outcome(&mut self, outcome: FetchOutcome) {
        let FetchOutcome { items, failures } = outcome;
        self.items = items;
        self.failed = failures.len();
        self.loading = false;

        // Keep the configured repos: a repo with no open items must still be
        // visible, otherwise it looks like it was never configured.
        if self.repos.is_empty() {
            for item in &self.items {
                if !self.repos.iter().any(|repo| repo == &item.repo) {
                    self.repos.push(item.repo.clone());
                }
            }
        }
        self.set_lengths(self.repos.len(), self.visible_items().len());
        // Row 0 is the "all repos" selector, not a repository: do not count it.
        let configured = self.repos.len().saturating_sub(1);
        self.status = match self.failed {
            0 => format!("{} items from {} repos", self.items.len(), configured),
            failed => format!(
                "{} items from {} repos · {} failed",
                self.items.len(),
                configured,
                failed
            ),
        };
    }

    /// Install the configured repos. Row 0 is "all repos", so the user can get
    /// back to the unfiltered list without leaving the pane.
    pub fn set_repos(&mut self, repos: Vec<String>) {
        self.repos = std::iter::once("— all repos —".to_string())
            .chain(repos)
            .collect();
        self.repos_cursor = 0;
        self.repo_filter = RepoFilter::All;
        self.set_lengths(self.repos.len(), self.visible_items().len());
    }

    /// Row 0 means "everything"; any other row filters the list to that repo.
    /// The row text becomes a `RepoSlug` here, once: downstream only ever
    /// sees `All` or a valid slug. Configured repos are validated at import
    /// (`config`), so a failure here is a bug, not user input.
    fn apply_repo_filter(&mut self) {
        self.repo_filter = match self.repos_cursor {
            0 => RepoFilter::All,
            n => match self.repos.get(n) {
                Some(row) => RepoFilter::One(
                    RepoSlug::parse(row).expect("configured repos are validated at import"),
                ),
                None => RepoFilter::All,
            },
        };
        self.list_cursor = 0;
        self.set_lengths(self.repos.len(), self.visible_items().len());
        // The detail belonged to the previous list: showing it here would say
        // "this is the item you are looking at" about an item that is gone.
        self.detail.clear();
        self.detail_for = None;
        self.detail_requested = None;
        let shown = self.visible_items().len();
        self.status = match &self.repo_filter {
            RepoFilter::One(slug) => format!("{shown} items in {slug}"),
            RepoFilter::All => format!("— all repos — · {shown} items"),
        };
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
        if self.pane == Pane::Detail {
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
        }
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
        let live: Vec<&str> = self.repos.iter().skip(1).map(String::as_str).collect();
        let marked: BTreeSet<usize> = self
            .all_repos
            .iter()
            .enumerate()
            .filter(|(_, row)| live.contains(&row.as_str()))
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
    fn apply_selection(&mut self, selected: Vec<String>) {
        let shown = if selected.is_empty() {
            self.all_repos.clone()
        } else {
            selected.clone()
        };
        self.selection_applied = Some(selected);
        self.modal = None;
        self.set_repos(shown);
    }

    /// The item under the cursor, if any.
    pub fn selected_item(&self) -> Option<&Item> {
        self.visible_items().get(self.list_cursor).copied()
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
        let Some(binaries) = self.binaries.clone() else {
            self.status = "no binaries resolved".into();
            return;
        };
        let action = Action::Merge {
            repo: slug,
            number,
            method: FIRST_ACTION_METHOD,
            delete_branch: FIRST_ACTION_DELETES_BRANCH,
        };
        match argv_for(&binaries, &action) {
            Ok(argv) => {
                let Confirmation {
                    summary,
                    argv: shown,
                } = confirmation_for(&action, &argv);
                self.status = shown.join(" ");
                self.modal = Some(Modal::Confirm(summary));
                self.pending_action = Some(PendingAction { action, argv });
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
                            let selected: Vec<String> = menu
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
            match key {
                Key::Esc | Key::Char('n') => self.modal = None,
                Key::Char('y') => {
                    self.modal = None;
                    match self.pending_action.take() {
                        // X-1: the message comes from the type, never invented.
                        // `Unknown` says "it may have been applied", which is the
                        // only honest sentence when a write times out.
                        Some(pending) => {
                            self.status = describe(execute(&pending.argv, self.write_timeout));
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
            Key::Esc => match self.pane {
                Pane::Detail => self.pane = Pane::List,
                // Drilled in (a repo filter is active): back to the repos
                // pane, by identity — the list may have changed while
                // drilled in, so an index could point at another repo.
                Pane::List if matches!(self.repo_filter, RepoFilter::One(_)) => {
                    self.pane = Pane::Repos;
                    match self
                        .last_repo
                        .as_ref()
                        .and_then(|slug| self.repos.iter().position(|row| row == slug.as_str()))
                    {
                        Some(i) => self.repos_cursor = i,
                        // Gone: home to row 0, which also resets the filter
                        // (a cursor on "all" with a dead filter would show
                        // an empty list and claim a repo that is gone).
                        None => {
                            self.repos_cursor = 0;
                            self.last_repo = None;
                            self.apply_repo_filter();
                        }
                    }
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
            // items. The origin is recorded by identity for the way back.
            Key::Enter if self.pane == Pane::Repos => {
                self.last_repo = match &self.repo_filter {
                    RepoFilter::One(slug) => Some(slug.clone()),
                    RepoFilter::All => None,
                };
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
    match outcome {
        Executed::Finished(status) => format!("done ({status})"),
        Executed::Refused(why) => format!("refused: {why}"),
        Executed::SpawnFailed(why) => format!("cannot run: {why}"),
        Executed::Unknown(why) => format!("unknown — it may have been applied: {why}"),
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
