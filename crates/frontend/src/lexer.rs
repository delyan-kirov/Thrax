//! Streaming lexer for Thrax.
//!
//! The public surface is a forward cursor: [`Lexer::peek`] and
//! [`Lexer::next_token`] hand out one token at a time, lexing on demand, and
//! [`Lexer::mark`] /
//! [`Lexer::reset`] give the parser cheap backtracking. Comments are lexed but
//! transparently skipped. This is the same reusable API as the C++ lexer.
//!
//! Tokens are borrow-free: a [`Kind`] tag plus a [`Span`]. A lexeme is
//! `source[span]`, resolved on demand; a string literal's bytes are decoded
//! from the source later by [`decode_string`]. So the token stream owns nothing
//! borrowed and is `Send`, and the lexer needs no arena.

pub mod data;
#[cfg(test)]
mod tests;

use utilities::{Code, Diagnostic, Line, Result, Span};

pub use crate::lexer::data::{Kind, Token};

/// A forward, buffered cursor over the source token stream. It borrows the
/// (read-only) source and emits borrow-free [`Token`]s: a token is just a tag
/// plus a span, so the token stream outlives the lexer and is `Send`. A token's
/// lexeme is recovered from the source on demand (see [`Lexer::source`]).
pub struct Lexer<'a> {
    src: &'a str,
    /// Added to every emitted span, so a lexer over a *slice* of a larger source
    /// (a string interpolant's `{...}`) still reports absolute source offsets.
    base: usize,
    cursor: usize, // byte offset of the next unlexed character
    line: Line,
    /// Already-lexed, comment-free tokens; the cursor into them is `pos`.
    buffer: Vec<Token>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            src,
            base: 0,
            cursor: 0,
            line: 1,
            buffer: Vec::new(),
            pos: 0,
        }
    }

    /// A lexer over a sub-slice of a larger source, whose spans are offset by
    /// `base` (the slice's start) and whose lines count from `line`. Used to
    /// re-lex a string interpolant's expression with absolute source spans.
    pub fn sub(src: &'a str, base: usize, line: Line) -> Lexer<'a> {
        Lexer {
            src,
            base,
            cursor: 0,
            line,
            buffer: Vec::new(),
            pos: 0,
        }
    }

    /// The borrowed source, for whoever resolves token lexemes (the parser)
    /// now that tokens themselves carry only spans.
    pub fn source(&self) -> &'a str {
        self.src
    }

    fn bytes(&self) -> &[u8] {
        self.src.as_bytes()
    }

    /// Peek the token `n` positions ahead (`peek(0)` is what `next_token` will
    /// return).
    pub fn peek(&mut self, n: usize) -> Result<Token> {
        self.ensure(n)?;
        Ok(self.buffer[self.pos + n])
    }

    /// Consume and return the next token. On reaching the end it returns a
    /// sticky [`Kind::Eof`] every time, so callers can keep peeking past it.
    pub fn next_token(&mut self) -> Result<Token> {
        self.ensure(0)?;
        let tok = self.buffer[self.pos];
        if !matches!(tok.kind, Kind::Eof) {
            self.pos += 1;
        }
        Ok(tok)
    }

    /// Snapshot the cursor for later [`Lexer::reset`].
    pub fn mark(&self) -> usize {
        self.pos
    }

    /// Rewind to a snapshot taken by [`Lexer::mark`].
    pub fn reset(&mut self, mark: usize) {
        debug_assert!(
            mark <= self.buffer.len(),
            "reset mark is beyond lexed tokens"
        );
        self.pos = mark;
    }

    /// Eagerly lex the whole stream into a vector ending in [`Kind::Eof`].
    pub fn tokenize(src: &'a str) -> Result<Vec<Token>> {
        let mut lx = Lexer::new(src);
        let mut out = Vec::new();
        loop {
            let tok = lx.next_token()?;
            let done = matches!(tok.kind, Kind::Eof);
            out.push(tok);
            if done {
                return Ok(out);
            }
        }
    }

    // -- buffering ----------------------------------------------------------

    /// Guarantee `buffer[pos + n]` exists, lexing (and dropping comments) as
    /// needed.
    fn ensure(&mut self, n: usize) -> Result<()> {
        while self.buffer.len() <= self.pos + n {
            let tok = self.lex_one()?;
            if matches!(tok.kind, Kind::Comment) {
                continue;
            }
            self.buffer.push(tok);
        }
        Ok(())
    }

    // -- low-level cursor ---------------------------------------------------

    fn at(&self, i: usize) -> u8 {
        if i < self.bytes().len() {
            self.bytes()[i]
        } else {
            0
        }
    }

    fn cur(&self) -> u8 {
        self.at(self.cursor)
    }

    fn slice_from(&self, start: usize) -> &str {
        &self.src[start..self.cursor]
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(self.base + start, self.base + self.cursor)
    }

    fn mk(&self, kind: Kind, start: usize, line: Line) -> Token {
        Token {
            kind,
            span: self.span_from(start),
            line,
        }
    }

    fn err(&self, code: Code, start: usize, line: Line, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(code, self.span_from(start), line, msg)
    }

    fn skip_whitespace(&mut self) {
        while self.cursor < self.bytes().len() {
            match self.cur() {
                b' ' | b'\t' | b'\r' => self.cursor += 1,
                b'\n' => {
                    self.cursor += 1;
                    self.line += 1;
                }
                _ => break,
            }
        }
    }

    // -- one token ----------------------------------------------------------

    fn lex_one(&mut self) -> Result<Token> {
        self.skip_whitespace();
        let start = self.cursor;
        let line = self.line;

        if self.cursor >= self.bytes().len() {
            return Ok(self.mk(Kind::Eof, start, line));
        }

        let c = self.cur();
        match c {
            b'#' => self.lex_comment(start, line),
            b'"' => self.lex_string(start, line),
            b'@' => self.lex_intrinsic(start, line),
            _ if crate::lexer::data::is_digit(c) => self.lex_number(start, line),
            _ if crate::lexer::data::is_ident_start(c) => Ok(self.lex_word(start, line)),
            _ if crate::lexer::data::is_operator_char(c) => self.lex_operator(start, line),
            // `...` is one token (an inclusive range in patterns); a lone or double
            // `.` stays a `Dot` delimiter, so `..rest` and `[..]` are unaffected.
            b'.' if self.at(self.cursor + 1) == b'.' && self.at(self.cursor + 2) == b'.' => {
                self.cursor += 3;
                Ok(self.mk(Kind::Ellipsis, start, line))
            }
            _ => {
                if let Some(kind) = crate::lexer::data::delimiter(c) {
                    self.cursor += 1;
                    return Ok(self.mk(kind, start, line));
                }
                self.cursor += 1;
                let (code, what) = if c < 0x20 {
                    (Code::AsciiControlChar, "a control character")
                } else if c >= 0x80 {
                    (Code::NonAsciiChar, "a non-ASCII byte")
                } else {
                    (Code::UnknownSymbol, "an unknown character")
                };
                Err(self.err(
                    code,
                    start,
                    line,
                    format!("unexpected {what} (byte 0x{c:02X})"),
                ))
            }
        }
    }

    // -- per-kind scanners --------------------------------------------------

    /// `# ...` line comment, or a nesting `#- ... -#` block comment.
    fn lex_comment(&mut self, start: usize, line: Line) -> Result<Token> {
        if self.at(self.cursor + 1) == b'-' {
            self.cursor += 2; // opening `#-`
            let mut depth = 1usize;
            while depth > 0 {
                match self.cur() {
                    0 if self.cursor >= self.bytes().len() => {
                        return Err(self.err(
                            Code::UnclosedQuote,
                            start,
                            line,
                            "block comment is not closed with '-#'",
                        ));
                    }
                    b'#' if self.at(self.cursor + 1) == b'-' => {
                        depth += 1;
                        self.cursor += 2;
                    }
                    b'-' if self.at(self.cursor + 1) == b'#' => {
                        depth -= 1;
                        self.cursor += 2;
                    }
                    b'\n' => {
                        self.line += 1;
                        self.cursor += 1;
                    }
                    _ => self.cursor += 1,
                }
            }
            return Ok(self.mk(Kind::Comment, start, line));
        }

        while self.cursor < self.bytes().len() && self.cur() != b'\n' {
            self.cursor += 1;
        }
        Ok(self.mk(Kind::Comment, start, line))
    }

    /// A double-quoted string literal, possibly with `{expr}` interpolations.
    /// The lexer only finds the literal's extent; the chunks and interpolants
    /// are decoded/re-parsed later from `source[span]` (see the parser), which
    /// keeps the lexer allocation-free and its tokens borrow-free. `depth`
    /// tracks interpolation braces so a `"` inside `{...}` (a nested string in an
    /// interpolant) does not end the literal.
    fn lex_string(&mut self, start: usize, line: Line) -> Result<Token> {
        self.cursor += 1; // opening quote
        let mut depth = 0usize;
        loop {
            if self.cursor >= self.bytes().len() || self.cur() == b'\n' {
                let msg = if depth > 0 {
                    "unterminated `{...}` interpolation in a string \
                     (write `\\{` for a literal brace)"
                } else {
                    "string literal is not closed with a '\"'"
                };
                return Err(self.err(Code::UnclosedQuote, start, line, msg));
            }
            match self.cur() {
                // In literal text, `\` escapes the next char (so `\"`/`\{` are
                // literal). Inside an interpolant `\` is ordinary Thrax (lambda).
                b'\\' if depth == 0 => {
                    self.cursor += 1;
                    if self.cursor < self.bytes().len() && self.cur() != b'\n' {
                        self.cursor += 1;
                    }
                }
                b'"' if depth == 0 => {
                    self.cursor += 1; // closing quote
                    return Ok(self.mk(Kind::Str, start, line));
                }
                b'"' => self.skip_nested_string(start, line)?, // string in an interpolant
                b'{' => {
                    depth += 1;
                    self.cursor += 1;
                }
                b'}' if depth > 0 => {
                    depth -= 1;
                    self.cursor += 1;
                }
                _ => self.cursor += 1,
            }
        }
    }

    /// Skip a string literal nested inside an interpolant, from its opening `"`
    /// to its closing `"` (respecting `\"`), so it does not end the outer string.
    fn skip_nested_string(&mut self, start: usize, line: Line) -> Result<()> {
        self.cursor += 1; // opening quote
        loop {
            if self.cursor >= self.bytes().len() || self.cur() == b'\n' {
                return Err(self.err(
                    Code::UnclosedQuote,
                    start,
                    line,
                    "string literal is not closed with a '\"'",
                ));
            }
            match self.cur() {
                b'\\' => {
                    self.cursor += 1;
                    if self.cursor < self.bytes().len() && self.cur() != b'\n' {
                        self.cursor += 1;
                    }
                }
                b'"' => {
                    self.cursor += 1;
                    return Ok(());
                }
                _ => self.cursor += 1,
            }
        }
    }

    /// `@name` intrinsic: an at-sign followed by an identifier.
    fn lex_intrinsic(&mut self, start: usize, line: Line) -> Result<Token> {
        self.cursor += 1; // '@'
        if !crate::lexer::data::is_ident_start(self.cur()) {
            return Err(self.err(
                Code::UnknownSymbol,
                start,
                line,
                "expected an intrinsic name after '@'",
            ));
        }
        self.scan_while(crate::lexer::data::is_ident_cont);
        Ok(self.mk(Kind::At, start, line))
    }

    /// An identifier or a keyword.
    fn lex_word(&mut self, start: usize, line: Line) -> Token {
        self.scan_while(crate::lexer::data::is_ident_cont);
        let kind = crate::lexer::data::keyword_or_word(self.slice_from(start));
        self.mk(kind, start, line)
    }

    /// A maximal run of operator characters, resolved against the table.
    fn lex_operator(&mut self, start: usize, line: Line) -> Result<Token> {
        self.scan_while(crate::lexer::data::is_operator_char);
        let lexeme = self.slice_from(start);
        match crate::lexer::data::operator(lexeme) {
            Some(kind) => Ok(self.mk(kind, start, line)),
            None => Err(self.err(
                Code::UnknownSymbol,
                start,
                line,
                format!("'{lexeme}' is not a known operator"),
            )),
        }
    }

    // -- numbers ------------------------------------------------------------

    /// An integer (decimal / `0x` hex / `0b` binary) or a real. Digit runs may
    /// carry interior `_` separators, which are stripped before parsing.
    fn lex_number(&mut self, start: usize, line: Line) -> Result<Token> {
        if self.cur() == b'0' && matches!(self.at(self.cursor + 1), b'x' | b'X') {
            return self.lex_radix(start, line, 16, crate::lexer::data::is_hex_digit);
        }
        if self.cur() == b'0' && matches!(self.at(self.cursor + 1), b'b' | b'B') {
            return self.lex_radix(start, line, 2, crate::lexer::data::is_bin_digit);
        }

        self.scan_while_digit_or_sep(crate::lexer::data::is_digit);

        // A real has a fractional part and/or an exponent.
        let mut is_real = false;
        if self.cur() == b'.' && crate::lexer::data::is_digit(self.at(self.cursor + 1)) {
            is_real = true;
            self.cursor += 1; // '.'
            self.scan_while_digit_or_sep(crate::lexer::data::is_digit);
        }
        if matches!(self.cur(), b'e' | b'E') {
            let mut look = self.cursor + 1;
            if matches!(self.at(look), b'+' | b'-') {
                look += 1;
            }
            if crate::lexer::data::is_digit(self.at(look)) {
                is_real = true;
                self.cursor = look;
                self.scan_while_digit_or_sep(crate::lexer::data::is_digit);
            }
        }

        let lexeme = self.slice_from(start);
        let cleaned: String = lexeme.chars().filter(|&c| c != '_').collect();
        if is_real {
            let value: f64 = cleaned.parse().map_err(|_| {
                self.err(
                    Code::NumberParsingFailure,
                    start,
                    line,
                    "malformed real literal",
                )
            })?;
            Ok(self.mk(Kind::Real(value), start, line))
        } else {
            let value: i64 = cleaned.parse().map_err(|_| {
                self.err(
                    Code::IntLiteralRange,
                    start,
                    line,
                    "integer literal does not fit in a 64-bit integer",
                )
            })?;
            Ok(self.mk(Kind::Int(value), start, line))
        }
    }

    /// A `0x` / `0b` prefixed integer.
    fn lex_radix(
        &mut self,
        start: usize,
        line: Line,
        radix: u32,
        member: fn(u8) -> bool,
    ) -> Result<Token> {
        self.cursor += 2; // the `0x` / `0b` prefix
        let digits_start = self.cursor;
        self.scan_while_digit_or_sep(member);
        if self.cursor == digits_start {
            return Err(self.err(
                Code::NumberParsingFailure,
                start,
                line,
                "expected digits after the radix prefix",
            ));
        }
        let cleaned: String = self
            .slice_from(digits_start)
            .chars()
            .filter(|&c| c != '_')
            .collect();
        let value = i64::from_str_radix(&cleaned, radix).map_err(|_| {
            self.err(
                Code::IntLiteralRange,
                start,
                line,
                "integer literal does not fit in a 64-bit integer",
            )
        })?;
        Ok(self.mk(Kind::Int(value), start, line))
    }

    // -- scan helpers -------------------------------------------------------

    fn scan_while(&mut self, member: fn(u8) -> bool) {
        while self.cursor < self.bytes().len() && member(self.cur()) {
            self.cursor += 1;
        }
    }

    /// Scan a digit run of `member`, tolerating interior `_` separators.
    fn scan_while_digit_or_sep(&mut self, member: fn(u8) -> bool) {
        while self.cursor < self.bytes().len() {
            let c = self.cur();
            if member(c) || c == b'_' {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }
}

// -- string decoding --------------------------------------------------------

/// Decode a string literal's bytes from its raw source slice `raw` (the literal
/// *including* its surrounding quotes), interpreting C-style escapes. `start` is
/// `raw`'s byte offset in the whole source and `line` its line, both only for
/// diagnostics. Thrax strings are byte vectors, so the result is raw bytes, not
/// required to be valid UTF-8. The lexer defers to this so it stays borrow-free;
/// the parser calls it when building a string node or pattern.
pub fn decode_string(raw: &str, start: usize, line: Line) -> Result<Vec<u8>> {
    let bytes = raw.as_bytes();
    debug_assert!(
        bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"',
        "decode_string expects a quoted literal"
    );
    let inner = &bytes[1..bytes.len() - 1];
    let body_start = start + 1; // absolute offset of `inner[0]`
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        i = decode_escape(inner, i, body_start, line, &mut out)?;
    }
    Ok(out)
}

