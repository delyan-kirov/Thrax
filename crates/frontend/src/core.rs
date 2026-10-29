//! The Core: a desugared lambda calculus the front-end AST lowers to.
//!
//! Every surface convenience (operators, `if`, `when`, list/cons sugar, pipes,
//! sequencing, record parameters) is gone. What remains is variables,
//! application, single-parameter lambdas, `let`, one branching form ([`Term::Case`]),
//! literals, and the aggregate constructors/accessors. This is the tree the
//! middle end ([`crate::ir`]) consumes on the way to the `interpreter` and `ccg`
//! backends.
//!
//! Like the front-end AST, the Core is handle-addressed: a [`Term`]'s children
//! are `Aol<Term>` handles into the [`Terms`] store of its [`CoreProgram`], and
//! every name is a [`StrId`]. Nodes are immutable once added. A pass rewrites a
//! program by adding new nodes beside the old ones (a subtree it leaves alone
//! keeps its handle), and an annotation a pass computes goes in a
//! [`SecondaryMap`] keyed by the node it describes ([`CoreProgram::de_bruijn`]).
//! The store carries no lifetime and owns plain data, so a compiled module can
//! be kept across compiles or handed to another thread.

use utilities::{Aol, SecondaryMap, Span, Store, StrId};

/// A whole lowered module: its `@mod` name and its globals in source order.
#[derive(Clone)]
pub struct CoreProgram {
    pub module: StrId,
    /// Every node the globals reach (and possibly some no longer reached, left
    /// behind by a rewrite).
    pub terms: Terms,
    pub effects: Vec<CoreEffect>,
    pub globals: Vec<(StrId, Aol<Term>)>,
    /// C memory layouts of C-repr structs, keyed by type name, for marshalling
    /// struct values across the `@extern` boundary. Same across every module (the
    /// resolver aggregates them); the IR lowering merges duplicates by name.
    pub crepr_layouts: Vec<(StrId, utilities::CLayout)>,
    /// The synthetic globals that back top-level `$ @e <expr>` directives, as
    /// `(name, source span of the `@e X`)`, in source order. The driver forces
    /// each at compile time and discards the value (a trap fails the build, and
    /// the span points the error at the directive). See [`super::desugar_program`].
    pub ct_runs: Vec<(StrId, Span)>,
    /// Expression-position `@e X` sites: `(synthetic global name, source span of
    /// the `@e X`)`. The driver forces the global at compile time, renders the
    /// value/code to source, substitutes it at the span, and re-compiles (the
    /// iterative expansion). See [`super::desugar_program`].
    pub ct_evals: Vec<(StrId, Span)>,
    /// Type-position `@e X` sites (`Foo : @e X = ...`): `(synthetic global name,
    /// source span of the `@e X`)`. The driver forces the global at compile time,
    /// splices the resulting `@Code`'s type source at the span, and re-compiles.
    /// See [`super::desugar_program`].
    pub ct_types: Vec<(StrId, Span)>,
    /// The synthetic global holding the call `@build {}`, present when this module
    /// defines `$ @build`. The driver forces it once per module at compile time and
    /// appends the items of the `@Code` it returns to the module's source. Unlike
    /// the `@e` sites there is no span: `@build` is a definition, not a directive,
    /// so it stays where it was written and its output is added to the module.
    pub ct_build: Option<StrId>,
    /// The De Bruijn index of each variable occurrence, filled by
    /// [`crate::passes::de_bruijn::assign_de_bruijn_indices`]: `0` (or no
    /// entry) marks a global, resolved by `module`/`name`, and a positive index
    /// counts outward through the binder stack to a local. The closure converter
    /// reads it to split a variable into a local slot or an environment capture.
    pub de_bruijn: SecondaryMap<Term, usize>,
}

impl CoreProgram {
    /// An empty program for module `module`.
    pub fn new(module: StrId) -> CoreProgram {
        CoreProgram {
            module,
            terms: Terms::new(),
            effects: Vec::new(),
            globals: Vec::new(),
            crepr_layouts: Vec::new(),
            ct_runs: Vec::new(),
            ct_evals: Vec::new(),
            ct_types: Vec::new(),
            ct_build: None,
            de_bruijn: SecondaryMap::new(),
        }
    }

    /// Replace each global's body with `f` of it, in order.
    pub fn rewrite_globals(&mut self, mut f: impl FnMut(&mut Terms, Aol<Term>) -> Aol<Term>) {
        for i in 0..self.globals.len() {
            let body = self.globals[i].1;
            self.globals[i].1 = f(&mut self.terms, body);
        }
    }
}

