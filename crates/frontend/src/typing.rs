//! Algorithm W over the [`crate::parser::data`] handle-based AST.
//!
//! Inference is driven by [`Checker::infer`] (expressions) and
//! [`Checker::type_pattern`] (patterns), threading the [`Engine`] for
//! unification and the lexical scope stack for variable types. The AST is read
//! through a borrowed [`Ast`]: a node handle is resolved with `Checker::node` /
//! `Checker::tnode` / `Checker::pnode`, and an interned name with
//! `Checker::text`. Because `ast` is a shared reference, those resolve to
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
//! front by `Checker::register_types`. A name has ONE definition, so nothing is
//! resolved from argument types except an operation name several effects declare.
//! Polymorphism over types goes through interfaces: a definition's leading `@ctx`
//! parameter is resolved BY TYPE at the definition boundary, against the locals in
//! scope and then a flat index of the instances in scope (see
//! `documentation/interfaces.md`). Definition bodies are checked against their
//! signatures (bidirectional checking); the monomorphism restriction keeps a
//! variable a pending requirement still constrains from being generalized early.

pub mod data;
pub mod engine;
pub mod exhaustive;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::lowering::{CtxVal, HookImpl};
use crate::parser::data::{
    Ast, Binding, Expr, FieldInit, FieldPat, Item, Pattern, Payload, Program, RecField,
    SliceSlot, Ty, Variance,
};
use utilities::Aol;
use utilities::{diag, Code, Diagnostic, ExternArg, Result, Span};

use crate::typing::data::{self as ty, Type, TypeNode, Types, VarId};
use crate::typing::engine::Engine;

/// An `@extern`'s marshalling spec: how the single applied Thrax value maps to
/// positional C arguments (`params`), the flattened C argument type names
/// (`arg_types`, one per positional C argument), and the result type name. The
/// three are consumed by lowering to build the extern value both engines call.
pub type ExternSpec = (Vec<ExternArg>, Vec<String>, String);

/// Which slots of a declared type are lazy: keyed by `(type name, variant tag)`
/// with `None` as the tag for a struct, holding one flag per slot in declaration
/// order. Only types with at least one lazy slot appear.
pub type LazySlots = HashMap<(String, Option<String>), Vec<bool>>;

/// A declared struct type. `params` are the implicit type parameters (the type
/// variables appearing in the fields, in order of first appearance); `fields`
/// keeps declaration order (which is also the positional-constructor order).
/// `crepr` is set for a C-layout foreign struct (`@struct @extern "abi"`): its
/// runtime value is a flat, unboxed C struct rather than a boxed record, and it
/// may cross the `@extern` boundary by value.
#[derive(Clone)]
struct StructInfo<'a> {
    params: Vec<&'a str>,
    fields: Vec<(&'a str, Aol<Ty>)>,
    crepr: bool,
    /// A C `union` (`@union @extern`): members share offset 0. Handled as a C-repr
    /// struct otherwise; only its layout and single-member construction differ.
    c_union: bool,
}

/// Pending `with Other` splices, keyed by the type that wrote them: whether it
/// is a struct (rather than a union), and each included name with the span it
/// was written at, so a bad include carets its own `with`.
type PendingIncludes<'a> = HashMap<&'a str, (bool, Vec<(&'a str, Span)>)>;

/// A declared union type: implicit `params` and one [`VariantSig`] per variant.
#[derive(Clone)]
struct UnionInfo<'a> {
    params: Vec<&'a str>,
    variants: Vec<VariantSig<'a>>,
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
    /// Which slots of a declared type hold their value lazily, keyed by
    /// `(type, Some(variant tag))` for a union and `(type, None)` for a struct,
    /// one flag per slot in declaration order. See [`Self::compute_lazy_slots`].
    lazy_slots: LazySlots,
    /// Computed C memory layout for each `@struct @extern "abi"` (C-repr) struct,
    /// keyed by type name. Used to marshal a struct value across the `@extern`
    /// boundary by value.
    crepr_layouts: HashMap<&'a str, utilities::CLayout>,
    /// Type aliases: `name -> (declared params, body)`. An applied alias is
    /// expanded by substituting its arguments for the parameters in the body.
    aliases: HashMap<&'a str, (Vec<&'a str>, Aol<Ty>)>,
    /// Each declared effect's operations, `effect -> op -> its `Arg -> Res`
    /// scheme. Used to type a handler clause head, which cannot be resolved by
    /// inference alone.
    effect_ops: HashMap<&'a str, HashMap<&'a str, Type>>,
    /// Operation names several EFFECTS declare, mapped to one candidate per effect.
    /// The only argument-type-driven selection left (see [`Cand`]).
    overloads: HashMap<&'a str, Vec<Cand<'a>>>,
    /// Effect-operation uses that were ambiguous when first seen. Solved to a
    /// fixpoint at each definition boundary.
    pending: Vec<Pending<'a>>,
    /// Names this module defines itself, so a use of one is NOT rewritten to an
    /// imported module's copy.
    local_defs: HashSet<&'a str>,
    /// Single imported values, `name -> module`, so a bare use lowers to the
    /// owning module even when another loaded module defines the same name.
    value_module: HashMap<&'a str, &'a str>,
    /// Names more than one import brings in, mapped to the modules that export
    /// them. A bare use is an error naming the owners; `Module.name` still works.
    ambiguous_imports: HashMap<&'a str, Vec<&'a str>>,
    /// Bare-call sites resolved to a specific module. Lowering rewrites the
    /// referenced `Expr::Var` to `MOD.name`.
    resolved_calls: HashMap<Aol<Expr>, &'a str>,
    /// The context parameter type of each context-bearing definition in this
    /// module, sharing its variables with the exposed type, so a printed signature
    /// can show the `@ctx` prefix.
    decl_ctx: HashMap<&'a str, Type>,
    /// Top-level definitions whose signature opens with a `@ctx` context
    /// parameter, by name, as the signature's AST handle (the context type is its
    /// first `from`). A use site plans the context from it and records the resolved
    /// value in [`Self::ctx_args`]; lowering injects it as a leading argument.
    /// Populated up front (own module) so resolution is order-independent, and
    /// extended from imports.
    global_ctx: HashMap<&'a str, Aol<Ty>>,
    /// The same, keyed by `(module, name)` so a QUALIFIED use (`MOD.f`) plans its
    /// context too (the bare-keyed map collides when two modules define the name).
    qualified_ctx: HashMap<(&'a str, &'a str), Aol<Ty>>,
    /// This module's own context-bearing definitions, re-exported to importers.
    own_ctx: Vec<(&'a str, Aol<Ty>)>,
    /// Each use site of a context-bearing function, mapped to the resolved context
    /// value lowering injects ahead of the explicit arguments.
    ctx_args: HashMap<Aol<Expr>, CtxVal>,
    /// Context resolutions deferred to the enclosing definition's boundary, where
    /// inference has pinned the requirement's type variables.
    ctx_pending: Vec<PendingCtx<'a>>,
    /// The elaborated signature of the definition currently being checked. Its type
    /// variables are the caller's to choose, so context resolution may not bind
    /// them: a `$ g : a -> @str = \x = show x` must declare the context rather than
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
    instances: HashMap<String, Vec<Inst<'a>>>,
    /// This module's own instances, re-exported to importers.
    own_instances: Vec<(&'a str, Inst<'a>)>,
    /// Types with unresolved `with Other` splices, `name -> (is_struct, includes)`.
    /// Drained as each type's members are copied in (see `splice_includes`). This
    /// is a declaration-time convenience only; no type relationship is recorded.
    pending_includes: PendingIncludes<'a>,
    /// Type variables introduced by integer literals, which may be Int or Real;
    /// leftovers default to Int at the definition boundary. Each carries the
    /// literal's source span so a defaulting error can point at it.
    numeric: Vec<(Type, Span)>,
    /// This module's own exports, recorded after checking.
    own_values: Vec<(&'a str, Type)>,
    own_type_names: Vec<&'a str>,
    /// Names declared after a `$ @private` marker: defined and usable within this
    /// module, but not exported, so no importer can bind or qualify them.
    private_names: HashSet<&'a str>,
    /// Private names of the modules this one imports, `module -> names`. A
    /// qualified reference (`A.helper`) to one is rejected rather than deferred to
    /// the runtime, which would otherwise reach the still-present global.
    imported_private: HashMap<&'a str, HashSet<&'a str>>,
    /// Value schemes pulled in from imports (with their source module), finalized
    /// once.
    imported: HashMap<&'a str, Vec<(Type, &'a str)>>,
    /// Imported names reachable qualified as `MOD.name`.
    qualified: HashMap<&'a str, HashMap<&'a str, Vec<Type>>>,
    module_name: &'a str,
    /// `[..]` literal sites resolved to a sized tensor `[n]T`: the one `[...]`
    /// shape no hook can build, since the payload cannot carry the static size.
    /// Lowering flattens the collected elements with `@tensor_stack`.
    tensor_exprs: HashSet<Aol<Expr>>,
    /// Argument sites promoted to a record: a bare scalar `1` or a positional
    /// `{1, 2}` passed where a record is expected, mapped to the target record's
    /// field names (in order). Lowering wraps the value into a name-keyed record.
    promotions: HashMap<Aol<Expr>, Vec<String>>,
    /// `.{ ... }` struct-literal sites, mapped to the nominal struct name the
    /// checker resolved them to (from a type annotation / expected type, or the
    /// field set). Lowering reads this so a positional literal gets the right field
    /// names instead of falling back to positional indices.
    struct_lit_names: HashMap<Aol<Expr>, String>,
    /// Literal sites (`"..."`, `[..]`, an int, a real) that a `@compiler_interface_*`
    /// construction hook builds into a user type instead of the built-in default,
    /// mapped to the resolved hook's `(owning module, emitted name)`. Lowering wraps
    /// the raw payload term in a call to this hook; an unrecorded literal folds to the
    /// plain built-in constant (no hook, no conversion).
    literal_hooks: HashMap<Aol<Expr>, HookImpl>,
    /// Blessed-interface VALUE sites (the `@IIndex` / `@IImagLit` head a desugar
    /// emits), mapped to the single field lowering projects off the instance that
    /// `ctx_args` records for the same site.
    blessed_sites: HashMap<Aol<Expr>, String>,
    /// Literal PATTERN sites (`is "foo"`, `is 42`) whose scrutinee is a user type,
    /// mapped to the resolved `(build hook, equality hook)`: the pattern matches by
    /// building the literal into the user type (build hook) and comparing it against
    /// the scrutinee (equality hook), instead of the built-in `==` on a primitive.
    literal_pattern_hooks: HashMap<Aol<Pattern>, (HookImpl, HookImpl)>,
    /// Sequence PATTERN sites (`is [a, b, ..rest]`, `is h :: t`, `is []`) whose
    /// scrutinee is a user type, mapped to the resolved `@compiler_interface_sequence_view`
    /// hook's `(module, emitted name)`. Lowering unfolds the pattern through this view.
    sequence_pattern_hooks: HashMap<Aol<Pattern>, HookImpl>,
    /// The union each variant pattern resolved to, so the exhaustiveness check
    /// reads the same union a bare `.Tag` was typed against.
    variant_pattern_unions: HashMap<Aol<Pattern>, &'a str>,
    /// Non-fatal diagnostics (unreachable match arms), in source order.
    warnings: Vec<Diagnostic>,
    /// The ordered field names each `with subject in body` brings into scope,
    /// keyed by the `With` node. Lowering desugars `with` into a `let` per field,
    /// so the Core has no name-binding-by-type node and stays De-Bruijn indexable.
    with_fields: HashMap<Aol<Expr>, Vec<String>>,
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
    own_externs: HashMap<&'a str, ExternSpec>,
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
}

/// One candidate of an operation name that several EFFECTS declare (`ask` in both
/// `Reader` and `Config`). This is the one place left that picks a definition from
/// argument types: values are never overloaded, a name has one definition.
#[derive(Clone)]
struct Cand<'a> {
    ty: Type,
    module: Option<&'a str>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Cand<'a> {
    /// An effect-operation candidate, owned by no module (never rewritten to a
    /// qualified reference).
    fn local(ty: Type) -> Cand<'a> {
        Cand { ty, module: None, _marker: std::marker::PhantomData }
    }
}

/// One candidate value for a context requirement: a global whose type's head is
/// the requirement's head constructor. `bundle` packs its exposed type and, when
/// the instance itself takes a context, that requirement, generalized together so
/// instantiating the bundle keeps their variables aligned.
#[derive(Clone)]
struct Inst<'a> {
    /// The global's emitted name: the source name, or the type-mangled one when it
    /// shares its name with another definition in the same module.
    name: String,
    module: Option<&'a str>,
    bundle: Type,
    has_ctx: bool,
}

/// One binder in scope that could satisfy a context requirement. `index` is set
/// when the binder holds a BUNDLE of contexts and this is one component of it, so
/// the resolved value is `name.index`.
#[derive(Clone)]
struct CtxLocal<'a> {
    depth: usize,
    name: &'a str,
    ty: Type,
    index: Option<usize>,
}

