//! The Core: a desugared lambda calculus the front-end AST lowers to.
//!
//! Every surface convenience (operators, `if`, `when`, list/cons sugar, pipes,
//! sequencing, record parameters) is gone. What remains is variables,
//! application, single-parameter lambdas, `let`, one branching form ([`Term::Case`]),
//! literals, and the aggregate constructors/accessors. This is the tree the
//! middle end ([`crate::ir`]) consumes on the way to the `interpreter` and `ccg`
//! backends.
//!
//! Unlike the front-end AST (handle-addressed nodes in a
//! [`crate::parser::data::Ast`] store), the Core owns its data. It is immutable
//! after lowering, and its child pointers are `Arc<Term>` (the collections the
//! backends walk lazily are `Arc<[T]>`). So it carries no lifetime, a shared
//! subterm is a pointer bump rather than a deep clone, and the whole program is
//! `Send`: a compiled module can be handed to another thread. The interpreter's
//! own per-thread value graph uses `Rc` instead, since it is mutable and never
//! crosses a thread.

use std::sync::Arc;

/// A whole lowered module: its `@mod` name and its globals in source order.
#[derive(Clone)]
pub struct CoreProgram {
    pub module: String,
    pub effects: Vec<CoreEffect>,
    pub globals: Vec<(String, Term)>,
    /// C memory layouts of C-repr structs, keyed by type name, for marshalling
    /// struct values across the `@extern` boundary. Same across every module (the
    /// resolver aggregates them); the IR lowering merges duplicates by name.
    pub crepr_layouts: Vec<(String, utilities::CLayout)>,
    /// The synthetic globals that back top-level `$ @e <expr>` directives, as
    /// `(name, source span of the `@e X`)`, in source order. The driver forces
    /// each at compile time and discards the value (a trap fails the build, and
    /// the span points the error at the directive). See [`super::desugar_program`].
    pub ct_runs: Vec<(String, utilities::Span)>,
    /// Expression-position `@e X` sites: `(synthetic global name, source span of
    /// the `@e X`)`. The driver forces the global at compile time, renders the
    /// value/code to source, substitutes it at the span, and re-compiles (the
    /// iterative expansion). See [`super::desugar_program`].
    pub ct_evals: Vec<(String, utilities::Span)>,
    /// Type-position `@e X` sites (`Foo : @e X = ...`): `(synthetic global name,
    /// source span of the `@e X`)`. The driver forces the global at compile time,
    /// splices the resulting `@Code`'s type source at the span, and re-compiles.
    /// See [`super::desugar_program`].
    pub ct_types: Vec<(String, utilities::Span)>,
    /// The synthetic global holding the call `@build {}`, present when this module
    /// defines `$ @build`. The driver forces it once per module at compile time and
    /// appends the items of the `@Code` it returns to the module's source. Unlike
    /// the `@e` sites there is no span: `@build` is a definition, not a directive,
    /// so it stays where it was written and its output is added to the module.
    pub ct_build: Option<String>,
}

/// One effect operation declared by `$ Effect : @effect = op : ...`.
#[derive(Clone, Debug)]
pub struct CoreEffect {
    pub effect: String,
    pub op: String,
}

