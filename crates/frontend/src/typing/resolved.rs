//! What the checker hands to desugaring.
//!
//! Desugaring cannot re-derive what inference decided: which module a bare call
//! resolved to, which instance satisfies a context, which hook builds a literal,
//! which C layout a struct has. The checker records each decision in a side
//! table keyed by AST node, and [`collect_resolved`] gathers every module's
//! tables into one [`Resolved`].
//!
//! Entry point: [`collect_resolved`].

use super::*;
/// Which slots of a declared type are lazy: keyed by `(type name, variant tag)`
/// with `None` as the tag for a struct, holding one flag per slot in declaration
/// order. Only types with at least one lazy slot appear.
pub type LazySlots = HashMap<(StrId, Option<StrId>), Vec<bool>>;

/// An `@extern`'s marshalling spec: how the single applied Thrax value maps to
/// positional C arguments (`params`), the flattened C argument type names
/// (`arg_types`, one per positional C argument), and the result type name. The
/// three are consumed by lowering to build the extern value both engines call.
pub type ExternSpec = (Vec<ExternArg>, Vec<String>, String);

/// One resolved context (`@ctx`) value at a use site, ready for lowering to inject
/// as a leading argument of the referenced function.
#[derive(Clone, Debug)]
pub enum CtxVal {
    /// A bare name: a local binder (the caller's own context parameter) or a
    /// builtin; De-Bruijn / the runtime resolves it.
    Bare(StrId),
    /// A top-level value `module.name`.
    Qualified { module: StrId, name: StrId },
    /// An instance that itself takes a context, applied to the context resolved
    /// for it (`ts_list ts_int`).
    App(Box<CtxVal>, Box<CtxVal>),
    /// A bundle for a tuple requirement, built at the use site so no global of the
    /// bundled type has to exist.
    Tuple(Vec<CtxVal>),
    /// One component of a bundle already in scope, as `base.index`: a function that
    /// received several contexts as one tuple passes a single one onward.
    Proj(Box<CtxVal>, usize),
    /// An explicit `(@ctx e)` argument; lowering lowers this AST node in place.
    Expr(Aol<Expr>),
}

/// A desugar site's resolved blessed interface: the instance found by type and the
/// single field holding its implementation. Lowering projects the field and applies
/// it to the site's operands.
#[derive(Clone, Debug)]
pub struct HookImpl {
    pub ctx: CtxVal,
    pub field: StrId,
}

