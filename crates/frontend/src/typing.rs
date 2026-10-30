//! The type checker: Hindley-Milner inference over the handle-based AST.
//!
//! A [`Checker`] checks one module. [`Checker::check_program`] registers the
//! module's type and effect declarations, imports what other modules export,
//! then groups the global definitions into strongly connected components of
//! the reference graph (Tarjan, see [`utilities::tarjan`]) and checks them in
//! dependency order. Members of one component share monomorphic types while
//! their bodies are inferred, so self and mutual recursion resolve; the
//! component is generalized before the components that depend on it.
//!
//! Inference is Algorithm W, bidirectional (`algorithm_w`), over the
//! [`Unifier`] ([`unify`]). A name has one definition; polymorphism over types
//! goes through interfaces, whose `@ctx` arguments are resolved by type
//! (`instances`). Matches are checked for exhaustiveness ([`usefulness`]).
//! What desugaring needs from all this is in [`resolved`]. The other private
//! modules each hold one concern of the [`Checker`]: `declarations`,
//! `signatures` (written types), `patterns`, `imports` and `externs`.
//!
//! The AST is read through a borrowed [`Ast`]: `Checker::node`, `tnode` and
//! `pnode` resolve handles to `'a`-lived data, independent of the `&mut self`
//! borrow, so a node can be read and its children inferred in the same method.
//! Names stay [`StrId`]s throughout: the checker's tables are keyed by them, and
//! a name becomes text only when a diagnostic or a printed type renders it.
//!
//! Input: one parsed module, plus the checkers of its imports. Output: each
//! global's generalized type, diagnostics, and the side tables in
//! [`resolved`].
//!
//! Entry point: [`Checker::check_program`].

mod algorithm_w;
mod declarations;
mod externs;
mod imports;
mod instances;
mod patterns;
pub mod resolved;
mod signatures;
pub mod types;
pub mod unify;
pub mod usefulness;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::rc::Rc;

use crate::parser::data::{
    Ast, Binding, EffectRow, Expr, FieldInit, FieldPat, Item, Pattern, Payload, Program, RecField,
    RowEntry, SliceSlot, Ty,
};
use utilities::{Aol, Slice, StrId};
use utilities::{diag, Applicability, Code, Diagnostic, Edit, ExternArg, Result, Span};

use crate::typing::types::{self as ty, Type, TypeNode, Types, VarId};
use crate::typing::unify::Unifier;

use algorithm_w::*;
use declarations::*;
use imports::*;
pub use resolved::{collect_resolved, CtxVal, Declarations, ExternSpec, HookImpl, LazySlots, ReflectInfo, Resolved};
pub use types::BASE_TYPES;
use types::*;

/// A declared struct type. `params` are the implicit type parameters (the type
/// variables appearing in the fields, in order of first appearance); `fields`
/// keeps declaration order (which is also the positional-constructor order).
/// `crepr` is set for a C-layout foreign struct (`@struct @extern "abi"`): its
/// runtime value is a flat, unboxed C struct rather than a boxed record, and it
/// may cross the `@extern` boundary by value.
#[derive(Clone)]
struct StructInfo {
    params: Vec<StrId>,
    fields: Vec<(StrId, Aol<Ty>)>,
    crepr: bool,
    /// A C `union` (`@union @extern`): members share offset 0. Handled as a C-repr
    /// struct otherwise; only its layout and single-member construction differ.
    c_union: bool,
}

/// Pending `with Other` splices, keyed by the type that wrote them: whether it
/// is a struct (rather than a union), and each included name with the span it
/// was written at, so a bad include carets its own `with`.
type PendingIncludes = HashMap<StrId, (bool, Vec<(StrId, Span)>)>;

/// A declared union type: implicit `params` and one [`VariantSig`] per variant.
#[derive(Clone)]
struct UnionInfo {
    params: Vec<StrId>,
    variants: Vec<VariantSig>,
}

/// A union variant: its tag and its (normalized) payload fields, each an optional
/// name and its declared type.
#[derive(Clone)]
struct VariantSig {
    tag: StrId,
    payload: Vec<(Option<StrId>, Aol<Ty>)>,
}

/// A variant's payload instantiated to concrete types: one `(optional-name,
/// type)` pair per field, in declaration order.
type VariantPayload = Vec<(Option<StrId>, Type)>;

