//! Desugaring: the front-end handle-based AST ([`syntax`]) to the [`crate::term`]
//! Core.
//!
//! This is the whole "remove the sugar" pass. Operators become applications of a
//! built-in variable (or, for `;`/`|>`/`<|`/`::`, a `let`/application/`List`
//! construction); `if` and `when` become a single [`Term::Case`]; list and cons
//! sugar become `List.Cons`/`List.Nil`; multi-parameter and pattern lambdas curry
//! into single-parameter lambdas over `Case`; leading record parameters of a
//! definition's signature are consumed. Positional struct and variant fields are
//! labelled and ordered against the declarations gathered by [`Decls`].
//!
//! The AST is read through a borrowed [`Ast`]: a handle is resolved with
//! `ast.expr`/`ast.ty`/`ast.pat`, a name with `ast.text`, a byte string with
//! `ast.bytes`. Because `ast` is shared, those resolve to `'a`-lived data
//! independent of the `&mut self` used for fresh-name generation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ex_data::{
    Ast, Binding, Expr, FieldInit, FieldPat, Item, Pattern, Payload, Program as AstProgram,
    RecField, Ty,
};
use utilities::Aol;

use crate::cr_data::{
    Arm, Clause as CoreClause, Effect, Handler as CoreHandler, Pat, Program, Term,
};

/// The type declarations a lowering needs: struct field order (to label
/// positional literals and patterns) and each variant's union and payload field
/// names. Gathered across every module (as owned strings) so a program can
/// construct a type declared in an imported module.
#[derive(Default)]
pub struct Decls {
    struct_fields: HashMap<String, Vec<String>>,
    variants: HashMap<String, VariantDecl>,
}

struct VariantDecl {
    union: String,
    fields: Vec<Option<String>>,
}

impl Decls {
    /// Gather declarations from every module of one shared [`Ast`].
    pub fn collect(ast: &Ast, programs: &[AstProgram]) -> Decls {
        let mut decls = Decls::default();
        for p in programs {
            decls.add(ast, p);
        }
        decls
    }

