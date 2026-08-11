//! The IR-to-C emitter: the port of `engines/CC.cpp`. Each lifted [`Code`] is
//! lowered to BLOCK FUNCTIONS driven by the CEK machine in the runtime
//! (`runtime.c`): a block runs straight-line C (atoms, pure lets, case branching)
//! and ends by calling exactly one TERMINATOR (`THxK_ret` / `THxK_tailcall` /
//! `THxK_apply` / `THxK_jump` / `THxK_handle` / `THxK_defer_run`). A non-tail
//! application is a suspension point: the rest of the computation becomes a
//! continuation block, so the delimited continuation a handler captures is a pure
//! heap object rather than live C-stack frames.
//!
//! The lowering is continuation-passing: [`Emitter::emit_expr`] lowers an
//! expression so its value reaches a [`Sink`], either delivered up the
//! continuation (`RET`) or written to a frame slot before jumping to a
//! continuation block (`SLOT`). Pure lets stay inline; only suspending
//! sub-expressions split into fresh blocks.

use std::collections::{HashMap, HashSet};

use frontend::ir::data::{Atom, AltKind, Expr, Program};

/// Where a computed value goes.
#[derive(Clone, Copy)]
enum Sink {
    /// Delivered up the continuation stack (`THxK_ret` / `THxK_tailcall`).
    Ret,
    /// Written into frame `slot` (a let-box back-patch); then, if `cont` is
    /// `Some`, control jumps to that continuation block. `None` is the pure-into
    /// sink (back-patch and fall through, used by a pure Case's branches).
    Slot { slot: usize, cont: Option<usize> },
}

/// A distinct `@extern` binding the C backend emits a wrapper for. Deduplicated
/// by symbol + library + signature; a `THxRT_extern(idx, arity)` value points at
/// its wrapper in the generated `THxRT_extern_table`.
#[derive(Clone)]
pub struct ExternSite {
    pub abi: String,
    pub symbol: String,
    pub lib: String,
    pub arg_types: Vec<String>,
    pub ret_type: String,
}

pub struct Emitter<'p> {
    prog: &'p Program,
    /// Canonical names of every global (`Module.name`); a `Glob` naming one
    /// resolves to a CAF.
    globals: HashSet<&'p str>,
    /// Bare last segments of the global names, the unqualified/entry fallback.
    bare: HashSet<&'p str>,
    /// Effect name -> the operations it declares (for qualified resolution).
    ops_by_effect: HashMap<&'p str, Vec<&'p str>>,
    /// Operation name -> the effects declaring it (for bare resolution).
    ops_by_name: HashMap<&'p str, Vec<&'p str>>,
    /// The distinct foreign functions, in wrapper-table order.
    externs: Vec<ExternSite>,
    /// Dedup key (`symbol\x1flib\x1fsig`) -> its index in `externs`.
    extern_idx: HashMap<String, usize>,
    /// One entry per emitted block: its full C function text, filled out of order
    /// (a block is reserved, then set once its body is built).
    blocks: Vec<String>,
    /// Code index -> its entry block id.
    code_entry: Vec<usize>,
    /// Codes reachable from the entry global; unreachable codes are emitted as
    /// trap stubs and their `@extern` sites are not collected.
    reachable: HashSet<usize>,
    tmp: usize,
}

/// The dedup key for an extern site.
fn extern_key(abi: &str, symbol: &str, lib: &str, arg_types: &[String], ret_type: &str) -> String {
    format!("{abi}\x1f{symbol}\x1f{lib}\x1f{}\x1f{ret_type}", arg_types.join(","))
}

/// Visit every [`Atom`] occurring in `e` (including those captured inside a
/// nested [`Atom::Clos`]).
fn for_each_atom(e: &Expr, f: &mut impl FnMut(&Atom)) {
    match e {
        Expr::Ret(a) | Expr::Field { rec: a, .. } => visit_atom(a, f),
        Expr::App { fun, arg, .. } => {
            visit_atom(fun, f);
            visit_atom(arg, f);
        }
        Expr::MkStruct { base, fields, .. } => {
            if let Some(b) = base {
                visit_atom(b, f);
            }
            for (_, v) in fields {
                visit_atom(v, f);
            }
        }
        Expr::MkVariant { fields, .. } => {
            for v in fields {
                visit_atom(v, f);
            }
        }
        Expr::MkTuple(items) => {
            for v in items {
                visit_atom(v, f);
            }
        }
        Expr::Fault(_) => {}
        Expr::Let { rhs, body, .. } => {
            for_each_atom(rhs, f);
            for_each_atom(body, f);
        }
        Expr::Case { scrut, alts, default } => {
            visit_atom(scrut, f);
            for al in alts {
                for_each_atom(&al.body, f);
            }
            for_each_atom(default, f);
        }
        Expr::Handle { body, clauses, els } => {
            for_each_atom(body, f);
            for c in clauses {
                visit_atom(&c.fun, f);
            }
            visit_atom(els, f);
        }
        Expr::Defer { cleanup, body } => {
            visit_atom(cleanup, f);
            for_each_atom(body, f);
        }
    }
}