/// A context resolution deferred to the enclosing definition's boundary, where the
/// requirement's type variables are pinned.
struct PendingCtx<'a> {
    site: Aol<Expr>,
    fname: String,
    req: Type,
    /// The enclosing definition's signature, whose variables resolution may not
    /// bind (zonked at resolution time, so a variable chain is followed).
    sig: Option<Type>,
    /// Binders in scope at the use site that could satisfy the requirement. Captured
    /// there because the scopes are gone by the time the boundary resolves.
    locals: Vec<CtxLocal<'a>>,
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
    pub fn new(ast: &'a Ast, types: Rc<Types>) -> Checker<'a> {
        let eng = Engine::new(types);
        let ambient = eng.types.row_empty();
        let mut c = Checker {
            ast,
            eng,
            scopes: vec![HashMap::new()],
            structs: HashMap::new(),
            unions: HashMap::new(),
            lazy_slots: HashMap::new(),
            crepr_layouts: HashMap::new(),
            aliases: HashMap::new(),
            effect_ops: HashMap::new(),
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
            private_names: HashSet::new(),
            imported_private: HashMap::new(),
            imported: HashMap::new(),
            qualified: HashMap::new(),
            module_name: "",
            tensor_exprs: HashSet::new(),
            promotions: HashMap::new(),
            struct_lit_names: HashMap::new(),
            literal_hooks: HashMap::new(),
            blessed_sites: HashMap::new(),
            literal_pattern_hooks: HashMap::new(),
            sequence_pattern_hooks: HashMap::new(),
            variant_pattern_unions: HashMap::new(),
            warnings: Vec::new(),
            with_fields: HashMap::new(),
            extern_tys: HashMap::new(),
            extern_specs: HashMap::new(),
            own_externs: HashMap::new(),
            ambient,
            unknown_type: None,
            lenient: false,
            open_effects: false,
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

    /// The `[..]` literal sites resolved to a sized tensor. Lowering builds a vector.
    pub fn tensor_nodes(&self) -> &HashSet<Aol<Expr>> {
        &self.tensor_exprs
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

    /// Literal sites a `@compiler_interface_*` construction hook builds into a user
    /// type, mapped to the hook's `(module, emitted name)`. Lowering wraps the raw
    /// payload term in a call to this hook.
    pub fn literal_hooks(&self) -> &HashMap<Aol<Expr>, HookImpl> {
        &self.literal_hooks
    }

    /// Blessed-interface value sites, mapped to the field lowering projects.
    pub fn blessed_sites(&self) -> &HashMap<Aol<Expr>, String> {
        &self.blessed_sites
    }

    /// Literal pattern sites matched through a user type's construction + equality
    /// hooks, mapped to `(build hook, equality hook)` as `(module, emitted name)`.
    pub fn literal_pattern_hooks(&self) -> &HashMap<Aol<Pattern>, (HookImpl, HookImpl)> {
        &self.literal_pattern_hooks
    }

    /// Sequence pattern sites matched through a user type's `sequence_view` hook,
    /// mapped to the hook's `(module, emitted name)`.
    pub fn sequence_pattern_hooks(&self) -> &HashMap<Aol<Pattern>, HookImpl> {
        &self.sequence_pattern_hooks
    }



    /// The lazy-slot table, for lowering to consult when it builds and consumes
    /// values of a declared type.
    pub fn lazy_slots(&self) -> &LazySlots {
        &self.lazy_slots
    }

    /// Bare-call `Expr::Var` sites this checker resolved to a specific module.
    /// Lowering rewrites each to a qualified `MOD.name` so the interpreter reaches
    /// the intended function rather than a same-named one from another module.
    pub fn call_modules(&self) -> &HashMap<Aol<Expr>, &'a str> {
        &self.resolved_calls
    }


    /// Each use site of a context-bearing function, mapped to the resolved context
    /// value lowering injects ahead of the explicit arguments.
    pub fn ctx_calls(&self) -> &HashMap<Aol<Expr>, CtxVal> {
        &self.ctx_args
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
    pub fn extern_sigs(&self) -> HashMap<Aol<Expr>, ExternSpec> {
        self.extern_specs.clone()
    }

    /// This module's foreign functions, keyed by the global name bound to the bare
    /// `@extern`. The driver aggregates these across modules (keyed by owner
    /// module) so a call site can flatten a foreign call cross-module.
    pub fn own_externs(&self) -> &HashMap<&'a str, ExternSpec> {
        &self.own_externs
    }

    /// This module's name.
    pub fn module_name(&self) -> &'a str {
        self.module_name
    }

    /// Validate every `@extern`'s shape and record its marshalling spec. A C
    /// function has no first-class closure to curry, so a Thrax `@extern` must be
    /// a function of EXACTLY ONE argument: the arrow spine may have only one link.
    /// A `{a: A, b: B} -> R` record groups multiple C parameters; `A -> B -> R`
    /// is currying and is rejected here with a diagnostic.
    fn build_extern_specs(&mut self) -> Result<()> {
        let sites: Vec<(Aol<Expr>, Type)> = self
            .extern_tys
            .iter()
            .map(|(&e, ty)| (e, self.eng.zonk(*ty)))
            .collect();
        for (e, ty) in sites {
            let span = self.ast.expr_span(e).unwrap_or_else(|| Span::at(0));
            let TypeNode::Arrow(param, ret, _) = self.eng.types.node(ty) else {
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "an `@extern` must be a foreign function; declare it `A -> B` (a single \
                     argument), or `{{a: A, b: B}} -> R` to pass several C parameters"
                ));
            };
            if matches!(self.eng.head(ret), TypeNode::Arrow(..)) {
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "a C `@extern` takes a SINGLE argument (C has no first-class functions to \
                     curry): group the C parameters into one record, `{{a: A, b: B}} -> R`, \
                     rather than currying as `A -> B -> R`"
                ));
            }
            let param = self.eng.resolve(param);
            let (params, arg_types) = self.extern_param_spec(param)?;
            let ret_name = marshal_name(&self.eng.types, self.eng.resolve(ret));
            self.extern_specs.insert(e, (params, arg_types, ret_name));
        }
        Ok(())
    }

    /// Reduce an extern's single parameter type to its positional C arguments:
    /// unit takes none; an anonymous record contributes one C argument per field,
    /// in declared order, pulled by name; any other type is one C argument used
    /// directly (a scalar, a C-repr struct by value, `@ptr`, `Str`, a callback, or
    /// a `List T` array).
    fn extern_param_spec(&mut self, param: Type) -> Result<(Vec<ExternArg>, Vec<String>)> {
        if is_unit_ty(&self.eng.types, param) {
            return Ok((vec![], vec![]));
        }
        // A name-keyed record: one C argument per field, in declared order,
        // pulled by name (so a reordered call site still marshals in C order).
        if let TypeNode::Record(_) = self.eng.types.node(param) {
            let fields = self.record_fields_of(param)?;
            let mut params = Vec::with_capacity(fields.len());
            let mut arg_types = Vec::with_capacity(fields.len());
            for (name, fty) in fields {
                params.push(ExternArg::Field(name));
                arg_types.push(marshal_name(&self.eng.types, self.eng.resolve(fty)));
            }
            return Ok((params, arg_types));
        }
        // A closed record parameter surfaces as a positional tuple: one C argument
        // per element, in order.
        if let TypeNode::Tuple(items) = self.eng.types.node(param) {
            let mut params = Vec::with_capacity(items.len());
            let mut arg_types = Vec::with_capacity(items.len());
            for (i, it) in self.eng.types.items(items).to_vec().into_iter().enumerate() {
                params.push(ExternArg::Elem(i));
                let r = self.eng.resolve(it);
                arg_types.push(marshal_name(&self.eng.types, r));
            }
            return Ok((params, arg_types));
        }
        Ok((vec![ExternArg::Whole], vec![marshal_name(&self.eng.types, param)]))
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

    /// Record every name declared under a `$ @private` marker. Symbols are public
    /// by default; a `$ @private` makes every declaration below it, to the end of
    /// the file, module-private. A private name is fully usable inside the module;
    /// it is only withheld from importers.
    fn collect_private_names(&mut self, program: &Program) {
        let mut private = false;
        for item in self.ast.slice(program.items).iter() {
            let name = match item {
                Item::Private => {
                    private = true;
                    continue;
                }
                _ if !private => continue,
                Item::Def { name, .. }
                | Item::Struct { name, .. }
                | Item::Union { name, .. }
                | Item::Alias { name, .. } => *name,
                _ => continue,
            };
            self.private_names.insert(self.text(name));
        }
    }

    /// Check a whole program, returning the inferred (generalized) type of every
    /// global definition, in source order.
    pub fn check_program(&mut self, program: &Program) -> Result<Vec<(&'a str, Type)>> {
        self.module_name = self.text(program.module);
        self.collect_private_names(program);
        self.finalize_imports();
        self.register_types(program)?;
        self.validate_crepr_structs()?;
        // Row registration elaborates struct field types only to cache their rows
        // for the bridge; it must not report type errors (a field may reference a
        // type not imported into this module, e.g. a re-exported struct's internals).
        // Real unknown-type errors are still caught when definitions are elaborated.
        let saved_unknown = self.unknown_type.take();
        self.register_struct_rows();
        self.unknown_type = saved_unknown;
        self.register_effects(program);

        let defs: Vec<Def<'a>> = self
            .ast
            .slice(program.items)
            .iter()
            .filter_map(|item| match item {
                Item::Def {
                    name,
                    sig,
                    ctx,
                    body,
                } => Some(Def {
                    name: self.text(*name),
                    sig: *sig,
                    ctx: *ctx,
                    body: *body,
                }),
                _ => None,
            })
            .collect();

        self.local_defs = defs.iter().map(|d| d.name).collect();

        // One name, one definition: a module may not define a name twice, since a
        // name's type is its contract and two definitions would make it dishonest.
        let mut seen: HashSet<&'a str> = HashSet::new();
        for d in &defs {
            if !seen.insert(d.name) {
                let span = self.ast.expr_span(d.body).unwrap_or(Span::at(0));
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "`{}` is defined twice in module `{}`", d.name, self.module_name;
                    note: "a name has one type; to give an operation several types, \
                           declare an interface and define one instance per type"
                ));
            }
        }
        // A local definition SHADOWS an imported value of the same name, so the
        // import's binding, owner, and context registration all give way to it.
        for d in &defs {
            self.value_module.remove(d.name);
            self.global_ctx.remove(d.name);
            self.ambiguous_imports.remove(d.name);
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
            let Some(head) = self.ctx_head(self.eng.zonk(exposed)) else {
                continue;
            };
            if !self.is_nominal(head) {
                continue;
            }
            let bundle = self.instance_bundle(exposed, ctx);
            let inst = Inst {
                name: d.name.to_string(),
                module: Some(self.module_name),
                bundle,
                has_ctx: ctx.is_some(),
            };
            let key = self.eng.types.name(head).to_string();
            self.own_instances.push((d.name, inst.clone()));
            self.instances.entry(key).or_default().push(inst);
        }

        let single_index: HashMap<&'a str, usize> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name, i))
            .collect();
        let graph = dependency_graph(self.ast, &defs, &single_index);

        let mut types: HashMap<&'a str, Type> = HashMap::new();
        for component in utilities::scc::scc(&graph) {
            self.check_component(&component, &defs, &mut types)?;
        }

        let out: Vec<(&'a str, Type)> =
            defs.iter().map(|d| (d.name, types[d.name])).collect();

        self.own_values = out.clone();
        if let Some(d) = self.unknown_type.take() {
            return Err(d);
        }
        self.build_extern_specs()?;
        for d in &defs {
            if matches!(self.node(d.body), Expr::Extern { .. }) {
                if let Some(spec) = self.extern_specs.get(&d.body) {
                    self.own_externs.insert(d.name, spec.clone());
                }
            }
        }
        // Type-check `$ @run <expr>` directives. All globals are in scope now, so
        // the expression resolves like a top-level body; a fresh pure ambient means
        // an effect it performs that the compile-time runtime cannot discharge is
        // rejected here. The value is discarded (the driver forces it at build
        // time), so its type is not recorded.
        let runs: Vec<(Aol<Expr>, bool)> = self
            .ast
            .slice(program.items)
            .iter()
            .filter_map(|item| match item {
                Item::Run(e, _, meta) => Some((*e, *meta)),
                _ => None,
            })
            .collect();
        for (e, meta) in runs {
            // `$ @run X` runs under a closed `<@meta>` handler: it discharges
            // `@meta` (so meta ops type-check) but nothing else, so a stray `@io`
            // in a generator is still rejected (hermetic). `$ @e X` requires a
            // pure operand (empty ambient), so a meta op in it is an error. The
            // context that discharges `<@io>` as well is the `@build` function.
            self.ambient = if meta {
                {
                        let empty = self.eng.types.row_empty();
                        self.eng.types.row_extend("@meta", empty)
                    }
            } else {
                self.eng.types.row_empty()
            };
            self.infer(e)?;
            // A directive is its own boundary: whatever it planned (an overload, a
            // context) is resolved here, since no definition encloses it.
            self.solve_pending()?;
            self.resolve_pending_ctx()?;
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
        for (name, scheme) in &other.own_values {
            if other.private_names.contains(name) {
                continue;
            }
            let qualified = self.import_scheme(*scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, vec![qualified]);
        }
    }

    pub fn import_from(&mut self, other: &Checker<'a>) {
        if !other.private_names.is_empty() {
            self.imported_private
                .entry(other.module_name)
                .or_default()
                .extend(other.private_names.iter().copied());
        }
        // Bring the exporter's context-bearing functions in, so a bare or qualified
        // use of an imported one plans its context (the signature handle lives in
        // the shared `Ast`).
        for (name, sig) in &other.own_ctx {
            if other.private_names.contains(name) {
                continue;
            }
            self.qualified_ctx.insert((other.module_name, name), *sig);
            self.global_ctx.insert(name, *sig);
        }
        // Bring the exporter's instances in, translating their packed types into
        // this module's engine so their variables stay generalized.
        for (name, inst) in &other.own_instances {
            if other.private_names.contains(name) {
                continue;
            }
            let mut map = HashMap::new();
            let bundle = self.import_ty(inst.bundle, &mut map);
            self.eng.note_tensor_sizes(bundle);
            let Some(head) = self.ctx_head(self.eng.zonk(self.bundle_parts(bundle).0)) else {
                continue;
            };
            let key = self.eng.types.name(head).to_string();
            self.instances.entry(key).or_default().push(Inst {
                name: inst.name.clone(),
                module: Some(other.module_name),
                bundle,
                has_ctx: inst.has_ctx,
            });
        }
        for &name in &other.own_type_names {
            if other.private_names.contains(name) {
                continue;
            }
            if let Some(s) = other.structs.get(name) {
                self.structs.insert(name, s.clone());
            }
            if let Some(u) = other.unions.get(name) {
                self.unions.insert(name, u.clone());
            }
            if let Some(a) = other.aliases.get(name) {
                self.aliases.insert(name, a.clone());
            }
        }
        let module = other.module_name;
        for (name, scheme) in &other.own_values {
            if other.private_names.contains(name) {
                continue;
            }
            let unqualified = self.import_scheme(*scheme);
            self.imported
                .entry(name)
                .or_default()
                .push((unqualified, module));
            let qualified = self.import_scheme(*scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, vec![qualified]);
        }
    }

    fn finalize_imports(&mut self) {
        let imported = std::mem::take(&mut self.imported);
        for (name, mut cands) in imported {
            if cands.len() == 1 {
                let (ty, module) = cands.pop().expect("one candidate");
                self.value_module.insert(name, module);
                self.bind(name, ty);
            } else {
                // Two modules export the same name. Global scope is flat, so the bare
                // name cannot pick one; it is an error at the USE site (with the
                // qualified form as the escape), not here, because a module may import
                // both and reference neither bare.
                let mut owners: Vec<&'a str> = cands.iter().map(|(_, m)| *m).collect();
                owners.sort_unstable();
                self.ambiguous_imports.insert(name, owners);
            }
        }
    }

    fn import_scheme(&mut self, ty: Type) -> Type {
        let mut map = HashMap::new();
        let imported = self.import_ty(ty, &mut map);
        // `import_ty` makes fresh plain generics, losing the `Nat` kind; re-mark the
        // tensor-size variables so an imported `[n]a -> ...` still kind-checks.
        self.eng.note_tensor_sizes(imported);
        imported
    }

    fn import_ty(&mut self, ty: Type, map: &mut HashMap<VarId, Type>) -> Type {
        match self.eng.types.node(ty) {
            TypeNode::Var(id) => match map.get(&id) {
                Some(t) => *t,
                None => {
                    let fresh = self.eng.fresh_generic();
                    map.insert(id, fresh);
                    fresh
                }
            },
            TypeNode::NatAdd(a, b) => {
                let (x, y) = (self.import_ty(a, map), self.import_ty(b, map));
                self.eng.types.add(TypeNode::NatAdd(x, y))
            }
            TypeNode::NatMul(a, b) => {
                let (x, y) = (self.import_ty(a, map), self.import_ty(b, map));
                self.eng.types.add(TypeNode::NatMul(x, y))
            }
            TypeNode::App(head, arg) => {
                let (h, a) = (self.import_ty(head, map), self.import_ty(arg, map));
                self.eng.types.app(h, a)
            }
            TypeNode::Arrow(from, to, eff) => {
                let (f, t, e) = (
                    self.import_ty(from, map),
                    self.import_ty(to, map),
                    self.import_ty(eff, map),
                );
                self.eng.types.arrow_eff(f, t, e)
            }
            TypeNode::RowExtend(label, rest) => {
                let r = self.import_ty(rest, map);
                self.eng.types.add(TypeNode::RowExtend(label, r))
            }
            TypeNode::Tuple(items) => {
                let items = self.eng.types.items(items).to_vec();
                let mapped: Vec<Type> = items.into_iter().map(|t| self.import_ty(t, map)).collect();
                self.eng.types.tuple(mapped)
            }
            TypeNode::Record(row) => {
                let r = self.import_ty(row, map);
                self.eng.types.record(r)
            }
            TypeNode::RowField(label, fty, rest) => {
                let (f, r) = (self.import_ty(fty, map), self.import_ty(rest, map));
                self.eng.types.add(TypeNode::RowField(label, f, r))
            }
            // Con, Nat and RowEmpty are already shared nodes.
            TypeNode::Con(_) | TypeNode::Nat(_) | TypeNode::RowEmpty => ty,
        }
    }

    /// The ambient a top-level body starts under: the pure closed row normally, or
    /// a fresh open row for the shell (`open_effects`), so an entered expression may
    /// perform effects the way a `main` body can without leaking them into its type.
    fn top_level_ambient(&mut self) -> Type {
        if self.open_effects {
            self.eng.fresh()
        } else {
            self.eng.types.row_empty()
        }
    }

    fn scheme_of_sig(&mut self, sig: Aol<Ty>) -> Type {
        self.eng.enter_level();
        let mut tvars = HashMap::new();
        let ty = self.ty_of_ast(sig, &mut tvars);
        self.eng.leave_level();
        self.eng.generalize(ty);
        self.eng.zonk(ty)
    }

    /// Elaborate a signature into the type callers see and, when it opens with a
    /// `@ctx` parameter, that parameter's type. Both are generalized together (as
    /// one tuple) so a variable they share becomes the same `Generic` in each.
    fn scheme_and_ctx(&mut self, sig: Aol<Ty>, has_ctx: bool) -> (Type, Option<Type>) {
        self.eng.enter_level();
        let mut tvars = HashMap::new();
        let full = self.ty_of_ast(sig, &mut tvars);
        self.eng.leave_level();
        let (ctx, exposed) = if has_ctx {
            match self.eng.types.node(full) {
                TypeNode::Arrow(from, to, _) => (Some(from), to),
                // A `@ctx` signature is built as an arrow by the parser; an alias
                // could still hide one, in which case there is nothing to strip.
                _ => (None, full),
            }
        } else {
            (None, full)
        };
        let items: Vec<Type> = std::iter::once(exposed).chain(ctx).collect();
        let bundle = self.eng.types.tuple(items);
        self.eng.generalize(bundle);
        let zonked = self.eng.zonk(bundle);
        let TypeNode::Tuple(parts) = self.eng.types.node(zonked) else {
            unreachable!("packed bundle stays a tuple");
        };
        let parts = self.eng.types.items(parts).to_vec();
        (parts[0], parts.get(1).copied())
    }

    /// Elaborate a context-bearing signature at a USE site: fresh variables for
    /// its type variables, split into the context requirement and the type the
    /// caller applies. One map for the whole signature keeps the two aligned, so
    /// `Ito_string t -> t -> @str` ties the requirement to the argument.
    fn ctx_use_type(&mut self, sig: Aol<Ty>) -> (Type, Type) {
        let mut tvars = HashMap::new();
        let full = self.ty_of_ast(sig, &mut tvars);
        match self.eng.types.node(full) {
            TypeNode::Arrow(from, to, _) => (from, to),
            _ => (full, full),
        }
    }

    /// The module that owns a bare global name, so lowering emits a qualified
    /// reference instead of a bare one (which the runtime would read as a builtin).
    fn owner_of(&self, name: &str) -> Option<&'a str> {
        if self.local_defs.contains(name) {
            return Some(self.module_name);
        }
        self.value_module.get(name).copied()
    }

    /// Whether an expression is a literal whose type its construction hook decides
    /// (so checking it against an expected type can build a user type instead).
    fn is_literal(&self, e: Aol<Expr>) -> bool {
        matches!(
            self.node(e),
            Expr::Int(_) | Expr::Real(_) | Expr::Str(_) | Expr::List(_)
        )
    }

    /// Pack an instance's exposed type with its own context requirement, so one
    /// instantiation keeps their variables aligned.
    fn instance_bundle(&mut self, exposed: Type, ctx: Option<Type>) -> Type {
        let items: Vec<Type> = std::iter::once(exposed).chain(ctx).collect();
        self.eng.types.tuple(items)
    }

    /// A context parameter's type must be a declared nominal type applied to its
    /// arguments. A bare variable or a base type would turn the search into "any
    /// value of this type in scope", which is never what is meant, and the error
    /// belongs at the declaration rather than at every call site.
    fn check_ctx_head(&mut self, name: &str, ctx: Aol<Ty>) -> Result<()> {
        let mut tvars = HashMap::new();
        let ty = self.ty_of_ast(ctx, &mut tvars);
        let zonked = self.eng.zonk(ty);
        // A bundle of several contexts travels as a tuple; each component carries
        // the same requirement.
        let parts = match self.eng.types.node(zonked) {
            TypeNode::Tuple(items) => self.eng.types.items(items).to_vec(),
            _ => vec![zonked],
        };
        for part in parts {
            let ok = self.ctx_head(part).is_some_and(|h| self.is_nominal(h));
            if !ok {
                let span = self.ast.ty_span(ctx).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "the `@ctx` parameter of `{name}` must be a declared type, not `{}`",
                    self.show(part)
                )
                .with_note(
                    "a context is found by its type, so it needs a named one; wrap it \
                     in a `@struct` whose fields are the operations"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Whether a type-constructor name is a declared struct or union (as opposed to
    /// a base type like `@int` or an arrow).
    fn is_nominal(&self, head: utilities::StrId) -> bool {
        let name = self.eng.types.name(head).to_string();
        self.structs.contains_key(name.as_str()) || self.unions.contains_key(name.as_str())
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
            self.bind(defs[i].name, v);
            declared.push(v);
        }
        for (&i, decl) in component.iter().zip(&declared) {
            self.check_def_body(&defs[i], *decl)?;
        }
        self.solve_pending()?;
        self.resolve_pending_ctx()?;
        self.eng.leave_level();
        let mono = self.pending_vars();
        for (&i, decl) in component.iter().zip(&declared) {
            self.eng.generalize_except(*decl, &mono);
            types.insert(defs[i].name, self.eng.zonk(*decl));
        }
        Ok(())
    }

    fn check_def_body(&mut self, def: &Def<'a>, decl: Type) -> Result<()> {
        self.ambient = self.top_level_ambient();
        if let Some(sig) = def.sig {
            let mut tvars = HashMap::new();
            let sig_ty = self.ty_of_ast(sig, &mut tvars);
            // The body sees the context parameter (it binds it like any other
            // leading parameter); callers do not, so the definition's own type is
            // the signature with that parameter stripped.
            let exposed = match (def.ctx, self.eng.types.node(sig_ty)) {
                (Some(_), TypeNode::Arrow(from, to, _)) => {
                    self.decl_ctx.insert(def.name, from);
                    to
                }
                _ => sig_ty,
            };
            self.eng.unify(
                decl,
                exposed,
                &format!("against the signature of `{}`", def.name),
            )?;
            let saved_sig = self.current_sig.replace(sig_ty);
            let r = self.check_body_against_sig(def.body, sig, sig_ty);
            self.current_sig = saved_sig;
            r
        } else {
            let inferred = self.infer(def.body)?;
            self.eng.unify(
                decl,
                inferred,
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
        sig_ty: Type,
    ) -> Result<()> {
        // A bare `@extern` takes its whole argument opaquely (a record parameter is
        // marshalled, not destructured into locals), so it is checked against the
        // FULL signature arrow rather than having its record/unit parameter stripped
        // by the sugar below. This keeps the extern node's type the complete arrow,
        // which `build_extern_specs` reads to plan the marshalling.
        if matches!(self.node(body), Expr::Extern { .. }) {
            return self.check(body, sig_ty);
        }
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
                Ty::Record { fields, tail: None } => self.ast.slice(*fields),
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
        self.bind_record_param(fields, param_ty)?;
        // The body may perform this arrow's latent effect (the declared row).
        let saved = std::mem::replace(&mut self.ambient, eff);
        let out = self.check_body_against_sig(body, to, result_ty);
        self.ambient = saved;
        self.leave_scope();
        out
    }

    fn bind_record_param(&mut self, fields: &'a [RecField], param_ty: Type) -> Result<()> {
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
                .map(|(_, t)| *t)
                .unwrap_or_else(|| self.eng.fresh());
            self.bind(name, t);
            if f.with {
                self.scope_struct_fields(t)?;
            }
        }
        Ok(())
    }

    /// Bring the struct's fields into scope, returning their names in declaration
    /// order (empty if `ty` is not a known struct). Lowering keys the `with`
    /// desugaring off these names.
    fn scope_struct_fields(&mut self, ty: Type) -> Result<Vec<String>> {
        let (head, args) = self.spine(ty);
        let mut names = Vec::new();
        if let TypeNode::Con(name) = self.eng.types.node(head) {
            if let Some(info) = self.structs.get(self.eng.types.name(name).as_str()).cloned() {
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
    ///
    /// Fills in the expression's span the way [`Self::infer`] does, so a
    /// diagnostic raised down here points at the offending expression instead of
    /// the start of the file.
    fn check(&mut self, e: Aol<Expr>, expected: Type) -> Result<()> {
        let r = self.check_node(e, expected);
        r.map_err(|d| match self.ast.expr_span(e) {
            Some(span) => d.fill_span(span),
            None => d,
        })
    }

    fn check_node(&mut self, e: Aol<Expr>, expected: Type) -> Result<()> {
        // A literal aimed at a user type may build it through a `@compiler_interface_*`
        // construction hook; when one applies, that supersedes the built-in default.
        if self.literal_hook_check(e, expected)? {
            return Ok(());
        }
        match self.node(e) {
            Expr::Lambda { params, body } => {
                let params = self.ast.slice(*params);
                self.enter_scope();
                let mut exp = expected;
                let mut body_eff = self.ambient;
                let mut tvars = HashMap::new();
                for p in params.iter() {
                    let (param_ty, rest, eff) = self.arrow_parts(exp)?;
                    if let Some(sig) = p.sig {
                        let sig_ty = self.ty_of_ast(sig, &mut tvars);
                        self.eng.unify(
                            param_ty,
                            sig_ty,
                            "in a parameter's type annotation",
                        )?;
                    }
                    self.type_pattern(p.pat, param_ty)?;
                    exp = rest;
                    body_eff = eff; // the innermost arrow's effect: the body's ambient
                }
                let saved = std::mem::replace(&mut self.ambient, body_eff);
                let out = self.check(*body, exp);
                self.ambient = saved;
                self.leave_scope();
                out
            }
            // `[..]` where a sized tensor `[n]T` is expected: the literal's length
            // fixes `n`, and every element is checked against `T`. Lowering builds
            // a vector.
            Expr::List(items) if self.tensor_parts(expected).is_some() => {
                let items = self.ast.slice(*items);
                let (size, elem) = self.tensor_parts(expected).expect("guarded");
                let n = self.eng.types.nat(items.len() as u64);
                self.eng.unify(
                    size,
                    n,
                    "in a tensor literal (its length fixes the size)",
                )?;
                for item in items.iter() {
                    self.check(*item, elem)?;
                }
                self.tensor_exprs.insert(e);
                Ok(())
            }
            // A range `[lo ... hi]` where a sized tensor `[n]T` is expected. The
            // length must be a COMPILE-TIME constant, so the bounds must be literals;
            // it fixes `n = hi - lo + 1` (0 when `hi < lo`). Bounds are checked
            // against the element type, so `[4]Nat = [1 ... 4]` types the ends as Nat.
            Expr::Range { lo, hi } if self.tensor_parts(expected).is_some() => {
                let lo = *lo;
                let Some(hi) = *hi else {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "an open range `[lo ...]` is infinite and cannot build a sized \
                         tensor `[n]T`; it builds a `Stream`"
                    ));
                };
                let (size, elem) = self.tensor_parts(expected).expect("guarded");
                let (Some(l), Some(h)) = (self.int_literal(lo), self.int_literal(hi)) else {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a range building a sized tensor `[n]T` needs literal bounds, \
                         so its length is known at compile time";
                        note: "a compile-time-constant bound (e.g. `let a = 4 in [1 ... a]`) \
                               is not resolved yet; use integer literals, or annotate the \
                               range as a `List` for a runtime-length sequence"
                    ));
                };
                let n = if h >= l { (h - l + 1) as u64 } else { 0 };
                let n = self.eng.types.nat(n);
                self.eng.unify(
                    size,
                    n,
                    "in a range tensor (its bounds fix the length)",
                )?;
                self.check(lo, elem)?;
                self.check(hi, elem)?;
                self.tensor_exprs.insert(e);
                Ok(())
            }
            // Any other range resolves its hook against the expected type, so a
            // user sequence's overload is picked by context.
            Expr::Range { lo, hi } => {
                let (lo, hi) = (*lo, *hi);
                let got = self.range_hook(e, lo, hi, Some(expected))?;
                self.eng.unify(got, expected, "against the expected type")
            }
            // A `.{ .. }` literal (bare or `Type.{ .. }`) checked against an expected
            // type: resolve the struct from the qualifier or the expected type, and
            // pass the expected type down so the parameters are pinned BEFORE fields
            // are checked (bidirectional). A bare positional literal, which carries no
            // field names, depends on this to resolve its struct at all.
            Expr::StructLit { ty, fields, spread } => {
                let (ty, spread) = (*ty, *spread);
                let fields = self.ast.slice(*fields);
                let save = self.eng.save();
                let name = ty.map(|t| self.text(t)).or_else(|| self.struct_name_of(expected));
                let direct = self
                    .infer_struct_lit(e, name, fields, spread, Some(expected))
                    .and_then(|got| {
                        self.eng.unify(got, expected, "against the expected type")
                    });
                let Err(err) = direct else {
                    return Ok(());
                };
                // The literal is not that record itself, so try promoting it INTO the
                // record the way a struct-typed variable already promotes: `diag
                // Point.{ .. }` fills a `{with q: Point}` parameter. Only a closed row
                // has known fields to promote into; anything else keeps the direct
                // mismatch, which is the more useful error.
                if !self.record_is_closed(expected) {
                    return Err(err);
                }
                self.eng.restore(save);
                let got = self.infer_struct_lit(e, ty.map(|t| self.text(t)), fields, spread, None)?;
                self.promote_to_record(e, &[got], expected).map_err(|_| err)
            }
            // A branch does not produce a value of its own, so the expectation
            // passes THROUGH it to each result. Without this, an arm body is only
            // ever inferred, which is why a construction that needs its type from
            // context had to be written `(e : T)` inside `is` or `if`.
            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let tc = self.infer(cond)?;
                let b = self.eng.types.con(ty::BOOL);
                self.eng.unify(tc, b, "in an 'if' condition")?;
                self.check(then, expected)?;
                self.check(alt, expected)
            }
            Expr::Match { scrut, arms } => {
                let ts = self.infer(*scrut)?;
                for arm in self.ast.slice(*arms).iter() {
                    self.enter_scope();
                    let r = (|this: &mut Self| {
                        for pat in this.ast.slice(arm.patterns).iter() {
                            this.type_pattern(*pat, ts)?;
                        }
                        if let Some(guard) = arm.guard {
                            let tg = this.infer(guard)?;
                            let b = this.eng.types.con(ty::BOOL);
                            this.eng.unify(tg, b, "in a match guard")?;
                        }
                        this.check(arm.body, expected)
                    })(self);
                    self.leave_scope();
                    r?;
                }
                self.check_coverage(e, *arms)
            }
            // A `{ .f = e, ... }` literal checked against a declared struct builds
            // THAT struct rather than an anonymous row record, so construction
            // consults the type's own slots. That is what gives a recursive field
            // its laziness however the value is written.
            Expr::Record {
                fields,
                with: None,
                update: None,
            } if self.struct_name_of(expected).is_some()
                && self.ast.slice(*fields).iter().all(|f| matches!(f, FieldInit::Named { .. })) =>
            {
                let name = self.struct_name_of(expected).expect("guarded just above");
                let fields = self.ast.slice(*fields);
                self.infer_struct_lit(e, Some(name), fields, None, Some(expected))?;
                Ok(())
            }
            // A bare `.Tag` takes its union from the expected type (type-directed), so
            // a constructor name shared by several unions resolves unambiguously.
            Expr::Variant { module: None, ty: None, tag, fields }
                if self
                    .union_head_with_tag(expected, self.text(*tag))
                    .is_some() =>
            {
                let tag = self.text(*tag);
                let uname = self
                    .union_head_with_tag(expected, tag)
                    .expect("guarded above");
                let fields = self.ast.slice(*fields);
                let got = self.infer_variant(Some(uname), tag, fields)?;
                self.eng
                    .unify(got, expected, "against the expected type")
            }
            // A `Real` literal takes the expected float width, like an integer
            // literal takes its width: `1.0` checks against `Real32` as well as
            // `Real`/`Real64`. The runtime value stays a `Real` and is narrowed to
            // the field/parameter width at the `@extern` boundary.
            Expr::Real(_) if self.is_float_type(expected) => Ok(()),
            // `@cast x` reinterprets an integer operand at the expected width. It is
            // type-directed: the target integer type comes from the checking context.
            Expr::App(f, arg) if self.is_cast_head(*f) => {
                let arg = *arg;
                self.check_cast(arg, expected)
            }
            // A bare reference to an overloaded name, with no argument types to
            // dispatch on (`(+)` passed as a value, `foldl (+) 0 xs`). The expected
            // type selects the overload, and the choice is recorded at this site so
            // lowering emits the resolved global rather than the bare name. An
            // ambiguity here is deferred like any other, so an expected type that is
            // still an unpinned variable is settled once inference constrains it.
            Expr::Var { module: None, name }
                if self.overloads.contains_key(self.text(*name))
                    && self.lookup(self.text(*name)).is_none()
                    && !self.shadowed_locally(self.text(*name)) =>
            {
                let name = self.text(*name);
                let cands = self.overloads.get(name).cloned().expect("guarded above");
                let got = self.resolve_overload(name, &cands, &[], Some(e))?;
                self.eng.unify(got, expected, "against the expected type")
            }
            _ => {
                let got = self.infer(e)?;
                // Promotion at an argument position: a bare scalar or a positional
                // tuple passed where a record is expected is wrapped into that record
                // (`foo 1` -> `foo { .x = 1 }`, `foo {1,2}` -> `foo { .x=1, .y=2 }`).
                if let TypeNode::Record(_) = self.eng.head(expected) {
                    // Try a direct unification first (a record value, or a nominal
                    // struct via the `Con ~ Record` bridge). Skip it for a numeric
                    // literal, whose undefaulted variable would wrongly unify with
                    // the record. If unification fails, promote a scalar / tuple /
                    // struct into a CLOSED record (an open row has no known fields to
                    // promote into, so a mismatch there is a real error).
                    if !self.is_numeric(got) {
                        let save = self.eng.save();
                        if self.eng.unify(got, expected, "against the expected type").is_ok() {
                            return Ok(());
                        }
                        self.eng.restore(save);
                    }
                    if self.record_is_closed(expected) {
                        let g = self.eng.head(got);
                        let values: Vec<Type> = match g {
                            TypeNode::Tuple(items) => self.eng.types.items(items).to_vec(),
                            _ => vec![got],
                        };
                        return self.promote_to_record(e, &values, expected);
                    }
                }
                self.eng.unify(got, expected, "against the expected type")
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
        record_ty: Type,
    ) -> Result<()> {
        let fields = self.record_fields_of(record_ty)?;
        if fields.len() != values.len() {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "cannot pass {} value(s) as a record with {} field(s)",
                values.len(), fields.len()
            ));
        }
        for (val, (_, fty)) in values.to_vec().into_iter().zip(fields.clone()) {
            self.eng.unify(val, fty, "promoting an argument to a record")?;
        }
        self.promotions
            .insert(site, fields.into_iter().map(|(n, _)| n).collect());
        Ok(())
    }

    /// Whether `ty` is a floating type (`Real`/`Real64`/`Real32`, either spelling),
    /// so a `Real` literal may take its width.
    fn is_float_type(&self, ty: Type) -> bool {
        matches!(
            self.eng.head(ty),
            TypeNode::Con(name)
                if matches!(self.eng.types.name(name).as_str(), "@float64" | "@float32")
        )
    }

    /// Whether `ty` is a sized integer type (any width, signed or unsigned). `@cast`
    /// reinterprets between these; float and non-numeric types are rejected.
    fn is_int_scalar(&self, ty: Type) -> bool {
        matches!(self.eng.head(ty),
            TypeNode::Con(name) if matches!(self.eng.types.name(name).as_str(),
                "@int" | "@nat"
                    | "@int8" | "@int16" | "@int32" | "@int64"
                    | "@nat8" | "@nat16" | "@nat32" | "@nat64"))
    }

    /// Whether `f` is the `@cast` intrinsic in head position.
    fn is_cast_head(&self, f: Aol<Expr>) -> bool {
        matches!(self.node(f), Expr::Var { module: None, name } if self.text(*name) == "@cast")
    }

    /// Check `@cast x` against the expected integer type. Both engines box integers
    /// uniformly, so the cast is erased after checking (lowering emits the operand);
    /// the width matters only at the `@extern` boundary, where marshalling narrows to
    /// the C type. A numeric literal operand is accepted (it defaults to `Int`).
    fn check_cast(&mut self, arg: Aol<Expr>, expected: Type) -> Result<()> {
        if !self.is_int_scalar(expected) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`@cast` converts between integer widths, but the target type here is `{}`, \
                 not a sized integer",
                self.show(expected)
            ));
        }
        let src = self.infer(arg)?;
        // A still-unresolved operand, e.g. the result of a deferred overload (`a +
        // b` whose resolution waits on numeric defaulting) or a bare numeric
        // literal, gets pinned by later solving. The cast is erased, so accept it
        // here rather than reject on an incomplete type.
        if matches!(self.eng.head(src), TypeNode::Var(_)) {
            return Ok(());
        }
        if !self.is_int_scalar(src) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`@cast` expects an integer operand, but got `{}`",
                self.show(src)
            ));
        }
        Ok(())
    }

    /// Decompose a function type into (parameter, result, latent effect). If it is
    /// not yet known to be an arrow, force it to one with fresh parts.
    fn arrow_parts(&mut self, ty: Type) -> Result<(Type, Type, Type)> {
        match self.eng.head(ty) {
            TypeNode::Arrow(from, to, eff) => Ok((from, to, eff)),
            _ => {
                let from = self.eng.fresh();
                let to = self.eng.fresh();
                let eff = self.eng.fresh();
                let want = self.eng.types.arrow_eff(from, to, eff);
                self.eng.unify(ty, want, "expected a function")?;
                Ok((from, to, eff))
            }
        }
    }

    fn pending_vars(&self) -> HashSet<VarId> {
        let mut out = HashSet::new();
        for p in &self.pending {
            for a in &p.args {
                self.eng.collect_vars(*a, &mut out);
            }
            self.eng.collect_vars(p.result, &mut out);
        }
        for (t, _) in &self.numeric {
            self.eng.collect_vars(*t, &mut out);
        }
        // A variable an unresolved context still mentions must stay monomorphic:
        // generalizing it would quantify the very type the search is waiting for,
        // and a `let square = \x = x * x` would then resolve `IArith` against a
        // quantified variable instead of the `@int` its use site supplies.
        for p in &self.ctx_pending {
            self.eng.collect_vars(p.req, &mut out);
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
        collected: Vec<(&'a str, Span)>,
    ) -> Result<Vec<&'a str>> {
        let declared: Vec<&'a str> = declared.iter().map(|p| self.text(*p)).collect();
        for (v, span) in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, false).fill_span(*span));
            }
        }
        Ok(declared)
    }

    /// A declared type's recorded span, or the no-location sentinel.
    fn ty_span_or_none(&self, ty: Aol<Ty>) -> Span {
        self.ast.ty_span(ty).unwrap_or(Span::at(0))
    }

    /// Every type variable used across `tys`, each paired with the span of the
    /// type it was found in, so an undeclared one carets that field or payload
    /// rather than the whole declaration.
    fn tyvars_with_spans(&self, tys: impl IntoIterator<Item = Aol<Ty>>) -> Vec<(&'a str, Span)> {
        let mut out: Vec<(&'a str, Span)> = Vec::new();
        for ty in tys {
            let mut vs = Vec::new();
            collect_tyvars(self.ast, ty, &mut vs);
            let span = self.ty_span_or_none(ty);
            for v in vs {
                if !out.iter().any(|(n, _)| *n == v) {
                    out.push((v, span));
                }
            }
        }
        out
    }

    /// Re-check a struct/union's parameters after its `with` splices are copied in:
    /// the parameters stay as declared, but every type variable in the now-complete
    /// field/variant set, including spliced-in ones, must still be covered.
    fn splice_params(
        &self,
        kind: &str,
        name: &'a str,
        collected: Vec<(&'a str, Span)>,
    ) -> Result<Vec<&'a str>> {
        let declared = self
            .structs
            .get(name)
            .map(|i| i.params.clone())
            .or_else(|| self.unions.get(name).map(|i| i.params.clone()))
            .expect("registered");
        for (v, span) in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, true).fill_span(*span));
            }
        }
        Ok(declared)
    }

    fn register_types(&mut self, program: &Program) -> Result<()> {
        for item in self.ast.slice(program.items).iter() {
            match item {
                Item::Struct {
                    name,
                    params,
                    includes,
                    fields,
                    abi,
                    c_union,
                } => {
                    let (params, includes, fields) = (
                        self.ast.slice(*params),
                        self.ast.slice(*includes),
                        self.ast.slice(*fields),
                    );
                    let collected = self.tyvars_with_spans(fields.iter().map(|f| f.ty));
                    let name = self.text(*name);
                    // A blessed interface is the compiler's own: CORE declares it, and
                    // exactly one field, because a desugar site projects that field
                    // without knowing its name.
                    if crate::parser::table::is_blessed_interface(name) {
                        let at = fields
                            .first()
                            .map_or(Span::at(0), |f| self.ty_span_or_none(f.ty));
                        if self.module_name != "CORE" {
                            return Err(diag!(
                                Code::TypeMismatch, at, 0,
                                "`{name}` is a blessed interface and only CORE may declare it";
                                note: "implement it instead: a value of the applied type \
                                       (`$ r : @IRange @int Span = .{{ .range = ... }}`)"
                            ));
                        }
                        if fields.len() != 1 {
                            return Err(diag!(
                                Code::TypeMismatch, at, 0,
                                "the blessed interface `{name}` must have exactly one field, \
                                 not {}", fields.len()
                            ));
                        }
                    }
                    let params = self.resolve_type_params("struct", name, params, collected)?;
                    let fields = fields.iter().map(|f| (self.text(f.name), f.ty)).collect();
                    let crepr = abi.is_some();
                    self.structs.insert(
                        name,
                        StructInfo {
                            params,
                            fields,
                            crepr,
                            c_union: *c_union,
                        },
                    );
                    self.own_type_names.push(name);
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| (self.text(p.name), p.span)).collect();
                        self.pending_includes.insert(name, (true, ps));
                    }
                }
                Item::Union {
                    name,
                    params,
                    includes,
                    variants,
                } => {
                    let (params, includes, variants) = (
                        self.ast.slice(*params),
                        self.ast.slice(*includes),
                        self.ast.slice(*variants),
                    );
                    let mut payload_tys = Vec::new();
                    let mut vs = Vec::with_capacity(variants.len());
                    for v in variants.iter() {
                        let payload = payload_fields(self.ast, &v.payload);
                        payload_tys.extend(payload.iter().map(|(_, ty)| *ty));
                        vs.push(VariantSig {
                            tag: self.text(v.tag),
                            payload,
                        });
                    }
                    let collected = self.tyvars_with_spans(payload_tys);
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
                        let ps = includes.iter().map(|p| (self.text(p.name), p.span)).collect();
                        self.pending_includes.insert(name, (false, ps));
                    }
                }
                Item::Alias { name, params, ty } => {
                    let params = self.ast.slice(*params);
                    let collected = self.tyvars_with_spans([*ty]);
                    let name = self.text(*name);
                    let params = self.resolve_type_params("alias", name, params, collected)?;
                    self.aliases.insert(name, (params, *ty));
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
        self.compute_lazy_slots();
        Ok(())
    }

    /// Decide which slots hold their value lazily. A slot is lazy exactly when it
    /// can lead back to the type that owns it, so a recursive type can be built
    /// and consumed one cell at a time instead of all at once. Laziness belongs to
    /// the slot, not to the expression that fills it, so it holds however the
    /// value was constructed.
    ///
    /// A C-repr struct is excluded: its runtime value is a flat C struct, and a
    /// thunk cannot cross the `@extern` boundary.
    fn compute_lazy_slots(&mut self) {
        let mut mentions: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
        let mut slot_types: Vec<(&'a str, Option<&'a str>, Vec<Aol<Ty>>)> = Vec::new();
        for (name, info) in &self.structs {
            if info.crepr {
                continue;
            }
            slot_types.push((name, None, info.fields.iter().map(|(_, t)| *t).collect()));
        }
        for (name, info) in &self.unions {
            for v in &info.variants {
                slot_types.push((name, Some(v.tag), v.payload.iter().map(|(_, t)| *t).collect()));
            }
        }
        // An alias is transparent here: a slot typed through one still reaches
        // whatever the alias expands to.
        for (name, (_, ty)) in &self.aliases {
            let mut cons = Vec::new();
            collect_tycons(self.ast, *ty, &mut cons);
            mentions.entry(name).or_default().extend(cons);
        }
        for (owner, _, tys) in &slot_types {
            let entry = mentions.entry(owner).or_default();
            for ty in tys {
                let mut cons = Vec::new();
                collect_tycons(self.ast, *ty, &mut cons);
                for c in cons {
                    if !entry.contains(&c) {
                        entry.push(c);
                    }
                }
            }
        }
        // Transitive closure of "mentions", so a slot typed `B` counts as
        // recursive when `B` leads back to the owner through any chain.
        let mut reach: HashMap<&'a str, HashSet<&'a str>> =
            mentions.iter().map(|(k, v)| (*k, v.iter().copied().collect())).collect();
        loop {
            let mut changed = false;
            let keys: Vec<&'a str> = reach.keys().copied().collect();
            for k in keys {
                let grown: HashSet<&'a str> = reach[k]
                    .iter()
                    .filter_map(|t| reach.get(t))
                    .flatten()
                    .copied()
                    .collect();
                let entry = reach.get_mut(k).expect("key came from this map");
                let before = entry.len();
                entry.extend(grown);
                changed |= entry.len() != before;
            }
            if !changed {
                break;
            }
        }
        let leads_back = |c: &'a str, owner: &'a str| {
            c == owner || reach.get(c).is_some_and(|r| r.contains(owner))
        };
        for (owner, tag, tys) in &slot_types {
            let flags: Vec<bool> = tys
                .iter()
                .map(|ty| {
                    let mut cons = Vec::new();
                    collect_tycons(self.ast, *ty, &mut cons);
                    cons.into_iter().any(|c| leads_back(c, owner))
                })
                .collect();
            if flags.iter().any(|f| *f) {
                self.lazy_slots
                    .insert((owner.to_string(), tag.map(str::to_string)), flags);
            }
        }
    }

    /// Compute and validate the C memory layout of every C-repr struct. Runs after
    /// `with` splicing, so a spliced-in field is laid out too. A C-repr struct may
    /// not be generic (a C type is monomorphic), and each field must be a
    /// C-representable scalar or a nested C-repr struct.
    fn validate_crepr_structs(&mut self) -> Result<()> {
        let names: Vec<&'a str> = self
            .structs
            .iter()
            .filter(|(_, s)| s.crepr)
            .map(|(n, _)| *n)
            .collect();
        for name in names {
            let mut visiting = HashSet::new();
            let layout = self.clayout_of(name, &mut visiting)?;
            self.crepr_layouts.insert(name, layout);
        }
        Ok(())
    }

    /// If `name` is a parameterless alias whose body is a bare type constructor,
    /// the constructor it aliases (`Quaternion` -> `Vector4`), else None. Used to
    /// see through aliases when deciding whether a C-repr field is representable.
    fn nullary_alias_target(&self, name: &str) -> Option<&'a str> {
        let (params, body) = self.aliases.get(name)?;
        if !params.is_empty() {
            return None;
        }
        match self.tnode(*body) {
            Ty::Con { name, .. } => Some(self.text(*name)),
            _ => None,
        }
    }

    /// The C layout of C-repr struct `name`, computed recursively through nested
    /// C-repr struct fields. `visiting` guards against a struct that (transitively)
    /// contains itself by value, which has no finite C layout.
    fn clayout_of(
        &self,
        name: &'a str,
        visiting: &mut HashSet<&'a str>,
    ) -> Result<utilities::CLayout> {
        if !visiting.insert(name) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "C-repr struct `{name}` contains itself by value (an infinite C layout)"
            ));
        }
        let info = self.structs.get(name).expect("crepr struct registered").clone();
        if !info.params.is_empty() {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "a C-repr struct `{name}` may not be generic; a C type is monomorphic"
            ));
        }
        let ptr_bits = utilities::Target::host().ptr_bits();
        let mut fields = Vec::with_capacity(info.fields.len());
        for (fname, fty) in &info.fields {
            let kind = match self.tnode(*fty) {
                Ty::Con { name: cn, .. } => {
                    // A nullary alias to a C-repr type (`typedef Vector4 Quaternion`)
                    // is C-representable: follow the alias chain when the name is
                    // itself neither a scalar nor a registered crepr struct.
                    let mut cn = self.text(*cn);
                    let mut steps = 0;
                    let kind = loop {
                        if let Some(k) = scalar_ckind(cn, ptr_bits) {
                            break k;
                        }
                        if self.structs.get(cn).map(|s| s.crepr).unwrap_or(false) {
                            let layout = self
                                .clayout_of(cn, visiting)
                                .map_err(|d| d.fill_span(self.ty_span_or_none(*fty)))?;
                            break utilities::CKind::Struct(cn.to_string(), layout);
                        }
                        match self.nullary_alias_target(cn) {
                            Some(next) if next != cn && steps < 256 => {
                                cn = next;
                                steps += 1;
                            }
                            _ => {
                                return Err(crepr_field_error(name, fname, cn)
                                    .fill_span(self.ty_span_or_none(*fty)))
                            }
                        }
                    };
                    kind
                }
                _ => {
                    return Err(crepr_field_error(name, fname, "a non-scalar type")
                        .fill_span(self.ty_span_or_none(*fty)))
                }
            };
            fields.push((fname.to_string(), kind));
        }
        visiting.remove(name);
        Ok(if info.c_union {
            utilities::CLayout::of_union(fields)
        } else {
            utilities::CLayout::of(fields)
        })
    }

    /// The C layouts of this module's C-repr structs, keyed by type name.
    pub fn crepr_layouts(&self) -> &HashMap<&'a str, utilities::CLayout> {
        &self.crepr_layouts
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
        // Every failure below is about what a `with` dragged in, so the first
        // include is the fallback caret when the member itself has no span.
        let at_with = includes.first().map_or(Span::at(0), |(_, s)| *s);
        if !visiting.insert(name) {
            let span = at_with;
            return Err(diag!(
                Code::TypeMismatch, span, 0,
                "type `{name}` includes itself (a `with` cycle)"
            ));
        }
        if is_struct {
            let mut fields: Vec<(&'a str, Aol<Ty>)> = Vec::new();
            for (p, span) in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.structs.get(p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, *span, 0,
                        "`{name}` does `with {p}`, which is not a known struct")
                })?;
                for f in &pinfo.fields {
                    if fields.iter().any(|(n, _)| n == &f.0) {
                        return Err(dup_member(name, f.0, "field").fill_span(*span));
                    }
                    fields.push(*f);
                }
            }
            let own = self.structs.get(name).expect("registered").fields.clone();
            for f in own {
                if fields.iter().any(|(n, _)| n == &f.0) {
                    return Err(dup_member(name, f.0, "field").fill_span(self.ty_span_or_none(f.1)));
                }
                fields.push(f);
            }
            let collected = self.tyvars_with_spans(fields.iter().map(|(_, ty)| *ty));
            let params = self.splice_params("struct", name, collected)?;
            let prev = self.structs.get(name).expect("registered");
            let (crepr, c_union) = (prev.crepr, prev.c_union);
            self.structs.insert(
                name,
                StructInfo {
                    params,
                    fields,
                    crepr,
                    c_union,
                },
            );
        } else {
            let mut variants: Vec<VariantSig<'a>> = Vec::new();
            for (p, span) in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.unions.get(p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, *span, 0,
                        "`{name}` does `with {p}`, which is not a known union")
                })?;
                for v in &pinfo.variants {
                    if variants.iter().any(|w| w.tag == v.tag) {
                        return Err(dup_member(name, v.tag, "variant").fill_span(*span));
                    }
                    variants.push(v.clone());
                }
            }
            let own = self.unions.get(name).expect("registered").variants.clone();
            for v in own {
                if variants.iter().any(|w| w.tag == v.tag) {
                    let span = v
                        .payload
                        .first()
                        .map_or(at_with, |(_, ty)| self.ty_span_or_none(*ty));
                    return Err(dup_member(name, v.tag, "variant").fill_span(span));
                }
                variants.push(v);
            }
            let payload_tys: Vec<Aol<Ty>> = variants
                .iter()
                .flat_map(|v| v.payload.iter().map(|(_, ty)| *ty))
                .collect();
            let collected = self.tyvars_with_spans(payload_tys);
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
                    let id = match self.eng.types.node(v) {
                        TypeNode::Var(id) => id,
                        _ => unreachable!("fresh() returns a variable"),
                    };
                    tvars.insert(*p, v);
                    id
                })
                .collect();
            let mut row = self.eng.types.row_empty();
            for (fname, fty) in fields.iter().rev() {
                let f = self.ty_of_ast(*fty, &mut tvars);
                row = self.eng.types.row_field(fname, f, row);
            }
            rows.insert(name.to_string(), (param_ids, row));
        }
        self.eng.set_struct_rows(rows);
    }

    fn register_effects(&mut self, program: &Program) {
        let mut per_op: HashMap<&'a str, Vec<Type>> = HashMap::new();
        for item in self.ast.slice(program.items).iter() {
            let Item::Effect { name, ops } = item else {
                continue;
            };
            let effect = self.text(*name);
            let mut op_schemes = HashMap::new();
            for op in self.ast.slice(*ops).iter() {
                let op_name = self.text(op.name);
                let base = self.scheme_of_sig(op.ty);
                let scheme = self.with_effect(base, effect);
                op_schemes.insert(op_name, scheme);
                per_op.entry(op_name).or_default().push(scheme);
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
        match self.eng.types.node(scheme) {
            TypeNode::Arrow(from, to, _) => {
                let mu = self.eng.fresh_generic();
                let eff = self.eng.types.row_extend(effect, mu);
                self.eng.types.arrow_eff(from, to, eff)
            }
            _ => scheme,
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
                found = Some(*scheme);
            }
        }
        found
    }

    // -- struct / union typing ----------------------------------------------

    fn infer_field(&mut self, rec_ty: Type, field: &str) -> Result<Type> {
        let (head, args) = self.spine(rec_ty);
        if let TypeNode::Con(name) = self.eng.types.node(head) {
            if let Some(info) = self.structs.get(self.eng.types.name(name).as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                if let Some((_, ty)) = info.fields.iter().find(|(n, _)| *n == field) {
                    return Ok(self.ty_of_ast(*ty, &mut subst));
                }
            }
        }
        if let TypeNode::Tuple(items) = self.eng.types.node(head) {
            if let Ok(idx) = field.parse::<usize>() {
                if let Some(t) = self.eng.types.items(items).get(idx) {
                    return Ok(*t);
                }
            }
        }
        // A structural record (an open-row parameter, say): look the field up in
        // the row, growing an open tail to include it.
        if let TypeNode::Record(_) = self.eng.head(rec_ty) {
            return self
                .eng
                .record_field(rec_ty, field, &format!("accessing field `{field}`"));
        }
        // The record's type is not known yet, so CONSTRAIN it to have this field
        // rather than guessing: it becomes an open row, which unification solves
        // (and the `Con ~ Record` bridge still lets a nominal struct satisfy it).
        if let TypeNode::Var(_) = self.eng.head(rec_ty) {
            // A NUMERIC label is ambiguous between a tuple index and a record
            // field, and nothing here can yet say which, so it stays unconstrained
            // until the type is known some other way. This is the one place a
            // field access still fabricates a type.
            if field.parse::<usize>().is_ok() {
                return Ok(self.eng.fresh());
            }
            let fty = self.eng.fresh();
            let tail = self.eng.fresh();
            let row = self.eng.types.row_field(field, fty, tail);
            let rec = self.eng.types.record(row);
            self.eng.unify(rec_ty, rec, &format!("accessing field `{field}`"))?;
            return Ok(fty);
        }
        // Otherwise the type is known and has no such field. Returning a fresh
        // variable here would unify with anything, which is how a typo used to
        // type-check and fault at run time.
        let shown = self.show(rec_ty);
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "`{shown}` has no field `{field}`"
        ))
    }

    fn infer_struct_lit(
        &mut self,
        site: Aol<Expr>,
        ty: Option<&'a str>,
        fields: &'a [FieldInit],
        spread: Option<Aol<Expr>>,
        expected: Option<Type>,
    ) -> Result<Type> {
        let mut struct_name = ty.unwrap_or("");
        let (info, result, mut subst) = if let Some(base) = spread {
            let base_ty = self.infer(base)?;
            let (head, args) = self.spine(base_ty);
            match self.eng.types.node(head) {
                TypeNode::Con(n) if self.structs.contains_key(self.eng.types.name(n).as_str()) => {
                    let (name, info) = self
                        .structs
                        .get_key_value(self.eng.types.name(n).as_str())
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
                    struct_name = name;
                    let (args, subst) = self.instantiate_params(&info.params);
                    (info, applied(&self.eng.types, name, &args), subst)
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

        // Pin the instantiated parameters to the expected type BEFORE checking the
        // fields, so a field's declared type (e.g. `a`) is the actual parameter
        // variable rather than a fresh placeholder. This lets a value-position `@ctx`
        // dictionary in a field resolve by type (`.fst = blank` picks `blank : a`).
        if let Some(exp) = expected {
            self.eng.unify(result, exp, "against the expected type")?;
        }
        let mut covered = vec![false; info.fields.len()];
        for (i, fi) in fields.iter().enumerate() {
            let (slot, value) = match fi {
                // A clause the struct has no slot for is an error: skipping it would
                // let a literal claim a type whose shape it does not have.
                FieldInit::Named { name, value } => {
                    let name = self.text(*name);
                    match info.fields.iter().position(|(n, _)| *n == name) {
                        Some(slot) => (slot, *value),
                        None => {
                            self.infer(*value)?;
                            return Err(diag!(
                                Code::TypeMismatch, Span::at(0), 0,
                                "struct `{struct_name}` has no field `{name}`"
                            ));
                        }
                    }
                }
                FieldInit::Positional(value) if i < info.fields.len() => (i, *value),
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    let n = info.fields.len();
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "struct `{struct_name}` has {n} fields, so there is no field {i}"
                    ));
                }
            };
            covered[slot] = true;
            // Check (not infer) so a `[..]` literal field takes its element/size or
            // Array-ness from the declared field type (bidirectional), like a call arg.
            let want = self.ty_of_ast(info.fields[slot].1, &mut subst);
            self.check(value, want)?;
        }
        // A C union's members share offset 0, so a literal picks exactly one of
        // them; every other literal stands for a whole value and must give every
        // field. An update (`.{ .. | base }`) takes the rest from its base.
        if spread.is_none() {
            if info.c_union {
                let n = fields.len();
                if n != 1 {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "C union `{struct_name}` is built from exactly one member, not {n}"
                    ));
                }
            } else if let Some(slot) = covered.iter().position(|c| !c) {
                let missing = info.fields[slot].0;
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "struct `{struct_name}` literal is missing field `{missing}`";
                    note: "a literal gives every field; to change only some, \
                           update another value with `.{{ .field = v | base }}`"
                ));
            }
        }
        Ok(result)
    }

    fn infer_variant(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &'a [FieldInit],
    ) -> Result<Type> {
        let union = match ty {
            Some(n) => Some(n),
            None => self.find_union_by_tag(tag),
        };
        let resolved = union.and_then(|u| self.variant_sig(u, tag));
        // No fresh() escape hatch, for the same reason `infer_struct_lit` has none:
        // a fresh variable unifies with anything, so an unresolvable constructor
        // would type-check here and fault at run time.
        let Some((result, payload)) = resolved else {
            self.infer_field_inits(fields)?;
            return Err(match ty {
                Some(n) if !self.unions.contains_key(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{n}` is not a union, so it has no constructor `{tag}`"
                ),
                Some(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "union `{n}` has no constructor `{tag}`"
                ),
                None => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "no union has a constructor `{tag}`";
                    note: "a `.Tag` builds a variant of some declared `@union`"
                ),
            });
        };
        let label = variant_label(union, tag);
        let mut covered = vec![false; payload.len()];
        for (i, fi) in fields.iter().enumerate() {
            let (slot, value) = match fi {
                FieldInit::Named { name, value } => {
                    let name = self.text(*name);
                    match payload.iter().position(|(n, _)| *n == Some(name)) {
                        Some(slot) => (slot, *value),
                        None => {
                            self.infer(*value)?;
                            return Err(diag!(
                                Code::TypeMismatch, Span::at(0), 0,
                                "constructor `{label}` has no field `{name}`"
                            ));
                        }
                    }
                }
                FieldInit::Positional(value) if i < payload.len() => (i, *value),
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    let n = payload.len();
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "constructor `{label}` takes {n} field(s), so there is no field {i}"
                    ));
                }
            };
            covered[slot] = true;
            // Check (not infer) against the declared payload type, so the
            // expectation flows into a nested value: a bare `.Tag` payload resolves
            // type-directedly, and a literal takes its construction hook / element type.
            self.check(value, payload[slot].1)?;
        }
        // Every payload slot must be given: a constructor builds a whole value, and
        // a missing slot would leave the variant holding something its type denies.
        if let Some(slot) = covered.iter().position(|c| !c) {
            let n = payload.len();
            let which = match payload[slot].0 {
                Some(name) => format!("`{name}`"),
                None => format!("at index {slot}"),
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "constructor `{label}` takes {n} field(s) and is missing the one {which}"
            ));
        }
        Ok(result)
    }

    fn variant_sig(&mut self, union: &str, tag: &str) -> Option<(Type, VariantPayload<'a>)> {
        let info = self.unions.get(union)?.clone();
        let pos = info.variants.iter().position(|v| v.tag == tag)?;
        let (args, mut subst) = self.instantiate_params(&info.params);
        let result = applied(&self.eng.types, union, &args);
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
        self.unions
            .iter()
            .find_map(|(name, info)| info.variants.iter().any(|v| v.tag == tag).then_some(*name))
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
                let want = self.field_type_of(base_ty, n)?;
                self.eng.unify(*got, want, "in a record update")?;
            }
            return Ok(base_ty);
        }
        if let Some(w) = with {
            // Stack: prepend the explicit fields onto the base's row, keeping the
            // base's tail (so stacking onto an open row stays open).
            let wty = self.infer(w)?;
            let row = self.record_row_of(wty)?;
            let full = explicit
                .into_iter()
                .rev()
                .fold(row, |rest, (n, t)| self.eng.types.row_field(&n, t, rest));
            return Ok(self.eng.types.record(full));
        }
        Ok(self.eng.types.record_of(explicit.into_iter()))
    }

    /// The type of field `name` in a record value: through the row for a structural
    /// record (open tail grows to include it), or the declared field for a struct.
    fn field_type_of(&mut self, base: Type, name: &str) -> Result<Type> {
        if let TypeNode::Record(_) = self.eng.head(base) {
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
    fn record_row_of(&mut self, base: Type) -> Result<Type> {
        if let TypeNode::Record(row) = self.eng.head(base) {
            return Ok(row);
        }
        let fields = self.record_fields_of(base)?;
        Ok(fields
            .into_iter()
            .rev()
            .fold(self.eng.types.row_empty(), |rest, (n, t)| self.eng.types.row_field(&n, t, rest)))
    }

    /// Whether `ty` is a record-shaped target: a structural record row, or a
    /// The nominal struct name `ty` resolves to (possibly applied), if any.
    fn struct_name_of(&self, ty: Type) -> Option<&'a str> {
        let (head, _) = self.spine(ty);
        if let TypeNode::Con(n) = self.eng.types.node(head) {
            if let Some((k, _)) = self.structs.get_key_value(self.eng.types.name(n).as_str()) {
                return Some(k);
            }
        }
        None
    }


    /// The union `ty`'s head names, if it is a (possibly applied) declared union that
    /// has a variant `tag`. Lets a BARE `.Tag` resolve type-directedly against the
    /// expected type, so two unions sharing a constructor name (e.g. a user list and
    /// the builtin `List`, both with `Cons`/`Nil`) do not collide.
    fn union_head_with_tag(&self, ty: Type, tag: &str) -> Option<&'a str> {
        let (head, _) = self.spine(ty);
        let TypeNode::Con(n) = self.eng.types.node(head) else {
            return None;
        };
        let (k, info) = self.unions.get_key_value(self.eng.types.name(n).as_str())?;
        info.variants.iter().any(|v| v.tag == tag).then_some(*k)
    }


    /// The `(label, type)` fields of a record value: a structural [`TypeNode::Record`]
    /// (its closed row) or a nominal struct (its declared fields, instantiated).
    fn record_fields_of(&mut self, ty: Type) -> Result<Vec<(String, Type)>> {
        if let TypeNode::Record(row) = self.eng.head(ty) {
            let mut out = Vec::new();
            let mut cur = row;
            loop {
                match self.eng.head(cur) {
                    TypeNode::RowField(l, fty, rest) => {
                        out.push((self.eng.types.name(l).to_string(), fty));
                        cur = rest;
                    }
                    TypeNode::RowEmpty => return Ok(out),
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
        if let TypeNode::Con(name) = self.eng.types.node(head) {
            if let Some(info) = self.structs.get(self.eng.types.name(name).as_str()).cloned() {
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
            subst.insert(*p, v);
            args.push(v);
        }
        (args, subst)
    }

    fn spine(&self, ty: Type) -> (Type, Vec<Type>) {
        let mut args = Vec::new();
        let mut cur = self.eng.resolve(ty);
        while let TypeNode::App(head, arg) = self.eng.types.node(cur) {
            args.push(self.eng.resolve(arg));
            cur = self.eng.resolve(head);
        }
        args.reverse();
        (cur, args)
    }

    // -- environment --------------------------------------------------------

    /// The type store this checker built its types in, so a caller holding a
    /// `Type` can read it back.
    pub fn types(&self) -> &Types {
        &self.eng.types
    }

    pub fn show(&self, ty: Type) -> String {
        self.eng.show(ty)
    }

    /// The declared type of a global `name` in scope, as a user reads it (with any
    /// `@ctx` prefix). The shell's `:type` uses this for a bare name: inferring the
    /// name as an expression would have to pick a type for its context, which is
    /// exactly what the declaration leaves open.
    pub fn show_name(&mut self, name: &str) -> Option<String> {
        if let Some(sig) = self.global_ctx.get(name).copied() {
            let (exposed, ctx) = self.scheme_and_ctx(sig, true);
            if let Some(ctx) = ctx {
                let parts = self.eng.show_all(&[ctx, exposed]);
                return Some(format!("@ctx {} -> {}", parts[0], parts[1]));
            }
        }
        self.lookup(name).map(|ty| self.show(ty))
    }

    /// How a definition's type reads to a user: its exposed arrow, prefixed with the
    /// `@ctx` parameter when it declares one. A printed type never hides the context
    /// a call site has to satisfy, which is the whole point of #213.
    pub fn show_decl(&self, name: &str, ty: Type) -> String {
        match self.decl_ctx.get(name) {
            Some(ctx) => {
                let parts = self.eng.show_all(&[*ctx, ty]);
                format!("@ctx {} -> {}", parts[0], parts[1])
            }
            None => self.show(ty),
        }
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
                let span = self.ast.expr_span(e).unwrap_or_else(|| Span::at(0));
                self.numeric.push((t, span));
                self.eng.mark_int_literal(t);
                Ok(t)
            }
            Expr::Real(_) => Ok(self.eng.types.con(ty::REAL)),
            Expr::Str(_) => Ok(self.eng.types.con(ty::STR)),
            Expr::Bool(_) => Ok(self.eng.types.con(ty::BOOL)),
            Expr::Unit => Ok(self.eng.types.con(ty::UNIT)),

            Expr::Var { module, name } => {
                let module = module.map(|m| self.text(m));
                let name = self.text(*name);
                self.infer_var(module, name, e)
            }

            Expr::App(..) => self.infer_app(e),

            Expr::Slice { recv, slots } => {
                let (recv, slots) = (*recv, *slots);
                let slots = self.ast.slice(slots);
                self.infer_slice(e, recv, slots)
            }

            Expr::BinOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (self.text(*op), *lhs, *rhs);
                // An operator that takes a context (`(+)` over `IArith t`) plans it
                // at this site, which is where lowering injects it. Its operands
                // share one type, so the side that is a literal is CHECKED against
                // the other's type rather than inferred on its own: that is what
                // lets a literal reach its type-directed construction hook, as in
                // `3.4 + 1.2i`, where the real literal is built as a `Cpx`.
                let ctx_sig = self.global_ctx.get(op).copied();
                if let Some(sig) = ctx_sig {
                    let (req, exposed) = self.ctx_use_type(sig);
                    self.plan_ctx(e, op, req)?;
                    let owner = self.owner_of(op);
                    self.record_call(Some(e), owner);
                    // Take the operand types from the OPERATOR's signature rather
                    // than from each other: that way a literal is checked against
                    // the parameter the operator declares, which is what lets it
                    // reach its type-directed construction hook, and it stays right
                    // for an operator whose operands differ (`x :: xs`).
                    let (tl, tr) = (self.eng.fresh(), self.eng.fresh());
                    let result = self.eng.fresh();
                    let eff = self.eng.fresh();
                    let inner = self.eng.types.arrow_eff(tr, result, eff);
                    let want = self.eng.types.arrow(tl, inner);
                    self.eng
                        .unify(exposed, want, &format!("in operator `{op}`"))?;
                    // The operands that FIX types go first, so a literal sees as
                    // much of the signature as possible.
                    for (operand, param) in [(lhs, tl), (rhs, tr)] {
                        if !self.is_literal(operand) {
                            let t = self.infer(operand)?;
                            self.eng
                                .unify(t, param, &format!("in operator `{op}`"))?;
                        }
                    }
                    for (operand, param) in [(lhs, tl), (rhs, tr)] {
                        if self.is_literal(operand) {
                            self.check(operand, param)?;
                        }
                    }
                    let amb = self.ambient;
                    self.eng
                        .subrow(eff, amb, &format!("in operator `{op}`"))?;
                    return Ok(result);
                }
                let tl = self.infer(lhs)?;
                let tr = self.infer(rhs)?;
                let op_ty = {
                    let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                    self.eng.instantiate(scheme)
                };
                let result = self.eng.fresh();
                // The operator's result arrow may carry a latent effect: `<|` and
                // `|>` pass one through from their function argument (`f <| x` is a
                // call to `f`). Force it into the ambient the way a plain
                // application would. For every other operator this row is empty and
                // the subrow is a no-op.
                let eff = self.eng.fresh();
                let inner = self.eng.types.arrow_eff(tr, result, eff);
                let want = self.eng.types.arrow(tl, inner);
                self.eng
                    .unify(op_ty, want, &format!("in operator `{op}`"))?;
                let amb = self.ambient;
                self.eng
                    .subrow(eff, amb, &format!("in operator `{op}`"))?;
                Ok(result)
            }

            Expr::UnOp { op, operand } => {
                let (op, operand) = (self.text(*op), *operand);
                let t = self.infer(operand)?;
                let op_ty = match self.global_ctx.get(op).copied() {
                    Some(sig) => {
                        let (req, exposed) = self.ctx_use_type(sig);
                        self.plan_ctx(e, op, req)?;
                        let owner = self.owner_of(op);
                        self.record_call(Some(e), owner);
                        exposed
                    }
                    None => {
                        let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                        self.eng.instantiate(scheme)
                    }
                };
                let result = self.eng.fresh();
                let want = self.eng.types.arrow(t, result);
                self.eng.unify(
                    op_ty,
                    want,
                    &format!("in unary `{op}`"),
                )?;
                Ok(result)
            }

            Expr::Tuple(items) => {
                let items = self.ast.slice(*items);
                let mut tys = Vec::with_capacity(items.len());
                for item in items.iter() {
                    tys.push(self.infer(*item)?);
                }
                Ok(self.eng.types.tuple(tys))
            }

            // An unconstrained `[...]` DEFAULTS to a `@vec`, then builds it through
            // the same `@ISeqLit` instance a user sequence uses (CORE's `@vec` one is
            // the identity on the payload).
            Expr::List(items) => {
                let items = self.ast.slice(*items);
                let elem = self.eng.fresh();
                for item in items.iter() {
                    let t = self.infer(*item)?;
                    self.eng.unify(elem, t, "in a sequence literal")?;
                }
                let vec_con = self.eng.types.con(ty::VEC);
                let vec = self.eng.types.app(vec_con, elem);
                if let Some((hook, _)) = self.blessed(IF_SEQ_LIT, &[elem, vec]) {
                    self.literal_hooks.insert(e, hook);
                }
                Ok(vec)
            }

            // A range builds whatever its `@compiler_interface_range` /
            // `_range_from` hook returns (CORE: a `@vec` for `[lo ... hi]`, a
            // `Stream` for the unbounded `[lo ...]`). The sized-tensor target is
            // reached only through `check` against a `[n]T`.
            Expr::Range { lo, hi } => {
                let (lo, hi) = (*lo, *hi);
                self.range_hook(e, lo, hi, None)
            }

            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let tc = self.infer(cond)?;
                let b = self.eng.types.con(ty::BOOL);
                self.eng.unify(tc, b, "in an 'if' condition")?;
                let tt = self.infer(then)?;
                let ta = self.infer(alt)?;
                self.eng
                    .unify(tt, ta, "between the branches of an 'if'")?;
                Ok(tt)
            }

            Expr::Let { bindings, body } => {
                let (bindings, body) = (self.ast.slice(*bindings), *body);
                self.enter_scope();
                self.infer_let_group(bindings)?;
                let t = self.infer(body);
                self.leave_scope();
                t
            }

            Expr::Lambda { params, body } => {
                let (params, body) = (self.ast.slice(*params), *body);
                self.enter_scope();
                let mut param_tys = Vec::with_capacity(params.len());
                let mut tvars = HashMap::new();
                for p in params.iter() {
                    let pv = match p.sig {
                        Some(sig) => self.ty_of_ast(sig, &mut tvars),
                        None => self.eng.fresh(),
                    };
                    self.type_pattern(p.pat, pv)?;
                    param_tys.push(pv);
                }
                // Constructing the closure performs nothing under the current
                // ambient; the body runs under its own fresh ambient, which becomes
                // the innermost arrow's latent effect. Outer (curried, partial-
                // application) arrows stay pure.
                let e_body = self.eng.fresh();
                let saved = std::mem::replace(&mut self.ambient, e_body);
                let body_ty = self.infer(body);
                self.ambient = saved;
                self.leave_scope();
                let body_ty = body_ty?;
                let last = param_tys.len() - 1;
                let ty = param_tys.into_iter().enumerate().rev().fold(
                    body_ty,
                    |acc, (i, p)| {
                        if i == last {
                            self.eng.types.arrow_eff(p, acc, e_body)
                        } else {
                            self.eng.types.arrow(p, acc)
                        }
                    },
                );
                Ok(ty)
            }

            Expr::Match { scrut, arms } => {
                let ts = self.infer(*scrut)?;
                let result = self.eng.fresh();
                for arm in self.ast.slice(*arms).iter() {
                    self.enter_scope();
                    for pat in self.ast.slice(arm.patterns).iter() {
                        self.type_pattern(*pat, ts)?;
                    }
                    if let Some(guard) = arm.guard {
                        let tg = self.infer(guard)?;
                        let b = self.eng.types.con(ty::BOOL);
                        self.eng.unify(tg, b, "in a match guard")?;
                    }
                    let tb = self.infer(arm.body)?;
                    self.eng.unify(result, tb, "between match arms")?;
                    self.leave_scope();
                }
                self.check_coverage(e, *arms)?;
                Ok(result)
            }

            Expr::Field { record, name } => {
                let (record, name) = (*record, self.text(*name));
                let rec_ty = self.infer(record)?;
                self.infer_field(rec_ty, name)
            }
            Expr::StructLit { ty, fields, spread } => {
                let (ty, fields, spread) = (ty.map(|t| self.text(t)), self.ast.slice(*fields), *spread);
                self.infer_struct_lit(e, ty, fields, spread, None)
            }
            Expr::Record {
                fields,
                with,
                update,
            } => {
                let fields = self.ast.slice(*fields);
                self.infer_record(fields, *with, *update)
            }
            Expr::Variant {
                ty, tag, fields, ..
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                let fields = self.ast.slice(*fields);
                self.infer_variant(ty, tag, fields)
            }

            Expr::Array { size } => {
                let size = *size;
                let ts = self.infer(size)?;
                let i = self.eng.types.con(ty::INT);
                self.eng.unify(ts, i, "in an array size")?;
                Ok(self.eng.types.con(ty::ARRAY))
            }
            Expr::With { subject, body } => {
                let (subject, body) = (*subject, *body);
                let subject_ty = self.infer(subject)?;
                self.enter_scope();
                let names = self.scope_struct_fields(subject_ty)?;
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
                self.extern_tys.insert(e, v);
                Ok(v)
            }

            // `(inner : T)`: check `inner` against `T` and take `T` as the type. Any
            // type variable written in `T` is fresh (a local, monomorphic annotation).
            Expr::Ascribe { expr, ty } => {
                let (expr, ty) = (*expr, *ty);
                let mut tvars = HashMap::new();
                let t = self.ty_of_ast(ty, &mut tvars);
                self.check(expr, t)?;
                Ok(t)
            }

            // A `(@ctx e)` reaching inference on its own is misplaced: it is only
            // meaningful as the first argument of an application, which `infer_app`
            // peels off before the head is inferred.
            Expr::CtxArg(_) => Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`(@ctx ...)` must be the FIRST argument of a call to a function that \
                 declares a `@ctx` parameter"
            )),
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

    /// Type a `do body ctl k <clauses> <value arms>` handler. `R` is the result
    /// of the whole handled computation: every clause body and the value arms have
    /// type `R`, and with no value arm the body's own value passes through (`R` is
    /// the body type). In a clause handling `op : Arg -> Res`, the payload `arg` has
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
        let mut inner = self.ambient;
        let mut seen: HashSet<String> = HashSet::new();
        for clause in self.ast.slice(handler.clauses).iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            if let Some(eff_name) = self.op_owner(effect, op) {
                if seen.insert(eff_name.clone()) {
                    inner = self.eng.types.row_extend(&eff_name, inner);
                }
            }
        }
        let saved = std::mem::replace(&mut self.ambient, inner);
        let body_ty = self.infer(body);
        self.ambient = saved;
        let body_ty = body_ty?;

        for clause in self.ast.slice(handler.clauses).iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            let (arg_ty, res_ty) = match self.resolve_op_ty(effect, op) {
                Some(scheme) => {
                    let inst = self.eng.instantiate(scheme);
                    let (a, r, _) = self.arrow_parts(inst)?;
                    (a, r)
                }
                None => (self.eng.fresh(), self.eng.fresh()),
            };
            self.enter_scope();
            self.bind(self.text(clause.arg), arg_ty);
            // Deep handler: resuming continues the computation under the outer
            // ambient, so `k : Res -[amb]-> R`.
            let amb = self.ambient;
            let k_ty = self.eng.types.arrow_eff(res_ty, result, amb);
            self.bind(self.text(handler.continuation), k_ty);
            let cb = self.infer(clause.body)?;
            self.eng.unify(cb, result, "in a handler clause")?;
            self.leave_scope();
        }
        match &handler.value {
            Some((name, value_body)) => {
                self.enter_scope();
                self.bind(self.text(*name), body_ty);
                let vb = self.infer(*value_body)?;
                self.eng.unify(vb, result, "in a handler value arm")?;
                self.leave_scope();
            }
            // With no value arm the body's result becomes the handler's result.
            // When they differ the handler needs a value arm to convert it.
            None => {
                if self.eng.unify(body_ty, result, "in a handled body").is_err() {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "the body produces {}, but the handler's clauses produce {}",
                        self.eng.show(body_ty), self.eng.show(result);
                        note: "with no value arm the body's result is returned unchanged; add a `| x => ...` arm to convert it"
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
        // A blessed interface in VALUE position is a desugar's reference to the single
        // method of the instance the compiler resolves here (`.[i]` emits `@IIndex`,
        // `1.2i` emits `@IImagLit`). Resolution is deferred like any other context, so
        // the surrounding types pin the instance before the search runs.
        if module.is_none() && crate::parser::table::is_blessed_interface(name) {
            let arity = self.structs.get(name).map(|i| i.params.len()).unwrap_or(0);
            let args: Vec<Type> = (0..arity).map(|_| self.eng.fresh()).collect();
            let Some((req, field, field_ty)) = self.blessed_req(name, &args) else {
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeUnbound, span, 0,
                    "`{name}` is not declared";
                    note: "it is a blessed interface the compiler resolves at this site; \
                           CORE declares it as a one-field `@struct`"
                ));
            };
            self.plan_ctx(site, name, req)?;
            self.blessed_sites.insert(site, field);
            return Ok(field_ty);
        }
        if let Some(m) = module {
            if self.imported_private.get(m).is_some_and(|s| s.contains(name)) {
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeUnbound, span, 0,
                    "`{m}.{name}` is private to module `{m}` and cannot be used from another module"
                ));
            }
            // A qualified reference to a `@ctx`-bearing function plans its context
            // just like a bare one, so `LA.dot u v` injects its dictionary rather
            // than staying an under-applied function.
            if let Some(sig) = self.qualified_ctx.get(&(m, name)).copied() {
                let (req, exposed) = self.ctx_use_type(sig);
                self.plan_ctx(site, name, req)?;
                return Ok(exposed);
            }
            return match self.qualified_candidates(m, name) {
                Some(cands) if cands.len() == 1 => Ok(self.eng.instantiate(cands[0])),
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
        // same name): instantiate its signature and requirement type with one shared
        // variable map and plan the context. The returned type is the plain arrow, so
        // callers apply only the explicit parameters.
        if !self.shadowed_locally(name) {
            if let Some(sig) = self.global_ctx.get(name).copied() {
                let (req, exposed) = self.ctx_use_type(sig);
                self.plan_ctx(site, name, req)?;
                return Ok(exposed);
            }
        }
        // Several imports bring this name in. Global scope is flat, so the bare form
        // names nothing; say which modules have it and let the user qualify.
        if !self.shadowed_locally(name) && !self.local_defs.contains(name) {
            if let Some(owners) = self.ambiguous_imports.get(name) {
                let list = owners
                    .iter()
                    .map(|m| format!("`{m}.{name}`"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::AmbiguousName, span, 0,
                    "`{name}` is imported from more than one module";
                    note: "write {list}"
                ));
            }
        }
        if let Some(scheme) = self.lookup(name) {
            Ok(self.eng.instantiate(scheme))
        } else if self.overloads.contains_key(name) {
            Ok(self.eng.fresh())
        } else if self.lenient {
            // A metaprogram-expansion round: the name may be injected by a
            // generator that has not run yet. Defer it as a fresh variable; the
            // final strict round rejects it if it is still unbound.
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

    // -- context (`@ctx`) resolution ----------------------------------------


    /// Plan the context argument of `fname` at use `site`. An explicit `(@ctx e)`
    /// is checked and used at once; otherwise resolution is deferred to the
    /// enclosing definition's boundary, where the requirement's type variables are
    /// pinned (so `max_of 3 7` searches for a context over `@int` rather than over
    /// a bare variable). The binders that could satisfy it are captured now,
    /// because the scopes are gone by then.
    fn plan_ctx(&mut self, site: Aol<Expr>, fname: &str, req: Type) -> Result<()> {
        if let Some(value) = self.ctx_override.remove(&site) {
            self.check(value, req)?;
            self.ctx_args.insert(site, CtxVal::Expr(value));
            return Ok(());
        }
        let locals = self.ctx_locals(req);
        self.ctx_pending.push(PendingCtx {
            site,
            fname: fname.to_string(),
            req,
            sig: self.current_sig,
            locals,
        });
        Ok(())
    }

    /// The binders in scope that could satisfy `req`, innermost scope first. A
    /// binder qualifies only when its type's head constructor is already the
    /// requirement's: a binder whose type is still open is not a candidate, because
    /// matching it would decide its type rather than read it.
    fn ctx_locals(&mut self, req: Type) -> Vec<CtxLocal<'a>> {
        let entries: Vec<(usize, &'a str, Type)> = self
            .scopes
            .iter()
            .enumerate()
            .skip(1)
            .flat_map(|(d, s)| s.iter().map(move |(n, t)| (d, *n, *t)))
            .collect();
        let zonked = self.eng.zonk(req);
        let want = self.ctx_head(zonked);
        if want.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (depth, name, ty) in entries {
            let z = self.eng.zonk(ty);
            if self.ctx_head(z) == want {
                out.push(CtxLocal { depth, name, ty, index: None });
                continue;
            }
            // A binder holding a BUNDLE offers each of its components, so a function
            // that received several contexts at once can pass one of them onward.
            if let TypeNode::Tuple(items) = self.eng.types.node(z) {
                let parts = self.eng.types.items(items).to_vec();
                for (i, part) in parts.into_iter().enumerate() {
                    if self.ctx_head(part) == want {
                        out.push(CtxLocal { depth, name, ty: part, index: Some(i) });
                    }
                }
            }
        }
        out.sort_by(|a, b| b.depth.cmp(&a.depth));
        out
    }

    /// The head constructor of a zonked type: `Ito_string (List t)` has head
    /// `Ito_string`. `None` when the head is not a constructor.
    fn ctx_head(&self, ty: Type) -> Option<utilities::StrId> {
        let mut cur = ty;
        loop {
            match self.eng.types.node(cur) {
                TypeNode::App(h, _) => cur = h,
                TypeNode::Con(n) => return Some(n),
                _ => return None,
            }
        }
    }

    /// Solve every deferred context, at a definition boundary after overload
    /// solving and numeric defaulting have pinned the types.
    fn resolve_pending_ctx(&mut self) -> Result<()> {
        for p in std::mem::take(&mut self.ctx_pending) {
            let req = self.eng.zonk(p.req);
            let rigid: HashSet<VarId> = match p.sig {
                Some(sig) => {
                    let mut vs = HashSet::new();
                    self.eng.collect_vars(self.eng.zonk(sig), &mut vs);
                    vs
                }
                None => HashSet::new(),
            };
            let resolved = self.resolve_ctx(&p.fname, req, &p.locals, &rigid, 0);
            // A metaprogram-expansion round: the instance may be injected by a
            // generator that has not run yet, so leave the site unresolved rather
            // than failing. The final strict round rejects it if it is still missing.
            let val = match resolved {
                Ok(val) => val,
                Err(_) if self.lenient => continue,
                Err(d) => {
                    return Err(match self.ast.expr_span(p.site) {
                        Some(span) => d.fill_span(span),
                        None => d,
                    })
                }
            };
            self.ctx_args.insert(p.site, val);
        }
        Ok(())
    }

    /// Resolve one context requirement. A tuple requirement splits into its
    /// components (several contexts travel as one bundle, built here so no global
    /// of the bundled type has to exist). Otherwise the innermost scope offering
    /// exactly one binder of the type wins, then the instances in scope, of which
    /// exactly one must match because global scope is flat. An instance that itself
    /// takes a context has it resolved here too, bounded by [`CTX_DEPTH`].
    fn resolve_ctx(
        &mut self,
        fname: &str,
        req: Type,
        locals: &[CtxLocal<'a>],
        rigid: &HashSet<VarId>,
        depth: usize,
    ) -> Result<CtxVal> {
        if depth > CTX_DEPTH {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "resolving the context of `{fname}` did not terminate within {CTX_DEPTH} steps"
            )
            .with_note(
                "an instance whose own context leads back to itself cannot be built; \
                 pass the context explicitly instead"
                    .to_string(),
            ));
        }
        if let TypeNode::Tuple(parts) = self.eng.types.node(req) {
            let parts = self.eng.types.items(parts).to_vec();
            let mut vals = Vec::with_capacity(parts.len());
            for part in parts {
                let part = self.eng.zonk(part);
                vals.push(self.resolve_ctx(fname, part, locals, rigid, depth + 1)?);
            }
            return Ok(CtxVal::Tuple(vals));
        }
        let Some(head) = self.ctx_head(req) else {
            return Err(self.ctx_unresolved(fname, req));
        };
        // The requirement's own rigid variables: a candidate may not bind these, so
        // only something that mentions the same variables (the enclosing
        // definition's context parameter) can satisfy a still-open requirement.
        // Which of the requirement's variables a candidate may not bind. For a LOCAL
        // binder that is all of them: a dictionary in scope must satisfy the
        // requirement as written, never specialize it, or an unrelated operator
        // whose type is not pinned yet (`i + 1` inside a function that takes a
        // dictionary) would silently adopt it. The instance search is allowed to
        // pin a flexible variable, which is how a unique instance decides the type
        // of a literal; it may still never touch a variable of the enclosing
        // signature, which belongs to the caller.
        let all: HashSet<VarId> = {
            let mut vs = HashSet::new();
            self.eng.collect_vars(req, &mut vs);
            vs
        };
        let open: HashSet<VarId> = all.intersection(rigid).copied().collect();
        let req_vars = all.len();
        let local_closed: HashSet<VarId> = all.union(rigid).copied().collect();
        let depths: Vec<usize> = {
            let mut ds: Vec<usize> = locals.iter().map(|l| l.depth).collect();
            ds.dedup();
            ds
        };
        for d in depths {
            let hits: Vec<CtxLocal<'a>> = locals
                .iter()
                .filter(|l| l.depth == d)
                .filter(|l| self.ctx_matches(l.ty, req, &local_closed))
                .cloned()
                .collect();
            if hits.len() > 1 {
                let names: Vec<String> = hits.iter().map(|l| l.name.to_string()).collect();
                return Err(self.ctx_ambiguous(fname, req, &names));
            }
            if let Some(l) = hits.first() {
                let (name, ty, index) = (l.name, l.ty, l.index);
                let inst = self.eng.instantiate(ty);
                self.eng.unify(
                    inst,
                    req,
                    &format!("resolving the context of `{fname}`"),
                )?;
                let base = CtxVal::Bare(name.to_string());
                return Ok(match index {
                    Some(i) => CtxVal::Proj(Box::new(base), i),
                    None => base,
                });
            }
        }
        let key = self.eng.types.name(head).to_string();
        let cands = self.instances.get(&key).cloned().unwrap_or_default();
        let mut hits: Vec<Inst<'a>> = Vec::new();
        for c in &cands {
            let save = self.eng.save();
            let inst = self.eng.instantiate(c.bundle);
            let exposed = self.bundle_parts(inst).0;
            let mut ok = self.eng.unify(exposed, req, "resolving a context").is_ok();
            if ok {
                ok = self.keeps_open(&open);
            }
            self.eng.restore(save);
            if ok {
                hits.push(c.clone());
            }
        }
        // With the requirement still open, neither "none in scope" nor "several in
        // scope" is the user's problem: the type is not pinned, so say that instead.
        if req_vars > 0 && hits.len() != 1 {
            return Err(self.ctx_unresolved(fname, req));
        }
        if hits.len() > 1 {
            let names: Vec<String> = hits.iter().map(|c| c.name.clone()).collect();
            return Err(self.ctx_ambiguous(fname, req, &names));
        }
        let Some(c) = hits.first().cloned() else {
            return Err(self.ctx_missing(fname, req));
        };
        let inst = self.eng.instantiate(c.bundle);
        let (exposed, own) = self.bundle_parts(inst);
        self.eng.unify(
            exposed,
            req,
            &format!("resolving the context of `{fname}`"),
        )?;
        let base = match c.module {
            Some(m) => CtxVal::Qualified {
                module: m.to_string(),
                name: c.name.clone(),
            },
            None => CtxVal::Bare(c.name.clone()),
        };
        if !c.has_ctx {
            return Ok(base);
        }
        let own = self.eng.zonk(own.expect("an instance with a context packs it"));
        let arg = self.resolve_ctx(&c.name.clone(), own, locals, rigid, depth + 1)?;
        Ok(CtxVal::App(Box::new(base), Box::new(arg)))
    }

    /// Whether a candidate of type `ty` satisfies `req` without binding any variable
    /// in `closed`. The trial unification is rolled back either way.
    fn ctx_matches(&mut self, ty: Type, req: Type, closed: &HashSet<VarId>) -> bool {
        let save = self.eng.save();
        let inst = self.eng.instantiate(ty);
        let mut ok = self.eng.unify(inst, req, "resolving a context").is_ok();
        if ok {
            ok = self.keeps_open(closed);
        }
        self.eng.restore(save);
        ok
    }

    /// Whether every variable in `closed` is still unbound after a trial
    /// unification. One that has been bound was specialized by the candidate, which
    /// is not this resolution's business: the variable belongs to the caller (a
    /// signature's) or to the requirement itself.
    fn keeps_open(&self, closed: &HashSet<VarId>) -> bool {
        closed.iter().all(|v| self.eng.is_free(*v))
    }

    /// Split an instantiated instance bundle into its exposed type and, when the
    /// instance takes a context of its own, that requirement.
    fn bundle_parts(&self, bundle: Type) -> (Type, Option<Type>) {
        match self.eng.types.node(bundle) {
            TypeNode::Tuple(parts) => {
                let items = self.eng.types.items(parts);
                (items[0], items.get(1).copied())
            }
            _ => (bundle, None),
        }
    }

    /// No value of the requirement's type is in scope.
    fn ctx_missing(&self, fname: &str, req: Type) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no value of type `{}` in scope to satisfy the context of `{fname}`",
            self.show(req)
        )
        .with_note(format!(
            "define or import a value of type `{}`, or pass one explicitly as \
             `{fname} (@ctx value) ...`",
            self.show(req)
        ))
    }

    /// The requirement is still open, so there is nothing to search for. Reported
    /// with both escapes, because neither is obvious from the call alone.
    fn ctx_unresolved(&self, fname: &str, req: Type) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "the context `{}` of `{fname}` is not determined here",
            self.show(req)
        )
        .with_note(
            "annotate the call so its type is known, or declare the same `@ctx` parameter on \
             the enclosing definition so the context is passed in"
                .to_string(),
        )
    }

    /// Several values of the requirement's type are in scope. An annotation cannot
    /// break this tie, since the candidates share a type, so the note offers the
    /// explicit form instead.
    fn ctx_ambiguous(&self, fname: &str, req: Type, names: &[String]) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "several values of type `{}` satisfy the context of `{fname}`: {}",
            self.show(req),
            names.join(", ")
        )
        .with_note(format!(
            "pass the one you mean explicitly as `{fname} (@ctx {}) ...`",
            names.first().map(String::as_str).unwrap_or("value")
        ))
    }


    /// A multi-axis tensor slice `recv.[s0, ...]`. The receiver must be a tensor of
    /// rank >= the slot count; each `Index` slot reduces its axis, each `Range`/`Full`
    /// slot keeps it (a `Range` gets a fresh existential size, modular indexing making
    /// an unknown static size fine). The result wraps the kept axes around whatever
    /// remains below the sliced axes.
    fn infer_slice(
        &mut self,
        site: Aol<Expr>,
        recv: Aol<Expr>,
        slots: &'a [SliceSlot],
    ) -> Result<Type> {
        let rt = self.infer(recv)?;
        // A lone `lo ... hi` over a non-tensor receiver is the `@ISlice` interface,
        // which `@vec` / `@array` / `@str` and any user sequence share. A tensor keeps
        // the path below: its result SHAPE is computed from the slots, which no
        // interface method's type can express.
        if let [SliceSlot::Range(lo, hi)] = slots {
            if self.tensor_parts(rt).is_none() {
                let (lo, hi) = (*lo, *hi);
                let result = self.eng.fresh();
                if let Some((hook, _)) = self.blessed(IF_SLICE, &[rt, result]) {
                    let int = self.eng.types.con(ty::INT);
                    self.check(lo, int)?;
                    self.check(hi, int)?;
                    self.literal_hooks.insert(site, hook);
                    return Ok(result);
                }
            }
        }
        let elem = self.eng.fresh();
        let dims: Vec<Type> = (0..slots.len()).map(|_| self.eng.fresh_nat()).collect();
        // A fresh variance var per sliced axis, so the receiver may have any
        // variance and each kept axis carries its own variance through unchanged.
        let vars: Vec<Type> = (0..slots.len()).map(|_| self.eng.fresh()).collect();
        let mut expected = elem;
        for i in (0..slots.len()).rev() {
            expected = tensor_type(&self.eng.types, vars[i], dims[i], expected);
        }
        self.eng.unify(rt, expected, "slicing a tensor")?;

        let int = self.eng.types.con(ty::INT);
        let mut kept: Vec<(Type, Type)> = Vec::new();
        for (i, s) in slots.iter().enumerate() {
            match s {
                SliceSlot::Index(x) => self.check(*x, int)?,
                SliceSlot::Range(lo, hi) => {
                    self.check(*lo, int)?;
                    self.check(*hi, int)?;
                    kept.push((vars[i], self.eng.fresh_nat()));
                }
                SliceSlot::Full => kept.push((vars[i], dims[i])),
            }
        }
        let mut result = elem;
        for (v, d) in kept.iter().rev() {
            result = tensor_type(&self.eng.types, *v, *d, result);
        }
        Ok(result)
    }

    fn infer_app(&mut self, e: Aol<Expr>) -> Result<Type> {
        let mut args_rev = Vec::new();
        let mut head = e;
        while let Expr::App(f, x) = self.node(head) {
            args_rev.push(*x);
            head = *f;
        }
        args_rev.reverse();
        let mut args = args_rev;

        // An explicit `(@ctx e)` is the first argument. Peel it off and register it
        // against the head's reference site, which is where the context is planned,
        // then let the rest of the call proceed as an ordinary application.
        if let Some(first) = args.first().copied() {
            if let Expr::CtxArg(value) = self.node(first) {
                let value = *value;
                args.remove(0);
                self.ctx_override.insert(head, value);
            }
        }
        if let Some(stray) = args.iter().find(|a| matches!(self.node(**a), Expr::CtxArg(_))) {
            let span = self.ast.expr_span(*stray).unwrap_or_else(|| Span::at(0));
            return Err(diag!(
                Code::TypeMismatch, span, 0,
                "`(@ctx ...)` must be the FIRST argument of the call"
            ));
        }
        let args = args;

        // `@cast` is type-directed: its result width comes from the checking context,
        // handled in `check`. Reaching it here means it has no expected type.
        if self.is_cast_head(head) {
            if args.len() != 1 {
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`@cast` takes exactly one argument"
                ));
            }
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "the target type of `@cast` must be known from context; annotate it \
                 (e.g. `let n : Int = @cast x`) or use it where a specific integer type is expected"
            ));
        }

        // `@e X` / `@run X` runs X at compile time and embeds the result at this
        // site. For a value it is the identity on X's type; for a `@code` fragment
        // the embedded type is only known after the driver splices the code and
        // re-checks, so it is a fresh variable here (which the context binds).
        if let Expr::Var { module: None, name } = self.node(head) {
            let n = self.text(*name);
            if n == "@e" || n == "@run" {
                if args.len() != 1 {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "`{n}` takes exactly one argument"
                    ));
                }
                // `@run` is the eliminator of `<@meta>`: run the operand under a
                // closed `<@meta>` ambient so meta ops discharge here. `@e` runs
                // under a pure ambient, so its operand must be pure (a meta op in
                // it is a "effect `@meta` not handled" error; use `@run`).
                let ambient = if n == "@run" {
                    {
                        let empty = self.eng.types.row_empty();
                        self.eng.types.row_extend("@meta", empty)
                    }
                } else {
                    self.eng.types.row_empty()
                };
                let saved = std::mem::replace(&mut self.ambient, ambient);
                let arg_ty = self.infer(args[0]);
                self.ambient = saved;
                let arg_ty = arg_ty?;
                let z = self.eng.zonk(arg_ty);
                let is_code = matches!(
                    self.eng.types.node(z),
                    TypeNode::Con(cn) if self.eng.types.name(cn) == "@code"
                );
                return Ok(if is_code { self.eng.fresh() } else { arg_ty });
            }
        }

        if let Expr::Var { module: None, name } = self.node(head) {
            let name = self.text(*name);
            if crate::parser::table::is_blessed_interface(name) {
                return self.infer_blessed_app(head, name, &args);
            }
        }

        // An effect operation whose name several effects declare still needs the
        // argument types to pick one (`ask {}` under a `<Reader>` ambient); nothing
        // else dispatches by argument type any more.
        if let Expr::Var { module: None, name } = self.node(head) {
            let name = self.text(*name);
            if !self.shadowed_locally(name) {
                if let Some(cands) = self.overloads.get(name).cloned() {
                    let arg_tys = args
                        .iter()
                        .map(|a| self.infer(*a))
                        .collect::<Result<Vec<_>>>()?;
                    return self.resolve_overload(name, &cands, &arg_tys, Some(head));
                }
            }
        }

        let mut tf = self.infer(head)?;
        for a in &args {
            let (param, result, eff) = self.arrow_parts(tf)?;
            // The callee may perform at most what the ambient allows; performing
            // an operation (whose latent row is `<Effect | mu>`) forces its effect
            // into the ambient here.
            let amb = self.ambient;
            self.eng.subrow(eff, amb, "in a function application")?;
            self.check(*a, param)?;
            tf = result;
        }
        Ok(tf)
    }

    /// The requirement type, single field name, and field type of blessed interface
    /// `name` applied to `args`. `None` when the interface is not declared in scope,
    /// so a desugar site keeps its built-in meaning.
    fn blessed_req(&mut self, name: &str, args: &[Type]) -> Option<(Type, String, Type)> {
        let info = self.structs.get(name)?.clone();
        if info.params.len() != args.len() || info.fields.len() != 1 {
            return None;
        }
        let mut tvars: HashMap<&'a str, Type> = info
            .params
            .iter()
            .copied()
            .zip(args.iter().copied())
            .collect();
        let (field, field_ty) = info.fields[0];
        let field_ty = self.ty_of_ast(field_ty, &mut tvars);
        let mut req = self.eng.types.con(name);
        for a in args {
            req = self.eng.types.app(req, *a);
        }
        Some((req, field.to_string(), field_ty))
    }

    /// Resolve blessed interface `name` applied to `args` at a desugar site: the
    /// instance found by type, plus the type of its single field (what the site's
    /// operands are checked against). A failure leaves the engine untouched and
    /// returns `None`, so the site falls back to its built-in meaning.
    fn blessed(&mut self, name: &str, args: &[Type]) -> Option<(HookImpl, Type)> {
        let save = self.eng.save();
        let Some((req, field, field_ty)) = self.blessed_req(name, args) else {
            self.eng.restore(save);
            return None;
        };
        match self.resolve_blessed(name, req, field) {
            Some(hook) => Some((hook, field_ty)),
            None => {
                self.eng.restore(save);
                None
            }
        }
    }

    /// The instance satisfying an already-built blessed requirement. Unlike
    /// [`Self::blessed`] a failure is NOT rolled back, so callers that have already
    /// committed unifications probe with their own `save`/`restore` first.
    fn resolve_blessed(&mut self, name: &str, req: Type, field: String) -> Option<HookImpl> {
        let locals = self.ctx_locals(req);
        let rigid = HashSet::new();
        self.resolve_ctx(name, req, &locals, &rigid, 0)
            .ok()
            .map(|ctx| HookImpl { ctx, field })
    }

    /// A desugar's blessed-interface call (`.[i]` emits `@IIndex`, `1.2i` emits
    /// `@IImagLit`). The arguments pin the interface's parameters through its
    /// method's declared type; the instance resolves here when that is enough, and
    /// otherwise at the enclosing definition's boundary, once the surrounding types
    /// pin what is left.
    fn infer_blessed_app(
        &mut self,
        head: Aol<Expr>,
        name: &'a str,
        args: &[Aol<Expr>],
    ) -> Result<Type> {
        let arity = self.structs.get(name).map(|i| i.params.len()).unwrap_or(0);
        let vars: Vec<Type> = (0..arity).map(|_| self.eng.fresh()).collect();
        let Some((req, field, method)) = self.blessed_req(name, &vars) else {
            let span = self.ast.expr_span(head).unwrap_or_else(|| Span::at(0));
            return Err(diag!(
                Code::TypeUnbound, span, 0,
                "`{name}` is not declared";
                note: "it is a blessed interface the compiler resolves at this site; \
                       CORE declares it as a one-field `@struct`"
            ));
        };
        let mut rest = method;
        for a in args {
            let (param, next, eff) = self.arrow_parts(rest)?;
            let amb = self.ambient;
            self.eng.subrow(eff, amb, "in a function application")?;
            self.check(*a, param)?;
            rest = next;
        }
        let probe = self.eng.save();
        let zonked = self.eng.zonk(req);
        let resolvable = self
            .resolve_blessed(name, zonked, field.clone())
            .is_some();
        self.eng.restore(probe);
        if resolvable {
            let zonked = self.eng.zonk(req);
            let hook = self
                .resolve_blessed(name, zonked, field)
                .expect("the probe above resolved it");
            self.ctx_args.insert(head, hook.ctx);
            self.blessed_sites.insert(head, hook.field);
            return Ok(self.eng.zonk(rest));
        }
        self.plan_ctx(head, name, req)?;
        self.blessed_sites.insert(head, field);
        Ok(rest)
    }

    /// Every instance of blessed interface `name` in scope, as the diagnostic that
    /// lists what a range can build needs them.
    fn blessed_instances(&self, name: &str) -> Vec<Inst<'a>> {
        self.instances.get(name).cloned().unwrap_or_default()
    }

    /// The head constructor of `ty` if it is a user-declared type (struct / union /
    /// alias), following an application spine (`MyVec Int` -> `MyVec`). A
    /// builtin, a variable, a tuple, or a function returns `None`, so a construction
    /// hook only ever intercepts a literal aimed at a user type.
    fn user_type_head(&self, ty: Type) -> Option<String> {
        let mut cur = self.eng.resolve(ty);
        loop {
            match self.eng.types.node(cur) {
                TypeNode::App(head, _) => cur = self.eng.resolve(head),
                TypeNode::Con(name) => {
                    let n = self.eng.types.name(name);
                    let k = n.as_str();
                    return (self.structs.contains_key(k)
                        || self.unions.contains_key(k)
                        || self.aliases.contains_key(k))
                    .then_some(n);
                }
                _ => return None,
            }
        }
    }

    /// Route a literal (`"..."`, an int, a real, `[..]`) through the
    /// `@compiler_interface_*` construction hook whose result type is the expected
    /// one. On a unique match it unifies, checks any element types, records the hook
    /// at `site` for lowering, and returns `true`; otherwise it leaves the engine
    /// untouched and returns `false`, so the literal falls back to its built-in
    /// default (which lowering folds to a plain constant).
    fn literal_hook_check(&mut self, e: Aol<Expr>, expected: Type) -> Result<bool> {
        // The target type must already have a constructor head: with it still open,
        // resolving would DECIDE what the literal is rather than read it.
        let zonked = self.eng.zonk(expected);
        if self.ctx_head(zonked).is_none() {
            return Ok(false);
        }
        let (name, args, elem) = match self.node(e) {
            Expr::Str(_) => (IF_STR_LIT, vec![expected], None),
            Expr::Int(_) => (IF_INT_LIT, vec![expected], None),
            Expr::Real(_) => (IF_REAL_LIT, vec![expected], None),
            Expr::List(_) => {
                let elem = self.eng.fresh();
                (IF_SEQ_LIT, vec![elem, expected], Some(elem))
            }
            _ => return Ok(false),
        };
        let Some((hook, _)) = self.blessed(name, &args) else {
            return Ok(false);
        };
        // A sequence literal checks each element against the instance's element type.
        if let (Expr::List(items), Some(elem)) = (self.node(e), elem) {
            let items = self.ast.slice(*items).to_vec();
            for it in items {
                self.check(it, elem)?;
            }
        }
        self.literal_hooks.insert(e, hook);
        Ok(true)
    }

    /// Resolve a range (`[lo ... hi]` / `[lo ...]`) through its `@IRange` /
    /// `@IRangeFrom` instance: the bounds are the instance method's arguments, and
    /// what it returns is the range's type. Records the instance at `site` so
    /// lowering projects and applies it. `expected`, when known, takes part in the
    /// resolution, so a second instance is picked by context.
    fn range_hook(
        &mut self,
        site: Aol<Expr>,
        lo: Aol<Expr>,
        hi: Option<Aol<Expr>>,
        expected: Option<Type>,
    ) -> Result<Type> {
        let name = if hi.is_some() { IF_RANGE } else { IF_RANGE_FROM };
        let bound = self.infer(lo)?;
        if let Some(hi) = hi {
            let t = self.infer(hi)?;
            self.eng.unify(bound, t, "between a range's bounds")?;
        }
        let result = self.eng.fresh();
        if let Some(exp) = expected {
            let save = self.eng.save();
            if self.eng.unify(result, exp, "in a range").is_err() {
                self.eng.restore(save);
            }
        }
        // With nothing to pin the result, several instances fit. Rather than call that
        // ambiguous, the FIRST one declared is the default, so CORE's declaration
        // order sets what a bare range builds: a `@vec` for `[lo ... hi]`, a `Stream`
        // for `[lo ...]`. An annotation or a known expected type still picks another.
        let hook = self
            .blessed(name, &[bound, result])
            .or_else(|| self.blessed_first(name, &[bound, result]));
        let Some((hook, field_ty)) = hook else {
            let offered = self
                .blessed_instances(name)
                .iter()
                .map(|i| i.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let want = self.show(result);
            let note = if offered.is_empty() {
                format!("`{name}` is declared in CORE; a range builds whatever its instance returns")
            } else {
                format!("the `{name}` instances in scope are: {offered}")
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "no `{name}` instance builds a `{want}` from this range";
                note: "{note}"
            ));
        };
        // The method's own type decides how the bounds are typed, so a range over a
        // non-`@int` bound works exactly as the instance declares it.
        let mut rest = field_ty;
        for arg in std::iter::once(lo).chain(hi) {
            let (param, next, _) = self.arrow_parts(rest)?;
            self.check(arg, param)?;
            rest = next;
        }
        self.eng.unify(rest, result, "in a range")?;
        self.literal_hooks.insert(site, hook);
        Ok(self.eng.zonk(result))
    }

    /// The FIRST blessed instance of `name` that satisfies the requirement,
    /// committing it. Used as the default when nothing constrains the result and
    /// several instances fit; the strict unique-match rule is [`Self::blessed`].
    fn blessed_first(&mut self, name: &str, args: &[Type]) -> Option<(HookImpl, Type)> {
        for inst in self.blessed_instances(name) {
            let save = self.eng.save();
            let Some((req, field, field_ty)) = self.blessed_req(name, args) else {
                self.eng.restore(save);
                return None;
            };
            let packed = self.eng.instantiate(inst.bundle);
            let (exposed, own) = self.bundle_parts(packed);
            if self.eng.unify(exposed, req, "resolving a range").is_err() {
                self.eng.restore(save);
                continue;
            }
            let base = match inst.module {
                Some(m) => CtxVal::Qualified {
                    module: m.to_string(),
                    name: inst.name.clone(),
                },
                None => CtxVal::Bare(inst.name.clone()),
            };
            let ctx = if inst.has_ctx {
                let own = self.eng.zonk(own.expect("an instance with a context packs it"));
                let locals = self.ctx_locals(own);
                match self.resolve_ctx(&inst.name, own, &locals, &HashSet::new(), 0) {
                    Ok(arg) => CtxVal::App(Box::new(base), Box::new(arg)),
                    Err(_) => {
                        self.eng.restore(save);
                        continue;
                    }
                }
            } else {
                base
            };
            return Some((HookImpl { ctx, field }, field_ty));
        }
        None
    }

    /// The blessed construction interface for a literal PATTERN kind
    /// (`Str`/`Int`/`Real`), so a literal pattern reuses the same builder a literal
    /// expression does.
    fn pattern_literal_hook(pat: &Pattern) -> Option<&'static str> {
        Some(match pat {
            Pattern::Str(_) => IF_STR_LIT,
            Pattern::Int(_) => IF_INT_LIT,
            Pattern::Real(_) => IF_REAL_LIT,
            _ => return None,
        })
    }

    /// Route a sequence pattern (`is [a, b, ..r]`, `is h :: t`, `is []`) whose
    /// scrutinee is a user type through that type's `@compiler_interface_sequence_view`
    /// hook. On success records the hook at `pat` and returns `Some(element type)` so
    /// the caller types the sub-patterns; otherwise returns `None` (built-in typing).
    fn sequence_pattern_hook_check(
        &mut self,
        pat: Aol<Pattern>,
        expected: Type,
    ) -> Result<Option<Type>> {
        let elem = self.eng.fresh();
        let Some((hook, _)) = self.blessed(IF_SEQ_VIEW, &[expected, elem]) else {
            return Ok(None);
        };
        self.sequence_pattern_hooks.insert(pat, hook);
        Ok(Some(elem))
    }

    /// The element type of a sequence PATTERN's scrutinee. A user sequence type (or a
    /// scrutinee already fixed to `@vec`) resolves via its `sequence_view` hook;
    /// otherwise the scrutinee DEFAULTS to `@vec elem` and resolves through `@vec`'s
    /// view. Records the hook at `pat` either way.
    fn sequence_pattern_elem(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<Type> {
        if let Some(elem) = self.sequence_pattern_hook_check(pat, expected)? {
            return Ok(elem);
        }
        let elem = self.eng.fresh();
        let vec_con = self.eng.types.con(ty::VEC);
        let want = self.eng.types.app(vec_con, elem);
        self.eng.unify(expected, want, "in a sequence pattern")?;
        Ok(self.sequence_pattern_hook_check(pat, expected)?.unwrap_or(elem))
    }

    /// Route a literal pattern (`is "foo"`, `is 42`) whose scrutinee is a user type
    /// through that type's construction + equality hooks: it matches by building the
    /// literal into the user type and comparing with `@compiler_interface_equality`.
    /// On success records both hooks at `pat` for lowering and returns `true`;
    /// otherwise leaves the engine untouched and returns `false` (the caller applies
    /// the built-in literal-pattern typing).
    fn literal_pattern_hook_check(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<bool> {
        if self.user_type_head(expected).is_none() {
            return Ok(false);
        }
        let Some(build_name) = Self::pattern_literal_hook(self.pnode(pat)) else {
            return Ok(false);
        };
        let save = self.eng.save();
        let build = self.blessed(build_name, &[expected]);
        let eq = self.blessed(IF_EQ, &[expected]);
        match (build, eq) {
            (Some((build, _)), Some((eq, _))) => {
                self.literal_pattern_hooks.insert(pat, (build, eq));
                Ok(true)
            }
            _ => {
                self.eng.restore(save);
                Ok(false)
            }
        }
    }




    fn resolve_overload(
        &mut self,
        name: &str,
        candidates: &[Cand<'a>],
        args: &[Type],
        site: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let result = self.eng.fresh();
        match self.match_overload(candidates, args, result) {
            Match::Unique(idx) => {
                let cand = candidates[idx].clone();
                self.apply_overload(cand.ty, args, result)?;
                let module = candidates[idx].module;
                self.record_call(site, module);
                Ok(result)
            }
            // In a lenient expansion round the matching overload may be injected
            // by a generator that has not run yet (a derived `to_string` for a new
            // type); leave the result an unresolved variable rather than erroring.
            Match::None if self.lenient => Ok(result),
            Match::None => Err(self.no_overload(name, args, site)),
            Match::Ambiguous => {
                self.pending.push(Pending {
                    name: name.to_string(),
                    candidates: candidates.to_vec(),
                    args: args.to_vec(),
                    result: result,
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

    /// Which candidate the argument types select: exactly one is a match, none is a
    /// miss, several is an ambiguity the caller defers.
    fn match_overload(&mut self, candidates: &[Cand<'a>], args: &[Type], result: Type) -> Match {
        let mut matched = None;
        let mut count = 0;
        for (idx, cand) in candidates.iter().enumerate() {
            let save = self.eng.save();
            let ok = self.apply_overload(cand.ty, args, result).is_ok();
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
                match self.match_overload(&p.candidates, &p.args, p.result) {
                    Match::Unique(idx) => {
                        let cand = p.candidates[idx].clone();
                        self.apply_overload(cand.ty, &p.args, p.result)?;
                        let module = p.candidates[idx].module;
                        self.record_call(p.site, module);
                        progress = true;
                    }
                    // Lenient round: a still-unmatched overload may be satisfied by
                    // an about-to-be-injected definition; drop it rather than error.
                    Match::None if self.lenient => {}
                    Match::None => return Err(self.no_overload(&p.name, &p.args, p.site)),
                    Match::Ambiguous => still.push(p),
                }
            }
            self.pending = still;
            if progress {
                continue;
            }
            if self.propagate_result_to_operands()? {
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
                let err = match p.site.and_then(|s| self.ast.expr_span(s)) {
                    Some(span) => err.fill_span(span),
                    None => err,
                };
                return Err(err);
            }
            return Ok(());
        }
    }

    /// Break an ambiguity where a pending overload's RESULT is already a concrete
    /// integer type but its operands are still unconstrained numeric literals. For
    /// homogeneous arithmetic (`@int32 + @int32 -> @int32`), the operands should
    /// take the result type, not default to `@int`; e.g. `let x : @int32 = 2 + 3`
    /// resolves to the `@int32` overload rather than failing because both literals
    /// became `@int`. Each candidate is tried under that constraint and kept only if
    /// it makes the overload unique, so a genuinely wrong guess is rolled back.
    fn propagate_result_to_operands(&mut self) -> Result<bool> {
        let batch = std::mem::take(&mut self.pending);
        let mut progress = false;
        let mut still = Vec::new();
        for p in batch {
            let result = self.eng.resolve(p.result);
            if !self.is_int_scalar(result) {
                still.push(p);
                continue;
            }
            let save = self.eng.save();
            let mut pinned = true;
            for a in &p.args {
                if matches!(self.eng.head(*a), TypeNode::Var(_))
                    && self
                        .eng
                        .unify(*a, result, "numeric operand adopting result type")
                        .is_err()
                {
                    pinned = false;
                    break;
                }
            }
            if pinned {
                if let Match::Unique(idx) = self.match_overload(&p.candidates, &p.args, p.result) {
                    let cand_ty = p.candidates[idx].ty;
                    self.apply_overload(cand_ty, &p.args, p.result)?;
                    let module = p.candidates[idx].module;
                    self.record_call(p.site, module);
                    progress = true;
                    continue;
                }
            }
            self.eng.restore(save);
            still.push(p);
        }
        self.pending = still;
        Ok(progress)
    }

    /// Whether `ty` is a record whose row is closed (ends in `RowEmpty`, so its
    /// fields are fully known) rather than open (a tail variable).
    fn record_is_closed(&self, ty: Type) -> bool {
        let TypeNode::Record(row) = self.eng.head(ty) else {
            return false;
        };
        let mut cur = row;
        loop {
            match self.eng.head(cur) {
                TypeNode::RowField(_, _, rest) => cur = rest,
                TypeNode::RowEmpty => return true,
                _ => return false,
            }
        }
    }

    /// Whether `ty` resolves to a not-yet-defaulted numeric-literal variable.
    fn is_numeric(&self, ty: Type) -> bool {
        let r = self.eng.resolve(ty);
        matches!(self.eng.types.node(r), TypeNode::Var(_))
            && self.numeric.iter().any(|(n, _)| self.eng.resolve(*n) == r)
    }

    fn default_numerics(&mut self) -> Result<bool> {
        let vars = std::mem::take(&mut self.numeric);
        let mut changed = false;
        for (t, span) in &vars {
            match self.eng.head(*t) {
                // Still unconstrained: default to `Int`.
                TypeNode::Var(_) => {
                    self.eng
                        .unify(*t, self.eng.types.con(ty::INT), "defaulting an integer literal")?;
                    changed = true;
                }
                // Pinned to a numeric type by use: fine.
                TypeNode::Con(name) if is_numeric_type(&self.eng.types.name(name)) => {}
                // Pinned to a genuinely UNKNOWN con (a typo'd type): let that type's
                // own "unknown type" diagnostic surface instead of the numeric error.
                TypeNode::Con(name) if !self.is_known_type(&self.eng.types.name(name)) => {}
                // Pinned to a known non-numeric type (e.g. `if 1` wants `@bool`): a
                // bare literal is a number, so this is a type error, not a coercion.
                _ => {
                    let shown = self.show(*t);
                    return Err(diag!(
                        Code::TypeMismatch, *span, 0,
                        "a numeric literal cannot be used where `{}` is expected",
                        shown
                    ));
                }
            }
        }
        Ok(changed)
    }

    fn no_overload(&self, name: &str, args: &[Type], site: Option<Aol<Expr>>) -> Diagnostic {
        let shown: Vec<String> = args.iter().map(|a| self.show(*a)).collect();
        let mut d = diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no viable overload of `{name}` for argument types ({})",
            shown.join(", ")
        );
        if let Some(span) = site.and_then(|s| self.ast.expr_span(s)) {
            d = d.fill_span(span);
        }
        d
    }

    fn apply_overload(&mut self, candidate: Type, args: &[Type], result: Type) -> Result<()> {
        let mut f = self.eng.instantiate(candidate);
        for a in args {
            let next = self.eng.fresh();
            let eff = self.eng.fresh();
            let want = self.eng.types.arrow_eff(*a, next, eff);
            self.eng.unify(f, want, "in an overloaded application")?;
            // An effectful operation resolved by overload still injects its effect
            // into the ambient (same as a plain call; see `infer_app`).
            let amb = self.ambient;
            self.eng.subrow(eff, amb, "in an overloaded application")?;
            f = next;
        }
        self.eng.unify(f, result, "in an overloaded application")
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
                self.bind(name, v);
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
                self.check(b.value, sig_ty)?;
                sig_ty
            }
            None => self.infer(b.value)?,
        };
        if let Some(decl) = &declared {
            self.eng
                .unify(*decl, value_ty, "in a recursive 'let' binding")?;
        }
        self.eng.leave_level();
        let mono = self.pending_vars();
        match declared {
            Some(decl) => self.eng.generalize_except(decl, &mono),
            None => {
                self.eng.generalize_except(value_ty, &mono);
                self.type_pattern(b.pat, value_ty)?;
            }
        }
        Ok(())
    }

    // -- pattern typing -----------------------------------------------------

    pub fn type_pattern(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<()> {
        match self.pnode(pat) {
            Pattern::Wild => Ok(()),
            Pattern::Var(name) => {
                self.bind(self.text(*name), expected);
                Ok(())
            }
            Pattern::Int(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Real(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Str(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Int(_) => {
                self.eng
                    .unify(expected, self.eng.types.con(ty::INT), "in an integer pattern")
            }
            Pattern::Real(_) => self
                .eng
                .unify(expected, self.eng.types.con(ty::REAL), "in a real pattern"),
            Pattern::Str(_) => self
                .eng
                .unify(expected, self.eng.types.con(ty::STR), "in a string pattern"),
            Pattern::Bool(_) => {
                self.eng
                    .unify(expected, self.eng.types.con(ty::BOOL), "in a boolean pattern")
            }
            Pattern::Range { lo, hi } => {
                // Both bounds are typed against the scrutinee, so a range forces its
                // scalar (`1 ... 5` -> Int, `1.0 ... 5.0` -> Real) and rejects mixed
                // bounds. An open range `lo ...` types only its lower bound. Binds
                // nothing.
                let (lo, hi) = (*lo, *hi);
                self.type_pattern(lo, expected)?;
                if let Some(hi) = hi {
                    self.type_pattern(hi, expected)?;
                }
                Ok(())
            }
            Pattern::StrPrefix { rest, .. } => {
                let rest = *rest;
                self.eng
                    .unify(expected, self.eng.types.con(ty::STR), "in a string-prefix pattern")?;
                self.type_pattern(rest, self.eng.types.con(ty::STR))
            }
            Pattern::Tuple(pats) => {
                let pats = self.ast.slice(*pats);
                let vars: Vec<Type> = pats.iter().map(|_| self.eng.fresh()).collect();
                let want = self.eng.types.tuple(vars.clone());
                self.eng.unify(expected, want, "in a tuple pattern")?;
                for (p, v) in pats.iter().zip(&vars) {
                    self.type_pattern(*p, *v)?;
                }
                Ok(())
            }
            Pattern::Cons { head, tail } => {
                // `h :: t` matches any sequence via its `sequence_view` hook (a user
                // type, else the default `@vec`); the tail keeps the scrutinee's type.
                let (head, tail) = (*head, *tail);
                let elem = self.sequence_pattern_elem(pat, expected)?;
                self.type_pattern(head, elem)?;
                self.type_pattern(tail, expected)
            }
            Pattern::List { elems, rest } => {
                // `[a, b, ..r]` / `[]` matches any sequence via its `sequence_view` hook
                // (a user type, else the default `@vec`): each element at the view's
                // element type, `..rest` at the sequence type.
                let elem = self.sequence_pattern_elem(pat, expected)?;
                let (elems, rest) = (self.ast.slice(*elems), *rest);
                for e in elems.iter() {
                    self.type_pattern(*e, elem)?;
                }
                if let Some(rest) = rest {
                    self.type_pattern(rest, expected)?;
                }
                Ok(())
            }
            Pattern::Struct { ty, fields } => {
                let ty = self.text(*ty);
                let fields = self.ast.slice(*fields);
                self.type_struct_pattern(ty, fields, expected)
            }
            Pattern::Record { fields, rest } => {
                let fields = self.ast.slice(*fields);
                self.type_record_pattern(fields, *rest, expected)
            }
            Pattern::Variant {
                ty, tag, fields, ..
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                let fields = self.ast.slice(*fields);
                self.type_variant_pattern(pat, ty, tag, fields, expected)
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
        expected: Type,
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
            .fold(tail, |rest, (n, t, _)| self.eng.types.row_field(n, *t, rest));
        let want = self.eng.types.record(row);
        self.eng.unify(expected, want, "in a record pattern")?;
        for (name, t, pat) in entries {
            match pat {
                Some(p) => self.type_pattern(p, t)?,
                None => self.bind(name, t),
            }
        }
        // `..name` binds the leftover fields as a record over the row tail (which
        // unification has bound to the remaining fields); `.._` is a discard.
        if let Some(r) = rest {
            let rec = self.eng.types.record(tail);
            self.type_pattern(r, rec)?;
        }
        Ok(())
    }

    fn type_struct_pattern(
        &mut self,
        ty: &'a str,
        fields: &'a [FieldPat],
        expected: Type,
    ) -> Result<()> {
        // No loose fallback: binding a pattern's fields to fresh variables without
        // unifying against the scrutinee would let the arm claim any type it liked,
        // so an unresolvable pattern is an error, not an unconstrained one.
        let Some(info) = self.structs.get(ty).cloned() else {
            let what = if self.unions.contains_key(ty) {
                "a union, so match its variants (`{ty}.Tag.{{ .. }}`)".to_string()
            } else if self.is_known_type(ty) {
                "not a struct".to_string()
            } else {
                "not a declared type".to_string()
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`{ty}.{{ .. }}` is not a struct pattern: `{ty}` is {what}"
            ));
        };
        let (args, mut subst) = self.instantiate_params(&info.params);
        self.eng
            .unify(expected, applied(&self.eng.types, ty, &args), "in a struct pattern")?;
        // A pattern may bind fewer fields than the struct has, but never more: a
        // binder with no field behind it names nothing and faults when read.
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let name = self.text(*name);
                    let want = self.struct_field_ty(&info, &mut subst, Some(name), i);
                    let want = want.ok_or_else(|| no_such_field("struct", ty, name))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = self.struct_field_ty(&info, &mut subst, None, i);
                    let n = info.fields.len();
                    let want = want.ok_or_else(|| no_field_at("struct", ty, i, n))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = self.text(*name);
                    let want = self.struct_field_ty(&info, &mut subst, Some(name), i);
                    let want = want.ok_or_else(|| no_such_field("struct", ty, name))?;
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }



    fn type_variant_pattern(
        &mut self,
        pat: Aol<Pattern>,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &'a [FieldPat],
        expected: Type,
    ) -> Result<()> {
        let union = match ty {
            Some(n) => Some(n),
            None => self.find_union_by_tag(tag),
        };
        let resolved = union.and_then(|u| self.variant_sig(u, tag));
        // As in `type_struct_pattern`, an unresolvable tag is an error rather than
        // an arm that binds fresh variables and constrains nothing.
        let Some((result, payload)) = resolved else {
            return Err(match ty {
                Some(n) if !self.unions.contains_key(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{n}` is not a union, so it has no variant `{tag}`"
                ),
                Some(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "union `{n}` has no variant `{tag}`"
                ),
                None => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "no union has a variant `{tag}`";
                    note: "a `.Tag` pattern names a variant of some declared `@union`"
                ),
            });
        };
        self.eng.unify(expected, result, "in a variant pattern")?;
        if let Some(u) = union {
            self.variant_pattern_unions.insert(pat, u);
        }
        let label = variant_label(union, tag);
        // As for a struct pattern: fewer binders than slots is fine, more is not.
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let name = self.text(*name);
                    let want = variant_field_ty(&payload, Some(name), i);
                    let want =
                        want.ok_or_else(|| no_such_field("constructor", &label, name))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = variant_field_ty(&payload, None, i);
                    let n = payload.len();
                    let want = want.ok_or_else(|| no_field_at("constructor", &label, i, n))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = self.text(*name);
                    let want = variant_field_ty(&payload, Some(name), i);
                    let want =
                        want.ok_or_else(|| no_such_field("constructor", &label, name))?;
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    /// Check that the typed match `e` covers every value of its scrutinee (an
    /// error otherwise) and record a warning for each arm no value can reach. A
    /// handler's value arms are parsed into such a match on `%ret`; its
    /// diagnostics speak of the value arms, never of that name.
    fn check_coverage(&mut self, e: Aol<Expr>, arms: utilities::Slice<crate::parser::data::Arm>) -> Result<()> {
        let arms = self.ast.slice(arms);
        let rows: Vec<(Vec<exhaustive::DPat>, bool)> = arms
            .iter()
            .map(|arm| {
                let alts = self.ast.slice(arm.patterns).iter().map(|p| self.dpat(*p)).collect();
                (alts, arm.guard.is_some())
            })
            .collect();
        let report = exhaustive::check(&rows);
        let handler = matches!(
            self.node(e),
            Expr::Match { scrut, .. }
                if matches!(self.node(*scrut), Expr::Var { module: None, name } if self.text(*name) == "%ret")
        );
        for i in report.unreachable {
            let span = self.ast.expr_span(arms[i].body).unwrap_or(Span::at(0));
            if self.warnings.iter().any(|w| w.root().span == span) {
                continue;
            }
            self.warnings.push(diag!(
                Code::UnreachableArm, span, 0,
                "unreachable arm: the arms before it already match every value it matches"
            ));
        }
        let Some(missing) = report.missing else {
            return Ok(());
        };
        let span = self.ast.expr_span(e).unwrap_or(Span::at(0));
        let what = if handler { "the handler's value arms do" } else { "this `is` does" };
        Err(diag!(
            Code::NonExhaustiveMatch, span, 0,
            "non-exhaustive match: {what} not cover `{}`", exhaustive::show(&missing);
            note: "add an arm for it, or a catch-all `| _ => ...`"
        ))
    }

    /// Translate a typed pattern for [`exhaustive::check`].
    fn dpat(&self, pat: Aol<Pattern>) -> exhaustive::DPat {
        use exhaustive::{Ctor, DPat, ProductKind};
        match self.pnode(pat) {
            Pattern::Wild | Pattern::Var(_) => DPat::Wild,
            Pattern::Int(_) | Pattern::Real(_) | Pattern::Str(_)
                if self.literal_pattern_hooks.contains_key(&pat) =>
            {
                DPat::Opaque
            }
            Pattern::Int(n) => DPat::Ctor(Ctor::Lit(n.to_string()), Vec::new()),
            Pattern::Real(x) => DPat::Ctor(Ctor::Lit(format!("{x:?}")), Vec::new()),
            Pattern::Str(s) => {
                let text = String::from_utf8_lossy(self.ast.bytes(*s));
                DPat::Ctor(Ctor::Lit(format!("{text:?}")), Vec::new())
            }
            Pattern::Bool(b) => DPat::Ctor(Ctor::Bool(*b), Vec::new()),
            Pattern::Range { .. } | Pattern::StrPrefix { .. } => DPat::Opaque,
            Pattern::Cons { head, tail } => {
                DPat::Ctor(Ctor::SeqMore, vec![self.dpat(*head), self.dpat(*tail)])
            }
            Pattern::List { elems, rest } => {
                let end = match rest {
                    Some(r) => self.dpat(*r),
                    None => DPat::Ctor(Ctor::SeqEmpty, Vec::new()),
                };
                self.ast.slice(*elems).iter().rev().fold(end, |tail, e| {
                    DPat::Ctor(Ctor::SeqMore, vec![self.dpat(*e), tail])
                })
            }
            Pattern::Tuple(pats) => {
                let pats = self.ast.slice(*pats);
                if pats.is_empty() {
                    return DPat::Wild;
                }
                let fields = pats.iter().enumerate().map(|(i, p)| (i.to_string(), self.dpat(*p)));
                DPat::Product(ProductKind::Tuple, fields.collect())
            }
            Pattern::Struct { ty, fields } => {
                let ty = self.text(*ty);
                let Some(info) = self.structs.get(ty) else {
                    return DPat::Opaque;
                };
                let mut out = Vec::new();
                for (i, f) in self.ast.slice(*fields).iter().enumerate() {
                    match f {
                        FieldPat::Named { name, pat } => {
                            out.push((self.text(*name).to_string(), self.dpat(*pat)))
                        }
                        FieldPat::Positional(pat) => match info.fields.get(i) {
                            Some((name, _)) => out.push((name.to_string(), self.dpat(*pat))),
                            None => return DPat::Opaque,
                        },
                        FieldPat::Shorthand(_) => {}
                    }
                }
                DPat::Product(ProductKind::Struct(ty.into()), out)
            }
            Pattern::Record { fields, .. } => {
                let fields = self.ast.slice(*fields).iter().filter_map(|f| match f {
                    FieldPat::Named { name, pat } => {
                        Some((self.text(*name).to_string(), self.dpat(*pat)))
                    }
                    _ => None,
                });
                DPat::Product(ProductKind::Record, fields.collect())
            }
            Pattern::Variant { ty, tag, fields, .. } => {
                let tag = self.text(*tag);
                let union = self
                    .variant_pattern_unions
                    .get(&pat)
                    .copied()
                    .or_else(|| ty.map(|t| self.text(t)))
                    .or_else(|| self.find_union_by_tag(tag));
                let Some((union, info)) = union.and_then(|u| Some((u, self.unions.get(u)?))) else {
                    return DPat::Opaque;
                };
                let Some(index) = info.variants.iter().position(|v| v.tag == tag) else {
                    return DPat::Opaque;
                };
                let payload = &info.variants[index].payload;
                let mut args = vec![DPat::Wild; payload.len()];
                for (i, f) in self.ast.slice(*fields).iter().enumerate() {
                    let (slot, p) = match f {
                        FieldPat::Named { name, pat } => {
                            let name = self.text(*name);
                            (payload.iter().position(|(n, _)| *n == Some(name)), *pat)
                        }
                        FieldPat::Positional(pat) => (Some(i), *pat),
                        FieldPat::Shorthand(_) => continue,
                    };
                    match slot.and_then(|s| args.get_mut(s)) {
                        Some(a) => *a = self.dpat(p),
                        None => return DPat::Opaque,
                    }
                }
                let siblings: Vec<(String, usize)> = info
                    .variants
                    .iter()
                    .map(|v| (v.tag.to_string(), v.payload.len()))
                    .collect();
                DPat::Ctor(
                    Ctor::Variant { union: union.into(), index, siblings: siblings.into() },
                    args,
                )
            }
        }
    }

    /// The non-fatal diagnostics this check produced (unreachable match arms).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
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
    }

    /// The number of type parameters a user-declared type constructor takes, if
    /// `name` is a struct or union (the ones whose arity we track).
    fn type_arity(&self, name: &str) -> Option<usize> {
        self.structs
            .get(name)
            .map(|i| i.params.len())
            .or_else(|| self.unions.get(name).map(|i| i.params.len()))
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
            Ty::Nat(n) => self.eng.types.add(TypeNode::Nat(*n)),
            Ty::Var(name) => {
                let name = self.text(*name);
                let eng = &mut self.eng;
                *tvars.entry(name).or_insert_with(|| eng.fresh_nat())
            }
            Ty::SizeAdd(a, b) => {
                let (a, b) = (*a, *b);
                let (x, y) = (
                    self.size_ty_of_ast(a, tvars),
                    self.size_ty_of_ast(b, tvars),
                );
                self.eng.types.add(TypeNode::NatAdd(x, y))
            }
            Ty::SizeMul(a, b) => {
                let (a, b) = (*a, *b);
                let (x, y) = (
                    self.size_ty_of_ast(a, tvars),
                    self.size_ty_of_ast(b, tvars),
                );
                self.eng.types.add(TypeNode::NatMul(x, y))
            }
            _ => self.ty_of_ast(ty, tvars),
        }
    }

    /// The value of an `Int` literal expression, else `None` (used to read the
    /// compile-time bounds of a range that builds a sized tensor).
    fn int_literal(&self, e: Aol<Expr>) -> Option<i64> {
        match self.node(e) {
            Expr::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Peel a sized-tensor type `@tensor variance size elem` into `(size, elem)`,
    /// discarding the variance (callers here only need the length and element).
    fn tensor_parts(&self, ty: Type) -> Option<(Type, Type)> {
        if let TypeNode::App(head, elem) = self.eng.head(ty) {
            if let TypeNode::App(head2, size) = self.eng.head(head) {
                if let TypeNode::App(con, _variance) = self.eng.head(head2) {
                    if matches!(self.eng.head(con), TypeNode::Con(n) if self.eng.types.name(n) == TENSOR)
                    {
                        return Some((size, elem));
                    }
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
                self.eng.types.con(name)
            }
            Ty::Var(name) => {
                let name = self.text(*name);
                *tvars
                    .entry(name)
                    .or_insert_with(|| self.eng.fresh())
            }
            Ty::App(head, arg) => {
                self.check_type_arity(ty);
                let (head, arg) = (*head, *arg);
                let (h, a) = (self.ty_of_ast(head, tvars), self.ty_of_ast(arg, tvars));
                self.eng.types.app(h, a)
            }
            // `@e X` in type position: infer `X` (so its calls/overloads resolve
            // for lowering) and require it to build `@code`; the spliced-in type is
            // unknown until the driver's expand loop runs `X`, so it stands as a
            // fresh variable here. Only reachable in a lenient (pre-expansion)
            // round; the final strict compile sees the substituted concrete type.
            Ty::MetaE(expr, meta) => {
                let (expr, meta) = (*expr, *meta);
                // `@run` discharges `<@meta>` (its operand may perform meta ops);
                // `@e` runs a pure operand. Either way the spliced-in type is
                // unknown until the driver expands it, so it stands as a fresh var.
                let ambient = if meta {
                    {
                        let empty = self.eng.types.row_empty();
                        self.eng.types.row_extend("@meta", empty)
                    }
                } else {
                    self.eng.types.row_empty()
                };
                let saved = std::mem::replace(&mut self.ambient, ambient);
                if let Ok(t) = self.infer(expr) {
                    let _ = self.eng.unify(t, self.eng.types.con("@code"), "in a `@e` type splice");
                }
                self.ambient = saved;
                self.eng.fresh()
            }
            Ty::Nat(n) => self.eng.types.add(TypeNode::Nat(*n)),
            // A size expression written in type position (only well-formed inside a
            // `[..]`); elaborate it as a size so kind-checking flags any misuse.
            Ty::SizeAdd(..) | Ty::SizeMul(..) => self.size_ty_of_ast(ty, tvars),
            Ty::Sized {
                variance,
                size,
                elem,
            } => {
                let (variance, size, elem) = (*variance, *size, *elem);
                let size_ty = self.size_ty_of_ast(size, tvars);
                let elem_ty = self.ty_of_ast(elem, tvars);
                tensor_type(&self.eng.types, variance_con(&self.eng.types, variance), size_ty, elem_ty)
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
                                *tvars.entry(name).or_insert_with(|| self.eng.fresh())
                            }
                            None => self.eng.types.row_empty(),
                        };
                        for &label in self.ast.slice(row.names).iter() {
                            e = self.eng.types.row_extend(self.text(label), e);
                        }
                        e
                    }
                    None => self.eng.types.row_empty(),
                };
                let (f, t) = (self.ty_of_ast(from, tvars), self.ty_of_ast(to, tvars));
                self.eng.types.arrow_eff(f, t, eff)
            }
            Ty::Unit => self.eng.types.con(ty::UNIT),
            Ty::Tuple(items) => {
                let items = self.ast.slice(*items);
                let mapped: Vec<Type> =
                    items.iter().map(|t| self.ty_of_ast(*t, tvars)).collect();
                self.eng.types.tuple(mapped)
            }
            Ty::Record { fields, tail } => {
                // A record type: `{ x: A | r }` is open (row variable tail), `{ x: A,
                // y: B }` is closed (empty tail). Records are real, name-keyed types;
                // positional/scalar values promote to them at call sites.
                let rest = match tail {
                    Some(tvar) => {
                        let name = self.text(*tvar);
                        *tvars.entry(name).or_insert_with(|| self.eng.fresh())
                    }
                    None => self.eng.types.row_empty(),
                };
                let decls: Vec<_> = self.ast.slice(*fields).to_vec();
                let mut row = rest;
                for f in decls.iter().rev() {
                    let fty = self.ty_of_ast(f.ty, tvars);
                    row = self.eng.types.row_field(self.text(f.name), fty, row);
                }
                self.eng.types.record(row)
            }
        }
    }

    // -- built-ins ----------------------------------------------------------

    /// A fresh element variable and the `@vec` of it, for a vector built-in.
    fn fresh_vec(&mut self, vec_con: Type) -> (Type, Type) {
        let t = self.eng.fresh_generic();
        let vt = self.eng.types.app(vec_con, t);
        (t, vt)
    }

    fn install_builtins(&mut self) {
        let int = self.eng.types.con(ty::INT);
        let bool_ = self.eng.types.con(ty::BOOL);

        // `+ - * / % ^`, the comparisons, and the unary `neg` are all defined in
        // CORE.thx over their interfaces, not seeded here. `!` is the one operator
        // with a single type, so it stays a plain binding.
        let t = self.eng.types.arrow(bool_, bool_);
        self.bind("not", t);

        // Monomorphic arithmetic intrinsics: the primitive floor the operator
        // overloads are built on. Typed `t -> t -> t` (like the `@vec_*`
        // primitives): a single width-agnostic op the runtime implements over the
        // value rep. The `@i*`/`@u*`/`@f*`/`@f32*` split is behavioural, not typed;
        // callers pass the right numeric type. `@f*` computes at f64 (`@float64`);
        // `@f32*` rounds each operand and its result to single precision.
        for name in [
            "@iadd", "@isub", "@imul", "@idiv", "@imod", "@udiv", "@umod", "@fadd", "@fsub",
            "@fmul", "@fdiv", "@fmod", "@fpow", "@f32add", "@f32sub", "@f32mul", "@f32div",
            "@f32mod", "@f32pow",
        ] {
            let t = self.eng.fresh_generic();
            self.bind(name, self.eng.types.arrow(t, self.eng.types.arrow(t, t)));
        }

        // Comparison intrinsics, the floor under the comparison overloads. Same
        // shape as the arithmetic ones (`t -> t -> @bool`, the behavioural
        // `@i`/`@u`/`@f`/`@s` split picked by the overload that calls them). `@ieq`
        // serves signed and unsigned alike (equality reads the same bits), and the
        // float pair compares at `@float64`, which is exact for a `@float32` too.
        for name in ["@ieq", "@ilt", "@ult", "@feq", "@flt", "@seq", "@slt"] {
            let t = self.eng.fresh_generic();
            self.bind(name, self.eng.types.arrow(t, self.eng.types.arrow(t, bool_)));
        }

        // `@acat` joins two byte buffers of the same kind: the primitive CORE's
        // `ICat @str` / `ICat @array` instances are built on, now that `++` is an
        // ordinary function. Typed like the arithmetic intrinsics (`t -> t -> t`),
        // since the runtime reads the kind off the values.
        {
            let t = self.eng.fresh_generic();
            self.bind("@acat", self.eng.types.arrow(t, self.eng.types.arrow(t, t)));
        }
        // The byte-buffer primitives serve `@array` and `@str` alike (one runtime rep,
        // two types), so they are typed with a single receiver variable like the
        // `@vec_*` family: the runtime reads the kind off the value.
        for (name, mids, returns_self) in [
            ("@array_len", 0, false),
            ("@array_get", 1, false),
            ("@array_push", 1, true),
            ("@array_set", 2, true),
            ("@array_slice", 2, true),
        ] {
            let recv = self.eng.fresh_generic();
            let ret = if returns_self { recv } else { int };
            let params = std::iter::once(recv).chain((0..mids).map(|_| int));
            let t = self.eng.types.arrows(params, ret);
            self.bind(name, t);
        }

        let vec_con = self.eng.types.con(ty::VEC);
        let (_t, vt) = self.fresh_vec(vec_con);
        let t = self.eng.types.arrow(self.eng.types.con(ty::UNIT), vt);
        self.bind("@vec_new", t);
        let (t, vt) = self.fresh_vec(vec_con);
        let t = self.eng.types.arrow(int, self.eng.types.arrow(t, vt));
        self.bind("@vec_fill", t);
        let (_t, vt) = self.fresh_vec(vec_con);
        let t = self.eng.types.arrow(vt, int);
        self.bind("@vec_len", t);
        let (t, vt) = self.fresh_vec(vec_con);
        let t = self.eng.types.arrow(vt, self.eng.types.arrow(int, t));
        self.bind("@vec_get", t);
        let (t, vt) = self.fresh_vec(vec_con);
        self.bind(
            "@vec_set",
            self.eng.types.arrow(vt, self.eng.types.arrow(int, self.eng.types.arrow(t, vt))),
        );
        let (t, vt) = self.fresh_vec(vec_con);
        let t = self.eng.types.arrow(vt, self.eng.types.arrow(t, vt));
        self.bind("@vec_push", t);
        let (_t, vt) = self.fresh_vec(vec_con);
        self.bind(
            "@vec_slice",
            self.eng.types.arrow(vt, self.eng.types.arrow(int, self.eng.types.arrow(int, vt))),
        );

        // Metaprogramming primitives (compile-time; usable inside `$ @run`). `@lex`
        // tokenizes a string into an opaque `@token` vector; a lex error traps
        // (fails the build). Tokens are inspected via the `@token_*` accessors, not
        // pattern-matched. `@token` is an opaque builtin type (see `is_base_type`).
        let token = self.eng.types.con("@token");
        let str_ty = self.eng.types.con(ty::STR);
        self.bind(
            "@lex",
            self.eng.types.arrow(str_ty, self.eng.types.app(self.eng.types.con(ty::VEC), token)),
        );
        let t = self.eng.types.arrow(token, str_ty);
        self.bind("@token_kind", t);
        let t = self.eng.types.arrow(token, str_ty);
        self.bind("@token_text", t);
        // `@parse_str` parses a string as an expression fragment into an opaque
        // `@code` value; a syntax error traps (fails the build). `@parse_items`
        // validates the string as top-level item(s) instead, for a `$ @e` that
        // injects definitions.
        let t = self.eng.types.arrow(str_ty, self.eng.types.con("@code"));
        self.bind("@parse_str", t);
        let t = self.eng.types.arrow(str_ty, self.eng.types.con("@code"));
        self.bind("@parse_items", t);
        // `@parse` consumes a token vector (from `@lex`) into the same opaque
        // `@code` as `@parse_str`, closing the `@str -> @token -> @code` pipeline.
        self.bind(
            "@parse",
            self.eng.types.arrow(self.eng.types.app(self.eng.types.con(ty::VEC), token), self.eng.types.con("@code")),
        );
        // The metaprogramming ops carry the `<@meta>` effect, so they are usable
        // only where a `<@meta>` handler is installed: inside `@e` (see the
        // `@e`-position ambients in `check_program` / `infer_app` / `ty_of_ast`).
        // A use in ordinary code fails to unify the `<@meta>` latent row into the
        // pure ambient, giving a clean "effect `@meta` is performed but not
        // handled" error instead of a runtime no-op/fault. Lexing/parsing
        // (`@lex`/`@parse`/`@parse_str`/...) stay PURE: they need no compiler state.
        let meta_row = {
            let empty = self.eng.types.row_empty();
            self.eng.types.row_extend("@meta", empty)
        };
        // `@eval` compiles and runs an `@code` fragment at build time and returns
        // its value. Its result type is fully polymorphic (`a`): the produced
        // value is embedded as-is, so a mismatch with the use site is a runtime
        // (compile-time) fault, not a static error.
        let eval_res = self.eng.fresh_generic();
        let t = self.eng.types.arrow_eff(self.eng.types.con("@code"), eval_res, meta_row);
        self.bind("@eval", t);
        // Compile-time diagnostics. `@abort` fails the build with its message (a
        // clean user-land `assert` is `if ok => {} else @abort "..."`); its result
        // is polymorphic since it never returns. `@emit` prints a message and
        // continues.
        let abort_res = self.eng.fresh_generic();
        let t = self.eng.types.arrow_eff(str_ty, abort_res, meta_row);
        self.bind("@abort", t);
        let t = self.eng.types.arrow_eff(str_ty, self.eng.types.con(ty::UNIT), meta_row);
        self.bind("@emit", t);
        // `@e X` runs X at compile time and embeds its value at the use site, so
        // type-wise it is the identity on X's type (the value case). The fold
        // happens in lowering + the driver; `@e` must be applied directly.
        let e_ty = self.eng.fresh_generic();
        let t = self.eng.types.arrow(e_ty, e_ty);
        self.bind("@e", t);
        // `@run` is the `<@meta>` eliminator: like `@e` (compile-time run + embed)
        // but its operand may perform `<@meta>` (it is discharged here). Special-
        // cased in `infer_app`/`ty_of_ast`; this binding is the first-class fallback.
        let run_ty = self.eng.fresh_generic();
        let t = self.eng.types.arrow(run_ty, run_ty);
        self.bind("@run", t);
        // `@fresh prefix` mints a unique identifier string (`prefix` + a counter),
        // for generating hygienic, non-colliding binders in compile-time codegen.
        let t = self.eng.types.arrow_eff(str_ty, str_ty, meta_row);
        self.bind("@fresh", t);
        // `@link name` / `@link_path p`: steer the build (add a library / search
        // path to the link line), used at compile time via `$ @e (@link "curl")`.
        // The effect is the directive; the call returns unit.
        let t = self.eng.types.arrow_eff(str_ty, self.eng.types.con(ty::UNIT), meta_row);
        self.bind("@link", t);
        let t = self.eng.types.arrow_eff(str_ty, self.eng.types.con(ty::UNIT), meta_row);
        self.bind("@link_path", t);
        // Compile-time reflection over a declared type (resolved by the driver's
        // type host inside `$ @e`). `@type_kind` is `"struct"`/`"union"`;
        // `@type_fields` the struct's field names; `@type_variants` the union's
        // `(tag, arity)` pairs. A derive-style macro reads these and generates code.
        let t = self.eng.types.arrow(str_ty, str_ty);
        self.bind("@type_kind", t);
        self.bind(
            "@type_params",
            self.eng.types.arrow(str_ty, self.eng.types.app(self.eng.types.con(ty::VEC), str_ty)),
        );
        self.bind(
            "@type_fields",
            self.eng.types.arrow(str_ty, self.eng.types.app(self.eng.types.con(ty::VEC), str_ty)),
        );
        self.bind(
            "@type_variants",
            self.eng.types.arrow(
                str_ty,
                {
                    let i = self.eng.types.con(ty::INT);
                    let pair = self.eng.types.tuple(vec![str_ty, i]);
                    let v = self.eng.types.con(ty::VEC);
                    self.eng.types.app(v, pair)
                },
            ),
        );

        // The sized-tensor PRIMITIVES. `@`-sigil marks them as compiler intrinsics
        // (like `@int64`), the minimal set the runtime provides; every nice name
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
        // OVERLOADABLE `@compiler_interface_indexing` hook (its tensor candidate in
        // `library/LA` calls `@tensor_index`); a custom container adds its own hook
        // overload, the import merge coexists.
        {
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let v = self.eng.fresh_generic();
            let vn = tensor_type(&self.eng.types, v, n, a);
            let t = self.eng.types.arrow(vn, self.eng.types.arrow(int, a));
        self.bind("@tensor_index", t);
        }
        // `@tensor_length : [n]a -> Int` (runtime size, untied to `n`: no dependent values).
        {
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let v = self.eng.fresh_generic();
            let vn = tensor_type(&self.eng.types, v, n, a);
            let t = self.eng.types.arrow(vn, int);
        self.bind("@tensor_length", t);
        }
        // `@tensor_create : [n]x -> (Int -> a) -> [n]a`: build a tensor the SAME SIZE as a
        // template from an index function (sound: result size = template size). The
        // constructing primitive that lets `transpose`/`matmul` be library code; it
        // preserves the template's variance `v` onto the result.
        {
            let x = self.eng.fresh_generic();
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let v = self.eng.fresh_generic();
            let template = tensor_type(&self.eng.types, v, n, x);
            let idx_fn = self.eng.types.arrow(int, a);
            let result = tensor_type(&self.eng.types, v, n, a);
            self.bind(
                "@tensor_create",
                self.eng.types.arrow(template, self.eng.types.arrow(idx_fn, result)),
            );
        }
        // `@tensor_concat : [n]a -> [m]a -> [n+m]a`: the size-CHANGING join (which `@tensor_create`,
        // being size-preserving, cannot express), so it is a primitive. Both operands
        // and the result share the concatenated axis's variance `v`.
        {
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let m = self.eng.fresh_generic_nat();
            let v = self.eng.fresh_generic();
            let tn = tensor_type(&self.eng.types, v, n, a);
            let tm = tensor_type(&self.eng.types, v, m, a);
            let tnm = tensor_type(&self.eng.types, v, self.eng.types.add(TypeNode::NatAdd(n, m)), a);
            let t = self.eng.types.arrow(tn, self.eng.types.arrow(tm, tnm));
        self.bind("@tensor_concat", t);
        }
        // `@tensor_transpose : [m][n]a -> [n][m]a`: an O(1) VIEW (swap axes/strides),
        // so `LA.transpose` copies nothing. Variance stays per POSITION (outer `vm`,
        // inner `vn`); only the sizes swap, so a `[@Contra m, @Co n]` matrix
        // transposes to `[@Contra n, @Co m]`.
        {
            let a = self.eng.fresh_generic();
            let m = self.eng.fresh_generic_nat();
            let n = self.eng.fresh_generic_nat();
            let vm = self.eng.fresh_generic();
            let vn = self.eng.fresh_generic();
            let mn = tensor_type(&self.eng.types, vm, m, tensor_type(&self.eng.types, vn, n, a));
            let nm = tensor_type(&self.eng.types, vm, n, tensor_type(&self.eng.types, vn, m, a));
            let t = self.eng.types.arrow(mn, nm);
        self.bind("@tensor_transpose", t);
        }
        // `@tensor_slice : [n]a -> Int -> Int -> [k]a`: an O(1) VIEW over `[lo, hi)` of
        // the leading axis. The result size `k` is a runtime value, so it is a fresh
        // nat (not `hi - lo`, which are runtime Ints); indexing it is modular/total, so
        // an unknown static size is consistent with the rest of the tensor design. The
        // sliced axis keeps its variance `v`.
        {
            let a = self.eng.fresh_generic();
            let n = self.eng.fresh_generic_nat();
            let k = self.eng.fresh_generic_nat();
            let v = self.eng.fresh_generic();
            let src = tensor_type(&self.eng.types, v, n, a);
            let out = tensor_type(&self.eng.types, v, k, a);
            self.bind(
                "@tensor_slice",
                self.eng.types.arrow(src, self.eng.types.arrow(int, self.eng.types.arrow(int, out))),
            );
        }

        // Comparison has no built-in tier: `==` and `<` are CORE functions over the
        // `IEq` / `IOrd` interfaces, so a type is comparable exactly when it has an
        // instance. The structural built-in that used to catch everything else is
        // gone, because it made `==` type-check on values it could not compare.
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            let t = self.eng.types.arrow(a, self.eng.types.arrow(b, b));
        self.bind(";", t);
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            let e = self.eng.fresh_generic();
            let f = self.eng.types.arrow_eff(a, b, e);
            let t = self.eng.types.arrow(a, self.eng.types.arrow_eff(f, b, e));
        self.bind("|>", t);
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            let e = self.eng.fresh_generic();
            let f = self.eng.types.arrow_eff(a, b, e);
            let t = self.eng.types.arrow(f, f);
        self.bind("<|", t);
        }
    }
}

