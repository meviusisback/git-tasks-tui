//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

use crate::fetch::FetchOutcome;
use crate::model::Item;

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Repos,
    List,
    Detail,
}

/// The keys the app cares about, independent of the terminal backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Up,
    Down,
    Tab,
    BackTab,
    Backspace,
    /// Ctrl-C always quits, whatever mode we are in.
    CtrlC,
}

/// Everything that can change the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Key(Key),
    Tick,
}

/// A confirmation dialog. The payload is the question key, resolved to text by
/// the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    Confirm(&'static str),
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
    /// Distinct repos, in the order they first appear.
    pub repos: Vec<String>,
    /// A fetch is in flight: `r` must not start a second one.
    pub loading: bool,
    /// Repos whose last fetch failed.
    pub failed: usize,
    /// Set by the key handler, consumed by the event loop that owns the thread.
    pub refresh_requested: bool,
    /// Markdown body of the selected item.
    pub detail: String,
    /// Which item `detail` belongs to, so the same body is not fetched twice.
    pub detail_for: Option<(String, u64)>,
    /// Set by the key handler: the loop fetches this body.
    pub detail_requested: Option<(String, u64)>,
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
            loading: false,
            failed: 0,
            refresh_requested: false,
            detail: String::new(),
            detail_for: None,
            detail_requested: None,
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

        self.repos.clear();
        for item in &self.items {
            if !self.repos.iter().any(|repo| repo == &item.repo) {
                self.repos.push(item.repo.clone());
            }
        }

        self.set_lengths(self.repos.len(), self.items.len());
        self.status = match self.failed {
            0 => format!("{} items from {} repos", self.items.len(), self.repos.len()),
            failed => format!(
                "{} items from {} repos · {} failed",
                self.items.len(),
                self.repos.len(),
                failed
            ),
        };
    }

    /// Items matching the search box. Case-insensitive on the title and the
    /// number, so `/42` finds `#42`.
    pub fn visible_items(&self) -> Vec<&Item> {
        if self.query.trim().is_empty() {
            return self.items.iter().collect();
        }
        let needle = self.query.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&needle)
                    || item.number.to_string().contains(&needle)
            })
            .collect()
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
            match key {
                Key::Esc | Key::Char('n') => self.modal = None,
                Key::Char('y') => {
                    self.modal = None;
                    self.status = "confirmed".into();
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
                Key::Up | Key::Down | Key::Tab | Key::BackTab | Key::CtrlC => {}
            }
            return;
        }

        match key {
            Key::Char('/') => self.searching = true,
            // One refresh at a time: a second `r` while loading is ignored.
            Key::Char('r') if !self.loading => {
                self.loading = true;
                self.refresh_requested = true;
                self.status = "refreshing…".to_string();
            }
            Key::Esc | Key::Char('q') => self.quit = true,
            Key::Tab => self.pane = next_pane(self.pane, true),
            Key::BackTab => self.pane = next_pane(self.pane, false),
            Key::Up => self.move_cursor(-1),
            Key::Down => self.move_cursor(1),
            Key::Backspace => {}
            Key::Char(_) => {}
            Key::Enter => self.request_detail(),
            // Handled at the top; repeated here because the match must be total.
            Key::CtrlC => {}
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        match self.pane {
            Pane::Repos => self.repos_cursor = step(self.repos_cursor, delta, self.repos_len),
            Pane::List => self.list_cursor = step(self.list_cursor, delta, self.list_len),
            Pane::Detail => {
                let next = self.detail_scroll as i32 + delta;
                self.detail_scroll = next.max(0) as u16;
            }
        }
    }
}

fn next_pane(pane: Pane, forward: bool) -> Pane {
    match (pane, forward) {
        (Pane::Repos, true) | (Pane::Detail, false) => Pane::List,
        (Pane::List, true) | (Pane::Repos, false) => Pane::Detail,
        (Pane::Detail, true) | (Pane::List, false) => Pane::Repos,
    }
}

fn step(cursor: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = cursor as i32 + delta;
    next.clamp(0, len as i32 - 1) as usize
}