fn visit_atom(a: &Atom, f: &mut impl FnMut(&Atom)) {
    f(a);
    if let Atom::Clos { captures, .. } = a {
        for c in captures {
            visit_atom(c, f);
        }
    }
}

/// The codes reachable from the entry global, resolving a `Glob` reference the
/// way the runtime does (exact canonical global, else the bare last-segment
/// fallback; builtins/`TARGET`/operations are not codes) and following each
/// `Clos`'s lifted code. Unreachable codes (e.g. the auto-injected `C` libc
/// bindings a program never calls) are pruned so their `@extern` wrappers are
/// not emitted; on a strict-signature target like wasm those redeclarations
/// would otherwise clash with the real libc.
pub fn reachable_codes(prog: &Program, entry: &str) -> HashSet<usize> {
    let mut canon: HashMap<&str, usize> = HashMap::new();
    let mut bare: HashMap<&str, usize> = HashMap::new();
    for (name, code) in &prog.globals {
        canon.entry(name.as_str()).or_insert(*code);
        let last = name.rsplit('.').next().unwrap_or(name);
        bare.entry(last).or_insert(*code);
    }
    let resolve = |name: &str| -> Option<usize> {
        canon.get(name).copied().or_else(|| {
            let last = name.rsplit('.').next().unwrap_or(name);
            bare.get(last).copied()
        })
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut work: Vec<usize> = resolve(entry).into_iter().collect();
    while let Some(c) = work.pop() {
        if !seen.insert(c) {
            continue;
        }
        for_each_atom(&prog.codes[c].body, &mut |a| match a {
            Atom::Glob { name } => {
                if let Some(cc) = resolve(name) {
                    work.push(cc);
                }
            }
            Atom::Clos { code, .. } => work.push(*code),
            _ => {}
        });
    }
    seen
}

/// Register any `Atom::Extern` reachable in `e` into the table (deduplicated).
fn collect_externs(
    e: &Expr,
    externs: &mut Vec<ExternSite>,
    idx: &mut HashMap<String, usize>,
) {
    let mut atom = |a: &Atom| collect_extern_atom(a, externs, idx);
    match e {
        Expr::Ret(a) | Expr::Field { rec: a, .. } => atom(a),
        Expr::App { fun, arg, .. } => {
            atom(fun);
            atom(arg);
        }
        Expr::MkStruct { base, fields, .. } => {
            if let Some(b) = base {
                atom(b);
            }
            fields.iter().for_each(|(_, v)| atom(v));
        }
        Expr::MkVariant { fields, .. } => fields.iter().for_each(&mut atom),
        Expr::MkTuple(items) => items.iter().for_each(&mut atom),
        Expr::Fault(_) => {}
        Expr::Let { rhs, body, .. } => {
            collect_externs(rhs, externs, idx);
            collect_externs(body, externs, idx);
        }
        Expr::Case { scrut, alts, default } => {
            collect_extern_atom(scrut, externs, idx);
            for al in alts {
                collect_externs(&al.body, externs, idx);
            }
            collect_externs(default, externs, idx);
        }
        Expr::Handle { body, clauses, els } => {
            collect_externs(body, externs, idx);
            for c in clauses {
                collect_extern_atom(&c.fun, externs, idx);
            }
            collect_extern_atom(els, externs, idx);
        }
        Expr::Defer { cleanup, body } => {
            collect_extern_atom(cleanup, externs, idx);
            collect_externs(body, externs, idx);
        }
    }
}

fn collect_extern_atom(a: &Atom, externs: &mut Vec<ExternSite>, idx: &mut HashMap<String, usize>) {
    match a {
        Atom::Extern {
            abi,
            symbol,
            lib,
            arg_types,
            ret_type,
        } => {
            let key = extern_key(abi, symbol, lib, arg_types, ret_type);
            idx.entry(key).or_insert_with(|| {
                externs.push(ExternSite {
                    abi: abi.clone(),
                    symbol: symbol.clone(),
                    lib: lib.clone(),
                    arg_types: arg_types.clone(),
                    ret_type: ret_type.clone(),
                });
                externs.len() - 1
            });
        }
        Atom::Clos { captures, .. } => {
            for c in captures {
                collect_extern_atom(c, externs, idx);
            }
        }
        _ => {}
    }
}

/// A C string literal (with surrounding quotes) for an arbitrary byte view.
pub fn cstr(bytes: &[u8]) -> String {
    let mut s = String::from("\"");
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(c as char),
            _ => {
                s.push_str(&format!("\\x{c:02x}"));
                // Guard against a following hex digit extending the escape.
                if let Some(&n) = bytes.get(i + 1) {
                    if n.is_ascii_hexdigit() {
                        s.push_str("\"\"");
                    }
                }
            }
        }
    }
    s.push('"');
    s
}

