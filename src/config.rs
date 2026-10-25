//! Configuration (`config.toml`, mode 0600) and the pure validators.
//!
//! File writes go through [`write_private`]: a symlinked target is refused, a
//! fresh `0600` temp file in the same directory is written and fsynced, then
//! renamed over the target. A crash leaves either the old file or a `*.tmp.*`
//! artifact, never a half-written config.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub repos: Vec<String>,
    pub worktree_root: Option<PathBuf>,
    pub parallel: usize,
    pub timeout_s: u64,
    pub write_timeout_s: u64,
    pub refresh_s: u64,
    pub limit: u32,
    pub default_profile: Option<String>,
    pub trust_worktree_rules: bool,
    /// Launch agents through their `<agent>-sandboxed` wrapper (off by
    /// default). The wrapper contains the filesystem (bubblewrap); the
    /// launch also adds each agent's non-interactive mode where one exists:
    /// omp gets `--approval-mode yolo`, opencode/agy already carry theirs
    /// inside the wrapper, hermes keeps its Smart approvals (the default
    /// `approvals.mode`), pi has no equivalent flag. Off means the plain
    /// binaries with their normal approval prompts.
    pub sandbox_agents: bool,
    /// Terminal font size for the whole terminal (a TUI cannot resize its
    /// own glyphs): Small 9px, Medium 11px, Large 14px.
    pub font_size: FontSize,
    /// Terminal typeface for the whole terminal, from the installed
    /// monospace fonts. Needs a Nerd Font for the glyphs, otherwise the
    /// markers fall back.
    pub font_family: String,
    /// Profile the agent handoff (`g`) passes to `hermes -p`. Validated like
    /// every profile: lowercase, digits, `_`/`-`, starting alphanumeric.
    pub agent_profile: String,
    /// Directory holding local clones (`<clone_root>/<repo-name>`). `None`
    /// means the agent cannot be placed: refusing beats guessing.
    pub clone_root: Option<PathBuf>,
    /// Where to look for local clones, in order. This is the shape the old key already had
    /// (`clone_roots = ["~/repo", "~/repos"]`), and the reason it exists: clones really do live
    /// in more than one directory, and one path cannot say so. `clone_root`, when set, is tried
    /// first, so a single-root config keeps working.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clone_roots: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            repos: Vec::new(),
            worktree_root: None,
            parallel: 4,
            timeout_s: 30,
            write_timeout_s: 60,
            refresh_s: 300,
            limit: 30,
            default_profile: None,
            trust_worktree_rules: false,
            sandbox_agents: false,
            font_size: FontSize::default(),
            font_family: "monospace".to_string(),
            agent_profile: "default".to_string(),
            clone_root: None,
            clone_roots: Vec::new(),
        }
    }
}
/// Terminal font size: the whole terminal, because a TUI cannot resize its
/// own glyphs — the terminal owns every pixel. Small is 9px, Medium is 11px
/// (Foot's default), Large is 14px.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FontSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl FontSize {
    /// Pixels for the Foot `font=…:size=` line.
    pub fn px(self) -> u32 {
        match self {
            FontSize::Small => 9,
            FontSize::Medium => 11,
            FontSize::Large => 14,
        }
    }

    /// Short name for the settings row.
    pub fn name(self) -> &'static str {
        match self {
            FontSize::Small => "Small",
            FontSize::Medium => "Medium",
            FontSize::Large => "Large",
        }
    }

    /// Cycle for the settings toggle: Small → Medium → Large → Small.
    pub fn next(self) -> Self {
        match self {
            FontSize::Small => FontSize::Medium,
            FontSize::Medium => FontSize::Large,
            FontSize::Large => FontSize::Small,
        }
    }

    /// Nearest step for a size read from Foot (users edit it by hand too).
    pub fn from_px(px: u32) -> Self {
        if px <= 10 {
            FontSize::Small
        } else if px <= 12 {
            FontSize::Medium
        } else {
            FontSize::Large
        }
    }
}

/// Where Foot keeps its config: `$XDG_CONFIG_HOME/foot/foot.ini`, falling
/// back to `~/.config`.
pub fn foot_ini_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("foot").join("foot.ini"))
}

/// The `font=` line of a Foot config: `(family, size)`. `None` when the
/// file is missing or sets no font — then Foot's own default applies and
/// there is nothing to show. Only the first uncommented `font=` line
/// counts; a `:style=` suffix is preserved, not parsed.
pub fn read_foot_font(path: &Path) -> Option<(String, u32)> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(value) = line.strip_prefix("font=") else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (family, size) = match value.split_once(":size=") {
            Some((family, rest)) => {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                (family.trim(), digits.parse::<u32>().ok()?)
            }
            None => (value, 11),
        };
        if family.is_empty() {
            continue;
        }
        return Some((family.to_string(), size));
    }
    None
}