/// A global `$` definition, extracted from the program for dependency analysis.
#[derive(Clone)]
struct Def<'a> {
    name: &'a str,
    sig: Option<Aol<Ty>>,
    /// The context parameter's type when the signature declares one. It is also
    /// `sig`'s first `from`, so the signature stays what the user wrote.
    ctx: Option<Aol<Ty>>,
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
                    bound.push(ast.text(clause.arg));
                    bound.push(ast.text(h.continuation));
                    free_globals(ast, clause.body, globals, bound, out);
                    bound.truncate(mark);
                }
                if let Some((name, value_body)) = &h.value {
                    let mark = bound.len();
                    bound.push(ast.text(*name));
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
            ast.slice(*elems)
                .iter()
                .for_each(|p| collect_pattern_binders(ast, *p, bound));
            if let Some(r) = rest {
                collect_pattern_binders(ast, *r, bound);
            }
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
                    FieldPat::Shorthand(name) => bound.push(ast.text(*name)),
                }
            }
        }
        Pattern::Record { fields, rest } => {
            for f in ast.slice(*fields).iter() {
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

fn applied(types: &Types, name: &str, args: &[Type]) -> Type {
    let mut acc = types.con(name);
    for a in args {
        acc = types.app(acc, *a);
    }
    acc
}

/// The mangled global name for one overload: `name#<type-key>`. Two overloads of
/// one name in one module get distinct keys, so their globals no longer collide
//// Whether a type is unit `{}` (a nullary C function's zero-argument parameter),
/// as either the `{}` constructor or the empty tuple.
fn is_unit_ty(types: &Types, ty: Type) -> bool {
    match types.node(ty) {
        TypeNode::Con(n) => types.name(n) == ty::UNIT,
        TypeNode::Tuple(v) => v.is_empty(),
        _ => false,
    }
}

/// A type's marshalling name for the FFI seam. A type variable or any composite
/// (the checker's fallback, matching the C++ `desc_of`) marshals word-sized, so
/// the backends read it as `Int`.
fn marshal_name(types: &Types, ty: Type) -> String {
    match types.node(ty) {
        TypeNode::Con(name) => types.name(name).to_string(),
        TypeNode::Tuple(items) if items.is_empty() => ty::UNIT.to_string(),
        // A function-typed `@extern` parameter is a C function pointer (callback):
        // encode its scalar signature as `@fn(a,b,...)->r` so the seam can wrap a
        // Thrax closure into a C-callable pointer.
        TypeNode::Arrow(..) => {
            let mut args = Vec::new();
            let mut cur = ty;
            while let TypeNode::Arrow(from, to, _) = types.node(cur) {
                args.push(marshal_name(types, from));
                cur = to;
            }
            format!("@fn({})->{}", args.join(","), marshal_name(types, cur))
        }
        // A `@vec T` parameter is a C array of `T`: passed as a `T*` pointing at a
        // contiguous packed buffer. Encoded so the seam can find `T`'s layout.
        TypeNode::App(head, arg) => match (types.node(head), types.node(arg)) {
            (TypeNode::Con(vec), TypeNode::Con(elem)) if types.name(vec) == ty::VEC => {
                format!("@structs({})", types.name(elem))
            }
            _ => ty::INT.to_string(),
        },
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
        Ty::Tuple(items) => ast.slice(*items).iter().for_each(|t| collect_tyvars(ast, *t, out)),
        Ty::Record { fields, tail } => {
            ast.slice(*fields).iter().for_each(|f| collect_tyvars(ast, f.ty, out));
            if let Some(t) = tail {
                let name = ast.text(*t);
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        Ty::Sized { size, elem, .. } => {
            collect_tyvars(ast, *size, out);
            collect_tyvars(ast, *elem, out);
        }
        Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_tyvars(ast, *a, out);
            collect_tyvars(ast, *b, out);
        }
        // A `@e X` type splice contributes no type variables: its type is unknown
        // until the driver expands it, after which this node no longer exists.
        Ty::Con { .. } | Ty::Nat(_) | Ty::Unit | Ty::MetaE(..) => {}
    }
}

/// Collect the type-constructor names `ty` mentions in a STRICT position: one
/// that must be built to build a value of `ty`. Used for the type dependency
/// graph behind the lazy-slot decision.
///
/// An arrow is not descended into. A function is already a suspension, so a
/// recursive occurrence behind one (`Susp: { @int, {} -> Task }`) costs nothing
/// at construction and must stay a plain function: thunking it again would leave
/// callers applying a thunk. A type variable contributes nothing either, since a
/// parameter cannot make its owner recursive on its own.
fn collect_tycons<'a>(ast: &'a Ast, ty: Aol<Ty>, out: &mut Vec<&'a str>) {
    match ast.ty(ty) {
        Ty::Con { name, .. } => {
            let name = ast.text(*name);
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ty::App(a, b) | Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_tycons(ast, *a, out);
            collect_tycons(ast, *b, out);
        }
        Ty::Sized { size, elem, .. } => {
            collect_tycons(ast, *size, out);
            collect_tycons(ast, *elem, out);
        }
        Ty::Tuple(items) => ast.slice(*items).iter().for_each(|t| collect_tycons(ast, *t, out)),
        Ty::Record { fields, .. } => {
            ast.slice(*fields).iter().for_each(|f| collect_tycons(ast, f.ty, out))
        }
        Ty::Arrow { .. } | Ty::Var(_) | Ty::Nat(_) | Ty::Unit | Ty::MetaE(..) => {}
    }
}

/// Normalize a variant payload into `(optional-name, type-handle)` pairs.
fn payload_fields<'a>(ast: &'a Ast, p: &Payload) -> Vec<(Option<&'a str>, Aol<Ty>)> {
    match p {
        Payload::None => vec![],
        Payload::Bare(ty) => vec![(None, *ty)],
        Payload::Fields(fs) => ast
            .slice(*fs)
            .iter()
            .map(|f| (f.name.map(|n| ast.text(n)), f.ty))
            .collect(),
    }
}

/// Select a payload field's type by name (if named) or by position.
/// A pattern names a field the type does not declare. `what` is "struct" or
/// "constructor"; `owner` is its name (`Union.Tag` for a constructor).
fn no_such_field(what: &str, owner: &str, field: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "{what} `{owner}` has no field `{field}`"
    )
}