/// A Core term.
#[derive(Clone, Debug)]
pub enum Term {
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Bool(bool),
    Unit,
    /// A variable. `module` is set for a qualified reference (`MOD.name`); an
    /// unqualified name resolves through the local environment, then the globals,
    /// then the built-in operators.
    ///
    /// `idx` is a De Bruijn index filled by
    /// [`crate::passes::de_bruijn::assign_de_bruijn_indices`]: `0` marks a
    /// global (resolved by `module`/`name`), and a positive index counts
    /// outward through the binder stack to a local. The closure converter reads
    /// it to split a variable into a local slot or an environment capture.
    Var {
        module: Option<String>,
        name: String,
        idx: usize,
    },
    /// Application of one argument (curried).
    App(Arc<Term>, Arc<Term>),
    /// A single-parameter lambda; multi-parameter surface lambdas curry into a
    /// nest of these, and a pattern parameter becomes a lambda over a fresh name
    /// whose body is a [`Term::Case`] that destructures it.
    Lam {
        param: String,
        body: Arc<Term>,
    },
    /// `let name = val in body`. `rec` marks a self-recursive binding (the name is
    /// in scope inside `val`), used for a plain-variable binder.
    Let {
        name: String,
        rec: bool,
        val: Arc<Term>,
        body: Arc<Term>,
    },
    /// The one branching form: force `scrut`, take the first arm whose pattern
    /// matches (and whose guard, if any, holds), else `default`. `if` and `is`
    /// both lower here. See Note [Case is never lowered to application].
    Case {
        scrut: Arc<Term>,
        arms: Arc<[CoreArm]>,
        default: Option<Arc<Term>>,
    },
    Tuple(Arc<[Term]>),
    /// A struct literal, fields in declaration order (named after lowering). When
    /// `base` is set the literal is a record update: the base struct's fields
    /// seed the value and the listed fields override (or extend) them.
    Struct {
        name: String,
        base: Option<Arc<Term>>,
        fields: Arc<[(String, Term)]>,
    },
    /// A union construction: the union type name, the tag, and the payload in the
    /// variant's declared order.
    Variant {
        ty: String,
        tag: String,
        fields: Arc<[Term]>,
    },
    /// Field access `record.field`; a numeric `field` indexes a tuple.
    Field(Arc<Term>, String),
    /// `do body ctl k ...`: install an algebraic-effect handler around `body`.
    Handle {
        body: Arc<Term>,
        handler: Arc<CoreHandler>,
    },
    /// `defer cleanup in body`: run `cleanup` when `body`'s dynamic scope exits.
    /// That is on normal completion of `body`, when a continuation capturing it is
    /// resumed to completion, or when a handler abandons such a continuation
    /// (`cleanup` still runs, under the enclosing handlers). Nested defers run
    /// innermost-first.
    Defer {
        cleanup: Arc<Term>,
        body: Arc<Term>,
    },
    /// A foreign function bound by `@extern "abi" "symbol" "lib"`. A curried,
    /// first-class value (like a builtin): applying it accumulates arguments and,
    /// once saturated to `arg_types.len()` (a nullary C function still takes one
    /// unit argument), marshals them across the seam and calls `symbol`. `abi`
    /// selects the seam: `"C"` is a C library symbol, `"wasm"` is a host import
    /// supplied by the embedder (the browser playground). `arg_types`/`ret_type`
    /// are the marshalling type names the checker recovered from the declared
    /// signature (`Str`/`Ptr`/`Int`/`Real`/`{}`/sized), driving how each slot
    /// crosses the seam. The record that groups several C parameters is flattened
    /// into positional arguments at the call site (see `Lowering::expr`), so this
    /// node never sees or materializes it.
    Extern {
        abi: String,
        symbol: String,
        lib: String,
        arg_types: Arc<[String]>,
        ret_type: String,
    },
    /// A form the interpreter does not yet support. It raises a runtime fault
    /// only if actually forced, so a module may still run the globals that avoid
    /// it.
    Fault(String),
}

/// One arm of a [`Term::Case`]: a pattern, an optional guard, and a body. Surface
/// or-patterns (`is p1 is p2 then e`) expand into one arm per alternative sharing
/// the guard and body.
#[derive(Clone, Debug)]
pub struct CoreArm {
    pub pat: Pat,
    pub guard: Option<Arc<Term>>,
    pub body: Arc<Term>,
}

