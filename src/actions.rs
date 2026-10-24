//! Task 9: actions. This module starts with the safest one — open an item in
//! the browser — because it is the only action that changes nothing locally.
//!
//! S10: `xdg-open` is only ever handed an `https://` URL on `github.com` or
//! `www.github.com`. The URL in the payload is attacker-influenced, so it is
//! validated here, never passed through.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

/// Hosts allowed to reach the browser.
const ALLOWED_HOSTS: &[&str] = &["github.com", "www.github.com"];

/// Returns the URL unchanged when it is safe to open, otherwise the reason.
///
/// Rejected: any scheme but `https`, any other host, credentials in the
/// authority (`https://github.com@evil.example`), an explicit non-443 port, and
/// control characters.
pub fn validate_url(url: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(!url.is_empty(), "empty url");
    // `is_control()` only covers Cc: the line/paragraph separators and the
    // zero-width no-break space are `Cf` and would slip through (U-1). Keep this
    // list identical to `sanitize::strip_controls`.
    anyhow::ensure!(
        !url.chars()
            .any(|c| c.is_control() || c == ' ' || matches!(c as u32, 0x2028 | 0x2029 | 0xFEFF)),
        "url contains control, separator or space characters"
    );
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| anyhow::anyhow!("only https:// is allowed, got {url:?}"))?;

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];

    anyhow::ensure!(
        !authority.contains('@'),
        "credentials in the url are refused: {url:?}"
    );
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port {
        anyhow::ensure!(port == "443", "unexpected port in {url:?}");
    }
    anyhow::ensure!(
        ALLOWED_HOSTS.contains(&host),
        "refusing to open a non-GitHub host: {host:?}"
    );
    Ok(url)
}

/// The argv for `xdg-open`: one element, never a shell string.
pub fn open_args(url: &str) -> anyhow::Result<Vec<OsString>> {
    let url = validate_url(url)?;
    Ok(vec![OsString::from(url)])
}

