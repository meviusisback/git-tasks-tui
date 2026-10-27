//! The agent runs *inside* the agent box: a pty-backed terminal embedded in
//! the TUI, parsed with `vt100` and rendered as cells every frame.
//!
//! What this replaces: the old handoff suspended the TUI and gave the real
//! terminal to the agent — the box showed only a record afterwards. Now the
//! agent lives in the box from the first byte: output streams into the pane,
//! keys typed with the pane focused go to the agent.
//!
//! Security discipline, same as the old interactive path (F2, S3): the child
//! starts with `env_clear()` plus the agent allowlist — no `GH_TOKEN`, no
//! `GIT_*` — in the worktree directory. A pty hangup (master closed when
//! gtasks dies) delivers SIGHUP to the child, and quitting kills it
//! explicitly: no orphan agent is left running without a box.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use vt100::Color as VtTerm;

use crate::agent::{AgentRecord, SeedFile};
use crate::app::Key;
use crate::gh::CheckedArgv;

/// A cell colour in the embedded terminal. Kept separate from the `vt100`
/// type so the app state (`App`, view model) never names a parser type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// One rendered cell: text plus style. `text` is one grapheme cluster as
/// `vt100` segments it — never a control character, but still sanitised at
/// the render boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCell {
    pub text: String,
    pub fg: Palette,
    pub bg: Palette,
    pub bold: bool,
    pub inverse: bool,
}

/// The visible screen: one row per line, one cell per column. Rows are
/// trimmed of trailing blank cells (invisible either way); the journal
/// stores them as-is, so trimming also bounds its memory.
pub type AgentScreen = Vec<Vec<AgentCell>>;

/// Past live screens, oldest first: the wheel's history for repaint-style
/// apps. See `scroll` for how the two histories divide the work.
const JOURNAL_CAP: usize = 120;

/// Minimum gap between two recorded frames. Spinners and progress bars
/// repaint constantly with big diffs (a full-width bar every 100 ms); the
/// gap keeps them from evicting the real history while still sampling it.
const JOURNAL_GAP: Duration = Duration::from_millis(500);

/// Cells that must change for a frame to count as history. A spinner tick
/// moves one or two cells — that is motion, not history. Anything bigger
/// (a finished line, a shifted viewport) is kept.
const JOURNAL_THRESHOLD: usize = 16;

/// A cell that renders as nothing: blank text in default style. The
/// snapshot trims trailing runs of these.
fn is_blank(cell: &AgentCell) -> bool {
    cell.text == " "
        && cell.fg == Palette::Default
        && cell.bg == Palette::Default
        && !cell.bold
        && !cell.inverse
}

/// How many cells differ between two frames, position by position (a row
/// present on one side only counts whole). The journal's "did the screen
/// move on" measure.
fn frame_distance(a: &AgentScreen, b: &AgentScreen) -> usize {
    let mut changed = 0;
    for i in 0..a.len().max(b.len()) {
        match (a.get(i), b.get(i)) {
            (Some(ra), Some(rb)) => {
                for j in 0..ra.len().max(rb.len()) {
                    if ra.get(j) != rb.get(j) {
                        changed += 1;
                    }
                }
            }
            (Some(r), None) | (None, Some(r)) => changed += r.len(),
            // Unreachable: `i` runs below the longer side's length.
            (None, None) => {}
        }
    }
    changed
}

fn map_color(color: VtTerm) -> Palette {
    match color {
        VtTerm::Default => Palette::Default,
        VtTerm::Idx(index) => Palette::Idx(index),
        VtTerm::Rgb(r, g, b) => Palette::Rgb(r, g, b),
    }
}

/// The alternate-screen fold: drops exactly the enter/exit sequences
/// (`CSI ? 1047` / `? 1049` with `h` / `l`) and feeds everything else
/// untouched. Cursor visibility (`?25`), mouse tracking (`?1000`) and
/// bracketed paste (`?2004`) pass through (the parser ignores them, as
/// before), and a combined parameter list (`?25;1049h`) passes through
/// whole — dropping half a request would be worse than keeping the
/// alternate screen. Streaming-safe: a sequence split across two reads is
/// held back (bounded) until it completes or proves itself something else.
/// Held bytes are only ever `ESC [ ? digits` — never text — so a child that
/// dies mid-sequence loses nothing visible.
pub struct AltScreenFilter {
    pending: Vec<u8>,
}