/// The type checker's resolutions that lowering cannot re-derive without types.
/// `call_modules`, for instance, maps a bare-call `Expr::Var` to the module its
/// checker resolved to, so lowering can emit a qualified `MOD.name`. Empty (the
/// default) means "no resolutions", correct for callers without a checker.
#[derive(Default)]
pub struct Resolved {
    /// `[..]` literal sites resolved to a sized tensor (a vector value), from
    /// [`Checker::tensor_nodes`]. Lowering builds a vector.
    pub tensor_exprs: HashSet<Aol<Expr>>,
    /// Argument sites promoted to a record, mapped to the target field names (from
    /// [`Checker::promotions`]); lowering wraps the value.
    pub promotions: HashMap<Aol<Expr>, Vec<StrId>>,
    /// `.{ .. }` struct-literal sites mapped to their resolved struct name (from
    /// [`Checker::struct_lit_names`]); lowering uses it for the
    /// field-name layout of a positional literal.
    pub struct_lit_names: HashMap<Aol<Expr>, StrId>,
    /// Literal sites (`"..."`, `[..]`, an int, a real) a blessed `@I*`
    /// construction interface builds into a user type, mapped to the hook's
    /// `(owning module, emitted name)` (from
    /// [`Checker::literal_hooks`]). Lowering wraps the raw payload term
    /// in a call to this global; an unrecorded literal folds to the plain constant.
    pub literal_hooks: HashMap<Aol<Expr>, HookImpl>,
    /// Blessed-interface value sites (`@IIndex`, `@IImagLit`) mapped to the field
    /// lowering projects off the instance `ctx_args` records for the same site (from
    /// [`Checker::blessed_sites`]).
    pub blessed_sites: HashMap<Aol<Expr>, StrId>,
    /// Literal PATTERN sites matched through a user type's construction + equality
    /// hooks, mapped to `(build hook, equality hook)` as `(module, emitted name)`
    /// (from [`Checker::literal_pattern_hooks`]). Lowering emits a
    /// `Pat::HookEq` that builds the literal and compares it with the equality hook.
    pub literal_pattern_hooks: HashMap<Aol<Pattern>, (HookImpl, HookImpl)>,
    /// Sequence pattern sites matched through a user type's `sequence_view` hook,
    /// mapped to the hook's `(module, emitted name)` (from
    /// [`Checker::sequence_pattern_hooks`]). Lowering emits a
    /// `Pat::SeqView` that unfolds the view.
    pub sequence_pattern_hooks: HashMap<Aol<Pattern>, HookImpl>,
    /// Sequence patterns on a sized tensor, with its static size (from
    /// [`Checker::tensor_patterns`]).
    pub tensor_patterns: HashMap<Aol<Pattern>, u64>,
    pub call_modules: HashMap<Aol<Expr>, StrId>,
    /// Each use site of a `@ctx`-bearing function, mapped to the context value
    /// lowering injects ahead of the explicit arguments (from
    /// [`Checker::ctx_calls`]).
    pub ctx_args: HashMap<Aol<Expr>, CtxVal>,
    /// The ordered field names each `with` expression binds, keyed by the `With`
    /// node (from [`Checker::with_fields`]). Lowering desugars
    /// `with` into a `let` per field so the Core carries no `with` node.
    pub with_fields: HashMap<Aol<Expr>, Vec<StrId>>,
    /// Each `@cast` to a type under 64 bits, mapped to the target's signedness
    /// and width (from [`Checker::cast_widths`]).
    pub cast_widths: HashMap<Aol<Expr>, (bool, u32)>,
    /// Each `@extern` node's marshalling spec (how the single applied value maps
    /// to positional C args, the flattened C argument type names, and the result
    /// name) from [`Checker::extern_sigs`]. Lowering needs these to
    /// build the `Term::Extern` the checker erased into a var.
    pub extern_sigs: HashMap<Aol<Expr>, ExternSpec>,
    /// Every foreign function's marshalling spec keyed by its resolved owner
    /// `(module, name)`, aggregated across all modules' checkers. A call site
    /// looks its head up here to flatten the record of C parameters into
    /// positional arguments (so the record is never built), and a first-class
    /// reference to a multi-argument extern eta-expands against it.
    pub externs: HashMap<(StrId, StrId), ExternSpec>,
    /// Every `@extern "thrax"` declaration as its `(module, name)`, so a reference
    /// lowers to the bare builtin instead of a global.
    pub builtins: HashSet<(StrId, StrId)>,
    /// The C memory layout of each C-repr struct (`@struct @extern "abi"`), keyed
    /// by type name (from [`Checker::crepr_layouts`]). Carried to
    /// the runtime so a struct value can be marshalled across the `@extern`
    /// boundary by value.
    pub crepr_layouts: HashMap<StrId, utilities::CLayout>,
    /// Which slots of a declared type are lazy (from
    /// [`Checker::lazy_slots`]). Construction wraps such a slot in
    /// a thunk and the machine forces it when the value is scrutinised, so a
    /// recursive type can be built and consumed a cell at a time.
    pub lazy_slots: LazySlots,
    /// Every module's declared types, as the checker resolved them.
    pub declarations: Declarations,
}