pub struct Checker<'a> {
    ast: &'a Ast,
    unifier: Unifier,
    scopes: Vec<HashMap<StrId, Type>>,
    structs: HashMap<StrId, StructInfo>,
    unions: HashMap<StrId, UnionInfo>,
    /// Which slots of a declared type hold their value lazily, keyed by
    /// `(type, Some(variant tag))` for a union and `(type, None)` for a struct,
    /// one flag per slot in declaration order. See [`Self::compute_lazy_slots`].
    lazy_slots: LazySlots,
    /// Computed C memory layout for each `@struct @extern "abi"` (C-repr) struct,
    /// keyed by type name. Used to marshal a struct value across the `@extern`
    /// boundary by value.
    crepr_layouts: HashMap<StrId, utilities::CLayout>,
    /// Type aliases: `name -> (declared params, body)`. An applied alias is
    /// expanded by substituting its arguments for the parameters in the body.
    aliases: HashMap<StrId, (Vec<StrId>, Aol<Ty>)>,
    /// Each declared effect's operations, `effect -> op -> its `Arg -> Res`
    /// scheme. Used to type a handler clause head, which cannot be resolved by
    /// inference alone.
    effect_ops: HashMap<StrId, HashMap<StrId, Type>>,
    /// The same for effects imported from other modules, consulted after
    /// `effect_ops` so a handler of an imported effect types its clauses too.
    imported_effect_ops: HashMap<StrId, HashMap<StrId, Type>>,
    /// How many type parameters each visible effect declares.
    effect_arity: HashMap<StrId, usize>,
    /// While an effect's operations are elaborated: that effect and its
    /// parameters, which a bare mention of it in an operation type stands for.
    effect_self: Option<(StrId, Type)>,
    /// Operation names several EFFECTS declare, mapped to one candidate per effect.
    /// The only argument-type-driven selection left (see [`Cand`]).
    overloads: HashMap<StrId, Vec<Cand>>,
    /// Effect-operation uses that were ambiguous when first seen. Solved to a
    /// fixpoint at each definition boundary.
    pending: Vec<Pending>,
    /// Names this module defines itself, so a use of one is NOT rewritten to an
    /// imported module's copy.
    local_defs: HashSet<StrId>,
    /// Single imported values, `name -> module`, so a bare use lowers to the
    /// owning module even when another loaded module defines the same name.
    value_module: HashMap<StrId, StrId>,
    /// Names more than one import brings in, mapped to the modules that export
    /// them. A bare use is an error naming the owners; `Module.name` still works.
    ambiguous_imports: HashMap<StrId, Vec<StrId>>,
    /// Bare-call sites resolved to a specific module. Lowering rewrites the
    /// referenced `Expr::Var` to `MOD.name`.
    resolved_calls: HashMap<Aol<Expr>, StrId>,
    /// The context parameter type of each context-bearing definition in this
    /// module, sharing its variables with the exposed type, so a printed signature
    /// can show the `@ctx` prefix.
    decl_ctx: HashMap<StrId, Type>,
    /// Top-level definitions whose signature opens with a `@ctx` context
    /// parameter, by name, as the signature's AST handle (the context type is its
    /// first `from`). A use site plans the context from it and records the resolved
    /// value in [`Self::ctx_args`]; lowering injects it as a leading argument.
    /// Populated up front (own module) so resolution is order-independent, and
    /// extended from imports.
    global_ctx: HashMap<StrId, Aol<Ty>>,
    /// The same, keyed by `(module, name)` so a QUALIFIED use (`MOD.f`) plans its
    /// context too (the bare-keyed map collides when two modules define the name).
    qualified_ctx: HashMap<(StrId, StrId), Aol<Ty>>,
    /// This module's own context-bearing definitions, re-exported to importers.
    own_ctx: Vec<(StrId, Aol<Ty>)>,
    /// This module's interface wrappers: a context-bearing definition whose body
    /// only projects one field of its context, mapped to that field (see
    /// [`instances::wrapper_field`]).
    own_wrappers: HashMap<StrId, StrId>,
    /// Each use site of a context-bearing function, mapped to the resolved context
    /// value lowering injects ahead of the explicit arguments.
    ctx_args: HashMap<Aol<Expr>, CtxVal>,
    /// Context resolutions deferred to the enclosing definition's boundary, where
    /// inference has pinned the requirement's type variables.
    ctx_pending: Vec<PendingCtx>,
    /// The elaborated signature of the definition currently being checked. Its type
    /// variables are the caller's to choose, so context resolution may not bind
    /// them: a `$ g : a -> @Str = \x = show x` must declare the context rather than
    /// have `a` silently specialized to the one instance that happens to exist. The
    /// signature is kept as a type (not a variable set) because unification may
    /// point its variables at others, and zonking at resolution time follows that.
    current_sig: Option<Type>,
    /// Explicit `(@ctx e)` arguments, keyed by the head reference site they apply
    /// to. Consumed when that reference plans its context.
    ctx_override: HashMap<Aol<Expr>, Aol<Expr>>,
    /// Every global whose type's head is a declared nominal type, keyed by that
    /// head's name: the candidates a context requirement searches. Implementing an
    /// interface is just defining a value of its type, so there is nothing to mark.
    instances: HashMap<StrId, Vec<Inst>>,
    /// This module's own instances, re-exported to importers.
    own_instances: Vec<(StrId, Inst)>,
    /// Types with unresolved `with Other` splices, `name -> (is_struct, includes)`.
    /// Drained as each type's members are copied in (see `splice_includes`). This
    /// is a declaration-time convenience only; no type relationship is recorded.
    pending_includes: PendingIncludes,
    /// Type variables introduced by integer literals, which may be Int or Real;
    /// leftovers default to Int at the definition boundary. Each carries the
    /// literal's source span so a defaulting error can point at it.
    numeric: Vec<(Type, Span)>,
    /// This module's own exports, recorded after checking.
    own_values: Vec<(StrId, Type)>,
    own_type_names: Vec<StrId>,
    /// Types this module reaches through an import's declarations without
    /// importing their home module (`BB.Outer`'s field of `AA.Inner`), each with
    /// that home. They are in `structs`/`unions`/`aliases` so imported
    /// declarations elaborate, but a type this module writes cannot name them.
    hidden_types: HashMap<StrId, StrId>,
    /// The module that declares each type in scope, to name the import a use of
    /// a hidden type is missing.
    type_home: HashMap<StrId, StrId>,
    /// Every type node of an imported declaration (struct fields, variant
    /// payloads, alias bodies, context signatures). Elaborating one of these
    /// may name a hidden type.
    foreign_tys: HashSet<Aol<Ty>>,
    /// The effects this module declares, exported so an importer can type a
    /// qualified `Effect.op` reference.
    own_effects: Vec<StrId>,
    /// Names declared after a `$ @private` marker: defined and usable within this
    /// module, but not exported, so no importer can bind or qualify them.
    private_names: HashSet<StrId>,
    /// Private names of the modules this one imports, `module -> names`. A
    /// qualified reference (`A.helper`) to one is rejected rather than deferred to
    /// the runtime, which would otherwise reach the still-present global.
    imported_private: HashMap<StrId, HashSet<StrId>>,
    /// Value schemes pulled in from imports (with their source module), finalized
    /// once.
    imported: HashMap<StrId, Vec<(Type, StrId)>>,
    /// Imported names reachable qualified as `MOD.name` (or `ALIAS.name`).
    qualified: HashMap<StrId, HashMap<StrId, Vec<Type>>>,
    /// Each import alias in scope (`$ with V = VEC`), mapped to its module.
    alias_modules: HashMap<StrId, StrId>,
    /// Values an aliased-only import exports, mapped to the aliases that reach
    /// them, so a bare use can name the qualified form.
    alias_values: HashMap<StrId, Vec<StrId>>,
    /// Types an aliased-only import exports, mapped to the alias that reaches
    /// them. Like a hidden type, a bare mention is an error; a qualified one
    /// (`V.Type`) is not.
    alias_types: HashMap<StrId, StrId>,
    module_name: StrId,
    /// `[..]` literal sites resolved to a sized tensor `[n]T`: the one `[...]`
    /// shape no hook can build, since the payload cannot carry the static size.
    /// Lowering flattens the collected elements with `@tensor_stack`.
    tensor_exprs: HashSet<Aol<Expr>>,
    /// Argument sites promoted to a record: a bare scalar `1` or a positional
    /// `{1, 2}` passed where a record is expected, mapped to the target record's
    /// field names (in order). Lowering wraps the value into a name-keyed record.
    promotions: HashMap<Aol<Expr>, Vec<StrId>>,
    /// `.{ ... }` struct-literal sites, mapped to the nominal struct name the
    /// checker resolved them to (from a type annotation / expected type, or the
    /// field set). Lowering reads this so a positional literal gets the right field
    /// names instead of falling back to positional indices.
    struct_lit_names: HashMap<Aol<Expr>, StrId>,
    /// Literal sites (`"..."`, `[..]`, an int, a real) that a blessed `@I*`
    /// construction interface builds into a user type instead of the built-in default,
    /// mapped to the resolved hook's `(owning module, emitted name)`. Lowering wraps
    /// the raw payload term in a call to this hook; an unrecorded literal folds to the
    /// plain built-in constant (no hook, no conversion).
    literal_hooks: HashMap<Aol<Expr>, HookImpl>,
    /// Blessed-interface VALUE sites (the `@IIndex` / `@IImagLit` head a desugar
    /// emits), mapped to the single field lowering projects off the instance that
    /// `ctx_args` records for the same site.
    blessed_sites: HashMap<Aol<Expr>, StrId>,
    /// Literal PATTERN sites (`is "foo"`, `is 42`) whose scrutinee is a user type,
    /// mapped to the resolved `(build hook, equality hook)`: the pattern matches by
    /// building the literal into the user type (build hook) and comparing it against
    /// the scrutinee (equality hook), instead of the built-in `==` on a primitive.
    literal_pattern_hooks: HashMap<Aol<Pattern>, (HookImpl, HookImpl)>,
    /// Sequence PATTERN pieces (`is [a, b]`, the `[h]` of `h :: t`, the `[z]` of
    /// `r ++ [z]`), mapped to the resolved view: `@ISeqView` for a piece matched
    /// from the front, `@ISeqViewBack` for one matched from the back. Lowering
    /// unfolds the piece through it.
    sequence_pattern_hooks: HashMap<Aol<Pattern>, HookImpl>,
    /// Sequence patterns on a sized tensor, mapped to the tensor's (static) size.
    /// Lengths are checked here, so lowering reads elements and slices directly.
    tensor_patterns: HashMap<Aol<Pattern>, u64>,
    /// The union each variant pattern resolved to, so the exhaustiveness check
    /// reads the same union a bare `.Tag` was typed against.
    variant_pattern_unions: HashMap<Aol<Pattern>, StrId>,
    /// The scrutinee type of each integer literal and range pattern, read once the
    /// match is typed to give the exhaustiveness check the type's value range.
    int_pattern_tys: HashMap<Aol<Pattern>, Type>,
    /// Each `@cast` application under 64 bits, mapped to its target's signedness
    /// and width. Lowering wraps the operand into that range.
    cast_widths: HashMap<Aol<Expr>, (bool, u32)>,
    /// Every integer literal's type, value, and span, in an expression or a
    /// pattern, range-checked once the module is solved (see [`Self::check_int_literals`]).
    int_literals: Vec<(Type, i64, Span)>,
    /// Non-fatal diagnostics (unreachable match arms), in source order.
    warnings: Vec<Diagnostic>,
    /// Errors the check recovered from, in the order met: a definition that fails
    /// is recorded here, given the error type, and checking moves on to the next.
    errors: Vec<Diagnostic>,
    /// Names of items the parser could not read. A use of one is not reported
    /// again: it gets the error type.
    broken: HashSet<StrId>,
    /// Where a missing `$ with M` goes, and the text around the module name:
    /// after the last import, or before the first item.
    import_anchor: Option<(usize, &'static str, &'static str)>,
    /// Modules the driver could import, by the value names they define, for an
    /// "import it" fix on an unbound name.
    import_hints: Rc<HashMap<StrId, Vec<StrId>>>,
    /// Every `???` met, with its type and the binders in scope there (the
    /// candidates for filling it), reported once the definition is solved.
    holes: Vec<(Aol<Expr>, Type, Vec<(StrId, Type)>)>,
    /// Each expression's inferred type, kept only when an editor asks
    /// ([`Self::set_record_types`]): hover reads it after the check.
    expr_types: Option<HashMap<Aol<Expr>, Type>>,
    /// The ordered field names each `with subject in body` brings into scope,
    /// keyed by the `With` node. Lowering desugars `with` into a `let` per field,
    /// so the Core has no name-binding-by-type node and stays De-Bruijn indexable.
    with_fields: HashMap<Aol<Expr>, Vec<StrId>>,
    /// Each `@extern` node's type variable, zonked after solving to recover the
    /// concrete arrow the declaration constrained it to. Lowering reads the
    /// flattened arg/result marshalling names off this.
    extern_tys: HashMap<Aol<Expr>, Type>,
    /// Each `@extern` node's marshalling spec, built from `extern_tys` once the
    /// program is solved (see `build_extern_specs`): how the single applied Thrax
    /// value maps to positional C arguments, the flattened C argument type names,
    /// and the result type name.
    extern_specs: HashMap<Aol<Expr>, ExternSpec>,
    /// This module's own foreign functions, keyed by the global name bound to the
    /// bare `@extern`. Aggregated with every dependency's `own_externs` (keyed by
    /// owner module) so lowering can flatten a foreign call it did not itself
    /// resolve across modules.
    own_externs: HashMap<StrId, ExternSpec>,
    /// This module's `@extern "thrax"` definitions, each named for the runtime
    /// builtin it declares.
    own_builtins: HashSet<StrId>,
    /// The builtins imported modules declare, which no definition may reuse.
    imported_builtins: HashSet<StrId>,
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
    /// Set during a metaprogram-expansion round (`$ @e` codegen not yet run): an
    /// unbound value name is treated as a fresh type variable rather than an error,
    /// so a module that forward-references an about-to-be-injected definition still
    /// checks well enough to run its generators. The final round checks strictly.
    lenient: bool,
    /// Set for the interactive shell: a top-level body is checked under an open
    /// ambient effect row rather than the pure closed row, so an entered expression
    /// may perform effects (`println`, file IO) the way a `main` body can. A
    /// batch-compiled file leaves this off, so its top level stays pure.
    open_effects: bool,
    /// The one global allowed to omit its signature: the binding a tool (the shell,
    /// `@eval`) wraps around an expression to learn its type. Never set for a file.
    inferred: Option<StrId>,
}