impl Default for AltScreenFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AltScreenFilter {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            if input[i] != 0x1b {
                out.push(input[i]);
                i += 1;
                continue;
            }
            match scan_csi(&input[i..]) {
                Csi::Complete { len, drop } => {
                    if !drop {
                        out.extend_from_slice(&input[i..i + len]);
                    }
                    i += len;
                }
                Csi::Incomplete => {
                    if input.len() - i > 32 {
                        // Longer than any sequence we drop: not ours, flush.
                        out.extend_from_slice(&input[i..]);
                        i = input.len();
                    } else {
                        break;
                    }
                }
                Csi::Other => {
                    out.push(input[i]);
                    i += 1;
                }
            }
        }
        self.pending = input[i..].to_vec();
        out
    }
}

enum Csi {
    Complete { len: usize, drop: bool },
    Incomplete,
    Other,
}

/// Classify the CSI starting at `buf[0]` (`ESC`): complete (with whether it
/// is an alternate-screen switch), incomplete (need more bytes — including a
/// lone trailing `ESC`, which may be a split sequence), or not a CSI at all.
fn scan_csi(buf: &[u8]) -> Csi {
    if buf.len() < 2 {
        return Csi::Incomplete;
    }
    if buf[1] != b'[' {
        return Csi::Other;
    }
    let mut j = 2;
    let mut private = false;
    if j < buf.len() && buf[j] == b'?' {
        private = true;
        j += 1;
    }
    let params_start = j;
    while j < buf.len() && (buf[j].is_ascii_digit() || buf[j] == b';') {
        j += 1;
    }
    if j >= buf.len() {
        return Csi::Incomplete;
    }
    let params_end = j;
    while j < buf.len() && (0x20..=0x2f).contains(&buf[j]) {
        j += 1;
    }
    if j >= buf.len() {
        return Csi::Incomplete;
    }
    if !(0x40..=0x7e).contains(&buf[j]) {
        return Csi::Other;
    }
    let len = j + 1;
    // A combined list (`?25;1049h`) is not ours: only the exact single
    // parameter drops, the rest passes through whole.
    let params = &buf[params_start..params_end];
    let drop =
        private && (buf[j] == b'h' || buf[j] == b'l') && (params == b"1047" || params == b"1049");
    Csi::Complete { len, drop }
}

/// One agent inside the box: the pty child, its output parsed live, the seed
/// file kept until the run ends.
///
/// Fullscreen TUIs (Pi, agy, …) draw on the alternate screen, whose scrolls
/// never reach the main scrollback — the wheel would sit on an empty
/// history while the agent visibly works. Folding the enter/exit sequences
/// (`?1047`/`?1049` + `h`/`l`) away makes the app draw on the main screen,
/// where every scrolled line stays reachable. Only those four shapes are
/// dropped; everything else passes through untouched.
pub struct EmbeddedSession {
    parser: vt100::Parser,
    alt: AltScreenFilter,
    /// Past live screens, oldest first (cap `JOURNAL_CAP`): the wheel's
    /// history for repaint apps that never scroll. See `scroll`.
    journal: VecDeque<AgentScreen>,
    /// How far back into the journal the view sits. Zero is the live tail;
    /// typing resets it. Only read while `in_journal` holds.
    journal_offset: usize,
    /// Whether the wheel currently walks the journal. Set by `scroll`
    /// from the scrollback probe; cleared by typing.
    in_journal: bool,
    last_record: Instant,
    rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn std::io::Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    seed: Option<SeedFile>,
    repo: String,
    number: u64,
    profile: String,
    agent: String,
    worktree: PathBuf,
    seed_lines: usize,
    start: Instant,
    cols: u16,
    rows: u16,
}

