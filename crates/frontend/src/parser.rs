//! A Pratt (precedence-climbing) parser.
//!
//! Structure mirrors the grammar so the Rust call stack matches it, which keeps
//! error context meaningful:
//!
//! * [`Parser::parse_expr`] is the precedence-climbing core: it parses a prefix,
//!   then folds infix operators and juxtaposition (application) by binding power
//!   from [`crate::parser::table`].
//! * [`Parser::parse_primary`] parses one atom then a left-associative postfix
//!   `.` chain (field access, tuple index, struct/variant literals,
//!   module-qualified names) that binds tighter than application.
//! * The control forms (`let`, `if`, `when`, `\`, `with`, `do`, `defer`) are
//!   primaries, each with its own `parse_*` method.
//!
//! Nodes are built into the [`Ast`] stores as they are parsed (bottom-up), so a
//! parse method returns an [`Aol`] handle, not a reference. Names are interned to
//! [`StrId`]. Sugar (`;`, `|>`, `<|`, `::`, `[..]`) is preserved as explicit AST
//! nodes; a later pass desugars it.

pub mod data;
mod table;
#[cfg(test)]
mod tests;

use crate::lexer::data::{Kind, Token};
use crate::lexer::Lexer;
use utilities::{Aol, Line, Span, StrId};
use utilities::{Code, Diagnostic, Result};

use crate::parser::data::*;

pub struct Parser<'a> {
    lex: Lexer<'a>,
    ast: Ast,
    /// The (read-only) source, so a token's lexeme is `src[token.span]`. Tokens
    /// themselves carry only spans; this is where they resolve.
    src: &'a str,
    /// End offset of the most recently consumed token, so a node's span can run
    /// from its first token's start to here.
    last_end: usize,
}

/// Consume a token of an expected kind, or fail with an `UnexpectedToken`
/// diagnostic naming what was wanted. Evaluates to the consumed [`Token`].
macro_rules! expect {
    ($self:ident, $pat:pat, $what:expr) => {{
        let t = $self.peek()?;
        if !matches!(t.kind, $pat) {
            return Err($self.unexpected(&t, $what));
        }
        $self.bump()?
    }};
}