/// One candidate of an operation name that several EFFECTS declare (`ask` in both
/// `Reader` and `Config`). This is the one place left that picks a definition from
/// argument types: values are never overloaded, a name has one definition.
#[derive(Clone)]
struct Cand {
    ty: Type,
    module: Option<StrId>,
}

impl Cand {
    /// An effect-operation candidate, owned by no module (never rewritten to a
    /// qualified reference).
    fn local(ty: Type) -> Cand {
        Cand { ty, module: None }
    }
}

/// One candidate value for a context requirement: a global whose type's head is
/// the requirement's head constructor. `bundle` packs its exposed type and, when
/// the instance itself takes a context, that requirement, generalized together so
/// instantiating the bundle keeps their variables aligned.
#[derive(Clone)]
struct Inst {
    /// The global's name.
    name: StrId,
    module: Option<StrId>,
    bundle: Type,
    has_ctx: bool,
}

/// One binder in scope that could satisfy a context requirement. `index` is set
/// when the binder holds a BUNDLE of contexts and this is one component of it, so
/// the resolved value is `name.index`.
#[derive(Clone)]
struct CtxLocal {
    depth: usize,
    name: StrId,
    ty: Type,
    index: Option<usize>,
}

/// A context resolution deferred to the enclosing definition's boundary, where the
/// requirement's type variables are pinned.
struct PendingCtx {
    site: Aol<Expr>,
    fname: StrId,
    req: Type,
    /// The enclosing definition's signature, whose variables resolution may not
    /// bind (zonked at resolution time, so a variable chain is followed).
    sig: Option<Type>,
    /// Binders in scope at the use site that could satisfy the requirement. Captured
    /// there because the scopes are gone by the time the boundary resolves.
    locals: Vec<CtxLocal>,
}