impl EmbeddedSession {
    /// Spawn `argv` (the checked agent argv) attached to a fresh pty sized
    /// `cols`×`rows`. The environment is cleared plus the agent allowlist,
    /// built from `parent_env` so tests can plant a dirty parent.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        argv: &CheckedArgv,
        seed: SeedFile,
        seed_lines: usize,
        repo: String,
        number: u64,
        profile: String,
        agent: String,
        worktree: PathBuf,
        cols: u16,
        rows: u16,
        parent_env: &HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let system = native_pty_system();
        let pair = system
            .openpty(size)
            .context("cannot open a pty for the agent")?;
        let mut cmd = CommandBuilder::new(&argv.program);
        cmd.args(&argv.args);
        cmd.cwd(&argv.cwd);
        cmd.env_clear();
        for (key, value) in crate::env::agent_env_from(parent_env) {
            cmd.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("cannot spawn the agent in the pty")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("cannot read the agent pty")?;
        let writer = pair
            .master
            .take_writer()
            .context("cannot write to the agent pty")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || pump(reader, tx));
        Ok(Self {
            parser: vt100::Parser::new(
                NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN),
                NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN),
                1000,
            ),
            alt: AltScreenFilter::new(),
            rx,
            writer,
            master: pair.master,
            child,
            seed: Some(seed),
            repo,
            number,
            profile,
            agent,
            worktree,
            seed_lines,
            start: Instant::now(),
            cols,
            rows,
            journal: VecDeque::new(),
            journal_offset: 0,
            in_journal: false,
            last_record: Instant::now(),
        })
    }

    /// Feed every pending output chunk into the parser, then file the
    /// repaint if there was one. Scrolled lines need no filing: the parser
    /// keeps them, and the wheel reads them back through it. True when the
    /// screen changed and the caller should re-snapshot.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..256 {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    let clean = self.alt.feed(&bytes);
                    self.parser.process(&clean);
                    changed = true;
                }
                Err(_) => break,
            }
        }
        if changed {
            let live = self.live_snapshot();
            self.record_frame(&live);
        }
        changed
    }

    /// Record the live screen when it moved on substantially: a spinner
    /// tick (a cell or two) is not history, a new line or a repainted frame
    /// is. Compared against the last recorded frame, so slow output
    /// accumulates until it is worth keeping.
    fn record_frame(&mut self, live: &AgentScreen) {
        let changed = match self.journal.back() {
            None => usize::MAX,
            Some(last) => frame_distance(live, last),
        };
        if changed >= JOURNAL_THRESHOLD
            && (self.journal.is_empty() || self.last_record.elapsed() >= JOURNAL_GAP)
        {
            if self.journal.len() >= JOURNAL_CAP {
                self.journal.pop_front();
            }
            self.journal.push_back(live.clone());
            if self.journal_offset > 0 {
                // The newest frame appended at the end: keep pointing at
                // the same frame instead of drifting one step newer.
                self.journal_offset = (self.journal_offset + 1).min(self.journal.len());
            }
            self.last_record = Instant::now();
        }
    }

    /// The screen for the UI layer. Offset zero is the live tail; above
    /// that the wheel walks the journal frames. Rows are trimmed of
    /// trailing blanks either way.
    pub fn snapshot(&self) -> AgentScreen {
        if self.in_journal && self.journal_offset > 0 {
            if let Some(frame) = self
                .journal
                .len()
                .checked_sub(self.journal_offset)
                .and_then(|i| self.journal.get(i))
            {
                return frame.clone();
            }
        }
        self.live_snapshot()
    }

    /// The current parsed screen, row by row. Trailing blank cells are
    /// dropped: invisible on screen, and the journal stores this shape, so
    /// keeping them would only bloat it.
    fn live_snapshot(&self) -> AgentScreen {
        let screen = self.parser.screen();
        let mut out = Vec::with_capacity(usize::from(self.rows));
        for row in 0..self.rows {
            let mut line = Vec::with_capacity(usize::from(self.cols));
            for col in 0..self.cols {
                match screen.cell(row, col) {
                    Some(cell) => line.push(AgentCell {
                        text: cell.contents().to_string(),
                        fg: map_color(cell.fgcolor()),
                        bg: map_color(cell.bgcolor()),
                        bold: cell.bold(),
                        inverse: cell.inverse(),
                    }),
                    None => line.push(AgentCell {
                        text: " ".to_string(),
                        fg: Palette::Default,
                        bg: Palette::Default,
                        bold: false,
                        inverse: false,
                    }),
                }
            }
            while line.last().is_some_and(is_blank) {
                line.pop();
            }
            out.push(line);
        }
        out
    }

    /// Scroll the wheel's history: exact scrolled lines when the child
    /// scrolled (the parser keeps them, the probe below measures them),
    /// journal frames when it never did (repaint apps). Clamped at both
    /// ends — past the oldest entry stays on it.
    pub fn scroll(&mut self, delta: i32) {
        // How much real scrollback sits behind the live screen: push the
        // offset to the top and read back where it clamped. Restored right
        // after — nothing renders in between, so the probe is invisible.
        let current = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let content = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(current);
        if content > 0 {
            let next = (current as i32 + delta).max(0) as usize;
            self.parser.screen_mut().set_scrollback(next.min(content));
            self.journal_offset = 0;
            self.in_journal = false;
        } else {
            let next = (self.journal_offset as i32 + delta).max(0) as usize;
            self.journal_offset = next.min(self.journal.len());
            self.in_journal = true;
        }
    }

    /// The directory the child runs in: the loop refuses to rename or delete
    /// the tree a live session is working in.
    pub fn worktree(&self) -> &std::path::Path {
        &self.worktree
    }

    /// Send raw bytes to the agent. Typing jumps back to the live tail, so a
    /// reply never lands on a history view the user is reading. Failures are
    /// ignored — a dead child is reported by `poll_exit` on the next tick,
    /// not by a write error here.
    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.screen_mut().set_scrollback(0);
        self.journal_offset = 0;
        self.in_journal = false;
        use std::io::Write as _;
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize the pty and the parser. No-ops when nothing changed: resizing
    /// reflows the parser, so it must not run on every tick.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let nonzero_cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
        let nonzero_rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let _ = self.master.resize(size);
        self.parser.set_size(nonzero_rows, nonzero_cols);
    }

    /// Non-blocking exit check. `Some` once, when the child just died: the
    /// record is built here, the seed file unlinked with it.
    pub fn poll_exit(&mut self) -> Option<AgentRecord> {
        let exit = match self.child.try_wait() {
            Ok(Some(exit)) => exit,
            Ok(None) | Err(_) => return None,
        };
        let secs = self.start.elapsed().as_secs();
        drop(self.seed.take());
        let status = crate::agent::status_for_pty_exit(exit.exit_code(), exit.signal(), secs);
        Some(AgentRecord {
            repo: self.repo.clone(),
            number: self.number,
            profile: self.profile.clone(),
            agent: self.agent.clone(),
            worktree: self.worktree.clone(),
            seed_lines: self.seed_lines,
            status,
        })
    }

    /// Kill the child, best effort: quitting must not leave an agent running
    /// without a box. The seed file is unlinked on drop either way.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