/// A C double literal that round-trips the value.
fn cdbl(v: f64) -> String {
    format!("{v:.17e}")
}

/// How a Thrax scalar crosses the C ABI in a generated `@extern` wrapper.
/// `Int(t)` names the exact C integer type; the word-sized fallback (`Int`, a
/// type variable, or any unrecognized name) is `int64_t`, matching the C++
/// `desc_of`. Both the friendly (`Int8`) and `@`-sigil (`@int8`) spellings map
/// to the same ABI.
enum Cabi {
    Bytes,             // Str/Array: the byte pointer (char*)
    Ptr,               // Ptr: an opaque pointer carried as a word (void*)
    F32,               // Real32: float
    F64,               // Real/Real64: double
    Unit,              // {}: a void result
    Int(&'static str), // a sized/word integer: the named C int type
}

fn cabi(name: &str) -> Cabi {
    match name {
        "Str" | "Array" | "@str" => Cabi::Bytes,
        "Ptr" | "@ptr" => Cabi::Ptr,
        "Real32" | "@float32" => Cabi::F32,
        "Real" | "Real64" | "@float64" => Cabi::F64,
        "{}" => Cabi::Unit,
        "Int8" | "@int8" => Cabi::Int("int8_t"),
        "Int16" | "@int16" => Cabi::Int("int16_t"),
        "Int32" | "@int32" => Cabi::Int("int32_t"),
        "Int64" | "@int64" => Cabi::Int("int64_t"),
        "Nat8" | "@nat8" => Cabi::Int("uint8_t"),
        "Nat16" | "@nat16" => Cabi::Int("uint16_t"),
        "Nat32" | "@nat32" => Cabi::Int("uint32_t"),
        "Nat64" | "@nat64" | "Nat" => Cabi::Int("uint64_t"),
        _ => Cabi::Int("int64_t"),
    }
}

/// The C parameter type and the expression marshalling `args[i]` into it.
fn c_param(name: &str, i: usize) -> (&'static str, String) {
    match cabi(name) {
        Cabi::Bytes => ("char*", format!("(char*)THxVALUE_str(args[{i}])")),
        Cabi::Ptr => ("void*", format!("(void*)(intptr_t)THxVALUE_as_int(args[{i}])")),
        Cabi::F32 => ("float", format!("(float)THxVALUE_as_num(args[{i}])")),
        Cabi::F64 => ("double", format!("THxVALUE_as_num(args[{i}])")),
        // A `{}` parameter is unusual; treat it as a word.
        Cabi::Unit => ("int64_t", format!("(int64_t)THxVALUE_as_int(args[{i}])")),
        Cabi::Int(t) => (t, format!("({t})THxVALUE_as_int(args[{i}])")),
    }
}

/// The C return type, and (for a non-void result) the expression wrapping the
/// C result `_r` back into a `Value*`.
fn c_ret(name: &str) -> (&'static str, Option<&'static str>) {
    match cabi(name) {
        Cabi::Bytes => ("char*", Some("_r ? THxRT_str(_r, strlen(_r)) : THxRT_str(\"\", 0)")),
        Cabi::Ptr => ("void*", Some("THxRT_int((long long)(intptr_t)_r)")),
        Cabi::F32 => ("float", Some("THxRT_real((double)_r)")),
        Cabi::F64 => ("double", Some("THxRT_real(_r)")),
        Cabi::Unit => ("void", None),
        Cabi::Int(t) => (t, Some("THxRT_int((long long)_r)")),
    }
}

/// Emit the foreign-function wrappers and the `THxRT_extern_table` the runtime
/// dispatches through. Each wrapper declares the C symbol with an `__asm__`
/// label (a direct call, resolved by the linker) and marshals the collected
/// arguments across the seam.
pub fn emit_extern_table(externs: &[ExternSite]) -> String {
    let mut out = String::from("\n/* -- foreign functions (@extern) -- */\n#include <stdint.h>\n");
    let libs: Vec<&str> = {
        let mut ls: Vec<&str> = externs
            .iter()
            .filter(|e| e.abi != "WASM")
            .map(|e| e.lib.as_str())
            .collect();
        ls.sort_unstable();
        ls.dedup();
        ls
    };
    if !libs.is_empty() {
        out.push_str(&format!("/* link against: {} */\n", libs.join(", ")));
    }
    for (n, e) in externs.iter().enumerate() {
        // A `"WASM"` extern is a host import from the wasm embedder; standalone C
        // has no such host, so its wrapper faults if ever called. (In the browser
        // playground this program is interpreted, not compiled to C.)
        if e.abi == "WASM" {
            out.push_str(&format!(
                "static Value* THx_extern_{n}(Value** args) {{\n  (void)args;\n  \
                 thrax_fault({});\n}}\n",
                cstr(format!("@extern \"WASM\" host import `{}` unavailable in native C", e.symbol)
                    .as_bytes())
            ));
            continue;
        }
        let (ret_ty, wrap) = c_ret(&e.ret_type);
        // A `{}` parameter carries no C argument (a `void`-taking function like
        // `getchar`); it stays in the Thrax arity but is dropped from the call.
        let params: Vec<(String, String)> = e
            .arg_types
            .iter()
            .enumerate()
            .filter(|(_, t)| !matches!(cabi(t), Cabi::Unit))
            .map(|(i, t)| {
                let (ty, expr) = c_param(t, i);
                (ty.to_string(), expr)
            })
            .collect();
        let sig = if params.is_empty() {
            "void".to_string()
        } else {
            params.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>().join(", ")
        };
        out.push_str(&format!(
            "extern {ret_ty} THx_sym_{n}({sig}) __asm__({});\n",
            cstr(e.symbol.as_bytes())
        ));
        out.push_str(&format!("static Value* THx_extern_{n}(Value** args) {{\n  (void)args;\n"));
        let call_args: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(i, (ty, expr))| {
                out.push_str(&format!("  {ty} a{i} = {expr};\n"));
                format!("a{i}")
            })
            .collect();
        let call = format!("THx_sym_{n}({})", call_args.join(", "));
        match wrap {
            None => {
                out.push_str(&format!("  {call};\n  return THxRT_unit();\n"));
            }
            Some(w) => {
                out.push_str(&format!("  {ret_ty} _r = {call};\n  return {w};\n"));
            }
        }
        out.push_str("}\n");
    }
    let n = externs.len();
    if n == 0 {
        out.push_str("ExternFn THxRT_extern_table[1] = {0};\n");
    } else {
        out.push_str("ExternFn THxRT_extern_table[] = {\n");
        for i in 0..n {
            out.push_str(&format!("  THx_extern_{i},\n"));
        }
        out.push_str("};\n");
    }
    out.push_str(&format!("const size_t THxRT_extern_count = {n};\n"));
    out
}

