//! A Pratt (precedence-climbing) parser.
//!
//! Structure mirrors the grammar so the Rust call stack matches it, which keeps
//! error context meaningful:
//!
//! * [`Parser::parse_expr`] is the precedence-climbing core: it parses a prefix,
//!   then folds infix operators and juxtaposition (application) by binding power
//!   from [`crate::ex_table`].
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

use crate::lx::Lexer;
use crate::lx_data::{Kind, Token};
use utilities::{Aol, StrId};
use utilities::{Code, Diagnostic, Result};

use crate::ex_data::*;
use crate::ex_table;

pub struct Parser<'a> {
    lex: Lexer<'a>,
    ast: Ast,
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
        Parser { lex, ast }
    }

    /// Consume the parser, yielding the filled store bundle.
    pub fn into_ast(self) -> Ast {
        self.ast
    }

    // -- store + token helpers ----------------------------------------------

    fn expr(&mut self, e: Expr) -> Aol<Expr> {
        self.ast.exprs.create(e)
    }
    fn ty(&mut self, t: Ty) -> Aol<Ty> {
        self.ast.tys.create(t)
    }
    fn pat(&mut self, p: Pattern) -> Aol<Pattern> {
        self.ast.pats.create(p)
    }
    fn intern(&mut self, s: &str) -> StrId {
        self.ast.strings.intern(s)
    }
    fn intern_bytes(&mut self, b: &[u8]) -> StrId {
        self.ast.strings.intern_bytes(b)
    }
    /// Consume the next token and intern its text (the token is a checked word).
    fn bump_word(&mut self) -> Result<StrId> {
        let t = self.bump()?;
        Ok(self.intern(t.text))
    }
    /// Expect a `Word` and intern it. Combines `expect!` + `intern` so the token
    /// is bound to a local first (a nested `self.intern(self.bump()?...)` would
    /// mutably borrow `self` twice in one expression).
    fn expect_word(&mut self, what: &str) -> Result<StrId> {
        let t = expect!(self, Kind::Word, what);
        Ok(self.intern(t.text))
    }
    /// Consume a `` `type-var `` token and intern its name.
    fn bump_tyvar(&mut self) -> Result<StrId> {
        let t = self.bump()?;
        Ok(self.intern(t.tyvar_name()))
    }

    /// If `base` is a bare, unqualified variable, its interned name.
    fn bare_var_name(&self, base: Aol<Expr>) -> Option<StrId> {
        match self.ast.exprs.lookup(base) {
            Expr::Var { module: None, name } => Some(*name),
            _ => None,
        }
    }

    fn peek(&mut self) -> Result<Token<'a>> {
        self.lex.peek(0)
    }
    fn peek_at(&mut self, n: usize) -> Result<Token<'a>> {
        self.lex.peek(n)
    }
    fn peek_kind(&mut self) -> Result<Kind<'a>> {
        Ok(self.lex.peek(0)?.kind)
    }
    fn bump(&mut self) -> Result<Token<'a>> {
        self.lex.next_token()
    }
    /// Consume the next token if it matches `pred`; report whether it did.
    fn eat(&mut self, pred: impl Fn(Kind<'a>) -> bool) -> Result<bool> {
        if pred(self.peek()?.kind) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    /// Is the next token the operator with lexeme `s`?
    fn at_op(&mut self, s: &str) -> Result<bool> {
        Ok(matches!(self.peek()?.kind, Kind::Op) && self.peek()?.text == s)
    }
    fn unexpected(&self, t: &Token<'a>, what: &str) -> Diagnostic {
        Diagnostic::error(
            Code::UnexpectedToken,
            t.span,
            t.line,
            format!("{what}, found {}", describe(t)),
        )
    }

    // -- program + globals --------------------------------------------------

    /// Parse a whole compilation unit.
    pub fn parse_program(&mut self) -> Result<Program> {
        let at = expect!(self, Kind::At, "expected '@mod' at the start of the file");
        if at.intrinsic_name() != "mod" {
            return Err(self.unexpected(&at, "expected '@mod' at the start of the file"));
        }
        let name = expect!(self, Kind::Word, "expected a module name after '@mod'");
        let module = self.intern(name.text);

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
    fn parse_directive(&mut self, at: Token<'a>) -> Result<Item> {
        match at.intrinsic_name() {
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
        let op = self.intern(op_tok.text);
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
        let first = expect!(self, Kind::Word, "expected a module name").text;
        let mut names = vec![self.intern(first)];
        while matches!(self.peek_kind()?, Kind::Dot) {
            self.bump()?;
            let part = expect!(self, Kind::Word, "expected a name after '.'").text;
            names.push(self.intern(part));
        }
        Ok(names.into_boxed_slice())
    }

    /// `$ name ...`: a value definition, or a struct/union/alias/effect type.
    fn parse_named_global(&mut self) -> Result<Item> {
        let name = self.bump_word()?;
        if !self.eat(|k| matches!(k, Kind::Colon))? {
            expect!(self, Kind::Eq, "expected ':' or '=' after the name");
            return Ok(Item::Def {
                name,
                sig: None,
                body: self.parse_expr(0)?,
            });
        }
        // After `name :`, an `@struct`/`@union`/`@alias`/`@effect` keyword opens a
        // type declaration; anything else is a type signature on a value.
        if let Kind::At = self.peek_kind()? {
            let kw = self.peek()?.intrinsic_name();
            match kw {
                "struct" => {
                    self.bump()?;
                    expect!(self, Kind::Eq, "expected '=' after '@struct'");
                    let fields = self.parse_field_decls()?;
                    return Ok(Item::Struct { name, fields });
                }
                "union" => {
                    self.bump()?;
                    expect!(self, Kind::Eq, "expected '=' after '@union'");
                    let variants = self.parse_union_body()?;
                    return Ok(Item::Union { name, variants });
                }
                "alias" => {
                    self.bump()?;
                    expect!(self, Kind::Eq, "expected '=' after '@alias'");
                    let ty = self.parse_type()?;
                    return Ok(Item::Alias { name, ty });
                }
                "effect" => {
                    self.bump()?;
                    expect!(self, Kind::Eq, "expected '=' after '@effect'");
                    let ops = self.parse_field_decls()?;
                    return Ok(Item::Effect { name, ops });
                }
                _ => {} // an @tycon type signature; fall through
            }
        }
        let sig = Some(self.parse_type()?);
        expect!(self, Kind::Eq, "expected '=' after the type signature");
        let body = self.parse_expr(0)?;
        Ok(Item::Def { name, sig, body })
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

    fn parse_union_body(&mut self) -> Result<Box<[VariantDecl]>> {
        let mut variants = Vec::new();
        while matches!(self.peek_kind()?, Kind::Word) {
            let tag = self.bump_word()?;
            let payload = if self.eat(|k| matches!(k, Kind::Colon))? {
                self.parse_payload()?
            } else {
                Payload::None
            };
            variants.push(VariantDecl { tag, payload });
            if !self.eat(|k| matches!(k, Kind::Comma))? {
                break;
            }
        }
        Ok(variants.into_boxed_slice())
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

    fn peek_kind_at(&mut self, n: usize) -> Result<Kind<'a>> {
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
        Ok(matches!(
            self.peek_kind()?,
            Kind::Word | Kind::At | Kind::TyVar | Kind::LParen | Kind::LBrace
        ))
    }

    fn parse_type_atom(&mut self) -> Result<Aol<Ty>> {
        let t = self.peek()?;
        match t.kind {
            Kind::Word => {
                self.bump()?;
                if matches!(self.peek_kind()?, Kind::Dot) {
                    self.bump()?; // '.'
                    let member = expect!(self, Kind::Word, "expected a type name after '.'");
                    let module = self.intern(t.text);
                    let name = self.intern(member.text);
                    Ok(self.ty(Ty::Con {
                        module: Some(module),
                        name,
                    }))
                } else {
                    let name = self.intern(t.text);
                    Ok(self.ty(Ty::Con { module: None, name }))
                }
            }
            Kind::At => {
                self.bump()?;
                let name = self.intern(t.text);
                Ok(self.ty(Ty::Con { module: None, name }))
            }
            Kind::TyVar => {
                self.bump()?;
                let name = self.intern(t.tyvar_name());
                Ok(self.ty(Ty::Var(name)))
            }
            Kind::LParen => {
                self.bump()?;
                let inner = self.parse_type()?;
                expect!(self, Kind::RParen, "expected ')' to close the type");
                Ok(inner)
            }
            Kind::LBrace => self.parse_brace_type(),
            _ => Err(self.unexpected(&t, "expected a type")),
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
            loop {
                let with = self.eat(|k| matches!(k, Kind::With))?;
                let name = self.expect_word("expected a record field name")?;
                expect!(self, Kind::Colon, "expected ':' after the field name");
                let ty = self.parse_type()?;
                fields.push(RecField { with, name, ty });
                if !self.eat(|k| matches!(k, Kind::Comma))?
                    || matches!(self.peek_kind()?, Kind::RBrace)
                {
                    break;
                }
            }
            expect!(self, Kind::RBrace, "expected '}' to close the record type");
            Ok(self.ty(Ty::Record(fields.into_boxed_slice())))
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

    /// An optional effect row right after `->`: `<>`, `<`e>`, `<A, B>`,
    /// `<A, B | `e>`, or `<| `e>`.
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
            let tail = expect!(
                self,
                Kind::TyVar,
                "expected a `type variable in the effect row"
            );
            let tail = self.intern(tail.tyvar_name());
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
        if matches!(self.peek_kind()?, Kind::TyVar) {
            let tail = self.bump_tyvar()?;
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
            let t = expect!(self, Kind::TyVar, "expected a `type variable after '|'");
            Some(self.intern(t.tyvar_name()))
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
        let mut lhs = self.parse_prefix()?;
        loop {
            let t = self.peek()?;
            match t.kind {
                Kind::Op => match ex_table::infix(t.text) {
                    Some(bp) if bp.left >= min_bp => {
                        self.bump()?;
                        let op = self.intern(t.text);
                        let rhs = self.parse_expr(bp.right)?;
                        lhs = self.expr(Expr::BinOp { op, lhs, rhs });
                    }
                    _ => break,
                },
                _ if self.starts_operand(t.kind) => {
                    if ex_table::APP.left < min_bp {
                        break;
                    }
                    let rhs = self.parse_expr(ex_table::APP.right)?;
                    lhs = self.expr(Expr::App(lhs, rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Tokens that can begin an operand, so juxtaposition means application.
    /// `do` is deliberately excluded (a `do` block is never an argument).
    fn starts_operand(&self, k: Kind<'a>) -> bool {
        matches!(
            k,
            Kind::Int(_)
                | Kind::Real(_)
                | Kind::Str(_)
                | Kind::Word
                | Kind::LParen
                | Kind::Let
                | Kind::If
                | Kind::When
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
            if let Some(name) = ex_table::prefix(t.text) {
                self.bump()?;
                let op = self.intern(name);
                let operand = self.parse_expr(ex_table::PREFIX)?;
                return Ok(self.expr(Expr::UnOp { op, operand }));
            }
        }
        self.parse_primary()
    }

    /// One atom followed by a left-associative postfix `.` chain.
    fn parse_primary(&mut self) -> Result<Aol<Expr>> {
        let mut base = self.parse_atom()?;
        while matches!(self.peek_kind()?, Kind::Dot) {
            self.bump()?; // '.'
            base = self.parse_postfix(base)?;
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Aol<Expr>> {
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
            Kind::Str(s) => {
                self.bump()?;
                let s = self.intern_bytes(s);
                Ok(self.expr(Expr::Str(s)))
            }
            Kind::Word => {
                self.bump()?;
                let name = self.intern(t.text);
                Ok(self.expr(Expr::Var { module: None, name }))
            }
            Kind::LParen => self.parse_group(),
            Kind::Let => self.parse_let(),
            Kind::With => self.parse_with(),
            Kind::If => self.parse_if(),
            Kind::When => self.parse_when(),
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
            Kind::Word if is_upper(ahead.text) => {
                let ty = self.expect_bare_type_name(base, "a variant constructor")?;
                self.bump()?; // the tag / type name
                let ahead_name = self.intern(ahead.text);
                // `Module.Type.Tag`: another uppercase `.Name` follows.
                if matches!(self.peek_kind()?, Kind::Dot)
                    && matches!(self.peek_kind_at(1)?, Kind::Word)
                    && is_upper(self.peek_at(1)?.text)
                {
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
                        let member = self.intern(ahead.text);
                        return Ok(self.expr(Expr::Var {
                            module: Some(name),
                            name: member,
                        }));
                    }
                }
                self.bump()?;
                let name = self.intern(ahead.text);
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
    fn tuple_indices(&mut self, base: Aol<Expr>, tok: Token<'a>) -> Aol<Expr> {
        let mut record = base;
        for part in tok.text.split('.') {
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
        if !is_upper(tag.text) {
            return Err(self.unexpected(&tag, "a variant tag must start uppercase"));
        }
        let tag = self.intern(tag.text);
        self.parse_variant_lit(None, None, tag)
    }

    /// A struct literal body `{ .field = e, ..., ..spread }`. Assumes the next
    /// token is `{`.
    fn parse_struct_lit(&mut self, ty: Option<StrId>) -> Result<Aol<Expr>> {
        expect!(
            self,
            Kind::LBrace,
            "expected '{' to open the struct literal"
        );
        let mut fields = Vec::new();
        let mut spread = None;
        while !matches!(self.peek_kind()?, Kind::RBrace) {
            if matches!(self.peek_kind()?, Kind::Dot) && matches!(self.peek_kind_at(1)?, Kind::Dot)
            {
                self.bump()?;
                self.bump()?; // '..'
                spread = Some(self.parse_expr(0)?);
                break; // a spread is always the final entry
            }
            fields.push(self.parse_field_init()?);
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
        if matches!(self.peek_kind()?, Kind::Dot)
            && matches!(self.peek_kind_at(1)?, Kind::Word)
            && !is_upper(self.peek_at(1)?.text)
        {
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
        match at.intrinsic_name() {
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
        if let Kind::Str(s) = t.kind {
            self.bump()?;
            Ok(self.intern_bytes(s))
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
        expect!(self, Kind::Then, "expected 'then' after the 'if' condition");
        let then = self.parse_expr(0)?;
        expect!(self, Kind::Else, "expected 'else' in the 'if' expression");
        let alt = self.parse_expr(0)?;
        Ok(self.expr(Expr::If { cond, then, alt }))
    }

    fn parse_when(&mut self) -> Result<Aol<Expr>> {
        self.bump()?; // 'when'
        let scrut = self.parse_expr(0)?;
        let mut arms = Vec::new();
        while matches!(self.peek_kind()?, Kind::Is) {
            let mut patterns = Vec::new();
            while self.eat(|k| matches!(k, Kind::Is))? {
                patterns.push(self.parse_pattern()?);
            }
            let guard = if matches!(self.peek_kind()?, Kind::If) {
                self.bump()?;
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            expect!(self, Kind::Then, "expected 'then' after the match pattern");
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
        while self.eat(|k| matches!(k, Kind::Is))? {
            let first = self.expect_word("expected an operation name in the handler clause")?;
            let (effect, op) = if matches!(self.peek_kind()?, Kind::Dot) {
                self.bump()?;
                let op = self.expect_word("expected an operation after '.'")?;
                (Some(first), op)
            } else {
                (None, first)
            };
            let arg = self.expect_word("expected the operation's argument binder")?;
            expect!(self, Kind::Eq, "expected '=' in the handler clause");
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
            expect!(self, Kind::Eq, "expected '=' after the 'else' binder");
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
            Kind::Str(s) => {
                self.bump()?;
                let s = self.intern_bytes(s);
                Ok(self.pat(Pattern::Str(s)))
            }
            Kind::Word if t.text == "_" => {
                self.bump()?;
                Ok(self.pat(Pattern::Wild))
            }
            Kind::Word if is_upper(t.text) => self.parse_qualified_pattern(),
            Kind::Word => {
                self.bump()?;
                let name = self.intern(t.text);
                Ok(self.pat(Pattern::Var(name)))
            }
            Kind::At => {
                self.bump()?;
                match t.intrinsic_name() {
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
                if !is_upper(tag.text) {
                    return Err(self.unexpected(&tag, "a variant tag must start uppercase"));
                }
                let tag = self.intern(tag.text);
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
        let second_name = self.intern(second.text);
        // `Module.Type.Tag`
        if matches!(self.peek_kind()?, Kind::Dot)
            && matches!(self.peek_kind_at(1)?, Kind::Word)
            && is_upper(self.peek_at(1)?.text)
        {
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
}

// -- free helpers ------------------------------------------------------------

fn is_upper(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_uppercase())
}

/// A short human description of a token for diagnostics.
fn describe(t: &Token<'_>) -> String {
    match t.kind {
        Kind::Eof => "end of input".to_string(),
        Kind::Int(_)
        | Kind::Real(_)
        | Kind::Str(_)
        | Kind::Word
        | Kind::Op
        | Kind::At
        | Kind::TyVar => {
            format!("'{}'", t.text)
        }
        _ => format!("'{}'", t.text),
    }
}