/// A deferred overload use: its candidate set, the argument types, the fresh
/// result variable standing in for the (not-yet-known) result, and the call site
/// to annotate once it resolves.
struct Pending {
    name: StrId,
    candidates: Vec<Cand>,
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

// ## Construction

impl<'a> Checker<'a> {
    pub fn new(ast: &'a Ast, types: Rc<Types>) -> Checker<'a> {
        let unifier = Unifier::new(types);
        let ambient = unifier.types.row_empty();
        let mut c = Checker {
            ast,
            unifier,
            scopes: vec![HashMap::new()],
            structs: HashMap::new(),
            unions: HashMap::new(),
            lazy_slots: HashMap::new(),
            crepr_layouts: HashMap::new(),
            aliases: HashMap::new(),
            effect_ops: HashMap::new(),
            imported_effect_ops: HashMap::new(),
            effect_arity: HashMap::new(),
            effect_self: None,
            overloads: HashMap::new(),
            pending: Vec::new(),
            local_defs: HashSet::new(),
            value_module: HashMap::new(),
            ambiguous_imports: HashMap::new(),
            resolved_calls: HashMap::new(),
            decl_ctx: HashMap::new(),
            global_ctx: HashMap::new(),
            qualified_ctx: HashMap::new(),
            own_ctx: Vec::new(),
            own_wrappers: HashMap::new(),
            ctx_args: HashMap::new(),
            ctx_pending: Vec::new(),
            current_sig: None,
            ctx_override: HashMap::new(),
            instances: HashMap::new(),
            own_instances: Vec::new(),
            pending_includes: HashMap::new(),
            numeric: Vec::new(),
            own_values: Vec::new(),
            own_type_names: Vec::new(),
            hidden_types: HashMap::new(),
            type_home: HashMap::new(),
            foreign_tys: HashSet::new(),
            own_effects: Vec::new(),
            private_names: HashSet::new(),
            imported_private: HashMap::new(),
            imported: HashMap::new(),
            qualified: HashMap::new(),
            alias_modules: HashMap::new(),
            alias_values: HashMap::new(),
            alias_types: HashMap::new(),
            module_name: StrId::intern(""),
            tensor_exprs: HashSet::new(),
            promotions: HashMap::new(),
            struct_lit_names: HashMap::new(),
            literal_hooks: HashMap::new(),
            blessed_sites: HashMap::new(),
            literal_pattern_hooks: HashMap::new(),
            sequence_pattern_hooks: HashMap::new(),
            tensor_patterns: HashMap::new(),
            variant_pattern_unions: HashMap::new(),
            int_pattern_tys: HashMap::new(),
            cast_widths: HashMap::new(),
            int_literals: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            broken: HashSet::new(),
            import_anchor: None,
            import_hints: Rc::new(HashMap::new()),
            expr_types: None,
            holes: Vec::new(),
            with_fields: HashMap::new(),
            extern_tys: HashMap::new(),
            extern_specs: HashMap::new(),
            own_externs: HashMap::new(),
            own_builtins: HashSet::new(),
            imported_builtins: HashSet::new(),
            ambient,
            unknown_type: None,
            lenient: false,
            open_effects: false,
            inferred: None,
        };
        c.install_builtins();
        c
    }

    /// Enable lenient checking for a metaprogram-expansion round: an unbound value
    /// name becomes a fresh variable instead of an error, so a module that
    /// forward-references an about-to-be-injected definition still type-checks
    /// enough to run its `$ @e` generators. The final round leaves this off.
    pub fn set_lenient(&mut self, lenient: bool) {
        self.lenient = lenient;
    }

    /// Check top-level bodies under an open ambient effect row (see `open_effects`).
    /// The interactive shell turns this on so an entered expression may perform IO.
    pub fn set_open_effects(&mut self, open: bool) {
        self.open_effects = open;
    }

    /// Let the global `name` omit its signature (see `inferred`).
    pub fn set_inferred(&mut self, name: &'static str) {
        self.inferred = Some(StrId::intern(name));
    }

    /// Supply the module names an unbound name may suggest importing.
    pub fn set_import_hints(&mut self, hints: Rc<HashMap<StrId, Vec<StrId>>>) {
        self.import_hints = hints;
    }

    /// Keep every expression's type for [`Self::expr_type`].
    pub fn set_record_types(&mut self, on: bool) {
        self.expr_types = on.then(HashMap::new);
    }
}

// ## AST access

impl<'a> Checker<'a> {
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
}

// ## Checking a program

impl<'a> Checker<'a> {
    /// Check a whole program, returning the inferred (generalized) type of every
    /// global definition, in source order.
    pub fn check_program(&mut self, program: &Program) -> Result<Vec<(StrId, Type)>> {
        self.module_name = program.module;
        self.register_target_namespace();
        self.collect_private_names(program);
        self.check_import_aliases(program);
        self.finalize_imports();
        self.register_types(program)?;
        self.register_row_folds();
        self.validate_crepr_structs()?;
        // Row registration elaborates struct field types only to cache their rows
        // for the bridge; it must not report type errors (a field may reference a
        // type not imported into this module, e.g. a re-exported struct's internals).
        // Real unknown-type errors are still caught when definitions are elaborated.
        let saved_unknown = self.unknown_type.take();
        self.register_struct_rows();
        self.unknown_type = saved_unknown;
        self.register_effects(program)?;

        let defs: Vec<Def> = self
            .ast
            .slice(program.items)
            .iter()
            .zip(self.ast.slice(program.spans))
            .filter_map(|(item, span)| match item {
                Item::Def {
                    name,
                    sig,
                    ctx,
                    body,
                } => Some(Def {
                    name: *name,
                    name_span: span.name,
                    sig: *sig,
                    ctx: *ctx,
                    body: *body,
                }),
                _ => None,
            })
            .collect();

        self.local_defs = defs.iter().map(|d| d.name).collect();
        for def in defs.iter().filter(|d| d.ctx.is_some()) {
            if let Some(field) = instances::wrapper_field(self.ast, def.body) {
                self.own_wrappers.insert(def.name, field);
            }
        }

        // One name, one definition: a module may not define a name twice, since a
        // name's type is its contract and two definitions would make it dishonest.
        let mut seen: HashSet<StrId> = HashSet::new();
        let mut defs = defs;
        let mut dups = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            if self.imported_builtins.contains(&d.name) {
                let span = self.ast.expr_span(d.body).unwrap_or(Span::at(0));
                self.errors.push(diag!(
                    Code::DuplicateSymbol, span, 0,
                    "`{}` is a builtin and cannot be redefined", d.name;
                    note: "CORE declares it with `@extern \"thrax\"`, and the compiler \
                           relies on its meaning"
                ));
                dups.push(i);
            } else if !seen.insert(d.name) {
                let span = self.ast.expr_span(d.body).unwrap_or(Span::at(0));
                self.errors.push(diag!(
                    Code::DuplicateSymbol, span, 0,
                    "`{}` is defined twice in module `{}`", d.name, self.module_name;
                    note: "a name has one type";
                    help: "to give an operation several types, declare an interface and \
                           define one instance per type"
                ));
                dups.push(i);
            }
        }
        for i in dups.into_iter().rev() {
            defs.remove(i);
        }
        for name in self.ast.slice(program.broken) {
            self.broken.insert(*name);
        }
        self.import_anchor = import_anchor(self.ast, program);
        // A local definition SHADOWS an imported value of the same name, so the
        // import's binding, owner, and context registration all give way to it.
        for d in &defs {
            self.value_module.remove(&d.name);
            self.global_ctx.remove(&d.name);
            self.ambiguous_imports.remove(&d.name);
        }

