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
pub fn open_in_browser(program: &Path, url: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        program.is_absolute(),
        "browser launcher must be an absolute path: {}",
        program.display()
    );
    let args = open_args(url)?;
    Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| anyhow::anyhow!("cannot run {}: {err}", program.display()))?;
    Ok(())
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
