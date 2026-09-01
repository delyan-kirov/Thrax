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

/// The parser role of an operator lexeme, and the single source of truth for it.
/// Every operator declares exactly one role, so a lexeme cannot lex as a token
/// yet be unknown to the parser (the class of bug where an operator is added to
/// the lexer alone). The lexer derives a token's [`Kind`] from the role; the
/// parser derives binding power and prefix/infix status from the same role.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OpRole {
    /// Structural punctuation with its own [`Kind`] (`\`, `=`, `=>`, `->`, `:`,
    /// `$`). Not [`Kind::Op`]; carries no binding power.
    Structural(Kind),
    /// Infix binary operator with `(left, right)` binding power.
    Infix(u8, u8),
    /// Both infix and unary prefix (only `-`); the two forms stay distinct so
    /// prefix `-` never aliases binary `-`.
    InfixPrefix(u8, u8),
    /// Unary prefix only (`!`).
    Prefix,
    /// A [`Kind::Op`] lexeme that is a grammatical delimiter (`<`, `>`, `|`,
    /// `<>`): it never folds as an operator, legitimately ends an expression,
    /// and is consumed by the construct (row type, pattern) that expects it.
    Delimiter,
}

/// One operator lexeme paired with its [`OpRole`].
pub struct OpDef {
    pub lexeme: &'static str,
    pub role: OpRole,
}

impl OpDef {
    /// The token [`Kind`] the lexer emits for this operator.
    pub const fn kind(&self) -> Kind {
        match self.role {
            OpRole::Structural(k) => k,
            _ => Kind::Op,
        }
    }
}

/// Operator table: a *maximal* run of operator characters is looked up whole.
/// A structural operator carries its own kind; every other operator is a
/// [`Kind::Op`] whose lexeme the parser turns into a variable of that name. A
/// run absent from this table is an error. Binding powers: a left-associative
/// operator uses `left < right`, a right-associative one `left > right`.
pub const OPERATORS: &[OpDef] = &[
    // Structural.
    OpDef { lexeme: "\\", role: OpRole::Structural(Kind::Lambda) },
    OpDef { lexeme: "=", role: OpRole::Structural(Kind::Eq) },
    OpDef { lexeme: "=>", role: OpRole::Structural(Kind::FatArrow) },
    OpDef { lexeme: "->", role: OpRole::Structural(Kind::Arrow) },
    OpDef { lexeme: ":", role: OpRole::Structural(Kind::Colon) },
    OpDef { lexeme: "$", role: OpRole::Structural(Kind::Dollar) },
    // Arithmetic. `^` (exponentiation) is right-associative and binds tighter
    // than `*`; unary prefix still binds tighter than `^`.
    OpDef { lexeme: "+", role: OpRole::Infix(20, 21) },
    OpDef { lexeme: "-", role: OpRole::InfixPrefix(20, 21) },
    OpDef { lexeme: "*", role: OpRole::Infix(30, 31) },
    OpDef { lexeme: "/", role: OpRole::Infix(30, 31) },
    OpDef { lexeme: "%", role: OpRole::Infix(30, 31) },
    OpDef { lexeme: "^", role: OpRole::Infix(35, 34) },
    OpDef { lexeme: "!", role: OpRole::Prefix },
    // Comparison (all one precedence, left-associative).
    OpDef { lexeme: "?=", role: OpRole::Infix(10, 11) },
    OpDef { lexeme: "?>", role: OpRole::Infix(10, 11) },
    OpDef { lexeme: "?<", role: OpRole::Infix(10, 11) },
    OpDef { lexeme: "<=", role: OpRole::Infix(10, 11) },
    OpDef { lexeme: ">=", role: OpRole::Infix(10, 11) },
    // Effect-row delimiters and their coalesced forms.
    OpDef { lexeme: "<", role: OpRole::Delimiter },
    OpDef { lexeme: ">", role: OpRole::Delimiter },
    OpDef { lexeme: "|", role: OpRole::Delimiter },
    OpDef { lexeme: "<>", role: OpRole::Delimiter },
    // Pipe into a function on the left; right-associative.
    OpDef { lexeme: "<|", role: OpRole::Infix(5, 4) },
    // Short-circuit boolean and/or (desugared to a lazy `if` in the parser).
    OpDef { lexeme: "&&", role: OpRole::Infix(9, 10) },
    OpDef { lexeme: "||", role: OpRole::Infix(8, 9) },
    // Sequencing / pipe-forward (desugared in the parser).
    OpDef { lexeme: ";", role: OpRole::Infix(2, 1) },
    OpDef { lexeme: "|>", role: OpRole::Infix(6, 7) },
    // List cons (right-associative) and Str/Array concatenation.
    OpDef { lexeme: "::", role: OpRole::Infix(15, 14) },
    OpDef { lexeme: "++", role: OpRole::Infix(16, 17) },
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
    op_def(s).map(OpDef::kind)
}

/// Look up an operator lexeme's [`OpRole`]; the parser's binding-power tables
/// read this. `None` means the lexeme is not an operator.
pub fn op_role(s: &str) -> Option<OpRole> {
    op_def(s).map(|d| d.role)
}

fn op_def(s: &str) -> Option<&'static OpDef> {
    OPERATORS.iter().find(|d| d.lexeme == s)
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
