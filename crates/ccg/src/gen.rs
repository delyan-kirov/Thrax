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
    /// One entry per emitted block: its full C function text, filled out of order
    /// (a block is reserved, then set once its body is built).
    blocks: Vec<String>,
    /// Code index -> its entry block id.
    code_entry: Vec<usize>,
    tmp: usize,
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

/// The arity of a built-in operator, or `None`. Mirrors the runtime's table and
/// the interpreter's `builtin_arity`.
fn builtin_arity(name: &str) -> Option<usize> {
    let n = match name {
        "not" | "neg" | "array_len" | "array_alloc" | "vec_len" | "vec_new" => 1,
        "+" | "-" | "*" | "/" | "%" | "?=" | "?<" | "?>" | "<=" | ">=" | "++" | "array_get"
        | "array_push" | "vec_get" | "vec_push" | "vec_fill" => 2,
        "array_set" | "array_slice" | "vec_set" => 3,
        _ => return None,
    };
    Some(n)
}

/// The arity of a supported `C.<fn>`, or `None`.
fn c_arity(name: &str) -> Option<usize> {
    Some(match name {
        "getenv" | "fclose" | "fgetc" | "ftell" | "remove" | "getchar" | "time" => 1,
        "fopen" | "fputs" => 2,
        "fseek" | "write" => 3,
        _ => return None,
    })
}

impl<'p> Emitter<'p> {
    pub fn new(prog: &'p Program) -> Emitter<'p> {
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
        Emitter {
            prog,
            globals,
            bare,
            ops_by_effect,
            ops_by_name,
            blocks: Vec::new(),
            code_entry: vec![0; prog.codes.len()],
            tmp: 0,
        }
    }

    /// Emit every code, then return the finished block function texts and the
    /// code index -> entry block map.
    pub fn run(mut self) -> (Vec<String>, Vec<usize>) {
        for id in 0..self.prog.codes.len() {
            self.emit_code(id);
        }
        (self.blocks, self.code_entry)
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
    /// mirroring the machine's `glob` order: a global CAF (canonical or bare), a
    /// built-in, a `C.`/`TARGET.` member, then an effect operation.
    fn glob_atom(&self, name: &str) -> String {
        if self.globals.contains(name) || self.bare.contains(name) {
            return format!("THxRT_glob({})", cstr(name.as_bytes()));
        }
        if let Some(arity) = builtin_arity(name) {
            return format!("THxRT_builtin({}, {arity})", cstr(name.as_bytes()));
        }
        if let Some(suffix) = name.strip_prefix("C.") {
            if let Some(arity) = c_arity(suffix) {
                return format!("THxRT_builtin({}, {arity})", cstr(name.as_bytes()));
            }
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
        let eff = match self.ops_by_name.get(name) {
            Some(effs) if effs.len() == 1 => cstr(effs[0].as_bytes()),
            _ => "NULL".to_string(),
        };
        format!("THxK_op({eff}, {})", cstr(name.as_bytes()))
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
        let prog = self.prog;
        let mut out = String::new();
        self.emit_expr(&prog.codes[id].body, Sink::Ret, &mut out);
        self.set_block(entry, &out);
    }
}