        // Register context-bearing definitions up front so a use anywhere in the
        // module resolves them regardless of source order.
        for d in &defs {
            let Some(ctx) = d.ctx else { continue };
            self.check_ctx_head(d.name, ctx)?;
            let sig = d.sig.expect("a `@ctx` is parsed as part of a signature");
            self.own_ctx.push((d.name, sig));
            self.qualified_ctx.insert((self.module_name, d.name), sig);
            self.global_ctx.insert(d.name, sig);
        }

        // Index every global whose type's head is a declared nominal type: these are
        // the candidates a context requirement searches.
        for d in &defs {
            let Some(sig) = d.sig else { continue };
            let (exposed, ctx) = self.scheme_and_ctx(sig, d.ctx.is_some());
            let Some(head) = self.ctx_head(self.unifier.zonk(exposed)) else {
                continue;
            };
            if !self.is_nominal(head) {
                continue;
            }
            let bundle = self.instance_bundle(exposed, ctx);
            let inst = Inst {
                name: d.name,
                module: Some(self.module_name),
                bundle,
                has_ctx: ctx.is_some(),
            };
            self.own_instances.push((d.name, inst.clone()));
            self.instances.entry(head).or_default().push(inst);
        }

        let single_index: HashMap<StrId, usize> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name, i))
            .collect();
        let graph = dependency_graph(self.ast, &defs, &single_index);

        let mut types: HashMap<StrId, Type> = HashMap::new();
        for component in utilities::tarjan::strongly_connected_components(&graph) {
            let level = self.unifier.level();
            // An unknown type name met along the way is the likelier root cause
            // of whatever went wrong after it (`foo : MyTyp = .{..}` fails to
            // build a struct because `MyTyp` does not exist), so it wins.
            let checked = self.check_component(&component, &defs, &mut types);
            self.report_holes();
            match checked {
                Ok(()) => self.require_signatures(&component, &defs, &types),
                Err(d) => {
                    let d = self.unknown_type.take().unwrap_or(d);
                    self.errors.push(d);
                    self.recover_component(level, &component, &defs, &mut types);
                }
            }
        }

        let out: Vec<(StrId, Type)> =
            defs.iter().map(|d| (d.name, types[&d.name])).collect();