/// A pattern binds more positional fields than the type has.
fn no_field_at(what: &str, owner: &str, index: usize, n: usize) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "{what} `{owner}` has {n} field(s), so there is no field {index}"
    )
}

/// How a constructor is named in a diagnostic: `Union.Tag` when the union is
/// known, otherwise the bare tag (a `.Tag` whose union never resolved).
fn variant_label(union: Option<&str>, tag: &str) -> String {
    match union {
        Some(u) => format!("{u}.{tag}"),
        None => tag.to_string(),
    }
}

fn variant_field_ty(payload: &VariantPayload, name: Option<&str>, index: usize) -> Option<Type> {
    match name {
        Some(name) => payload
            .iter()
            .find(|(n, _)| *n == Some(name))
            .map(|(_, t)| *t),
        None => payload.get(index).map(|(_, t)| *t),
    }
}

/// Map the sigil/alias type constructors to their canonical built-in name. Every
/// sized integer width (signed and unsigned, `@`-sigil and friendly alias) is

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

fn crepr_field_error(ty: &str, field: &str, field_ty: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "field `{field}` of C-repr struct `{ty}` has type `{field_ty}`, which is not \
         C-representable; a `@struct @extern \"C\"` field must be a sized number \
         (`@int8..64`/`@nat8..64`/`@float32`/`@float64`), `@int`/`@nat`/`Real`, `@ptr`, \
         `@bool`, or another C-repr struct"
    )
}

