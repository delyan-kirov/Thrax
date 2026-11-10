//! The kernel's script language: a lexer and a recursive-descent parser.
//!
//! A script is a list of globals in the spelling of the language's dependent
//! design (issue #406), so kernel tests read like the programs the frontend
//! will one day elaborate:
//!
//! ```text
//! $ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \t x => x
//! $ two : @Type 1 -> @Type 1 = id @ctx {.u = 2} (@Type 1 -> @Type 1) (\a => a)
//! ```
//!
//! Every argument is explicit except levels, which a reference to a
//! level-polymorphic global supplies with `@ctx {.u = l}`. A global without a
//! body is an axiom, and one whose body is `@data | c fields | ...` an
//! inductive type. Primitive forms are `@name` followed by a fixed number
//! of arguments, read from the [`PRIMITIVES`] table.
//!
//! Input: source text. Output: [`Item`]s of named, unresolved [`Raw`] terms.
//!
//! Entry point: [`parse_script`].

use utilities::{Code, Diagnostic, Result, Span};

use crate::term::Name;

// ## Raw terms

#[derive(Clone, Debug)]
pub struct Raw {
    pub kind: RawKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum RawKind {
    /// A name, with the level arguments of `name @ctx {.u = l}`.
    Var(Name, Vec<LevelArg>),
    Sort(RawLevel),
    Pi(Name, Box<Raw>, Box<Raw>),
    /// `\x => body` or `\(x: A) => body`, one binder per node.
    Lam(Name, Option<Box<Raw>>, Box<Raw>),
    App(Box<Raw>, Box<Raw>),
    Let(Name, Option<Box<Raw>>, Box<Raw>, Box<Raw>),
    /// `(e : T)`.
    Ann(Box<Raw>, Box<Raw>),
    Prim(Primitive, Vec<Raw>),
    /// `(x: A) * B`.
    Sigma(Name, Box<Raw>, Box<Raw>),
    Pair(Box<Raw>, Box<Raw>),
    /// `e.1`, `e.2`.
    Fst(Box<Raw>),
    Snd(Box<Raw>),
    /// `a ~ b`.
    Eq(Box<Raw>, Box<Raw>),
}

#[derive(Clone, Debug)]
pub struct LevelArg {
    pub name: Name,
    pub level: RawLevel,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum RawLevel {
    Num(u32),
    Name(Name, Span),
    Plus(Box<RawLevel>, u32),
    Max(Box<RawLevel>, Box<RawLevel>),
    IMax(Box<RawLevel>, Box<RawLevel>),
}

/// A primitive form `@name arg ...`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Primitive {
    Top,
    Tt,
    Bottom,
    Absurd,
    /// `@Eq A a b`, the equality with its type given.
    Eq,
    Refl,
    /// `@transp t B u t' e`: from `u : B t` and `e : t ~ t'`, a proof of
    /// `B t'` (transport into a proposition).
    Transp,
    Cast,
}

/// Every primitive's spelling and argument count.
pub const PRIMITIVES: &[(&str, Primitive, usize)] = &[
    ("Top", Primitive::Top, 0),
    ("tt", Primitive::Tt, 0),
    ("Bottom", Primitive::Bottom, 0),
    ("absurd", Primitive::Absurd, 2),
    ("Eq", Primitive::Eq, 3),
    ("refl", Primitive::Refl, 1),
    ("transp", Primitive::Transp, 5),
    ("cast", Primitive::Cast, 4),
];

#[derive(Clone, Debug)]
pub struct Item {
    pub name: Name,
    pub span: Span,
    pub level_params: Vec<(Name, Span)>,
    pub ty: Raw,
    pub body: ItemBody,
}

#[derive(Clone, Debug)]
pub enum ItemBody {
    Definition(Raw),
    Axiom,
    /// `@data | c (x: A) B | ...`.
    Data(Vec<RawConstructor>),
}

#[derive(Clone, Debug)]
pub struct RawConstructor {
    pub name: Name,
    pub span: Span,
    /// Each field's name (`_` when unnamed) and type.
    pub fields: Vec<(Name, Raw)>,
}

// ## Lexer

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Ident(Name),
    Num(u32),
    /// `@word`.
    At(Name),
    Dollar,
    Colon,
    Equals,
    FatArrow,
    Arrow,
    Backslash,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Pipe,
    Plus,
    Star,
    Tilde,
    Let,
    In,
    Eof,
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(n) => format!("`{n}`"),
        Tok::Num(n) => format!("`{n}`"),
        Tok::At(n) => format!("`@{n}`"),
        Tok::Eof => "the end of the script".to_string(),
        other => {
            let s = match other {
                Tok::Dollar => "$",
                Tok::Colon => ":",
                Tok::Equals => "=",
                Tok::FatArrow => "=>",
                Tok::Arrow => "->",
                Tok::Backslash => "\\",
                Tok::LParen => "(",
                Tok::RParen => ")",
                Tok::LBrace => "{",
                Tok::RBrace => "}",
                Tok::Comma => ",",
                Tok::Dot => ".",
                Tok::Pipe => "|",
                Tok::Plus => "+",
                Tok::Star => "*",
                Tok::Tilde => "~",
                Tok::Let => "let",
                Tok::In => "in",
                _ => unreachable!(),
            };
            format!("`{s}`")
        }
    }
}

