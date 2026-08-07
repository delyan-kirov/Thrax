use super::*;

fn kinds(src: &str) -> Vec<Kind> {
    Lexer::tokenize(src)
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
    let toks = Lexer::tokenize("0xFF 0b1010 1_000 3.5 2e3").unwrap();
    let vals: Vec<Kind> = toks.into_iter().map(|t| t.kind).collect();
    assert_eq!(vals[0], Kind::Int(255));
    assert_eq!(vals[1], Kind::Int(10));
    assert_eq!(vals[2], Kind::Int(1000));
    assert_eq!(vals[3], Kind::Real(3.5));
    assert_eq!(vals[4], Kind::Real(2000.0));
}

#[test]
fn string_literal_lexes_as_str_and_decodes() {
    // The lexer only tags the literal's extent; decoding is deferred.
    let src = r#""a\tb\x41""#;
    let toks = Lexer::tokenize(src).unwrap();
    assert_eq!(toks[0].kind, Kind::Str);
    let lexeme = &src[toks[0].span.start..toks[0].span.end];
    assert_eq!(decode_string(lexeme, toks[0].span.start, toks[0].line).unwrap(), b"a\tbA");
}

#[test]
fn string_raw_bytes_and_unicode() {
    // 0xFF raw byte, 'A', then the 4-byte UTF-8 of U+1F600.
    let raw = r#""\xFF\u{41}\u{1F600}""#;
    assert_eq!(
        decode_string(raw, 0, 1).unwrap(),
        b"\xFFA\xF0\x9F\x98\x80"
    );
}

#[test]
fn bad_escape_is_an_error() {
    assert_eq!(
        decode_string(r#""\q""#, 0, 1).unwrap_err().root().code,
        Code::InvalidEscape
    );
}

#[test]
fn intrinsic_and_word_lexemes() {
    let src = "@struct t";
    let toks = Lexer::tokenize(src).unwrap();
    assert_eq!(toks[0].kind, Kind::At);
    assert_eq!(&src[toks[0].span.start..toks[0].span.end], "@struct");
    assert_eq!(toks[1].kind, Kind::Word);
    assert_eq!(&src[toks[1].span.start..toks[1].span.end], "t");
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
    let src = "a b c";
    let lexeme = |t: Token| &src[t.span.start..t.span.end];
    let mut lx = Lexer::new(src);
    assert_eq!(lx.peek(0).unwrap().kind, Kind::Word);
    assert_eq!(lx.peek(2).unwrap().kind, Kind::Word);
    let mark = lx.mark();
    assert_eq!(lexeme(lx.next_token().unwrap()), "a");
    assert_eq!(lexeme(lx.next_token().unwrap()), "b");
    lx.reset(mark);
    assert_eq!(lexeme(lx.next_token().unwrap()), "a");
}

#[test]
fn unclosed_string_is_an_error() {
    let err = Lexer::tokenize("\"oops").unwrap_err();
    assert_eq!(err.root().code, Code::UnclosedQuote);
}

#[test]
fn line_numbers_track_newlines() {
    let toks = Lexer::tokenize("a\n\nb").unwrap();
    assert_eq!(toks[0].line, 1);
    assert_eq!(toks[1].line, 3);
}

#[test]
fn tokens_are_borrow_free_and_send() {
    // The point of the span-based token model: the stream owns nothing borrowed,
    // so it is `Send` and outlives the source (ready for parallel parsing).
    fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<Token>();
    assert_send_static::<Kind>();
    assert_send_static::<Vec<Token>>();
}