/// Set the `font=` line to `family:size=px`, preserving every other line
/// and any `:size=` suffix. A missing file or missing line grows a minimal
/// `[main]` section — Foot merges it with its defaults, so nothing else
/// changes. Returns true when the file changed.
///
/// The write is atomic (temp file + rename) with mode 0600: a crash never
/// leaves a half-written `foot.ini`, and a symlink swapped in at the target
/// path is *replaced*, never followed. Ancestor symlinks are the user's own
/// choice (stow-style setups link `~/.config` subdirs) and are left alone.
/// The family allows no control characters: it is interpolated into the file,
/// so a newline would inject extra lines across the trust boundary.
pub fn set_foot_font(path: &Path, family: &str, px: u32) -> anyhow::Result<bool> {
    let family = family.trim();
    ensure!(!family.is_empty(), "font family must not be empty");
    ensure!(
        !family.chars().any(|c| c.is_control()),
        "font family must not contain control characters: {family:?}"
    );
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "refusing to write through a symlink: {}",
            path.display()
        );
    }
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    let mut in_main = false;
    let mut has_main = false;
    for line in old.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_main = trimmed == "[main]";
            if in_main {
                has_main = true;
            }
            out.push(line.to_string());
            continue;
        }
        if !done
            && in_main
            && !trimmed.starts_with('#')
            && let Some(value) = trimmed.strip_prefix("font=")
        {
            let suffix = match value.split_once(":size=") {
                Some((_, rest)) => {
                    let cut = rest
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(rest.len());
                    rest[cut..].to_string()
                }
                None => String::new(),
            };
            out.push(format!("font={family}:size={px}{suffix}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !has_main {
            out.push("[main]".to_string());
        }
        out.push(format!("font={family}:size={px}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    if text == old {
        return Ok(false);
    }
    // Atomic write, same discipline as the private files: a crash leaves the
    // old file or the new one, never half of each. The temp file is created
    // with `create_new` (no follow, no clobber) and renamed over the target —
    // a symlink at the target is replaced, never followed, so even a swap
    // between the check above and this write cannot redirect the bytes.
    // A missing parent is created 0700; a missing file lands 0600.
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
            #[cfg(unix)]
            std::fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .with_context(|| format!("cannot chmod {}", parent.display()))?;
        }
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = std::path::PathBuf::from(tmp);
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut f = opts
            .open(&tmp_path)
            .with_context(|| format!("cannot write {}", tmp_path.display()))?;
        f.write_all(text.as_bytes())
            .with_context(|| format!("cannot write {}", tmp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("cannot sync {}", tmp_path.display()))?;
    }
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(true)
}

/// Parse `fc-list :spacing=mono family`: one family per line, aliases comma
/// separated. Sorted, deduped, blanks dropped.
pub fn parse_fc_list(output: &str) -> Vec<String> {
    let mut families: Vec<String> = output
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    families.sort();
    families.dedup();
    families
}

/// The installed monospace families for the font toggle. A failed or empty
/// query falls back to `monospace` — the toggle always has somewhere to go.
/// The binary is resolved once (no bare-PATH spawn), runs with a cleared
/// environment, and is killed after five seconds: a hung fontconfig must not
/// hang TUI startup.
pub fn mono_fonts() -> Vec<String> {
    let program = match crate::binaries::resolve("fc-list") {
        Ok(p) => p,
        Err(_) => return vec!["monospace".to_string()],
    };
    let mut child = match std::process::Command::new(&program)
        .args([":spacing=mono", "family"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return vec!["monospace".to_string()],
    };
    let timeout = std::time::Duration::from_secs(5);
    let mut stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let done = stdout
            .as_mut()
            .map(|out| std::io::Read::read_to_end(out, &mut buf).is_ok())
            .unwrap_or(false);
        let _ = tx.send(done.then_some(buf));
    });
    // A hung fontconfig must not hang TUI startup: five seconds, then dead.
    let buf = match rx.recv_timeout(timeout) {
        Ok(Some(buf)) => buf,
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return vec!["monospace".to_string()];
        }
    };
    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => return vec!["monospace".to_string()],
    };
    if !status.success() {
        return vec!["monospace".to_string()];
    }
    let fonts = parse_fc_list(&String::from_utf8_lossy(&buf));
    if fonts.is_empty() {
        vec!["monospace".to_string()]
    } else {
        fonts
    }
}