/// A Core pattern: the surface patterns with sugar resolved (lists and `::` are
/// `List` variants, struct/variant field patterns are positional-or-named against
/// the declaration).
#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Var(String),
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Bool(bool),
    Tuple(Vec<Pat>),
    /// Match a union value by `tag`, binding the payload positionally.
    Variant {
        tag: String,
        fields: Vec<Pat>,
    },
    /// Match a struct: each `(field, subpattern)` is checked by field name; fields
    /// not listed are ignored. `rest`, when set, binds the leftover fields (the
    /// record minus the listed labels) to that name (`..name` in a record pattern).
    Struct {
        fields: Vec<(String, Pat)>,
        rest: Option<String>,
    },
    /// A literal byte-string prefix `"GET " ++ rest`, or with `back` a suffix
    /// `rest ++ ".thx"`. `rest` matches what is left.
    StrPrefix {
        prefix: Vec<u8>,
        back: bool,
        rest: Box<Pat>,
    },
    /// An inclusive numeric range `lo ... hi`: matches when `lo <= x <= hi`. `lo`
    /// and `hi` are numeric literal terms; compiled to two comparison tests. An open
    /// range `lo ...` has no `hi` and matches when `lo <= x` (one test).
    Range {
        lo: Term,
        hi: Option<Term>,
    },
    /// A literal pattern on a USER type (Stage 2): matches when the resolved `IEq`
    /// method `eq scrut value` is `@true`, where `value` is the literal built into the
    /// user type (its construction interface applied to the raw payload). Binds nothing.
    /// Compiled to a boolean test by match compilation, so it never reaches the later passes.
    HookEq {
        eq: Term,
        value: Box<Term>,
    },
    /// A sequence pattern on a USER type (Stage 2): matches by unfolding the
    /// `@ISeqView` instance's method (`view`) one step at a time. `elems` are the
    /// leading element patterns; `rest`, when present, binds the remaining sequence,
    /// else the tail must be `Empty`. Compiled to nested `SeqView` (`More`/`Empty`)
    /// matches by match compilation, so it never reaches the later passes.
    /// With `back`, `view` is an `@ISeqViewBack` method and `elems` are the
    /// trailing element patterns, matched from the last one inward.
    SeqView {
        view: Term,
        back: bool,
        elems: Vec<Pat>,
        rest: Option<Box<Pat>>,
    },
    /// A sequence pattern on a sized tensor of static length `len`: `front`
    /// elements from index 0, `back` elements ending at `len - 1`, and `mid` the
    /// slice between. The checker has proven the lengths, so nothing is tested.
    Tensor {
        len: u64,
        front: Vec<Pat>,
        mid: Option<Box<Pat>>,
        back: Vec<Pat>,
    },
}

/// A Core effect handler.
#[derive(Clone, Debug)]
pub struct CoreHandler {
    pub continuation: String,
    pub clauses: Vec<CoreClause>,
    pub default: Option<(String, Term)>,
}

/// One `is Effect.op arg = body` handler clause.
#[derive(Clone, Debug)]
pub struct CoreClause {
    pub effect: Option<String>,
    pub op: String,
    pub arg: String,
    pub body: Term,
}

// ## Builders

impl Term {
    /// `f x`.
    pub fn app(f: Term, x: Term) -> Term {
        Term::App(Arc::new(f), Arc::new(x))
    }

    /// An unqualified variable, before De Bruijn indexing.
    pub fn var(name: impl Into<String>) -> Term {
        Term::Var {
            module: None,
            name: name.into(),
            idx: 0,
        }
    }

    /// `op l r`: a binary built-in applied to both operands.
    pub fn bin(op: &str, l: Term, r: Term) -> Term {
        Term::app(Term::app(Term::var(op), l), r)
    }

    /// Whether the term is an atom: a value that takes no evaluation step to
    /// produce (a literal, a variable, a foreign function, or a lambda). A
    /// [`Term::Fault`] is not one, since it raises when forced.
    pub fn is_atom(&self) -> bool {
        matches!(
            self,
            Term::Int(_)
                | Term::Real(_)
                | Term::Str(_)
                | Term::Bool(_)
                | Term::Unit
                | Term::Var { .. }
                | Term::Extern { .. }
                | Term::Lam { .. }
        )
    }
}

// ## Traversal