/// Map a scalar type name (friendly or `@`-sigil) to its fixed-width C kind.
/// `Int`/`Nat`/`Ptr` resolve to the target's word width. Returns `None` for a
/// non-scalar (a nested struct, a variable, or a non-C type).
fn scalar_ckind(name: &str, ptr_bits: u32) -> Option<utilities::CKind> {
    use utilities::CKind::*;
    let word = if ptr_bits == 32 { S32 } else { S64 };
    let uword = if ptr_bits == 32 { U32 } else { U64 };
    Some(match name {
        "@int8" => S8,
        "@int16" => S16,
        "@int32" => S32,
        "@int64" => S64,
        "@nat8" => U8,
        "@nat16" => U16,
        "@nat32" => U32,
        "@nat64" => U64,
        "@float32" => F32,
        "@float64" => F64,
        "@int" => word,
        "@nat" => uword,
        "@ptr" => uword,
        "@bool" => U8,
        _ => return None,
    })
}

/// The numeric types a bare integer/real literal may take: the friendly word-size
/// `Int`/`Nat`/`Real` and every sized `@`-form. A literal used anywhere else (a
/// `@bool` condition, a `@ptr`, a `Str`) is a type error.
fn is_numeric_type(name: &str) -> bool {
    matches!(
        name,
        "@int" | "@nat" | "@float64"
            | "@int8" | "@int16" | "@int32" | "@int64"
            | "@nat8" | "@nat16" | "@nat32" | "@nat64"
            | "@float32"
    )
}