impl Config {
    /// The roots to search for a local clone, most specific first: `clone_root` (single) then
    /// `clone_roots` (list), without duplicates. Empty means agents cannot start — which is what
    /// the refusal sentence has always said.
    pub fn agent_roots(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        if let Some(primary) = &self.clone_root {
            out.push(primary.clone());
        }
        for root in &self.clone_roots {
            if !out.contains(root) {
                out.push(root.clone());
            }
        }
        out
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            (1..=8).contains(&self.parallel),
            "parallel must be 1..=8, got {}",
            self.parallel
        );
        ensure!(
            (1..=600).contains(&self.timeout_s),
            "timeout_s must be 1..=600, got {}",
            self.timeout_s
        );
        ensure!(
            (1..=600).contains(&self.write_timeout_s),
            "write_timeout_s must be 1..=600, got {}",
            self.write_timeout_s
        );
        ensure!(
            self.refresh_s == 0 || (10..=86_400).contains(&self.refresh_s),
            "refresh_s must be 0 (off) or 10..=86400, got {}",
            self.refresh_s
        );
        // The gateway drops calls that run past ~10 s; above 30 the limit
        // does not fetch more, it fetches nothing. Refusing at load beats
        // a silent truncation — same rule as parallel (S-config).
        ensure!(
            (1..=30).contains(&self.limit),
            "limit must be 1..=30, got {}",
            self.limit
        );
        for repo in &self.repos {
            validate_repo_slug(repo)?;
        }
        if let Some(profile) = &self.default_profile {
            validate_profile(profile)?;
        }
        validate_profile(&self.agent_profile).context("invalid agent_profile")?;
        if !self.font_family.trim().is_empty() {
            ensure!(
                !self.font_family.chars().any(|c| c.is_control()),
                "font_family must be a single line without control characters: {:?}",
                self.font_family
            );
        }
        Ok(())
    }

    /// Missing file → defaults. A present but invalid file is an error: a typo
    /// must not silently fall back to something the user did not ask for.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = read_private(path)?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self).context("cannot serialize the config")?;
        write_private(path, text.as_bytes())
    }
}

/// Why the config on disk cannot be used. Two different failures, two
/// different remedies: *unreadable* (permissions, syntax — fix the file
/// itself) versus *invalid* (readable TOML with a rejected value — fix the
/// value, the message names it and the accepted range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    Unreadable { path: PathBuf, reason: String },
    Invalid { path: PathBuf, reason: String },
}

impl ConfigProblem {
    /// The on-screen sentence: file first, then what is wrong. The two kinds
    /// start with different words so a test — and a glance — tells them apart.
    pub fn message(&self) -> String {
        match self {
            ConfigProblem::Unreadable { path, reason } => {
                format!("cannot read config {}: {}", path.display(), elide(reason))
            }
            ConfigProblem::Invalid { path, reason } => {
                format!("invalid config {}: {}", path.display(), elide(reason))
            }
        }
    }
}

/// Bound a reason so the notice fits the box it is shown in.
///
/// A `toml` parse error repeats the offending line, so a 400-character token produces a
/// ~1000-character sentence. The head of that sentence carries everything a person needs — file,
/// line, column — and the tail is the line itself, printed twice. Keep the head: the first line,
/// and at most 200 characters of it.
fn elide(reason: &str) -> String {
    const MAX: usize = 200;
    let first = reason.lines().next().unwrap_or("");
    if first.chars().count() <= MAX {
        return first.to_string();
    }
    let mut out: String = first.chars().take(MAX).collect();
    out.push('…');
    out
}

/// Load the config, falling back to the demo repo when the file cannot be
/// used — but never silently: the second value names the problem for the
/// screen. A missing file is not a problem (first run); anything present
/// but unusable is.
pub fn load_or_demo(path: &Path) -> (Config, Option<ConfigProblem>) {
    if !path.exists() {
        return (demo_config(), None);
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            return (
                demo_config(),
                Some(ConfigProblem::Unreadable {
                    path: path.to_path_buf(),
                    reason: format!("{err:#}"),
                }),
            );
        }
    };
    // Syntax first (is it TOML at all?), value second (is it ours?): an
    // unknown key is readable text with a rejected meaning, so it lands in
    // Invalid, not Unreadable.
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            return (
                demo_config(),
                Some(ConfigProblem::Unreadable {
                    path: path.to_path_buf(),
                    reason: format!("{err:#}"),
                }),
            );
        }
    };
    let config: Config = match value.try_into() {
        Ok(config) => config,
        Err(err) => {
            return (
                demo_config(),
                Some(ConfigProblem::Invalid {
                    path: path.to_path_buf(),
                    reason: format!("{err:#}"),
                }),
            );
        }
    };
    if let Err(err) = config.validate() {
        return (
            demo_config(),
            Some(ConfigProblem::Invalid {
                path: path.to_path_buf(),
                reason: format!("{err:#}"),
            }),
        );
    }
    if config.repos.is_empty() {
        return (
            demo_config(),
            Some(ConfigProblem::Invalid {
                path: path.to_path_buf(),
                reason: "lists no repos — add at least one \"owner/name\"".to_string(),
            }),
        );
    }
    (config, None)
}

