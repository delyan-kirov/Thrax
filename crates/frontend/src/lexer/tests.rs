use super::*;

fn kinds(src: &str) -> Vec<Kind<'_>> {
    // Leak a boxed arena so the returned kinds can borrow it for the test.
    let arena: &'static Arena = Box::leak(Box::new(Arena::new()));
    Lexer::tokenize(src, arena)
        .expect("lex ok")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn global_declaration() {
    let ks = kinds("$fib : Int -> Int = \\n = n");
    assert_eq!(
        ks,
        vec![
            Kind::Dollar,
            Kind::Word,
            Kind::Colon,
            Kind::Word,
            Kind::Arrow,
            Kind::Word,
            Kind::Eq,
            Kind::Lambda,
            Kind::Word,
            Kind::Eq,
            Kind::Word,
            Kind::Eof,
        ]
    );
}

#[test]
fn operators_maximal_munch() {
    assert_eq!(
        kinds("a ?= b :: c ++ d |> e"),
        vec![
            Kind::Word,
            Kind::Op,
            Kind::Word,
            Kind::Op,
            Kind::Word,
            Kind::Op,
            Kind::Word,
            Kind::Op,
            Kind::Word,
            Kind::Eof,
        ]
    );
}

#[test]
fn numbers_ints_reals_radix() {
    let arena = Arena::new();
    let toks = Lexer::tokenize("0xFF 0b1010 1_000 3.5 2e3", &arena).unwrap();
    let vals: Vec<Kind> = toks.into_iter().map(|t| t.kind).collect();
    assert_eq!(vals[0], Kind::Int(255));
    assert_eq!(vals[1], Kind::Int(10));
    assert_eq!(vals[2], Kind::Int(1000));
    assert_eq!(vals[3], Kind::Real(3.5));
    assert_eq!(vals[4], Kind::Real(2000.0));
}

#[test]
fn string_escapes_decoded() {
    let arena = Arena::new();
    let toks = Lexer::tokenize(r#""a\tb\x41""#, &arena).unwrap();
    assert_eq!(toks[0].kind, Kind::Str(b"a\tbA"));
}

#[test]
fn string_raw_bytes_and_unicode() {
    let arena = Arena::new();
    let toks = Lexer::tokenize(r#""\xFF\u{41}\u{1F600}""#, &arena).unwrap();
    // 0xFF raw byte, 'A', then the 4-byte UTF-8 of U+1F600.
    assert_eq!(toks[0].kind, Kind::Str(b"\xFFA\xF0\x9F\x98\x80"));
}

#[test]
fn intrinsic_names() {
    let arena = Arena::new();
    let toks = Lexer::tokenize("@struct t", &arena).unwrap();
    assert_eq!(toks[0].kind, Kind::At);
    assert_eq!(toks[0].intrinsic_name(), "struct");
    assert_eq!(toks[1].kind, Kind::Word);
    assert_eq!(toks[1].text, "t");
}

#[test]
fn keywords_recognized() {
    assert_eq!(
        kinds("let x in is else"),
        vec![
            Kind::Let,
            Kind::Word,
            Kind::In,
            Kind::Is,
            Kind::Else,
            Kind::Eof,
        ]
    );
}

#[test]
fn comments_are_skipped_including_nested_blocks() {
    assert_eq!(
        kinds("a # line\n b #- outer #- inner -# still -# c"),
        vec![Kind::Word, Kind::Word, Kind::Word, Kind::Eof]
    );
}

#[test]
fn peek_then_next_and_backtrack() {
    let arena = Arena::new();
    let mut lx = Lexer::new("a b c", &arena);
    assert_eq!(lx.peek(0).unwrap().kind, Kind::Word);
    assert_eq!(lx.peek(2).unwrap().kind, Kind::Word);
    let mark = lx.mark();
    assert_eq!(lx.next_token().unwrap().text, "a");
    assert_eq!(lx.next_token().unwrap().text, "b");
    lx.reset(mark);
    assert_eq!(lx.next_token().unwrap().text, "a");
}

#[test]
fn unclosed_string_is_an_error() {
    let arena = Arena::new();
    let err = Lexer::tokenize("\"oops", &arena).unwrap_err();
    assert_eq!(err.root().code, Code::UnclosedQuote);
}

#[test]
fn line_numbers_track_newlines() {
    let arena = Arena::new();
    let toks = Lexer::tokenize("a\n\nb", &arena).unwrap();
    assert_eq!(toks[0].line, 1);
    assert_eq!(toks[1].line, 3);
}