/// Gather every checker resolution lowering needs into one [`Resolved`]: `[..]`
/// array/tensor nodes, resolved bare calls, literal/pattern
/// hooks, context (`@ctx`) values, extern specs, C-repr layouts, ...
///
/// Lowering `expect`s several of these to be present (a `::` pattern without its
/// sequence-view hook panics), so every caller that checks a program and then
/// lowers it must fill `Resolved` the same way. This is that one way: a second
/// hand-written copy silently loses whichever map it was written before.
pub fn collect_resolved(checkers: &[Checker]) -> Resolved {
    let mut resolved = Resolved::default();
    for checker in checkers {
        resolved.declarations.add_module(checker);
        resolved.tensor_exprs.extend(checker.tensor_nodes().iter().copied());
        for (&site, names) in checker.promotions() {
            resolved.promotions.insert(site, names.clone());
        }
        for (&site, n) in checker.struct_lit_names() {
            resolved.struct_lit_names.insert(site, *n);
        }
        for (&site, hook) in checker.literal_hooks() {
            resolved.literal_hooks.insert(site, hook.clone());
        }
        for (&site, field) in checker.blessed_sites() {
            resolved.blessed_sites.insert(site, *field);
        }
        for (&site, pair) in checker.literal_pattern_hooks() {
            resolved.literal_pattern_hooks.insert(site, pair.clone());
        }
        for (&site, hook) in checker.sequence_pattern_hooks() {
            resolved.sequence_pattern_hooks.insert(site, hook.clone());
        }
        for (&site, &len) in checker.tensor_patterns() {
            resolved.tensor_patterns.insert(site, len);
        }
        for (key, flags) in checker.lazy_slots() {
            resolved.lazy_slots.insert(*key, flags.clone());
        }
        if std::env::var_os("THX_LAZY_DEBUG").is_some() {
            for (k, v) in checker.lazy_slots() { eprintln!("LAZY {k:?} -> {v:?}"); }
        }
        resolved.call_modules.extend(checker.call_modules());
        for (&site, val) in checker.ctx_calls() {
            resolved.ctx_args.insert(site, val.clone());
        }
        for (&site, fields) in checker.with_fields() {
            resolved.with_fields.insert(site, fields.clone());
        }
        resolved.cast_widths.extend(checker.cast_widths());
        resolved.extern_sigs.extend(checker.extern_sigs());
        let module = checker.module_name();
        for (&name, spec) in checker.own_externs() {
            resolved.externs.insert((module, name), spec.clone());
        }
        for &name in checker.own_builtins() {
            resolved.builtins.insert((module, name));
        }
        for (&name, layout) in checker.crepr_layouts() {
            resolved.crepr_layouts.insert(name, layout.clone());
        }
    }
    resolved
}

// ## What lowering reads

impl<'a> Checker<'a> {
    /// The `[..]` literal sites resolved to a sized tensor. Lowering builds a vector.
    pub fn tensor_nodes(&self) -> &HashSet<Aol<Expr>> {
        &self.tensor_exprs
    }

    /// Argument sites promoted to a record, mapped to the target field names;
    /// lowering wraps the value into a name-keyed record.
    pub fn promotions(&self) -> &HashMap<Aol<Expr>, Vec<StrId>> {
        &self.promotions
    }

    /// `.{ ... }` struct-literal sites mapped to the resolved struct name, so
    /// lowering emits the correct field names (esp. for positional literals).
    pub fn struct_lit_names(&self) -> &HashMap<Aol<Expr>, StrId> {
        &self.struct_lit_names
    }

    /// Literal sites a blessed `@I*` construction interface builds into a user
    /// type, mapped to the hook's `(module, emitted name)`. Lowering wraps the raw
    /// payload term in a call to this hook.
    pub fn literal_hooks(&self) -> &HashMap<Aol<Expr>, HookImpl> {
        &self.literal_hooks
    }