/// Decode one escape sequence. `i` points at the backslash; `body` is the byte
/// slice being decoded and `body_start` its absolute source offset (for
/// diagnostics). Appends the decoded byte(s) to `out` and returns the index just
/// past the sequence. Shared by plain literals and interpolation chunks.
pub fn decode_escape(
    body: &[u8],
    i: usize,
    body_start: usize,
    line: Line,
    out: &mut Vec<u8>,
) -> Result<usize> {
    let esc_at = body_start + i; // offset of the backslash
    let bad = |at: usize, msg: &str| {
        Diagnostic::error(Code::InvalidEscape, Span::new(at, at + 1), line, msg.to_string())
    };
    let mut i = i + 1; // past the backslash
    let sel = *body.get(i).ok_or_else(|| bad(esc_at, "dangling escape '\\'"))?;
    i += 1;
    match sel {
        b'n' => out.push(b'\n'),
        b't' => out.push(b'\t'),
        b'r' => out.push(b'\r'),
        b'0' => out.push(b'\0'),
        b'\\' => out.push(b'\\'),
        b'"' => out.push(b'"'),
        b'\'' => out.push(b'\''),
        b'{' => out.push(b'{'), // literal brace (not a string interpolation)
        b'}' => out.push(b'}'),
        b'a' => out.push(0x07), // bell
        b'b' => out.push(0x08), // backspace
        b'f' => out.push(0x0C), // form feed
        b'v' => out.push(0x0B), // vertical tab
        b'x' => i = decode_hex_byte(body, i, esc_at, line, out)?,
        b'u' => i = decode_unicode(body, i, esc_at, line, out)?,
        e => return Err(bad(esc_at, &format!("unknown escape '\\{}'", e as char))),
    }
    Ok(i)
}