/// The fallback repo. Validated, not assumed: a `debug_assert` for the
/// developer and a log line for release, so a future default change that
/// breaks the demo says so instead of shipping a broken fallback.
pub fn demo_config() -> Config {
    let config = Config {
        repos: vec!["meviusisback/git-tasks-tui".to_string()],
        ..Config::default()
    };
    debug_assert!(
        config.validate().is_ok() && !config.repos.is_empty(),
        "the demo fallback must always be usable"
    );
    if let Err(err) = config.validate() {
        tracing::warn!(error = %format!("{err:#}"), "demo fallback invalid, using it anyway");
    }
    config
}

/// `owner/name`, both parts from the safe charset; no traversal, no flags.
pub fn validate_repo_slug(slug: &str) -> anyhow::Result<()> {
    ensure!(
        !slug.is_empty() && slug.len() <= 140,
        "invalid repo slug length"
    );
    let (owner, name) = slug
        .split_once('/')
        .with_context(|| format!("repo slug must be owner/name: {slug:?}"))?;
    for part in [owner, name] {
        ensure!(!part.is_empty(), "empty part in repo slug {slug:?}");
        ensure!(
            !part.starts_with('-'),
            "repo slug part must not start with `-`: {slug:?}"
        );
        ensure!(
            part.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "invalid character in repo slug {slug:?}"
        );
    }
    ensure!(
        !slug.contains(".."),
        "repo slug must not contain `..`: {slug:?}"
    );
    Ok(())
}

/// Hermes profile names: lowercase, digits, `_`/`-`, starting alphanumeric.
pub fn validate_profile(profile: &str) -> anyhow::Result<()> {
    ensure!(
        !profile.is_empty() && profile.len() <= 64,
        "invalid profile length"
    );
    let mut chars = profile.chars();
    let first = chars.next().expect("checked non-empty");
    ensure!(
        first.is_ascii_lowercase() || first.is_ascii_digit(),
        "profile must start with a lowercase letter or digit: {profile:?}"
    );
    ensure!(
        profile
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-')),
        "invalid character in profile {profile:?}"
    );
    Ok(())
}

/// Import the plugin's selection: **only** `selectedRepos`, validated.
#[derive(Deserialize)]
struct PluginConfig {
    #[serde(default, rename = "selectedRepos")]
    selected_repos: Vec<String>,
}

pub fn from_plugin_json(text: &str) -> anyhow::Result<Vec<String>> {
    let plugin: PluginConfig =
        serde_json::from_str(text).context("cannot parse the plugin config")?;
    for repo in &plugin.selected_repos {
        validate_repo_slug(repo)?;
    }
    Ok(plugin.selected_repos)
}

/// Write `bytes` to `path` with mode 0600, atomically, refusing symlinks.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "refusing to write through a symlink: {}",
            path.display()
        );
        ensure!(
            meta.file_type().is_file(),
            "refusing to replace a non-file: {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        crate::logging::ensure_state_dir(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("cannot fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

/// Read a file, refusing a symlink at `path`.
///
/// The refusal is **atomic**: `O_NOFOLLOW` makes the open itself fail on a
/// symlink, so there is no window between a `symlink_metadata` check and the
/// read in which the link could be planted (the same TOCTOU class as S5's L4).
pub(crate) fn read_private(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("cannot open {} (symlinks are refused)", path.display()))?;
    ensure!(
        file.metadata()
            .with_context(|| format!("cannot fstat {}", path.display()))?
            .is_file(),
        "refusing to read a non-regular file: {}",
        path.display()
    );
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(text)
}

#[cfg(test)]
mod read_private_tests {
    use super::read_private;

    #[test]
    fn a_symlinked_config_is_refused_at_open_time() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        std::fs::write(&real, "repos = []").unwrap();
        let link = dir.path().join("link.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err = read_private(&link).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("symlink") || text.contains("Too many levels"),
            "the open itself must fail, got: {text}"
        );
    }

    #[test]
    fn a_regular_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "repos = [\"a/b\"]").unwrap();
        assert_eq!(read_private(&path).unwrap(), "repos = [\"a/b\"]");
    }
}
