//! Desugaring: the front-end handle-based AST ([`crate::parser::data`]) to the
//! [`crate::lowering::data`] Core.
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

pub mod anf;
pub mod data;
pub mod debruijn;
pub mod patmat;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::parser::data::{
    Ast, Binding, Expr, FieldDecl, FieldInit, FieldPat, Item, Pattern, Payload,
    Program as AstProgram, RecField, Ty,
};
use utilities::Aol;

use crate::lowering::data::{
    Arm, Clause as CoreClause, Effect, Handler as CoreHandler, Pat, Program, Term,
};

/// The type declarations a lowering needs: struct field order (to label
/// positional literals and patterns) and each variant's union and payload field
/// names. Two modules may declare the same-named type (`Pair`, `Maybe`, ...), so
/// declarations are grouped BY MODULE and every lookup resolves against the
/// lowering module's own types first, then the modules it imports (`$ with`),
/// with a global fallback. This mirrors the C++ module-resolution layer, which
/// namespaces every type to its owning module.
#[derive(Default)]
pub struct Decls {
    /// module name -> (struct name -> field names, in declaration order).
    structs: HashMap<String, HashMap<String, Vec<String>>>,
    /// module name -> (variant tag -> its union name and payload field names).
    unions: HashMap<String, HashMap<String, VariantDecl>>,
    /// `with Other` splices to apply once every module is collected:
    /// `(module, type, is_struct, included)`. The checker has already validated it.
    includes: Vec<(String, String, bool, Vec<String>)>,
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
        decls.resolve_includes();
        decls
    }

    fn add(&mut self, ast: &Ast, program: &AstProgram) {
        let module = ast.text(program.module).to_string();
        for item in program.items.iter() {
            match item {
                Item::Struct {
                    name,
                    includes,
                    fields,
                    ..
                } => {
                    let names = fields
                        .iter()
                        .map(|f| ast.text(f.name).to_string())
                        .collect();
                    let name = ast.text(*name).to_string();
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| ast.text(*p).to_string()).collect();
                        self.includes
                            .push((module.clone(), name.clone(), true, ps));
                    }
                    self.structs
                        .entry(module.clone())
                        .or_default()
                        .insert(name, names);
                }
                Item::Union {
                    name,
                    includes,
                    variants,
                    ..
                } => {
                    let uname = ast.text(*name).to_string();
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| ast.text(*p).to_string()).collect();
                        self.includes
                            .push((module.clone(), uname.clone(), false, ps));
                    }
                    for v in variants.iter() {
                        let fields = match &v.payload {
                            Payload::None => Vec::new(),
                            Payload::Bare(_) => vec![None],
                            Payload::Fields(fs) => fs
                                .iter()
                                .map(|f| f.name.map(|n| ast.text(n).to_string()))
                                .collect(),
                        };
                        self.unions.entry(module.clone()).or_default().insert(
                            ast.text(v.tag).to_string(),
                            VariantDecl {
                                union: uname.clone(),
                                fields,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// Copy `with Other` members into each splicing type's field / variant tables,
    /// matching the checker. Runs a fixpoint so an included type that itself
    /// splices is copied first. The checker has already rejected cycles / kind
    /// mismatches, so anything left unresolved is simply skipped.
    fn resolve_includes(&mut self) {
        let mut pending = std::mem::take(&mut self.includes);
        loop {
            let waiting: HashSet<String> = pending.iter().map(|(_, c, _, _)| c.clone()).collect();
            // A type is ready when none of the types it includes is still waiting.
            let ready = |included: &[String]| included.iter().all(|p| !waiting.contains(p));
            let (now, later): (Vec<_>, Vec<_>) =
                pending.into_iter().partition(|(_, _, _, ps)| ready(ps));
            if now.is_empty() {
                break; // nothing more resolvable (a cycle the checker would have caught)
            }
            for (module, ty, is_struct, included) in now {
                if is_struct {
                    let mut fields = Vec::new();
                    for p in &included {
                        if let Some(pf) = self.lookup_struct(&module, p) {
                            fields.extend(pf.iter().cloned());
                        }
                    }
                    if let Some(own) = self.structs.get(&module).and_then(|m| m.get(&ty)) {
                        fields.extend(own.iter().cloned());
                    }
                    self.structs.entry(module).or_default().insert(ty, fields);
                } else {
                    let copied = self.included_variants(&module, &included, &ty);
                    let map = self.unions.entry(module).or_default();
                    for (tag, decl) in copied {
                        map.insert(tag, decl);
                    }
                }
            }
            pending = later;
        }
    }

    /// A struct's field names, resolved from `module` first then any module.
    fn lookup_struct(&self, module: &str, name: &str) -> Option<Vec<String>> {
        self.structs
            .get(module)
            .and_then(|m| m.get(name))
            .or_else(|| self.structs.values().find_map(|m| m.get(name)))
            .cloned()
    }

    /// The variants each included union contributes to `ty`, retagged to it.
    fn included_variants(
        &self,
        module: &str,
        included: &[String],
        ty: &str,
    ) -> Vec<(String, VariantDecl)> {
        let mut out = Vec::new();
        for p in included {
            // Prefer the included union's own module's variants, else scan globally.
            let scan = |m: &HashMap<String, VariantDecl>| {
                m.iter()
                    .filter(|(_, d)| d.union == *p)
                    .map(|(tag, d)| {
                        (
                            tag.clone(),
                            VariantDecl {
                                union: ty.to_string(),
                                fields: d.fields.clone(),
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let mut found = self.unions.get(module).map(scan).unwrap_or_default();
            if found.is_empty() {
                for m in self.unions.values() {
                    found = scan(m);
                    if !found.is_empty() {
                        break;
                    }
                }
            }
            out.extend(found);
        }
        out
    }

    /// Resolve `pick` against `module`'s own declarations, then each imported
    /// module's, then any module (a last-resort global fallback).
    fn resolve<'d, T>(
        &'d self,
        table: &'d HashMap<String, HashMap<String, T>>,
        module: &str,
        imports: &[String],
        key: &str,
    ) -> Option<&'d T> {
        table
            .get(module)
            .and_then(|m| m.get(key))
            .or_else(|| imports.iter().find_map(|i| table.get(i).and_then(|m| m.get(key))))
            .or_else(|| table.values().find_map(|m| m.get(key)))
    }

    /// The payload arity and (union, field names) for a variant tag. `List`'s
    /// `Cons`/`Nil` are prelude, so they are answered directly.
    fn variant(
        &self,
        module: &str,
        imports: &[String],
        tag: &str,
    ) -> Option<(&str, &[Option<String>])> {
        match tag {
            "Cons" => Some(("List", CONS_FIELDS)),
            "Nil" => Some(("List", &[])),
            _ => self
                .resolve(&self.unions, module, imports, tag)
                .map(|v| (v.union.as_str(), v.fields.as_slice())),
        }
    }

    /// A struct's field names in declaration order, if `name` is a known struct.
    fn fields_of(&self, module: &str, imports: &[String], name: &str) -> Option<&[String]> {
        self.resolve(&self.structs, module, imports, name)
            .map(Vec::as_slice)
    }

    /// The struct whose exact set of field names matches an all-named literal,
    /// preferring the lowering module's own then imported declarations.
    fn struct_by_fields(&self, module: &str, imports: &[String], names: &[&str]) -> Option<&str> {
        fn hit<'d>(structs: &'d HashMap<String, Vec<String>>, names: &[&str]) -> Option<&'d str> {
            structs.iter().find_map(|(sname, fields)| {
                let same = fields.len() == names.len()
                    && names.iter().all(|n| fields.iter().any(|f| f == n));
                same.then_some(sname.as_str())
            })
        }
        self.structs
            .get(module)
            .and_then(|s| hit(s, names))
            .or_else(|| {
                imports
                    .iter()
                    .find_map(|i| self.structs.get(i).and_then(|s| hit(s, names)))
            })
            .or_else(|| self.structs.values().find_map(|s| hit(s, names)))
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
/// One resolved implicit (`@ctx`) argument at a use site, ready for lowering to
/// inject as a leading argument of the referenced function.
#[derive(Clone, Debug)]
pub enum ImplicitArg {
    /// A bare name: a local binder (the caller's own `@ctx` param) or a builtin;
    /// De-Bruijn / the runtime resolves it.
    Bare(String),
    /// A top-level value `module.name` (already type-mangled if overloaded).
    Qualified { module: String, name: String },
    /// An explicit override expression from `@ctx e` / `@ctx { .c = e }`; lowering
    /// lowers this AST node in place.
    Expr(Aol<Expr>),
}

#[derive(Default)]
pub struct Resolved {
    pub array_exprs: HashSet<Aol<Expr>>,
    pub array_pats: HashSet<Aol<Pattern>>,
    /// `[..]` literal sites resolved to a sized tensor (a vector value), from
    /// [`crate::typing::Checker::tensor_nodes`]. Lowering builds a vector.
    pub tensor_exprs: HashSet<Aol<Expr>>,
    /// Argument sites promoted to a record, mapped to the target field names (from
    /// [`crate::typing::Checker::promotions`]); lowering wraps the value.
    pub promotions: HashMap<Aol<Expr>, Vec<String>>,
    /// `.{ .. }` struct-literal sites mapped to their resolved struct name (from
    /// [`crate::typing::Checker::struct_lit_names`]); lowering uses it for the
    /// field-name layout of a positional literal.
    pub struct_lit_names: HashMap<Aol<Expr>, String>,
    /// `{ .obs = e }` codata-construction sites (each clause becomes a thunk).
    pub codata_lits: HashSet<Aol<Expr>>,
    /// `x.obs` observation sites (lowered to running the thunk: `field {}`).
    pub observations: HashSet<Aol<Expr>>,
    pub call_modules: HashMap<Aol<Expr>, String>,
    /// Each use site of a `@ctx`-bearing function, mapped to the ordered implicit
    /// arguments lowering injects ahead of the explicit ones (from
    /// [`crate::typing::Checker::implicit_calls`]).
    pub implicit_args: HashMap<Aol<Expr>, Vec<ImplicitArg>>,
    /// Overloaded-call `Expr::Var` sites whose target module defines the name more
    /// than once, mapped to the type-mangled bare name lowering emits in place of
    /// the source name (from [`crate::typing::Checker::overload_calls`]). The
    /// qualifying module still comes from `call_modules`.
    pub overload_calls: HashMap<Aol<Expr>, String>,
    /// Same-module overloaded definitions, keyed by body handle, mapped to the
    /// type-mangled bare name lowering gives the global (from
    /// [`crate::typing::Checker::def_keys`]), so the overloads stay distinct.
    pub def_keys: HashMap<Aol<Expr>, String>,
    /// The ordered field names each `with` expression binds, keyed by the `With`
    /// node (from [`crate::typing::Checker::with_fields`]). Lowering desugars
    /// `with` into a `let` per field so the Core carries no `with` node.
    pub with_fields: HashMap<Aol<Expr>, Vec<String>>,
    /// Each `@extern` node's marshalling signature (flattened argument type names
    /// and result name) from [`crate::typing::Checker::extern_sigs`]. Lowering
    /// needs the types to build the `Term::Extern` the checker erased into a var.
    pub extern_sigs: HashMap<Aol<Expr>, (Vec<String>, String)>,
}

/// Lower one module's globals to Core.
pub fn lower_program(
    ast: &Ast,
    program: &AstProgram,
    decls: &Decls,
    resolved: &Resolved,
) -> Program {
    let imports = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Import { module, .. } => Some(
                module
                    .iter()
                    .map(|&p| ast.text(p))
                    .collect::<Vec<_>>()
                    .join("."),
            ),
            _ => None,
        })
        .collect();
    let mut lw = Lowerer {
        ast,
        decls,
        resolved,
        module: ast.text(program.module).to_string(),
        imports,
        fresh: 0,
    };
    let mut effects = Vec::new();
    let mut globals = Vec::new();
    for item in program.items.iter() {
        match item {
            Item::Def {
                name,
                sig,
                implicits,
                body,
            } => {
                let term = lw.def(*sig, implicits, *body);
                let key = resolved
                    .def_keys
                    .get(body)
                    .cloned()
                    .unwrap_or_else(|| ast.text(*name).to_string());
                globals.push((key, term));
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
    /// This lowering's own `@mod` name and the modules it imports (`$ with`),
    /// used to resolve a same-named type to the right module's declaration.
    module: String,
    imports: Vec<String>,
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

    /// The head type constructor's name, if `t` is a (possibly applied) `Con`.
    fn ty_head_con(&self, t: Aol<Ty>) -> Option<&'a str> {
        match self.tnode(t) {
            Ty::Con { name, .. } => Some(self.text(*name)),
            Ty::App(head, _) => self.ty_head_con(*head),
            _ => None,
        }
    }

    /// Desugar `with subject in body` into a `let` per field: bind the subject to
    /// a fresh name (so it is forced once) and each of its `fields` to a field
    /// access on it. This removes the name-binding-by-type `with` node from the
    /// Core, keeping it De-Bruijn indexable.
    fn desugar_with(&mut self, subject: Term, fields: &[String], body: Term) -> Term {
        let s = self.fresh();
        let mut inner = body;
        for f in fields.iter().rev() {
            inner = Term::Let {
                name: f.clone(),
                rec: false,
                val: Arc::new(Term::Field(Arc::new(Term::var(s.clone())), f.clone())),
                body: Arc::new(inner),
            };
        }
        Term::Let {
            name: s,
            rec: false,
            val: Arc::new(subject),
            body: Arc::new(inner),
        }
    }

    /// Lower a definition, consuming leading record parameters of its signature.
    /// `@ctx` implicits become leading lambda parameters (dictionary passing): the
    /// body binds them by name, and each call site injects the resolved values as
    /// leading arguments (see [`Self::expr`]'s `Var` case).
    fn def(&mut self, sig: Option<Aol<Ty>>, implicits: &[FieldDecl], body: Aol<Expr>) -> Term {
        let term = self.expr(body);
        let mut inner = self.record_params(sig, term);
        for f in implicits.iter().rev() {
            inner = Term::Lam {
                param: self.text(f.name).to_string(),
                body: Arc::new(inner),
            };
        }
        inner
    }

    fn record_params(&mut self, sig: Option<Aol<Ty>>, body: Term) -> Term {
        let Some(sig) = sig else { return body };
        let Ty::Arrow { from, to, .. } = self.tnode(sig) else {
            return body;
        };
        let (from, to) = (*from, *to);
        // The body's leading lambdas bind the LAST k of the m signature parameters,
        // so a parameter is only auto-bound when the lambdas do not reach it
        // (`k < m`). This lets `\p = p.x` name a record parameter explicitly, while
        // still auto-binding a record ahead of a later explicit lambda parameter.
        let m = 1 + self.arrow_arity(to);
        if leading_lams(&body) >= m {
            return body;
        }
        // A CLOSED record parameter is the destructuring sugar; an open `{ x | r }`
        // is a real record value (plain parameter); a unit parameter takes no
        // fields, so the sugar just introduces the thunk parameter (`f : {} -> T`).
        let fields: &[RecField] = match self.tnode(from) {
            Ty::Record { fields, tail: None } => fields,
            Ty::Unit => &[],
            _ => return body,
        };
        let inner = self.record_params(Some(to), body);
        self.record_param(fields, inner)
    }

    /// The number of parameters in a signature's arrow spine.
    fn arrow_arity(&self, ty: Aol<Ty>) -> usize {
        let mut n = 0;
        let mut cur = ty;
        while let Ty::Arrow { to, .. } = self.tnode(cur) {
            n += 1;
            cur = *to;
        }
        n
    }

    /// Wrap `body` in the lambda binding a record parameter's fields by name. The
    /// parameter is a name-keyed record value, so each field is bound by field
    /// access (`param.field`); the field bindings are outermost so a `with` field's
    /// scoping can reference them.
    fn record_param(&mut self, fields: &[RecField], body: Term) -> Term {
        let param = self.fresh();
        let mut inner = body;
        for f in fields.iter().rev() {
            if f.with {
                let names: Vec<String> = self
                    .ty_head_con(f.ty)
                    .and_then(|s| self.decls.fields_of(&self.module, &self.imports, s))
                    .map(<[String]>::to_vec)
                    .unwrap_or_default();
                let subject = Term::var(self.text(f.name));
                inner = self.desugar_with(subject, &names, inner);
            }
        }
        for f in fields.iter().rev() {
            let name = self.text(f.name).to_string();
            let val = Term::Field(Arc::new(Term::var(param.clone())), name.clone());
            inner = Term::Let {
                name,
                rec: false,
                val: Arc::new(val),
                body: Arc::new(inner),
            };
        }
        Term::Lam {
            param,
            body: Arc::new(inner),
        }
    }

    fn expr(&mut self, e: Aol<Expr>) -> Term {
        // A promoted argument (scalar or positional tuple passed where a record is
        // expected) is wrapped into a name-keyed record here.
        if let Some(names) = self.resolved.promotions.get(&e) {
            let names = names.clone();
            return self.promote_to_record(e, &names);
        }
        self.expr_core(e)
    }

    /// Wrap a promoted argument into a record with `names`: a positional tuple
    /// literal maps its elements to the names; a scalar becomes a one-field record;
    /// a tuple-typed value is bound once and projected by index.
    fn promote_to_record(&mut self, e: Aol<Expr>, names: &[String]) -> Term {
        if let Expr::Tuple(items) = self.node(e) {
            let items: Vec<Aol<Expr>> = items.to_vec();
            let fields: Vec<(String, Term)> = names
                .iter()
                .cloned()
                .zip(items.into_iter().map(|it| self.expr(it)))
                .collect();
            return Term::Struct {
                name: String::new(),
                base: None,
                fields: Arc::from(fields),
            };
        }
        let val = self.expr_core(e);
        if names.len() == 1 {
            return Term::Struct {
                name: String::new(),
                base: None,
                fields: Arc::from([(names[0].clone(), val)]),
            };
        }
        let t = self.fresh();
        let fields: Vec<(String, Term)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.clone(),
                    Term::Field(Arc::new(Term::var(t.clone())), i.to_string()),
                )
            })
            .collect();
        Term::Let {
            name: t.clone(),
            rec: false,
            val: Arc::new(val),
            body: Arc::new(Term::Struct {
                name: String::new(),
                base: None,
                fields: Arc::from(fields),
            }),
        }
    }

    fn expr_core(&mut self, e: Aol<Expr>) -> Term {
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
                        // A same-module overload resolves to a type-mangled global
                        // name; other references keep the source name.
                        let name = self
                            .resolved
                            .overload_calls
                            .get(&e)
                            .cloned()
                            .unwrap_or_else(|| name.to_string());
                        let base = Term::Var {
                            module,
                            name,
                            idx: 0,
                        };
                        // A `@ctx`-bearing function takes its implicits as leading
                        // arguments (dictionary passing); inject the resolved ones
                        // ahead of the explicit application.
                        self.apply_implicits(e, base)
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
                    let mut acc = Term::app(Term::var("@array_alloc"), Term::Int(0));
                    for it in items {
                        let x = self.expr(it);
                        acc = Term::app(Term::app(Term::var("@array_push"), acc), x);
                    }
                    acc
                } else if self.resolved.tensor_exprs.contains(&e) {
                    // A sized tensor: a vector, pushed left to right onto an empty one.
                    let mut acc = Term::app(Term::var("@vec_new"), Term::Unit);
                    for it in items {
                        let x = self.expr(it);
                        acc = Term::app(Term::app(Term::var("@vec_push"), acc), x);
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

            Expr::Array { size } => Term::app(Term::var("@array_alloc"), self.expr(*size)),

            Expr::Field { record, name } => {
                let (record, name) = (*record, self.text(*name).to_string());
                let field = Term::Field(Arc::new(self.expr(record)), name);
                // A codata observation runs the stored thunk (`field {}`).
                if self.resolved.observations.contains(&e) {
                    Term::app(field, Term::Unit)
                } else {
                    field
                }
            }

            Expr::StructLit { ty, fields, spread } => {
                // The nominal struct name comes from the AST (`Type.{..}`) or, for a
                // bare `.{..}`, from the checker's resolution -- so a positional
                // literal gets the right field-name layout.
                let ty = ty
                    .map(|t| self.text(t).to_string())
                    .or_else(|| self.resolved.struct_lit_names.get(&e).cloned());
                let spread = *spread;
                self.struct_lit(ty.as_deref(), fields, spread)
            }
            Expr::Record {
                fields,
                with,
                update,
            } => {
                // Codata construction: each observation clause becomes a thunk
                // (`\%u = clause`), so construction is finite and observing runs the
                // clause afresh (non-memoized). Observing (see `Expr::Field`) applies
                // the thunk to unit.
                if self.resolved.codata_lits.contains(&e) {
                    let obs: Vec<(String, Term)> = fields
                        .iter()
                        .filter_map(|fi| match fi {
                            FieldInit::Named { name, value } => {
                                let body = self.expr(*value);
                                Some((
                                    self.text(*name).to_string(),
                                    Term::Lam {
                                        param: self.fresh(),
                                        body: Arc::new(body),
                                    },
                                ))
                            }
                            FieldInit::Positional(_) => None,
                        })
                        .collect();
                    return Term::Struct {
                        name: String::new(),
                        base: None,
                        fields: Arc::from(obs),
                    };
                }
                // A name-keyed record value. Update (`| base`) and stack (`with
                // base`) build over that base with the listed fields overriding /
                // adding; an anonymous record has no nominal name (access is
                // name-keyed, so the empty name is fine).
                let base = (*update).or(*with).map(|b| Arc::new(self.expr(b)));
                let mut out = Vec::new();
                for fi in fields.iter() {
                    if let FieldInit::Named { name, value } = fi {
                        out.push((self.text(*name).to_string(), self.expr(*value)));
                    }
                }
                Term::Struct {
                    name: String::new(),
                    base,
                    fields: Arc::from(out),
                }
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
                let subject_t = self.expr(subject);
                let body_t = self.expr(body);
                let fields = self.resolved.with_fields.get(&e).cloned().unwrap_or_default();
                self.desugar_with(subject_t, &fields, body_t)
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
            Expr::Extern { abi, symbol, lib } => {
                let abi = self.text(*abi).to_string();
                let symbol = self.text(*symbol).to_string();
                let lib = self.text(*lib).to_string();
                match self.resolved.extern_sigs.get(&e) {
                    Some((arg_types, ret_type)) => Term::Extern {
                        abi,
                        symbol,
                        lib,
                        arg_types: arg_types.iter().cloned().collect(),
                        ret_type: ret_type.clone(),
                    },
                    // No checker resolution (e.g. an untyped standalone lowering):
                    // an extern with an unknown signature cannot be marshalled.
                    None => Term::Fault(format!("foreign function `{symbol}` has no resolved type")),
                }
            }

            // `callee @ctx ...` is transparent here: the implicit arguments (given
            // or resolved) are injected at the head function reference, keyed by its
            // site in `implicit_args`.
            Expr::Ctx { callee, .. } => self.expr(*callee),
        }
    }

    /// Wrap a `@ctx`-bearing function reference in applications of its resolved
    /// implicit arguments (leading, so they precede the explicit application).
    fn apply_implicits(&mut self, site: Aol<Expr>, base: Term) -> Term {
        let Some(args) = self.resolved.implicit_args.get(&site) else {
            return base;
        };
        let args = args.clone();
        let mut term = base;
        for a in args {
            let arg = match a {
                ImplicitArg::Bare(name) => Term::var(name),
                ImplicitArg::Qualified { module, name } => Term::Var {
                    module: Some(module),
                    name,
                    idx: 0,
                },
                ImplicitArg::Expr(e) => self.expr(e),
            };
            term = Term::app(term, arg);
        }
        term
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
            .and_then(|n| self.decls.fields_of(&self.module, &self.imports, n))
            .map(<[String]>::to_vec)
            .or_else(|| {
                named.as_ref().and_then(|ns| {
                    self.decls
                        .struct_by_fields(&self.module, &self.imports, ns)
                        .map(|_| ns.iter().map(|s| s.to_string()).collect())
                })
            });

        let name = ty
            .map(str::to_string)
            .or_else(|| {
                named.and_then(|ns| {
                    self.decls
                        .struct_by_fields(&self.module, &self.imports, &ns)
                        .map(str::to_string)
                })
            })
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
        let (union, names) = match self.decls.variant(&self.module, &self.imports, tag) {
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

    /// A range pattern's bound as a literal term (the parser guarantees numeric).
    fn range_bound(&self, p: Aol<Pattern>) -> Term {
        match self.pnode(p) {
            Pattern::Int(n) => Term::Int(*n),
            Pattern::Real(r) => Term::Real(*r),
            _ => Term::Fault("range bound is not a numeric literal".into()),
        }
    }

    fn pat(&mut self, p: Aol<Pattern>) -> Pat {
        match self.pnode(p) {
            Pattern::Wild => Pat::Wild,
            Pattern::Var(name) => Pat::Var(self.text(*name).to_string()),
            Pattern::Int(n) => Pat::Int(*n),
            Pattern::Real(r) => Pat::Real(*r),
            Pattern::Str(s) => Pat::Str(self.ast.bytes(*s).to_vec()),
            Pattern::Bool(b) => Pat::Bool(*b),
            Pattern::Range { lo, hi } => Pat::Range {
                lo: self.range_bound(*lo),
                hi: self.range_bound(*hi),
            },
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
                let sname = self.text(*ty);
                let names = self
                    .decls
                    .fields_of(&self.module, &self.imports, sname)
                    .map(<[String]>::to_vec);
                let fields: Vec<FieldPat> = fields.to_vec();
                Pat::Struct {
                    fields: self.field_pats(&fields, names.as_deref()),
                    rest: None,
                }
            }
            // A record pattern matches a name-keyed record/struct by field name.
            // `..name` binds the leftover fields; `.._` (a wild rest) discards them.
            Pattern::Record { fields, rest } => {
                let rest = rest.and_then(|r| match self.pnode(r) {
                    Pattern::Var(name) => Some(self.text(*name).to_string()),
                    _ => None,
                });
                let fields: Vec<FieldPat> = fields.to_vec();
                Pat::Struct {
                    fields: self.field_pats(&fields, None),
                    rest,
                }
            }
            Pattern::Variant {
                ty, tag, fields, ..
            } => {
                let tag = self.text(*tag).to_string();
                let ty = ty.map(|t| self.text(t));
                let names = self
                    .decls
                    .variant(&self.module, &self.imports, &tag)
                    .map(|(_, ns)| ns.to_vec())
                    .or_else(|| {
                        ty.and_then(|t| self.decls.fields_of(&self.module, &self.imports, t))
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
    Term::app(Term::var("@array_len"), Term::var(v))
}

/// `array_get v i`.
fn array_get(v: &str, i: usize) -> Term {
    Term::app(
        Term::app(Term::var("@array_get"), Term::var(v)),
        Term::Int(i as i64),
    )
}

/// `array_slice v from (array_len v)` (the open tail from `from`).
fn array_slice(v: &str, from: usize) -> Term {
    Term::app(
        Term::app(
            Term::app(Term::var("@array_slice"), Term::var(v)),
            Term::Int(from as i64),
        ),
        array_len(v),
    )
}

/// A binary operator application `l <op> r`.
fn bin(op: &str, l: Term, r: Term) -> Term {
    Term::app(Term::app(Term::var(op), l), r)
}

/// The number of leading lambdas a body opens with (the explicit parameters the
/// user wrote), so the record-parameter sugar knows which parameters are already
/// bound by hand.
fn leading_lams(mut t: &Term) -> usize {
    let mut n = 0;
    while let Term::Lam { body, .. } = t {
        n += 1;
        t = body;
    }
    n
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
