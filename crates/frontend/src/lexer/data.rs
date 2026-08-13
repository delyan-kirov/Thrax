//! The token model plus the keyword/operator/delimiter lookup tables.
//!
//! A token is pure data: a [`Kind`] tag plus a [`Span`] into the source. It
//! carries no borrow, so the token stream is `Send` and outlives the source. A
//! token's lexeme is `source[span]`, resolved on demand by whoever holds the
//! source (see the parser). `Int`/`Real` keep their parsed value inline; a
//! string literal's decoded bytes are produced later (the `Str` tag only marks
//! the literal's extent), so decoding stays out of the lexer.

use utilities::{Line, Span};

/// A lexical token: a [`Kind`] tag and the source [`Span`] it covers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Token {
    pub kind: Kind,
    pub span: Span,
    pub line: Line,
}

/// The lexical category. Textbook name: this is the token's *tag*.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    // Literals.
    Int(i64),
    Real(f64),
    Str, // a string literal; its bytes are decoded from `source[span]` later

    // Names.
    Word, // identifier; lexeme is `source[span]`
    At,   // @name intrinsic
    Op,   // operator lexeme (+ - * / % ! ?= ?< ?> <= >= < > | ; |> :: ++ ...)

    // Structural punctuation that carries its own meaning.
    Eq,       // =
    FatArrow, // =>
    Arrow,    // ->
    Lambda,   // \
    Colon,    // :
    Dollar,   // $
    Comma,    // ,
    Dot,      // .
    Ellipsis, // ... (inclusive range in patterns)
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBrack,
    RBrack,

    // Keywords.
    Let,
    In,
    If,
    Is,
    Else,
    Ext,
    With,
    Do,
    Ctl,
    Defer,

    // Only the lexer emits this; peek/next skip it.
    Comment,
    Eof,
}

/// Keyword table: lexeme -> kind. Linear-searched.
pub const KEYWORDS: &[(&str, Kind)] = &[
    ("let", Kind::Let),
    ("in", Kind::In),
    ("if", Kind::If),
    ("is", Kind::Is),
    ("else", Kind::Else),
    ("ext", Kind::Ext),
    ("with", Kind::With),
    ("do", Kind::Do),
    ("ctl", Kind::Ctl),
    ("defer", Kind::Defer),
];

/// Operator table: a *maximal* run of operator characters is looked up whole.
/// The structural operators map to their own kind; every evaluable operator
/// shares [`Kind::Op`] with the lexeme kept in `Token::text` (the parser turns
/// each into a variable of that name). A run absent from this table is an error.
pub const OPERATORS: &[(&str, Kind)] = &[
    // Structural.
    ("\\", Kind::Lambda),
    ("=", Kind::Eq),
    ("=>", Kind::FatArrow),
    ("->", Kind::Arrow),
    (":", Kind::Colon),
    ("$", Kind::Dollar),
    // Arithmetic / comparison.
    ("+", Kind::Op),
    ("-", Kind::Op),
    ("*", Kind::Op),
    ("/", Kind::Op),
    ("%", Kind::Op),
    ("!", Kind::Op),
    ("?=", Kind::Op),
    ("?>", Kind::Op),
    ("?<", Kind::Op),
    ("<=", Kind::Op),
    (">=", Kind::Op),
    // Effect-row delimiters and their coalesced forms.
    ("<", Kind::Op),
    (">", Kind::Op),
    ("|", Kind::Op),
    ("<>", Kind::Op),
    ("<|", Kind::Op),
    // Sequencing / pipes (desugared in the parser).
    (";", Kind::Op),
    ("|>", Kind::Op),
    // List cons and Str/Array concatenation.
    ("::", Kind::Op),
    ("++", Kind::Op),
];

/// Single-character delimiters. Unlike operators these never coalesce.
pub const DELIMITERS: &[(u8, Kind)] = &[
    (b'(', Kind::LParen),
    (b')', Kind::RParen),
    (b',', Kind::Comma),
    (b'.', Kind::Dot),
    (b'{', Kind::LBrace),
    (b'}', Kind::RBrace),
    (b'[', Kind::LBrack),
    (b']', Kind::RBrack),
];

/// Look up `s` as a keyword, falling back to [`Kind::Word`].
pub fn keyword_or_word(s: &str) -> Kind {
    lookup_str(KEYWORDS, s).unwrap_or(Kind::Word)
}

/// Look up a full operator run; `None` means the run is not a valid operator.
pub fn operator(s: &str) -> Option<Kind> {
    lookup_str(OPERATORS, s)
}

/// Look up a single delimiter byte.
pub fn delimiter(b: u8) -> Option<Kind> {
    DELIMITERS.iter().find(|&&(k, _)| k == b).map(|&(_, v)| v)
}

fn lookup_str(table: &[(&str, Kind)], s: &str) -> Option<Kind> {
    table.iter().find(|&&(k, _)| k == s).map(|&(_, v)| v)
}

// -- character classes (matches the C++ `LXxDATA` predicates) ----------------

pub fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

pub fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

pub fn is_bin_digit(b: u8) -> bool {
    b == b'0' || b == b'1'
}

pub fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The operator characters: a maximal run of these forms one operator token.
pub fn is_operator_char(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'/'
            | b':'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'^'
            | b'|'
            | b'~'
            | b'\\'
    )
}