    fn add(&mut self, ast: &Ast, program: &AstProgram) {
        for item in program.items.iter() {
            match item {
                Item::Struct { name, fields } => {
                    let names = fields
                        .iter()
                        .map(|f| ast.text(f.name).to_string())
                        .collect();
                    self.struct_fields
                        .insert(ast.text(*name).to_string(), names);
                }
                Item::Union { name, variants } => {
                    for v in variants.iter() {
                        let fields = match &v.payload {
                            Payload::None => Vec::new(),
                            Payload::Bare(_) => vec![None],
                            Payload::Fields(fs) => fs
                                .iter()
                                .map(|f| f.name.map(|n| ast.text(n).to_string()))
                                .collect(),
                        };
                        self.variants.insert(
                            ast.text(v.tag).to_string(),
                            VariantDecl {
                                union: ast.text(*name).to_string(),
                                fields,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// The payload arity and (union, field names) for a variant tag. `List`'s
    /// `Cons`/`Nil` are prelude, so they are answered directly.
    fn variant(&self, tag: &str) -> Option<(&str, &[Option<String>])> {
        match tag {
            "Cons" => Some(("List", CONS_FIELDS)),
            "Nil" => Some(("List", &[])),
            _ => self
                .variants
                .get(tag)
                .map(|v| (v.union.as_str(), v.fields.as_slice())),
        }
    }

    /// The struct whose exact set of field names matches an all-named literal.
    fn struct_by_fields(&self, names: &[&str]) -> Option<&str> {
        self.struct_fields.iter().find_map(|(sname, fields)| {
            let same =
                fields.len() == names.len() && names.iter().all(|n| fields.iter().any(|f| f == n));
            same.then_some(sname.as_str())
        })
    }
}

const CONS_FIELDS: &[Option<String>] = &[None, None];

/// One surface match arm's unresolved handles: its patterns (an or-pattern has
/// several), an optional guard, and the body.
type ArmHandles = (Vec<Aol<Pattern>>, Option<Aol<Expr>>, Aol<Expr>);

/// The type checker's resolutions that lowering cannot re-derive without types.
/// `array_exprs`/`array_pats` are the `[..]` nodes resolved to `Array` (a byte
/// vector) rather than the default `List`; `call_modules` maps a bare-call
/// `Expr::Var` to the module its overload resolved to, so lowering can emit a
/// qualified `MOD.name`. Empty (the default) means "no resolutions", correct for
/// callers without a checker (all `[..]` are `List`, all calls stay bare).
#[derive(Default)]
pub struct Resolved {
    pub array_exprs: HashSet<Aol<Expr>>,
    pub array_pats: HashSet<Aol<Pattern>>,
    pub call_modules: HashMap<Aol<Expr>, String>,
}

/// Lower one module's globals to Core.
pub fn lower_program(
    ast: &Ast,
    program: &AstProgram,
    decls: &Decls,
    resolved: &Resolved,
) -> Program {
    let mut lw = Lowerer {
        ast,
        decls,
        resolved,
        fresh: 0,
    };
    let mut effects = Vec::new();
    let mut globals = Vec::new();
    for item in program.items.iter() {
        match item {
            Item::Def { name, sig, body } => {
                let term = lw.def(*sig, *body);
                globals.push((ast.text(*name).to_string(), term));
            }
            Item::Effect { name, ops } => {
                let effect = ast.text(*name).to_string();
                for op in ops.iter() {
                    effects.push(Effect {
                        effect: effect.clone(),
                        op: ast.text(op.name).to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    Program {
        module: ast.text(program.module).to_string(),
        effects,
        globals,
    }
}

struct Lowerer<'a> {
    ast: &'a Ast,
    decls: &'a Decls,
    resolved: &'a Resolved,
    fresh: u32,
}

impl<'a> Lowerer<'a> {
    fn node(&self, e: Aol<Expr>) -> &'a Expr {
        self.ast.expr(e)
    }
    fn tnode(&self, t: Aol<Ty>) -> &'a Ty {
        self.ast.ty(t)
    }
    fn pnode(&self, p: Aol<Pattern>) -> &'a Pattern {
        self.ast.pat(p)
    }
    fn text(&self, id: utilities::StrId) -> &'a str {
        self.ast.text(id)
    }

    fn fresh(&mut self) -> String {
        self.fresh += 1;
        format!("%{}", self.fresh)
    }

    /// Lower a definition, consuming leading record parameters of its signature.
    fn def(&mut self, sig: Option<Aol<Ty>>, body: Aol<Expr>) -> Term {
        let term = self.expr(body);
        self.record_params(sig, term)
    }

    fn record_params(&mut self, sig: Option<Aol<Ty>>, body: Term) -> Term {
        let Some(sig) = sig else { return body };
        let Ty::Arrow { from, to, .. } = self.tnode(sig) else {
            return body;
        };
        let (from, to) = (*from, *to);
        let Ty::Record(fields) = self.tnode(from) else {
            return body;
        };
        let inner = self.record_params(Some(to), body);
        self.record_param(fields, inner)
    }

    /// Wrap `body` in the lambda that binds one record parameter's fields.
    fn record_param(&mut self, fields: &[RecField], body: Term) -> Term {
        let mut inner = body;
        for f in fields.iter().rev() {
            if f.with {
                inner = Term::With {
                    subject: Arc::new(Term::var(self.text(f.name))),
                    body: Arc::new(inner),
                };
            }
        }
        if let [f] = fields {
            return Term::Lam {
                param: self.text(f.name).to_string(),
                body: Arc::new(inner),
            };
        }
        let param = self.fresh();
        let pat = Pat::Tuple(
            fields
                .iter()
                .map(|f| Pat::Var(self.text(f.name).to_string()))
                .collect(),
        );
        Term::Lam {
            param: param.clone(),
            body: Arc::new(Term::Case {
                scrut: Arc::new(Term::var(param)),
                arms: Arc::from([Arm {
                    pat,
                    guard: None,
                    body: Arc::new(inner),
                }]),
                default: None,
            }),
        }
    }

    fn expr(&mut self, e: Aol<Expr>) -> Term {
        match self.node(e) {
            Expr::Int(n) => Term::Int(*n),
            Expr::Real(r) => Term::Real(*r),
            Expr::Str(s) => Term::Str(self.ast.bytes(*s).to_vec()),
            Expr::Bool(b) => Term::Bool(*b),
            Expr::Unit => Term::Unit,

            Expr::Var { module, name } => {
                let name = self.text(*name);
                match name {
                    "true" => Term::Bool(true),
                    "false" => Term::Bool(false),
                    _ => {
                        let module = match module {
                            Some(m) => Some(self.text(*m).to_string()),
                            None => self.resolved.call_modules.get(&e).cloned(),
                        };
                        Term::Var {
                            module,
                            name: name.to_string(),
                        }
                    }
                }
            }

            Expr::App(f, x) => {
                let (f, x) = (*f, *x);
                Term::app(self.expr(f), self.expr(x))
            }

            Expr::BinOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (self.text(*op), *lhs, *rhs);
                self.binop(op, lhs, rhs)
            }
            Expr::UnOp { op, operand } => {
                let (op, operand) = (self.text(*op).to_string(), *operand);
                Term::app(Term::var(op), self.expr(operand))
            }

            Expr::Tuple(items) => {
                let items: Vec<Aol<Expr>> = items.to_vec();
                Term::Tuple(items.into_iter().map(|e| self.expr(e)).collect())
            }

            Expr::List(items) => {
                let items: Vec<Aol<Expr>> = items.to_vec();
                if self.resolved.array_exprs.contains(&e) {
                    // A byte vector: start empty, push each element left to right.
                    let mut acc = Term::app(Term::var("array_alloc"), Term::Int(0));
                    for it in items {
                        let x = self.expr(it);
                        acc = Term::app(Term::app(Term::var("array_push"), acc), x);
                    }
                    acc
                } else {
                    let mut acc = nil();
                    for e in items.into_iter().rev() {
                        acc = cons(self.expr(e), acc);
                    }
                    acc
                }
            }

            Expr::Array { size } => Term::app(Term::var("array_alloc"), self.expr(*size)),

            Expr::Field { record, name } => {
                let (record, name) = (*record, self.text(*name).to_string());
                Term::Field(Arc::new(self.expr(record)), name)
            }

            Expr::StructLit { ty, fields, spread } => {
                let ty = ty.map(|t| self.text(t));
                let spread = *spread;
                self.struct_lit(ty, fields, spread)
            }
            Expr::Variant {
                ty, tag, fields, ..
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                self.variant(ty, tag, fields)
            }

            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                Term::Case {
                    scrut: Arc::new(self.expr(cond)),
                    arms: Arc::from([Arm {
                        pat: Pat::Bool(true),
                        guard: None,
                        body: Arc::new(self.expr(then)),
                    }]),
                    default: Some(Arc::new(self.expr(alt))),
                }
            }

            Expr::Match {
                scrut,
                arms,
                default,
            } => {
                let scrut = *scrut;
                let default = *default;
                let mut lowered = Vec::new();
                // Collect handles first so the node borrow does not span the
                // recursive lowering of children.
                let arm_data: Vec<ArmHandles> = arms
                    .iter()
                    .map(|arm| (arm.patterns.to_vec(), arm.guard, arm.body))
                    .collect();
                for (patterns, guard, body) in arm_data {
                    let user_guard = guard.map(|g| self.expr(g));
                    let body = Arc::new(self.expr(body));
                    for pat in patterns {
                        if self.resolved.array_pats.contains(&pat) {
                            lowered.push(self.array_arm(pat, user_guard.clone(), body.clone()));
                        } else {
                            lowered.push(Arm {
                                pat: self.pat(pat),
                                guard: user_guard.clone().map(Arc::new),
                                body: body.clone(),
                            });
                        }
                    }
                }
                Term::Case {
                    scrut: Arc::new(self.expr(scrut)),
                    arms: lowered.into(),
                    default: default.map(|d| Arc::new(self.expr(d))),
                }
            }

            Expr::Lambda { params, body } => {
                let params: Vec<Aol<Pattern>> = params.to_vec();
                let body = *body;
                let mut term = self.expr(body);
                for p in params.into_iter().rev() {
                    term = self.lambda_param(p, term);
                }
                term
            }

            Expr::Let { bindings, body } => {
                let bindings: Vec<Binding> = bindings.to_vec();
                let body = *body;
                let mut term = self.expr(body);
                for b in bindings.into_iter().rev() {
                    term = self.binding(&b, term);
                }
                term
            }

            Expr::With { subject, body } => {
                let (subject, body) = (*subject, *body);
                Term::With {
                    subject: Arc::new(self.expr(subject)),
                    body: Arc::new(self.expr(body)),
                }
            }

            Expr::Handle { body, handler } => {
                let body = *body;
                let Some(handler) = handler.as_deref() else {
                    return self.expr(body);
                };
                let continuation = self.text(handler.continuation).to_string();
                let clauses: Vec<(Option<String>, String, String, Aol<Expr>)> = handler
                    .clauses
                    .iter()
                    .map(|clause| {
                        (
                            clause.effect.map(|e| self.text(e).to_string()),
                            self.text(clause.op).to_string(),
                            self.text(clause.arg).to_string(),
                            clause.body,
                        )
                    })
                    .collect();
                let default = handler
                    .default
                    .map(|(name, body)| (self.text(name).to_string(), body));
                Term::Handle {
                    body: Arc::new(self.expr(body)),
                    handler: Arc::new(CoreHandler {
                        continuation,
                        clauses: clauses
                            .into_iter()
                            .map(|(effect, op, arg, body)| CoreClause {
                                effect,
                                op,
                                arg,
                                body: self.expr(body),
                            })
                            .collect(),
                        default: default.map(|(name, body)| (name, self.expr(body))),
                    }),
                }
            }
            Expr::Defer { cleanup, body } => {
                let (cleanup, body) = (*cleanup, *body);
                Term::Defer {
                    cleanup: Arc::new(self.expr(cleanup)),
                    body: Arc::new(self.expr(body)),
                }
            }
            Expr::Extern { .. } => Term::Fault("foreign function".into()),
        }
    }

    fn binop(&mut self, op: &str, lhs: Aol<Expr>, rhs: Aol<Expr>) -> Term {
        match op {
            ";" => {
                let name = self.fresh();
                Term::Let {
                    name,
                    rec: false,
                    val: Arc::new(self.expr(lhs)),
                    body: Arc::new(self.expr(rhs)),
                }
            }
            "|>" => Term::app(self.expr(rhs), self.expr(lhs)),
            "<|" => Term::app(self.expr(lhs), self.expr(rhs)),
            "::" => cons(self.expr(lhs), self.expr(rhs)),
            _ => Term::app(Term::app(Term::var(op), self.expr(lhs)), self.expr(rhs)),
        }
    }

    fn lambda_param(&mut self, pat: Aol<Pattern>, body: Term) -> Term {
        match self.pnode(pat) {
            Pattern::Var(name) => Term::Lam {
                param: self.text(*name).to_string(),
                body: Arc::new(body),
            },
            Pattern::Wild => Term::Lam {
                param: self.fresh(),
                body: Arc::new(body),
            },
            _ => {
                let param = self.fresh();
                let scrut = Term::var(param.clone());
                let arm_pat = self.pat(pat);
                Term::Lam {
                    param,
                    body: Arc::new(Term::Case {
                        scrut: Arc::new(scrut),
                        arms: Arc::from([Arm {
                            pat: arm_pat,
                            guard: None,
                            body: Arc::new(body),
                        }]),
                        default: None,
                    }),
                }
            }
        }
    }

    fn binding(&mut self, b: &Binding, body: Term) -> Term {
        match self.pnode(b.pat) {
            Pattern::Var(name) => Term::Let {
                name: self.text(*name).to_string(),
                rec: true,
                val: Arc::new(self.expr(b.value)),
                body: Arc::new(body),
            },
            Pattern::Wild => Term::Let {
                name: self.fresh(),
                rec: false,
                val: Arc::new(self.expr(b.value)),
                body: Arc::new(body),
            },
            _ => {
                let pat = self.pat(b.pat);
                Term::Case {
                    scrut: Arc::new(self.expr(b.value)),
                    arms: Arc::from([Arm {
                        pat,
                        guard: None,
                        body: Arc::new(body),
                    }]),
                    default: None,
                }
            }
        }
    }

    fn struct_lit(
        &mut self,
        ty: Option<&str>,
        fields: &[FieldInit],
        spread: Option<Aol<Expr>>,
    ) -> Term {
        let base = spread.map(|s| Arc::new(self.expr(s)));

        let named: Option<Vec<&str>> = fields
            .iter()
            .map(|f| match f {
                FieldInit::Named { name, .. } => Some(self.text(*name)),
                FieldInit::Positional(_) => None,
            })
            .collect();

        let field_names: Option<Vec<String>> = ty
            .and_then(|n| self.decls.struct_fields.get(n).cloned())
            .or_else(|| {
                named.as_ref().and_then(|ns| {
                    self.decls
                        .struct_by_fields(ns)
                        .map(|_| ns.iter().map(|s| s.to_string()).collect())
                })
            });

        let name = ty
            .map(str::to_string)
            .or_else(|| named.and_then(|ns| self.decls.struct_by_fields(&ns).map(str::to_string)))
            .unwrap_or_default();

        let mut out = Vec::with_capacity(fields.len());
        for (i, fi) in fields.iter().enumerate() {
            match fi {
                FieldInit::Named { name, value } => {
                    let fname = self.text(*name).to_string();
                    out.push((fname, self.expr(*value)));
                }
                FieldInit::Positional(value) => {
                    let fname = field_names
                        .as_ref()
                        .and_then(|ns| ns.get(i))
                        .cloned()
                        .unwrap_or_else(|| i.to_string());
                    out.push((fname, self.expr(*value)));
                }
            }
        }
        Term::Struct {
            name,
            base,
            fields: out.into(),
        }
    }

    fn variant(&mut self, ty: Option<&str>, tag: &str, fields: &[FieldInit]) -> Term {
        let (union, names) = match self.decls.variant(tag) {
            Some((u, ns)) => (u.to_string(), ns.to_vec()),
            None => (ty.unwrap_or_default().to_string(), Vec::new()),
        };

        let arity = names.len().max(fields.len());
        let mut slots: Vec<Option<Term>> = (0..arity).map(|_| None).collect();
        let mut next = 0;
        for fi in fields {
            match fi {
                FieldInit::Named { name, value } => {
                    let fname = self.text(*name);
                    let idx = names
                        .iter()
                        .position(|n| n.as_deref() == Some(fname))
                        .unwrap_or(next);
                    slots[idx] = Some(self.expr(*value));
                }
                FieldInit::Positional(value) => {
                    slots[next] = Some(self.expr(*value));
                    next += 1;
                }
            }
        }
        let out = slots.into_iter().map(|s| s.unwrap_or(Term::Unit)).collect();
        Term::Variant {
            ty: if union.is_empty() {
                ty.unwrap_or_default().to_string()
            } else {
                union
            },
            tag: tag.to_string(),
            fields: out,
        }
    }

    // -- patterns -----------------------------------------------------------

    fn pat(&mut self, p: Aol<Pattern>) -> Pat {
        match self.pnode(p) {
            Pattern::Wild => Pat::Wild,
            Pattern::Var(name) => Pat::Var(self.text(*name).to_string()),
            Pattern::Int(n) => Pat::Int(*n),
            Pattern::Real(r) => Pat::Real(*r),
            Pattern::Str(s) => Pat::Str(self.ast.bytes(*s).to_vec()),
            Pattern::Bool(b) => Pat::Bool(*b),
            Pattern::StrPrefix { prefix, rest } => Pat::StrPrefix {
                prefix: self.ast.bytes(*prefix).to_vec(),
                rest: Box::new(self.pat(*rest)),
            },
            Pattern::Tuple(pats) => {
                let pats: Vec<Aol<Pattern>> = pats.to_vec();
                Pat::Tuple(pats.into_iter().map(|p| self.pat(p)).collect())
            }
            Pattern::Cons { head, tail } => {
                let (head, tail) = (*head, *tail);
                Pat::Variant {
                    tag: "Cons".into(),
                    fields: vec![self.pat(head), self.pat(tail)],
                }
            }
            Pattern::List { elems, rest } => {
                let elems: Vec<Aol<Pattern>> = elems.to_vec();
                let rest = *rest;
                let mut acc = match rest {
                    Some(r) => self.pat(r),
                    None => Pat::Variant {
                        tag: "Nil".into(),
                        fields: Vec::new(),
                    },
                };
                for e in elems.into_iter().rev() {
                    acc = Pat::Variant {
                        tag: "Cons".into(),
                        fields: vec![self.pat(e), acc],
                    };
                }
                acc
            }
            Pattern::Struct { ty, fields } => {
                let names = self.decls.struct_fields.get(self.text(*ty)).cloned();
                let fields: Vec<FieldPat> = fields.to_vec();
                Pat::Struct {
                    fields: self.field_pats(&fields, names.as_deref()),
                }
            }
            Pattern::Variant {
                ty, tag, fields, ..
            } => {
                let tag = self.text(*tag).to_string();
                let ty = ty.map(|t| self.text(t));
                let names = self
                    .decls
                    .variant(&tag)
                    .map(|(_, ns)| ns.to_vec())
                    .or_else(|| {
                        ty.and_then(|t| self.decls.struct_fields.get(t))
                            .map(|ns| ns.iter().cloned().map(Some).collect())
                    });
                let fields: Vec<FieldPat> = fields.to_vec();
                let positional = self.variant_field_pats(&fields, names.as_deref());
                Pat::Variant {
                    tag,
                    fields: positional,
                }
            }
        }
    }

    fn field_pats(&mut self, fields: &[FieldPat], names: Option<&[String]>) -> Vec<(String, Pat)> {
        let mut out = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    out.push((self.text(*name).to_string(), self.pat(*pat)))
                }
                FieldPat::Shorthand(name) => {
                    let name = self.text(*name).to_string();
                    out.push((name.clone(), Pat::Var(name)))
                }
                FieldPat::Positional(pat) => {
                    let fname = names
                        .and_then(|ns| ns.get(i))
                        .cloned()
                        .unwrap_or_else(|| i.to_string());
                    out.push((fname, self.pat(*pat)));
                }
            }
        }
        out
    }

    fn variant_field_pats(
        &mut self,
        fields: &[FieldPat],
        names: Option<&[Option<String>]>,
    ) -> Vec<Pat> {
        let arity = names
            .map(<[_]>::len)
            .unwrap_or(fields.len())
            .max(fields.len());
        let mut slots: Vec<Pat> = (0..arity).map(|_| Pat::Wild).collect();
        let mut next = 0;
        for f in fields {
            match f {
                FieldPat::Named { name, pat } => {
                    let fname = self.text(*name);
                    let idx = names
                        .and_then(|ns| ns.iter().position(|n| n.as_deref() == Some(fname)))
                        .unwrap_or(next);
                    slots[idx] = self.pat(*pat);
                }
                FieldPat::Shorthand(name) => {
                    let fname = self.text(*name);
                    let idx = names
                        .and_then(|ns| ns.iter().position(|n| n.as_deref() == Some(fname)))
                        .unwrap_or(next);
                    slots[idx] = Pat::Var(fname.to_string());
                    next += 1;
                }
                FieldPat::Positional(pat) => {
                    slots[next] = self.pat(*pat);
                    next += 1;
                }
            }
        }
        slots
    }

    /// Lower a type-directed array (byte-vector) pattern into a guarded arm. The
    /// whole scrutinee binds to a fresh `v`; a length test (exact, or `>=` when an
    /// open `..rest` follows) plus one equality per literal element form the
    /// guard, and named elements/`rest` are extracted with `array_get` /
    /// `array_slice`. The extractions run only after the length test passes, so
    /// they never index out of bounds. The bindings are duplicated into the guard
    /// so a user guard (and the literal checks) can see the element names.
    fn array_arm(&mut self, pat: Aol<Pattern>, user_guard: Option<Term>, body: Arc<Term>) -> Arm {
        let (elems, rest) = match self.pnode(pat) {
            Pattern::List { elems, rest } => (elems.to_vec(), *rest),
            _ => unreachable!("array_arm on a non-list pattern"),
        };
        let v = self.fresh();
        let n = elems.len();
        let mut binds: Vec<(String, Term)> = Vec::new();
        let mut checks: Vec<Term> = Vec::new();
        for (i, e) in elems.iter().enumerate() {
            match self.pnode(*e) {
                Pattern::Var(name) => binds.push((self.text(*name).to_string(), array_get(&v, i))),
                Pattern::Wild => {}
                Pattern::Int(k) => checks.push(bin("?=", array_get(&v, i), Term::Int(*k))),
                Pattern::Real(r) => checks.push(bin("?=", array_get(&v, i), Term::Real(*r))),
                Pattern::Bool(b) => checks.push(bin("?=", array_get(&v, i), Term::Bool(*b))),
                _ => {}
            }
        }
        if let Some(r) = rest {
            if let Pattern::Var(name) = self.pnode(r) {
                binds.push((self.text(*name).to_string(), array_slice(&v, n)));
            }
        }
        let len_check = if rest.is_some() {
            bin(">=", array_len(&v), Term::Int(n as i64))
        } else {
            bin("?=", array_len(&v), Term::Int(n as i64))
        };
        let guard = if checks.is_empty() && user_guard.is_none() {
            len_check
        } else {
            let mut inner = user_guard.unwrap_or(Term::Bool(true));
            for c in checks.into_iter().rev() {
                inner = if_then_else(c, inner, Term::Bool(false));
            }
            let inner = let_chain(&binds, inner);
            if_then_else(len_check, inner, Term::Bool(false))
        };
        let body = let_chain(&binds, (*body).clone());
        Arm {
            pat: Pat::Var(v),
            guard: Some(Arc::new(guard)),
            body: Arc::new(body),
        }
    }
}

/// `array_len v`.
fn array_len(v: &str) -> Term {
    Term::app(Term::var("array_len"), Term::var(v))
}

/// `array_get v i`.
fn array_get(v: &str, i: usize) -> Term {
    Term::app(
        Term::app(Term::var("array_get"), Term::var(v)),
        Term::Int(i as i64),
    )
}

/// `array_slice v from (array_len v)` (the open tail from `from`).
fn array_slice(v: &str, from: usize) -> Term {
    Term::app(
        Term::app(
            Term::app(Term::var("array_slice"), Term::var(v)),
            Term::Int(from as i64),
        ),
        array_len(v),
    )
}

/// A binary operator application `l <op> r`.
fn bin(op: &str, l: Term, r: Term) -> Term {
    Term::app(Term::app(Term::var(op), l), r)
}

/// `if cond then t else e`, lowered like the surface `if`.
fn if_then_else(cond: Term, t: Term, e: Term) -> Term {
    Term::Case {
        scrut: Arc::new(cond),
        arms: Arc::from([Arm {
            pat: Pat::Bool(true),
            guard: None,
            body: Arc::new(t),
        }]),
        default: Some(Arc::new(e)),
    }
}

/// Wrap `inner` in a nest of non-recursive `let`s binding each `(name, value)`.
fn let_chain(binds: &[(String, Term)], inner: Term) -> Term {
    let mut acc = inner;
    for (name, val) in binds.iter().rev() {
        acc = Term::Let {
            name: name.clone(),
            rec: false,
            val: Arc::new(val.clone()),
            body: Arc::new(acc),
        };
    }
    acc
}

fn nil() -> Term {
    Term::Variant {
        ty: "List".into(),
        tag: "Nil".into(),
        fields: Arc::from([]),
    }
}

fn cons(head: Term, tail: Term) -> Term {
    Term::Variant {
        ty: "List".into(),
        tag: "Cons".into(),
        fields: Arc::from([head, tail]),
    }
}
