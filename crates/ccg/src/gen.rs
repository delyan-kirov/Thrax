//! The Core-to-C emitter. Each Core [`Term`] compiles to C statements that build
//! a runtime `Value*` (see `runtime.c`). Lambdas become top-level C functions
//! closing over a linked environment; globals become lazily forced, memoized
//! functions. The scheme is direct-style: evaluation runs on the C stack.
//!
//! Algebraic effects (`Handle`, `Defer`, effect operations) cannot capture a
//! resumable continuation in direct-style C, so they compile to a runtime fault,
//! exactly as the interpreter faults on the FFI `Fault` node. The effect-free
//! core (the bulk of the language) compiles and runs.

use frontend::lowering::data::{Pat, Term};

/// Accumulates the C functions a program lowers to. `decls` are forward
/// declarations (so functions may call each other in any order); `defs` are the
/// bodies. `n` mints fresh C identifiers.
pub struct Emitter {
    pub decls: Vec<String>,
    pub defs: Vec<String>,
    n: usize,
}

impl Emitter {
    pub fn new() -> Emitter {
        Emitter {
            decls: Vec::new(),
            defs: Vec::new(),
            n: 0,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.n += 1;
        format!("{prefix}{}", self.n)
    }

    /// Emit a global's memoized forcer `thrax_g{idx}`, returning its definition
    /// text. `key` is `Module.name`, used in the self-reference fault message.
    pub fn emit_global(&mut self, idx: usize, key: &str, term: &Term) {
        let mut body = String::new();
        let res = self.emit(term, "NULL", &mut body);
        let def = format!(
            "static Value *thrax_g{idx}(void) {{\n\
             static Value *cache;\n\
             static int state;\n\
             if (state == 2) return cache;\n\
             if (state == 1) thrax_fault({selfref});\n\
             state = 1;\n\
             {body}\
             cache = {res};\n\
             state = 2;\n\
             return cache;\n\
             }}\n",
            selfref = c_string(&format!("`{key}` refers to itself while being defined")),
        );
        self.decls
            .push(format!("static Value *thrax_g{idx}(void);"));
        self.defs.push(def);
    }

    /// Emit statements computing `term` into `body`, returning a C expression
    /// (a temp name or a simple literal) of type `Value*`. `env` is a C
    /// expression of type `Env*` naming the current environment.
    fn emit(&mut self, term: &Term, env: &str, body: &mut String) -> String {
        match term {
            Term::Int(n) => {
                if *n == i64::MIN {
                    "mk_int(INT64_MIN)".into()
                } else {
                    format!("mk_int({n}LL)")
                }
            }
            Term::Real(r) => format!("mk_real({})", c_double(*r)),
            Term::Bool(b) => format!("mk_bool({b})"),
            Term::Unit => "mk_unit()".into(),
            Term::Str(bytes) => format!("mk_str({}, {})", c_bytes(bytes), bytes.len()),

            Term::Var { module, name } => {
                let m = match module {
                    Some(m) => c_string(m),
                    None => "NULL".into(),
                };
                let t = self.fresh("t");
                body.push_str(&format!(
                    "Value *{t} = resolve_var({env}, {m}, {});\n",
                    c_string(name)
                ));
                t
            }

            Term::App(f, x) => {
                let fe = self.emit(f, env, body);
                let ft = self.hold(body, &fe);
                let xe = self.emit(x, env, body);
                let t = self.fresh("t");
                body.push_str(&format!("Value *{t} = apply({ft}, {xe});\n"));
                t
            }

            Term::Lam { param, body: b } => {
                let fname = self.emit_lambda(param, b);
                format!("mk_closure({fname}, {env})")
            }

            Term::Let {
                name,
                rec,
                val,
                body: b,
            } => {
                let e = self.fresh("e");
                if *rec {
                    body.push_str(&format!(
                        "Env *{e} = env_extend({env}, {}, mk_unit());\n",
                        c_string(name)
                    ));
                    let v = self.emit(val, &e, body);
                    body.push_str(&format!("{e}->val = {v};\n"));
                } else {
                    let v = self.emit(val, env, body);
                    body.push_str(&format!(
                        "Env *{e} = env_extend({env}, {}, {v});\n",
                        c_string(name)
                    ));
                }
                self.emit(b, &e, body)
            }

            Term::Case {
                scrut,
                arms,
                default,
            } => {
                let se = self.emit(scrut, env, body);
                let scr = self.hold(body, &se);
                let res = self.fresh("r");
                body.push_str(&format!("Value *{res} = NULL;\n"));
                let done = self.fresh("Ldone");
                let labels: Vec<String> = arms.iter().map(|_| self.fresh("Larm")).collect();
                let ldefault = self.fresh("Ldefault");
                for (i, arm) in arms.iter().enumerate() {
                    let fail = if i + 1 < arms.len() {
                        labels[i + 1].clone()
                    } else {
                        ldefault.clone()
                    };
                    body.push_str(&format!("{}: ;\n{{\n", labels[i]));
                    let e = self.fresh("e");
                    body.push_str(&format!("Env *{e} = {env};\n"));
                    self.emit_pat(&arm.pat, &scr, &e, &fail, body);
                    if let Some(guard) = &arm.guard {
                        let g = self.emit(guard, &e, body);
                        let gt = self.hold(body, &g);
                        body.push_str(&format!(
                            "if (!({gt}->tag == T_BOOL && {gt}->b)) goto {fail};\n"
                        ));
                    }
                    let bexpr = self.emit(&arm.body, &e, body);
                    body.push_str(&format!("{res} = {bexpr};\ngoto {done};\n}}\n"));
                }
                body.push_str(&format!("{ldefault}: ;\n"));
                match default {
                    Some(d) => {
                        let dv = self.emit(d, env, body);
                        body.push_str(&format!("{res} = {dv};\n"));
                    }
                    None => body.push_str(&format!(
                        "{res} = thrax_fault({});\n",
                        c_string("no pattern matched (non-exhaustive `when`)")
                    )),
                }
                body.push_str(&format!("{done}: ;\n"));
                res
            }

            Term::Tuple(items) => {
                let arr = self.emit_array(items, env, body);
                format!("mk_tuple({arr}, {})", items.len())
            }

            Term::Variant { ty, tag, fields } => {
                let arr = self.emit_array(fields, env, body);
                format!(
                    "mk_variant({}, {}, {arr}, {})",
                    c_string(ty),
                    c_string(tag),
                    fields.len()
                )
            }

            Term::Struct { name, base, fields } => {
                let n = fields.len();
                let farr = if n == 0 {
                    "NULL".to_string()
                } else {
                    let arr = self.fresh("f");
                    body.push_str(&format!("Field *{arr} = xmalloc({n} * sizeof(Field));\n"));
                    for (i, (fname, fexpr)) in fields.iter().enumerate() {
                        let v = self.emit(fexpr, env, body);
                        body.push_str(&format!(
                            "{arr}[{i}].name = {}; {arr}[{i}].val = {v};\n",
                            c_string(fname)
                        ));
                    }
                    arr
                };
                match base {
                    Some(b) => {
                        let be = self.emit(b, env, body);
                        let bt = self.hold(body, &be);
                        format!("mk_struct_update({bt}, {}, {farr}, {n})", c_string(name))
                    }
                    None => format!("mk_struct({}, {farr}, {n})", c_string(name)),
                }
            }

            Term::Field(record, name) => {
                let r = self.emit(record, env, body);
                let rt = self.hold(body, &r);
                let t = self.fresh("t");
                body.push_str(&format!(
                    "Value *{t} = thrax_field({rt}, {});\n",
                    c_string(name)
                ));
                t
            }

            Term::With { subject, body: b } => {
                let s = self.emit(subject, env, body);
                let st = self.hold(body, &s);
                body.push_str(&format!(
                    "if ({st}->tag != T_STRUCT) thrax_fault({});\n",
                    c_string("`with` on a non-struct value")
                ));
                let e = self.fresh("e");
                let i = self.fresh("i");
                body.push_str(&format!("Env *{e} = {env};\n"));
                body.push_str(&format!(
                    "for (size_t {i} = 0; {i} < {st}->strct.len; {i}++) \
                     {e} = env_extend({e}, {st}->strct.fields[{i}].name, \
                     {st}->strct.fields[{i}].val);\n"
                ));
                self.emit(b, &e, body)
            }

            // Effects and FFI cannot be expressed in direct-style C; fault when
            // forced, mirroring the interpreter's treatment of `Term::Fault`.
            Term::Handle { .. } => self.fault(body, "ccg: algebraic effects are not supported"),
            Term::Defer { .. } => self.fault(body, "ccg: `defer` is not supported"),
            Term::Fault(what) => self.fault(body, &format!("unsupported at runtime: {what}")),
        }
    }

    /// Bind a just-computed expression to a fresh temp so it can be used more
    /// than once without re-evaluating (which would re-force a global or
    /// re-allocate a literal).
    fn hold(&mut self, body: &mut String, expr: &str) -> String {
        let t = self.fresh("t");
        body.push_str(&format!("Value *{t} = {expr};\n"));
        t
    }

    fn fault(&mut self, body: &mut String, msg: &str) -> String {
        let t = self.fresh("t");
        body.push_str(&format!("Value *{t} = thrax_fault({});\n", c_string(msg)));
        t
    }

    /// Evaluate `terms` left to right into a fresh `Value**` array, returning its
    /// C name (or `NULL` for an empty array).
    fn emit_array(&mut self, terms: &[Term], env: &str, body: &mut String) -> String {
        if terms.is_empty() {
            return "NULL".into();
        }
        let arr = self.fresh("a");
        body.push_str(&format!(
            "Value **{arr} = xmalloc({} * sizeof(Value *));\n",
            terms.len()
        ));
        for (i, t) in terms.iter().enumerate() {
            let v = self.emit(t, env, body);
            body.push_str(&format!("{arr}[{i}] = {v};\n"));
        }
        arr
    }

    fn emit_lambda(&mut self, param: &str, term: &Term) -> String {
        let fname = {
            self.n += 1;
            format!("thrax_lam{}", self.n)
        };
        let mut fbody = String::new();
        let e = self.fresh("e");
        fbody.push_str(&format!(
            "Env *{e} = env_extend(env, {}, arg);\n",
            c_string(param)
        ));
        let res = self.emit(term, &e, &mut fbody);
        fbody.push_str(&format!("return {res};\n"));
        self.decls
            .push(format!("static Value *{fname}(Env *, Value *);"));
        self.defs.push(format!(
            "static Value *{fname}(Env *env, Value *arg) {{\n{fbody}}}\n"
        ));
        fname
    }

    /// Emit a pattern test against the C expression `val`. On a mismatch, jump to
    /// `fail`; on a match, extend `env_var` (a mutable `Env*` C variable) with any
    /// bindings.
    fn emit_pat(&mut self, pat: &Pat, val: &str, env_var: &str, fail: &str, body: &mut String) {
        match pat {
            Pat::Wild => {}
            Pat::Var(name) => body.push_str(&format!(
                "{env_var} = env_extend({env_var}, {}, {val});\n",
                c_string(name)
            )),
            Pat::Int(n) => {
                let lit = if *n == i64::MIN {
                    "INT64_MIN".to_string()
                } else {
                    format!("{n}LL")
                };
                body.push_str(&format!(
                    "if (!({val}->tag == T_INT && {val}->i == {lit})) goto {fail};\n"
                ));
            }
            Pat::Real(r) => body.push_str(&format!(
                "if (!({val}->tag == T_REAL && {val}->r == {})) goto {fail};\n",
                c_double(*r)
            )),
            Pat::Bool(b) => body.push_str(&format!(
                "if (!({val}->tag == T_BOOL && {val}->b == {b})) goto {fail};\n"
            )),
            Pat::Str(s) => {
                if s.is_empty() {
                    body.push_str(&format!(
                        "if (!({val}->tag == T_STR && {val}->str.len == 0)) goto {fail};\n"
                    ));
                } else {
                    body.push_str(&format!(
                        "if (!({val}->tag == T_STR && {val}->str.len == {len} && \
                         memcmp({val}->str.data, {lit}, {len}) == 0)) goto {fail};\n",
                        len = s.len(),
                        lit = c_bytes(s),
                    ));
                }
            }
            Pat::Tuple(pats) => {
                body.push_str(&format!(
                    "if (!({val}->tag == T_TUPLE && {val}->seq.len == {})) goto {fail};\n",
                    pats.len()
                ));
                for (i, p) in pats.iter().enumerate() {
                    let sub = self.fresh("t");
                    body.push_str(&format!("Value *{sub} = {val}->seq.items[{i}];\n"));
                    self.emit_pat(p, &sub, env_var, fail, body);
                }
            }
            Pat::Variant { tag, fields } => {
                body.push_str(&format!(
                    "if (!({val}->tag == T_VARIANT && strcmp({val}->variant.tag, {tag}) == 0 \
                     && {val}->variant.len == {n})) goto {fail};\n",
                    tag = c_string(tag),
                    n = fields.len(),
                ));
                for (i, p) in fields.iter().enumerate() {
                    let sub = self.fresh("t");
                    body.push_str(&format!("Value *{sub} = {val}->variant.fields[{i}];\n"));
                    self.emit_pat(p, &sub, env_var, fail, body);
                }
            }
            Pat::Struct { fields } => {
                body.push_str(&format!("if ({val}->tag != T_STRUCT) goto {fail};\n"));
                for (fname, p) in fields {
                    let sub = self.fresh("t");
                    body.push_str(&format!(
                        "Value *{sub} = struct_field({val}, {});\n",
                        c_string(fname)
                    ));
                    body.push_str(&format!("if (!{sub}) goto {fail};\n"));
                    self.emit_pat(p, &sub, env_var, fail, body);
                }
            }
            Pat::StrPrefix { prefix, rest } => {
                let plen = prefix.len();
                body.push_str(&format!(
                    "if (!({val}->tag == T_STR && {val}->str.len >= {plen} && \
                     memcmp({val}->str.data, {lit}, {plen}) == 0)) goto {fail};\n",
                    lit = c_bytes(prefix),
                ));
                let tail = self.fresh("t");
                body.push_str(&format!(
                    "Value *{tail} = mk_str({val}->str.data + {plen}, {val}->str.len - {plen});\n"
                ));
                self.emit_pat(rest, &tail, env_var, fail, body);
            }
        }
    }
}

/// A C string literal for `s` (ASCII identifiers, tags, operator names).
pub fn c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for b in s.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

/// A C `const uint8_t*` for a byte string: a compound-literal array, or `""` for
/// the empty string (an empty compound literal is not valid C).
fn c_bytes(b: &[u8]) -> String {
    if b.is_empty() {
        return "(const uint8_t *)\"\"".into();
    }
    let items: Vec<String> = b.iter().map(|x| x.to_string()).collect();
    format!("(const uint8_t[]){{{}}}", items.join(", "))
}

/// A C double literal that round-trips `r`. Non-finite values map to the C99
/// `<math.h>`-free spellings the runtime accepts.
fn c_double(r: f64) -> String {
    if r.is_nan() {
        "(0.0 / 0.0)".into()
    } else if r.is_infinite() {
        if r < 0.0 {
            "(-1.0 / 0.0)".into()
        } else {
            "(1.0 / 0.0)".into()
        }
    } else {
        // Rust's Debug formatting of f64 is a round-tripping decimal that is also
        // a valid C double literal (always has a `.` or exponent).
        format!("{r:?}")
    }
}
