//! Nerd Font icons, only when the host has them.
//!
//! The glyphs are Private Use Area codepoints: on a terminal without a Nerd
//! Font they render as boxes or blanks, which is worse than no icon. So the
//! icons are gated behind a probe — `GTASKS_NERD=1` forces them on, `=0`
//! forces them off, otherwise font directories are scanned for a family with
//! "nerd" in the path. The probe runs once per process; the override is read
//! on every call so tests stay deterministic.
//!
//! The codepoints are Font Awesome names from the Nerd Fonts set, written as
//! escapes (never pasted glyphs): `f07b` folder, `f126` code-fork,
//! `f06a` exclamation-circle, `f120` terminal.

use std::path::PathBuf;
use std::sync::OnceLock;

static PROBE: OnceLock<bool> = OnceLock::new();

/// True when icons may be used: explicit override first, host probe after.
pub fn nerd() -> bool {
    if let Some(value) = std::env::var_os("GTASKS_NERD") {
        let text = value.to_string_lossy().to_ascii_lowercase();
        return !matches!(text.as_str(), "" | "0" | "false" | "no" | "off");
    }
    *PROBE.get_or_init(probe_host)
}

/// A font directory whose path mentions "nerd" means a Nerd Font family is
/// installed. Only the path is read, never font contents: fast and enough.
fn probe_host() -> bool {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        roots.push(home.join(".local/share/fonts"));
        roots.push(home.join(".fonts"));
    }
    roots.push(PathBuf::from("/usr/share/fonts"));
    roots.push(PathBuf::from("/usr/local/share/fonts"));
    let mut seen = 0;
    roots.iter().any(|root| scan(root, 4, &mut seen))
}

/// Depth-limited walk looking for "nerd" in any entry path. Capped so a huge
/// font tree cannot slow startup: 3000 entries are more than enough to find
/// a family name.
fn scan(dir: &std::path::Path, depth: usize, seen: &mut usize) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        *seen += 1;
        if *seen > 3000 {
            return false;
        }
        let path = entry.path();
        if path.to_string_lossy().to_ascii_lowercase().contains("nerd") {
            return true;
        }
        if depth > 0 && path.is_dir() && scan(&path, depth - 1, seen) {
            return true;
        }
    }
    false
}

/// Repo row marker: a folder glyph, otherwise the plain triangle.
pub fn repo() -> &'static str {
    if nerd() { "\u{f07b} " } else { "\u{25b8} " }
}

/// Pull-request glyph, empty without a Nerd Font (colour alone marks it).
pub fn kind_pr() -> &'static str {
    if nerd() { "\u{f126} " } else { "" }
}

/// Issue glyph, empty without a Nerd Font (colour alone marks it).
pub fn kind_issue() -> &'static str {
    if nerd() { "\u{f06a} " } else { "" }
}

/// Branch glyph for the `head → base` line, empty without a Nerd Font.
pub fn branch() -> &'static str {
    if nerd() { "\u{f126} " } else { "" }
}

/// Agent pane glyph, empty without a Nerd Font.
pub fn agent() -> &'static str {
    if nerd() { "\u{f120} " } else { "" }
}
