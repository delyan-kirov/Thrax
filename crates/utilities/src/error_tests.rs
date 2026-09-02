use super::*;

#[test]
fn root_is_first_frame() {
    let d = Diagnostic::error(Code::UnknownSymbol, Span::new(2, 3), 1, "bad").context(
        Code::UnexpectedToken,
        Span::new(0, 3),
        1,
        "while parsing",
    );
    assert_eq!(d.root().code, Code::UnknownSymbol);
    assert_eq!(d.frames().len(), 2);
}

#[test]
fn caret_lands_under_the_span() {
    let src = "let x = ?\n";
    let d = Diagnostic::error(
        Code::UnknownSymbol,
        Span::new(8, 9),
        1,
        "unknown symbol '?'",
    );
    let text = d.render(src, "test.thx");
    assert!(text.contains("UNKNOWN_SYMBOL"));
    assert!(
        text.contains("        ^"),
        "caret should sit under column 9:\n{text}"
    );
}

#[test]
fn caret_pad_preserves_leading_tabs() {
    // A tab-indented line: the caret row must repeat the tab so both rows hit
    // the same tab stop and the caret still lands under the span.
    let src = "\tab\n";
    let d = Diagnostic::error(Code::UnknownSymbol, Span::new(1, 3), 1, "bad");
    let text = d.render(src, "test.thx");
    assert!(
        text.contains("   | \t^^"),
        "caret pad should keep the leading tab:\n{text}"
    );
}

#[test]
fn column_is_one_based() {
    let src = "ab\ncd";
    let (line_no, col, line) = locate(src, 4);
    assert_eq!(line_no, 2);
    assert_eq!(col, 2);
    assert_eq!(line, "cd");
}

#[test]
fn note_renders_as_trailing_line() {
    let src = "let x = ?\n";
    let d = diag!(
        Code::UnknownSymbol, Span::new(8, 9), 1, "unknown symbol {}", "'?'";
        note: "remove the {} character", "stray"
    );
    let text = d.render(src, "test.thx");
    let (body, last) = text.trim_end().rsplit_once('\n').unwrap();
    assert!(body.contains("unknown symbol '?'"));
    assert_eq!(last, "note: remove the stray character");
}

#[test]
fn diag_without_note_has_no_note_line() {
    let src = "x\n";
    let d = diag!(Code::UnknownSymbol, Span::new(0, 1), 1, "bad {}", 1);
    assert!(!d.render(src, "t.thx").contains("note:"));
}