/// The arity of a built-in operator, or `None`. Mirrors the runtime's table and
/// the interpreter's `builtin_arity`.
fn builtin_arity(name: &str) -> Option<usize> {
    let n = match name {
        "not" | "neg" | "@array_len" | "@array_alloc" | "@vec_len" | "@vec_new" | "@tensor_length" => 1,
        "+" | "-" | "*" | "/" | "%" | "?=" | "?<" | "?>" | "<=" | ">=" | "++" | "@array_get"
        | "@array_push" | "@vec_get" | "@vec_push" | "@vec_fill" | "record_without"
        | "@tensor_concat" | "@tensor_index" | "@tensor_create" => 2,
        "@array_set" | "@array_slice" | "@vec_set" => 3,
        _ => return None,
    };
    Some(n)
}

impl<'p> Emitter<'p> {
    pub fn new(prog: &'p Program, reachable: HashSet<usize>) -> Emitter<'p> {
        let globals: HashSet<&str> = prog.globals.iter().map(|(n, _)| n.as_str()).collect();
        let bare = prog
            .globals
            .iter()
            .map(|(n, _)| n.rsplit('.').next().unwrap_or(n))
            .collect();
        let mut ops_by_effect: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut ops_by_name: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &prog.effects {
            ops_by_effect
                .entry(e.effect.as_str())
                .or_default()
                .push(e.op.as_str());
            ops_by_name
                .entry(e.op.as_str())
                .or_default()
                .push(e.effect.as_str());
        }
        for effects in ops_by_name.values_mut() {
            effects.sort_unstable();
            effects.dedup();
        }
        let mut externs = Vec::new();
        let mut extern_idx = HashMap::new();
        for (id, code) in prog.codes.iter().enumerate() {
            if reachable.contains(&id) {
                collect_externs(&code.body, &mut externs, &mut extern_idx);
            }
        }
        Emitter {
            prog,
            globals,
            bare,
            ops_by_effect,
            ops_by_name,
            externs,
            extern_idx,
            blocks: Vec::new(),
            code_entry: vec![0; prog.codes.len()],
            reachable,
            tmp: 0,
        }
    }

    /// Emit every code, then return the finished block function texts, the code
    /// index -> entry block map, and the foreign-function table.
    pub fn run(mut self) -> (Vec<String>, Vec<usize>, Vec<ExternSite>) {
        for id in 0..self.prog.codes.len() {
            self.emit_code(id);
        }
        (self.blocks, self.code_entry, self.externs)
    }

    fn fresh(&mut self, p: &str) -> String {
        let s = format!("{p}{}", self.tmp);
        self.tmp += 1;
        s
    }

    fn reserve_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(String::new());
        id
    }

