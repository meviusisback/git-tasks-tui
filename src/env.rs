//! S3 — per-child environment allowlists.
//!
//! Every child starts from `env_clear()` plus exactly the variables built
//! here; nothing is ever inherited implicitly. The `_from` cores take the
//! parent environment as a parameter so tests can simulate a dirty parent
//! (a planted `GIT_DIR` or `GIT_CONFIG_COUNT`) without mutating the process
//! environment. The plain wrappers read `std::env` (production path).

use std::collections::HashMap;
use std::ffi::OsString;

pub type EnvMap = Vec<(OsString, OsString)>;

fn take(parent: &HashMap<String, String>, key: &str, out: &mut EnvMap) {
    if let Some(value) = parent.get(key) {
        out.push((OsString::from(key), OsString::from(value)));
    }
}

/// Base allowlist: `PATH`, `HOME`, `LANG`/`LC_ALL` and `TERM`, each only if
/// present in the parent (an absent variable cannot be invented).
pub fn base_env_from(parent: &HashMap<String, String>) -> EnvMap {
    let mut out = EnvMap::new();
    for key in ["PATH", "HOME", "LANG", "LC_ALL", "TERM"] {
        take(parent, key, &mut out);
    }
    out
}

/// `gh` child: base + token passthrough (if present) + non-interactive knobs.
pub fn gh_env_from(parent: &HashMap<String, String>) -> EnvMap {
    let mut out = base_env_from(parent);
    for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
        take(parent, key, &mut out);
    }
    for (key, value) in [
        ("GH_PROMPT_DISABLED", "1"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("GH_PAGER", "cat"),
        ("NO_COLOR", "1"),
    ] {
        out.push((OsString::from(key), OsString::from(value)));
    }
    out
}

/// `git` child: base + `GIT_TERMINAL_PROMPT=0`, and **no** `GIT_*` from the
/// parent — in particular no `GIT_DIR`, `GIT_WORK_TREE`, `GIT_CONFIG_COUNT`
/// or `GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` hijack.
pub fn git_env_from(parent: &HashMap<String, String>) -> EnvMap {
    let mut out = base_env_from(parent);
    out.push((OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0")));
    debug_assert!(
        out.iter().all(|(key, _)| key != "GIT_DIR"),
        "GIT_* must never leak into a git child"
    );
    out
}

/// Agent child: base only — no GH tokens, no `GIT_*`.
pub fn agent_env_from(parent: &HashMap<String, String>) -> EnvMap {
    base_env_from(parent)
}

fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// Production wrappers: build from the real process environment.
pub fn base_env() -> EnvMap {
    base_env_from(&process_env())
}
pub fn gh_env() -> EnvMap {
    gh_env_from(&process_env())
}
pub fn git_env() -> EnvMap {
    git_env_from(&process_env())
}
pub fn agent_env() -> EnvMap {
    agent_env_from(&process_env())
}

/// True when the environment contains no `GIT_*` variable. Tests use it to
/// assert the allowlist cannot leak one.
pub fn has_no_git_vars(env: &[(OsString, OsString)]) -> bool {
    !env.iter()
        .any(|(key, _)| key.to_string_lossy().starts_with("GIT_"))
}
