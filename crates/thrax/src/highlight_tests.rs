use super::*;

fn classes(src: &str) -> Vec<(&str, Class)> {
    spans(src).into_iter().map(|(s, c)| (&src[s.start..s.end], c)).collect()
}

#[test]
fn classifies_tokens() {
    assert_eq!(
        classes("_= let x : @Int = 1 in Some \"hi\" # done"),
        [
            ("_", Class::Variable),
            ("=", Class::Operator),
            ("let", Class::Keyword),
            ("x", Class::Variable),
            (":", Class::Operator),
            ("@Int", Class::Type),
            ("=", Class::Operator),
            ("1", Class::Number),
            ("in", Class::Keyword),
            ("Some", Class::Type),
            ("\"hi\"", Class::String),
            ("# done", Class::Comment),
        ]
    );
}

#[test]
fn unfinished_input_still_highlights() {
    assert_eq!(classes("f \"abc"), [("f", Class::Variable), ("\"abc", Class::String)]);
}

#[test]
fn ansi_keeps_the_text() {
    let src = "let s = \"a\nb\" in s";
    let painted = ansi(src);
    let mut plain = String::new();
    let mut esc = false;
    for c in painted.chars() {
        match c {
            '\x1b' => esc = true,
            'm' if esc => esc = false,
            _ if esc => {}
            _ => plain.push(c),
        }
    }
    assert_eq!(plain, src);
    assert!(painted.contains("\x1b[35mlet\x1b[0m"));
    assert!(!painted.contains("\x1b[32m\"a\n"), "color is closed before each newline");
}

