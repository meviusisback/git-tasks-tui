//! L-1: the log is a sink like any other — it must not store tokens.

use std::io::Write;

use gtasks::logging::SanitisingWriter;

fn through(text: &str) -> String {
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = SanitisingWriter::new(&mut sink);
        writer.write_all(text.as_bytes()).unwrap();
        writer.flush().unwrap();
    }
    String::from_utf8(sink).unwrap()
}

#[test]
fn a_token_in_an_error_message_is_redacted_before_it_reaches_the_file() {
    let line = "2026-10-24T10:00:00Z WARN gtasks: gh failed: HTTP 401: Bad credentials token: gho_16C7eABCdef0123456789\n";
    let written = through(line);

    assert!(
        !written.contains("gho_16C7e"),
        "token must not be stored: {written}"
    );
    assert!(written.contains("[REDACTED]"), "got {written}");
    assert!(written.contains("HTTP 401"), "the diagnostic survives");
}

#[test]
fn escape_sequences_do_not_reach_the_log() {
    let written = through("error: \u{1b}[31mred\u{1b}[0m title\u{202e}\n");
    assert!(!written.contains('\u{1b}'), "got {written:?}");
    assert!(!written.contains('\u{202e}'), "got {written:?}");
    assert!(written.contains("red"), "the text survives");
}

#[test]
fn ordinary_lines_are_untouched() {
    let line = "2026-10-24T10:00:00Z INFO gtasks: gtasks starting version=\"0.1.0\"\n";
    assert_eq!(through(line), line);
}

/// L-2 (CWE-117): a newline in untrusted text must not start a new record.
#[test]
fn a_newline_in_a_message_cannot_forge_a_record() {
    let forged =
        "gh failed\n2026-10-24T12:00:00Z  INFO gtasks: action merge committed successfully\n";
    let written = through(forged);

    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 1, "one write, one record: {written:?}");
    assert!(
        lines[0].contains("action merge committed successfully"),
        "the forged text stays on the first line: {written:?}"
    );
    assert!(
        !lines[0].contains('\n') && written.matches('\n').count() == 1,
        "exactly one real newline: {written:?}"
    );
}

/// L-3: `Display` and `write_fmt` may split a token across two `write` calls,
/// so sanitising per write would let half of it through.
#[test]
fn a_token_split_across_writes_is_still_redacted() {
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = SanitisingWriter::new(&mut sink);
        writer.write_all(b"token: ghp_").unwrap();
        writer
            .write_all(b"16C7e42F292c6912E7710c838347Ae178B4a\n")
            .unwrap();
        writer.flush().unwrap();
    }
    let written = String::from_utf8(sink).unwrap();

    assert!(
        !written.contains("ghp_16C7e"),
        "the split token must not survive: {written:?}"
    );
    // The tail matters as much as the prefix: sanitising per write redacts the
    // first fragment and leaves the rest, which is the (L-3) leak.
    assert!(
        !written.contains("16C7e42F292c6912E7710c838347Ae178B4a"),
        "no fragment of the token may survive: {written:?}"
    );
    assert!(written.contains("[REDACTED]"), "got {written:?}");
    assert!(
        written.trim_end().ends_with("[REDACTED]"),
        "the whole token is redacted, not half of it: {written:?}"
    );
}

/// The cap is a second cut point: flushing at exactly `MAX_PENDING` would split
/// a token that straddles it and leave the body with no prefix to match.
#[test]
fn the_cap_does_not_cut_through_a_token() {
    const CAP: usize = 64 * 1024;
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = SanitisingWriter::new(&mut sink);
        // No newline for the whole cap, so the early flush triggers…
        writer.write_all(&vec![b'a'; CAP - 1]).unwrap();
        // …and the token arrives in two pieces that straddle the cut.
        writer.write_all(b"ghp_16C7e42F").unwrap();
        writer.write_all(b"292c6912E7710c838347Ae178B4a\n").unwrap();
        writer.flush().unwrap();
    }
    let written = String::from_utf8(sink).unwrap();

    assert!(
        !written.contains("16C7e42F292c6912E7710c838347Ae178B4a"),
        "no fragment of the token may survive the cap: …{}",
        &written[written.len().saturating_sub(80)..]
    );
    assert!(
        !written.contains("E7710c838347Ae178B4a"),
        "the tail must not leak: …{}",
        &written[written.len().saturating_sub(80)..]
    );
}
