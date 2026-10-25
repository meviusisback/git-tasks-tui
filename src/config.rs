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
    /// Profile the agent handoff (`g`) passes to `hermes -p`. Validated like
    /// every profile: lowercase, digits, `_`/`-`, starting alphanumeric.
    pub agent_profile: String,
    /// Directory holding local clones (`<clone_root>/<repo-name>`). `None`
    /// means the agent cannot be placed: refusing beats guessing.
    pub clone_root: Option<PathBuf>,
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
            agent_profile: "default".to_string(),
            clone_root: None,
        }
    }
}

impl Config {
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
                format!("cannot read config {}: {reason}", path.display())
            }
            ConfigProblem::Invalid { path, reason } => {
                format!("invalid config {}: {reason}", path.display())
            }
        }
    }
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
