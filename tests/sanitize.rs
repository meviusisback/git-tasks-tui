//! S13: control/bidi stripping and credential redaction before display.

use gtasks::sanitize::{redact_credentials, sanitize, strip_bidi, strip_controls};

#[test]
fn controls_are_removed_but_layout_is_kept() {
    // ESC (the start of every terminal escape sequence) goes; \n and \t stay.
    assert_eq!(strip_controls("a\u{1b}[31mb\nc\td"), "a[31mb\nc\td");
    assert_eq!(strip_controls("bell\u{07}del\u{7f}"), "belldel");
    assert_eq!(strip_controls("c1\u{9b}done"), "c1done");
}

#[test]
fn bidi_overrides_are_removed() {
    // Trojan Source: the RLO would reorder the rendered line.
    let trojan = "if (ok) { // \u{202e} } else {";
    assert!(!strip_bidi(trojan).contains('\u{202e}'));
    assert_eq!(strip_bidi("abc\u{200f}def"), "abcdef");
    assert_eq!(strip_bidi("x\u{2066}y\u{2069}z"), "xyz");
}

#[test]
fn credential_shapes_are_redacted() {
    let text = "token ghp_ABCdef0123456789 and sk-proj-XYZ, plus github_pat_11ABC";
    let out = redact_credentials(text);
    assert!(
        !out.contains("ghp_"),
        "github token must not survive: {out}"
    );
    assert!(!out.contains("sk-proj"), "api key must not survive: {out}");
    assert!(!out.contains("github_pat_"), "pat must not survive: {out}");
    assert_eq!(out.matches("[REDACTED]").count(), 3, "got: {out}");
    assert!(out.starts_with("token"), "surrounding text stays: {out}");
}

#[test]
fn ordinary_text_is_untouched() {
    let text = "Fix: crash on start — naïve café ✓";
    assert_eq!(sanitize(text), text);
}

#[test]
fn sanitize_composes_all_three_steps() {
    let evil = "title\u{1b}]52;c;x\u{07} \u{202e}ghp_deadbeef";
    let out = sanitize(evil);
    assert!(!out.contains('\u{1b}'), "escape must go");
    assert!(!out.contains('\u{202e}'), "bidi must go");
    assert!(out.contains("[REDACTED]"), "token must be redacted");
    assert!(out.starts_with("title"), "prefix stays: {out}");
}

#[test]
fn invisible_separators_are_removed() {
    // U+2028/U+2029 break lines in some renderers and U+FEFF hides a zero-width
    // character: neither belongs in a title or a body.
    let text = "a\u{2028}b\u{2029}c\u{feff}d";
    assert_eq!(strip_controls(text), "abcd");
}
