//! Algorithm W over the [`crate::parser::data`] handle-based AST.
//!
//! Inference is driven by [`Checker::infer`] (expressions) and
//! [`Checker::type_pattern`] (patterns), threading the [`Engine`] for
//! unification and the lexical scope stack for variable types. The AST is read
//! through a borrowed [`Ast`]: a node handle is resolved with [`Checker::node`] /
//! [`Checker::tnode`] / [`Checker::pnode`], and an interned name with
//! [`Checker::text`]. Because `ast` is a shared reference, those resolve to
//! `'a`-lived data independent of the `&mut self` borrow, so a node can be read
//! and its children inferred in the same method.
//!
//! Global definitions are grouped into strongly-connected components (see
//! [`utilities::scc`]) and checked in dependency order: members of a component are
//! bound to fresh monomorphic variables while their bodies are inferred (so self-
//! and mutual recursion resolve), then the component is generalized before the
//! components that depend on it (let-polymorphism).
//!
//! Structs, unions, aliases, and their generic parameters are registered up
//! front by [`Checker::register_types`]. Overloaded names (built-in arithmetic,
//! the `array_*` primitives, and any user name defined several times) are
//! resolved at each use site by trial unification against the argument and result
//! types; ambiguous uses are deferred and solved to a fixpoint at the definition
//! boundary. Definition bodies are checked against their signatures (bidirectional
//! checking); the monomorphism restriction keeps overload-constrained variables
//! from being generalized early.

pub mod data;
pub mod engine;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::lowering::ImplicitArg;
use crate::parser::data::{
    Ast, Binding, Expr, FieldDecl, FieldInit, FieldPat, Item, Pattern, Payload, Program, RecField,
    Ty,
};
use utilities::Aol;
use utilities::{diag, Code, Diagnostic, Result, Span};

use crate::typing::data::{self as ty, Type, VarId};
use crate::typing::engine::Engine;

/// A declared struct type. `params` are the implicit type parameters (the type
/// variables appearing in the fields, in order of first appearance); `fields`
/// keeps declaration order (which is also the positional-constructor order).
#[derive(Clone)]
struct StructInfo<'a> {
    params: Vec<&'a str>,
    fields: Vec<(&'a str, Aol<Ty>)>,
}

/// A declared union type: implicit `params` and one [`VariantSig`] per variant.
#[derive(Clone)]
struct UnionInfo<'a> {
    params: Vec<&'a str>,
    variants: Vec<VariantSig<'a>>,
}

/// A declared codata type: implicit `params` and one `(observation, result type)`
/// per destructor. Dual to a struct; observing runs a thunk.
#[derive(Clone)]
struct CodataInfo<'a> {
    params: Vec<&'a str>,
    observations: Vec<(&'a str, Aol<Ty>)>,
}


/// A union variant: its tag and its (normalized) payload fields, each an optional
/// name and its declared type.
#[derive(Clone)]
struct VariantSig<'a> {
    tag: &'a str,
    payload: Vec<(Option<&'a str>, Aol<Ty>)>,
}

/// A variant's payload instantiated to concrete types: one `(optional-name,
/// type)` pair per field, in declaration order.
type VariantPayload<'a> = Vec<(Option<&'a str>, Type)>;

pub struct Checker<'a> {
    ast: &'a Ast,
    eng: Engine,
    scopes: Vec<HashMap<&'a str, Type>>,
    structs: HashMap<&'a str, StructInfo<'a>>,
    unions: HashMap<&'a str, UnionInfo<'a>>,
    codata: HashMap<&'a str, CodataInfo<'a>>,
    /// `{ .obs = e }` sites the checker resolved to codata construction, and
    /// `x.obs` field-access sites resolved to a codata observation. Lowering
    /// desugars the former to a record of thunks and the latter to `field {}`.
    codata_lits: HashSet<Aol<Expr>>,
    observations: HashSet<Aol<Expr>>,
    /// Type aliases: `name -> (declared params, body)`. An applied alias is
    /// expanded by substituting its arguments for the parameters in the body.
    aliases: HashMap<&'a str, (Vec<&'a str>, Aol<Ty>)>,
    /// Each declared effect's operations, `effect -> op -> its `Arg -> Res`
    /// scheme. Used to type a handler clause head, which cannot be resolved by
    /// inference alone.
    effect_ops: HashMap<&'a str, HashMap<&'a str, Type>>,
    /// Names with more than one candidate (built-in arithmetic, any user name
    /// defined several times, and same-named functions imported from several
    /// modules). Each candidate carries its source module so a resolved use can be
    /// lowered to a qualified `MOD.name`.
    overloads: HashMap<&'a str, Vec<Cand<'a>>>,
    /// Overload uses that were ambiguous when first seen. Solved to a fixpoint at
    /// each definition boundary.
    pending: Vec<Pending<'a>>,
    /// Names this module defines itself, so a use of one is NOT rewritten to an
    /// imported module's copy.
    local_defs: HashSet<&'a str>,
    /// Single imported values, `name -> module`, so a bare use lowers to the
    /// owning module even when another loaded module defines the same name.
    value_module: HashMap<&'a str, &'a str>,
    /// Bare-call sites resolved to a specific module. Lowering rewrites the
    /// referenced `Expr::Var` to `MOD.name`.
    resolved_calls: HashMap<Aol<Expr>, &'a str>,
    /// Overloaded-call sites whose winning module defines the name more than once,
    /// so `MOD.name` alone would collide. Maps the site to the type-mangled bare
    /// name (`name#sig`) lowering must emit instead. The definition side gets the
    /// matching key in [`Self::def_keys`].
    overload_calls: HashMap<Aol<Expr>, String>,
    /// Overloaded definitions that share their name with another definition in the
    /// same module, keyed by the def's body handle. The value is the type-mangled
    /// bare name lowering assigns the global so its several overloads stay distinct.
    def_keys: HashMap<Aol<Expr>, String>,
    /// Names this module defines more than once (same-module overloads), which
    /// therefore need type-mangling to keep the globals apart.
    overloaded_multi: HashSet<&'a str>,
    /// Top-level definitions carrying `@ctx` implicit parameters, by name. A use
    /// site resolves each implicit by name against the current scope and records
    /// the result in [`Self::implicit_args`]; lowering injects them as leading
    /// arguments. Populated up front (own module) so resolution is order-independent,
    /// and extended from imports.
    global_implicits: HashMap<&'a str, GlobImpl>,
    /// This module's own `@ctx`-bearing definitions, re-exported to importers.
    own_implicits: Vec<(&'a str, GlobImpl)>,
    /// Each use site of an implicit-bearing function, mapped to the resolved
    /// implicit arguments (declaration order) lowering injects ahead of the
    /// explicit ones.
    implicit_args: HashMap<Aol<Expr>, Vec<ImplicitArg>>,
    /// Implicit-argument slots being assembled: per site, one entry per implicit,
    /// `Some` once resolved. A local binder or explicit override fills its slot
    /// immediately; a global/overloaded provider is deferred (its slot stays `None`
    /// until the requirement type is pinned). Completed sites move to
    /// [`Self::implicit_args`].
    implicit_slots: HashMap<Aol<Expr>, Vec<Option<ImplicitArg>>>,
    /// Deferred global/overload implicit resolutions, solved at each definition
    /// boundary once inference has pinned the requirement's type variables.
    implicit_pending: Vec<PendingImpl<'a>>,
    /// Explicit `@ctx` overrides keyed by the head-function reference site they
    /// apply to (`callee @ctx ...`). Consumed when that reference resolves its
    /// implicits, so given values override by-name resolution; `..` (the bool)
    /// fills the unmentioned implicits from scope.
    ctx_overrides: HashMap<Aol<Expr>, (Vec<FieldInit>, bool)>,
    /// Types with unresolved `with Other` splices, `name -> (is_struct, includes)`.
    /// Drained as each type's members are copied in (see `splice_includes`). This
    /// is a declaration-time convenience only; no type relationship is recorded.
    pending_includes: HashMap<&'a str, (bool, Vec<&'a str>)>,
    /// Type variables introduced by integer literals, which may be Int or Real;
    /// leftovers default to Int at the definition boundary.
    numeric: Vec<Type>,
    /// This module's own exports, recorded after checking.
    own_values: Vec<(&'a str, Type)>,
    own_overloads: Vec<(&'a str, Vec<Type>)>,
    own_type_names: Vec<&'a str>,
    /// Value schemes pulled in from imports (with their source module), finalized
    /// once.
    imported: HashMap<&'a str, Vec<Cand<'a>>>,
    /// Imported names reachable qualified as `MOD.name`.
    qualified: HashMap<&'a str, HashMap<&'a str, Vec<Type>>>,
    module_name: &'a str,
    /// `[..]` literal/pattern nodes the checker resolved to `Array` (a byte
    /// vector) rather than the default `List`. Lowering reads this to emit array
    /// construction / destructuring instead of `Cons`/`Nil`.
    array_exprs: HashSet<Aol<Expr>>,
    /// `[..]` literal sites resolved to a sized tensor `[n]T` (a vector value),
    /// distinct from the byte-`Array` sites in `array_exprs`. Lowering builds a
    /// vector; the index `t.[i]` reads it modulo the length.
    tensor_exprs: HashSet<Aol<Expr>>,
    /// `t.[i]` index sites, so lowering emits the modular vector read.
    index_exprs: HashSet<Aol<Expr>>,
    /// Argument sites promoted to a record: a bare scalar `1` or a positional
    /// `{1, 2}` passed where a record is expected, mapped to the target record's
    /// field names (in order). Lowering wraps the value into a name-keyed record.
    promotions: HashMap<Aol<Expr>, Vec<String>>,
    /// `.{ ... }` struct-literal sites, mapped to the nominal struct name the
    /// checker resolved them to (from a type annotation / expected type, or the
    /// field set). Lowering reads this so a positional literal gets the right field
    /// names instead of falling back to positional indices.
    struct_lit_names: HashMap<Aol<Expr>, String>,
    array_pats: HashSet<Aol<Pattern>>,
    /// The ordered field names each `with subject in body` brings into scope,
    /// keyed by the `With` node. Lowering desugars `with` into a `let` per field,
    /// so the Core has no name-binding-by-type node and stays De-Bruijn indexable.
    with_fields: HashMap<Aol<Expr>, Vec<String>>,
    /// Each `@extern` node's type variable, zonked after solving to recover the
    /// concrete arrow the declaration constrained it to. Lowering reads the
    /// flattened arg/result marshalling names off this.
    extern_tys: HashMap<Aol<Expr>, Type>,
    /// The ambient effect row: the effects the expression currently being
    /// inferred is allowed to perform. A call subsumes its callee's latent effect
    /// into this; a lambda body and a handler body run under a fresh/extended
    /// ambient; a top-level body runs under the empty closed row, so an unhandled
    /// effect fails to unify.
    ambient: Type,
    /// The first unknown type name met while converting a signature (a bare
    /// `Con` that is neither a base type nor a declared struct/union/alias). A
    /// type variable is a lowercase name, so an unknown capitalized name is a
    /// typo, surfaced at the end of the check.
    unknown_type: Option<Diagnostic>,
}

/// One candidate of an overloaded name: its type and, for an imported one, the
/// module that owns it (`None` for a built-in or a definition in this module).
#[derive(Clone)]
struct Cand<'a> {
    ty: Type,
    module: Option<&'a str>,
}

impl<'a> Cand<'a> {
    /// A built-in or effect-operation candidate, owned by no module (never
    /// rewritten to a qualified reference).
    fn local(ty: Type) -> Cand<'a> {
        Cand { ty, module: None }
    }
    fn from(ty: Type, module: Option<&'a str>) -> Cand<'a> {
        Cand { ty, module }
    }
}

/// A `@ctx`-bearing definition's metadata: its (arrow) signature and the implicit
/// parameter declarations. Both are AST handles into the shared `Ast`, so they
/// stay valid across modules. Instantiating the signature and the requirement
/// types with one shared type-variable map keeps their variables aligned.
#[derive(Clone)]
struct GlobImpl {
    sig: Aol<Ty>,
    decls: Vec<FieldDecl>,
}

/// A deferred implicit resolution: one requirement of an implicit-bearing function
/// whose provider is a global (so it needs the requirement type pinned first). The
/// `site` and `idx` locate the slot in [`Checker::implicit_slots`] to fill.
struct PendingImpl<'a> {
    site: Aol<Expr>,
    idx: usize,
    fname: String,
    implname: &'a str,
    reqty: Type,
}

/// A deferred overload use: its candidate set, the argument types, the fresh
/// result variable standing in for the (not-yet-known) result, and the call site
/// to annotate once it resolves.
struct Pending<'a> {
    name: String,
    candidates: Vec<Cand<'a>>,
    args: Vec<Type>,
    result: Type,
    site: Option<Aol<Expr>>,
}

/// The outcome of trying a candidate set against argument types.
enum Match {
    Unique(usize),
    None,
    Ambiguous,
}

impl<'a> Checker<'a> {
    pub fn new(ast: &'a Ast) -> Checker<'a> {
        let mut c = Checker {
            ast,
            eng: Engine::new(),
            scopes: vec![HashMap::new()],
            structs: HashMap::new(),
            unions: HashMap::new(),
            codata: HashMap::new(),
            codata_lits: HashSet::new(),
            observations: HashSet::new(),
            aliases: HashMap::new(),
            effect_ops: HashMap::new(),
            overloads: HashMap::new(),
            pending: Vec::new(),
            local_defs: HashSet::new(),
            value_module: HashMap::new(),
            resolved_calls: HashMap::new(),
            overload_calls: HashMap::new(),
            def_keys: HashMap::new(),
            overloaded_multi: HashSet::new(),
            global_implicits: HashMap::new(),
            own_implicits: Vec::new(),
            implicit_args: HashMap::new(),
            implicit_slots: HashMap::new(),
            implicit_pending: Vec::new(),
            ctx_overrides: HashMap::new(),
            pending_includes: HashMap::new(),
            numeric: Vec::new(),
            own_values: Vec::new(),
            own_overloads: Vec::new(),
            own_type_names: Vec::new(),
            imported: HashMap::new(),
            qualified: HashMap::new(),
            module_name: "",
            array_exprs: HashSet::new(),
            tensor_exprs: HashSet::new(),
            index_exprs: HashSet::new(),
            promotions: HashMap::new(),
            struct_lit_names: HashMap::new(),
            array_pats: HashSet::new(),
            with_fields: HashMap::new(),
            extern_tys: HashMap::new(),
            ambient: Type::RowEmpty,
            unknown_type: None,
        };
        c.install_builtins();
        c
    }

    /// The `[..]` expression and pattern nodes this checker resolved to `Array`.
    /// Lowering consults these to choose byte-vector construction/matching over
    /// the default `List`.
    pub fn array_nodes(&self) -> (&HashSet<Aol<Expr>>, &HashSet<Aol<Pattern>>) {
        (&self.array_exprs, &self.array_pats)
    }

    /// The `[..]` literal sites resolved to a sized tensor, and the `t.[i]` index
    /// sites. Lowering builds a vector for the former and a modular read for the latter.
    pub fn tensor_nodes(&self) -> (&HashSet<Aol<Expr>>, &HashSet<Aol<Expr>>) {
        (&self.tensor_exprs, &self.index_exprs)
    }

    /// Argument sites promoted to a record, mapped to the target field names;
    /// lowering wraps the value into a name-keyed record.
    pub fn promotions(&self) -> &HashMap<Aol<Expr>, Vec<String>> {
        &self.promotions
    }

    /// `.{ ... }` struct-literal sites mapped to the resolved struct name, so
    /// lowering emits the correct field names (esp. for positional literals).
    pub fn struct_lit_names(&self) -> &HashMap<Aol<Expr>, String> {
        &self.struct_lit_names
    }

    /// Codata sites: `{ .obs = e }` construction literals (lowered to a record of
    /// thunks) and `x.obs` observations (lowered to `field {}`).
    pub fn codata_sites(&self) -> (&HashSet<Aol<Expr>>, &HashSet<Aol<Expr>>) {
        (&self.codata_lits, &self.observations)
    }

    /// Bare-call `Expr::Var` sites this checker resolved to a specific module.
    /// Lowering rewrites each to a qualified `MOD.name` so the interpreter reaches
    /// the intended function rather than a same-named one from another module.
    pub fn call_modules(&self) -> &HashMap<Aol<Expr>, &'a str> {
        &self.resolved_calls
    }

