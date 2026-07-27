//! The token model plus the keyword/operator/delimiter lookup tables.
//!
//! Payload-carrying kinds (`Int`, `Real`, `Str`) hold their parsed value inline;
//! every other kind is fully determined by its tag plus the lexeme text. This
//! mirrors the C++ lexer's `TokenTag` + payload `variant` split.

use diag::{Line, Span};

/// A lexical token: a [`Kind`], the exact source lexeme, and its position.
#[derive(Clone, Copy, Debug)]
pub struct Token<'a> {
    pub kind: Kind<'a>,
    /// The verbatim source slice (includes sigils like `@`, `` ` ``, quotes).
    pub text: &'a str,
    pub span: Span,
    pub line: Line,
}

impl<'a> Token<'a> {
    /// The identifier text of a `@name` intrinsic, past the leading `@`.
    pub fn intrinsic_name(&self) -> &'a str {
        debug_assert!(
            matches!(self.kind, Kind::At),
            "intrinsic_name on non-@ token"
        );
        &self.text[1..]
    }

    /// The name of a `` `T `` type variable, past the leading backtick.
    pub fn tyvar_name(&self) -> &'a str {
        debug_assert!(
            matches!(self.kind, Kind::TyVar),
            "tyvar_name on non-tyvar token"
        );
        &self.text[1..]
    }
}

/// The lexical category. Textbook name: this is the token's *tag*.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind<'a> {
    // Literals (payload-carrying).
    Int(i64),
    Real(f64),
    Str(&'a [u8]), // decoded bytes (Thrax strings are byte vectors, not UTF-8);
    // `Token::text` still holds the quotes

    // Names.
    Word,  // identifier; lexeme in `Token::text`
    TyVar, // `T
    At,    // @name intrinsic
    Op,    // operator lexeme (+ - * / % ! ?= ?< ?> <= >= < > | ; |> :: ++ ...)

    // Structural punctuation that carries its own meaning.
    Eq,       // =
    FatArrow, // =>
    Arrow,    // ->
    Lambda,   // \
    Colon,    // :
    Dollar,   // $
    Comma,    // ,
    Dot,      // .
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
    When,
    Is,
    Then,
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

/// Keyword table: lexeme -> kind. Linear-searched (twelve entries).
pub const KEYWORDS: &[(&str, Kind<'static>)] = &[
    ("let", Kind::Let),
    ("in", Kind::In),
    ("if", Kind::If),
    ("when", Kind::When),
    ("is", Kind::Is),
    ("then", Kind::Then),
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
pub const OPERATORS: &[(&str, Kind<'static>)] = &[
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
pub const DELIMITERS: &[(u8, Kind<'static>)] = &[
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
pub fn keyword_or_word(s: &str) -> Kind<'static> {
    lookup_str(KEYWORDS, s).unwrap_or(Kind::Word)
}

/// Look up a full operator run; `None` means the run is not a valid operator.
pub fn operator(s: &str) -> Option<Kind<'static>> {
    lookup_str(OPERATORS, s)
}

/// Look up a single delimiter byte.
pub fn delimiter(b: u8) -> Option<Kind<'static>> {
    DELIMITERS.iter().find(|&&(k, _)| k == b).map(|&(_, v)| v)
}

fn lookup_str(table: &[(&str, Kind<'static>)], s: &str) -> Option<Kind<'static>> {
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
