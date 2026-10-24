//! Per-child environment allowlists (S3).
//!
//! Children are spawned with `env_clear()`, so the only variables they see are
//! the ones built here. `gh` gets its token; `git` and the agent never do, and
//! no `GIT_*` variable is ever inherited (that is how a hostile `core.hooksPath`
//! or `GIT_DIR` would otherwise be injected).

use std::ffi::OsString;
use std::path::Path;

/// Which child the environment is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    /// `gh`: needs its token and the non-interactive knobs.
    Gh,
    /// `git`: no token, prompt disabled.
    Git,
    /// The agent (`hermes`): neither token nor git knobs.
    Agent,
}

/// Build the allowlisted environment for `child`.
///
/// `path` is the `PATH` to expose: callers pass the directories holding the
/// verified absolute binaries, so `#!/usr/bin/env` interpreters still resolve
/// without inheriting the user's full `PATH`.
pub fn env_for(child: Child, path: &Path) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = Vec::new();

    out.push(("PATH".into(), path.as_os_str().to_owned()));
    for key in ["HOME", "LANG", "LC_ALL", "TERM"] {
        if let Some(value) = std::env::var_os(key) {
            out.push((key.into(), value));
        }
    }

    match child {
        Child::Gh => {
            for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
                if let Some(value) = std::env::var_os(key) {
                    out.push((key.into(), value));
                }
            }
            out.push(("GH_PROMPT_DISABLED".into(), "1".into()));
            out.push(("GH_NO_UPDATE_NOTIFIER".into(), "1".into()));
            out.push(("GH_PAGER".into(), "cat".into()));
            out.push(("NO_COLOR".into(), "1".into()));
        }
        Child::Git => {
            out.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
        }
        Child::Agent => {}
    }

    out
}

/// True when the environment contains no `GIT_*` variable. Tests use it to
/// assert the allowlist cannot leak one.
pub fn has_no_git_vars(env: &[(OsString, OsString)]) -> bool {
    !env.iter()
        .any(|(key, _)| key.to_string_lossy().starts_with("GIT_"))
}