    /// Overloaded-call sites whose target needs a type-mangled name (its module
    /// defines the name several times). Lowering emits the mapped bare name in
    /// place of the source name; the qualifying module comes from `call_modules`.
    pub fn overload_calls(&self) -> &HashMap<Aol<Expr>, String> {
        &self.overload_calls
    }

    /// Same-module overloaded definitions, keyed by body handle, with the mangled
    /// bare name lowering must give each global so the overloads stay distinct.
    pub fn def_keys(&self) -> &HashMap<Aol<Expr>, String> {
        &self.def_keys
    }

    /// Each use site of a `@ctx`-bearing function, mapped to the ordered implicit
    /// arguments lowering injects as leading arguments.
    pub fn implicit_calls(&self) -> &HashMap<Aol<Expr>, Vec<ImplicitArg>> {
        &self.implicit_args
    }

    /// The ordered field names each `with` expression binds, keyed by the `With`
    /// node. Lowering desugars `with` into a `let` per field using these.
    pub fn with_fields(&self) -> &HashMap<Aol<Expr>, Vec<String>> {
        &self.with_fields
    }

    /// Each `@extern` site's marshalling signature: the flattened argument type
    /// names and the result type name, recovered by zonking the site's inferred
    /// type (the declared arrow constrained it). Lowering builds `Term::Extern`
    /// from these.
    pub fn extern_sigs(&self) -> HashMap<Aol<Expr>, (Vec<String>, String)> {
        self.extern_tys
            .iter()
            .map(|(&site, ty)| (site, flatten_extern(&self.eng.zonk(ty))))
            .collect()
    }

    // -- AST accessors (resolve to `'a`-lived data, independent of `&self`) --