impl<'a> Parser<'a> {
    /// Build a parser that fills `ast`. Passing an existing [`Ast`] lets several
    /// modules of one compilation share stores, so a handle minted for one module
    /// resolves in every module (needed for cross-module type imports).
    pub fn new(lex: Lexer<'a>, ast: Ast) -> Parser<'a> {
        let src = lex.source();
        Parser {
            lex,
            ast,
            src,
            last_end: 0,
        }
    }

    /// Consume the parser, yielding the filled store bundle.
    pub fn into_ast(self) -> Ast {
        self.ast
    }

    // -- store + token helpers ----------------------------------------------

    fn expr(&mut self, e: Expr) -> Aol<Expr> {
        self.ast.exprs.create(e)
    }
    /// Record `node`'s source span as running from `start` to the end of the most
    /// recently consumed token, then return it (builder-style, for diagnostics).
    fn stamp(&mut self, start: usize, node: Aol<Expr>) -> Aol<Expr> {
        self.ast.expr_spans.insert(node, Span::new(start, self.last_end));
        node
    }
    /// Like [`stamp`](Self::stamp), for a `Ty` node.
    fn stamp_ty(&mut self, start: usize, node: Aol<Ty>) -> Aol<Ty> {
        self.ast.ty_spans.insert(node, Span::new(start, self.last_end));
        node
    }
    /// The start offset of the next token, marking where a node begins.
    fn here(&mut self) -> Result<usize> {
        Ok(self.peek()?.span.start)
    }
    fn ty(&mut self, t: Ty) -> Aol<Ty> {
        self.ast.tys.create(t)
    }
    fn pat(&mut self, p: Pattern) -> Aol<Pattern> {
        self.ast.pats.create(p)
    }
    /// A token's source lexeme, `src[t.span]`. The returned slice is tied to the
    /// source (`'a`), not to `&self`, so it can be handed straight to a `&mut
    /// self` method like [`intern`](Self::intern) without a borrow clash.
    fn text(&self, t: Token) -> &'a str {
        let src: &'a str = self.src;
        &src[t.span.start..t.span.end]
    }
    /// The identifier of an `@name` token, past the leading `@`.
    fn intrinsic_name(&self, t: Token) -> &'a str {
        debug_assert!(matches!(t.kind, Kind::At), "intrinsic_name on non-@ token");
        &self.text(t)[1..]
    }
    fn intern(&mut self, s: &str) -> StrId {
        self.ast.strings.intern(s)
    }
    /// Decode a `Kind::Str` token's literal (escapes and all) into interned
    /// bytes. Decoding is deferred to here so the lexer stays borrow-free. Used
    /// for non-interpolating strings (patterns, `@extern` operands).
    fn intern_str(&mut self, t: Token) -> Result<StrId> {
        let bytes = crate::lexer::decode_string(self.text(t), t.span.start, t.line)?;
        Ok(self.ast.strings.intern_bytes(&bytes))
    }
    /// A `Str` literal expression from raw decoded bytes.
    fn str_expr(&mut self, bytes: &[u8]) -> Aol<Expr> {
        let s = self.ast.strings.intern_bytes(bytes);
        self.expr(Expr::Str(s))
    }

    /// Build a string-literal expression, expanding `{expr}` interpolations. `t`
    /// is the `Kind::Str` token. `"a {e} b"` desugars to `"a " ++ to_string e ++ " b"`;
    /// a literal chunk seeds the `++` chain so the whole expression types as `Str`,
    /// and each interpolant is stringified through the overloaded `to_string`.
    /// `\{`/`\}` are literal braces; a bare `}` is literal too.
    fn build_string(&mut self, t: Token) -> Result<Aol<Expr>> {
        let raw = self.text(t);
        let bytes = raw.as_bytes();
        let inner = &bytes[1..bytes.len() - 1];
        let body_start = t.span.start + 1; // absolute offset of `inner[0]`

        let mut segs: Vec<Aol<Expr>> = Vec::new();
        let mut chunk: Vec<u8> = Vec::new();
        let mut interpolated = false;
        let mut i = 0;
        while i < inner.len() {
            match inner[i] {
                b'\\' => i = crate::lexer::decode_escape(inner, i, body_start, t.line, &mut chunk)?,
                b'{' => {
                    interpolated = true;
                    let seg = self.str_expr(&chunk);
                    segs.push(seg);
                    chunk.clear();
                    // Balance nested `{}` to find this interpolation's close,
                    // skipping nested strings so their braces don't miscount.
                    let expr_start = i + 1;
                    let mut depth = 1usize;
                    let mut j = expr_start;
                    while j < inner.len() {
                        match inner[j] {
                            b'"' => {
                                j += 1;
                                while j < inner.len() && inner[j] != b'"' {
                                    j += if inner[j] == b'\\' { 2 } else { 1 };
                                }
                                j += 1; // past the closing quote
                            }
                            b'{' => {
                                depth += 1;
                                j += 1;
                            }
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                j += 1;
                            }
                            _ => j += 1,
                        }
                    }
                    if depth != 0 {
                        let at = body_start + i;
                        return Err(Diagnostic::error(
                            Code::UnexpectedToken,
                            Span::new(at, at + 1),
                            t.line,
                            "string interpolation '{' is not closed with '}'".to_string(),
                        ));
                    }
                    let full: &'a str = self.src;
                    let abs_start = body_start + expr_start;
                    let slice = &full[abs_start..body_start + j];
                    let e = self.parse_subexpr(slice, abs_start, t.line)?;
                    // `{e}` stringifies via the overloaded `to_string`, so an
                    // interpolant of any type with a `to_string` (base types ship
                    // one in the auto-imported `CORE`) reads as `Str`. The call
                    // inherits the interpolant's span so a resolution failure
                    // points at the interpolant, not a synthetic node.
                    let span = self.ast.expr_span(e).unwrap_or(t.span);
                    let to_string = self.intern("to_string");
                    let f = self.expr(Expr::Var {
                        module: None,
                        name: to_string,
                    });
                    let call = self.expr(Expr::App(f, e));
                    self.ast.expr_spans.insert(call, span);
                    segs.push(call);
                    i = j + 1; // past '}'
                }
                c => {
                    chunk.push(c);
                    i += 1;
                }
            }
        }
        let seg = self.str_expr(&chunk);
        segs.push(seg);

        if !interpolated {
            return Ok(segs.pop().expect("at least the whole-string chunk"));
        }
        let concat = self.intern("++");
        let mut acc = segs[0];
        for &rhs in &segs[1..] {
            acc = self.expr(Expr::BinOp {
                op: concat,
                lhs: acc,
                rhs,
            });
        }
        Ok(acc)
    }

    /// Parse a single expression from a sub-slice of the source (a string
    /// interpolant's `{...}` body), re-lexing it with absolute spans. Restores
    /// the outer token stream afterwards, even on error.
    fn parse_subexpr(&mut self, slice: &'a str, base: usize, line: Line) -> Result<Aol<Expr>> {
        let saved = std::mem::replace(&mut self.lex, Lexer::sub(slice, base, line));
        let saved_end = self.last_end;
        let out = self.parse_expr(0).and_then(|e| {
            if matches!(self.peek_kind()?, Kind::Eof) {
                Ok(e)
            } else {
                let t = self.peek()?;
                Err(self.unexpected(&t, "expected a single expression in the interpolation"))
            }
        });
        self.lex = saved;
        self.last_end = saved_end;
        out
    }
    /// Consume the next token and intern its text (the token is a checked word).
    fn bump_word(&mut self) -> Result<StrId> {
        let t = self.bump()?;
        Ok(self.intern(self.text(t)))
    }
    /// Expect a `Word` and intern it. Combines `expect!` + `intern` so the token
    /// is bound to a local first (a nested `self.intern(self.bump()?...)` would
    /// mutably borrow `self` twice in one expression).
    fn expect_word(&mut self, what: &str) -> Result<StrId> {
        let t = expect!(self, Kind::Word, what);
        Ok(self.intern(self.text(t)))
    }
    /// Consume a lowercase-initial type variable name and intern it.
    fn expect_tyvar(&mut self, what: &str) -> Result<StrId> {
        let t = expect!(self, Kind::Word, what);
        if !self.text(t).starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(self.unexpected(&t, "a type variable must start with a lowercase letter"));
        }
        Ok(self.intern(self.text(t)))
    }
    /// Is the next token a lowercase-initial word, i.e. a type variable?
    fn at_tyvar(&mut self) -> Result<bool> {
        let t = self.peek()?;
        Ok(matches!(t.kind, Kind::Word) && self.text(t).starts_with(|c: char| c.is_ascii_lowercase()))
    }
    /// The optional type parameters after a `@struct`/`@union`/`@codata` keyword
    /// and before `=`: `@struct a b = ...`. Empty when omitted (the parameters are
    /// then inferred from the free type variables in the body).
    fn parse_type_params(&mut self) -> Result<Box<[StrId]>> {
        let mut params = Vec::new();
        while self.at_tyvar()? {
            params.push(self.expect_tyvar("expected a type parameter")?);
        }
        Ok(params.into_boxed_slice())
    }

    /// If `base` is a bare, unqualified variable, its interned name.
    fn bare_var_name(&self, base: Aol<Expr>) -> Option<StrId> {
        match self.ast.exprs.lookup(base) {
            Expr::Var { module: None, name } => Some(*name),
            _ => None,
        }
    }

    fn peek(&mut self) -> Result<Token> {
        self.lex.peek(0)
    }
    fn peek_at(&mut self, n: usize) -> Result<Token> {
        self.lex.peek(n)
    }
    fn peek_kind(&mut self) -> Result<Kind> {
        Ok(self.lex.peek(0)?.kind)
    }
    fn bump(&mut self) -> Result<Token> {
        let t = self.lex.next_token()?;
        self.last_end = t.span.end;
        Ok(t)
    }
    /// Consume the next token if it matches `pred`; report whether it did.
    fn eat(&mut self, pred: impl Fn(Kind) -> bool) -> Result<bool> {
        if pred(self.peek()?.kind) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    /// Is the next token the operator with lexeme `s`?
    fn at_op(&mut self, s: &str) -> Result<bool> {
        let t = self.peek()?;
        Ok(matches!(t.kind, Kind::Op) && self.text(t) == s)
    }
    fn unexpected(&self, t: &Token, what: &str) -> Diagnostic {
        Diagnostic::error(
            Code::UnexpectedToken,
            t.span,
            t.line,
            format!("{what}, found {}", describe(t, self.text(*t))),
        )
    }

    /// A type name must start with a capital letter; a lowercase name in type
    /// position is a type variable. `tok` is the offending name token, for the
    /// error span.
    fn require_type_capital(&self, text: &str, tok: &Token) -> Result<()> {
        if text.starts_with(|c: char| c.is_ascii_uppercase()) {
            Ok(())
        } else {
            Err(self.unexpected(
                tok,
                "a type name must start with a capital letter (a lowercase name is a type variable)",
            ))
        }
    }

    // -- program + globals --------------------------------------------------

    /// Parse a whole compilation unit.
    pub fn parse_program(&mut self) -> Result<Program> {
        let at = expect!(self, Kind::At, "expected '@mod' at the start of the file");
        if self.intrinsic_name(at) != "mod" {
            return Err(self.unexpected(&at, "expected '@mod' at the start of the file"));
        }
        let name = expect!(self, Kind::Word, "expected a module name after '@mod'");
        let module = self.intern(self.text(name));

        let mut items = Vec::new();
        while !matches!(self.peek_kind()?, Kind::Eof) {
            items.push(self.parse_global()?);
        }
        Ok(Program {
            module,
            items: items.into_boxed_slice(),
        })
    }

    fn parse_global(&mut self) -> Result<Item> {
        expect!(
            self,
            Kind::Dollar,
            "expected a global declaration starting with '$'"
        );
        let t = self.peek()?;
        match t.kind {
            Kind::With => self.parse_import(),
            Kind::At => self.parse_directive(t),
            Kind::Word => self.parse_named_global(),
            _ => Err(self.unexpected(&t, "expected a name or directive after '$'")),
        }
    }

    /// A `$ @...` directive: visibility, assert, run, or an operator definition.
    fn parse_directive(&mut self, at: Token) -> Result<Item> {
        match self.intrinsic_name(at) {
            "private" => {
                self.bump()?;
                Ok(Item::Visibility(Visibility::Private))
            }
            "public" => {
                self.bump()?;
                Ok(Item::Visibility(Visibility::Public))
            }
            "assert" => {
                self.bump()?;
                Ok(Item::Assert(self.parse_expr(0)?))
            }
            "run" => {
                self.bump()?;
                Ok(Item::Run(self.parse_expr(0)?))
            }
            "operator" => self.parse_operator_def(),
            other => Err(self.unexpected(&at, &format!("'@{other}' is not a valid '$' directive"))),
        }
    }

    /// `$ @operator.{ op } : ty = expr`
    fn parse_operator_def(&mut self) -> Result<Item> {
        self.bump()?; // '@operator'
        expect!(self, Kind::Dot, "expected '.{' after '@operator'");
        expect!(self, Kind::LBrace, "expected '{' after '@operator.'");
        let op_tok = expect!(self, Kind::Op, "expected an operator between the braces");
        let op = self.intern(self.text(op_tok));
        expect!(self, Kind::RBrace, "expected '}' after the operator");
        expect!(
            self,
            Kind::Colon,
            "expected ':' and a type for the operator"
        );
        let sig = self.parse_type()?;
        expect!(
            self,
            Kind::Eq,
            "expected '=' before the operator's definition"
        );
        let body = self.parse_expr(0)?;
        Ok(Item::OperatorDef { op, sig, body })
    }

    /// `$ with module [= rename]`
    fn parse_import(&mut self) -> Result<Item> {
        self.bump()?; // 'with'
        let module = self.parse_dotted_name()?;
        let rename = if self.eat(|k| matches!(k, Kind::Eq))? {
            Some(self.parse_dotted_name()?)
        } else {
            None
        };
        Ok(Item::Import { module, rename })
    }

    fn parse_dotted_name(&mut self) -> Result<Box<[StrId]>> {
        let first = expect!(self, Kind::Word, "expected a module name");
        let mut names = vec![self.intern(self.text(first))];
        while matches!(self.peek_kind()?, Kind::Dot) {
            self.bump()?;
            let part = expect!(self, Kind::Word, "expected a name after '.'");
            names.push(self.intern(self.text(part)));
        }
        Ok(names.into_boxed_slice())
    }

    /// `$ name ...`: a value definition, or a struct/union/alias/effect type.
    fn parse_named_global(&mut self) -> Result<Item> {
        let name_tok = self.bump()?;
        let name = self.intern(self.text(name_tok));
        if !self.eat(|k| matches!(k, Kind::Colon))? {
            expect!(self, Kind::Eq, "expected ':' or '=' after the name");
            return Ok(Item::Def {
                name,
                sig: None,
                implicits: Box::from([]),
                body: self.parse_expr(0)?,
            });
        }
        // After `name :`, an `@struct`/`@union`/`@alias`/`@effect` keyword opens a
        // type declaration; anything else is a type signature on a value.
        if let Kind::At = self.peek_kind()? {
            let at_tok = self.peek()?;
            let kw = self.intrinsic_name(at_tok);
            if matches!(kw, "struct" | "union" | "alias" | "effect" | "codata") {
                self.require_type_capital(self.text(name_tok), &name_tok)?;
            }
            match kw {
                "struct" => {
                    self.bump()?;
                    let params = self.parse_type_params()?;
                    expect!(self, Kind::Eq, "expected '=' after '@struct'");
                    let (includes, fields) = self.parse_struct_body()?;
                    return Ok(Item::Struct {
                        name,
                        params,
                        includes,
                        fields,
                    });
                }
                "union" => {
                    self.bump()?;
                    let params = self.parse_type_params()?;
                    expect!(self, Kind::Eq, "expected '=' after '@union'");
                    let (includes, variants) = self.parse_union_body()?;
                    return Ok(Item::Union {
                        name,
                        params,
                        includes,
                        variants,
                    });
                }
                "alias" => {
                    self.bump()?;
                    let params = self.parse_type_params()?;
                    expect!(self, Kind::Eq, "expected '=' after '@alias'");
                    let ty = self.parse_type()?;
                    return Ok(Item::Alias { name, params, ty });
                }
                "effect" => {
                    self.bump()?;
                    expect!(self, Kind::Eq, "expected '=' after '@effect'");
                    let ops = self.parse_field_decls()?;
                    return Ok(Item::Effect { name, ops });
                }
                "codata" => {
                    self.bump()?;
                    let params = self.parse_type_params()?;
                    expect!(self, Kind::Eq, "expected '=' after '@codata'");
                    let observations = self.parse_field_decls()?;
                    return Ok(Item::Codata {
                        name,
                        params,
                        observations,
                    });
                }
                _ => {} // an @tycon type signature; fall through
            }
        }
        let sig = Some(self.parse_type()?);
        let implicits = self.parse_ctx_decls()?;
        expect!(self, Kind::Eq, "expected '=' after the type signature");
        let body = self.parse_expr(0)?;
        Ok(Item::Def {
            name,
            sig,
            implicits,
            body,
        })
    }

    /// Parse the `@ctx` declarations that may follow a definition's type
    /// signature: `@ctx name : Type` (repeatable) or `@ctx { a : A, b : B }`.
    /// Each becomes an implicit parameter resolved by name at the call site.
    fn parse_ctx_decls(&mut self) -> Result<Box<[FieldDecl]>> {
        let mut decls = Vec::new();
        while self.at_ctx()? {
            self.bump()?; // '@ctx'
            if self.eat(|k| matches!(k, Kind::LBrace))? {
                for d in self.parse_field_decls()?.into_vec() {
                    decls.push(d);
                }
                expect!(self, Kind::RBrace, "expected '}' to close the '@ctx' block");
            } else {
                let name = self.expect_word("expected an implicit parameter name after '@ctx'")?;
                expect!(self, Kind::Colon, "expected ':' after the '@ctx' name");
                let ty = self.parse_type()?;
                decls.push(FieldDecl { name, ty });
            }
        }
        Ok(decls.into_boxed_slice())
    }

    /// Is the next token the `@ctx` keyword?
    fn at_ctx(&mut self) -> Result<bool> {
        let t = self.peek()?;
        Ok(matches!(t.kind, Kind::At) && self.intrinsic_name(t) == "ctx")
    }

    /// Parse a postfix `@ctx` override on `callee`: a single positional value
    /// (`@ctx e`, an atom) or a record `@ctx { .name = e, ..., .. }` where a
    /// trailing `..` fills the unmentioned implicits by name.
    fn parse_ctx_override(&mut self, start: usize, callee: Aol<Expr>) -> Result<Aol<Expr>> {
        self.bump()?; // '@ctx'
        let mut overrides = Vec::new();
        let mut rest = false;
        if matches!(self.peek_kind()?, Kind::LBrace) {
            self.bump()?; // '{'
            while !matches!(self.peek_kind()?, Kind::RBrace) {
                if matches!(self.peek_kind()?, Kind::Dot)
                    && matches!(self.peek_kind_at(1)?, Kind::Dot)
                {
                    self.bump()?;
                    self.bump()?; // '..'
                    rest = true;
                    break;
                }
                overrides.push(self.parse_field_init()?);
                if !self.eat(|k| matches!(k, Kind::Comma))? {
                    break;
                }
            }
            expect!(
                self,
                Kind::RBrace,
                "expected '}' to close the '@ctx' overrides"
            );
        } else {
            overrides.push(FieldInit::Positional(self.parse_primary()?));
        }
        let node = self.expr(Expr::Ctx {
            callee,
            overrides: overrides.into_boxed_slice(),
            rest,
        });
        Ok(self.stamp(start, node))
    }

    /// Comma-separated `name : Type` declarations (struct fields, effect ops).
    fn parse_field_decls(&mut self) -> Result<Box<[FieldDecl]>> {
        let mut fields = Vec::new();
        while matches!(self.peek_kind()?, Kind::Word) {
            let name = self.bump_word()?;
            expect!(self, Kind::Colon, "expected ':' after the field name");
            let ty = self.parse_type()?;
            fields.push(FieldDecl { name, ty });
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        Ok(fields.into_boxed_slice())
    }

    /// A struct body: leading `with Other` clauses (copied-in fields) then the
    /// declared `name : Type` fields, comma-separated and freely interleaved.
    fn parse_struct_body(&mut self) -> Result<(Box<[StrId]>, Box<[FieldDecl]>)> {
        let mut includes = Vec::new();
        let mut fields = Vec::new();
        loop {
            match self.peek_kind()? {
                Kind::With => includes.push(self.parse_with_include()?),
                Kind::Word => {
                    let name = self.bump_word()?;
                    expect!(self, Kind::Colon, "expected ':' after the field name");
                    let ty = self.parse_type()?;
                    fields.push(FieldDecl { name, ty });
                }
                _ => break,
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        Ok((includes.into_boxed_slice(), fields.into_boxed_slice()))
    }

    fn parse_union_body(&mut self) -> Result<(Box<[StrId]>, Box<[VariantDecl]>)> {
        let mut includes = Vec::new();
        let mut variants = Vec::new();
        loop {
            match self.peek_kind()? {
                Kind::With => includes.push(self.parse_with_include()?),
                Kind::Word => {
                    let tag = self.bump_word()?;
                    let payload = if self.eat(|k| matches!(k, Kind::Colon))? {
                        self.parse_payload()?
                    } else {
                        Payload::None
                    };
                    variants.push(VariantDecl { tag, payload });
                }
                _ => break,
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        Ok((includes.into_boxed_slice(), variants.into_boxed_slice()))
    }

    /// A `with Other` clause inside a type declaration: the (capitalized) name of a
    /// same-kind type whose fields/variants are copied into the one declared. Pure
    /// splicing, no type relationship.
    fn parse_with_include(&mut self) -> Result<StrId> {
        self.bump()?; // 'with'
        let t = expect!(self, Kind::Word, "expected a type name after 'with'");
        self.require_type_capital(self.text(t), &t)?;
        Ok(self.intern(self.text(t)))
    }

    fn parse_payload(&mut self) -> Result<Payload> {
        if !matches!(self.peek_kind()?, Kind::LBrace) {
            return Ok(Payload::Bare(self.parse_type()?));
        }
        self.bump()?; // '{'
        let mut fields = Vec::new();
        while !matches!(self.peek_kind()?, Kind::RBrace) {
            // A named field is `name : Type`; anything else is a positional type.
            let named = matches!(self.peek_kind()?, Kind::Word)
                && matches!(self.peek_kind_at(1)?, Kind::Colon);
            if named {
                let name = self.bump_word()?;
                self.bump()?; // ':'
                let ty = self.parse_type()?;
                fields.push(PayloadField {
                    name: Some(name),
                    ty,
                });
            } else {
                let ty = self.parse_type()?;
                fields.push(PayloadField { name: None, ty });
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        expect!(
            self,
            Kind::RBrace,
            "expected '}' to close the variant payload"
        );
        Ok(Payload::Fields(fields.into_boxed_slice()))
    }

    fn peek_kind_at(&mut self, n: usize) -> Result<Kind> {
        Ok(self.lex.peek(n)?.kind)
    }

    // -- types --------------------------------------------------------------

    fn parse_type(&mut self) -> Result<Aol<Ty>> {
        let from = self.parse_type_app()?;
        if !matches!(self.peek_kind()?, Kind::Arrow) {
            return Ok(from);
        }
        self.bump()?; // '->'
        let effect = self.parse_effect_row_opt()?;
        let to = self.parse_type()?; // right-associative
        Ok(self.ty(Ty::Arrow { from, effect, to }))
    }

    fn parse_type_app(&mut self) -> Result<Aol<Ty>> {
        let mut head = self.parse_type_atom()?;
        while self.starts_type_atom()? {
            let arg = self.parse_type_atom()?;
            head = self.ty(Ty::App(head, arg));
        }
        Ok(head)
    }

    fn starts_type_atom(&mut self) -> Result<bool> {
        if self.at_tyvar()? {
            return Ok(true);
        }
        // `@ctx` ends a signature and opens its implicit-parameter clauses; it is
        // not a type atom, so type application must not swallow it.
        if self.at_ctx()? {
            return Ok(false);
        }
        Ok(matches!(
            self.peek_kind()?,
            Kind::Word | Kind::At | Kind::LParen | Kind::LBrace
        ))
    }

    fn parse_type_atom(&mut self) -> Result<Aol<Ty>> {
        let start = self.here()?;
        let node = self.parse_type_atom_inner()?;
        Ok(self.stamp_ty(start, node))
    }

    fn parse_type_atom_inner(&mut self) -> Result<Aol<Ty>> {
        if self.at_tyvar()? {
            let name = self.expect_tyvar("expected a type-variable name")?;
            return Ok(self.ty(Ty::Var(name)));
        }
        let t = self.peek()?;
        match t.kind {
            Kind::Word => {
                self.bump()?;
                if matches!(self.peek_kind()?, Kind::Dot) {
                    self.bump()?; // '.'
                    let member = expect!(self, Kind::Word, "expected a type name after '.'");
                    self.require_type_capital(self.text(member), &member)?;
                    let module = self.intern(self.text(t));
                    let name = self.intern(self.text(member));
                    Ok(self.ty(Ty::Con {
                        module: Some(module),
                        name,
                    }))
                } else {
                    self.require_type_capital(self.text(t), &t)?;
                    let name = self.intern(self.text(t));
                    Ok(self.ty(Ty::Con { module: None, name }))
                }
            }
            Kind::At => {
                self.bump()?;
                let name = self.intern(self.text(t));
                Ok(self.ty(Ty::Con { module: None, name }))
            }
            Kind::LParen => {
                self.bump()?;
                let inner = self.parse_type()?;
                expect!(self, Kind::RParen, "expected ')' to close the type");
                Ok(inner)
            }
            Kind::LBrace => self.parse_brace_type(),
            // `[size]elem`: a sized tensor type. `size` is a Nat literal or a
            // lowercase (Nat-kinded) variable; `elem` binds as one atom (nest
            // `[m][n]T`, parenthesize a compound `[n](List a)`).
            Kind::LBrack => {
                self.bump()?; // '['
                let size = self.parse_size()?;
                expect!(self, Kind::RBrack, "expected ']' to close the tensor size");
                let elem = self.parse_type_atom()?;
                Ok(self.ty(Ty::Sized { size, elem }))
            }
            _ => Err(self.unexpected(&t, "expected a type")),
        }
    }

    /// A tensor size expression: `+` of `*`-terms over Nat literals and size
    /// variables (`[n+m]`, `[2*n+1]`). `*` binds tighter than `+`; `( )` groups.
    fn parse_size(&mut self) -> Result<Aol<Ty>> {
        let mut lhs = self.parse_size_term()?;
        while self.at_op("+")? {
            self.bump()?;
            let rhs = self.parse_size_term()?;
            lhs = self.ty(Ty::SizeAdd(lhs, rhs));
        }
        Ok(lhs)
    }

    fn parse_size_term(&mut self) -> Result<Aol<Ty>> {
        let mut lhs = self.parse_size_atom()?;
        while self.at_op("*")? {
            self.bump()?;
            let rhs = self.parse_size_atom()?;
            lhs = self.ty(Ty::SizeMul(lhs, rhs));
        }
        Ok(lhs)
    }

    fn parse_size_atom(&mut self) -> Result<Aol<Ty>> {
        let t = self.peek()?;
        match t.kind {
            Kind::Int(v) if v >= 0 => {
                self.bump()?;
                Ok(self.ty(Ty::Nat(v as u64)))
            }
            Kind::Int(_) => Err(self.unexpected(&t, "a tensor size must be non-negative")),
            Kind::LParen => {
                self.bump()?;
                let inner = self.parse_size()?;
                expect!(self, Kind::RParen, "expected ')' in a tensor size");
                Ok(inner)
            }
            _ if self.at_tyvar()? => {
                let name = self.expect_tyvar("expected a size variable")?;
                Ok(self.ty(Ty::Var(name)))
            }
            _ => Err(self.unexpected(
                &t,
                "expected a size (a Nat literal, a lowercase variable, or `+`/`*` of them)",
            )),
        }
    }

    /// `{}` unit, `{A, B}` tuple, or `{x: A, y: B}` named-record parameter sugar.
    fn parse_brace_type(&mut self) -> Result<Aol<Ty>> {
        self.bump()?; // '{'
        if self.eat(|k| matches!(k, Kind::RBrace))? {
            return Ok(self.ty(Ty::Unit));
        }
        let is_record = matches!(self.peek_kind()?, Kind::With)
            || (matches!(self.peek_kind()?, Kind::Word)
                && matches!(self.peek_kind_at(1)?, Kind::Colon));
        if is_record {
            let mut fields = Vec::new();
            let mut tail = None;
            loop {
                let with = self.eat(|k| matches!(k, Kind::With))?;
                let name = self.expect_word("expected a record field name")?;
                expect!(self, Kind::Colon, "expected ':' after the field name");
                let ty = self.parse_type()?;
                fields.push(RecField { with, name, ty });
                // `{ x: A, y: B | r }`: `| r` opens the record with a row variable.
                if self.at_op("|")? {
                    self.bump()?;
                    tail = Some(self.expect_tyvar("expected a row variable after '|'")?);
                    break;
                }
                if !self.eat(|k| matches!(k, Kind::Comma))?
                    || matches!(self.peek_kind()?, Kind::RBrace)
                {
                    break;
                }
            }
            expect!(self, Kind::RBrace, "expected '}' to close the record type");
            Ok(self.ty(Ty::Record {
                fields: fields.into_boxed_slice(),
                tail,
            }))
        } else {
            let mut tys = Vec::new();
            loop {
                tys.push(self.parse_type()?);
                if !self.eat(|k| matches!(k, Kind::Comma))?
                    || matches!(self.peek_kind()?, Kind::RBrace)
                {
                    break;
                }
            }
            expect!(self, Kind::RBrace, "expected '}' to close the tuple type");
            Ok(self.ty(Ty::Tuple(tys.into_boxed_slice())))
        }
    }

    /// An optional effect row right after `->`: `<>`, `<e>`, `<A, B>`,
    /// `<A, B | e>`, or `<| e>`. Effect names are capitalized; a lowercase
    /// name is the row-polymorphic tail variable.
    fn parse_effect_row_opt(&mut self) -> Result<Option<EffectRow>> {
        if self.at_op("<>")? {
            self.bump()?;
            return Ok(Some(EffectRow {
                names: Box::new([]),
                tail: None,
            }));
        }
        if self.at_op("<|")? {
            self.bump()?;
            let tail = self.expect_tyvar("expected a type variable in the effect row")?;
            self.expect_op(">", "expected '>' to close the effect row")?;
            return Ok(Some(EffectRow {
                names: Box::new([]),
                tail: Some(tail),
            }));
        }
        if !self.at_op("<")? {
            return Ok(None);
        }
        self.bump()?; // '<'
        if self.at_tyvar()? {
            let tail = self.expect_tyvar("expected a type variable in the effect row")?;
            self.expect_op(">", "expected '>' to close the effect row")?;
            return Ok(Some(EffectRow {
                names: Box::new([]),
                tail: Some(tail),
            }));
        }
        let mut names = Vec::new();
        loop {
            let n = self.expect_word("expected an effect name")?;
            names.push(n);
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        let tail = if self.at_op("|")? {
            self.bump()?;
            Some(self.expect_tyvar("expected a type variable after '|'")?)
        } else {
            None
        };
        self.expect_op(">", "expected '>' to close the effect row")?;
        Ok(Some(EffectRow {
            names: names.into_boxed_slice(),
            tail,
        }))
    }

    fn expect_op(&mut self, s: &str, what: &str) -> Result<()> {
        if self.at_op(s)? {
            self.bump()?;
            Ok(())
        } else {
            let t = self.peek()?;
            Err(self.unexpected(&t, what))
        }
    }

    // -- expressions: the Pratt core ----------------------------------------

    fn parse_expr(&mut self, min_bp: u8) -> Result<Aol<Expr>> {
        let start = self.here()?;
        let mut lhs = self.parse_prefix()?;
        loop {
            let t = self.peek()?;
            // A postfix `@ctx` overrides the callee's implicit arguments. Handled
            // before the operand check because `@ctx` starts with `@` (an operand
            // starter) but must attach to `lhs`, not be applied as an argument.
            if self.at_ctx()? {
                if table::CTX.left < min_bp {
                    break;
                }
                lhs = self.parse_ctx_override(start, lhs)?;
                continue;
            }
            match t.kind {
                Kind::Op => match table::infix(self.text(t)) {
                    Some(bp) if bp.left >= min_bp => {
                        self.bump()?;
                        let op = self.intern(self.text(t));
                        let rhs = self.parse_expr(bp.right)?;
                        let node = self.expr(Expr::BinOp { op, lhs, rhs });
                        lhs = self.stamp(start, node);
                    }
                    _ => break,
                },
                _ if self.starts_operand(t.kind) => {
                    if table::APP.left < min_bp {
                        break;
                    }
                    let rhs = self.parse_expr(table::APP.right)?;
                    let node = self.expr(Expr::App(lhs, rhs));
                    lhs = self.stamp(start, node);
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Tokens that can begin an operand, so juxtaposition means application.
    /// `do` is deliberately excluded (a `do` block is never an argument).
    fn starts_operand(&self, k: Kind) -> bool {
        matches!(
            k,
            Kind::Int(_)
                | Kind::Real(_)
                | Kind::Str
                | Kind::Word
                | Kind::LParen
                | Kind::Let
                | Kind::If
                | Kind::Is
                | Kind::Lambda
                | Kind::With
                | Kind::LBrace
                | Kind::LBrack
                | Kind::At
        )
    }

    fn parse_prefix(&mut self) -> Result<Aol<Expr>> {
        let t = self.peek()?;
        if let Kind::Op = t.kind {
            if let Some(name) = table::prefix(self.text(t)) {
                let start = t.span.start;
                self.bump()?;
                let op = self.intern(name);
                let operand = self.parse_expr(table::PREFIX)?;
                let node = self.expr(Expr::UnOp { op, operand });
                return Ok(self.stamp(start, node));
            }
        }
        self.parse_primary()
    }

    /// One atom followed by a left-associative postfix `.` chain.
    fn parse_primary(&mut self) -> Result<Aol<Expr>> {
        let start = self.here()?;
        let mut base = self.parse_atom()?;
        while matches!(self.peek_kind()?, Kind::Dot) {
            self.bump()?; // '.'
            base = self.parse_postfix(base)?;
            base = self.stamp(start, base);
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Aol<Expr>> {
        let start = self.here()?;
        let node = self.parse_atom_inner()?;
        Ok(self.stamp(start, node))
    }

    fn parse_atom_inner(&mut self) -> Result<Aol<Expr>> {
        let t = self.peek()?;
        match t.kind {
            Kind::Int(v) => {
                self.bump()?;
                Ok(self.expr(Expr::Int(v)))
            }
            Kind::Real(v) => {
                self.bump()?;
                Ok(self.expr(Expr::Real(v)))
            }
            Kind::Str => {
                self.bump()?;
                self.build_string(t)
            }
            Kind::Word => {
                self.bump()?;
                let name = self.intern(self.text(t));
                Ok(self.expr(Expr::Var { module: None, name }))
            }
            Kind::LParen => self.parse_group(),
            Kind::Let => self.parse_let(),
            Kind::With => self.parse_with(),
            Kind::If => self.parse_if(),
            Kind::Is => self.parse_match(),
            Kind::Do => self.parse_handle(),
            Kind::Defer => self.parse_defer(),
            Kind::Lambda => self.parse_lambda(),
            Kind::LBrack => self.parse_list(),
            Kind::At => self.parse_at_term(),
            Kind::LBrace => self.parse_brace_expr(),
            Kind::Dot => self.parse_leading_dot(),
            _ => Err(self.unexpected(&t, "expected an expression")),
        }
    }

    /// The postfix action after a `.`: struct literal, tuple index, variant
    /// constructor, module-qualified variable, or field access.
    fn parse_postfix(&mut self, base: Aol<Expr>) -> Result<Aol<Expr>> {
        let ahead = self.peek()?;
        match ahead.kind {
            Kind::LBrace => {
                let ty = self.expect_bare_type_name(base, "a struct literal")?;
                self.parse_struct_lit(Some(ty))
            }
            Kind::Int(_) | Kind::Real(_) => {
                let tok = self.bump()?;
                Ok(self.tuple_indices(base, tok))
            }
            // `recv.[i]` / `recv.[i, j, ...]`: tensor indexing (modular). A
            // comma-list folds to nested single-axis indexing (`t.[i].[j]`).
            Kind::LBrack => {
                self.bump()?; // '['
                let mut recv = base;
                loop {
                    let index = self.parse_expr(0)?;
                    recv = self.expr(Expr::Index { recv, index });
                    if !self.eat(|k| matches!(k, Kind::Comma))? {
                        break;
                    }
                }
                expect!(self, Kind::RBrack, "expected ']' to close the index");
                Ok(recv)
            }
            Kind::Word if is_upper(self.text(ahead)) => {
                let ty = self.expect_bare_type_name(base, "a variant constructor")?;
                self.bump()?; // the tag / type name
                let ahead_name = self.intern(self.text(ahead));
                // `Module.Type.Tag`: another uppercase `.Name` follows.
                let qualifies = matches!(self.peek_kind()?, Kind::Dot)
                    && matches!(self.peek_kind_at(1)?, Kind::Word)
                    && {
                        let t1 = self.peek_at(1)?;
                        is_upper(self.text(t1))
                    };
                if qualifies {
                    self.bump()?; // '.'
                    let tag = self.bump_word()?;
                    self.parse_variant_lit(Some(ty), Some(ahead_name), tag)
                } else {
                    self.parse_variant_lit(None, Some(ty), ahead_name)
                }
            }
            Kind::Word => {
                // `Module.name`: uppercase base, lowercase member -> qualified var.
                if let Some(name) = self.bare_var_name(base) {
                    if is_upper(self.ast.text(name)) {
                        self.bump()?;
                        let member = self.intern(self.text(ahead));
                        return Ok(self.expr(Expr::Var {
                            module: Some(name),
                            name: member,
                        }));
                    }
                }
                self.bump()?;
                let name = self.intern(self.text(ahead));
                Ok(self.expr(Expr::Field { record: base, name }))
            }
            _ => Err(self.unexpected(&ahead, "expected a field name, tag, or '{' after '.'")),
        }
    }

    /// Require that `base` is a bare, uppercase-initial type name and return it.
    fn expect_bare_type_name(&self, base: Aol<Expr>, what: &str) -> Result<StrId> {
        if let Some(name) = self.bare_var_name(base) {
            if is_upper(self.ast.text(name)) {
                return Ok(name);
            }
        }
        // No token to point at cheaply here; anchor at a synthetic message.
        Err(Diagnostic::error(
            Code::UnexpectedToken,
            utilities::Span::at(0),
            0,
            format!("{what} must be qualified with an uppercase type name, e.g. Type.{{ ... }}"),
        ))
    }

    /// Split a `.0` / `.0.1` index token into nested field accesses.
    fn tuple_indices(&mut self, base: Aol<Expr>, tok: Token) -> Aol<Expr> {
        let mut record = base;
        for part in self.text(tok).split('.') {
            debug_assert!(
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
                "a tuple index token is dot-separated digit runs"
            );
            let name = self.intern(part);
            record = self.expr(Expr::Field { record, name });
        }
        record
    }

    fn parse_group(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '('
        let e = self.parse_expr(0)?;
        expect!(self, Kind::RParen, "expected ')' to close the group");
        Ok(e)
    }

    /// `{}` unit or `{a, b, ...}` tuple.
    fn parse_brace_expr(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '{'
        if self.eat(|k| matches!(k, Kind::RBrace))? {
            return Ok(self.expr(Expr::Unit));
        }
        // An anonymous record literal starts with a `.field =` entry or `with`;
        // anything else (including a `.Tag` variant element) is a tuple.
        let is_record = matches!(self.peek_kind()?, Kind::With)
            || (matches!(self.peek_kind()?, Kind::Dot)
                && matches!(self.peek_kind_at(1)?, Kind::Word)
                && matches!(self.peek_kind_at(2)?, Kind::Eq));
        if is_record {
            return self.parse_record_expr();
        }
        let mut elems = Vec::new();
        loop {
            elems.push(self.parse_expr(0)?);
            if !self.eat(|k| matches!(k, Kind::Comma))? || matches!(self.peek_kind()?, Kind::RBrace)
            {
                break;
            }
        }
        expect!(self, Kind::RBrace, "expected '}' to close the tuple");
        Ok(self.expr(Expr::Tuple(elems.into_boxed_slice())))
    }

    /// A record value body (the `{` is already consumed): `.field = e` entries and
    /// `with base` splices, optionally ending in `| base` (update). `with` and `|`
    /// are mutually exclusive.
    fn parse_record_expr(&mut self) -> Result<Aol<Expr>> {
        let mut fields = Vec::new();
        let mut with = None;
        let mut update = None;
        loop {
            if self.eat(|k| matches!(k, Kind::With))? {
                with = Some(self.parse_expr(0)?);
            } else {
                fields.push(self.parse_field_init()?);
            }
            if self.at_op("|")? {
                self.bump()?;
                update = Some(self.parse_expr(0)?);
                break;
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? || matches!(self.peek_kind()?, Kind::RBrace)
            {
                break;
            }
        }
        expect!(self, Kind::RBrace, "expected '}' to close the record");
        Ok(self.expr(Expr::Record {
            fields: fields.into_boxed_slice(),
            with,
            update,
        }))
    }

    /// A leading-dot atom: bare struct literal `.{...}` or bare variant `.Tag`.
    fn parse_leading_dot(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '.'
        if matches!(self.peek_kind()?, Kind::LBrace) {
            return self.parse_struct_lit(None);
        }
        let tag = expect!(
            self,
            Kind::Word,
            "expected '{' or a variant tag after a leading '.'"
        );
        if !is_upper(self.text(tag)) {
            return Err(self.unexpected(&tag, "a variant tag must start uppercase"));
        }
        let tag = self.intern(self.text(tag));
        self.parse_variant_lit(None, None, tag)
    }

    /// A struct literal body `{ .field = e, ..., | base }`. Assumes the next token
    /// is `{`. The optional trailing `| base` is a record update: the listed fields
    /// override, the rest come from `base` (an expression of the same struct type).
    /// A bare `{ | base }` clones `base`.
    fn parse_struct_lit(&mut self, ty: Option<StrId>) -> Result<Aol<Expr>> {
        expect!(
            self,
            Kind::LBrace,
            "expected '{' to open the struct literal"
        );
        let mut fields = Vec::new();
        let mut spread = None;
        while !matches!(self.peek_kind()?, Kind::RBrace) {
            if self.at_op("|")? {
                self.bump()?;
                spread = Some(self.parse_expr(0)?);
                break; // the update base is always the final entry
            }
            fields.push(self.parse_field_init()?);
            if self.at_op("|")? {
                self.bump()?;
                spread = Some(self.parse_expr(0)?);
                break;
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        expect!(
            self,
            Kind::RBrace,
            "expected '}' to close the struct literal"
        );
        Ok(self.expr(Expr::StructLit {
            ty,
            fields: fields.into_boxed_slice(),
            spread,
        }))
    }

    /// A variant constructor with an optional `.{ payload }`.
    fn parse_variant_lit(
        &mut self,
        module: Option<StrId>,
        ty: Option<StrId>,
        tag: StrId,
    ) -> Result<Aol<Expr>> {
        let mut fields = Vec::new();
        if matches!(self.peek_kind()?, Kind::Dot) && matches!(self.peek_kind_at(1)?, Kind::LBrace) {
            self.bump()?; // '.'
            self.bump()?; // '{'
            while !matches!(self.peek_kind()?, Kind::RBrace) {
                fields.push(self.parse_field_init()?);
                if !self.eat(|k| matches!(k, Kind::Comma))? {
                    break;
                }
            }
            expect!(
                self,
                Kind::RBrace,
                "expected '}' to close the variant payload"
            );
        }
        Ok(self.expr(Expr::Variant {
            module,
            ty,
            tag,
            fields: fields.into_boxed_slice(),
        }))
    }

    fn parse_field_init(&mut self) -> Result<FieldInit> {
        // A named field is `.field = e` (lowercase field name). A positional
        // value may itself be a leading-dot literal (`.Tag`, `.{ ... }`), which
        // starts with `.` too, so only a lowercase `.name` is a named field.
        let named = matches!(self.peek_kind()?, Kind::Dot)
            && matches!(self.peek_kind_at(1)?, Kind::Word)
            && {
                let t1 = self.peek_at(1)?;
                !is_upper(self.text(t1))
            };
        if named {
            self.bump()?; // '.'
            let name = self.bump_word()?;
            expect!(self, Kind::Eq, "expected '=' after the field name");
            let value = self.parse_expr(0)?;
            Ok(FieldInit::Named { name, value })
        } else {
            Ok(FieldInit::Positional(self.parse_expr(0)?))
        }
    }

    fn parse_list(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '['
        let mut elems = Vec::new();
        while !matches!(self.peek_kind()?, Kind::RBrack) {
            elems.push(self.parse_expr(0)?);
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        expect!(self, Kind::RBrack, "expected ']' to close the list");
        Ok(self.expr(Expr::List(elems.into_boxed_slice())))
    }

    /// An `@`-intrinsic in expression position: `@true`, `@false`, `@array`,
    /// `@extern`.
    fn parse_at_term(&mut self) -> Result<Aol<Expr>> {
        let at = self.peek()?;
        match self.intrinsic_name(at) {
            "true" => {
                self.bump()?;
                Ok(self.expr(Expr::Bool(true)))
            }
            "false" => {
                self.bump()?;
                Ok(self.expr(Expr::Bool(false)))
            }
            "array" => self.parse_array(),
            "extern" => self.parse_extern(),
            other => {
                Err(self.unexpected(&at, &format!("'@{other}' is not valid in an expression")))
            }
        }
    }

    /// `@array.{ n }` or `@array.{ .field = n }`.
    fn parse_array(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '@array'
        expect!(self, Kind::Dot, "expected '.{' after '@array'");
        expect!(self, Kind::LBrace, "expected '{' after '@array.'");
        if matches!(self.peek_kind()?, Kind::Dot) {
            self.bump()?; // '.'
            expect!(self, Kind::Word, "expected a field name after '.'");
            expect!(self, Kind::Eq, "expected '=' after the field name");
        }
        let size = self.parse_expr(0)?;
        self.eat(|k| matches!(k, Kind::Comma))?;
        expect!(
            self,
            Kind::RBrace,
            "expected '}' to close the array literal"
        );
        Ok(self.expr(Expr::Array { size }))
    }

    /// `@extern "abi" "symbol" "lib"`.
    fn parse_extern(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '@extern'
        let abi = self.expect_string("expected an ABI string after '@extern'")?;
        let symbol = self.expect_string("expected a symbol string")?;
        let lib = self.expect_string("expected a library string")?;
        Ok(self.expr(Expr::Extern { abi, symbol, lib }))
    }

    fn expect_string(&mut self, what: &str) -> Result<StrId> {
        let t = self.peek()?;
        if let Kind::Str = t.kind {
            self.bump()?;
            Ok(self.intern_str(t)?)
        } else {
            Err(self.unexpected(&t, what))
        }
    }

    // -- control forms ------------------------------------------------------

    fn parse_let(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'let'
        let mut bindings = Vec::new();
        loop {
            let pat = self.parse_pattern()?;
            let is_var = matches!(self.ast.pats.lookup(pat), Pattern::Var(_));
            let sig = if is_var && matches!(self.peek_kind()?, Kind::Colon) {
                self.bump()?;
                Some(self.parse_type()?)
            } else {
                None
            };
            expect!(self, Kind::Eq, "expected '=' in the 'let' binding");
            let value = self.parse_expr(0)?;
            bindings.push(Binding { pat, sig, value });
            // A trailing comma before `in` is allowed.
            if !self.eat(|k| matches!(k, Kind::Comma))? || matches!(self.peek_kind()?, Kind::In) {
                break;
            }
        }
        expect!(self, Kind::In, "expected 'in' after the 'let' bindings");
        let body = self.parse_expr(0)?;
        Ok(self.expr(Expr::Let {
            bindings: bindings.into_boxed_slice(),
            body,
        }))
    }

    fn parse_with(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'with'
        let subject = self.parse_expr(0)?;
        expect!(self, Kind::In, "expected 'in' after the 'with' subject");
        let body = self.parse_expr(0)?;
        Ok(self.expr(Expr::With { subject, body }))
    }

    fn parse_if(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'if'
        let cond = self.parse_expr(0)?;
        expect!(self, Kind::FatArrow, "expected '=>' after the 'if' condition");
        let then = self.parse_expr(0)?;
        expect!(self, Kind::Else, "expected 'else' in the 'if' expression");
        let alt = self.parse_expr(0)?;
        Ok(self.expr(Expr::If { cond, then, alt }))
    }

    fn parse_match(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'is'
        let scrut = self.parse_expr(0)?;
        let mut arms = Vec::new();
        while self.at_op("|")? {
            let mut patterns = Vec::new();
            while self.at_op("|")? {
                self.bump()?; // '|'
                patterns.push(self.parse_pattern()?);
            }
            let guard = if matches!(self.peek_kind()?, Kind::If) {
                self.bump()?;
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            expect!(self, Kind::FatArrow, "expected '=>' after the match pattern");
            let body = self.parse_expr(0)?;
            arms.push(Arm {
                patterns: patterns.into_boxed_slice(),
                guard,
                body,
            });
        }
        let default = if matches!(self.peek_kind()?, Kind::Else) {
            self.bump()?;
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        Ok(self.expr(Expr::Match {
            scrut,
            arms: arms.into_boxed_slice(),
            default,
        }))
    }

    fn parse_lambda(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // '\'
        let mut params = Vec::new();
        while !matches!(self.peek_kind()?, Kind::Eq) {
            params.push(self.parse_pattern_atom()?);
        }
        expect!(self, Kind::Eq, "expected '=' after the lambda parameters");
        let body = self.parse_expr(0)?;
        Ok(self.expr(Expr::Lambda {
            params: params.into_boxed_slice(),
            body,
        }))
    }

    fn parse_handle(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'do'
        let body = self.parse_expr(0)?;
        if !matches!(self.peek_kind()?, Kind::Ctl) {
            return Ok(self.expr(Expr::Handle {
                body,
                handler: None,
            }));
        }
        self.bump()?; // 'ctl'
        let continuation = self.expect_word("expected a continuation name after 'ctl'")?;
        let mut clauses = Vec::new();
        while self.at_op("|")? {
            self.bump()?; // '|'
            let first = self.expect_word("expected an operation name in the handler clause")?;
            let (effect, op) = if matches!(self.peek_kind()?, Kind::Dot) {
                self.bump()?;
                let op = self.expect_word("expected an operation after '.'")?;
                (Some(first), op)
            } else {
                (None, first)
            };
            let arg = self.expect_word("expected the operation's argument binder")?;
            expect!(self, Kind::FatArrow, "expected '=>' in the handler clause");
            let body = self.parse_expr(0)?;
            clauses.push(Clause {
                effect,
                op,
                arg,
                body,
            });
        }
        let default = if matches!(self.peek_kind()?, Kind::Else) {
            self.bump()?;
            let name = self.expect_word("expected a value binder after 'else'")?;
            expect!(self, Kind::FatArrow, "expected '=>' after the 'else' binder");
            Some((name, self.parse_expr(0)?))
        } else {
            None
        };
        let handler = Box::new(Handler {
            continuation,
            clauses: clauses.into_boxed_slice(),
            default,
        });
        Ok(self.expr(Expr::Handle {
            body,
            handler: Some(handler),
        }))
    }

    fn parse_defer(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'defer'
        let cleanup = self.parse_expr(0)?; // stops at 'do' (not an operand starter)
        expect!(self, Kind::Do, "expected 'do' after the 'defer' cleanup");
        let body = self.parse_expr(0)?;
        Ok(self.expr(Expr::Defer { cleanup, body }))
    }

    // -- patterns -----------------------------------------------------------

    fn parse_pattern(&mut self) -> Result<Aol<Pattern>> {
        let atom = self.parse_pattern_atom()?;
        if self.at_op("::")? {
            self.bump()?;
            let tail = self.parse_pattern()?; // right-associative
            return Ok(self.pat(Pattern::Cons { head: atom, tail }));
        }
        if self.at_op("++")? {
            if let Pattern::Str(prefix) = self.ast.pats.lookup(atom) {
                let prefix = *prefix;
                self.bump()?;
                let rest = self.parse_pattern()?;
                return Ok(self.pat(Pattern::StrPrefix { prefix, rest }));
            }
            let t = self.peek()?;
            return Err(self.unexpected(&t, "'++' in a pattern needs a string-literal prefix"));
        }
        // `lo ... hi`: an inclusive numeric range. Both bounds are numeric literals.
        if matches!(self.peek_kind()?, Kind::Ellipsis) {
            if !matches!(self.ast.pats.lookup(atom), Pattern::Int(_) | Pattern::Real(_)) {
                let t = self.peek()?;
                return Err(self.unexpected(&t, "a range '...' needs a numeric literal on its left"));
            }
            self.bump()?; // '...'
            let hi_tok = self.peek()?;
            let hi = self.parse_pattern_atom()?;
            if !matches!(self.ast.pats.lookup(hi), Pattern::Int(_) | Pattern::Real(_)) {
                return Err(self.unexpected(&hi_tok, "a range '...' needs a numeric literal on its right"));
            }
            return Ok(self.pat(Pattern::Range { lo: atom, hi }));
        }
        Ok(atom)
    }

    fn parse_pattern_atom(&mut self) -> Result<Aol<Pattern>> {
        let t = self.peek()?;
        match t.kind {
            Kind::Int(v) => {
                self.bump()?;
                Ok(self.pat(Pattern::Int(v)))
            }
            Kind::Real(v) => {
                self.bump()?;
                Ok(self.pat(Pattern::Real(v)))
            }
            Kind::Str => {
                self.bump()?;
                let s = self.intern_str(t)?;
                Ok(self.pat(Pattern::Str(s)))
            }
            Kind::Word if self.text(t) == "_" => {
                self.bump()?;
                Ok(self.pat(Pattern::Wild))
            }
            Kind::Word if is_upper(self.text(t)) => self.parse_qualified_pattern(),
            Kind::Word => {
                self.bump()?;
                let name = self.intern(self.text(t));
                Ok(self.pat(Pattern::Var(name)))
            }
            Kind::At => {
                self.bump()?;
                match self.intrinsic_name(t) {
                    "true" => Ok(self.pat(Pattern::Bool(true))),
                    "false" => Ok(self.pat(Pattern::Bool(false))),
                    other => Err(self.unexpected(&t, &format!("'@{other}' is not a pattern"))),
                }
            }
            Kind::LBrack => self.parse_list_pattern(),
            Kind::LBrace => self.parse_tuple_pattern(),
            Kind::Dot => {
                self.bump()?; // '.'
                let tag = expect!(self, Kind::Word, "expected a variant tag after '.'");
                if !is_upper(self.text(tag)) {
                    return Err(self.unexpected(&tag, "a variant tag must start uppercase"));
                }
                let tag = self.intern(self.text(tag));
                let fields = self.parse_pattern_payload()?;
                Ok(self.pat(Pattern::Variant {
                    module: None,
                    ty: None,
                    tag,
                    fields,
                }))
            }
            _ => Err(self.unexpected(&t, "expected a pattern")),
        }
    }

    /// An uppercase-led pattern: `Type.{ ... }` struct, `Type.Tag`, or
    /// `Module.Type.Tag`, each with an optional `.{ ... }` payload.
    fn parse_qualified_pattern(&mut self) -> Result<Aol<Pattern>> {
        let head = self.bump_word()?;
        expect!(
            self,
            Kind::Dot,
            "expected '.' after the type name in a pattern"
        );
        if matches!(self.peek_kind()?, Kind::LBrace) {
            let fields = self.parse_field_pats()?;
            return Ok(self.pat(Pattern::Struct { ty: head, fields }));
        }
        let second = expect!(self, Kind::Word, "expected a variant tag after '.'");
        let second_name = self.intern(self.text(second));
        // `Module.Type.Tag`
        let qualifies = matches!(self.peek_kind()?, Kind::Dot)
            && matches!(self.peek_kind_at(1)?, Kind::Word)
            && {
                let t1 = self.peek_at(1)?;
                is_upper(self.text(t1))
            };
        if qualifies {
            self.bump()?; // '.'
            let tag = self.bump_word()?;
            let fields = self.parse_pattern_payload()?;
            Ok(self.pat(Pattern::Variant {
                module: Some(head),
                ty: Some(second_name),
                tag,
                fields,
            }))
        } else {
            let fields = self.parse_pattern_payload()?;
            Ok(self.pat(Pattern::Variant {
                module: None,
                ty: Some(head),
                tag: second_name,
                fields,
            }))
        }
    }

    fn parse_pattern_payload(&mut self) -> Result<Box<[FieldPat]>> {
        if matches!(self.peek_kind()?, Kind::Dot) && matches!(self.peek_kind_at(1)?, Kind::LBrace) {
            self.bump()?; // '.'
            self.parse_field_pats()
        } else {
            Ok(Box::new([]))
        }
    }

    /// A `{ field-patterns }` body. Assumes the next token is `{`.
    fn parse_field_pats(&mut self) -> Result<Box<[FieldPat]>> {
        expect!(
            self,
            Kind::LBrace,
            "expected '{' to open the field patterns"
        );
        let mut fields = Vec::new();
        while !matches!(self.peek_kind()?, Kind::RBrace) {
            if matches!(self.peek_kind()?, Kind::Dot) {
                self.bump()?; // '.'
                let name = self.expect_word("expected a field name after '.'")?;
                if self.eat(|k| matches!(k, Kind::Eq))? {
                    let pat = self.parse_pattern()?;
                    fields.push(FieldPat::Named { name, pat });
                } else {
                    fields.push(FieldPat::Shorthand(name));
                }
            } else {
                fields.push(FieldPat::Positional(self.parse_pattern()?));
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        expect!(
            self,
            Kind::RBrace,
            "expected '}' to close the field patterns"
        );
        Ok(fields.into_boxed_slice())
    }

    fn parse_list_pattern(&mut self) -> Result<Aol<Pattern>> {
        self.bump()?; // '['
        let mut elems = Vec::new();
        let mut rest = None;
        while !matches!(self.peek_kind()?, Kind::RBrack) {
            if matches!(self.peek_kind()?, Kind::Dot) && matches!(self.peek_kind_at(1)?, Kind::Dot)
            {
                self.bump()?;
                self.bump()?; // '..'
                rest = Some(self.parse_pattern()?);
                break;
            }
            elems.push(self.parse_pattern()?);
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        expect!(self, Kind::RBrack, "expected ']' to close the list pattern");
        Ok(self.pat(Pattern::List {
            elems: elems.into_boxed_slice(),
            rest,
        }))
    }

    fn parse_tuple_pattern(&mut self) -> Result<Aol<Pattern>> {
        self.bump()?; // '{'
        // A record pattern starts with a `.field` entry (or `..rest`); anything
        // else is a positional tuple pattern.
        if matches!(self.peek_kind()?, Kind::Dot) {
            return self.parse_record_pattern();
        }
        let mut elems = Vec::new();
        loop {
            elems.push(self.parse_pattern()?);
            if !self.eat(|k| matches!(k, Kind::Comma))? || matches!(self.peek_kind()?, Kind::RBrace)
            {
                break;
            }
        }
        expect!(
            self,
            Kind::RBrace,
            "expected '}' to close the tuple pattern"
        );
        Ok(self.pat(Pattern::Tuple(elems.into_boxed_slice())))
    }

    /// A record pattern body (the `{` is consumed): `.field = pat` / `.field`
    /// entries, and an optional trailing `..rest` (`..name` binds, `.._` discards).
    fn parse_record_pattern(&mut self) -> Result<Aol<Pattern>> {
        let mut fields = Vec::new();
        let mut rest = None;
        loop {
            // `..rest`: two dots then a binder.
            if matches!(self.peek_kind()?, Kind::Dot) && matches!(self.peek_kind_at(1)?, Kind::Dot) {
                self.bump()?;
                self.bump()?;
                rest = Some(self.parse_pattern()?);
                break;
            }
            expect!(self, Kind::Dot, "expected '.field' in a record pattern");
            let name = self.expect_word("expected a field name after '.'")?;
            if self.eat(|k| matches!(k, Kind::Eq))? {
                let pat = self.parse_pattern()?;
                fields.push(FieldPat::Named { name, pat });
            } else {
                fields.push(FieldPat::Shorthand(name));
            }
            if !self.eat(|k| matches!(k, Kind::Comma))? || matches!(self.peek_kind()?, Kind::RBrace)
            {
                break;
            }
        }
        expect!(self, Kind::RBrace, "expected '}' to close the record pattern");
        Ok(self.pat(Pattern::Record {
            fields: fields.into_boxed_slice(),
            rest,
        }))
    }
}

// -- free helpers ------------------------------------------------------------

fn is_upper(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_uppercase())
}

/// A short human description of a token for diagnostics. `text` is the token's
/// source lexeme (`source[t.span]`), resolved by the caller.
fn describe(t: &Token, text: &str) -> String {
    match t.kind {
        Kind::Eof => "end of input".to_string(),
        _ => format!("'{text}'"),
    }
}