/// One effect operation declared by `$ Effect : @effect = op : ...`.
#[derive(Clone, Debug)]
pub struct CoreEffect {
    pub effect: StrId,
    pub op: StrId,
}

/// A Core term. Children are handles into the program's [`Terms`].
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Int(i64),
    Real(f64),
    Str(StrId),
    Bool(bool),
    Unit,
    /// A variable. `module` is set for a qualified reference (`MOD.name`); an
    /// unqualified name resolves through the local environment, then the globals,
    /// then the built-in operators. Its De Bruijn index is a side table,
    /// [`CoreProgram::de_bruijn`].
    Var {
        module: Option<StrId>,
        name: StrId,
    },
    /// Application of one argument (curried).
    App(Aol<Term>, Aol<Term>),
    /// A single-parameter lambda; multi-parameter surface lambdas curry into a
    /// nest of these, and a pattern parameter becomes a lambda over a fresh name
    /// whose body is a [`Term::Case`] that destructures it.
    Lam {
        param: StrId,
        body: Aol<Term>,
    },
    /// `let name = val in body`. `rec` marks a self-recursive binding (the name is
    /// in scope inside `val`), used for a plain-variable binder.
    Let {
        name: StrId,
        rec: bool,
        val: Aol<Term>,
        body: Aol<Term>,
    },
    /// The one branching form: force `scrut`, take the first arm whose pattern
    /// matches (and whose guard, if any, holds), else `default`. `if` and `is`
    /// both lower here. See Note [Case is never lowered to application].
    Case {
        scrut: Aol<Term>,
        arms: Vec<CoreArm>,
        default: Option<Aol<Term>>,
    },
    Tuple(Vec<Aol<Term>>),
    /// A struct literal, fields in declaration order (named after lowering). When
    /// `base` is set the literal is a record update: the base struct's fields
    /// seed the value and the listed fields override (or extend) them.
    Struct {
        name: StrId,
        base: Option<Aol<Term>>,
        fields: Vec<(StrId, Aol<Term>)>,
    },
    /// A union construction: the union type name, the tag, and the payload in the
    /// variant's declared order.
    Variant {
        ty: StrId,
        tag: StrId,
        fields: Vec<Aol<Term>>,
    },
    /// Field access `record.field`; a numeric `field` indexes a tuple.
    Field(Aol<Term>, StrId),
    /// `do body ctl k ...`: install an algebraic-effect handler around `body`.
    Handle {
        body: Aol<Term>,
        handler: Box<CoreHandler>,
    },
    /// `defer cleanup in body`: run `cleanup` when `body`'s dynamic scope exits.
    /// That is on normal completion of `body`, when a continuation capturing it is
    /// resumed to completion, or when a handler abandons such a continuation
    /// (`cleanup` still runs, under the enclosing handlers). Nested defers run
    /// innermost-first.
    Defer {
        cleanup: Aol<Term>,
        body: Aol<Term>,
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
    /// into positional arguments at the call site (see `Desugarer::expr`), so this
    /// node never sees or materializes it.
    Extern {
        abi: StrId,
        symbol: StrId,
        lib: StrId,
        arg_types: Vec<StrId>,
        ret_type: StrId,
    },
    /// A form the interpreter does not yet support. It raises a runtime fault
    /// only if actually forced, so a module may still run the globals that avoid
    /// it.
    Fault(String),
}

/// One arm of a [`Term::Case`]: a pattern, an optional guard, and a body. Surface
/// or-patterns (`is p1 is p2 then e`) expand into one arm per alternative sharing
/// the guard and body.
#[derive(Clone, Debug, PartialEq)]
pub struct CoreArm {
    pub pat: Pat,
    pub guard: Option<Aol<Term>>,
    pub body: Aol<Term>,
}

