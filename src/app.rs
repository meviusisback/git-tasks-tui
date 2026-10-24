//! Application state and the **pure** update function: no terminal, no I/O,
//! no async. The event loop only translates crossterm events into [`Msg`] and
//! calls [`App::update`], so every transition is unit-testable.

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

#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => {}
            Msg::Key(key) => self.on_key(key),
        }
    }

    fn on_key(&mut self, key: Key) {
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
                Key::Up | Key::Down | Key::Tab | Key::BackTab => {}
            }
            return;
        }

        match key {
            Key::Char('/') => self.searching = true,
            Key::Esc | Key::Char('q') => self.quit = true,
            Key::Tab => self.pane = next_pane(self.pane, true),
            Key::BackTab => self.pane = next_pane(self.pane, false),
            Key::Up => self.move_cursor(-1),
            Key::Down => self.move_cursor(1),
            Key::Backspace => {}
            Key::Char(_) => {}
            Key::Enter => {
                self.status = format!("open {:?}", self.pane);
            }
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
