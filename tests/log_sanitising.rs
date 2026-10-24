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

/// L-2: untrusted text carrying a newline must not be able to forge a record.
#[test]
fn a_newline_in_a_message_cannot_forge_a_record() {
    let written = through(
        "gh failed\n2026-10-24T12:00:00Z  INFO gtasks: action merge committed successfully\n",
    );

    assert_eq!(
        written.lines().count(),
        1,
        "the payload must stay on one record: {written:?}"
    );
    assert!(
        written.contains("gh failed") && written.contains("action merge committed"),
        "both parts survive on the same line: {written:?}"
    );
    assert!(
        !written.contains("\n2026-10-24T12:00:00Z"),
        "the injected text must not start a line: {written:?}"
    );
}

/// L-3: sanitising per `write` is not atomic — a `Display` impl may emit a token in
/// several calls. A line is the unit, so the split must not matter.
#[test]
fn a_token_split_across_writes_is_still_redacted() {
    let token = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = SanitisingWriter::new(&mut sink);
        // the boundary falls inside the token prefix
        writer.write_all(b"token: gh").unwrap();
        writer
            .write_all(b"p_16C7e42F292c6912E7710c838347Ae178B4a\n")
            .unwrap();
        writer.flush().unwrap();
    }
    let written = String::from_utf8(sink).unwrap();

    assert!(
        !written.contains(token),
        "a split token must not reach the file: {written:?}"
    );
    assert!(written.contains("[REDACTED]"), "got {written:?}");
}