/// A Core pattern: the surface patterns with sugar resolved (lists and `::` are
/// `List` variants, struct/variant field patterns are positional-or-named against
/// the declaration).
#[derive(Clone, Debug, PartialEq)]
pub enum Pat {
    Wild,
    Var(StrId),
    Int(i64),
    Real(f64),
    Str(StrId),
    Bool(bool),
    Tuple(Vec<Pat>),
    /// Match a union value by `tag`, binding the payload positionally.
    Variant {
        tag: StrId,
        fields: Vec<Pat>,
    },
    /// Match a struct: each `(field, subpattern)` is checked by field name; fields
    /// not listed are ignored. `rest`, when set, binds the leftover fields (the
    /// record minus the listed labels) to that name (`..name` in a record pattern).
    Struct {
        fields: Vec<(StrId, Pat)>,
        rest: Option<StrId>,
    },
    /// A literal byte-string prefix `"GET " ++ rest`, or with `back` a suffix
    /// `rest ++ ".thx"`. `rest` matches what is left.
    StrPrefix {
        prefix: StrId,
        back: bool,
        rest: Box<Pat>,
    },
    /// An inclusive numeric range `lo ... hi`: matches when `lo <= x <= hi`. `lo`
    /// and `hi` are numeric literal terms; compiled to two comparison tests. An open
    /// range `lo ...` has no `hi` and matches when `lo <= x` (one test).
    Range {
        lo: Aol<Term>,
        hi: Option<Aol<Term>>,
    },
    /// A literal pattern on a USER type (Stage 2): matches when the resolved `IEq`
    /// method `eq scrut value` is `@true`, where `value` is the literal built into the
    /// user type (its construction interface applied to the raw payload). Binds nothing.
    /// Compiled to a boolean test by match compilation, so it never reaches the later passes.
    HookEq {
        eq: Aol<Term>,
        value: Aol<Term>,
    },
    /// A sequence pattern on a USER type (Stage 2): matches by unfolding the
    /// `@ISeqView` instance's method (`view`) one step at a time. `elems` are the
    /// leading element patterns; `rest`, when present, binds the remaining sequence,
    /// else the tail must be `Empty`. Compiled to nested `SeqView` (`More`/`Empty`)
    /// matches by match compilation, so it never reaches the later passes.
    /// With `back`, `view` is an `@ISeqViewBack` method and `elems` are the
    /// trailing element patterns, matched from the last one inward.
    SeqView {
        view: Aol<Term>,
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
#[derive(Clone, Debug, PartialEq)]
pub struct CoreHandler {
    pub continuation: StrId,
    pub clauses: Vec<CoreClause>,
    pub default: Option<(StrId, Aol<Term>)>,
}

/// One `is Effect.op arg = body` handler clause.
#[derive(Clone, Debug, PartialEq)]
pub struct CoreClause {
    pub effect: Option<StrId>,
    pub op: StrId,
    pub arg: StrId,
    pub body: Aol<Term>,
}

/// The store of a program's [`Term`] nodes.
#[derive(Clone, Default)]
pub struct Terms {
    nodes: Store<Term>,
}

impl Terms {
    pub fn new() -> Terms {
        Terms::default()
    }

    /// Add a node and return its handle.
    pub fn add(&mut self, t: Term) -> Aol<Term> {
        self.nodes.create(t)
    }

    pub fn get(&self, t: Aol<Term>) -> &Term {
        self.nodes.lookup(t)
    }

    /// The number of nodes stored, reached or not.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

// ## Builders

impl Terms {
    /// `f x`.
    pub fn app(&mut self, f: Aol<Term>, x: Aol<Term>) -> Aol<Term> {
        self.add(Term::App(f, x))
    }

    /// An unqualified variable.
    pub fn var(&mut self, name: impl Into<StrId>) -> Aol<Term> {
        let name = name.into();
        self.add(Term::Var { module: None, name })
    }

    /// `op l r`: a binary built-in applied to both operands.
    pub fn bin(&mut self, op: &str, l: Aol<Term>, r: Aol<Term>) -> Aol<Term> {
        let op = self.var(op);
        let partial = self.app(op, l);
        self.app(partial, r)
    }

    /// `\param = body`.
    pub fn lam(&mut self, param: StrId, body: Aol<Term>) -> Aol<Term> {
        self.add(Term::Lam { param, body })
    }

    /// A non-recursive `let name = val in body`.
    pub fn let_(&mut self, name: StrId, val: Aol<Term>, body: Aol<Term>) -> Aol<Term> {
        self.add(Term::Let { name, rec: false, val, body })
    }

    /// `record.field`.
    pub fn field(&mut self, record: Aol<Term>, field: impl Into<StrId>) -> Aol<Term> {
        self.add(Term::Field(record, field.into()))
    }

    /// Whether the term is an atom: a value that takes no evaluation step to
    /// produce (a literal, a variable, a foreign function, or a lambda). A
    /// [`Term::Fault`] is not one, since it raises when forced.
    pub fn is_atom(&self, t: Aol<Term>) -> bool {
        matches!(
            self.get(t),
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

    /// Copy the tree under `t` from `src` into this store.
    pub fn import(&mut self, src: &Terms, t: Aol<Term>) -> Aol<Term> {
        let node = map_node(src.get(t), &mut |child, _| self.import(src, child));
        self.add(node)
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

impl Terms {
    /// Visit each immediate subterm of `t`, in evaluation order, with the names
    /// it binds around it (innermost last). See Note [Core binder discipline].
    pub fn for_each_child(&self, t: Aol<Term>, f: &mut impl FnMut(Aol<Term>, &[StrId])) {
        map_node(self.get(t), &mut |child, bound| {
            f(child, bound);
            child
        });
    }

    /// `t` with each immediate subterm replaced by `f` of it, called in the same
    /// order and with the same binders as [`Terms::for_each_child`]. When `f`
    /// hands every child back unchanged, so is `t`.
    pub fn map_children(
        &mut self,
        t: Aol<Term>,
        f: &mut impl FnMut(&mut Terms, Aol<Term>, &[StrId]) -> Aol<Term>,
    ) -> Aol<Term> {
        let node = self.get(t).clone();
        let mapped = map_node(&node, &mut |child, bound| f(self, child, bound));
        if mapped == node {
            t
        } else {
            self.add(mapped)
        }
    }

    /// Whether `name` occurs as an unqualified variable anywhere under `t`.
    /// Shadowing is ignored, so this over-approximates.
    pub fn mentions(&self, t: Aol<Term>, name: StrId) -> bool {
        if let Term::Var { module: None, name: n, .. } = self.get(t) {
            return *n == name;
        }
        let mut hit = false;
        self.for_each_child(t, &mut |c, _| hit = hit || self.mentions(c, name));
        hit
    }
}

impl Terms {
    /// The tree under `t` as an S-expression, `(App (Var f) (Int 1))`, for tests
    /// and debugging. Patterns print as their `Debug` form.
    pub fn render(&self, t: Aol<Term>) -> String {
        let node = self.get(t);
        let head = match node {
            Term::Int(n) => return format!("(Int {n})"),
            Term::Real(r) => return format!("(Real {r})"),
            Term::Str(s) => return format!("(Str {s:?})"),
            Term::Bool(b) => return format!("(Bool {b})"),
            Term::Unit => return "Unit".to_string(),
            Term::Var { module: Some(m), name } => return format!("(Var {m}.{name})"),
            Term::Var { module: None, name } => return format!("(Var {name})"),
            Term::Extern { symbol, .. } => return format!("(Extern {symbol})"),
            Term::Fault(msg) => return format!("(Fault {msg:?})"),
            Term::App(..) => "App".to_string(),
            Term::Lam { param, .. } => format!("Lam {param}"),
            Term::Let { name, rec, .. } => format!("{} {name}", if *rec { "LetRec" } else { "Let" }),
            Term::Case { arms, .. } => {
                let pats: Vec<String> = arms.iter().map(|a| format!("{:?}", a.pat)).collect();
                format!("Case [{}]", pats.join(", "))
            }
            Term::Tuple(_) => "Tuple".to_string(),
            Term::Struct { name, fields, .. } => {
                let labels: Vec<&str> = fields.iter().map(|(l, _)| l.as_str()).collect();
                format!("Struct {name} [{}]", labels.join(", "))
            }
            Term::Variant { ty, tag, .. } => format!("Variant {ty}.{tag}"),
            Term::Field(_, field) => format!("Field {field}"),
            Term::Handle { .. } => "Handle".to_string(),
            Term::Defer { .. } => "Defer".to_string(),
        };
        let mut out = format!("({head}");
        self.for_each_child(t, &mut |child, _| {
            out.push(' ');
            out.push_str(&self.render(child));
        });
        out.push(')');
        out
    }
}

/// A function of one child of a node and the names bound around it.
type ChildFn<'f> = dyn FnMut(Aol<Term>, &[StrId]) -> Aol<Term> + 'f;

/// `node` with each child handle replaced by `f` of it, in evaluation order and
/// with the names bound around it. The one definition of a node's children.
/// See Note [Core binder discipline].
fn map_node(node: &Term, f: &mut ChildFn) -> Term {
    match node {
        Term::Int(_)
        | Term::Real(_)
        | Term::Str(_)
        | Term::Bool(_)
        | Term::Unit
        | Term::Var { .. }
        | Term::Extern { .. }
        | Term::Fault(_) => node.clone(),
        Term::App(fun, arg) => {
            let fun = f(*fun, &[]);
            Term::App(fun, f(*arg, &[]))
        }
        Term::Lam { param, body } => Term::Lam { param: *param, body: f(*body, &[*param]) },
        Term::Let { name, rec, val, body } => {
            let own = [*name];
            let val = f(*val, if *rec { &own } else { &[] });
            Term::Let { name: *name, rec: *rec, val, body: f(*body, &own) }
        }
        Term::Case { scrut, arms, default } => {
            let scrut = f(*scrut, &[]);
            let arms = arms
                .iter()
                .map(|arm| {
                    let pat = arm.pat.map_terms(&mut |t| f(t, &[]));
                    let bound = arm.pat.binders();
                    let guard = arm.guard.map(|g| f(g, &bound));
                    CoreArm { pat, guard, body: f(arm.body, &bound) }
                })
                .collect();
            let default = default.map(|d| f(d, &[]));
            Term::Case { scrut, arms, default }
        }
        Term::Tuple(items) => Term::Tuple(items.iter().map(|t| f(*t, &[])).collect()),
        Term::Variant { ty, tag, fields } => Term::Variant {
            ty: *ty,
            tag: *tag,
            fields: fields.iter().map(|t| f(*t, &[])).collect(),
        },
        Term::Struct { name, base, fields } => {
            let base = base.map(|b| f(b, &[]));
            Term::Struct { name: *name, base, fields: fields.iter().map(|(n, t)| (*n, f(*t, &[]))).collect() }
        }
        Term::Field(record, field) => Term::Field(f(*record, &[]), *field),
        Term::Handle { body, handler } => {
            let body = f(*body, &[]);
            let clauses = handler
                .clauses
                .iter()
                .map(|c| CoreClause {
                    effect: c.effect,
                    op: c.op,
                    arg: c.arg,
                    body: f(c.body, &[c.arg, handler.continuation]),
                })
                .collect();
            let default = handler.default.map(|(x, b)| (x, f(b, &[x])));
            Term::Handle {
                body,
                handler: Box::new(CoreHandler { continuation: handler.continuation, clauses, default }),
            }
        }
        Term::Defer { cleanup, body } => {
            let cleanup = f(*cleanup, &[]);
            Term::Defer { cleanup, body: f(*body, &[]) }
        }
    }
}

impl Pat {
    /// The variables the pattern binds, left to right, depth first. This order
    /// is the order of the local slots an arm's binders occupy.
    /// See Note [Core binder discipline].
    pub fn binders(&self) -> Vec<StrId> {
        let mut out = Vec::new();
        self.collect_binders(&mut out);
        out
    }

    fn collect_binders(&self, out: &mut Vec<StrId>) {
        match self {
            Pat::Wild | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => {}
            Pat::Range { .. } | Pat::HookEq { .. } => {}
            Pat::Var(n) => out.push(*n),
            Pat::Tuple(ps) | Pat::Variant { fields: ps, .. } => {
                ps.iter().for_each(|p| p.collect_binders(out));
            }
            Pat::Struct { fields, rest } => {
                fields.iter().for_each(|(_, p)| p.collect_binders(out));
                if let Some(name) = rest {
                    out.push(*name);
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

    /// The pattern with each embedded term (range bounds, a hooked literal's
    /// equality and value, a sequence view) replaced by `f` of it, outermost
    /// first.
    pub fn map_terms(&self, f: &mut impl FnMut(Aol<Term>) -> Aol<Term>) -> Pat {
        let all = |ps: &[Pat], f: &mut dyn FnMut(&Pat) -> Pat| ps.iter().map(|p| f(p)).collect::<Vec<_>>();
        match self {
            Pat::Wild | Pat::Var(_) | Pat::Int(_) | Pat::Real(_) | Pat::Str(_) | Pat::Bool(_) => self.clone(),
            Pat::Tuple(ps) => Pat::Tuple(all(ps, &mut |p| p.map_terms(f))),
            Pat::Variant { tag, fields } => Pat::Variant { tag: *tag, fields: all(fields, &mut |p| p.map_terms(f)) },
            Pat::Struct { fields, rest } => Pat::Struct {
                fields: fields.iter().map(|(n, p)| (*n, p.map_terms(f))).collect(),
                rest: *rest,
            },
            Pat::StrPrefix { prefix, back, rest } => Pat::StrPrefix {
                prefix: *prefix,
                back: *back,
                rest: Box::new(rest.map_terms(f)),
            },
            Pat::Range { lo, hi } => {
                let lo = f(*lo);
                Pat::Range { lo, hi: hi.map(&mut *f) }
            }
            Pat::HookEq { eq, value } => {
                let eq = f(*eq);
                Pat::HookEq { eq, value: f(*value) }
            }
            Pat::SeqView { view, back, elems, rest } => {
                let view = f(*view);
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
    pub fn name(&mut self) -> StrId {
        self.tagged("")
    }

    /// The next name, with `tag` between the prefix and the number.
    pub fn tagged(&mut self, tag: &str) -> StrId {
        let n = self.next;
        self.next += 1;
        StrId::intern(&format!("{}{tag}{n}", self.prefix))
    }
}

#[cfg(test)]
mod tests;
