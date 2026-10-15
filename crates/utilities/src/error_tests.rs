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
        text.contains("  | \t^^"),
        "caret pad should keep the leading tab:\n{text}"
    );
}

#[test]
fn column_is_one_based() {
    let src = "ab\ncd";
    let (line_no, col, line, _) = locate(src, 4);
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

#[test]
fn spanless_diagnostic_renders_without_a_location() {
    // `Span::at(0)` is the "no location" sentinel. Rendering it as `1:1` would
    // blame the first line of the file, which is what a runtime fault (the IR
    // carries no spans) used to do in the shell.
    let src = "$ @mod M\n$ x = 1\n";
    let d = Diagnostic::error(Code::RuntimeFault, Span::at(0), 0, "division by zero");
    let text = d.render(src, "<repl>");
    assert!(text.contains("  --> <repl>\n"), "{text}");
    assert!(!text.contains(":1:1"), "{text}");
    assert!(!text.contains('^'), "{text}");
    assert!(!text.contains("$ @mod M"), "{text}");
}

#[test]
fn spanless_display_omits_the_line() {
    let d = Diagnostic::error(Code::RuntimeFault, Span::at(0), 0, "boom");
    assert_eq!(d.to_string(), "RUNTIME_FAULT: boom");
    let located = Diagnostic::error(Code::RuntimeFault, Span::new(3, 4), 2, "boom");
    assert_eq!(located.to_string(), "RUNTIME_FAULT: boom (line 2)");
}

#[test]
fn a_located_frame_still_renders_when_the_root_has_no_span() {
    let src = "ab\ncd\n";
    let d = Diagnostic::error(Code::TypeMismatch, Span::at(0), 0, "root").context(
        Code::UnexpectedToken,
        Span::new(3, 5),
        2,
        "while checking",
    );
    let text = d.render(src, "t.thx");
    assert!(text.contains("error[TYPE_MISMATCH]: root\n  --> t.thx\n"), "{text}");
    assert!(
        text.contains("note [UNEXPECTED_TOKEN]: while checking\n  --> t.thx:2:1\n"),
        "{text}"
    );
    assert!(text.contains("2 | cd\n  | ^^\n"), "{text}");
}

#[test]
fn a_multi_line_span_underlines_each_line() {
    // A span may run past the newline (an application spanning two lines). Each
    // line gets its own underline, from its first non-blank character, so no
    // row outruns the line above it.
    let src = "\"aaa\"\n\t\"bbb\"\n";
    let d = Diagnostic::error(Code::TypeMismatch, Span::new(0, 12), 1, "not a function");
    let text = d.render(src, "t.thx");
    assert!(text.contains("1 | \"aaa\"\n  | ^^^^^\n"), "{text}");
    assert!(text.contains("2 | \t\"bbb\"\n  | \t^^^^^\n"), "{text}");
}

#[test]
fn a_long_span_elides_its_middle() {
    let src = "a\nb\nc\nd\ne\nf\n";
    let d = Diagnostic::error(Code::TypeMismatch, Span::new(0, 11), 1, "long");
    let text = d.render(src, "t.thx");
    assert!(text.contains("3 | c\n"), "{text}");
    assert!(!text.contains("4 | d\n"), "{text}");
    assert!(text.contains("  ...\n6 | f\n"), "{text}");
}

#[test]
fn end_of_input_points_after_the_last_character() {
    // A parser error at EOF carries a point span past the trailing newline. It
    // must caret where the text stopped, not an empty line after it.
    let src = "x = 2.\n\n";
    let d = Diagnostic::error(Code::UnexpectedToken, Span::at(src.len()), 3, "eof");
    let text = d.render(src, "t.thx");
    assert!(text.contains("--> t.thx:1:7\n"), "{text}");
    assert!(text.contains("1 | x = 2.\n  |       ^\n"), "{text}");
}

#[test]
fn a_label_underlines_a_secondary_span() {
    let src = "f (\"s\" : @int)\n";
    let d = Diagnostic::error(Code::TypeMismatch, Span::new(3, 6), 1, "mismatch")
        .with_label(Span::new(9, 13), "expected because of this");
    let text = d.render(src, "t.thx");
    assert!(text.contains("  |    ^^^\n"), "{text}");
    assert!(text.contains("  |          ---- expected because of this\n"), "{text}");
}

#[test]
fn asides_keep_their_kind_and_order() {
    let src = "x\n";
    let d = diag!(
        Code::TypeMismatch, Span::new(0, 1), 1, "bad";
        note: "why {}", 1;
        help: "fix {}", 2
    );
    let text = d.render(src, "t.thx");
    assert!(text.ends_with("note: why 1\nhelp: fix 2\n"), "{text}");
    assert!(!d.root().msg.contains("fix"), "{text}");
}

#[test]
fn closest_suggests_only_plausible_typos() {
    assert_eq!(closest("prinln", ["print", "println", "map"]), Some("println"));
    assert_eq!(closest("Mytype", ["MyTyp", "Other"]), Some("MyTyp"));
    assert_eq!(closest("xyz", ["println", "map"]), None);
    assert_eq!(closest("map", ["map"]), None);
    assert_eq!(closest("y", ["%", "x"]), None);
    assert_eq!(closest("y", ["Y"]), Some("Y"));
}

#[test]
fn caret_counts_characters_not_bytes() {
    // The span is a byte range; the caret row is characters, so a multi-byte
    // span must not draw one caret per byte.
    let src = "\"héllo\"\n";
    let d = Diagnostic::error(Code::TypeMismatch, Span::new(0, 8), 1, "bad");
    let text = d.render(src, "t.thx");
    assert!(text.contains("  | ^^^^^^^\n"), "{text}");
}
