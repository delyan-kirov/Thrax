//! Foreign functions: marshalling specs and C struct layouts.
//!
//! Once a module is solved, each `@extern` node's type is read back to decide
//! how its one Thrax argument maps to positional C arguments and what C types
//! cross the boundary ([`Checker::build_extern_specs`]). A struct declared
//! `@struct @extern "C"` gets a C memory layout ([`Checker::clayout_of`]), so a
//! value of it can cross by value.
//!
//! Entry points: [`Checker::build_extern_specs`], [`Checker::clayout_of`].

use super::*;

// ## Marshalling specs

impl<'a> Checker<'a> {
    /// Validate every `@extern`'s shape and record its marshalling spec. A C
    /// function has no first-class closure to curry, so a Thrax `@extern` must be
    /// a function of EXACTLY ONE argument: the arrow spine may have only one link.
    /// A `{a: A, b: B} -> R` record groups multiple C parameters; `A -> B -> R`
    /// is currying and is rejected here with a diagnostic.
    pub(super) fn build_extern_specs(&mut self) -> Result<()> {
        let sites: Vec<(Aol<Expr>, Type)> = self
            .extern_tys
            .iter()
            .map(|(&e, ty)| (e, self.unifier.zonk(*ty)))
            .collect();
        for (e, ty) in sites {
            let span = self.ast.expr_span(e).unwrap_or_else(|| Span::at(0));
            let TypeNode::Arrow(param, ret, _) = self.unifier.types.node(ty) else {
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "an `@extern` must be a foreign function";
                    help: "declare it `A -> B` (a single argument), or `{{a: A, b: B}} -> R` \
                           to pass several C parameters"
                ));
            };
            if matches!(self.unifier.head(ret), TypeNode::Arrow(..)) {
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "a C `@extern` takes a single argument";
                    note: "C has no first-class functions to curry";
                    help: "group the C parameters into one record, `{{a: A, b: B}} -> R`, \
                           rather than currying as `A -> B -> R`"
                ));
            }
            let param = self.unifier.resolve(param);
            let (params, arg_types) = self.extern_param_spec(param)?;
            let ret_name = marshal_name(&self.unifier.types, self.unifier.resolve(ret));
            self.extern_specs.insert(e, (params, arg_types, ret_name));
        }
        Ok(())
    }

    /// Reduce an extern's single parameter type to its positional C arguments:
    /// unit takes none; an anonymous record contributes one C argument per field,
    /// in declared order, pulled by name; any other type is one C argument used
    /// directly (a scalar, a C-repr struct by value, `@Ptr`, `Str`, a callback, or
    /// a `List T` array).
    pub(super) fn extern_param_spec(&mut self, param: Type) -> Result<(Vec<ExternArg>, Vec<String>)> {
        if is_unit_ty(&self.unifier.types, param) {
            return Ok((vec![], vec![]));
        }
        // A name-keyed record: one C argument per field, in declared order,
        // pulled by name (so a reordered call site still marshals in C order).
        if let TypeNode::Record(_) = self.unifier.types.node(param) {
            let fields = self.record_fields_of(param)?;
            let mut params = Vec::with_capacity(fields.len());
            let mut arg_types = Vec::with_capacity(fields.len());
            for (name, fty) in fields {
                params.push(ExternArg::Field(name.to_string()));
                arg_types.push(marshal_name(&self.unifier.types, self.unifier.resolve(fty)));
            }
            return Ok((params, arg_types));
        }
        // A closed record parameter surfaces as a positional tuple: one C argument
        // per element, in order.
        if let TypeNode::Tuple(items) = self.unifier.types.node(param) {
            let mut params = Vec::with_capacity(items.len());
            let mut arg_types = Vec::with_capacity(items.len());
            for (i, it) in self.unifier.types.items(items).to_vec().into_iter().enumerate() {
                params.push(ExternArg::Elem(i));
                let r = self.unifier.resolve(it);
                arg_types.push(marshal_name(&self.unifier.types, r));
            }
            return Ok((params, arg_types));
        }
        Ok((vec![ExternArg::Whole], vec![marshal_name(&self.unifier.types, param)]))
    }
}