/// How deep context resolution may chain instances (an instance whose own context
/// is satisfied by another instance) before it is reported as non-terminating.
const CTX_DEPTH: usize = 32;

/// The blessed interface each desugar site resolves. CORE declares them as
/// one-field `@struct`s (see `parser::table::BLESSED_INTERFACES`); the compiler
/// resolves a value of the applied type and projects that field.
const IF_STR_LIT: &str = "@IStrLit";
const IF_INT_LIT: &str = "@IIntLit";
const IF_REAL_LIT: &str = "@IRealLit";
const IF_SEQ_LIT: &str = "@ISeqLit";
const IF_RANGE: &str = "@IRange";
const IF_RANGE_FROM: &str = "@IRangeFrom";
const IF_SLICE: &str = "@ISlice";
const IF_SEQ_VIEW: &str = "@ISeqView";
/// The equality a literal PATTERN on a user type compares with: CORE's ordinary
/// `IEq`, the same interface `==` wraps, so a type needs no second instance.
const IF_EQ: &str = "IEq";

fn is_base_type(name: &str) -> bool {
    matches!(
        name,
        "@int" | "@nat"
            | "@int8" | "@int16" | "@int32" | "@int64"
            | "@nat8" | "@nat16" | "@nat32" | "@nat64"
            | "@float32" | "@float64"
            | "@str" | "@ptr" | "@bool" | "@array" | "@vec"
            | "@token" | "@code"
    )
}

/// The internal constructor name of a sized tensor. Its type is the application
/// spine `@tensor variance size elem`, one layer per axis. `@`-prefixed so it
/// cannot collide with a user type; the whole thing is erased before runtime.
const TENSOR: &str = "@tensor";
/// The three axis-variance markers, carried as nullary constructors in the
/// `variance` position of the `@tensor` spine (see [`Variance`]).
const VAR_NEUTRAL: &str = "@neutral";
const VAR_CO: &str = "@co";
const VAR_CONTRA: &str = "@contra";

/// The `Type` constructor for a source-level axis variance.
fn variance_con(types: &Types, v: Variance) -> Type {
    types.con(match v {
        Variance::Neutral => VAR_NEUTRAL,
        Variance::Co => VAR_CO,
        Variance::Contra => VAR_CONTRA,
    })
}

/// Build a sized-tensor type from its axis variance, size, and element type.
fn tensor_type(types: &Types, variance: Type, size: Type, elem: Type) -> Type {
    let con = types.con(TENSOR);
    let a = types.app(con, variance);
    let b = types.app(a, size);
    types.app(b, elem)
}

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