/// `\xHH`: exactly two hex digits naming one raw byte. `i` points at the first
/// hex digit; returns the index just past the two digits.
fn decode_hex_byte(
    inner: &[u8],
    i: usize,
    esc_at: usize,
    line: Line,
    out: &mut Vec<u8>,
) -> Result<usize> {
    let h1 = inner.get(i).copied().unwrap_or(0);
    let h2 = inner.get(i + 1).copied().unwrap_or(0);
    if !crate::lexer::data::is_hex_digit(h1) || !crate::lexer::data::is_hex_digit(h2) {
        return Err(Diagnostic::error(
            Code::InvalidEscape,
            Span::new(esc_at, esc_at + 1),
            line,
            "'\\x' must be followed by exactly two hex digits".to_string(),
        ));
    }
    let hi = (h1 as char).to_digit(16).unwrap() as u8;
    let lo = (h2 as char).to_digit(16).unwrap() as u8;
    out.push((hi << 4) | lo);
    Ok(i + 2)
}

/// `\u{H..}`: 1..6 hex digits naming a Unicode scalar value, encoded UTF-8. `i`
/// points at the `{`; returns the index just past the closing `}`.
fn decode_unicode(
    inner: &[u8],
    mut i: usize,
    esc_at: usize,
    line: Line,
    out: &mut Vec<u8>,
) -> Result<usize> {
    let bad = || {
        Diagnostic::error(
            Code::InvalidEscape,
            Span::new(esc_at, esc_at + 1),
            line,
            "'\\u{...}' needs 1-6 hex digits naming a Unicode scalar value \
             (<= U+10FFFF, not a surrogate U+D800..U+DFFF)"
                .to_string(),
        )
    };
    if inner.get(i).copied() != Some(b'{') {
        return Err(bad());
    }
    i += 1; // '{'
    let mut cp: u32 = 0;
    let mut digits = 0;
    while digits < 6 {
        let d = inner.get(i).copied().unwrap_or(0);
        if !crate::lexer::data::is_hex_digit(d) {
            break;
        }
        cp = cp * 16 + (d as char).to_digit(16).unwrap();
        i += 1;
        digits += 1;
    }
    if digits == 0 || inner.get(i).copied() != Some(b'}') {
        return Err(bad());
    }
    i += 1; // '}'
    let scalar = char::from_u32(cp).ok_or_else(bad)?;
    let mut buf = [0u8; 4];
    out.extend_from_slice(scalar.encode_utf8(&mut buf).as_bytes());
    Ok(i)
}