// Note [Core binder discipline]
//
// Several passes track which names are in scope at a subterm: De Bruijn
// indexing pushes them, closure conversion counts them to find free variables,
// and both must agree exactly or an index points at the wrong slot. So the
// binders a node introduces are defined once, by `for_each_child` and
// `map_children`:
//
// - a lambda binds its parameter in its body;
// - a `let` binds its name in its body, and also in its value when recursive;
// - a case arm binds its pattern's variables (`Pat::binders`, left to right,
//   depth first) in its guard and body, but not in the terms embedded in the
//   pattern itself, which are evaluated in the enclosing scope;
// - a handler clause binds the operation argument, then the continuation
//   (innermost), in its body; the default clause binds its one variable.
//
// Nothing else binds. Children are visited in evaluation order, which the
// passes that generate fresh names rely on to name things deterministically.

impl Term {
    /// Visit each immediate subterm, in evaluation order, with the names this
    /// term binds around it (innermost last). See Note [Core binder discipline].
    pub fn for_each_child<'t>(&'t self, f: &mut impl FnMut(&'t Term, &[&'t str])) {
        match self {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Fault(_) => {}
            Term::App(fun, arg) => {
                f(fun, &[]);
                f(arg, &[]);
            }
            Term::Lam { param, body } => f(body, &[param]),
            Term::Let { name, rec, val, body } => {
                let own = [name.as_str()];
                f(val, if *rec { &own } else { &[] });
                f(body, &own);
            }
            Term::Case { scrut, arms, default } => {
                f(scrut, &[]);
                for arm in arms.iter() {
                    arm.pat.for_each_term(&mut |t| f(t, &[]));
                    let bound = arm.pat.binders();
                    if let Some(guard) = &arm.guard {
                        f(guard, &bound);
                    }
                    f(&arm.body, &bound);
                }
                if let Some(default) = default {
                    f(default, &[]);
                }
            }
            Term::Tuple(items) | Term::Variant { fields: items, .. } => {
                items.iter().for_each(|t| f(t, &[]));
            }
            Term::Struct { base, fields, .. } => {
                if let Some(base) = base {
                    f(base, &[]);
                }
                fields.iter().for_each(|(_, t)| f(t, &[]));
            }
            Term::Field(record, _) => f(record, &[]),
            Term::Handle { body, handler } => {
                f(body, &[]);
                for clause in &handler.clauses {
                    f(&clause.body, &[&clause.arg, &handler.continuation]);
                }
                if let Some((x, body)) = &handler.default {
                    f(body, &[x]);
                }
            }
            Term::Defer { cleanup, body } => {
                f(cleanup, &[]);
                f(body, &[]);
            }
        }
    }

    /// Rebuild this term with each immediate subterm replaced by `f` of it,
    /// called in the same order and with the same binders as
    /// [`Term::for_each_child`].
    pub fn map_children(&self, f: &mut impl FnMut(&Term, &[&str]) -> Term) -> Term {
        let mut one = |t: &Term, bound: &[&str]| Arc::new(f(t, bound));
        match self {
            Term::Int(_)
            | Term::Real(_)
            | Term::Str(_)
            | Term::Bool(_)
            | Term::Unit
            | Term::Var { .. }
            | Term::Extern { .. }
            | Term::Fault(_) => self.clone(),
            Term::App(fun, arg) => {
                let fun = one(fun, &[]);
                Term::App(fun, one(arg, &[]))
            }
            Term::Lam { param, body } => Term::Lam {
                param: param.clone(),
                body: one(body, &[param]),
            },
            Term::Let { name, rec, val, body } => {
                let own = [name.as_str()];
                let val = one(val, if *rec { &own } else { &[] });
                Term::Let {
                    name: name.clone(),
                    rec: *rec,
                    val,
                    body: one(body, &[name]),
                }
            }
            Term::Case { scrut, arms, default } => {
                let scrut = one(scrut, &[]);
                let arms: Vec<CoreArm> = arms
                    .iter()
                    .map(|arm| {
                        let pat = arm.pat.map_terms(&mut |t| f(t, &[]));
                        let bound = arm.pat.binders();
                        let guard = arm.guard.as_ref().map(|g| Arc::new(f(g, &bound)));
                        CoreArm { pat, guard, body: Arc::new(f(&arm.body, &bound)) }
                    })
                    .collect();
                let default = default.as_ref().map(|d| Arc::new(f(d, &[])));
                Term::Case { scrut, arms: arms.into(), default }
            }
            Term::Tuple(items) => Term::Tuple(items.iter().map(|t| f(t, &[])).collect()),
            Term::Variant { ty, tag, fields } => Term::Variant {
                ty: ty.clone(),
                tag: tag.clone(),
                fields: fields.iter().map(|t| f(t, &[])).collect(),
            },
            Term::Struct { name, base, fields } => {
                let base = base.as_ref().map(|b| Arc::new(f(b, &[])));
                Term::Struct {
                    name: name.clone(),
                    base,
                    fields: fields.iter().map(|(n, t)| (n.clone(), f(t, &[]))).collect(),
                }
            }
            Term::Field(record, field) => Term::Field(one(record, &[]), field.clone()),
            Term::Handle { body, handler } => {
                let body = one(body, &[]);
                let clauses = handler
                    .clauses
                    .iter()
                    .map(|c| CoreClause {
                        effect: c.effect.clone(),
                        op: c.op.clone(),
                        arg: c.arg.clone(),
                        body: f(&c.body, &[&c.arg, &handler.continuation]),
                    })
                    .collect();
                let default = handler.default.as_ref().map(|(x, b)| (x.clone(), f(b, &[x])));
                Term::Handle {
                    body,
                    handler: Arc::new(CoreHandler {
                        continuation: handler.continuation.clone(),
                        clauses,
                        default,
                    }),
                }
            }
            Term::Defer { cleanup, body } => {
                let cleanup = one(cleanup, &[]);
                Term::Defer { cleanup, body: one(body, &[]) }
            }
        }
    }

    /// Whether `name` occurs as an unqualified variable anywhere in the term.
    /// Shadowing is ignored, so this over-approximates.
    pub fn mentions(&self, name: &str) -> bool {
        if let Term::Var { module: None, name: n, .. } = self {
            return n == name;
        }
        let mut hit = false;
        self.for_each_child(&mut |t, _| hit = hit || t.mentions(name));
        hit
    }
}