    /// Blessed-interface value sites, mapped to the field lowering projects.
    pub fn blessed_sites(&self) -> &HashMap<Aol<Expr>, StrId> {
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

    /// Sequence patterns on a sized tensor, mapped to the tensor's size.
    pub fn tensor_patterns(&self) -> &HashMap<Aol<Pattern>, u64> {
        &self.tensor_patterns
    }

    /// The lazy-slot table, for lowering to consult when it builds and consumes
    /// values of a declared type.
    pub fn lazy_slots(&self) -> &LazySlots {
        &self.lazy_slots
    }

    /// Bare-call `Expr::Var` sites this checker resolved to a specific module.
    /// Lowering rewrites each to a qualified `MOD.name` so the interpreter reaches
    /// the intended function rather than a same-named one from another module.
    pub fn call_modules(&self) -> &HashMap<Aol<Expr>, StrId> {
        &self.resolved_calls
    }

    /// The module a qualifier written in this module names: an import alias's
    /// module (`$ with V = VEC` makes `V` name `VEC`), otherwise the qualifier.
    pub fn qualifier_module(&self, qualifier: StrId) -> StrId {
        self.alias_modules.get(&qualifier).copied().unwrap_or(qualifier)
    }

    /// Each use site of a context-bearing function, mapped to the resolved context
    /// value lowering injects ahead of the explicit arguments.
    pub fn ctx_calls(&self) -> &HashMap<Aol<Expr>, CtxVal> {
        &self.ctx_args
    }

    /// The ordered field names each `with` expression binds, keyed by the `With`
    /// node. Lowering desugars `with` into a `let` per field using these.
    pub fn with_fields(&self) -> &HashMap<Aol<Expr>, Vec<StrId>> {
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
    pub fn own_externs(&self) -> &HashMap<StrId, ExternSpec> {
        &self.own_externs
    }

    /// This module's `@extern "thrax"` builtin declarations.
    pub fn own_builtins(&self) -> &HashSet<StrId> {
        &self.own_builtins
    }

    /// The C layouts of this module's C-repr structs, keyed by type name.
    pub fn crepr_layouts(&self) -> &HashMap<StrId, utilities::CLayout> {
        &self.crepr_layouts
    }
}

// ## Declarations

/// Every module's declared struct and union types, as the checker resolved
/// them (`with` splices applied), plus the interface wrappers. Desugaring reads
/// it to label positional fields and payloads. Two modules may declare the
/// same-named type (`Pair`, `Maybe`, ...), so everything is keyed by module, and
/// a lookup resolves against the desugaring module's own types first, then the
/// modules it imports, then any module.
#[derive(Default)]
pub struct Declarations {
    /// module name -> (struct name -> field names, in declaration order).
    structs: HashMap<StrId, HashMap<StrId, Vec<StrId>>>,
    /// module name -> (variant tag -> its union name and payload field names).
    unions: HashMap<StrId, HashMap<StrId, VariantLayout>>,
    /// module name -> (type name -> declared type parameters, in order), for every
    /// struct and union. Empty for a monomorphic type. Read by reflection.
    type_params: HashMap<StrId, HashMap<StrId, Vec<StrId>>>,
    /// Interface WRAPPERS: `(module, name) -> field`, for a context-bearing global
    /// whose body is exactly `\d p1 .. pn = d.field p1 .. pn` (every CORE operator).
    /// A call site then projects the resolved instance's field directly instead of
    /// calling through the wrapper, which is three closure applications per use.
    wrappers: HashMap<(StrId, StrId), StrId>,
}

/// One variant's layout: its union, its payload field names, and its position
/// among the union's variants after splicing, which reflection reports them in
/// (a derived comparison orders variants by it).
struct VariantLayout {
    union: StrId,
    fields: Vec<Option<StrId>>,
    order: usize,
}

/// The declared shape of every user type, for compile-time reflection: each
/// struct with its `(module, name, field names)`, each union with its
/// `(module, name, variants)` where a variant is `(tag, arity)`. The driver keys
/// its reflection host by both the bare `name` and the qualified `module.name`,
/// so a name shared across modules can be disambiguated. Built by
/// [`Declarations::reflect`], consumed by the driver's `$ @e` type-reflection host.
pub struct ReflectInfo {
    /// `(module, name, type params, field names)` per struct.
    pub structs: Vec<(String, String, Vec<String>, Vec<String>)>,
    /// `(module, name, type params, variants)` per union, a variant `(tag, arity)`.
    pub unions: Vec<(String, String, Vec<String>, Vec<(String, usize)>)>,
}

impl Declarations {
    /// Record one checked module's own types and wrappers.
    fn add_module(&mut self, checker: &Checker) {
        let module = checker.module_name;
        for &name in &checker.own_type_names {
            if let Some(info) = checker.structs.get(&name) {
                let fields = info.fields.iter().map(|(f, _)| *f).collect();
                self.structs.entry(module).or_default().insert(name, fields);
                self.type_params.entry(module).or_default().insert(name, info.params.clone());
            } else if let Some(info) = checker.unions.get(&name) {
                let by_tag = self.unions.entry(module).or_default();
                for (order, v) in info.variants.iter().enumerate() {
                    let fields = v.payload.iter().map(|(f, _)| *f).collect();
                    by_tag.insert(v.tag, VariantLayout { union: name, fields, order });
                }
                self.type_params.entry(module).or_default().insert(name, info.params.clone());
            }
        }
        for (&name, &field) in &checker.own_wrappers {
            self.wrappers.insert((module, name), field);
        }
    }

    /// The field an interface wrapper projects, when `body` is exactly
    /// `\d p1 .. pn = d.field p1 .. pn` over distinct variable patterns.
    pub(crate) fn wrapper_field_of(&self, module: StrId, name: StrId) -> Option<StrId> {
        self.wrappers.get(&(module, name)).copied()
    }

    /// The declared shape of every struct and union, for compile-time reflection.
    /// Unions are reconstructed from the tag-keyed table by grouping on the union
    /// name; a variant's arity is its payload field count (`0` for a unit payload).
    pub fn reflect(&self) -> ReflectInfo {
        let texts = |ids: &[StrId]| ids.iter().map(|i| i.to_string()).collect::<Vec<_>>();
        let params_of = |module: StrId, name: StrId| -> Vec<String> {
            self.type_params
                .get(&module)
                .and_then(|m| m.get(&name))
                .map_or_else(Vec::new, |ps| texts(ps))
        };
        let mut structs: Vec<(String, String, Vec<String>, Vec<String>)> = Vec::new();
        for (&module, fields_by_name) in &self.structs {
            for (&name, fields) in fields_by_name {
                structs.push((module.to_string(), name.to_string(), params_of(module, name), texts(fields)));
            }
        }
        // Group variants per union, carrying each variant's declaration order, then
        // sort by it so reflection reports variants in SOURCE order (the tag-keyed
        // map is unordered). A derived comparison orders variants by this.
        let mut unions_ord: Vec<(StrId, StrId, Vec<(StrId, usize, usize)>)> = Vec::new();
        for (&module, by_tag) in &self.unions {
            for (&tag, decl) in by_tag {
                let arity = decl.fields.len();
                match unions_ord
                    .iter_mut()
                    .find(|(m, n, _)| *m == module && *n == decl.union)
                {
                    Some((_, _, variants)) => {
                        if !variants.iter().any(|(t, _, _)| *t == tag) {
                            variants.push((tag, arity, decl.order));
                        }
                    }
                    None => unions_ord.push((module, decl.union, vec![(tag, arity, decl.order)])),
                }
            }
        }
        let unions = unions_ord
            .into_iter()
            .map(|(m, n, mut vs)| {
                vs.sort_by_key(|(_, _, order)| *order);
                let vs = vs.into_iter().map(|(t, a, _)| (t.to_string(), a)).collect();
                (m.to_string(), n.to_string(), params_of(m, n), vs)
            })
            .collect();
        ReflectInfo { structs, unions }
    }

    /// Resolve `pick` against `module`'s own declarations, then each imported
    /// module's, then any module (a last-resort global fallback).
    fn resolve<'d, T>(
        &'d self,
        table: &'d HashMap<StrId, HashMap<StrId, T>>,
        module: StrId,
        imports: &[StrId],
        key: StrId,
    ) -> Option<&'d T> {
        table
            .get(&module)
            .and_then(|m| m.get(&key))
            .or_else(|| imports.iter().find_map(|i| table.get(i).and_then(|m| m.get(&key))))
            .or_else(|| table.values().find_map(|m| m.get(&key)))
    }