/// Reader thread: blocking reads, chunked sends. Ends on EOF, error, or a
/// dropped receiver — whichever comes first.
fn pump(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
}

/// A keypress as pty bytes. `None` means "the app handles it": `Tab`
/// cycles panes, `Shift`+arrows move between boxes, `Ctrl-C` handled by
/// the caller (interrupt when focused, quit otherwise). Everything else is
/// agent input — including `Esc`, `q`, `?` and `R`. The caller routes:
/// focused on a running agent the bytes go to the pty, otherwise the key
/// goes to the app.
pub fn key_to_pty(key: &Key) -> Option<Vec<u8>> {
    match key {
        // Esc reaches the agent (vim mode, popups, cancel): leaving the box
        // is Tab's job. Tab/BackTab and the Shift arrows stay app keys.
        Key::Esc => Some(vec![0x1b]),
        Key::Tab | Key::BackTab => None,
        Key::ShiftUp | Key::ShiftDown | Key::ShiftLeft | Key::ShiftRight => None,
        Key::CtrlC => Some(vec![0x03]),
        Key::Enter => Some(b"\r".to_vec()),
        Key::Backspace => Some(vec![0x7f]),
        Key::Up => Some(b"\x1b[A".to_vec()),
        Key::Down => Some(b"\x1b[B".to_vec()),
        Key::Right => Some(b"\x1b[C".to_vec()),
        Key::Left => Some(b"\x1b[D".to_vec()),
        Key::Char(c) => {
            let mut bytes = [0u8; 4];
            Some(c.encode_utf8(&mut bytes).as_bytes().to_vec())
        }
    }
}
