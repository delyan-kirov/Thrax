//! Streaming lexer for Thrax.
//!
//! The public surface is a forward cursor: [`Lexer::peek`] and
//! [`Lexer::next_token`] hand out one token at a time, lexing on demand, and
//! [`Lexer::mark`] /
//! [`Lexer::reset`] give the parser cheap backtracking. Comments are lexed but
//! transparently skipped. This is the same reusable API as the C++ lexer.
//!
//! Tokens borrow the source for their lexeme; decoded string literals are copied
//! into a caller-supplied [`Arena`], so a `Token`'s payloads live as long as the
//! source and arena do.

pub mod data;
#[cfg(test)]
mod tests;

use utilities::Arena;
use utilities::{Code, Diagnostic, Line, Result, Span};

pub use crate::lexer::data::{Kind, Token};

/// A forward, buffered cursor over the source token stream.
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    arena: &'a Arena,
    cursor: usize, // byte offset of the next unlexed character
    line: Line,
    /// Already-lexed, comment-free tokens; the cursor into them is `pos`.
    buffer: Vec<Token<'a>>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, arena: &'a Arena) -> Lexer<'a> {
        Lexer {
            src,
            bytes: src.as_bytes(),
            arena,
            cursor: 0,
            line: 1,
            buffer: Vec::new(),
            pos: 0,
        }
    }

    /// Peek the token `n` positions ahead (`peek(0)` is what `next_token` will
    /// return).
    pub fn peek(&mut self, n: usize) -> Result<Token<'a>> {
        self.ensure(n)?;
        Ok(self.buffer[self.pos + n])
    }

    /// Consume and return the next token. On reaching the end it returns a
    /// sticky [`Kind::Eof`] every time, so callers can keep peeking past it.
    pub fn next_token(&mut self) -> Result<Token<'a>> {
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
    pub fn tokenize(src: &'a str, arena: &'a Arena) -> Result<Vec<Token<'a>>> {
        let mut lx = Lexer::new(src, arena);
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
        if i < self.bytes.len() {
            self.bytes[i]
        } else {
            0
        }
    }

    fn cur(&self) -> u8 {
        self.at(self.cursor)
    }

    fn slice_from(&self, start: usize) -> &'a str {
        &self.src[start..self.cursor]
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.cursor)
    }

    fn mk(&self, kind: Kind<'a>, start: usize, line: Line) -> Token<'a> {
        Token {
            kind,
            text: self.slice_from(start),
            span: self.span_from(start),
            line,
        }
    }

    fn err(&self, code: Code, start: usize, line: Line, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(code, self.span_from(start), line, msg)
    }

    fn skip_whitespace(&mut self) {
        while self.cursor < self.bytes.len() {
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

    fn lex_one(&mut self) -> Result<Token<'a>> {
        self.skip_whitespace();
        let start = self.cursor;
        let line = self.line;

        if self.cursor >= self.bytes.len() {
            return Ok(self.mk(Kind::Eof, start, line));
        }

        let c = self.cur();
        match c {
            b'#' => self.lex_comment(start, line),
            b'"' => self.lex_string(start, line),
            b'`' => self.lex_tyvar(start, line),
            b'@' => self.lex_intrinsic(start, line),
            _ if crate::lexer::data::is_digit(c) => self.lex_number(start, line),
            _ if crate::lexer::data::is_ident_start(c) => Ok(self.lex_word(start, line)),
            _ if crate::lexer::data::is_operator_char(c) => self.lex_operator(start, line),
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
    fn lex_comment(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
        if self.at(self.cursor + 1) == b'-' {
            self.cursor += 2; // opening `#-`
            let mut depth = 1usize;
            while depth > 0 {
                match self.cur() {
                    0 if self.cursor >= self.bytes.len() => {
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

        while self.cursor < self.bytes.len() && self.cur() != b'\n' {
            self.cursor += 1;
        }
        Ok(self.mk(Kind::Comment, start, line))
    }

    /// A double-quoted string literal with C-style escapes. The decoded bytes
    /// are copied into the arena; `Token::text` keeps the quotes.
    fn lex_string(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
        self.cursor += 1; // opening quote
        let mut decoded: Vec<u8> = Vec::new();
        loop {
            let c = self.cur();
            if self.cursor >= self.bytes.len() || c == b'\n' {
                return Err(self.err(
                    Code::UnclosedQuote,
                    start,
                    line,
                    "string literal is not closed with a '\"'",
                ));
            }
            if c == b'"' {
                self.cursor += 1; // closing quote
                let stored = self.arena.alloc_slice_copy(&decoded);
                return Ok(self.mk(Kind::Str(stored), start, line));
            }
            if c != b'\\' {
                // Ordinary text: the source is valid UTF-8, so copying its raw
                // bytes preserves multi-byte scalars unchanged.
                decoded.push(c);
                self.cursor += 1;
                continue;
            }
            self.cursor += 1; // the backslash
            match self.cur() {
                b'n' => decoded.push(b'\n'),
                b't' => decoded.push(b'\t'),
                b'r' => decoded.push(b'\r'),
                b'0' => decoded.push(b'\0'),
                b'\\' => decoded.push(b'\\'),
                b'"' => decoded.push(b'"'),
                b'\'' => decoded.push(b'\''),
                b'a' => decoded.push(0x07), // bell
                b'b' => decoded.push(0x08), // backspace
                b'f' => decoded.push(0x0C), // form feed
                b'v' => decoded.push(0x0B), // vertical tab
                b'x' => self.escape_hex_byte(start, line, &mut decoded)?,
                b'u' => self.escape_unicode(start, line, &mut decoded)?,
                e => {
                    return Err(self.err(
                        Code::InvalidEscape,
                        start,
                        line,
                        format!("unknown escape '\\{}'", e as char),
                    ));
                }
            }
            self.cursor += 1; // the escape's selector char
        }
    }

    /// `\xHH`: exactly two hex digits naming one raw byte.
    fn escape_hex_byte(&mut self, start: usize, line: Line, out: &mut Vec<u8>) -> Result<()> {
        let h1 = self.at(self.cursor + 1);
        let h2 = self.at(self.cursor + 2);
        if !crate::lexer::data::is_hex_digit(h1) || !crate::lexer::data::is_hex_digit(h2) {
            return Err(self.err(
                Code::InvalidEscape,
                start,
                line,
                "'\\x' must be followed by exactly two hex digits",
            ));
        }
        let hi = (h1 as char).to_digit(16).unwrap() as u8;
        let lo = (h2 as char).to_digit(16).unwrap() as u8;
        out.push((hi << 4) | lo);
        self.cursor += 2;
        Ok(())
    }

    /// `\u{H..}`: 1..6 hex digits naming a Unicode scalar value, encoded UTF-8.
    fn escape_unicode(&mut self, start: usize, line: Line, out: &mut Vec<u8>) -> Result<()> {
        let bad = |lx: &Self| {
            lx.err(
                Code::InvalidEscape,
                start,
                line,
                "'\\u{...}' needs 1-6 hex digits naming a Unicode scalar value \
                 (<= U+10FFFF, not a surrogate U+D800..U+DFFF)",
            )
        };
        if self.at(self.cursor + 1) != b'{' {
            return Err(bad(self));
        }
        self.cursor += 2; // 'u' and '{'
        let mut cp: u32 = 0;
        let mut digits = 0;
        while crate::lexer::data::is_hex_digit(self.cur()) && digits < 6 {
            cp = cp * 16 + (self.cur() as char).to_digit(16).unwrap();
            self.cursor += 1;
            digits += 1;
        }
        if digits == 0 || self.cur() != b'}' {
            return Err(bad(self));
        }
        let scalar = char::from_u32(cp).ok_or_else(|| bad(self))?;
        let mut buf = [0u8; 4];
        out.extend_from_slice(scalar.encode_utf8(&mut buf).as_bytes());
        // Leave the closing '}' as the "selector char" consumed by the caller.
        Ok(())
    }

    /// `` `T `` type variable: a backtick followed by a capitalized identifier.
    /// The leading letter must be uppercase, since all type names (constructors
    /// and variables alike) are capitalized.
    fn lex_tyvar(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
        self.cursor += 1; // backtick
        if !self.cur().is_ascii_uppercase() {
            return Err(self.err(
                Code::UnknownSymbol,
                start,
                line,
                "a type variable must start with a capital letter (e.g. `` `A ``)",
            ));
        }
        self.scan_while(crate::lexer::data::is_ident_cont);
        Ok(self.mk(Kind::TyVar, start, line))
    }

    /// `@name` intrinsic: an at-sign followed by an identifier.
    fn lex_intrinsic(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
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
    fn lex_word(&mut self, start: usize, line: Line) -> Token<'a> {
        self.scan_while(crate::lexer::data::is_ident_cont);
        let kind = crate::lexer::data::keyword_or_word(self.slice_from(start));
        self.mk(kind, start, line)
    }

    /// A maximal run of operator characters, resolved against the table.
    fn lex_operator(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
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
    fn lex_number(&mut self, start: usize, line: Line) -> Result<Token<'a>> {
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
    ) -> Result<Token<'a>> {
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
        while self.cursor < self.bytes.len() && member(self.cur()) {
            self.cursor += 1;
        }
    }

    /// Scan a digit run of `member`, tolerating interior `_` separators.
    fn scan_while_digit_or_sep(&mut self, member: fn(u8) -> bool) {
        while self.cursor < self.bytes.len() {
            let c = self.cur();
            if member(c) || c == b'_' {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }
}