    /// The payload arity and (union, field names) for a variant tag.
    pub(crate) fn variant(
        &self,
        module: StrId,
        imports: &[StrId],
        tag: StrId,
    ) -> Option<(StrId, &[Option<StrId>])> {
        self.resolve(&self.unions, module, imports, tag)
            .map(|v| (v.union, v.fields.as_slice()))
    }

    /// A struct's field names in declaration order, if `name` is a known struct.
    pub(crate) fn fields_of(&self, module: StrId, imports: &[StrId], name: StrId) -> Option<&[StrId]> {
        self.resolve(&self.structs, module, imports, name)
            .map(Vec::as_slice)
    }

    /// The struct whose exact set of field names matches an all-named literal,
    /// preferring the lowering module's own then imported declarations.
    pub(crate) fn struct_by_fields(&self, module: StrId, imports: &[StrId], names: &[StrId]) -> Option<StrId> {
        fn hit(structs: &HashMap<StrId, Vec<StrId>>, names: &[StrId]) -> Option<StrId> {
            structs.iter().find_map(|(sname, fields)| {
                let same = fields.len() == names.len()
                    && names.iter().all(|n| fields.contains(n));
                same.then_some(*sname)
            })
        }
        self.structs
            .get(&module)
            .and_then(|s| hit(s, names))
            .or_else(|| {
                imports
                    .iter()
                    .find_map(|i| self.structs.get(i).and_then(|s| hit(s, names)))
            })
            .or_else(|| self.structs.values().find_map(|s| hit(s, names)))
    }
}