/// The 1-based line of a byte offset.
pub fn line_of(src: &str, offset: usize) -> u32 {
    1 + src[..offset.min(src.len())].bytes().filter(|b| *b == b'\n').count() as u32
}

pub fn error(src: &str, code: Code, span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, span, line_of(src, span.start), msg)
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'\''
}

fn lex(src: &str) -> Result<Vec<(Tok, Span)>> {
    let bytes = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        let tok = if is_ident_start(c) {
            // A dot followed by a letter continues the name (`Nat.elim`); a
            // dot followed by a digit is a projection.
            while i < bytes.len()
                && (is_ident_char(bytes[i])
                    || (bytes[i] == b'.' && i + 1 < bytes.len() && is_ident_start(bytes[i + 1])))
            {
                i += 1;
            }
            match &src[start..i] {
                "let" => Tok::Let,
                "in" => Tok::In,
                word => Tok::Ident(word.into()),
            }
        } else if c.is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let n = src[start..i].parse().map_err(|_| {
                error(src, Code::NumberParsingFailure, Span::new(start, i), "number is too large")
            })?;
            Tok::Num(n)
        } else if c == b'@' {
            i += 1;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            if i == start + 1 {
                return Err(error(src, Code::UnknownSymbol, Span::new(start, i), "`@` must start a word"));
            }
            Tok::At(src[start + 1..i].into())
        } else {
            let two = src.get(i..i + 2).unwrap_or("");
            let (tok, len) = match two {
                "=>" => (Tok::FatArrow, 2),
                "->" => (Tok::Arrow, 2),
                _ => (
                    match c {
                        b'$' => Tok::Dollar,
                        b':' => Tok::Colon,
                        b'=' => Tok::Equals,
                        b'\\' => Tok::Backslash,
                        b'(' => Tok::LParen,
                        b')' => Tok::RParen,
                        b'{' => Tok::LBrace,
                        b'}' => Tok::RBrace,
                        b',' => Tok::Comma,
                        b'.' => Tok::Dot,
                        b'|' => Tok::Pipe,
                        b'+' => Tok::Plus,
                        b'*' => Tok::Star,
                        b'~' => Tok::Tilde,
                        _ => {
                            let end = start + src[start..].chars().next().map_or(1, char::len_utf8);
                            return Err(error(src, Code::UnknownSymbol, Span::new(start, end), "unexpected character"));
                        }
                    },
                    1,
                ),
            };
            i += len;
            tok
        };
        toks.push((tok, Span::new(start, i)));
    }
    toks.push((Tok::Eof, Span::at(src.len())));
    Ok(toks)
}

// ## Parser

struct Parser<'s> {
    src: &'s str,
    toks: Vec<(Tok, Span)>,
    pos: usize,
}

pub fn parse_script(src: &str) -> Result<Vec<Item>> {
    let mut p = Parser {
        src,
        toks: lex(src)?,
        pos: 0,
    };
    let mut items = Vec::new();
    while p.peek(0) != &Tok::Eof {
        items.push(p.item()?);
    }
    Ok(items)
}