    fn node(&self, e: Aol<Expr>) -> &'a Expr {
        self.ast.expr(e)
    }
    /// The number of parameters in a signature's arrow spine.
    fn arrow_arity_ty(&self, mut ty: Aol<Ty>) -> usize {
        let mut n = 0;
        while let Ty::Arrow { to, .. } = self.tnode(ty) {
            n += 1;
            ty = *to;
        }
        n
    }
    /// The number of leading lambda parameters a body binds explicitly.
    fn leading_lam_params(&self, mut e: Aol<Expr>) -> usize {
        let mut n = 0;
        while let Expr::Lambda { params, body } = self.node(e) {
            n += params.len();
            e = *body;
        }
        n
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

    /// Check a whole program, returning the inferred (generalized) type of every
    /// global definition, in source order.
    pub fn check_program(&mut self, program: &Program) -> Result<Vec<(&'a str, Type)>> {
        self.module_name = self.text(program.module);
        self.finalize_imports();
        self.register_types(program)?;
        // Row registration elaborates struct field types only to cache their rows
        // for the bridge; it must not report type errors (a field may reference a
        // type not imported into this module, e.g. a re-exported struct's internals).
        // Real unknown-type errors are still caught when definitions are elaborated.
        let saved_unknown = self.unknown_type.take();
        self.register_struct_rows();
        self.unknown_type = saved_unknown;
        self.register_effects(program);

        let defs: Vec<Def<'a>> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Def {
                    name,
                    sig,
                    implicits,
                    body,
                } => Some(Def {
                    name: self.text(*name),
                    sig: *sig,
                    implicits: implicits.to_vec(),
                    body: *body,
                }),
                _ => None,
            })
            .collect();

        self.local_defs = defs.iter().map(|d| d.name).collect();

        // A name is overloaded if defined more than once here, or if it adds to an
        // overload already imported.
        let mut counts: HashMap<&'a str, usize> = HashMap::new();
        for d in &defs {
            *counts.entry(d.name).or_insert(0) += 1;
        }
        let mut overloaded_names: HashSet<&'a str> = HashSet::new();
        for d in &defs {
            if counts[d.name] > 1 || self.overloads.contains_key(d.name) {
                overloaded_names.insert(d.name);
            }
        }
        let is_overloaded = |name: &str| overloaded_names.contains(name);

        // Names defined several times in THIS module need type-mangled globals so
        // the overloads do not collide under a single `MOD.name` key.
        self.overloaded_multi = defs
            .iter()
            .filter(|d| counts[d.name] > 1)
            .map(|d| d.name)
            .collect();

        // Register `@ctx`-bearing definitions up front so a use anywhere in the
        // module resolves them regardless of source order. A `@ctx` requires a
        // signature (the parser only accepts it after one) and is not allowed on an
        // overloaded name in this version.
        for d in &defs {
            if d.implicits.is_empty() {
                continue;
            }
            if is_overloaded(d.name) {
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{}` cannot be both overloaded and carry `@ctx` implicit parameters",
                    d.name
                ));
            }
            let Some(sig) = d.sig else {
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{}` needs a type signature to declare `@ctx` implicit parameters",
                    d.name
                ));
            };
            let gi = GlobImpl {
                sig,
                decls: d.implicits.clone(),
            };
            self.own_implicits.push((d.name, gi.clone()));
            self.global_implicits.insert(d.name, gi);
        }

        // Seed each overloaded name's candidates from its declared signatures, and
        // record the mangled global name for a same-module overload (so its several
        // definitions stay distinct at runtime).
        for d in &defs {
            if is_overloaded(d.name) {
                if let Some(sig) = d.sig {
                    let scheme = self.scheme_of_sig(sig);
                    let module = self.module_name;
                    if counts[d.name] > 1 {
                        self.def_keys.insert(d.body, overload_key(d.name, &scheme));
                    }
                    self.overloads
                        .entry(d.name)
                        .or_default()
                        .push(Cand::from(scheme, Some(module)));
                }
            }
        }

        let singles: Vec<Def<'a>> = defs
            .iter()
            .filter(|d| !is_overloaded(d.name))
            .cloned()
            .collect();
        let single_index: HashMap<&'a str, usize> = singles
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name, i))
            .collect();
        let graph = dependency_graph(self.ast, &singles, &single_index);

        let mut types: HashMap<&'a str, Type> = HashMap::new();
        for component in utilities::scc::scc(&graph) {
            self.check_component(&component, &singles, &mut types)?;
        }

        let mut overloaded_out = Vec::new();
        for d in &defs {
            if is_overloaded(d.name) {
                let ty = self.check_overloaded_def(d)?;
                overloaded_out.push((d.name, ty));
            }
        }

        let mut out: Vec<(&'a str, Type)> = defs
            .iter()
            .filter(|d| !is_overloaded(d.name))
            .map(|d| (d.name, types[d.name].clone()))
            .collect();

        self.own_values = out.clone();
        let mut own_ov: HashMap<&'a str, Vec<Type>> = HashMap::new();
        for (name, ty) in &overloaded_out {
            own_ov.entry(name).or_default().push(ty.clone());
        }
        self.own_overloads = own_ov.into_iter().collect();

        out.extend(overloaded_out);
        if let Some(d) = self.unknown_type.take() {
            return Err(d);
        }
        Ok(out)
    }

    /// Import another module's public exports into this checker.
    /// Import another module's exports as QUALIFIED-only (`Module.name`), without
    /// adding them to the unqualified namespace. Used for the auto-injected `C`
    /// namespace, which is reachable as `C.foo` everywhere but must not pollute
    /// bare names (a program's own `sqrt` is not libm's).
    pub fn import_qualified(&mut self, other: &Checker<'a>) {
        let module = other.module_name;
        for (name, cands) in &other.own_overloads {
            let qualified: Vec<Type> = cands.iter().map(|c| self.import_scheme(c)).collect();
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, qualified);
        }
        for (name, scheme) in &other.own_values {
            let qualified = self.import_scheme(scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, vec![qualified]);
        }
    }

    pub fn import_from(&mut self, other: &Checker<'a>) {
        // Bring the exporter's `@ctx`-bearing functions in, so a bare use of an
        // imported one resolves its implicits (the signature/decl handles live in
        // the shared `Ast`). A qualified use (`MOD.f`) does not yet inject them.
        for (name, gi) in &other.own_implicits {
            self.global_implicits.insert(name, gi.clone());
        }
        for &name in &other.own_type_names {
            if let Some(s) = other.structs.get(name) {
                self.structs.insert(name, s.clone());
            }
            if let Some(u) = other.unions.get(name) {
                self.unions.insert(name, u.clone());
            }
            if let Some(a) = other.aliases.get(name) {
                self.aliases.insert(name, a.clone());
            }
            if let Some(c) = other.codata.get(name) {
                self.codata.insert(name, c.clone());
            }
        }
        let module = other.module_name;
        for (name, cands) in &other.own_overloads {
            let mut qualified = Vec::with_capacity(cands.len());
            for c in cands {
                let unqualified = self.import_scheme(c);
                self.imported.entry(name).or_default().push(Cand {
                    ty: unqualified,
                    module: Some(module),
                });
                qualified.push(self.import_scheme(c));
            }
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, qualified);
        }
        for (name, scheme) in &other.own_values {
            let unqualified = self.import_scheme(scheme);
            self.imported.entry(name).or_default().push(Cand {
                ty: unqualified,
                module: Some(module),
            });
            let qualified = self.import_scheme(scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, vec![qualified]);
        }
    }

    fn finalize_imports(&mut self) {
        let imported = std::mem::take(&mut self.imported);
        for (name, mut cands) in imported {
            if let Some(existing) = self.overloads.get_mut(name) {
                existing.extend(cands);
            } else if cands.len() == 1 {
                let cand = cands.pop().expect("one candidate");
                if let Some(module) = cand.module {
                    self.value_module.insert(name, module);
                }
                self.bind(name, cand.ty);
            } else {
                self.overloads.insert(name, cands);
            }
        }
    }

    fn import_scheme(&mut self, ty: &Type) -> Type {
        let mut map = HashMap::new();
        self.import_ty(ty, &mut map)
    }

    fn import_ty(&mut self, ty: &Type, map: &mut HashMap<VarId, Type>) -> Type {
        match ty {
            Type::Var(id) => map
                .entry(*id)
                .or_insert_with(|| self.eng.fresh_generic())
                .clone(),
            Type::Con(name) => Type::Con(name.clone()),
            Type::Nat(n) => Type::Nat(*n),
            Type::NatAdd(a, b) => {
                Type::NatAdd(Box::new(self.import_ty(a, map)), Box::new(self.import_ty(b, map)))
            }
            Type::NatMul(a, b) => {
                Type::NatMul(Box::new(self.import_ty(a, map)), Box::new(self.import_ty(b, map)))
            }
            Type::App(head, arg) => Type::app(self.import_ty(head, map), self.import_ty(arg, map)),
            Type::Arrow(from, to, eff) => Type::arrow_eff(
                self.import_ty(from, map),
                self.import_ty(to, map),
                self.import_ty(eff, map),
            ),
            Type::RowEmpty => Type::RowEmpty,
            Type::RowExtend(label, rest) => {
                Type::RowExtend(label.clone(), Box::new(self.import_ty(rest, map)))
            }
            Type::Tuple(items) => {
                Type::Tuple(items.iter().map(|t| self.import_ty(t, map)).collect())
            }
            Type::Record(row) => Type::record(self.import_ty(row, map)),
            Type::RowField(label, ty, rest) => Type::RowField(
                label.clone(),
                Box::new(self.import_ty(ty, map)),
                Box::new(self.import_ty(rest, map)),
            ),
        }
    }

    fn check_overloaded_def(&mut self, def: &Def<'a>) -> Result<Type> {
        self.ambient = Type::RowEmpty; // a top-level body is pure
        self.eng.enter_level();
        let result = if def.sig.is_some() {
            let fresh = self.eng.fresh();
            self.check_def_body(def, &fresh)?;
            fresh
        } else {
            let inferred = self.infer(def.body)?;
            let module = self.module_name;
            self.overloads
                .entry(def.name)
                .or_default()
                .push(Cand::from(inferred.clone(), Some(module)));
            inferred
        };
        self.solve_pending()?;
        self.resolve_pending_implicits()?;
        self.eng.leave_level();
        let mono = self.pending_vars();
        self.eng.generalize_except(&result, &mono);
        let zonked = self.eng.zonk(&result);
        // A same-module overload defined without a signature is seeded from its
        // inferred type; the sig'd case is keyed at seeding time.
        if def.sig.is_none() && self.overloaded_multi.contains(def.name) {
            self.def_keys
                .insert(def.body, overload_key(def.name, &zonked));
        }
        Ok(zonked)
    }

    fn scheme_of_sig(&mut self, sig: Aol<Ty>) -> Type {
        self.eng.enter_level();
        let mut tvars = HashMap::new();
        let ty = self.ty_of_ast(sig, &mut tvars);
        self.eng.leave_level();
        self.eng.generalize(&ty);
        self.eng.zonk(&ty)
    }

    fn check_component(
        &mut self,
        component: &[usize],
        defs: &[Def<'a>],
        types: &mut HashMap<&'a str, Type>,
    ) -> Result<()> {
        self.eng.enter_level();
        let mut declared = Vec::with_capacity(component.len());
        for &i in component {
            let v = self.eng.fresh();
            self.bind(defs[i].name, v.clone());
            declared.push(v);
        }
        for (&i, decl) in component.iter().zip(&declared) {
            self.check_def_body(&defs[i], decl)?;
        }
        self.solve_pending()?;
        self.resolve_pending_implicits()?;
        self.eng.leave_level();
        let mono = self.pending_vars();
        for (&i, decl) in component.iter().zip(&declared) {
            self.eng.generalize_except(decl, &mono);
            types.insert(defs[i].name, self.eng.zonk(decl));
        }
        Ok(())
    }

    fn check_def_body(&mut self, def: &Def<'a>, decl: &Type) -> Result<()> {
        self.ambient = Type::RowEmpty; // a top-level body is pure
        if let Some(sig) = def.sig {
            let mut tvars = HashMap::new();
            let sig_ty = self.ty_of_ast(sig, &mut tvars);
            self.eng.unify(
                decl,
                &sig_ty,
                &format!("against the signature of `{}`", def.name),
            )?;
            if def.implicits.is_empty() {
                return self.check_body_against_sig(def.body, sig, &sig_ty);
            }
            // Bind each `@ctx` implicit as a local while checking the body, sharing
            // `tvars` with the signature so their type variables line up (a `List a`
            // signature and a `compare : a -> a -> Ordering` implicit share `a`).
            self.enter_scope();
            for d in &def.implicits {
                let name = self.text(d.name);
                let ty = self.ty_of_ast(d.ty, &mut tvars);
                self.bind(name, ty);
            }
            let r = self.check_body_against_sig(def.body, sig, &sig_ty);
            self.leave_scope();
            r
        } else {
            let inferred = self.infer(def.body)?;
            self.eng.unify(
                decl,
                &inferred,
                &format!("in the definition of `{}`", def.name),
            )
        }
    }

    /// Check a body against its signature, implicitly consuming leading record
    /// parameters (`{x: Int, y: Int}` binds its fields directly; a `with` field
    /// also scopes its struct's fields).
    fn check_body_against_sig(
        &mut self,
        body: Aol<Expr>,
        sig: Aol<Ty>,
        sig_ty: &Type,
    ) -> Result<()> {
        // The body's leading lambdas bind the last k of the m signature parameters,
        // so the record-parameter sugar only applies when the lambdas do not reach
        // this one (`k < m`). `\p = p.x` then names a record parameter itself.
        if self.leading_lam_params(body) >= self.arrow_arity_ty(sig) {
            return self.check(body, sig_ty);
        }
        let fields: &[RecField] = match self.tnode(sig) {
            Ty::Arrow { from, .. } => match self.tnode(*from) {
                // A CLOSED record parameter is the destructuring sugar; an open
                // `{ x | r }` is a real row-polymorphic record value, bound as-is.
                Ty::Record { fields, tail: None } => fields,
                // A unit parameter takes no fields: the sugar just introduces the
                // (unused) thunk parameter, so `f : {} -> T = <body>` needs no `\u =`.
                Ty::Unit => &[],
                _ => return self.check(body, sig_ty),
            },
            _ => return self.check(body, sig_ty),
        };
        let to = match self.tnode(sig) {
            Ty::Arrow { to, .. } => *to,
            _ => unreachable!("guarded on an arrow above"),
        };
        let (param_ty, result_ty, eff) = self.arrow_parts(sig_ty)?;
        self.enter_scope();
        self.bind_record_param(fields, &param_ty)?;
        // The body may perform this arrow's latent effect (the declared row).
        let saved = std::mem::replace(&mut self.ambient, eff);
        let out = self.check_body_against_sig(body, to, &result_ty);
        self.ambient = saved;
        self.leave_scope();
        out
    }

    fn bind_record_param(&mut self, fields: &'a [RecField], param_ty: &Type) -> Result<()> {
        // A unit parameter (the thunk sugar) binds nothing, so it needs no row.
        if fields.is_empty() {
            return Ok(());
        }
        // The record parameter auto-binds each field name (the "define with an
        // implicit destructuring" sugar). The parameter is a real record type, so
        // look each field up by name in its row.
        let recfields = self.record_fields_of(param_ty)?;
        for f in fields {
            let name = self.text(f.name);
            let t = recfields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| self.eng.fresh());
            self.bind(name, t.clone());
            if f.with {
                self.scope_struct_fields(&t)?;
            }
        }
        Ok(())
    }

    /// Bring the struct's fields into scope, returning their names in declaration
    /// order (empty if `ty` is not a known struct). Lowering keys the `with`
    /// desugaring off these names.
    fn scope_struct_fields(&mut self, ty: &Type) -> Result<Vec<String>> {
        let (head, args) = self.spine(ty);
        let mut names = Vec::new();
        if let Type::Con(name) = &head {
            if let Some(info) = self.structs.get(name.as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                for (fname, fty) in &info.fields {
                    let field_ty = self.ty_of_ast(*fty, &mut subst);
                    self.bind(fname, field_ty);
                    names.push(fname.to_string());
                }
            }
        }
        Ok(names)
    }

    /// Check an expression against an expected type (the checking direction).
    fn check(&mut self, e: Aol<Expr>, expected: &Type) -> Result<()> {
        match self.node(e) {
            Expr::Lambda { params, body } => {
                self.enter_scope();
                let mut exp = expected.clone();
                let mut body_eff = self.ambient.clone();
                for p in params.iter() {
                    let (param_ty, rest, eff) = self.arrow_parts(&exp)?;
                    self.type_pattern(*p, &param_ty)?;
                    exp = rest;
                    body_eff = eff; // the innermost arrow's effect: the body's ambient
                }
                let saved = std::mem::replace(&mut self.ambient, body_eff);
                let out = self.check(*body, &exp);
                self.ambient = saved;
                self.leave_scope();
                out
            }
            Expr::List(items) if self.is_array(expected) => {
                self.array_exprs.insert(e);
                for item in items.iter() {
                    let t = self.infer(*item)?;
                    self.eng
                        .unify(&t, &Type::con(ty::INT), "in an array element")?;
                }
                Ok(())
            }
            // `[..]` where a sized tensor `[n]T` is expected: the literal's length
            // fixes `n`, and every element is checked against `T`. Lowering builds
            // a vector.
            Expr::List(items) if self.tensor_parts(expected).is_some() => {
                let (size, elem) = self.tensor_parts(expected).expect("guarded");
                self.eng.unify(
                    &size,
                    &Type::Nat(items.len() as u64),
                    "in a tensor literal (its length fixes the size)",
                )?;
                for item in items.iter() {
                    self.check(*item, &elem)?;
                }
                self.tensor_exprs.insert(e);
                Ok(())
            }
            // `{ .obs = e, ... }` where a codata type is expected: construct it (each
            // clause becomes a thunk). Every observation must be given.
            Expr::Record {
                fields,
                with: None,
                update: None,
            } if self.codata_head(expected).is_some() => {
                self.check_codata_lit(e, fields, expected)
            }
            // A bare `.{ .. }` literal takes its struct from the expected type (the
            // checking direction). This is essential for a POSITIONAL literal, which
            // carries no field names to infer from.
            Expr::StructLit {
                ty: None,
                fields,
                spread,
            } => match self.struct_name_of(expected) {
                Some(name) => {
                    let got = self.infer_struct_lit(e, Some(name), fields, *spread)?;
                    self.eng.unify(&got, expected, "against the expected type")
                }
                None => {
                    let got = self.infer_struct_lit(e, None, fields, *spread)?;
                    self.eng.unify(&got, expected, "against the expected type")
                }
            },
            _ => {
                let got = self.infer(e)?;
                // Promotion at an argument position: a bare scalar or a positional
                // tuple passed where a record is expected is wrapped into that record
                // (`foo 1` -> `foo { .x = 1 }`, `foo {1,2}` -> `foo { .x=1, .y=2 }`).
                if let Type::Record(_) = self.eng.resolve(expected) {
                    // Try a direct unification first (a record value, or a nominal
                    // struct via the `Con ~ Record` bridge). Skip it for a numeric
                    // literal, whose undefaulted variable would wrongly unify with
                    // the record. If unification fails, promote a scalar / tuple /
                    // struct into a CLOSED record (an open row has no known fields to
                    // promote into, so a mismatch there is a real error).
                    if !self.is_numeric(&got) {
                        let save = self.eng.save();
                        if self.eng.unify(&got, expected, "against the expected type").is_ok() {
                            return Ok(());
                        }
                        self.eng.restore(save);
                    }
                    if self.record_is_closed(expected) {
                        let g = self.eng.resolve(&got);
                        let values: Vec<Type> = match g {
                            Type::Tuple(items) => items,
                            _ => vec![got.clone()],
                        };
                        return self.promote_to_record(e, &values, expected);
                    }
                }
                self.eng.unify(&got, expected, "against the expected type")
            }
        }
    }

    /// Promote a scalar or a positional tuple to the expected record type, unifying
    /// each value with the field in declaration order and recording the site so
    /// lowering wraps the value into a name-keyed record.
    fn promote_to_record(
        &mut self,
        site: Aol<Expr>,
        values: &[Type],
        record_ty: &Type,
    ) -> Result<()> {
        let fields = self.record_fields_of(record_ty)?;
        if fields.len() != values.len() {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "cannot pass {} value(s) as a record with {} field(s)",
                values.len(), fields.len()
            ));
        }
        for (val, (_, fty)) in values.iter().zip(&fields) {
            self.eng.unify(val, fty, "promoting an argument to a record")?;
        }
        self.promotions
            .insert(site, fields.into_iter().map(|(n, _)| n).collect());
        Ok(())
    }

    fn is_array(&self, ty: &Type) -> bool {
        matches!(self.eng.resolve(ty), Type::Con(name) if name == ty::ARRAY)
    }

    /// Decompose a function type into (parameter, result, latent effect). If it is
    /// not yet known to be an arrow, force it to one with fresh parts.
    fn arrow_parts(&mut self, ty: &Type) -> Result<(Type, Type, Type)> {
        match self.eng.resolve(ty) {
            Type::Arrow(from, to, eff) => Ok((*from, *to, *eff)),
            other => {
                let from = self.eng.fresh();
                let to = self.eng.fresh();
                let eff = self.eng.fresh();
                self.eng.unify(
                    &other,
                    &Type::arrow_eff(from.clone(), to.clone(), eff.clone()),
                    "expected a function",
                )?;
                Ok((from, to, eff))
            }
        }
    }

    fn pending_vars(&self) -> HashSet<VarId> {
        let mut out = HashSet::new();
        for p in &self.pending {
            for a in &p.args {
                self.eng.collect_vars(a, &mut out);
            }
            self.eng.collect_vars(&p.result, &mut out);
        }
        for t in &self.numeric {
            self.eng.collect_vars(t, &mut out);
        }
        out
    }

    // -- type declarations --------------------------------------------------

    /// A type's parameter list is exactly what it declares after the keyword. Every
    /// type variable used in the body must be declared (an undeclared one is an
    /// error: parameters are mandatory, never inferred). A declared parameter that
    /// appears nowhere is allowed (a phantom). `kind` is the keyword, for the error.
    fn resolve_type_params(
        &self,
        kind: &str,
        name: &'a str,
        declared: &[utilities::StrId],
        collected: Vec<&'a str>,
    ) -> Result<Vec<&'a str>> {
        let declared: Vec<&'a str> = declared.iter().map(|p| self.text(*p)).collect();
        for v in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, false));
            }
        }
        Ok(declared)
    }

    /// Re-check a struct/union's parameters after its `with` splices are copied in:
    /// the parameters stay as declared, but every type variable in the now-complete
    /// field/variant set, including spliced-in ones, must still be covered.
    fn splice_params(&self, kind: &str, name: &'a str, collected: Vec<&'a str>) -> Result<Vec<&'a str>> {
        let declared = self
            .structs
            .get(name)
            .map(|i| i.params.clone())
            .or_else(|| self.unions.get(name).map(|i| i.params.clone()))
            .expect("registered");
        for v in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, true));
            }
        }
        Ok(declared)
    }

    fn register_types(&mut self, program: &Program) -> Result<()> {
        for item in program.items.iter() {
            match item {
                Item::Struct {
                    name,
                    params,
                    includes,
                    fields,
                } => {
                    let mut collected = Vec::new();
                    for f in fields.iter() {
                        collect_tyvars(self.ast, f.ty, &mut collected);
                    }
                    let name = self.text(*name);
                    let params = self.resolve_type_params("struct", name, params, collected)?;
                    let fields = fields.iter().map(|f| (self.text(f.name), f.ty)).collect();
                    self.structs.insert(name, StructInfo { params, fields });
                    self.own_type_names.push(name);
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| self.text(*p)).collect();
                        self.pending_includes.insert(name, (true, ps));
                    }
                }
                Item::Union {
                    name,
                    params,
                    includes,
                    variants,
                } => {
                    let mut collected = Vec::new();
                    let mut vs = Vec::with_capacity(variants.len());
                    for v in variants.iter() {
                        let payload = payload_fields(self.ast, &v.payload);
                        for (_, ty) in &payload {
                            collect_tyvars(self.ast, *ty, &mut collected);
                        }
                        vs.push(VariantSig {
                            tag: self.text(v.tag),
                            payload,
                        });
                    }
                    let name = self.text(*name);
                    let params = self.resolve_type_params("union", name, params, collected)?;
                    self.unions.insert(
                        name,
                        UnionInfo {
                            params,
                            variants: vs,
                        },
                    );
                    self.own_type_names.push(name);
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| self.text(*p)).collect();
                        self.pending_includes.insert(name, (false, ps));
                    }
                }
                Item::Alias { name, params, ty } => {
                    let mut collected = Vec::new();
                    collect_tyvars(self.ast, *ty, &mut collected);
                    let name = self.text(*name);
                    let params = self.resolve_type_params("alias", name, params, collected)?;
                    self.aliases.insert(name, (params, *ty));
                    self.own_type_names.push(name);
                }
                Item::Codata {
                    name,
                    params,
                    observations,
                } => {
                    let mut collected = Vec::new();
                    let obs: Vec<(&'a str, Aol<Ty>)> = observations
                        .iter()
                        .map(|o| {
                            collect_tyvars(self.ast, o.ty, &mut collected);
                            (self.text(o.name), o.ty)
                        })
                        .collect();
                    let name = self.text(*name);
                    let params = self.resolve_type_params("codata", name, params, collected)?;
                    self.codata.insert(
                        name,
                        CodataInfo {
                            params,
                            observations: obs,
                        },
                    );
                    self.own_type_names.push(name);
                }
                _ => {}
            }
        }
        // Copy in each `with Other` type's members once every type is registered,
        // so an included type may be declared after (or imported by) the one that
        // names it.
        let pending: Vec<&'a str> = self.pending_includes.keys().copied().collect();
        for name in pending {
            let mut visiting = HashSet::new();
            self.splice_includes(name, &mut visiting)?;
        }
        Ok(())
    }

    /// Copy each included type's fields (struct) or variants (union) into `name`,
    /// ahead of its own, resolving includes recursively (an included type may
    /// itself splice). Detects cycles, kind mismatches, and duplicate members.
    /// This copies members; it records no subtype/relationship in the type system.
    fn splice_includes(&mut self, name: &'a str, visiting: &mut HashSet<&'a str>) -> Result<()> {
        let (is_struct, includes) = match self.pending_includes.get(name) {
            Some(entry) => entry.clone(),
            None => return Ok(()), // already spliced (or never used `with`)
        };
        if !visiting.insert(name) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "type `{name}` includes itself (a `with` cycle)"
            ));
        }
        if is_struct {
            let mut fields: Vec<(&'a str, Aol<Ty>)> = Vec::new();
            for p in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.structs.get(p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, Span::at(0), 0,
                        "`{name}` does `with {p}`, which is not a known struct")
                })?;
                for f in &pinfo.fields {
                    if fields.iter().any(|(n, _)| n == &f.0) {
                        return Err(dup_member(name, f.0, "field"));
                    }
                    fields.push(*f);
                }
            }
            let own = self.structs.get(name).expect("registered").fields.clone();
            for f in own {
                if fields.iter().any(|(n, _)| n == &f.0) {
                    return Err(dup_member(name, f.0, "field"));
                }
                fields.push(f);
            }
            let mut collected = Vec::new();
            for (_, ty) in &fields {
                collect_tyvars(self.ast, *ty, &mut collected);
            }
            let params = self.splice_params("struct", name, collected)?;
            self.structs.insert(name, StructInfo { params, fields });
        } else {
            let mut variants: Vec<VariantSig<'a>> = Vec::new();
            for p in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.unions.get(p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, Span::at(0), 0,
                        "`{name}` does `with {p}`, which is not a known union")
                })?;
                for v in &pinfo.variants {
                    if variants.iter().any(|w| w.tag == v.tag) {
                        return Err(dup_member(name, v.tag, "variant"));
                    }
                    variants.push(v.clone());
                }
            }
            let own = self.unions.get(name).expect("registered").variants.clone();
            for v in own {
                if variants.iter().any(|w| w.tag == v.tag) {
                    return Err(dup_member(name, v.tag, "variant"));
                }
                variants.push(v);
            }
            let mut collected = Vec::new();
            for v in &variants {
                for (_, ty) in &v.payload {
                    collect_tyvars(self.ast, *ty, &mut collected);
                }
            }
            let params = self.splice_params("union", name, collected)?;
            self.unions.insert(name, UnionInfo { params, variants });
        }
        self.pending_includes.remove(name);
        visiting.remove(name);
        Ok(())
    }

    /// Register every declared effect's operations. An operation `op : Arg -> Res`
    /// becomes a value: bound unqualified when a single effect declares that name,
    /// or an overload when several do (resolved by result type at the use site);
    /// always reachable qualified as `Effect.op`. Its `Arg -> Res` scheme is also
    /// kept per effect for handler-clause typing.
    /// Build the record row of every struct and hand them to the engine, so a
    /// nominal struct can unify with a structural record row (the hybrid bridge).
    /// Each row is a scheme: its parameters become fresh vars (recorded in
    /// declaration order), so a generic struct instance `Box Int` bridges by
    /// substituting its type arguments for those parameter vars per use.
    fn register_struct_rows(&mut self) {
        let decls: Vec<(&'a str, Vec<&'a str>, Vec<(&'a str, Aol<Ty>)>)> = self
            .structs
            .iter()
            .map(|(name, info)| (*name, info.params.clone(), info.fields.clone()))
            .collect();
        let mut rows = HashMap::new();
        for (name, params, fields) in decls {
            let mut tvars = HashMap::new();
            // A fresh var per parameter, in order, so App arguments line up with
            // them at bridge time (a phantom param still gets a slot, unused).
            let param_ids: Vec<VarId> = params
                .iter()
                .map(|p| {
                    let v = self.eng.fresh();
                    let id = match v {
                        Type::Var(id) => id,
                        _ => unreachable!("fresh() returns a variable"),
                    };
                    tvars.insert(*p, v);
                    id
                })
                .collect();
            let row = fields.iter().rev().fold(Type::RowEmpty, |rest, (fname, fty)| {
                Type::row_field(fname, self.ty_of_ast(*fty, &mut tvars), rest)
            });
            rows.insert(name.to_string(), (param_ids, row));
        }
        self.eng.set_struct_rows(rows);
    }

    fn register_effects(&mut self, program: &Program) {
        let mut per_op: HashMap<&'a str, Vec<Type>> = HashMap::new();
        for item in program.items.iter() {
            let Item::Effect { name, ops } = item else {
                continue;
            };
            let effect = self.text(*name);
            let mut op_schemes = HashMap::new();
            for op in ops.iter() {
                let op_name = self.text(op.name);
                let base = self.scheme_of_sig(op.ty);
                let scheme = self.with_effect(base, effect);
                op_schemes.insert(op_name, scheme.clone());
                per_op.entry(op_name).or_default().push(scheme.clone());
                self.qualified
                    .entry(effect)
                    .or_default()
                    .insert(op_name, vec![scheme]);
            }
            self.effect_ops.insert(effect, op_schemes);
        }
        for (op_name, mut schemes) in per_op {
            if schemes.len() == 1 {
                self.bind(op_name, schemes.pop().expect("one scheme"));
            } else {
                self.overloads
                    .entry(op_name)
                    .or_default()
                    .extend(schemes.into_iter().map(Cand::local));
            }
        }
    }

    /// Give an operation's outermost arrow the latent effect `<effect | mu>` (mu
    /// a fresh quantified row variable), so performing it forces `effect` into the
    /// ambient and fits any ambient already containing more effects.
    fn with_effect(&mut self, scheme: Type, effect: &str) -> Type {
        match scheme {
            Type::Arrow(from, to, _) => {
                let mu = self.eng.fresh_generic();
                Type::arrow_eff(*from, *to, Type::row_extend(effect, mu))
            }
            other => other,
        }
    }

    /// The effect a handler clause discharges: its explicit qualifier, or the
    /// unique effect declaring a bare operation (ambiguous / unknown -> `None`,
    /// left to dynamic dispatch).
    fn op_owner(&self, effect: Option<&str>, op: &str) -> Option<String> {
        if let Some(e) = effect {
            return Some(e.to_string());
        }
        let mut found = None;
        for (eff, ops) in &self.effect_ops {
            if ops.contains_key(op) {
                if found.is_some() {
                    return None;
                }
                found = Some((*eff).to_string());
            }
        }
        found
    }

    /// The `Arg -> Res` scheme of an operation named in a handler clause, resolved
    /// by its explicit effect or, for a bare name, by the unique effect declaring
    /// it.
    fn resolve_op_ty(&self, effect: Option<&str>, op: &str) -> Option<Type> {
        if let Some(e) = effect {
            return self.effect_ops.get(e).and_then(|ops| ops.get(op)).cloned();
        }
        let mut found = None;
        for ops in self.effect_ops.values() {
            if let Some(scheme) = ops.get(op) {
                if found.is_some() {
                    return None;
                }
                found = Some(scheme.clone());
            }
        }
        found
    }

    // -- struct / union typing ----------------------------------------------

    fn infer_field(&mut self, rec_ty: &Type, field: &str) -> Result<Type> {
        let (head, args) = self.spine(rec_ty);
        if let Type::Con(name) = &head {
            if let Some(info) = self.structs.get(name.as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                if let Some((_, ty)) = info.fields.iter().find(|(n, _)| *n == field) {
                    return Ok(self.ty_of_ast(*ty, &mut subst));
                }
            }
        }
        if let Type::Tuple(items) = &head {
            if let Ok(idx) = field.parse::<usize>() {
                if let Some(t) = items.get(idx) {
                    return Ok(t.clone());
                }
            }
        }
        // A structural record (an open-row parameter, say): look the field up in
        // the row, growing an open tail to include it.
        if let Type::Record(_) = self.eng.resolve(rec_ty) {
            return self
                .eng
                .record_field(rec_ty, field, &format!("accessing field `{field}`"));
        }
        Ok(self.eng.fresh())
    }

    fn infer_struct_lit(
        &mut self,
        site: Aol<Expr>,
        ty: Option<&'a str>,
        fields: &'a [FieldInit],
        spread: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let (info, result, mut subst) = if let Some(base) = spread {
            let base_ty = self.infer(base)?;
            let (head, args) = self.spine(&base_ty);
            match &head {
                Type::Con(n) if self.structs.contains_key(n.as_str()) => {
                    let (name, info) = self
                        .structs
                        .get_key_value(n.as_str())
                        .map(|(k, v)| (*k, v.clone()))
                        .expect("struct present");
                    self.struct_lit_names.insert(site, name.to_string());
                    let subst = subst_from_args(&info.params, &args, &mut self.eng);
                    (info, base_ty, subst)
                }
                _ => {
                    self.infer_field_inits(fields)?;
                    return Ok(base_ty);
                }
            }
        } else {
            let resolved = match ty {
                Some(n) => self.structs.get_key_value(n).map(|(k, v)| (*k, v.clone())),
                None => self.resolve_struct_by_fields(fields),
            };
            match resolved {
                Some((name, info)) => {
                    self.struct_lit_names.insert(site, name.to_string());
                    let (args, subst) = self.instantiate_params(&info.params);
                    (info, applied(name, &args), subst)
                }
                // No fresh() escape hatch: a struct literal whose type cannot be
                // determined is a compile error, not a silent runtime fault. The
                // checking direction (an annotation / expected type / qualified
                // `Type.{..}`) resolves it; a bare positional literal cannot.
                None => {
                    self.infer_field_inits(fields)?;
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "cannot infer which struct this `.{{ .. }}` builds"
                    )
                    .with_note(
                        "qualify it (`Type.{ .. }`), annotate the binding, or use named fields that match a struct"
                            .to_string(),
                    ));
                }
            }
        };

        for (i, fi) in fields.iter().enumerate() {
            let (decl_ty, value) = match fi {
                FieldInit::Named { name, value } => {
                    let name = self.text(*name);
                    match info.fields.iter().find(|(n, _)| *n == name) {
                        Some((_, t)) => (*t, *value),
                        None => {
                            self.infer(*value)?;
                            continue;
                        }
                    }
                }
                FieldInit::Positional(value) => match info.fields.get(i) {
                    Some((_, t)) => (*t, *value),
                    None => {
                        self.infer(*value)?;
                        continue;
                    }
                },
            };
            let want = self.ty_of_ast(decl_ty, &mut subst);
            let got = self.infer(value)?;
            self.eng.unify(&got, &want, "in a struct field")?;
        }
        Ok(result)
    }

    fn infer_variant(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &'a [FieldInit],
    ) -> Result<Type> {
        let resolved = match ty {
            Some(n) => self.variant_sig(n, tag),
            None => self
                .find_union_by_tag(tag)
                .and_then(|u| self.variant_sig(u, tag)),
        };
        let (result, payload) = match resolved {
            Some(r) => r,
            None => {
                self.infer_field_inits(fields)?;
                return Ok(self.eng.fresh());
            }
        };
        for (i, fi) in fields.iter().enumerate() {
            let (want, value) = match fi {
                FieldInit::Named { name, value } => (
                    variant_field_ty(&payload, Some(self.text(*name)), i),
                    *value,
                ),
                FieldInit::Positional(value) => (variant_field_ty(&payload, None, i), *value),
            };
            let got = self.infer(value)?;
            if let Some(want) = want {
                self.eng.unify(&got, &want, "in a variant payload")?;
            }
        }
        Ok(result)
    }

    fn variant_sig(&mut self, union: &str, tag: &str) -> Option<(Type, VariantPayload<'a>)> {
        if union == ty::LIST {
            let elem = self.eng.fresh();
            let list = Type::app(Type::con(ty::LIST), elem.clone());
            return match tag {
                "Nil" => Some((list, vec![])),
                "Cons" => Some((list.clone(), vec![(None, elem), (None, list)])),
                _ => None,
            };
        }
        let info = self.unions.get(union)?.clone();
        let pos = info.variants.iter().position(|v| v.tag == tag)?;
        let (args, mut subst) = self.instantiate_params(&info.params);
        let result = applied(union, &args);
        let variant = &info.variants[pos];
        let payload = variant
            .payload
            .clone()
            .into_iter()
            .map(|(name, ast_ty)| (name, self.ty_of_ast(ast_ty, &mut subst)))
            .collect();
        Some((result, payload))
    }

    fn find_union_by_tag(&self, tag: &str) -> Option<&'a str> {
        let user = self
            .unions
            .iter()
            .find_map(|(name, info)| info.variants.iter().any(|v| v.tag == tag).then_some(*name));
        user.or(match tag {
            "Nil" | "Cons" => Some(ty::LIST),
            _ => None,
        })
    }

    /// Infer an anonymous record value. Plain `{ .x = e }` builds a closed row from
    /// the fields' inferred types; `{ .x = e, with base }` stacks its fields on
    /// `base`'s (row concat); `{ .x = e | base }` updates `base` (its shape is
    /// preserved, each listed field must already exist).
    fn infer_record(
        &mut self,
        fields: &'a [FieldInit],
        with: Option<Aol<Expr>>,
        update: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let mut explicit: Vec<(String, Type)> = Vec::new();
        for fi in fields {
            match fi {
                FieldInit::Named { name, value } => {
                    let t = self.infer(*value)?;
                    explicit.push((self.text(*name).to_string(), t));
                }
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a record field needs a name (`.field = value`)"
                    ));
                }
            }
        }
        if let Some(base) = update {
            // Update preserves the base's shape (open or closed): each listed field
            // must resolve in the base, and the result type is the base's.
            let base_ty = self.infer(base)?;
            for (n, got) in &explicit {
                let want = self.field_type_of(&base_ty, n)?;
                self.eng.unify(got, &want, "in a record update")?;
            }
            return Ok(base_ty);
        }
        if let Some(w) = with {
            // Stack: prepend the explicit fields onto the base's row, keeping the
            // base's tail (so stacking onto an open row stays open).
            let wty = self.infer(w)?;
            let row = self.record_row_of(&wty)?;
            let full = explicit
                .into_iter()
                .rev()
                .fold(row, |rest, (n, t)| Type::row_field(&n, t, rest));
            return Ok(Type::record(full));
        }
        Ok(Type::record_of(explicit.into_iter()))
    }

    /// The type of field `name` in a record value: through the row for a structural
    /// record (open tail grows to include it), or the declared field for a struct.
    fn field_type_of(&mut self, base: &Type, name: &str) -> Result<Type> {
        if let Type::Record(_) = self.eng.resolve(base) {
            return self
                .eng
                .record_field(base, name, "in a record update");
        }
        for (fname, fty) in self.record_fields_of(base)? {
            if fname == name {
                return Ok(fty);
            }
        }
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "record update sets `{name}`, which the base record does not have"
        ))
    }

    /// The record row of a value (the inner row of a structural record, possibly
    /// open; or a struct's closed row), for stacking fields onto it.
    fn record_row_of(&mut self, base: &Type) -> Result<Type> {
        if let Type::Record(row) = self.eng.resolve(base) {
            return Ok((*row).clone());
        }
        let fields = self.record_fields_of(base)?;
        Ok(fields
            .into_iter()
            .rev()
            .fold(Type::RowEmpty, |rest, (n, t)| Type::row_field(&n, t, rest)))
    }

    /// Whether `ty` is a record-shaped target: a structural record row, or a
    /// The nominal struct name `ty` resolves to (possibly applied), if any.
    fn struct_name_of(&self, ty: &Type) -> Option<&'a str> {
        let (head, _) = self.spine(ty);
        if let Type::Con(n) = &head {
            if let Some((k, _)) = self.structs.get_key_value(n.as_str()) {
                return Some(k);
            }
        }
        None
    }

    /// The head codata type name and its type arguments, if `ty` is a (possibly
    /// applied) declared codata type.
    fn codata_head(&self, ty: &Type) -> Option<(&'a str, Vec<Type>)> {
        let (head, args) = self.spine(ty);
        if let Type::Con(name) = &head {
            if let Some((n, _)) = self.codata.get_key_value(name.as_str()) {
                return Some((n, args));
            }
        }
        None
    }

    /// Check a codata construction `{ .obs = e, ... }` against `expected`: every
    /// observation must be supplied and typed at its declared result type.
    fn check_codata_lit(
        &mut self,
        site: Aol<Expr>,
        fields: &'a [FieldInit],
        expected: &Type,
    ) -> Result<()> {
        let (name, args) = self.codata_head(expected).expect("guarded on a codata type");
        let info = self.codata[name].clone();
        let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
        for (obs, obs_ty) in &info.observations {
            let clause = fields.iter().find_map(|f| match f {
                FieldInit::Named { name, value } if self.text(*name) == *obs => Some(*value),
                _ => None,
            });
            match clause {
                Some(value) => {
                    let want = self.ty_of_ast(*obs_ty, &mut subst);
                    self.check(value, &want)?;
                }
                None => {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "codata `{name}` construction is missing observation `{obs}`"
                    ))
                }
            }
        }
        for f in fields {
            if let FieldInit::Named { name: fname, .. } = f {
                let n = self.text(*fname);
                if !info.observations.iter().any(|(o, _)| *o == n) {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "codata `{name}` has no observation `{n}`"
                    ));
                }
            }
        }
        self.codata_lits.insert(site);
        Ok(())
    }

    /// The `(label, type)` fields of a record value: a structural [`Type::Record`]
    /// (its closed row) or a nominal struct (its declared fields, instantiated).
    fn record_fields_of(&mut self, ty: &Type) -> Result<Vec<(String, Type)>> {
        if let Type::Record(row) = self.eng.resolve(ty) {
            let mut out = Vec::new();
            let mut cur = (*row).clone();
            loop {
                match self.eng.resolve(&cur) {
                    Type::RowField(l, fty, rest) => {
                        out.push((l, (*fty).clone()));
                        cur = *rest;
                    }
                    Type::RowEmpty => return Ok(out),
                    _ => {
                        return Err(diag!(
                            Code::TypeMismatch, Span::at(0), 0,
                            "cannot use a record with an unknown (open) row here"
                        ))
                    }
                }
            }
        }
        let (head, args) = self.spine(ty);
        if let Type::Con(name) = &head {
            if let Some(info) = self.structs.get(name.as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                return Ok(info
                    .fields
                    .iter()
                    .map(|(n, fty)| (n.to_string(), self.ty_of_ast(*fty, &mut subst)))
                    .collect());
            }
        }
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "expected a record or struct value here"
        ))
    }

    fn resolve_struct_by_fields(&self, fields: &[FieldInit]) -> Option<(&'a str, StructInfo<'a>)> {
        let mut names = Vec::with_capacity(fields.len());
        for f in fields {
            match f {
                FieldInit::Named { name, .. } => names.push(self.text(*name)),
                FieldInit::Positional(_) => return None,
            }
        }
        self.structs.iter().find_map(|(sname, info)| {
            let same = info.fields.len() == names.len()
                && names
                    .iter()
                    .all(|n| info.fields.iter().any(|(f, _)| f == n));
            same.then(|| (*sname, info.clone()))
        })
    }

    fn struct_field_ty(
        &mut self,
        info: &StructInfo<'a>,
        subst: &mut HashMap<&'a str, Type>,
        name: Option<&str>,
        index: usize,
    ) -> Option<Type> {
        let ast_ty = match name {
            Some(name) => info
                .fields
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, t)| *t),
            None => info.fields.get(index).map(|(_, t)| *t),
        }?;
        Some(self.ty_of_ast(ast_ty, subst))
    }

    fn instantiate_params(&mut self, params: &[&'a str]) -> (Vec<Type>, HashMap<&'a str, Type>) {
        let mut subst = HashMap::new();
        let mut args = Vec::with_capacity(params.len());
        for p in params {
            let v = self.eng.fresh();
            subst.insert(*p, v.clone());
            args.push(v);
        }
        (args, subst)
    }

    fn spine(&self, ty: &Type) -> (Type, Vec<Type>) {
        let mut args = Vec::new();
        let mut cur = self.eng.resolve(ty);
        while let Type::App(head, arg) = cur {
            args.push(self.eng.resolve(&arg));
            cur = self.eng.resolve(&head);
        }
        args.reverse();
        (cur, args)
    }

    // -- environment --------------------------------------------------------

    pub fn show(&self, ty: &Type) -> String {
        self.eng.show(ty)
    }

    fn bind(&mut self, name: &'a str, ty: Type) {
        self.scopes
            .last_mut()
            .expect("a scope is always open")
            .insert(name, ty);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn leave_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty(), "popped the global scope");
    }

    // -- expression inference ----------------------------------------------

    pub fn infer(&mut self, e: Aol<Expr>) -> Result<Type> {
        let r = self.infer_node(e);
        r.map_err(|d| match self.ast.expr_span(e) {
            Some(span) => d.fill_span(span),
            None => d,
        })
    }

    fn infer_node(&mut self, e: Aol<Expr>) -> Result<Type> {
        match self.node(e) {
            Expr::Int(_) => {
                let t = self.eng.fresh();
                self.numeric.push(t.clone());
                Ok(t)
            }
            Expr::Real(_) => Ok(Type::con(ty::REAL)),
            Expr::Str(_) => Ok(Type::con(ty::STR)),
            Expr::Bool(_) => Ok(Type::con(ty::BOOL)),
            Expr::Unit => Ok(Type::con(ty::UNIT)),

            Expr::Var { module, name } => {
                let module = module.map(|m| self.text(m));
                let name = self.text(*name);
                self.infer_var(module, name, e)
            }

            Expr::App(..) => self.infer_app(e),

            Expr::BinOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (self.text(*op), *lhs, *rhs);
                let tl = self.infer(lhs)?;
                let tr = self.infer(rhs)?;
                if let Some(cands) = self.overloads.get(op).cloned() {
                    return self.resolve_overload(op, &cands, &[tl, tr], None);
                }
                let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                let op_ty = self.eng.instantiate(&scheme);
                let result = self.eng.fresh();
                // The operator's result arrow may carry a latent effect: `<|` and
                // `|>` pass one through from their function argument (`f <| x` is a
                // call to `f`). Force it into the ambient the way a plain
                // application would. For every other operator this row is empty and
                // the subrow is a no-op.
                let eff = self.eng.fresh();
                let want = Type::arrow(tl, Type::arrow_eff(tr, result.clone(), eff.clone()));
                self.eng
                    .unify(&op_ty, &want, &format!("in operator `{op}`"))?;
                let amb = self.ambient.clone();
                self.eng
                    .subrow(&eff, &amb, &format!("in operator `{op}`"))?;
                Ok(result)
            }

            Expr::UnOp { op, operand } => {
                let (op, operand) = (self.text(*op), *operand);
                let t = self.infer(operand)?;
                if let Some(cands) = self.overloads.get(op).cloned() {
                    return self.resolve_overload(op, &cands, &[t], None);
                }
                let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                let op_ty = self.eng.instantiate(&scheme);
                let result = self.eng.fresh();
                self.eng.unify(
                    &op_ty,
                    &Type::arrow(t, result.clone()),
                    &format!("in unary `{op}`"),
                )?;
                Ok(result)
            }

            Expr::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                for item in items.iter() {
                    tys.push(self.infer(*item)?);
                }
                Ok(Type::Tuple(tys))
            }

            Expr::List(items) => {
                let elem = self.eng.fresh();
                for item in items.iter() {
                    let t = self.infer(*item)?;
                    self.eng.unify(&elem, &t, "in a list literal")?;
                }
                Ok(Type::app(Type::con(ty::LIST), elem))
            }

            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let tc = self.infer(cond)?;
                self.eng
                    .unify(&tc, &Type::con(ty::BOOL), "in an 'if' condition")?;
                let tt = self.infer(then)?;
                let ta = self.infer(alt)?;
                self.eng
                    .unify(&tt, &ta, "between the branches of an 'if'")?;
                Ok(tt)
            }

            Expr::Let { bindings, body } => {
                let body = *body;
                self.enter_scope();
                self.infer_let_group(bindings)?;
                let t = self.infer(body);
                self.leave_scope();
                t
            }

            Expr::Lambda { params, body } => {
                let body = *body;
                self.enter_scope();
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params.iter() {
                    let pv = self.eng.fresh();
                    self.type_pattern(*p, &pv)?;
                    param_tys.push(pv);
                }
                // Constructing the closure performs nothing under the current
                // ambient; the body runs under its own fresh ambient, which becomes
                // the innermost arrow's latent effect. Outer (curried, partial-
                // application) arrows stay pure.
                let e_body = self.eng.fresh();
                let saved = std::mem::replace(&mut self.ambient, e_body.clone());
                let body_ty = self.infer(body);
                self.ambient = saved;
                self.leave_scope();
                let body_ty = body_ty?;
                let last = param_tys.len() - 1;
                let ty = param_tys.into_iter().enumerate().rev().fold(
                    body_ty,
                    |acc, (i, p)| {
                        if i == last {
                            Type::arrow_eff(p, acc, e_body.clone())
                        } else {
                            Type::arrow(p, acc)
                        }
                    },
                );
                Ok(ty)
            }

            Expr::Match {
                scrut,
                arms,
                default,
            } => {
                let (scrut, default) = (*scrut, *default);
                let ts = self.infer(scrut)?;
                let result = self.eng.fresh();
                for arm in arms.iter() {
                    self.enter_scope();
                    for pat in arm.patterns.iter() {
                        self.type_pattern(*pat, &ts)?;
                    }
                    if let Some(guard) = arm.guard {
                        let tg = self.infer(guard)?;
                        self.eng
                            .unify(&tg, &Type::con(ty::BOOL), "in a match guard")?;
                    }
                    let tb = self.infer(arm.body)?;
                    self.eng.unify(&result, &tb, "between match arms")?;
                    self.leave_scope();
                }
                if let Some(d) = default {
                    let td = self.infer(d)?;
                    self.eng.unify(&result, &td, "in a match 'else' branch")?;
                }
                Ok(result)
            }

            Expr::Field { record, name } => {
                let (record, name) = (*record, self.text(*name));
                let rec_ty = self.infer(record)?;
                // `x.obs` on a codata value is an observation (record the site so
                // lowering runs the thunk); otherwise it is field/tuple access.
                if let Some((cname, args)) = self.codata_head(&rec_ty) {
                    let info = self.codata[cname].clone();
                    if let Some((_, obs_ty)) = info.observations.iter().find(|(o, _)| *o == name) {
                        let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                        let obs_ty = *obs_ty;
                        self.observations.insert(e);
                        return Ok(self.ty_of_ast(obs_ty, &mut subst));
                    }
                }
                self.infer_field(&rec_ty, name)
            }
            Expr::Index { recv, index } => {
                let (recv, index) = (*recv, *index);
                let rec_ty = self.infer(recv)?;
                let size = self.eng.fresh_nat();
                let elem = self.eng.fresh();
                let tensor = Type::app(Type::app(Type::con(TENSOR), size), elem.clone());
                self.eng.unify(&rec_ty, &tensor, "indexing a tensor `t.[i]`")?;
                let idx_ty = self.infer(index)?;
                self.eng
                    .unify(&idx_ty, &Type::con(ty::INT), "a tensor index must be an Int")?;
                self.index_exprs.insert(e);
                Ok(elem)
            }
            Expr::StructLit { ty, fields, spread } => {
                let ty = ty.map(|t| self.text(t));
                self.infer_struct_lit(e, ty, fields, *spread)
            }
            Expr::Record {
                fields,
                with,
                update,
            } => self.infer_record(fields, *with, *update),
            Expr::Variant {
                ty, tag, fields, ..
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                self.infer_variant(ty, tag, fields)
            }

            Expr::Array { size } => {
                let size = *size;
                let ts = self.infer(size)?;
                self.eng
                    .unify(&ts, &Type::con(ty::INT), "in an array size")?;
                Ok(Type::con(ty::ARRAY))
            }
            Expr::With { subject, body } => {
                let (subject, body) = (*subject, *body);
                let subject_ty = self.infer(subject)?;
                self.enter_scope();
                let names = self.scope_struct_fields(&subject_ty)?;
                self.with_fields.insert(e, names);
                let t = self.infer(body);
                self.leave_scope();
                t
            }
            Expr::Handle { body, handler } => match handler {
                None => self.infer(*body),
                Some(handler) => self.infer_handle(*body, handler),
            },
            Expr::Defer { cleanup, body } => {
                let (cleanup, body) = (*cleanup, *body);
                self.infer(cleanup)?;
                self.infer(body)
            }
            Expr::Extern { .. } => {
                let v = self.eng.fresh();
                self.extern_tys.insert(e, v.clone());
                Ok(v)
            }

            Expr::Ctx {
                callee,
                overrides,
                rest,
            } => {
                let (callee, rest) = (*callee, *rest);
                let overrides = overrides.to_vec();
                let Some(fsite) = self.head_var_site(callee) else {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "`@ctx` must be applied to a function that declares `@ctx` implicit parameters"
                    ));
                };
                self.ctx_overrides.insert(fsite, (overrides, rest));
                let ty = self.infer(callee)?;
                // The reference consumes its overrides; a leftover entry means the
                // callee has no `@ctx` implicits for them to apply to.
                if self.ctx_overrides.remove(&fsite).is_some() {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "the target of this `@ctx` has no implicit parameters"
                    ));
                }
                Ok(ty)
            }
        }
    }

    fn infer_field_inits(&mut self, fields: &'a [FieldInit]) -> Result<()> {
        for f in fields {
            match f {
                FieldInit::Named { value, .. } => {
                    self.infer(*value)?;
                }
                FieldInit::Positional(v) => {
                    self.infer(*v)?;
                }
            }
        }
        Ok(())
    }

    /// Type a `do body ctl k <clauses> [else x = e]` handler. `R` is the result
    /// of the whole handled computation: every clause body and the `else` body has
    /// type `R`, and with no `else` the body's own value passes through (`R` is the
    /// body type). In a clause handling `op : Arg -> Res`, the payload `arg` has
    /// type `Arg` and the continuation `k` has type `Res -> R` (a deep handler:
    /// resuming yields the final result).
    fn infer_handle(
        &mut self,
        body: Aol<Expr>,
        handler: &'a crate::parser::data::Handler,
    ) -> Result<Type> {
        let result = self.eng.fresh();

        // The handler discharges the effects its clauses name: the body runs under
        // the ambient extended with each DISTINCT handled effect (several clauses
        // may handle one effect, e.g. get/put both belong to State), the handle
        // expression itself under the outer ambient.
        let mut inner = self.ambient.clone();
        let mut seen: HashSet<String> = HashSet::new();
        for clause in handler.clauses.iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            if let Some(eff_name) = self.op_owner(effect, op) {
                if seen.insert(eff_name.clone()) {
                    inner = Type::row_extend(&eff_name, inner);
                }
            }
        }
        let saved = std::mem::replace(&mut self.ambient, inner);
        let body_ty = self.infer(body);
        self.ambient = saved;
        let body_ty = body_ty?;

        for clause in handler.clauses.iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            let (arg_ty, res_ty) = match self.resolve_op_ty(effect, op) {
                Some(scheme) => {
                    let inst = self.eng.instantiate(&scheme);
                    let (a, r, _) = self.arrow_parts(&inst)?;
                    (a, r)
                }
                None => (self.eng.fresh(), self.eng.fresh()),
            };
            self.enter_scope();
            self.bind(self.text(clause.arg), arg_ty);
            // Deep handler: resuming continues the computation under the outer
            // ambient, so `k : Res -[amb]-> R`.
            let amb = self.ambient.clone();
            self.bind(
                self.text(handler.continuation),
                Type::arrow_eff(res_ty, result.clone(), amb),
            );
            let cb = self.infer(clause.body)?;
            self.eng.unify(&cb, &result, "in a handler clause")?;
            self.leave_scope();
        }
        match &handler.default {
            Some((name, else_body)) => {
                self.enter_scope();
                self.bind(self.text(*name), body_ty);
                let eb = self.infer(*else_body)?;
                self.eng.unify(&eb, &result, "in a handler 'else' clause")?;
                self.leave_scope();
            }
            // With no `else` clause the return (value) case defaults to identity,
            // so the body's result becomes the handler's result. When they differ
            // the handler needs an `else` clause to convert the body's value.
            None => {
                if self.eng.unify(&body_ty, &result, "in a handled body").is_err() {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "the body produces {}, but the handler's clauses produce {}",
                        self.eng.show(&body_ty), self.eng.show(&result);
                        note: "with no `else` clause the body's result is returned unchanged; add an `else x => ...` clause to convert it"
                    ));
                }
            }
        }
        Ok(result)
    }

    // -- overloading --------------------------------------------------------

    fn infer_var(
        &mut self,
        module: Option<&'a str>,
        name: &'a str,
        site: Aol<Expr>,
    ) -> Result<Type> {
        if let Some(m) = module {
            return match self.qualified_candidates(m, name) {
                Some(cands) if cands.len() == 1 => Ok(self.eng.instantiate(&cands[0])),
                _ => Ok(self.eng.fresh()),
            };
        }
        // Qualify a reference to a global so the interpreter reaches the intended
        // definition rather than a same-named global from another loaded module.
        // A local definition resolves to this module; a single imported value to
        // its owner. Skip names an inner binder shadows, and overloaded names
        // (resolved instead at the application site).
        if !self.shadowed_locally(name) && !self.overloads.contains_key(name) {
            if self.local_defs.contains(name) {
                self.resolved_calls.insert(site, self.module_name);
            } else if let Some(m) = self.value_module.get(name).copied() {
                self.resolved_calls.insert(site, m);
            }
        }
        // A reference to a `@ctx`-bearing global (not shadowed by a local of the
        // same name): instantiate its signature and requirement types with one
        // shared variable map, resolve each implicit by name, and record the
        // arguments for lowering. The returned type is the plain arrow, so callers
        // apply only the explicit parameters.
        if !self.shadowed_locally(name) {
            if let Some(gi) = self.global_implicits.get(name).cloned() {
                let mut tvars = HashMap::new();
                let arrow = self.ty_of_ast(gi.sig, &mut tvars);
                let reqs: Vec<(&'a str, Type)> = gi
                    .decls
                    .iter()
                    .map(|d| (self.text(d.name), self.ty_of_ast(d.ty, &mut tvars)))
                    .collect();
                self.plan_implicits(site, name, &reqs)?;
                return Ok(arrow);
            }
        }
        if let Some(scheme) = self.lookup(name) {
            Ok(self.eng.instantiate(&scheme))
        } else if self.overloads.contains_key(name) {
            Ok(self.eng.fresh())
        } else {
            Err(unbound(name))
        }
    }

    /// Whether `name` is bound by an inner scope (a lambda/`let`/pattern binder),
    /// as opposed to the global scope where imports and top-level defs live.
    fn shadowed_locally(&self, name: &str) -> bool {
        self.scopes[1..].iter().any(|s| s.contains_key(name))
    }

    fn qualified_candidates(&self, module: &str, name: &str) -> Option<Vec<Type>> {
        self.qualified.get(module)?.get(name).cloned()
    }

    // -- implicit (`@ctx`) resolution ---------------------------------------

    /// Plan the implicit arguments of `fname` at use `site`. An explicit override
    /// or a local binder of the name is resolved immediately; a global/overloaded
    /// provider is deferred until inference pins the requirement's type variables
    /// (so `maxOf 3 7` knows the implicit is over `Int`, not a bare variable).
    fn plan_implicits(
        &mut self,
        site: Aol<Expr>,
        fname: &str,
        reqs: &[(&'a str, Type)],
    ) -> Result<()> {
        let overrides = self.ctx_overrides.remove(&site);
        let mut slots: Vec<Option<ImplicitArg>> = Vec::with_capacity(reqs.len());
        for (idx, (implname, reqty)) in reqs.iter().enumerate() {
            if let Some((ov, rest)) = &overrides {
                if let Some(value) = self.find_override(ov, implname, reqs.len()) {
                    self.check(value, reqty)?;
                    slots.push(Some(ImplicitArg::Expr(value)));
                    continue;
                }
                if !rest {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "`@ctx` for `{fname}` does not supply the implicit `{implname}`"
                    )
                    .with_note(
                        "add it to the `@ctx`, or end the `@ctx` with `..` to resolve the rest by name"
                            .to_string(),
                    ));
                }
            }
            if self.shadowed_locally(implname) {
                if let Some(t) = self.lookup(implname) {
                    let inst = self.eng.instantiate(&t);
                    self.unify_implicit(fname, implname, &inst, reqty)?;
                    slots.push(Some(ImplicitArg::Bare(implname.to_string())));
                    continue;
                }
            }
            slots.push(None);
            self.implicit_pending.push(PendingImpl {
                site,
                idx,
                fname: fname.to_string(),
                implname,
                reqty: reqty.clone(),
            });
        }
        if let Some((ov, _)) = &overrides {
            self.check_unknown_overrides(fname, ov, reqs)?;
        }
        self.implicit_slots.insert(site, slots);
        Ok(())
    }

    /// Solve every deferred implicit (called at a definition boundary, after
    /// overload solving and numeric defaulting have pinned the types), then move
    /// each fully-resolved site into [`Self::implicit_args`].
    fn resolve_pending_implicits(&mut self) -> Result<()> {
        for p in std::mem::take(&mut self.implicit_pending) {
            let reqty = self.eng.zonk(&p.reqty);
            let arg = self
                .resolve_deferred_implicit(&p.fname, p.implname, &reqty)
                .map_err(|d| match self.ast.expr_span(p.site) {
                    Some(span) => d.fill_span(span),
                    None => d,
                })?;
            if let Some(slots) = self.implicit_slots.get_mut(&p.site) {
                slots[p.idx] = Some(arg);
            }
        }
        let sites: Vec<Aol<Expr>> = self.implicit_slots.keys().copied().collect();
        for site in sites {
            let complete = self.implicit_slots[&site].iter().all(Option::is_some);
            if complete {
                let slots = self.implicit_slots.remove(&site).expect("just checked");
                let args: Vec<ImplicitArg> = slots.into_iter().map(Option::unwrap).collect();
                if !args.is_empty() {
                    self.implicit_args.insert(site, args);
                }
            }
        }
        Ok(())
    }

    /// Resolve one implicit whose provider is a global: an overloaded name picked
    /// by the (now pinned) requirement type, else a single global. The requirement
    /// still being polymorphic is the v1 limitation, reported here.
    fn resolve_deferred_implicit(
        &mut self,
        fname: &str,
        implname: &'a str,
        reqty: &Type,
    ) -> Result<ImplicitArg> {
        if let Some(cands) = self.overloads.get(implname).cloned() {
            return match self.match_implicit_overload(&cands, reqty) {
                Some(idx) => {
                    let inst = self.eng.instantiate(&cands[idx].ty);
                    self.unify_implicit(fname, implname, &inst, reqty)?;
                    Ok(self.implicit_global_ref(implname, &cands, idx))
                }
                None => Err(self.no_implicit(fname, implname, reqty)),
            };
        }
        if let Some(t) = self.lookup(implname) {
            let inst = self.eng.instantiate(&t);
            self.unify_implicit(fname, implname, &inst, reqty)?;
            let module = if self.local_defs.contains(implname) {
                Some(self.module_name.to_string())
            } else {
                self.value_module.get(implname).map(|m| m.to_string())
            };
            return Ok(match module {
                Some(module) => ImplicitArg::Qualified {
                    module,
                    name: implname.to_string(),
                },
                None => ImplicitArg::Bare(implname.to_string()),
            });
        }
        Err(self.no_implicit(fname, implname, reqty))
    }

    /// The override expression for implicit `implname`, if the `@ctx` gives one:
    /// a `.name = e` whose name matches, or the sole positional `@ctx e` when the
    /// function has exactly one implicit.
    fn find_override(
        &self,
        overrides: &[FieldInit],
        implname: &str,
        nreqs: usize,
    ) -> Option<Aol<Expr>> {
        for f in overrides {
            match f {
                FieldInit::Named { name, value } if self.text(*name) == implname => {
                    return Some(*value)
                }
                FieldInit::Positional(value) if nreqs == 1 => return Some(*value),
                _ => {}
            }
        }
        None
    }

    /// Error if a `.name = e` override names an implicit the function does not have.
    fn check_unknown_overrides(
        &self,
        fname: &str,
        overrides: &[FieldInit],
        reqs: &[(&'a str, Type)],
    ) -> Result<()> {
        for f in overrides {
            if let FieldInit::Named { name, .. } = f {
                let n = self.text(*name);
                if !reqs.iter().any(|(implname, _)| *implname == n) {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "`{fname}` has no `@ctx` implicit named `{n}`"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The reference site of an application's head, if it is a plain variable
    /// (the function whose `@ctx` implicits an override applies to).
    fn head_var_site(&self, mut e: Aol<Expr>) -> Option<Aol<Expr>> {
        loop {
            match self.node(e) {
                Expr::App(f, _) => e = *f,
                Expr::Var { .. } => return Some(e),
                _ => return None,
            }
        }
    }

    /// The unique overload candidate whose type unifies with `reqty`, or `None`
    /// (no match, or several) so the caller reports it.
    fn match_implicit_overload(&mut self, cands: &[Cand<'a>], reqty: &Type) -> Option<usize> {
        let mut found = None;
        let mut count = 0;
        for (i, c) in cands.iter().enumerate() {
            let save = self.eng.save();
            let inst = self.eng.instantiate(&c.ty);
            let ok = self.eng.unify(&inst, reqty, "resolving an implicit").is_ok();
            self.eng.restore(save);
            if ok {
                count += 1;
                found = Some(i);
            }
        }
        if count == 1 {
            found
        } else {
            None
        }
    }

    /// Build the global reference for a resolved overloaded implicit, mangling the
    /// name exactly as a normal overloaded call would (so it reaches the right one
    /// of several same-module definitions).
    fn implicit_global_ref(&self, implname: &str, cands: &[Cand<'a>], idx: usize) -> ImplicitArg {
        let cand = &cands[idx];
        let name = match cand.module {
            Some(m) if cands.iter().filter(|c| c.module == Some(m)).count() > 1 => {
                overload_key(implname, &self.eng.zonk(&cand.ty))
            }
            _ => implname.to_string(),
        };
        match cand.module {
            Some(m) => ImplicitArg::Qualified {
                module: m.to_string(),
                name,
            },
            None => ImplicitArg::Bare(name),
        }
    }

    fn unify_implicit(
        &mut self,
        fname: &str,
        implname: &str,
        actual: &Type,
        reqty: &Type,
    ) -> Result<()> {
        self.eng.unify(
            actual,
            reqty,
            &format!("resolving the implicit `{implname}` of `{fname}`"),
        )
    }

    fn no_implicit(&self, fname: &str, implname: &str, reqty: &Type) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no `{implname}` in scope to satisfy the `@ctx` requirement of `{fname}`"
        )
        .with_note(format!(
            "define or import a `{implname} : {}`, or pass it explicitly with `@ctx`",
            self.show(reqty)
        ))
    }

    fn infer_app(&mut self, e: Aol<Expr>) -> Result<Type> {
        let mut args_rev = Vec::new();
        let mut head = e;
        while let Expr::App(f, x) = self.node(head) {
            args_rev.push(*x);
            head = *f;
        }
        args_rev.reverse();
        let args = args_rev;

        if let Expr::Var { module, name } = self.node(head) {
            let module = module.map(|m| self.text(m));
            let name = self.text(*name);
            match module {
                // A bare overloaded call: resolve by argument types and record the
                // winning module so lowering can qualify it. A local binder of the
                // same name (a lambda/`let`/`@ctx` parameter) shadows the overload
                // set, so fall through to ordinary inference in that case.
                None if !self.shadowed_locally(name) => {
                    if let Some(cands) = self.overloads.get(name).cloned() {
                        let arg_tys = args
                            .iter()
                            .map(|a| self.infer(*a))
                            .collect::<Result<Vec<_>>>()?;
                        return self.resolve_overload(name, &cands, &arg_tys, Some(head));
                    }
                }
                None => {}
                // A qualified call already names its module; resolve among that
                // module's candidates without needing an annotation.
                Some(m) => {
                    if let Some(cands) = self.qualified_candidates(m, name).filter(|c| c.len() > 1)
                    {
                        let arg_tys = args
                            .iter()
                            .map(|a| self.infer(*a))
                            .collect::<Result<Vec<_>>>()?;
                        let cands: Vec<Cand> = cands.into_iter().map(Cand::local).collect();
                        return self.resolve_overload(name, &cands, &arg_tys, None);
                    }
                }
            }
        }

        let mut tf = self.infer(head)?;
        for a in &args {
            let (param, result, eff) = self.arrow_parts(&tf)?;
            // The callee may perform at most what the ambient allows; performing
            // an operation (whose latent row is `<Effect | mu>`) forces its effect
            // into the ambient here.
            let amb = self.ambient.clone();
            self.eng.subrow(&eff, &amb, "in a function application")?;
            self.check(*a, &param)?;
            tf = result;
        }
        Ok(tf)
    }

    fn resolve_overload(
        &mut self,
        name: &str,
        candidates: &[Cand<'a>],
        args: &[Type],
        site: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let result = self.eng.fresh();
        match self.match_overload(candidates, args, &result) {
            Match::Unique(idx) => {
                let cand_ty = candidates[idx].ty.clone();
                self.apply_overload(&cand_ty, args, &result)?;
                self.record_overload(site, name, candidates, idx);
                Ok(result)
            }
            Match::None => Err(self.no_overload(name, args)),
            Match::Ambiguous => {
                self.pending.push(Pending {
                    name: name.to_string(),
                    candidates: candidates.to_vec(),
                    args: args.to_vec(),
                    result: result.clone(),
                    site,
                });
                Ok(result)
            }
        }
    }

    /// Note that a bare call `site` resolved to `module`, so lowering can qualify
    /// it. A builtin/local candidate (`module` is `None`) needs no annotation.
    fn record_call(&mut self, site: Option<Aol<Expr>>, module: Option<&'a str>) {
        if let (Some(site), Some(module)) = (site, module) {
            self.resolved_calls.insert(site, module);
        }
    }

    /// Record a resolved overload use: qualify it to its module (`record_call`),
    /// and, when that module defines the name several times (so `MOD.name` alone
    /// would collide), record the type-mangled bare name lowering must emit. The
    /// mangling matches the definition's key in `def_keys` because both derive
    /// from the candidate's type.
    fn record_overload(
        &mut self,
        site: Option<Aol<Expr>>,
        name: &str,
        candidates: &[Cand<'a>],
        idx: usize,
    ) {
        let module = candidates[idx].module;
        self.record_call(site, module);
        if let (Some(site), Some(m)) = (site, module) {
            if candidates.iter().filter(|c| c.module == Some(m)).count() > 1 {
                let key = overload_key(name, &self.eng.zonk(&candidates[idx].ty));
                self.overload_calls.insert(site, key);
            }
        }
    }

    fn match_overload(&mut self, candidates: &[Cand<'a>], args: &[Type], result: &Type) -> Match {
        let mut matched = None;
        let mut count = 0;
        for (idx, cand) in candidates.iter().enumerate() {
            let save = self.eng.save();
            let ok = self.apply_overload(&cand.ty, args, result).is_ok();
            self.eng.restore(save);
            if ok {
                count += 1;
                matched = Some(idx);
            }
        }
        match count {
            1 => Match::Unique(matched.expect("a match was recorded")),
            0 => Match::None,
            _ => Match::Ambiguous,
        }
    }

    fn solve_pending(&mut self) -> Result<()> {
        loop {
            let batch = std::mem::take(&mut self.pending);
            let mut progress = false;
            let mut still = Vec::new();
            for p in batch {
                match self.match_overload(&p.candidates, &p.args, &p.result) {
                    Match::Unique(idx) => {
                        let cand_ty = p.candidates[idx].ty.clone();
                        self.apply_overload(&cand_ty, &p.args, &p.result)?;
                        self.record_overload(p.site, &p.name, &p.candidates, idx);
                        progress = true;
                    }
                    Match::None => return Err(self.no_overload(&p.name, &p.args)),
                    Match::Ambiguous => still.push(p),
                }
            }
            self.pending = still;
            if progress {
                continue;
            }
            if self.default_numerics()? {
                continue;
            }
            if let Some(p) = self.pending.first() {
                let mut mods: Vec<&str> = p.candidates.iter().filter_map(|c| c.module).collect();
                mods.sort_unstable();
                mods.dedup();
                let err = diag!(
                    Code::AmbiguousName, Span::at(0), 0,
                    "ambiguous overloaded use of `{}`", p.name
                );
                let err = match mods.as_slice() {
                    [a, b, ..] => err.with_note(format!(
                        "several imported modules define `{name}` with a matching type; \
                         qualify just this reference to pick one, e.g. `{a}.{name}` or `{b}.{name}` \
                         (the rest of the module keeps using the bare name)",
                        name = p.name
                    )),
                    _ => err,
                };
                return Err(err);
            }
            return Ok(());
        }
    }

    /// Whether `ty` is a record whose row is closed (ends in `RowEmpty`, so its
    /// fields are fully known) rather than open (a tail variable).
    fn record_is_closed(&self, ty: &Type) -> bool {
        let Type::Record(row) = self.eng.resolve(ty) else {
            return false;
        };
        let mut cur = (*row).clone();
        loop {
            match self.eng.resolve(&cur) {
                Type::RowField(_, _, rest) => cur = *rest,
                Type::RowEmpty => return true,
                _ => return false,
            }
        }
    }

    /// Whether `ty` resolves to a not-yet-defaulted numeric-literal variable.
    fn is_numeric(&self, ty: &Type) -> bool {
        let r = self.eng.resolve(ty);
        matches!(r, Type::Var(_)) && self.numeric.iter().any(|n| self.eng.resolve(n) == r)
    }

    fn default_numerics(&mut self) -> Result<bool> {
        let vars = std::mem::take(&mut self.numeric);
        let mut changed = false;
        for t in &vars {
            if let Type::Var(_) = self.eng.resolve(t) {
                self.eng
                    .unify(t, &Type::con(ty::INT), "defaulting an integer literal")?;
                changed = true;
            }
        }
        Ok(changed)
    }

    fn no_overload(&self, name: &str, args: &[Type]) -> Diagnostic {
        let shown: Vec<String> = args.iter().map(|a| self.show(a)).collect();
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no overload of `{name}` matches argument types ({})",
            shown.join(", ")
        )
    }

    fn apply_overload(&mut self, candidate: &Type, args: &[Type], result: &Type) -> Result<()> {
        let mut f = self.eng.instantiate(candidate);
        for a in args {
            let next = self.eng.fresh();
            let eff = self.eng.fresh();
            self.eng.unify(
                &f,
                &Type::arrow_eff(a.clone(), next.clone(), eff.clone()),
                "in an overloaded application",
            )?;
            // An effectful operation resolved by overload still injects its effect
            // into the ambient (same as a plain call; see `infer_app`).
            let amb = self.ambient.clone();
            self.eng.subrow(&eff, &amb, "in an overloaded application")?;
            f = next;
        }
        self.eng.unify(&f, result, "in an overloaded application")
    }

    fn infer_let_group(&mut self, bindings: &'a [Binding]) -> Result<()> {
        for b in bindings {
            self.infer_binding(b)?;
        }
        Ok(())
    }

    fn infer_binding(&mut self, b: &Binding) -> Result<()> {
        self.eng.enter_level();
        let declared = match self.pnode(b.pat) {
            Pattern::Var(name) => {
                let name = self.text(*name);
                let v = self.eng.fresh();
                self.bind(name, v.clone());
                Some(v)
            }
            _ => None,
        };
        // With a signature, CHECK the value against it (bidirectional), so the
        // expected type reaches constructs that need it -- e.g. a positional
        // `.{ .. }` literal resolves its struct from the annotation.
        let value_ty = match b.sig {
            Some(sig) => {
                let mut tvars = HashMap::new();
                let sig_ty = self.ty_of_ast(sig, &mut tvars);
                self.check(b.value, &sig_ty)?;
                sig_ty
            }
            None => self.infer(b.value)?,
        };
        if let Some(decl) = &declared {
            self.eng
                .unify(decl, &value_ty, "in a recursive 'let' binding")?;
        }
        self.eng.leave_level();
        let mono = self.pending_vars();
        match declared {
            Some(decl) => self.eng.generalize_except(&decl, &mono),
            None => {
                self.eng.generalize_except(&value_ty, &mono);
                self.type_pattern(b.pat, &value_ty)?;
            }
        }
        Ok(())
    }

    // -- pattern typing -----------------------------------------------------

    pub fn type_pattern(&mut self, pat: Aol<Pattern>, expected: &Type) -> Result<()> {
        match self.pnode(pat) {
            Pattern::Wild => Ok(()),
            Pattern::Var(name) => {
                self.bind(self.text(*name), expected.clone());
                Ok(())
            }
            Pattern::Int(_) => {
                self.eng
                    .unify(expected, &Type::con(ty::INT), "in an integer pattern")
            }
            Pattern::Real(_) => self
                .eng
                .unify(expected, &Type::con(ty::REAL), "in a real pattern"),
            Pattern::Str(_) => self
                .eng
                .unify(expected, &Type::con(ty::STR), "in a string pattern"),
            Pattern::Bool(_) => {
                self.eng
                    .unify(expected, &Type::con(ty::BOOL), "in a boolean pattern")
            }
            Pattern::Range { lo, hi } => {
                // Both bounds are typed against the scrutinee, so a range forces its
                // scalar (`1 ... 5` -> Int, `1.0 ... 5.0` -> Real) and rejects mixed
                // bounds. It binds nothing.
                let (lo, hi) = (*lo, *hi);
                self.type_pattern(lo, expected)?;
                self.type_pattern(hi, expected)
            }
            Pattern::StrPrefix { rest, .. } => {
                let rest = *rest;
                self.eng
                    .unify(expected, &Type::con(ty::STR), "in a string-prefix pattern")?;
                self.type_pattern(rest, &Type::con(ty::STR))
            }
            Pattern::Tuple(pats) => {
                let vars: Vec<Type> = pats.iter().map(|_| self.eng.fresh()).collect();
                self.eng
                    .unify(expected, &Type::Tuple(vars.clone()), "in a tuple pattern")?;
                for (p, v) in pats.iter().zip(&vars) {
                    self.type_pattern(*p, v)?;
                }
                Ok(())
            }
            Pattern::Cons { head, tail } => {
                let (head, tail) = (*head, *tail);
                let elem = self.eng.fresh();
                let list = Type::app(Type::con(ty::LIST), elem.clone());
                self.eng.unify(expected, &list, "in a '::' pattern")?;
                self.type_pattern(head, &elem)?;
                self.type_pattern(tail, &list)
            }
            Pattern::List { elems, rest } if self.is_array(expected) => {
                self.array_pats.insert(pat);
                let rest = *rest;
                for e in elems.iter() {
                    self.type_pattern(*e, &Type::con(ty::INT))?;
                }
                if let Some(rest) = rest {
                    self.type_pattern(rest, &Type::con(ty::ARRAY))?;
                }
                Ok(())
            }
            Pattern::List { elems, rest } => {
                let rest = *rest;
                let elem = self.eng.fresh();
                let list = Type::app(Type::con(ty::LIST), elem.clone());
                self.eng.unify(expected, &list, "in a list pattern")?;
                for e in elems.iter() {
                    self.type_pattern(*e, &elem)?;
                }
                if let Some(rest) = rest {
                    self.type_pattern(rest, &list)?;
                }
                Ok(())
            }
            Pattern::Struct { ty, fields } => {
                let ty = self.text(*ty);
                self.type_struct_pattern(ty, fields, expected)
            }
            Pattern::Record { fields, rest } => self.type_record_pattern(fields, *rest, expected),
            Pattern::Variant {
                ty, tag, fields, ..
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                self.type_variant_pattern(ty, tag, fields, expected)
            }
        }
    }

    /// Type a record pattern by building an OPEN row from its fields and unifying
    /// with the scrutinee (so it matches any record/struct that has them). Binds
    /// each field's subpattern; the rest may be discarded (`.._`) or bound
    /// (`..name`), in which case the binder gets the leftover row as its record type.
    fn type_record_pattern(
        &mut self,
        fields: &'a [FieldPat],
        rest: Option<Aol<Pattern>>,
        expected: &Type,
    ) -> Result<()> {
        let tail = self.eng.fresh();
        let mut entries: Vec<(&'a str, Type, Option<Aol<Pattern>>)> = Vec::new();
        for f in fields {
            match f {
                FieldPat::Named { name, pat } => {
                    entries.push((self.text(*name), self.eng.fresh(), Some(*pat)))
                }
                FieldPat::Shorthand(name) => entries.push((self.text(*name), self.eng.fresh(), None)),
                FieldPat::Positional(_) => {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a record pattern's fields need names (`.field = pat`)"
                    ))
                }
            }
        }
        let row = entries
            .iter()
            .rev()
            .fold(tail.clone(), |rest, (n, t, _)| Type::row_field(n, t.clone(), rest));
        self.eng
            .unify(expected, &Type::record(row), "in a record pattern")?;
        for (name, t, pat) in entries {
            match pat {
                Some(p) => self.type_pattern(p, &t)?,
                None => self.bind(name, t),
            }
        }
        // `..name` binds the leftover fields as a record over the row tail (which
        // unification has bound to the remaining fields); `.._` is a discard.
        if let Some(r) = rest {
            self.type_pattern(r, &Type::record(tail))?;
        }
        Ok(())
    }

    fn type_struct_pattern(
        &mut self,
        ty: &'a str,
        fields: &'a [FieldPat],
        expected: &Type,
    ) -> Result<()> {
        let info = match self.structs.get(ty).cloned() {
            Some(info) => info,
            None => return self.bind_field_patterns_loose(fields),
        };
        let (args, mut subst) = self.instantiate_params(&info.params);
        self.eng
            .unify(expected, &applied(ty, &args), "in a struct pattern")?;
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let want = self.struct_field_ty(&info, &mut subst, Some(self.text(*name)), i);
                    self.bind_field_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = self.struct_field_ty(&info, &mut subst, None, i);
                    self.bind_field_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = self.text(*name);
                    let want = self
                        .struct_field_ty(&info, &mut subst, Some(name), i)
                        .unwrap_or_else(|| self.eng.fresh());
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    fn type_variant_pattern(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &'a [FieldPat],
        expected: &Type,
    ) -> Result<()> {
        let resolved = match ty {
            Some(n) => self.variant_sig(n, tag),
            None => self
                .find_union_by_tag(tag)
                .and_then(|u| self.variant_sig(u, tag)),
        };
        let (result, payload) = match resolved {
            Some(r) => r,
            None => return self.bind_field_patterns_loose(fields),
        };
        self.eng.unify(expected, &result, "in a variant pattern")?;
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let want = variant_field_ty(&payload, Some(self.text(*name)), i);
                    self.bind_field_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = variant_field_ty(&payload, None, i);
                    self.bind_field_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = self.text(*name);
                    let want = variant_field_ty(&payload, Some(name), i)
                        .unwrap_or_else(|| self.eng.fresh());
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    fn bind_field_pattern(&mut self, pat: Aol<Pattern>, want: Option<Type>) -> Result<()> {
        let want = want.unwrap_or_else(|| self.eng.fresh());
        self.type_pattern(pat, &want)
    }

    fn bind_field_patterns_loose(&mut self, fields: &'a [FieldPat]) -> Result<()> {
        for f in fields {
            match f {
                FieldPat::Named { pat, .. } | FieldPat::Positional(pat) => {
                    let v = self.eng.fresh();
                    self.type_pattern(*pat, &v)?;
                }
                FieldPat::Shorthand(name) => {
                    let v = self.eng.fresh();
                    self.bind(self.text(*name), v);
                }
            }
        }
        Ok(())
    }

    // -- AST types ----------------------------------------------------------

    /// Is `name` a usable type: a built-in base type, or a struct/union/alias
    /// declared here or imported? A bare name that is none of these is a typo (a
    /// type variable is a lowercase name).
    fn is_known_type(&self, name: &str) -> bool {
        is_base_type(name)
            || self.structs.contains_key(name)
            || self.unions.contains_key(name)
            || self.aliases.contains_key(name)
            || self.codata.contains_key(name)
    }

    /// The number of type parameters a user-declared type constructor takes, if
    /// `name` is a struct / union / codata (the ones whose arity we track).
    fn type_arity(&self, name: &str) -> Option<usize> {
        self.structs
            .get(name)
            .map(|i| i.params.len())
            .or_else(|| self.unions.get(name).map(|i| i.params.len()))
            .or_else(|| self.codata.get(name).map(|i| i.params.len()))
            .or_else(|| self.aliases.get(name).map(|(p, _)| p.len()))
    }

    /// Flag an over-applied type constructor (`Weirdtype Int Int Int` for a
    /// two-parameter `Weirdtype`). Deferred like other type-name errors.
    fn check_type_arity(&mut self, ty: Aol<Ty>) {
        let mut count = 0;
        let mut cur = ty;
        while let Ty::App(h, _) = self.tnode(cur) {
            count += 1;
            cur = *h;
        }
        if let Ty::Con { name, .. } = self.tnode(cur) {
            let name = self.text(*name);
            if let Some(arity) = self.type_arity(name) {
                if count > arity && self.unknown_type.is_none() {
                    let mut d = diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "type `{name}` takes {arity} parameter(s) but {count} were given"
                    );
                    if let Some(span) = self.ast.ty_span(ty) {
                        d = d.fill_span(span);
                    }
                    self.unknown_type = Some(d);
                }
            }
        }
    }

    /// If `ty` is an alias applied to zero or more arguments, expand it: bind the
    /// alias's parameters to the arguments (a fresh variable for any not supplied,
    /// so an under-applied alias stays polymorphic, as for structs) and elaborate
    /// the body under that binding. Returns None when the spine head is not an alias.
    fn try_expand_alias(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<&'a str, Type>) -> Option<Type> {
        let mut args = Vec::new();
        let mut cur = ty;
        while let Ty::App(h, a) = self.tnode(cur) {
            args.push(*a);
            cur = *h;
        }
        let name = match self.tnode(cur) {
            Ty::Con { name, .. } => self.text(*name),
            _ => return None,
        };
        let (params, body) = self.aliases.get(name)?.clone();
        args.reverse();
        self.check_type_arity(ty);
        let mut sub: HashMap<&'a str, Type> = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            let t = match args.get(i) {
                Some(&a) => self.ty_of_ast(a, tvars),
                None => self.eng.fresh(),
            };
            sub.insert(*p, t);
        }
        Some(self.ty_of_ast(body, &mut sub))
    }

    /// Elaborate a tensor size: a `Nat` literal, or a (Nat-kinded) size variable
    /// bound by name in `tvars` so `[n]T -> [n]U` shares the one size.
    fn size_ty_of_ast(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<&'a str, Type>) -> Type {
        match self.tnode(ty) {
            Ty::Nat(n) => Type::Nat(*n),
            Ty::Var(name) => {
                let name = self.text(*name);
                let eng = &mut self.eng;
                tvars.entry(name).or_insert_with(|| eng.fresh_nat()).clone()
            }
            Ty::SizeAdd(a, b) => {
                let (a, b) = (*a, *b);
                Type::NatAdd(
                    Box::new(self.size_ty_of_ast(a, tvars)),
                    Box::new(self.size_ty_of_ast(b, tvars)),
                )
            }
            Ty::SizeMul(a, b) => {
                let (a, b) = (*a, *b);
                Type::NatMul(
                    Box::new(self.size_ty_of_ast(a, tvars)),
                    Box::new(self.size_ty_of_ast(b, tvars)),
                )
            }
            _ => self.ty_of_ast(ty, tvars),
        }
    }

    /// Peel a sized-tensor type `@tensor size elem` into `(size, elem)`.
    fn tensor_parts(&self, ty: &Type) -> Option<(Type, Type)> {
        if let Type::App(head, elem) = self.eng.resolve(ty) {
            if let Type::App(con, size) = self.eng.resolve(&head) {
                if matches!(self.eng.resolve(&con), Type::Con(n) if n == TENSOR) {
                    return Some((*size, *elem));
                }
            }
        }
        None
    }

    fn ty_of_ast(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<&'a str, Type>) -> Type {
        // An alias at the head of an application spine expands first, so `MapInt Bool`
        // substitutes into `Map Int Bool` rather than forming `App(alias, Bool)`.
        if let Some(t) = self.try_expand_alias(ty, tvars) {
            return t;
        }
        match self.tnode(ty) {
            Ty::Con { name, .. } => {
                let name = self.text(*name);
                if !self.is_known_type(name) && self.unknown_type.is_none() {
                    let mut d = unknown_type(name);
                    if let Some(span) = self.ast.ty_span(ty) {
                        d = d.fill_span(span);
                    }
                    self.unknown_type = Some(d);
                }
                Type::con(canonical_con(name))
            }
            Ty::Var(name) => {
                let name = self.text(*name);
                tvars
                    .entry(name)
                    .or_insert_with(|| self.eng.fresh())
                    .clone()
            }
            Ty::App(head, arg) => {
                self.check_type_arity(ty);
                let (head, arg) = (*head, *arg);
                Type::app(self.ty_of_ast(head, tvars), self.ty_of_ast(arg, tvars))
            }
            Ty::Nat(n) => Type::Nat(*n),
            // A size expression written in type position (only well-formed inside a
            // `[..]`); elaborate it as a size so kind-checking flags any misuse.
            Ty::SizeAdd(..) | Ty::SizeMul(..) => self.size_ty_of_ast(ty, tvars),
            Ty::Sized { size, elem } => {
                let (size, elem) = (*size, *elem);
                let size_ty = self.size_ty_of_ast(size, tvars);
                let elem_ty = self.ty_of_ast(elem, tvars);
                Type::app(Type::app(Type::con(TENSOR), size_ty), elem_ty)
            }
            Ty::Arrow { from, effect, to } => {
                let (from, to) = (*from, *to);
                // The row's tail (a shared row variable) or the empty closed row,
                // then each written label extended onto it. An unannotated arrow is
                // pure. Labels are validated as declared effects elsewhere.
                let eff = match effect {
                    Some(row) => {
                        let mut e = match row.tail {
                            Some(tail) => {
                                let name = self.text(tail);
                                tvars.entry(name).or_insert_with(|| self.eng.fresh()).clone()
                            }
                            None => Type::RowEmpty,
                        };
                        for &label in row.names.iter() {
                            e = Type::row_extend(self.text(label), e);
                        }
                        e
                    }
                    None => Type::RowEmpty,
                };
                Type::arrow_eff(self.ty_of_ast(from, tvars), self.ty_of_ast(to, tvars), eff)
            }
            Ty::Unit => Type::con(ty::UNIT),
            Ty::Tuple(items) => {
                Type::Tuple(items.iter().map(|t| self.ty_of_ast(*t, tvars)).collect())
            }
            Ty::Record { fields, tail } => {
                // A record type: `{ x: A | r }` is open (row variable tail), `{ x: A,
                // y: B }` is closed (empty tail). Records are real, name-keyed types;
                // positional/scalar values promote to them at call sites.
                let rest = match tail {
                    Some(tvar) => {
                        let name = self.text(*tvar);
                        tvars.entry(name).or_insert_with(|| self.eng.fresh()).clone()
                    }
                    None => Type::RowEmpty,
                };
                let row = fields.iter().rev().fold(rest, |rest, f| {
                    Type::row_field(self.text(f.name), self.ty_of_ast(f.ty, tvars), rest)
                });
                Type::record(row)
            }
        }
    }

    // -- built-ins ----------------------------------------------------------

    fn install_builtins(&mut self) {
        let int = || Type::con(ty::INT);
        let real = || Type::con(ty::REAL);
        let bool_ = || Type::con(ty::BOOL);

        for op in ["+", "-", "*", "/", "%"] {
            self.overloads.insert(
                op,
                vec![
                    Cand::local(Type::arrow(int(), Type::arrow(int(), int()))),
                    Cand::local(Type::arrow(real(), Type::arrow(real(), real()))),
                ],
            );
        }
        self.overloads.insert(
            "neg",
            vec![
                Cand::local(Type::arrow(int(), int())),
                Cand::local(Type::arrow(real(), real())),
            ],
        );
        self.bind("not", Type::arrow(bool_(), bool_()));

        let prim = |mids: &[&str], returns_self: bool| {
            [ty::ARRAY, ty::STR].map(|recv| {
                let ret = if returns_self { recv } else { ty::INT };
                let params = std::iter::once(recv).chain(mids.iter().copied());
                Type::arrows(params.map(Type::con), Type::con(ret))
            })
        };
        self.overloads
            .insert("array_len", prim(&[], false).map(Cand::local).into());
        self.overloads
            .insert("array_get", prim(&[ty::INT], false).map(Cand::local).into());
        self.overloads
            .insert("array_push", prim(&[ty::INT], true).map(Cand::local).into());
        self.overloads.insert(
            "array_set",
            prim(&[ty::INT, ty::INT], true).map(Cand::local).into(),
        );
        self.overloads.insert(
            "array_slice",
            prim(&[ty::INT, ty::INT], true).map(Cand::local).into(),
        );

        let vec = |eng: &mut Engine| {
            let t = eng.fresh_generic();
            (t.clone(), Type::app(Type::con(ty::VEC), t))
        };
        let (_t, vt) = vec(&mut self.eng);
        self.bind("vec_new", Type::arrow(Type::con(ty::UNIT), vt));
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_fill", Type::arrow(int(), Type::arrow(t, vt)));
        let (_t, vt) = vec(&mut self.eng);
        self.bind("vec_len", Type::arrow(vt, int()));
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_get", Type::arrow(vt, Type::arrow(int(), t)));
        let (t, vt) = vec(&mut self.eng);
        self.bind(
            "vec_set",
            Type::arrow(vt.clone(), Type::arrow(int(), Type::arrow(t, vt))),
        );
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_push", Type::arrow(vt.clone(), Type::arrow(t, vt)));

        // `concat : [n]a -> [m]a -> [n+m]a`: the tensor size arithmetic (Phase B).
        {
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let m = self.eng.fresh_generic_nat();
            let tensor = |size: Type, elem: Type| {
                Type::app(Type::app(Type::con(TENSOR), size), elem)
            };
            let tn = tensor(n.clone(), a.clone());
            let tm = tensor(m.clone(), a.clone());
            let tnm = tensor(Type::NatAdd(Box::new(n), Box::new(m)), a);
            self.bind("concat", Type::arrow(tn, Type::arrow(tm, tnm)));
        }

        self.bind("true", bool_());
        self.bind("false", bool_());

        for op in ["?=", "?<", "?>", "<=", ">="] {
            let a = self.eng.fresh_generic();
            let t = Type::arrow(a.clone(), Type::arrow(a, bool_()));
            self.bind(op, t);
        }
        {
            let a = self.eng.fresh_generic();
            self.bind("++", Type::arrow(a.clone(), Type::arrow(a.clone(), a)));
        }
        {
            let a = self.eng.fresh_generic();
            let list = Type::app(Type::con(ty::LIST), a.clone());
            self.bind("::", Type::arrow(a, Type::arrow(list.clone(), list)));
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            self.bind(";", Type::arrow(a, Type::arrow(b.clone(), b)));
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            let e = self.eng.fresh_generic();
            let f = Type::arrow_eff(a.clone(), b.clone(), e.clone());
            self.bind("|>", Type::arrow(a, Type::arrow_eff(f, b, e)));
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            let e = self.eng.fresh_generic();
            let f = Type::arrow_eff(a, b, e);
            self.bind("<|", Type::arrow(f.clone(), f));
        }
    }
}

/// A global `$` definition, extracted from the program for dependency analysis.
#[derive(Clone)]
struct Def<'a> {
    name: &'a str,
    sig: Option<Aol<Ty>>,
    implicits: Vec<FieldDecl>,
    body: Aol<Expr>,
}

/// Build the reference graph over globals: `graph[i]` lists the definitions that
/// definition `i` refers to.
fn dependency_graph<'a>(
    ast: &'a Ast,
    defs: &[Def<'a>],
    index: &HashMap<&'a str, usize>,
) -> Vec<Vec<usize>> {
    defs.iter()
        .map(|def| {
            let mut out = Vec::new();
            let mut bound = Vec::new();
            free_globals(ast, def.body, index, &mut bound, &mut out);
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect()
}

/// Collect the indices of global definitions referenced by `e`, skipping any
/// reference that a local binder in `bound` shadows.
fn free_globals<'a>(
    ast: &'a Ast,
    e: Aol<Expr>,
    globals: &HashMap<&'a str, usize>,
    bound: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) {
    match ast.expr(e) {
        Expr::Var { module: None, name } => {
            let name = ast.text(*name);
            if !bound.contains(&name) {
                if let Some(&idx) = globals.get(name) {
                    out.push(idx);
                }
            }
        }
        Expr::Int(_)
        | Expr::Real(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Unit
        | Expr::Var { .. }
        | Expr::Extern { .. } => {}

        Expr::App(f, x) => {
            free_globals(ast, *f, globals, bound, out);
            free_globals(ast, *x, globals, bound, out);
        }
        Expr::Index { recv, index } => {
            free_globals(ast, *recv, globals, bound, out);
            free_globals(ast, *index, globals, bound, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            free_globals(ast, *lhs, globals, bound, out);
            free_globals(ast, *rhs, globals, bound, out);
        }
        Expr::UnOp { operand, .. } => free_globals(ast, *operand, globals, bound, out),
        Expr::Tuple(items) | Expr::List(items) => items
            .iter()
            .for_each(|e| free_globals(ast, *e, globals, bound, out)),
        Expr::Array { size } => free_globals(ast, *size, globals, bound, out),
        Expr::Field { record, .. } => free_globals(ast, *record, globals, bound, out),
        Expr::StructLit { fields, spread, .. } => {
            free_globals_field_inits(ast, fields, globals, bound, out);
            if let Some(s) = spread {
                free_globals(ast, *s, globals, bound, out);
            }
        }
        Expr::Record {
            fields,
            with,
            update,
        } => {
            free_globals_field_inits(ast, fields, globals, bound, out);
            for base in with.iter().chain(update.iter()) {
                free_globals(ast, *base, globals, bound, out);
            }
        }
        Expr::Variant { fields, .. } => free_globals_field_inits(ast, fields, globals, bound, out),

        Expr::Let { bindings, body } => {
            let mark = bound.len();
            for b in bindings.iter() {
                collect_pattern_binders(ast, b.pat, bound);
            }
            for b in bindings.iter() {
                free_globals(ast, b.value, globals, bound, out);
            }
            free_globals(ast, *body, globals, bound, out);
            bound.truncate(mark);
        }
        Expr::If { cond, then, alt } => {
            free_globals(ast, *cond, globals, bound, out);
            free_globals(ast, *then, globals, bound, out);
            free_globals(ast, *alt, globals, bound, out);
        }
        Expr::Match {
            scrut,
            arms,
            default,
        } => {
            free_globals(ast, *scrut, globals, bound, out);
            for arm in arms.iter() {
                let mark = bound.len();
                for pat in arm.patterns.iter() {
                    collect_pattern_binders(ast, *pat, bound);
                }
                if let Some(g) = arm.guard {
                    free_globals(ast, g, globals, bound, out);
                }
                free_globals(ast, arm.body, globals, bound, out);
                bound.truncate(mark);
            }
            if let Some(d) = default {
                free_globals(ast, *d, globals, bound, out);
            }
        }
        Expr::Lambda { params, body } => {
            let mark = bound.len();
            for p in params.iter() {
                collect_pattern_binders(ast, *p, bound);
            }
            free_globals(ast, *body, globals, bound, out);
            bound.truncate(mark);
        }
        Expr::With { subject, body } => {
            free_globals(ast, *subject, globals, bound, out);
            free_globals(ast, *body, globals, bound, out);
        }
        Expr::Handle { body, handler } => {
            free_globals(ast, *body, globals, bound, out);
            if let Some(h) = handler {
                for clause in h.clauses.iter() {
                    let mark = bound.len();
                    bound.push(ast.text(clause.arg));
                    bound.push(ast.text(h.continuation));
                    free_globals(ast, clause.body, globals, bound, out);
                    bound.truncate(mark);
                }
                if let Some((name, else_body)) = &h.default {
                    let mark = bound.len();
                    bound.push(ast.text(*name));
                    free_globals(ast, *else_body, globals, bound, out);
                    bound.truncate(mark);
                }
            }
        }
        Expr::Defer { cleanup, body } => {
            free_globals(ast, *cleanup, globals, bound, out);
            free_globals(ast, *body, globals, bound, out);
        }
        Expr::Ctx {
            callee, overrides, ..
        } => {
            free_globals(ast, *callee, globals, bound, out);
            free_globals_field_inits(ast, overrides, globals, bound, out);
        }
    }
}

fn free_globals_field_inits<'a>(
    ast: &'a Ast,
    fields: &[FieldInit],
    globals: &HashMap<&'a str, usize>,
    bound: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) {
    for f in fields {
        match f {
            FieldInit::Named { value, .. } => free_globals(ast, *value, globals, bound, out),
            FieldInit::Positional(v) => free_globals(ast, *v, globals, bound, out),
        }
    }
}

/// Push every name a pattern binds onto `bound`.
fn collect_pattern_binders<'a>(ast: &'a Ast, pat: Aol<Pattern>, bound: &mut Vec<&'a str>) {
    match ast.pat(pat) {
        Pattern::Var(name) => bound.push(ast.text(*name)),
        Pattern::StrPrefix { rest, .. } => collect_pattern_binders(ast, *rest, bound),
        Pattern::Cons { head, tail } => {
            collect_pattern_binders(ast, *head, bound);
            collect_pattern_binders(ast, *tail, bound);
        }
        Pattern::List { elems, rest } => {
            elems
                .iter()
                .for_each(|p| collect_pattern_binders(ast, *p, bound));
            if let Some(r) = rest {
                collect_pattern_binders(ast, *r, bound);
            }
        }
        Pattern::Tuple(pats) => pats
            .iter()
            .for_each(|p| collect_pattern_binders(ast, *p, bound)),
        Pattern::Struct { fields, .. } | Pattern::Variant { fields, .. } => {
            for f in fields.iter() {
                match f {
                    FieldPat::Named { pat, .. } => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Positional(pat) => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Shorthand(name) => bound.push(ast.text(*name)),
                }
            }
        }
        Pattern::Record { fields, rest } => {
            for f in fields.iter() {
                match f {
                    FieldPat::Named { pat, .. } => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Positional(pat) => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Shorthand(name) => bound.push(ast.text(*name)),
                }
            }
            if let Some(r) = rest {
                collect_pattern_binders(ast, *r, bound);
            }
        }
        Pattern::Range { .. }
        | Pattern::Wild
        | Pattern::Int(_)
        | Pattern::Real(_)
        | Pattern::Str(_)
        | Pattern::Bool(_) => {}
    }
}

fn applied(name: &str, args: &[Type]) -> Type {
    args.iter()
        .fold(Type::con(name), |acc, a| Type::app(acc, a.clone()))
}

/// The mangled global name for one overload: `name#<type-key>`. Two overloads of
/// one name in one module get distinct keys, so their globals no longer collide
/// under a single `MOD.name`. The definition side and each use site both derive
/// the key from the candidate's type, so they agree. Effect rows are omitted (a
/// pair of overloads never differs only by effect), which also keeps the key free
/// of the noisy row variables that would otherwise vary between the two sides.
fn overload_key(name: &str, ty: &Type) -> String {
    let mut vars = Vec::new();
    format!("{name}#{}", ty_key(ty, &mut vars))
}

/// A structural, effect-free string for `ty` with variables canonicalized to
/// `t0`, `t1`, ... by first appearance, so structurally equal schemes (however
/// their variables happen to be numbered) produce the same string. `.` is
/// replaced so the key survives the runtime's split-on-`.` bare-name fallback.
fn ty_key(ty: &Type, vars: &mut Vec<VarId>) -> String {
    match ty {
        Type::Var(id) => {
            let i = vars.iter().position(|v| v == id).unwrap_or_else(|| {
                vars.push(*id);
                vars.len() - 1
            });
            format!("t{i}")
        }
        Type::Con(name) => name.replace('.', "_"),
        Type::Nat(n) => format!("N{n}"),
        Type::NatAdd(a, b) => format!("P{}_{}", ty_key(a, vars), ty_key(b, vars)),
        Type::NatMul(a, b) => format!("M{}_{}", ty_key(a, vars), ty_key(b, vars)),
        Type::App(head, arg) => {
            format!("A{}_{}", ty_key(head, vars), ty_key(arg, vars))
        }
        Type::Arrow(from, to, _) => {
            format!("F{}_{}", ty_key(from, vars), ty_key(to, vars))
        }
        Type::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(|t| ty_key(t, vars)).collect();
            format!("T{}", parts.join("_"))
        }
        Type::RowEmpty => "R".to_string(),
        Type::RowExtend(label, rest) => {
            format!("R{}_{}", label.replace('.', "_"), ty_key(rest, vars))
        }
        Type::Record(row) => format!("D{}", ty_key(row, vars)),
        Type::RowField(label, ty, rest) => format!(
            "{}:{}_{}",
            label.replace('.', "_"),
            ty_key(ty, vars),
            ty_key(rest, vars)
        ),
    }
}

/// Flatten a zonked arrow `A -> B -> ... -> R` into its argument marshalling
/// names and the result name.
fn flatten_extern(ty: &Type) -> (Vec<String>, String) {
    let mut args = Vec::new();
    let mut cur = ty;
    while let Type::Arrow(from, to, _) = cur {
        args.push(marshal_name(from));
        cur = to;
    }
    (args, marshal_name(cur))
}

/// A type's marshalling name for the FFI seam. A type variable or any composite
/// (the checker's fallback, matching the C++ `desc_of`) marshals word-sized, so
/// the backends read it as `Int`.
fn marshal_name(ty: &Type) -> String {
    match ty {
        Type::Con(name) => name.clone(),
        Type::Tuple(items) if items.is_empty() => ty::UNIT.to_string(),
        _ => ty::INT.to_string(),
    }
}

fn subst_from_args<'a>(
    params: &[&'a str],
    args: &[Type],
    eng: &mut Engine,
) -> HashMap<&'a str, Type> {
    let mut subst = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        let a = args.get(i).cloned().unwrap_or_else(|| eng.fresh());
        subst.insert(*p, a);
    }
    subst
}

/// Collect the type variables appearing in `ty`, in order of first appearance.
fn collect_tyvars<'a>(ast: &'a Ast, ty: Aol<Ty>, out: &mut Vec<&'a str>) {
    match ast.ty(ty) {
        Ty::Var(name) => {
            let name = ast.text(*name);
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ty::App(a, b) => {
            collect_tyvars(ast, *a, out);
            collect_tyvars(ast, *b, out);
        }
        Ty::Arrow { from, to, .. } => {
            collect_tyvars(ast, *from, out);
            collect_tyvars(ast, *to, out);
        }
        Ty::Tuple(items) => items.iter().for_each(|t| collect_tyvars(ast, *t, out)),
        Ty::Record { fields, tail } => {
            fields.iter().for_each(|f| collect_tyvars(ast, f.ty, out));
            if let Some(t) = tail {
                let name = ast.text(*t);
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        Ty::Sized { size, elem } => {
            collect_tyvars(ast, *size, out);
            collect_tyvars(ast, *elem, out);
        }
        Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_tyvars(ast, *a, out);
            collect_tyvars(ast, *b, out);
        }
        Ty::Con { .. } | Ty::Nat(_) | Ty::Unit => {}
    }
}

/// Normalize a variant payload into `(optional-name, type-handle)` pairs.
fn payload_fields<'a>(ast: &'a Ast, p: &Payload) -> Vec<(Option<&'a str>, Aol<Ty>)> {
    match p {
        Payload::None => vec![],
        Payload::Bare(ty) => vec![(None, *ty)],
        Payload::Fields(fs) => fs
            .iter()
            .map(|f| (f.name.map(|n| ast.text(n)), f.ty))
            .collect(),
    }
}

/// Select a payload field's type by name (if named) or by position.
fn variant_field_ty(payload: &VariantPayload, name: Option<&str>, index: usize) -> Option<Type> {
    match name {
        Some(name) => payload
            .iter()
            .find(|(n, _)| *n == Some(name))
            .map(|(_, t)| t.clone()),
        None => payload.get(index).map(|(_, t)| t.clone()),
    }
}

/// Map the sigil/alias type constructors to their canonical built-in name.
fn canonical_con(name: &str) -> &str {
    match name {
        "@int64" | "@int32" | "Nat" => ty::INT,
        "@float64" | "@float32" => ty::REAL,
        "@str" => ty::STR,
        "@bool" => ty::BOOL,
        other => other,
    }
}

/// A type that would receive a `{kind}` (`"field"`/`"variant"`) twice: one it
/// declares and one a `with` splice copies in, or one two included types share.
fn dup_member(ty: &str, member: &str, kind: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "type `{ty}` gets a duplicate {kind} `{member}` from a `with` include"
    )
}

fn unbound(name: &str) -> Diagnostic {
    diag!(Code::TypeUnbound, Span::at(0), 0, "unbound name `{name}`")
}

/// The built-in scalar and container type names, in both their friendly and
/// `@`-sigil spellings. A type in source is one of these, a declared type, or a
/// lowercase type variable.
fn is_base_type(name: &str) -> bool {
    matches!(
        name,
        "Int" | "Nat" | "Real"
            | "Int8" | "Int16" | "Int32" | "Int64"
            | "Nat8" | "Nat16" | "Nat32" | "Nat64"
            | "Real32" | "Real64"
            | "Str" | "Ptr" | "Bool"
            | "Array" | "Vec" | "List"
            | "@int8" | "@int16" | "@int32" | "@int64"
            | "@nat8" | "@nat16" | "@nat32" | "@nat64"
            | "@float32" | "@float64"
            | "@str" | "@ptr" | "@bool" | "@array"
    )
}

/// The internal constructor name of a sized tensor `[n]T` (`@tensor size elem`).
/// `@`-prefixed so it cannot collide with a user type; erased before runtime.
const TENSOR: &str = "@tensor";

fn unknown_type(name: &str) -> Diagnostic {
    diag!(
        Code::TypeUnbound, Span::at(0), 0, "unknown type `{name}`";
        note: "a type variable is written as a lowercase name; a capitalized type must be declared"
    )
}

/// A type declaration (or `with` splice) uses a type variable it does not declare.
/// Parameters are mandatory, so this reports the fix: list `v` after the keyword.
fn undeclared_param(kind: &str, name: &str, v: &str, declared: &[&str], spliced: bool) -> Diagnostic {
    let how = if spliced {
        format!("`{name}` splices in a field using the type variable `{v}`")
    } else {
        format!("`{name}` uses the type variable `{v}`")
    };
    let msg = if declared.is_empty() {
        format!("{how}, but `{name}` declares no type parameters")
    } else {
        format!(
            "{how}, which is not one of `{name}`'s declared parameters `{}`",
            declared.join(" ")
        )
    };
    Diagnostic::error(Code::TypeUnbound, Span::at(0), 0, msg).with_note(format!(
        "type parameters are mandatory; declare every one after the keyword, e.g. `@{kind} {v} = ...`"
    ))
}
