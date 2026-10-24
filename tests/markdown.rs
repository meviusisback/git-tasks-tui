//! Task 8: markdown for the detail pane.

use gtasks::markdown::{Style, line_text, render};

fn text(markdown: &str, width: usize) -> Vec<String> {
    render(markdown, width).iter().map(line_text).collect()
}

#[test]
fn headings_become_bold_text_without_the_hashes() {
    let lines = render("## Summary", 40);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "Summary");
    assert_eq!(lines[0][0].style, Style::Bold);
    assert!(!line_text(&lines[0]).contains('#'));
}

#[test]
fn emphasis_and_inline_code_keep_their_style() {
    let lines = render("This is **important** and *subtle* with `code()`.", 80);
    let spans = &lines[0];
    assert!(
        spans
            .iter()
            .any(|s| s.text == "important" && s.style == Style::Bold)
    );
    assert!(
        spans
            .iter()
            .any(|s| s.text == "subtle" && s.style == Style::Italic)
    );
    assert!(
        spans
            .iter()
            .any(|s| s.text == "code()" && s.style == Style::Code)
    );
    assert!(!line_text(&lines[0]).contains('*'));
}

#[test]
fn links_show_the_text_and_the_target() {
    let lines = render("See [the docs](https://example.com/x).", 80);
    let joined = line_text(&lines[0]);
    assert!(joined.contains("the docs"), "{joined}");
    assert!(joined.contains("https://example.com/x"), "{joined}");
}

#[test]
fn code_blocks_are_preserved_verbatim() {
    let source = "```\nlet x = 1;\nif x < 2 { ok }\n```";
    let lines = render(source, 80);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert!(texts.iter().any(|t| t == "let x = 1;"), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("if x < 2")), "{texts:?}");
    assert!(lines.iter().flatten().all(|s| s.style == Style::Code));
}

#[test]
fn long_paragraphs_wrap_at_the_requested_width() {
    let paragraph = "word ".repeat(30);
    let lines = render(paragraph.trim(), 20);
    assert!(lines.len() > 1, "must wrap");
    for line in &lines {
        assert!(
            line_text(line).chars().count() <= 20,
            "line too wide: {:?}",
            line_text(line)
        );
    }
}

#[test]
fn a_word_longer_than_the_line_is_hard_split() {
    let lines = render("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 10);
    assert!(lines.len() >= 3);
    for line in &lines {
        assert!(line_text(line).chars().count() <= 10);
    }
}

#[test]
fn control_characters_in_a_body_are_stripped() {
    // A PR body is attacker-influenced: no escape sequences reach the buffer.
    let lines = render("title\u{1b}[31m \u{202e}body", 80);
    let joined = line_text(&lines[0]);
    assert!(!joined.contains('\u{1b}'), "{joined:?}");
    assert!(!joined.contains('\u{202e}'), "{joined:?}");
    assert!(joined.contains("title"), "{joined:?}");
}

#[test]
fn lists_render_one_item_per_line() {
    let lines = render("- first\n- second\n- third", 40);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(texts[0].contains("first"));
    assert!(texts[2].contains("third"));
}