        self.own_values = out.clone();
        if let Some(d) = self.unknown_type.take() {
            self.errors.push(d);
        }
        if let Some(first) = self.first_error() {
            return Err(first);
        }
        self.check_int_literals()?;
        self.build_extern_specs()?;
        for d in &defs {
            match self.node(d.body) {
                Expr::Extern { .. } => {
                    if let Some(spec) = self.extern_specs.get(&d.body) {
                        self.own_externs.insert(d.name, spec.clone());
                    }
                }
                Expr::Builtin => {
                    self.own_builtins.insert(d.name);
                }
                _ => {}
            }
        }
        // Type-check `$ @e <expr>` directives. All globals are in scope now, so
        // the expression resolves like a top-level body. The directive runs under
        // a closed `<@Meta>` handler and nothing else, so a stray `@IO` in a
        // generator is rejected (hermetic); `@build` is the one context that also
        // discharges `<@IO>`.
        let runs: Vec<Aol<Expr>> = self
            .ast
            .slice(program.items)
            .iter()
            .filter_map(|item| match item {
                Item::Run(e, _) => Some(*e),
                _ => None,
            })
            .collect();
        for e in runs {
            self.ambient = self.meta_ambient();
            let t = self.infer(e)?;
            self.require_code(t)?;
            // A directive is its own boundary: whatever it planned (an overload, a
            // context) is resolved here, since no definition encloses it.
            self.solve_pending()?;
            self.resolve_pending_ctx()?;
        }
        Ok(out)
    }

    /// The ambient a top-level body starts under: the pure closed row normally, or
    /// a fresh open row for the shell (`open_effects`), so an entered expression may
    /// perform effects the way a `main` body can without leaking them into its type.
    fn top_level_ambient(&mut self) -> Type {
        if self.open_effects {
            self.unifier.fresh()
        } else {
            self.unifier.types.row_empty()
        }
    }

    fn check_component(
        &mut self,
        component: &[usize],
        defs: &[Def],
        types: &mut HashMap<StrId, Type>,
    ) -> Result<()> {
        // See Note [Rémy levels].
        self.unifier.enter_level();
        let mut declared = Vec::with_capacity(component.len());
        for &i in component {
            let v = self.unifier.fresh();
            // A use inside the group sees a signed member's signature, so it may
            // recurse at another type. Not with a row tail: the body may extend
            // the row, and a use that saw only the signature would miss that.
            let seen = match (defs[i].sig, defs[i].ctx) {
                (Some(sig), None) if !self.sig_has_row_tail(sig) => {
                    self.scheme_and_ctx(sig, false).0
                }
                _ => v,
            };
            self.bind(defs[i].name, seen);
            declared.push(v);
        }
        // A member with a signature has its declared shape before any body in
        // the group is checked. Otherwise a call to it from a sibling checked
        // first meets a bare variable, and the effect of applying it is equated
        // with that sibling's ambient row, which the signature later closes.
        for (&i, decl) in component.iter().zip(&declared) {
            if component.len() > 1 {
                self.bind_signature(&defs[i], *decl)?;
            }
        }
        let mut sig_vars = Vec::new();
        for (&i, decl) in component.iter().zip(&declared) {
            if let Some(vars) = self.check_def_body(&defs[i], *decl)? {
                sig_vars.push(vars);
            }
        }
        self.solve_pending()?;
        self.resolve_pending_ctx()?;
        for vars in &sig_vars {
            self.check_sig_vars_rigid(vars)?;
        }
        self.unifier.leave_level();
        let mono = self.pending_vars();
        for (&i, decl) in component.iter().zip(&declared) {
            self.unifier.generalize_except(*decl, &mono);
            self.bind(defs[i].name, *decl);
            types.insert(defs[i].name, self.unifier.zonk(*decl));
        }
        Ok(())
    }

    /// Report each member of a checked component that has no signature. A global's
    /// type is its contract with every importer, so it is always written; the
    /// inferred type is offered as the annotation.
    fn require_signatures(&mut self, component: &[usize], defs: &[Def], types: &HashMap<StrId, Type>) {
        for &i in component {
            let d = &defs[i];
            if d.sig.is_some() || self.inferred == Some(d.name) {
                continue;
            }
            let shown = self.show(types[&d.name]);
            let span = d.name_span.or_else(|| self.ast.expr_span(d.body)).unwrap_or(Span::at(0));
            let mut diag = diag!(
                Code::TypeAnnotationRequired, span, 0,
                "global `{}` has no type annotation", d.name;
                note: "global symbols require a type annotation"
            );
            let rewrite = format!("the type `{shown}` was deduced for `{}`, rewrite it like this", d.name);
            diag = match d.name_span {
                Some(at) => diag.with_fix(
                    rewrite,
                    vec![Edit::new(Span::at(at.end), format!(" : {shown}"))],
                    Applicability::MachineApplicable,
                ),
                None => diag.with_help(format!("the type `{shown}` was deduced for `{}`", d.name)),
            };
            self.errors.push(diag);
        }
    }

    /// Undo what a failed [`Self::check_component`] left half done, then give
    /// each member a type its uses can lean on: its signature when it has one
    /// (callers are still checked against it), the error type otherwise.
    fn recover_component(
        &mut self,
        level: crate::typing::types::Level,
        component: &[usize],
        defs: &[Def],
        types: &mut HashMap<StrId, Type>,
    ) {
        self.unifier.unwind_level(level);
        self.scopes.truncate(1);
        self.pending.clear();
        self.ctx_pending.clear();
        self.current_sig = None;
        self.ambient = self.unifier.types.row_empty();
        for &i in component {
            let d = &defs[i];
            let ty = match (d.sig, d.ctx) {
                (Some(sig), None) => self.scheme_and_ctx(sig, false).0,
                _ => self.unifier.types.error(),
            };
            self.unknown_type = None;
            self.bind(d.name, ty);
            types.insert(d.name, ty);
        }
    }

    /// Report each pending `???`: its type, and the names in scope that would
    /// fill it (GHC's valid hole fits), each offered as a fix.
    fn report_holes(&mut self) {
        let holes = std::mem::take(&mut self.holes);
        for (site, t, locals) in holes {
            let t = self.unifier.zonk(t);
            let shown = self.show(t);
            let globals: Vec<(StrId, Type)> = self.scopes[0].iter().map(|(n, t)| (*n, *t)).collect();
            let mut fits: Vec<StrId> = Vec::new();
            for (name, scheme) in locals.iter().chain(globals.iter()) {
                if name.as_str().starts_with('@') || *name == crate::lexer::HOLE || fits.contains(name) {
                    continue;
                }
                let save = self.unifier.save();
                let inst = self.unifier.instantiate(*scheme);
                let ok = self.unifier.unify(inst, t, "a hole fit").is_ok();
                self.unifier.restore(save);
                if ok {
                    fits.push(*name);
                }
            }
            // Locals first (they are the likeliest), then the rest by name.
            let n_local = fits.iter().filter(|f| locals.iter().any(|(l, _)| l == *f)).count();
            fits[n_local..].sort_unstable_by_key(|f| f.as_str());
            let span = self.ast.expr_span(site).unwrap_or(Span::at(0));
            let mut d = diag!(Code::TypedHole, span, 0, "hole `???` has type `{shown}`");
            if fits.is_empty() {
                d = d.with_note("nothing in scope has this type");
            } else {
                let listed: Vec<String> = fits.iter().take(MAX_HOLE_FITS).map(|f| format!("`{f}`")).collect();
                d = d.with_note(format!("in scope with this type: {}", listed.join(", ")));
                for f in fits.iter().take(MAX_HOLE_FITS) {
                    d = d.with_fix(
                        format!("fill the hole with `{f}`"),
                        vec![Edit::new(span, f.as_str())],
                        Applicability::MaybeIncorrect,
                    );
                }
            }
            self.errors.push(d);
        }
    }

    /// Every error recovered from so far, most fundamental first, without the
    /// consequences of an earlier one (a message naming the error type).
    pub fn errors(&self) -> Vec<Diagnostic> {
        let mut seen = HashSet::new();
        self.errors
            .iter()
            .filter(|d| !mentions_error_type(d))
            .filter(|d| seen.insert((d.root().code.name(), d.root().span.start, d.root().span.end, d.root().msg.clone())))
            .cloned()
            .collect()
    }

    /// Whether the check recovered from any error (so [`Self::errors`] has the
    /// full report, possibly empty once consequences are dropped).
    pub fn recovered(&self) -> bool {
        !self.errors.is_empty()
    }

    /// This module's definitions and their types, as exported to importers. After
    /// a failed check a broken definition has its signature or the error type.
    pub fn own_values(&self) -> &[(StrId, Type)] {
        &self.own_values
    }

    fn first_error(&self) -> Option<Diagnostic> {
        self.errors.first().cloned()
    }

    /// The non-fatal diagnostics this check produced (unreachable match arms).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// This module's name.
    pub fn module_name(&self) -> StrId {
        self.module_name
    }
}

// ## Scope

impl<'a> Checker<'a> {
    /// The type store this checker built its types in, so a caller holding a
    /// `Type` can read it back.
    pub fn types(&self) -> &Types {
        &self.unifier.types
    }

    /// Every value name visible at the top level of this module (its own and
    /// the imported ones), with its type as a user reads it.
    pub fn visible_values(&self) -> Vec<(&'static str, String)> {
        let mut out: Vec<(&'static str, String)> = self.scopes[0]
            .iter()
            .filter(|(n, _)| !n.as_str().starts_with('@'))
            .map(|(n, t)| (n.as_str(), self.show_decl(*n, *t)))
            .collect();
        out.sort_unstable();
        out
    }

    /// The members of a qualified module `name` in scope (`C`, an import), with
    /// their types.
    pub fn qualified_values(&self, module: &str) -> Vec<(String, String)> {
        let Some(members) = self.qualified.get(&StrId::intern(module)) else { return Vec::new() };
        let mut out: Vec<(String, String)> = members
            .iter()
            .filter_map(|(n, ts)| ts.first().map(|t| (n.to_string(), self.show(*t))))
            .collect();
        out.sort_unstable();
        out
    }

    /// The type inferred for expression `e`, as a user reads it, when types were
    /// recorded and `e` was checked.
    pub fn expr_type(&self, e: Aol<Expr>) -> Option<String> {
        let t = *self.expr_types.as_ref()?.get(&e)?;
        Some(self.show(self.unifier.zonk(t)))
    }

    pub fn show(&self, ty: Type) -> String {
        self.unifier.show(ty)
    }

    /// The declared type of a global `name` in scope, as a user reads it (with any
    /// `@ctx` prefix). The shell's `:type` uses this for a bare name: inferring the
    /// name as an expression would have to pick a type for its context, which is
    /// exactly what the declaration leaves open.
    pub fn show_name(&mut self, name: StrId) -> Option<String> {
        if let Some(sig) = self.global_ctx.get(&name).copied() {
            let (exposed, ctx) = self.scheme_and_ctx(sig, true);
            if let Some(ctx) = ctx {
                let parts = self.unifier.show_all(&[ctx, exposed]);
                return Some(format!("@ctx {} -> {}", parts[0], parts[1]));
            }
        }
        self.lookup(name).map(|ty| self.show(ty))
    }

    /// How a definition's type reads to a user: its exposed arrow, prefixed with the
    /// `@ctx` parameter when it declares one. A printed type never hides the context
    /// a call site has to satisfy, which is the whole point of #213.
    pub fn show_decl(&self, name: StrId, ty: Type) -> String {
        match self.decl_ctx.get(&name) {
            Some(ctx) => {
                let parts = self.unifier.show_all(&[*ctx, ty]);
                format!("@ctx {} -> {}", parts[0], parts[1])
            }
            None => self.show(ty),
        }
    }

    fn bind(&mut self, name: StrId, ty: Type) {
        self.scopes
            .last_mut()
            .expect("a scope is always open")
            .insert(name, ty);
    }

    fn lookup(&self, name: StrId) -> Option<Type> {
        self.scopes.iter().rev().find_map(|s| s.get(&name).cloned())
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn leave_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty(), "popped the global scope");
    }
}

