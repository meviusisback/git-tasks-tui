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

use std::collections::HashMap;
use std::io::Read;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

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

/// The visible screen: one row per line, one cell per column.
pub type AgentScreen = Vec<Vec<AgentCell>>;

fn map_color(color: VtTerm) -> Palette {
    match color {
        VtTerm::Default => Palette::Default,
        VtTerm::Idx(index) => Palette::Idx(index),
        VtTerm::Rgb(r, g, b) => Palette::Rgb(r, g, b),
    }
}

/// One agent inside the box: the pty child, its output parsed live, the seed
/// file kept until the run ends.
pub struct EmbeddedSession {
    parser: vt100::Parser,
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
        })
    }

    /// Feed every pending output chunk into the parser. True when the screen
    /// changed and the caller should re-snapshot.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..256 {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    self.parser.process(&bytes);
                    changed = true;
                }
                Err(_) => break,
            }
        }
        changed
    }

    /// The current visible screen, row by row, for the UI layer.
    pub fn snapshot(&self) -> AgentScreen {
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
            out.push(line);
        }
        out
    }

    /// Scroll the visible window through history: positive looks back, negative
    /// returns toward the live tail. Clamped at zero; vt100 clamps the top
    /// to the actual scrollback size.
    pub fn scroll(&mut self, delta: i32) {
        let current = self.parser.screen().scrollback() as i32;
        let next = (current + delta).max(0) as usize;
        self.parser.screen_mut().set_scrollback(next);
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

/// A keypress as pty bytes. `None` means "the app handles it": `Esc`
/// unfocuses the box, `Tab` cycles panes, `Shift`+arrows move between boxes,
/// `Ctrl-C` handled by the caller (interrupt when focused, quit otherwise).
/// Everything else is agent input — including `q`, `?` and `R`. The caller
/// routes: focused on a running agent the bytes go to the pty, otherwise the
/// key goes to the app.
pub fn key_to_pty(key: &Key) -> Option<Vec<u8>> {
    match key {
        Key::Esc | Key::Tab | Key::BackTab => None,
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