impl Parser<'_> {
    fn peek(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].0
    }

    fn span(&self) -> Span {
        self.toks[self.pos].1
    }

    fn last_end(&self) -> usize {
        self.toks[self.pos.saturating_sub(1)].1.end
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if t != Tok::Eof {
            self.pos += 1;
        }
        t
    }

    fn unexpected(&self, wanted: &str) -> Diagnostic {
        error(
            self.src,
            Code::UnexpectedToken,
            self.span(),
            format!("expected {wanted}, found {}", describe(self.peek(0))),
        )
    }

    fn expect(&mut self, t: Tok) -> Result<Span> {
        if *self.peek(0) == t {
            let span = self.span();
            self.next();
            Ok(span)
        } else {
            Err(self.unexpected(&describe(&t)))
        }
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek(0) == t {
            self.next();
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Result<(Name, Span)> {
        let span = self.span();
        match self.next() {
            Tok::Ident(n) => Ok((n, span)),
            _ => {
                self.pos -= 1;
                Err(self.unexpected("a name"))
            }
        }
    }

    fn at_keyword(&self, word: &str) -> bool {
        matches!(self.peek(0), Tok::At(w) if &**w == word)
    }

    fn raw(&self, kind: RawKind, start: usize) -> Raw {
        Raw {
            kind,
            span: Span::new(start, self.last_end().max(start)),
        }
    }

    // ## Items

    fn item(&mut self) -> Result<Item> {
        self.expect(Tok::Dollar)?;
        let (name, span) = self.ident()?;
        self.expect(Tok::Colon)?;
        let level_params = self.level_params()?;
        let ty = self.expr()?;
        let body = if self.eat(&Tok::Equals) {
            if self.at_keyword("data") {
                self.next();
                ItemBody::Data(self.constructors()?)
            } else {
                ItemBody::Definition(self.expr()?)
            }
        } else {
            ItemBody::Axiom
        };
        Ok(Item {
            name,
            span,
            level_params,
            ty,
            body,
        })
    }

    fn constructors(&mut self) -> Result<Vec<RawConstructor>> {
        let mut ctors = Vec::new();
        while self.eat(&Tok::Pipe) {
            let (name, span) = self.ident()?;
            let mut fields = Vec::new();
            loop {
                if self.named_fields_ahead() {
                    fields.extend(self.binder_group()?);
                } else if self.starts_atom() {
                    fields.push(("_".into(), self.atom()?));
                } else {
                    break;
                }
            }
            ctors.push(RawConstructor { name, span, fields });
        }
        Ok(ctors)
    }

    /// `(x y: A)` as a constructor's named fields.
    fn named_fields_ahead(&self) -> bool {
        if self.peek(0) != &Tok::LParen {
            return false;
        }
        let mut n = 1;
        while matches!(self.peek(n), Tok::Ident(_)) {
            n += 1;
        }
        n > 1 && self.peek(n) == &Tok::Colon
    }

    /// A signature's leading `@ctx {u: @l, ...} ->`.
    fn level_params(&mut self) -> Result<Vec<(Name, Span)>> {
        let mut params = Vec::new();
        if !self.at_keyword("ctx") {
            return Ok(params);
        }
        self.next();
        self.expect(Tok::LBrace)?;
        loop {
            params.push(self.ident()?);
            self.expect(Tok::Colon)?;
            if !self.at_keyword("l") {
                return Err(self.unexpected("`@l` (the kernel's `@ctx` binds levels only)"));
            }
            self.next();
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        self.expect(Tok::Arrow)?;
        Ok(params)
    }

    // ## Expressions

    fn expr(&mut self) -> Result<Raw> {
        let start = self.span().start;
        match self.peek(0) {
            Tok::Backslash => self.lambda(),
            Tok::Let => self.let_in(),
            _ if self.binder_group_ahead() => {
                let binders = self.binder_group()?;
                let pi = self.eat(&Tok::Arrow);
                if !pi {
                    self.expect(Tok::Star)?;
                }
                // `(x: A) * B -> C` is `((x: A) * B) -> C`.
                let cod = if pi { self.expr()? } else { self.product()? };
                let built = binders.into_iter().rev().fold(cod, |cod, (name, ty)| {
                    let kind = if pi {
                        RawKind::Pi(name, Box::new(ty), Box::new(cod))
                    } else {
                        RawKind::Sigma(name, Box::new(ty), Box::new(cod))
                    };
                    self.raw(kind, start)
                });
                self.arrow_tail(built, start)
            }
            _ => {
                let dom = self.product()?;
                self.arrow_tail(dom, start)
            }
        }
    }

    fn arrow_tail(&mut self, dom: Raw, start: usize) -> Result<Raw> {
        if self.eat(&Tok::Arrow) {
            let cod = self.expr()?;
            Ok(self.raw(RawKind::Pi("_".into(), Box::new(dom), Box::new(cod)), start))
        } else {
            Ok(dom)
        }
    }

    /// `A * B * C`, right-nested, binding tighter than `->`.
    fn product(&mut self) -> Result<Raw> {
        let start = self.span().start;
        if self.binder_group_ahead() {
            let binders = self.binder_group()?;
            self.expect(Tok::Star)?;
            let cod = self.product()?;
            return Ok(binders.into_iter().rev().fold(cod, |cod, (name, ty)| {
                self.raw(RawKind::Sigma(name, Box::new(ty), Box::new(cod)), start)
            }));
        }
        let first = self.equality()?;
        if self.eat(&Tok::Star) {
            let rest = self.product()?;
            Ok(self.raw(RawKind::Sigma("_".into(), Box::new(first), Box::new(rest)), start))
        } else {
            Ok(first)
        }
    }

    /// `(x y: A)` followed by `->` or `*`, told apart from a parenthesized term or
    /// an ascription by scanning to the matching parenthesis.
    fn binder_group_ahead(&self) -> bool {
        if self.peek(0) != &Tok::LParen || !matches!(self.peek(1), Tok::Ident(_)) {
            return false;
        }
        let mut n = 1;
        while matches!(self.peek(n), Tok::Ident(_)) {
            n += 1;
        }
        if self.peek(n) != &Tok::Colon {
            return false;
        }
        let mut depth = 0usize;
        loop {
            match self.peek(n) {
                Tok::LParen => depth += 1,
                Tok::RParen if depth == 0 => return matches!(self.peek(n + 1), Tok::Arrow | Tok::Star),
                Tok::RParen => depth -= 1,
                Tok::Eof => return false,
                _ => {}
            }
            n += 1;
        }
    }

    /// `(x y: A)`: each name with its own copy of the type.
    fn binder_group(&mut self) -> Result<Vec<(Name, Raw)>> {
        self.expect(Tok::LParen)?;
        let mut names = vec![self.ident()?.0];
        while let Tok::Ident(_) = self.peek(0) {
            names.push(self.ident()?.0);
        }
        self.expect(Tok::Colon)?;
        let ty = self.expr()?;
        self.expect(Tok::RParen)?;
        Ok(names.into_iter().map(|n| (n, ty.clone())).collect())
    }

    fn lambda(&mut self) -> Result<Raw> {
        let start = self.span().start;
        self.expect(Tok::Backslash)?;
        let mut binders: Vec<(Name, Option<Raw>)> = Vec::new();
        loop {
            match self.peek(0) {
                Tok::Ident(_) => binders.push((self.ident()?.0, None)),
                Tok::LParen => {
                    for (name, ty) in self.binder_group()? {
                        binders.push((name, Some(ty)));
                    }
                }
                _ => break,
            }
        }
        if binders.is_empty() {
            return Err(self.unexpected("a binder"));
        }
        self.expect(Tok::FatArrow)?;
        let body = self.expr()?;
        Ok(binders.into_iter().rev().fold(body, |body, (name, ty)| {
            self.raw(RawKind::Lam(name, ty.map(Box::new), Box::new(body)), start)
        }))
    }

    fn let_in(&mut self) -> Result<Raw> {
        let start = self.span().start;
        self.expect(Tok::Let)?;
        let (name, _) = self.ident()?;
        let ty = if self.eat(&Tok::Colon) { Some(Box::new(self.expr()?)) } else { None };
        self.expect(Tok::Equals)?;
        let val = self.expr()?;
        self.expect(Tok::In)?;
        let body = self.expr()?;
        Ok(self.raw(RawKind::Let(name, ty, Box::new(val), Box::new(body)), start))
    }

    /// `a ~ b`, non-associative, binding tighter than `*`.
    fn equality(&mut self) -> Result<Raw> {
        let start = self.span().start;
        let a = self.app()?;
        if self.eat(&Tok::Tilde) {
            let b = self.app()?;
            Ok(self.raw(RawKind::Eq(Box::new(a), Box::new(b)), start))
        } else {
            Ok(a)
        }
    }

    fn starts_atom(&self) -> bool {
        match self.peek(0) {
            Tok::Ident(_) | Tok::LParen => true,
            Tok::At(w) => !matches!(&**w, "ctx" | "l" | "data"),
            _ => false,
        }
    }

    fn app(&mut self) -> Result<Raw> {
        let start = self.span().start;
        let mut f = self.atom()?;
        loop {
            if self.starts_atom() {
                let a = self.atom()?;
                f = self.raw(RawKind::App(Box::new(f), Box::new(a)), start);
            } else if matches!(self.peek(0), Tok::Backslash | Tok::Let) {
                // A trailing lambda or let is the last argument.
                let a = self.expr()?;
                f = self.raw(RawKind::App(Box::new(f), Box::new(a)), start);
                break;
            } else {
                break;
            }
        }
        Ok(f)
    }

    fn atom(&mut self) -> Result<Raw> {
        let start = self.span().start;
        let mut e = self.atom_head()?;
        while self.peek(0) == &Tok::Dot {
            let kind = match self.peek(1) {
                Tok::Num(1) => RawKind::Fst(Box::new(e)),
                Tok::Num(2) => RawKind::Snd(Box::new(e)),
                _ => {
                    self.next();
                    return Err(self.unexpected("`1` or `2` (a pair's projections)"));
                }
            };
            self.next();
            self.next();
            e = self.raw(kind, start);
        }
        Ok(e)
    }

    fn atom_head(&mut self) -> Result<Raw> {
        let start = self.span().start;
        match self.peek(0).clone() {
            Tok::Ident(_) => {
                let (name, _) = self.ident()?;
                let levels = self.level_args()?;
                Ok(self.raw(RawKind::Var(name, levels), start))
            }
            Tok::LParen => {
                self.next();
                let e = self.expr()?;
                let kind = if self.eat(&Tok::Colon) {
                    let ty = self.expr()?;
                    RawKind::Ann(Box::new(e), Box::new(ty))
                } else if self.eat(&Tok::Comma) {
                    let second = self.expr()?;
                    RawKind::Pair(Box::new(e), Box::new(second))
                } else {
                    self.expect(Tok::RParen)?;
                    return Ok(e);
                };
                self.expect(Tok::RParen)?;
                Ok(self.raw(kind, start))
            }
            Tok::At(word) if &*word == "Type" => {
                self.next();
                let level = self.level_atom()?;
                Ok(self.raw(RawKind::Sort(level), start))
            }
            Tok::At(word) => match PRIMITIVES.iter().find(|(w, ..)| *w == &*word) {
                Some(&(_, prim, arity)) => {
                    self.next();
                    let mut args = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        args.push(self.atom()?);
                    }
                    Ok(self.raw(RawKind::Prim(prim, args), start))
                }
                None => Err(error(self.src, Code::UnknownSymbol, self.span(), format!("unknown primitive `@{word}`"))),
            },
            _ => Err(self.unexpected("a term")),
        }
    }

    /// `@ctx {.u = l, ...}` after a name.
    fn level_args(&mut self) -> Result<Vec<LevelArg>> {
        let mut args = Vec::new();
        if !self.at_keyword("ctx") {
            return Ok(args);
        }
        self.next();
        self.expect(Tok::LBrace)?;
        loop {
            let start = self.expect(Tok::Dot)?.start;
            let (name, _) = self.ident()?;
            self.expect(Tok::Equals)?;
            let level = self.level()?;
            args.push(LevelArg {
                name,
                level,
                span: Span::new(start, self.last_end()),
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        Ok(args)
    }

    // ## Levels

    fn level(&mut self) -> Result<RawLevel> {
        if self.at_keyword("max") || self.at_keyword("imax") {
            let is_max = self.at_keyword("max");
            self.next();
            let a = Box::new(self.level_atom()?);
            let b = Box::new(self.level_atom()?);
            return Ok(if is_max { RawLevel::Max(a, b) } else { RawLevel::IMax(a, b) });
        }
        let mut l = self.level_atom()?;
        while self.eat(&Tok::Plus) {
            match self.next() {
                Tok::Num(n) => l = RawLevel::Plus(Box::new(l), n),
                _ => {
                    self.pos -= 1;
                    return Err(self.unexpected("a number"));
                }
            }
        }
        Ok(l)
    }

    fn level_atom(&mut self) -> Result<RawLevel> {
        let span = self.span();
        match self.next() {
            Tok::Num(n) => Ok(RawLevel::Num(n)),
            Tok::Ident(n) => Ok(RawLevel::Name(n, span)),
            Tok::LParen => {
                let l = self.level()?;
                self.expect(Tok::RParen)?;
                Ok(l)
            }
            _ => {
                self.pos -= 1;
                Err(self.unexpected("a level"))
            }
        }
    }
}