/// Launch the URL with the resolved launcher. The child is detached (the TUI
/// must not wait) and inherits neither stdin nor our terminal output.
///
/// The pid is returned so a test can prove the child is reaped: spawning without
/// waiting would leave one zombie per press (B-2).
pub fn open_in_browser(program: &Path, url: &str) -> anyhow::Result<u32> {
    anyhow::ensure!(
        program.is_absolute(),
        "browser launcher must be an absolute path: {}",
        program.display()
    );
    let args = open_args(url)?;
    let mut cmd = Command::new(program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Same ETXTBSY window `run_pipe` handles (F-1): harmless in production, but
    // a test that writes a launcher and runs it immediately hits it.
    let mut child = crate::gh::spawn_retry(&mut cmd, program)?;
    let pid = child.id();
    // Reap it. `xdg-open` exits immediately and nothing else waits for it, so
    // without this thread every `o` would leave a zombie until the app exits.
    // Not `SIGCHLD = SIG_IGN`: that is process-wide and would break `try_wait`
    // in `run_pipe`.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::validate_url;

    #[test]
    fn github_https_urls_pass() {
        for url in [
            "https://github.com/owner/repo/pull/12",
            "https://www.github.com/owner/repo/issues/3",
            "https://github.com/o/r",
            "https://github.com:443/o/r/pull/1",
        ] {
            assert!(validate_url(url).is_ok(), "{url} should be allowed");
        }
    }

    #[test]
    fn other_schemes_and_hosts_are_refused() {
        for url in [
            "http://github.com/o/r",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://evil.example/o/r",
            "https://github.com.evil.example/o/r",
            "https://github.co/o/r",
            "",
        ] {
            assert!(validate_url(url).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn credentials_and_odd_ports_are_refused() {
        for url in [
            "https://github.com@evil.example/o/r",
            "https://user:pass@github.com/o/r",
            "https://github.com:8443/o/r",
        ] {
            assert!(validate_url(url).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn unicode_separators_are_refused_too() {
        for url in [
            "https://github.com/o/r\u{2028}",
            "https://github.com/o/r\u{2029}",
            "https://github.com/o/r\u{feff}",
        ] {
            assert!(validate_url(url).is_err(), "{url:?} must be refused");
        }
    }

    #[test]
    fn control_characters_never_reach_the_launcher() {
        for url in [
            "https://github.com/o/r\u{0}",
            "https://github.com/o/r\n",
            "https://github.com/o/r x",
        ] {
            assert!(validate_url(url).is_err(), "{url:?} must be refused");
        }
    }

    #[test]
    fn the_argv_is_a_single_element() {
        let url = "https://github.com/o/r/pull/7";
        let args = super::open_args(url).unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], url);
    }
}

// ---------------------------------------------------------------------------
// Task 9: actions that write. `argv_for` is the only place that turns an action
// into a gh argv, and it validates the argv it just built against a positive
// per-action allowlist (docs/task9-allowlist.md).
// ---------------------------------------------------------------------------

use crate::binaries::Binaries;
use crate::gh::{Argv, gh};

/// A repository slug that has already been validated. The field is private on
/// purpose: the type constrains only if `parse` is the only way in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSlug(String);

impl RepoSlug {
    /// The single door: every slug in the program passes through here.
    pub fn parse(slug: &str) -> anyhow::Result<Self> {
        crate::config::validate_repo_slug(slug)?;
        Ok(Self(slug.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a pull request is merged. Exactly one of these is emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn flag(self) -> &'static str {
        match self {
            MergeMethod::Merge => "--merge",
            MergeMethod::Squash => "--squash",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

/// The actions the app can perform. Adding a variant is a compile error until it
/// declares its flag list and its tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Merge {
        repo: RepoSlug,
        number: u64,
        method: MergeMethod,
        delete_branch: bool,
    },
    Comment {
        repo: RepoSlug,
        number: u64,
        body: String,
    },
    Close {
        repo: RepoSlug,
        number: u64,
        reason: Option<String>,
        comment: Option<String>,
    },
    Checkout {
        repo: RepoSlug,
        number: u64,
    },
}

/// A flag is allowlisted; a value is opaque. The distinction is a **type**, so a
/// value can never be refused as a flag, nor accepted as one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Flag(&'static str),
    Value(OsString),
}

const MERGE_FLAGS: &[&str] = &["--merge", "--squash", "--rebase", "--delete-branch"];
const COMMENT_FLAGS: &[&str] = &["--body"];
const CLOSE_FLAGS: &[&str] = &["--reason", "--comment"];
const CHECKOUT_FLAGS: &[&str] = &[];

/// Flags that must never appear in any action list.
pub const FORBIDDEN_ACTION_FLAGS: &[&str] = &[
    "--admin",
    "--force",
    "-f",
    "--auto",
    "--disable-auto",
    "--body-file",
    "-F",
    "--editor",
    "-e",
    "--web",
    "-w",
    "--attach",
    "--delete-last",
    "--edit-last",
    "--worktree",
    "--author-email",
    "-A",
    "--subject",
    "-t",
    "--match-head-commit",
    "--recurse-submodules",
];

fn flags_for(action: &Action) -> &'static [&'static str] {
    match action {
        Action::Merge { .. } => MERGE_FLAGS,
        Action::Comment { .. } => COMMENT_FLAGS,
        Action::Close { .. } => CLOSE_FLAGS,
        Action::Checkout { .. } => CHECKOUT_FLAGS,
    }
}

fn value(text: impl Into<OsString>) -> Token {
    Token::Value(text.into())
}

fn tokens_for(action: &Action) -> anyhow::Result<Vec<Token>> {
    let tokens = match action {
        Action::Merge {
            number,
            method,
            delete_branch,
            ..
        } => {
            let mut tokens = vec![
                value("pr"),
                value("merge"),
                value(number.to_string()),
                Token::Flag(method.flag()),
            ];
            if *delete_branch {
                tokens.push(Token::Flag("--delete-branch"));
            }
            tokens
        }
        Action::Comment { number, body, .. } => {
            ensure_no_nul(body)?;
            vec![
                value("pr"),
                value("comment"),
                value(number.to_string()),
                Token::Flag("--body"),
                value(body.clone()),
            ]
        }
        Action::Close {
            number,
            reason,
            comment,
            ..
        } => {
            let mut tokens = vec![value("pr"), value("close"), value(number.to_string())];
            if let Some(reason) = reason {
                ensure_no_nul(reason)?;
                tokens.push(Token::Flag("--reason"));
                tokens.push(value(reason.clone()));
            }
            if let Some(comment) = comment {
                ensure_no_nul(comment)?;
                tokens.push(Token::Flag("--comment"));
                tokens.push(value(comment.clone()));
            }
            tokens
        }
        Action::Checkout { number, .. } => {
            vec![value("pr"), value("checkout"), value(number.to_string())]
        }
    };
    Ok(tokens)
}

/// A NUL cannot appear in an argv element; anything else is legal, including a
/// leading `-` (it travels as a value bound to its flag).
fn ensure_no_nul(text: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!text.contains('\0'), "text contains a NUL byte");
    Ok(())
}

fn ensure_allowlisted(tokens: &[Token], allowed: &[&str]) -> anyhow::Result<()> {
    for token in tokens {
        if let Token::Flag(flag) = token {
            anyhow::ensure!(
                allowed.contains(flag),
                "flag {flag:?} is not allowlisted for this action"
            );
            anyhow::ensure!(
                !FORBIDDEN_ACTION_FLAGS.contains(flag),
                "flag {flag:?} is forbidden outright"
            );
        }
    }
    Ok(())
}

/// The single translation point: an action becomes a `gh` argv here, and the
/// argv is checked against the action's allowlist before it leaves.
pub fn argv_for(binaries: &Binaries, action: &Action) -> anyhow::Result<Argv> {
    let tokens = tokens_for(action)?;
    // Only the action's own tokens go through the allowlist: the `--repo`
    // prefix is not in `flags_for(action)` because the **type** produces it,
    // not because we skip it.
    ensure_allowlisted(&tokens, flags_for(action))?;

    let slug = match action {
        Action::Merge { repo, .. }
        | Action::Comment { repo, .. }
        | Action::Close { repo, .. }
        | Action::Checkout { repo, .. } => repo,
    };

    let cwd = std::env::current_dir()?;
    let mut args: Vec<OsString> = vec![OsString::from("--repo"), OsString::from(slug.as_str())];
    args.extend(tokens.into_iter().map(|token| match token {
        Token::Flag(flag) => OsString::from(flag),
        Token::Value(text) => text,
    }));
    Ok(gh(binaries, &cwd, args))
}

/// Exposed for tests: the flags an action may emit.
pub fn allowed_flags(action: &Action) -> &'static [&'static str] {
    flags_for(action)
}

/// The flags this action actually emits, values excluded. Asking the type which
/// tokens are flags is the only way to tell them apart — a prefix check cannot.
pub fn emitted_flags(action: &Action) -> anyhow::Result<Vec<&'static str>> {
    Ok(tokens_for(action)?
        .into_iter()
        .filter_map(|token| match token {
            Token::Flag(flag) => Some(flag),
            Token::Value(_) => None,
        })
        .collect())
}

#[cfg(test)]
mod action_tokens {
    use super::*;

    #[test]
    fn a_forbidden_flag_cannot_pass_the_allowlist() {
        let tokens = vec![Token::Flag("--admin")];
        let err = ensure_allowlisted(&tokens, MERGE_FLAGS).unwrap_err();
        assert!(err.to_string().contains("not allowlisted"), "got {err}");
    }

    #[test]
    fn a_value_is_never_inspected_as_a_flag() {
        let tokens = vec![
            Token::Flag("--body"),
            Token::Value(OsString::from("-admin")),
        ];
        // `--body` is allowlisted for comments and the value is not inspected.
        ensure_allowlisted(&tokens, COMMENT_FLAGS).expect("a value is not a flag");
    }

    #[test]
    fn no_action_list_contains_a_forbidden_flag() {
        for flags in [MERGE_FLAGS, COMMENT_FLAGS, CLOSE_FLAGS, CHECKOUT_FLAGS] {
            for flag in flags {
                assert!(
                    !FORBIDDEN_ACTION_FLAGS.contains(flag),
                    "{flag} must not be allowlisted"
                );
            }
        }
    }
}