    fn set_block(&mut self, id: usize, body: &str) {
        self.blocks[id] = format!(
            "static void blk_{id}(Frame* fr, Value* in) {{\n  (void)fr; (void)in;\n{body}}}\n\n"
        );
    }

    /// Resolve a `Glob` atom's single canonical name to its C expression,
    /// mirroring the machine's `glob` order: an EXACT canonical global first, then
    /// a built-in, a `TARGET.` member, an effect operation, and only LAST the
    /// unqualified fallback. Ops precede that fallback so an unrelated imported
    /// global whose last segment aliases into `bare` cannot shadow a same-named
    /// operation. The `C` libc namespace resolves as exact `@extern` globals.
    fn glob_atom(&self, name: &str) -> String {
        if self.globals.contains(name) {
            return format!("THxRT_glob({})", cstr(name.as_bytes()));
        }
        if let Some(arity) = builtin_arity(name) {
            return format!("THxRT_builtin({}, {arity})", cstr(name.as_bytes()));
        }
        if let Some(suffix) = name.strip_prefix("TARGET.") {
            return format!("THxRT_target({})", cstr(suffix.as_bytes()));
        }
        // An effect operation: `Effect.op` picks that effect, a bare op the sole
        // effect declaring it (else ambient / NULL, resolved by the handler).
        if let Some((eff, op)) = name.split_once('.') {
            if self.ops_by_effect.get(eff).is_some_and(|o| o.contains(&op)) {
                return format!("THxK_op({}, {})", cstr(eff.as_bytes()), cstr(op.as_bytes()));
            }
        }
        if let Some(effs) = self.ops_by_name.get(name) {
            let eff = if effs.len() == 1 {
                cstr(effs[0].as_bytes())
            } else {
                "NULL".to_string()
            };
            return format!("THxK_op({eff}, {})", cstr(name.as_bytes()));
        }
        // The unqualified fallback: a reference the codegen left bare (the entry
        // point, or a name not resolved to a module), matched by last segment.
        if self.bare.contains(name) {
            return format!("THxRT_glob({})", cstr(name.as_bytes()));
        }
        // An unknown bare name: an ambient effect operation, resolved by a handler.
        format!("THxK_op(NULL, {})", cstr(name.as_bytes()))
    }