// ## Built-in types

impl<'a> Checker<'a> {
    fn install_builtins(&mut self) {
        // Most builtins are declared in CORE with `@extern "thrax"`. These stay here
        // because their types have no surface syntax.
        let int = self.unifier.types.con(ty::INT);

        // `@e X` runs the `@Code` X at compile time and splices the code in.
        // Special-cased in `infer_app`/`ty_of_ast`; this binding is the
        // first-class fallback.
        let e_ty = self.unifier.fresh_generic();
        let t = self.unifier.types.arrow(self.unifier.types.con("@Code"), e_ty);
        self.bind(StrId::intern("@e"), t);

        // The sized-tensor PRIMITIVES. `@`-sigil marks them as compiler intrinsics
        // (like `@Int64`), the minimal set the runtime provides; every nice name
        // (`index`, `length`, `dot`, `matmul`, `transpose`, `concat`) is a `library/LA`
        // function built from these plus `@ctx`, not a builtin.
        //
        // The sized-tensor primitives. `[n]T` and `Vec T` share the runtime vector
        // rep but are DISTINCT types, so these carry their own tensor-typed
        // signatures (a `@vec_*` binding is typed for `Vec`, not `[n]a`). Each is
        // VARIANCE-POLYMORPHIC: a fresh generic variance var per axis, so the
        // primitive works on any `[@Contra n]`/`[@Co n]`/`[n]` axis and preserves it
        // (`transpose` keeps per-position variance and swaps only the sizes).
        //
        // `@tensor_index : [n]a -> Int -> a` (modular read). `.[..]` desugars to the
        // `@IIndex` interface (its tensor instance in `library/LA` calls
        // `@tensor_index`); a custom container provides its own instance.
        {
            let a = self.unifier.fresh_generic();
            let n = self.unifier.fresh_generic_nat();
            let v = self.unifier.fresh_generic();
            let vn = tensor_type(&self.unifier.types, v, n, a);
            let t = self.unifier.types.arrow(vn, self.unifier.types.arrow(int, a));
        self.bind(StrId::intern("@tensor_index"), t);
        }
        // `@tensor_length : [n]a -> Int` (runtime size, untied to `n`: no dependent values).
        {
            let a = self.unifier.fresh_generic();
            let n = self.unifier.fresh_generic_nat();
            let v = self.unifier.fresh_generic();
            let vn = tensor_type(&self.unifier.types, v, n, a);
            let t = self.unifier.types.arrow(vn, int);
        self.bind(StrId::intern("@tensor_length"), t);
        }
        // `@tensor_create : [n]x -> (Int -> a) -> [n]a`: build a tensor the SAME SIZE as a
        // template from an index function (sound: result size = template size). The
        // constructing primitive that lets `transpose`/`matmul` be library code; it
        // preserves the template's variance `v` onto the result.
        {
            let x = self.unifier.fresh_generic();
            let a = self.unifier.fresh_generic();
            let n = self.unifier.fresh_generic_nat();
            let v = self.unifier.fresh_generic();
            let template = tensor_type(&self.unifier.types, v, n, x);
            let idx_fn = self.unifier.types.arrow(int, a);
            let result = tensor_type(&self.unifier.types, v, n, a);
            self.bind(StrId::intern("@tensor_create"),
                self.unifier.types.arrow(template, self.unifier.types.arrow(idx_fn, result)),
            );
        }
        // `@tensor_concat : [n]a -> [m]a -> [n+m]a`: the size-CHANGING join (which `@tensor_create`,
        // being size-preserving, cannot express), so it is a primitive. Both operands
        // and the result share the concatenated axis's variance `v`.
        {
            let a = self.unifier.fresh_generic();
            let n = self.unifier.fresh_generic_nat();
            let m = self.unifier.fresh_generic_nat();
            let v = self.unifier.fresh_generic();
            let tn = tensor_type(&self.unifier.types, v, n, a);
            let tm = tensor_type(&self.unifier.types, v, m, a);
            let tnm = tensor_type(&self.unifier.types, v, self.unifier.types.add(TypeNode::NatAdd(n, m)), a);
            let t = self.unifier.types.arrow(tn, self.unifier.types.arrow(tm, tnm));
        self.bind(StrId::intern("@tensor_concat"), t);
        }
        // `@tensor_transpose : [m][n]a -> [n][m]a`: an O(1) VIEW (swap axes/strides),
        // so `LA.transpose` copies nothing. Variance stays per POSITION (outer `vm`,
        // inner `vn`); only the sizes swap, so a `[@Contra m, @Co n]` matrix
        // transposes to `[@Contra n, @Co m]`.
        {
            let a = self.unifier.fresh_generic();
            let m = self.unifier.fresh_generic_nat();
            let n = self.unifier.fresh_generic_nat();
            let vm = self.unifier.fresh_generic();
            let vn = self.unifier.fresh_generic();
            let mn = tensor_type(&self.unifier.types, vm, m, tensor_type(&self.unifier.types, vn, n, a));
            let nm = tensor_type(&self.unifier.types, vm, n, tensor_type(&self.unifier.types, vn, m, a));
            let t = self.unifier.types.arrow(mn, nm);
        self.bind(StrId::intern("@tensor_transpose"), t);
        }
        // `@tensor_slice : [n]a -> Int -> Int -> [k]a`: an O(1) VIEW over `[lo, hi)` of
        // the leading axis. The result size `k` is a runtime value, so it is a fresh
        // nat (not `hi - lo`, which are runtime Ints); indexing it is modular/total, so
        // an unknown static size is consistent with the rest of the tensor design. The
        // sliced axis keeps its variance `v`.
        {
            let a = self.unifier.fresh_generic();
            let n = self.unifier.fresh_generic_nat();
            let k = self.unifier.fresh_generic_nat();
            let v = self.unifier.fresh_generic();
            let src = tensor_type(&self.unifier.types, v, n, a);
            let out = tensor_type(&self.unifier.types, v, k, a);
            self.bind(StrId::intern("@tensor_slice"),
                self.unifier.types.arrow(src, self.unifier.types.arrow(int, self.unifier.types.arrow(int, out))),
            );
        }
    }
}

