//! Sanitising untrusted text before it reaches the terminal or a log (S13).
//!
//! GitHub titles, labels and error output are attacker-influenced. `ratatui`
//! filters control characters inside its `Buffer`, but text also reaches the
//! status line, the log and error messages, so the filtering happens here,
//! once, on the way in.

/// Remove C0 controls (except `\n` and `\t`) and DEL, plus C1 controls.
pub fn strip_controls(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            let code = *c as u32;
            match code {
                0x0A | 0x09 => true,         // newline and tab are layout
                0x00..=0x1F | 0x7F => false, // C0 and DEL
                0x80..=0x9F => false,        // C1
                0x2028 | 0x2029 => false,    // line/paragraph separators
                0xFEFF => false,             // zero-width no-break space
                _ => true,
            }
        })
        .collect()
}

/// Remove bidirectional overrides and other invisible direction controls,
/// which can reorder displayed text (Trojan Source).
pub fn strip_bidi(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            !matches!(
                *c as u32,
                0x061C                 // ARABIC LETTER MARK
                | 0x200E | 0x200F      // LRM / RLM
                | 0x202A..=0x202E      // LRE, RLE, PDF, LRO, RLO
                | 0x2066..=0x2069      // LRI, RLI, FSI, PDI
            )
        })
        .collect()
}

/// Redact strings that look like credentials. Deliberately conservative: a
/// false positive only hides text, a false negative leaks a token into a log.
pub fn redact_credentials(input: &str) -> String {
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "sk-",
        "xoxb-",
        "xoxp-",
    ];
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    'outer: while let Some(pos) = rest.find(|c: char| c.is_ascii_alphanumeric()) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        for prefix in PREFIXES {
            if tail.starts_with(prefix) {
                out.push_str("[REDACTED]");
                // Skip the token body: alphanumerics, `_` and `-`.
                let skip = tail
                    .char_indices()
                    .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
                    .map(|(i, _)| i)
                    .unwrap_or(tail.len());
                rest = &tail[skip..];
                continue 'outer;
            }
        }
        // Not a credential: keep the first character and move on.
        let ch = tail.chars().next().expect("pos is a char boundary");
        out.push(ch);
        rest = &tail[ch.len_utf8()..];
    }
    out.push_str(rest);
    // Exact token values, if the environment carries them: a future token
    // format no prefix list knows would otherwise pass through verbatim in
    // an error line. Short values are skipped — replacing a 3-letter string
    // would mangle innocent text for no safety gain.
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(secret) = std::env::var(var) {
            if secret.len() >= 16 {
                out = out.replace(&secret, "[REDACTED]");
            }
        }
    }
    out
}

/// Everything that must happen before untrusted text is displayed or logged.
pub fn sanitize(input: &str) -> String {
    redact_credentials(&strip_bidi(&strip_controls(input)))
}