    /// Each atom is a single, side-effect-light C expression.
    fn atom(&self, a: &Atom) -> String {
        match a {
            Atom::Local(i) => format!("THxVALUE_local(fr->locals, fr->nlocals, {i})"),
            Atom::Env(i) => format!("THxVALUE_env(fr->env, fr->nenv, {i})"),
            Atom::Glob { name } => self.glob_atom(name),
            Atom::LitI(n) => format!("THxRT_int({n}LL)"),
            Atom::LitR(r) => format!("THxRT_real({})", cdbl(*r)),
            Atom::LitS(v) => format!("THxRT_str({}, {})", cstr(v), v.len()),
            Atom::LitB(b) => format!("THxRT_bool({})", if *b { 1 } else { 0 }),
            Atom::Unit => "THxRT_unit()".into(),
            Atom::Clos { code, captures } => {
                if captures.is_empty() {
                    format!("THxRT_closure({code}, NULL, 0)")
                } else {
                    let caps: Vec<String> = captures.iter().map(|c| self.atom(c)).collect();
                    format!(
                        "THxRT_closure({code}, (Value*[]){{ {} }}, {})",
                        caps.join(", "),
                        captures.len()
                    )
                }
            }
            Atom::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
            } => {
                let key = extern_key(abi, symbol, lib, arg_types, ret_type);
                let idx = self.extern_idx[&key];
                format!("THxRT_extern({idx}, {})", arg_types.len())
            }
        }
    }

    /// A single C expression for a value-producing, non-suspending, non-branching
    /// expression. Case/Let/App/Handle/Defer are structural and never reach here.
    fn value_expr(&self, e: &Expr) -> String {
        match e {
            Expr::Ret(a) => self.atom(a),
            Expr::MkStruct { name, base, fields } => {
                let n = fields.len();
                let (fnames, vals) = if n == 0 {
                    ("NULL".to_string(), "NULL".to_string())
                } else {
                    let ns: Vec<String> = fields.iter().map(|(f, _)| cstr(f.as_bytes())).collect();
                    let vs: Vec<String> = fields.iter().map(|(_, v)| self.atom(v)).collect();
                    (
                        format!("(const char*[]){{ {} }}", ns.join(", ")),
                        format!("(Value*[]){{ {} }}", vs.join(", ")),
                    )
                };
                match base {
                    Some(b) => format!(
                        "THxRT_struct_update({}, {}, {n}, {fnames}, {vals})",
                        self.atom(b),
                        cstr(name.as_bytes())
                    ),
                    None => {
                        format!("THxRT_struct({}, {n}, {fnames}, {vals})", cstr(name.as_bytes()))
                    }
                }
            }
            Expr::Field { rec, name } => {
                format!("THxVALUE_field({}, {})", self.atom(rec), cstr(name.as_bytes()))
            }
            Expr::MkVariant { ty, tag, fields } => {
                if fields.is_empty() {
                    format!(
                        "THxRT_variant({}, {}, 0, NULL)",
                        cstr(ty.as_bytes()),
                        cstr(tag.as_bytes())
                    )
                } else {
                    let vs: Vec<String> = fields.iter().map(|f| self.atom(f)).collect();
                    format!(
                        "THxRT_variant({}, {}, {}, (Value*[]){{ {} }})",
                        cstr(ty.as_bytes()),
                        cstr(tag.as_bytes()),
                        fields.len(),
                        vs.join(", ")
                    )
                }
            }
            Expr::MkTuple(items) => {
                if items.is_empty() {
                    "THxRT_tuple(NULL, 0)".into()
                } else {
                    let vs: Vec<String> = items.iter().map(|a| self.atom(a)).collect();
                    format!("THxRT_tuple((Value*[]){{ {} }}, {})", vs.join(", "), items.len())
                }
            }
            Expr::Let { .. }
            | Expr::App { .. }
            | Expr::Case { .. }
            | Expr::Handle { .. }
            | Expr::Defer { .. } => unreachable!("value_expr on a non-value expression"),
            Expr::Fault(_) => unreachable!("Fault is emitted structurally, not as a value"),
        }
    }

    /// Does `e` perform a call or install a handler/defer anywhere in the value it
    /// produces? Then a `let` RHS must deliver through a continuation block. Counts
    /// tail Apps too: a `let` RHS is never really in tail position.
    fn has_call(&self, e: &Expr) -> bool {
        match e {
            Expr::Ret(_)
            | Expr::MkStruct { .. }
            | Expr::Field { .. }
            | Expr::MkVariant { .. }
            | Expr::MkTuple(_)
            | Expr::Fault(_) => false,
            Expr::App { .. } | Expr::Handle { .. } | Expr::Defer { .. } => true,
            Expr::Let { rhs, body, .. } => self.has_call(rhs) || self.has_call(body),
            Expr::Case { alts, default, .. } => {
                alts.iter().any(|a| self.has_call(&a.body)) || self.has_call(default)
            }
        }
    }

    /// Deliver a finished value C-expression to its sink.
    fn deliver(&self, val: &str, sink: Sink, out: &mut String) {
        match sink {
            Sink::Ret => out.push_str(&format!("  THxK_ret({val});\n")),
            Sink::Slot { slot, cont } => {
                out.push_str(&format!("  THxK_backpatch(fr, {slot}, {val});\n"));
                if let Some(cont) = cont {
                    out.push_str(&format!("  THxK_jump(blk_{cont});\n"));
                }
            }
        }
    }

    /// Compute a pure (suspension-free) expression's value into frame `slot`,
    /// emitted inline. Never reaches App/Handle/Defer.
    fn emit_pure_into(&mut self, e: &Expr, slot: usize, out: &mut String) {
        match e {
            Expr::Ret(_)
            | Expr::MkStruct { .. }
            | Expr::Field { .. }
            | Expr::MkVariant { .. }
            | Expr::MkTuple(_) => {
                let val = self.value_expr(e);
                out.push_str(&format!("  THxK_backpatch(fr, {slot}, {val});\n"));
            }
            Expr::Fault(msg) => {
                out.push_str(&format!("  thrax_fault({});\n", cstr(msg.as_bytes())))
            }
            Expr::Case { .. } => self.emit_case(e, Sink::Slot { slot, cont: None }, out),
            Expr::Let {
                slot: inner,
                rhs,
                body,
            } => {
                out.push_str(&format!("  THxK_setbox(fr, {inner});\n"));
                self.emit_pure_into(rhs, *inner, out);
                self.emit_pure_into(body, slot, out);
            }
            Expr::App { .. } | Expr::Handle { .. } | Expr::Defer { .. } => {
                unreachable!("emit_pure_into on a suspending expression")
            }
        }
    }

    /// The core continuation-passing lowering: lower `e` so its value reaches
    /// `sink`.
    fn emit_expr(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        match e {
            Expr::Ret(_)
            | Expr::MkStruct { .. }
            | Expr::Field { .. }
            | Expr::MkVariant { .. }
            | Expr::MkTuple(_) => {
                let val = self.value_expr(e);
                self.deliver(&val, sink, out);
            }
            Expr::Fault(msg) => {
                out.push_str(&format!("  thrax_fault({});\n", cstr(msg.as_bytes())))
            }

            Expr::App { fun, arg, .. } => {
                let fn_ = self.atom(fun);
                let arg = self.atom(arg);
                // Tail-ness follows the sink, not the ANF flag: only a RET sink is a
                // genuine tail position. A call the ANF marked tail but which IR
                // lowering hoisted into a `let` is delivered into a slot: a normal,
                // non-tail call.
                match sink {
                    Sink::Ret => out.push_str(&format!("  THxK_tailcall({fn_}, {arg});\n")),
                    Sink::Slot { slot, cont } => {
                        let cont = cont.expect("a non-tail App needs a continuation block");
                        out.push_str(&format!(
                            "  THxK_apply(fr, {fn_}, {arg}, blk_{cont}, {slot});\n"
                        ));
                    }
                }
            }

            Expr::Let { .. } => self.emit_let(e, sink, out),
            Expr::Case { .. } => self.emit_case(e, sink, out),
            Expr::Handle { .. } => self.emit_handle(e, sink, out),
            Expr::Defer { .. } => self.emit_defer(e, sink, out),
        }
    }

    fn emit_let(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        let Expr::Let { slot, rhs, body } = e else {
            unreachable!()
        };
        out.push_str(&format!("  THxK_setbox(fr, {slot});\n"));
        if self.has_call(rhs) {
            // The body becomes a continuation block; the rhs delivers into the slot
            // then control transfers there.
            let body_blk = self.reserve_block();
            self.emit_expr(
                rhs,
                Sink::Slot {
                    slot: *slot,
                    cont: Some(body_blk),
                },
                out,
            );
            let mut body_out = String::new();
            self.emit_expr(body, sink, &mut body_out);
            self.set_block(body_blk, &body_out);
        } else {
            // Pure rhs: compute into the slot inline, then continue in the same
            // block.
            self.emit_pure_into(rhs, *slot, out);
            self.emit_expr(body, sink, out);
        }
    }

    fn emit_case(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        let Expr::Case {
            scrut,
            alts,
            default,
        } = e
        else {
            unreachable!()
        };
        let s = self.fresh("scrut");
        out.push_str(&format!("  Value* {s} = {};\n", self.atom(scrut)));
        let mut first = true;
        for al in alts {
            let cond = match &al.kind {
                AltKind::Int(n) => format!("THxVALUE_as_int({s}) == {n}LL"),
                AltKind::Real(r) => format!("THxVALUE_as_num({s}) == {}", cdbl(*r)),
                AltKind::Bool(b) => {
                    format!("THxVALUE_as_bool({s}) == {}", if *b { 1 } else { 0 })
                }
                AltKind::Con(c) => {
                    format!("strcmp(THxVALUE_ctor({s}), {}) == 0", cstr(c.as_bytes()))
                }
            };
            out.push_str(&format!(
                "  {} ({cond}) {{\n",
                if first { "if" } else { "else if" }
            ));
            if let AltKind::Con(_) = &al.kind {
                for i in 0..al.binders.len() {
                    out.push_str(&format!(
                        "  THxK_setlocal(fr, {}, THxVALUE_variant_field({s}, {i}));\n",
                        al.binder_base + i
                    ));
                }
            }
            self.emit_branch(&al.body, sink, out);
            out.push_str("  }\n");
            first = false;
        }
        out.push_str(if first { "  {\n" } else { "  else {\n" });
        self.emit_branch(default, sink, out);
        out.push_str("  }\n");
    }

    /// A case branch: either a real sink, or the pure-into sink (back-patch and
    /// fall through, so the enclosing pure let continues after the if/else).
    fn emit_branch(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        match sink {
            Sink::Slot { slot, cont: None } => self.emit_pure_into(e, slot, out),
            _ => self.emit_expr(e, sink, out),
        }
    }

    fn emit_handle(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        let Expr::Handle {
            body,
            clauses,
            els,
        } = e
        else {
            unreachable!()
        };
        let hbody = self.reserve_block();
        let (effs, ops, cls) = if clauses.is_empty() {
            ("NULL".to_string(), "NULL".to_string(), "NULL".to_string())
        } else {
            let effs: Vec<String> = clauses
                .iter()
                .map(|c| match &c.effect {
                    Some(e) => cstr(e.as_bytes()),
                    None => "NULL".into(),
                })
                .collect();
            let ops: Vec<String> = clauses.iter().map(|c| cstr(c.op.as_bytes())).collect();
            let cls: Vec<String> = clauses.iter().map(|c| self.atom(&c.fun)).collect();
            (
                format!("(const char*[]){{ {} }}", effs.join(", ")),
                format!("(const char*[]){{ {} }}", ops.join(", ")),
                format!("(Value*[]){{ {} }}", cls.join(", ")),
            )
        };
        let els = self.atom(els);
        let (cont, slot) = match sink {
            Sink::Ret => ("NULL".to_string(), 0),
            Sink::Slot { slot, cont } => {
                let cont = cont.expect("a non-tail Handle needs a continuation block");
                (format!("blk_{cont}"), slot)
            }
        };
        out.push_str(&format!(
            "  THxK_handle(fr, {cont}, {slot}, {effs}, {ops}, {cls}, {}, {els}, blk_{hbody});\n",
            clauses.len()
        ));
        let mut body_out = String::new();
        self.emit_expr(body, Sink::Ret, &mut body_out);
        self.set_block(hbody, &body_out);
    }

    fn emit_defer(&mut self, e: &Expr, sink: Sink, out: &mut String) {
        let Expr::Defer { cleanup, body } = e else {
            unreachable!()
        };
        let hbody = self.reserve_block();
        let cl = self.atom(cleanup);
        let (cont, slot) = match sink {
            Sink::Ret => ("NULL".to_string(), 0),
            Sink::Slot { slot, cont } => {
                let cont = cont.expect("a non-tail Defer needs a continuation block");
                (format!("blk_{cont}"), slot)
            }
        };
        out.push_str(&format!(
            "  THxK_defer_run(fr, {cont}, {slot}, {cl}, blk_{hbody});\n"
        ));
        let mut body_out = String::new();
        self.emit_expr(body, Sink::Ret, &mut body_out);
        self.set_block(hbody, &body_out);
    }

    /// Emit one code as its entry block (plus any continuation blocks it spawns).
    fn emit_code(&mut self, id: usize) {
        let entry = self.reserve_block();
        self.code_entry[id] = entry;
        // An unreachable code (a pruned global's CAF, or an `@extern` binding the
        // program never calls) becomes a trap stub: its body is never emitted, so
        // it references no foreign wrapper that was not collected.
        if !self.reachable.contains(&id) {
            self.set_block(entry, "  thrax_fault(\"unreachable code\");\n");
            return;
        }
        let prog = self.prog;
        let mut out = String::new();
        self.emit_expr(&prog.codes[id].body, Sink::Ret, &mut out);
        self.set_block(entry, &out);
    }
}