// ## Dependency analysis

/// A global `$` definition, extracted from the program for dependency analysis.
#[derive(Clone)]
struct Def {
    name: StrId,
    name_span: Option<Span>,
    sig: Option<Aol<Ty>>,
    /// The context parameter's type when the signature declares one. It is also
    /// `sig`'s first `from`, so the signature stays what the user wrote.
    ctx: Option<Aol<Ty>>,
    body: Aol<Expr>,
}

/// Build the reference graph over globals: `graph[i]` lists the definitions that
/// definition `i` refers to.
fn dependency_graph(
    ast: &Ast,
    defs: &[Def],
    index: &HashMap<StrId, usize>,
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
fn free_globals(
    ast: &Ast,
    e: Aol<Expr>,
    globals: &HashMap<StrId, usize>,
    bound: &mut Vec<StrId>,
    out: &mut Vec<usize>,
) {
    match ast.expr(e) {
        Expr::Var { module: None, name } => {
            if !bound.contains(name) {
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
        | Expr::Extern { .. }
        | Expr::Builtin => {}

        Expr::App(f, x) => {
            free_globals(ast, *f, globals, bound, out);
            free_globals(ast, *x, globals, bound, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            free_globals(ast, *lhs, globals, bound, out);
            free_globals(ast, *rhs, globals, bound, out);
        }
        Expr::UnOp { operand, .. } => free_globals(ast, *operand, globals, bound, out),
        Expr::Tuple(items) | Expr::List(items) => ast
            .slice(*items)
            .iter()
            .for_each(|e| free_globals(ast, *e, globals, bound, out)),
        Expr::Range { lo, hi } => {
            free_globals(ast, *lo, globals, bound, out);
            if let Some(hi) = hi {
                free_globals(ast, *hi, globals, bound, out);
            }
        }
        Expr::Array { size } => free_globals(ast, *size, globals, bound, out),
        Expr::Slice { recv, slots } => {
            free_globals(ast, *recv, globals, bound, out);
            for s in ast.slice(*slots).iter() {
                match s {
                    SliceSlot::Index(x) => free_globals(ast, *x, globals, bound, out),
                    SliceSlot::Range(lo, hi) => {
                        free_globals(ast, *lo, globals, bound, out);
                        free_globals(ast, *hi, globals, bound, out);
                    }
                    SliceSlot::Full => {}
                }
            }
        }
        Expr::Field { record, .. } => free_globals(ast, *record, globals, bound, out),
        Expr::StructLit { fields, spread, .. } => {
            free_globals_field_inits(ast, ast.slice(*fields), globals, bound, out);
            if let Some(s) = spread {
                free_globals(ast, *s, globals, bound, out);
            }
        }
        Expr::Record {
            fields,
            with,
            update,
        } => {
            free_globals_field_inits(ast, ast.slice(*fields), globals, bound, out);
            for base in with.iter().chain(update.iter()) {
                free_globals(ast, *base, globals, bound, out);
            }
        }
        Expr::Variant { fields, .. } => free_globals_field_inits(ast, ast.slice(*fields), globals, bound, out),

        Expr::Let { bindings, body } => {
            let mark = bound.len();
            for b in ast.slice(*bindings).iter() {
                collect_pattern_binders(ast, b.pat, bound);
            }
            for b in ast.slice(*bindings).iter() {
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
        Expr::Match { scrut, arms } => {
            free_globals(ast, *scrut, globals, bound, out);
            for arm in ast.slice(*arms).iter() {
                let mark = bound.len();
                for pat in ast.slice(arm.patterns).iter() {
                    collect_pattern_binders(ast, *pat, bound);
                }
                if let Some(g) = arm.guard {
                    free_globals(ast, g, globals, bound, out);
                }
                free_globals(ast, arm.body, globals, bound, out);
                bound.truncate(mark);
            }
        }
        Expr::Lambda { params, body } => {
            let mark = bound.len();
            for p in ast.slice(*params).iter() {
                collect_pattern_binders(ast, p.pat, bound);
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
                for clause in ast.slice(h.clauses).iter() {
                    let mark = bound.len();
                    bound.push(clause.arg);
                    bound.push(h.continuation);
                    free_globals(ast, clause.body, globals, bound, out);
                    bound.truncate(mark);
                }
                if let Some((name, value_body)) = &h.value {
                    let mark = bound.len();
                    bound.push(*name);
                    free_globals(ast, *value_body, globals, bound, out);
                    bound.truncate(mark);
                }
            }
        }
        Expr::Defer { cleanup, body } => {
            free_globals(ast, *cleanup, globals, bound, out);
            free_globals(ast, *body, globals, bound, out);
        }
        Expr::CtxArg(value) => free_globals(ast, *value, globals, bound, out),
        Expr::Ascribe { expr, .. } => free_globals(ast, *expr, globals, bound, out),
    }
}

fn free_globals_field_inits(
    ast: &Ast,
    fields: &[FieldInit],
    globals: &HashMap<StrId, usize>,
    bound: &mut Vec<StrId>,
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
fn collect_pattern_binders(ast: &Ast, pat: Aol<Pattern>, bound: &mut Vec<StrId>) {
    match ast.pat(pat) {
        Pattern::Var(name) => bound.push(*name),
        Pattern::List(elems) => ast
            .slice(*elems)
            .iter()
            .for_each(|p| collect_pattern_binders(ast, *p, bound)),
        Pattern::Concat { pre, mid, suf } => {
            ast.slice(*pre)
                .iter()
                .chain(mid.iter())
                .chain(ast.slice(*suf).iter())
                .for_each(|p| collect_pattern_binders(ast, *p, bound));
        }
        Pattern::Tuple(pats) => ast
            .slice(*pats)
            .iter()
            .for_each(|p| collect_pattern_binders(ast, *p, bound)),
        Pattern::Struct { fields, .. } | Pattern::Variant { fields, .. } => {
            for f in ast.slice(*fields).iter() {
                match f {
                    FieldPat::Named { pat, .. } => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Positional(pat) => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Shorthand(name) => bound.push(*name),
                }
            }
        }
        Pattern::Record { fields, rest } => {
            for f in ast.slice(*fields).iter() {
                match f {
                    FieldPat::Named { pat, .. } => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Positional(pat) => collect_pattern_binders(ast, *pat, bound),
                    FieldPat::Shorthand(name) => bound.push(*name),
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

/// The most hole fits listed (and offered as fixes).
const MAX_HOLE_FITS: usize = 6;

/// Whether a diagnostic is a consequence of an earlier failure: it talks about a
/// value whose type is the error type, which only a recovered error introduces.
fn mentions_error_type(d: &Diagnostic) -> bool {
    let e = crate::typing::types::ERROR_CON;
    d.frames().iter().any(|f| f.msg.contains(e))
        || d.labels().iter().any(|l| l.msg.contains(e))
        || d.notes().iter().any(|n| n.contains(e))
}