impl Pat {
    /// The variables the pattern binds, left to right, depth first. This order
    /// is the order of the local slots an arm's binders occupy.
    /// See Note [Core binder discipline].
    pub fn binders(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_binders(&mut out);
        out
    }

    fn collect_binders<'p>(&'p self, out: &mut Vec<&'p str>) {
        match self {
            Pat::Wild | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => {}
            Pat::Range { .. } | Pat::HookEq { .. } => {}
            Pat::Var(n) => out.push(n),
            Pat::Tuple(ps) | Pat::Variant { fields: ps, .. } => {
                ps.iter().for_each(|p| p.collect_binders(out));
            }
            Pat::Struct { fields, rest } => {
                fields.iter().for_each(|(_, p)| p.collect_binders(out));
                if let Some(name) = rest {
                    out.push(name);
                }
            }
            Pat::StrPrefix { rest, .. } => rest.collect_binders(out),
            Pat::SeqView { elems, rest, .. } => {
                elems.iter().chain(rest.as_deref()).for_each(|p| p.collect_binders(out));
            }
            Pat::Tensor { front, mid, back, .. } => {
                front.iter().chain(mid.as_deref()).chain(back).for_each(|p| p.collect_binders(out));
            }
        }
    }

    /// Visit each term embedded in the pattern (range bounds, a hooked
    /// literal's equality and value, a sequence view), outermost first.
    pub fn for_each_term<'p>(&'p self, f: &mut impl FnMut(&'p Term)) {
        match self {
            Pat::Wild | Pat::Var(_) | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => {}
            Pat::Tuple(ps) | Pat::Variant { fields: ps, .. } => ps.iter().for_each(|p| p.for_each_term(f)),
            Pat::Struct { fields, .. } => fields.iter().for_each(|(_, p)| p.for_each_term(f)),
            Pat::StrPrefix { rest, .. } => rest.for_each_term(f),
            Pat::Range { lo, hi } => {
                f(lo);
                if let Some(hi) = hi {
                    f(hi);
                }
            }
            Pat::HookEq { eq, value } => {
                f(eq);
                f(value);
            }
            Pat::SeqView { view, elems, rest, .. } => {
                f(view);
                elems.iter().chain(rest.as_deref()).for_each(|p| p.for_each_term(f));
            }
            Pat::Tensor { front, mid, back, .. } => {
                front.iter().chain(mid.as_deref()).chain(back).for_each(|p| p.for_each_term(f));
            }
        }
    }

    /// Rebuild the pattern with each embedded term replaced by `f` of it, in
    /// the order of [`Pat::for_each_term`].
    pub fn map_terms(&self, f: &mut impl FnMut(&Term) -> Term) -> Pat {
        let all = |ps: &[Pat], f: &mut dyn FnMut(&Pat) -> Pat| ps.iter().map(|p| f(p)).collect::<Vec<_>>();
        match self {
            Pat::Wild | Pat::Var(_) | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => self.clone(),
            Pat::Tuple(ps) => Pat::Tuple(all(ps, &mut |p| p.map_terms(f))),
            Pat::Variant { tag, fields } => Pat::Variant {
                tag: tag.clone(),
                fields: all(fields, &mut |p| p.map_terms(f)),
            },
            Pat::Struct { fields, rest } => Pat::Struct {
                fields: fields.iter().map(|(n, p)| (n.clone(), p.map_terms(f))).collect(),
                rest: rest.clone(),
            },
            Pat::StrPrefix { prefix, back, rest } => Pat::StrPrefix {
                prefix: prefix.clone(),
                back: *back,
                rest: Box::new(rest.map_terms(f)),
            },
            Pat::Range { lo, hi } => {
                let lo = f(lo);
                Pat::Range { lo, hi: hi.as_ref().map(|h| f(h)) }
            }
            Pat::HookEq { eq, value } => {
                let eq = f(eq);
                Pat::HookEq { eq, value: Box::new(f(value)) }
            }
            Pat::SeqView { view, back, elems, rest } => {
                let view = f(view);
                Pat::SeqView {
                    view,
                    back: *back,
                    elems: all(elems, &mut |p| p.map_terms(f)),
                    rest: rest.as_ref().map(|r| Box::new(r.map_terms(f))),
                }
            }
            Pat::Tensor { len, front, mid, back } => Pat::Tensor {
                len: *len,
                front: all(front, &mut |p| p.map_terms(f)),
                mid: mid.as_ref().map(|m| Box::new(m.map_terms(f))),
                back: all(back, &mut |p| p.map_terms(f)),
            },
        }
    }
}

// ## Fresh names

/// A supply of names no source program can write: `prefix`, an optional tag,
/// then a counter. Each pass that invents binders owns one, with its own
/// prefix, so the names of different passes never collide.
pub struct FreshNames {
    prefix: &'static str,
    next: usize,
}

impl FreshNames {
    /// A supply whose first name is numbered `first`.
    pub fn new(prefix: &'static str, first: usize) -> FreshNames {
        FreshNames { prefix, next: first }
    }

    /// The next name.
    pub fn name(&mut self) -> String {
        self.tagged("")
    }

    /// The next name, with `tag` between the prefix and the number.
    pub fn tagged(&mut self, tag: &str) -> String {
        let n = self.next;
        self.next += 1;
        format!("{}{tag}{n}", self.prefix)
    }
}

#[cfg(test)]
mod tests;