// ## C layouts

impl<'a> Checker<'a> {
    /// Compute and validate the C memory layout of every C-repr struct. Runs after
    /// `with` splicing, so a spliced-in field is laid out too. A C-repr struct may
    /// not be generic (a C type is monomorphic), and each field must be a
    /// C-representable scalar or a nested C-repr struct.
    pub(super) fn validate_crepr_structs(&mut self) -> Result<()> {
        let names: Vec<StrId> = self
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

    /// The C layout of C-repr struct `name`, computed recursively through nested
    /// C-repr struct fields. `visiting` guards against a struct that (transitively)
    /// contains itself by value, which has no finite C layout.
    pub(super) fn clayout_of(
        &self,
        name: StrId,
        visiting: &mut HashSet<StrId>,
    ) -> Result<utilities::CLayout> {
        if !visiting.insert(name) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "C-repr struct `{name}` contains itself by value (an infinite C layout)"
            ));
        }
        let info = self.structs.get(&name).expect("crepr struct registered").clone();
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
                    let mut cn = *cn;
                    let mut steps = 0;
                    let kind = loop {
                        if let Some(k) = scalar_ckind(cn.as_str(), ptr_bits) {
                            break k;
                        }
                        if self.structs.get(&cn).map(|s| s.crepr).unwrap_or(false) {
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
                                return Err(crepr_field_error(name, fname, cn.as_str())
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
        visiting.remove(&name);
        Ok(if info.c_union {
            utilities::CLayout::of_union(fields)
        } else {
            utilities::CLayout::of(fields)
        })
    }
}

/// The mangled global name for one overload: `name#<type-key>`. Two overloads of
/// one name in one module get distinct keys, so their globals no longer collide
//// Whether a type is unit `{}` (a nullary C function's zero-argument parameter),
/// as either the `{}` constructor or the empty tuple.
pub(super) fn is_unit_ty(types: &Types, ty: Type) -> bool {
    match types.node(ty) {
        TypeNode::Con(n) => types.name(n) == ty::UNIT,
        TypeNode::Tuple(v) => v.is_empty(),
        _ => false,
    }
}

/// A type's marshalling name for the FFI seam. A type variable or any composite
/// (the checker's fallback) marshals word-sized, so
/// the backends read it as `Int`.
pub(super) fn marshal_name(types: &Types, ty: Type) -> String {
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
        // A `@Vec T` parameter is a C array of `T`: passed as a `T*` pointing at a
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

pub(super) fn crepr_field_error(ty: impl Display, field: impl Display, field_ty: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "field `{field}` of C-repr struct `{ty}` has type `{field_ty}`, which is not \
         C-representable; a `@struct @extern \"C\"` field must be a sized number \
         (`@Int8..64`/`@Nat8..64`/`@Float32`/`@Float64`), `@Int`/`@Nat`/`Real`, `@Ptr`, \
         `@Bool`, or another C-repr struct"
    )
}

/// Map a scalar type name (friendly or `@`-sigil) to its fixed-width C kind.
/// `Int`/`Nat`/`Ptr` resolve to the target's word width. Returns `None` for a
/// non-scalar (a nested struct, a variable, or a non-C type).
pub(super) fn scalar_ckind(name: &str, ptr_bits: u32) -> Option<utilities::CKind> {
    use utilities::CKind::*;
    let word = if ptr_bits == 32 { S32 } else { S64 };
    let uword = if ptr_bits == 32 { U32 } else { U64 };
    Some(match name {
        "@Int8" => S8,
        "@Int16" => S16,
        "@Int32" => S32,
        "@Int64" => S64,
        "@Nat8" => U8,
        "@Nat16" => U16,
        "@Nat32" => U32,
        "@Nat64" => U64,
        "@Float32" => F32,
        "@Float64" => F64,
        "@Int" => word,
        "@Nat" => uword,
        "@Ptr" => uword,
        "@Bool" => U8,
        _ => return None,
    })
}
