//! A tree-walking interpreter over the [`crate::term`] Core.
//!
//! Evaluation is strict and environment-passing. A definition is a lazily forced
//! global (so a module runs only the globals it needs, and forward references
//! resolve); locals live on a linked environment of scopes. Recursion uses a
//! mutable slot: a recursive `let` binds a placeholder, evaluates the value in an
//! environment that already contains the binding, then fills the slot.
//!
//! The Core program is immutable and shared through `Arc`, so the interpreter
//! stashes subterms (a closure body, a thunk, the arms of a `Case`) by pointer
//! bump and carries no lifetime. Everything the interpreter creates while running
//! (`Value`, `Closure`, `Scope`) is per-thread mutable state shared through `Rc`.
//! The suspension/resumption machinery for algebraic effects is continuation
//! passing (`bind`), and each continuation owns a clone of the `Interp` handle
//! rather than borrowing it.
//!
//! Built-in operators are dispatched dynamically on the runtime value kinds
//! (`+` on two `Int`s versus two `Real`s, `array_*` on the byte-vector value).
//! The program already type-checked, so the operands always have a kind the
//! implementation accepts; the interpreter needs no resolved overload keys.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use diag::{Code, Diagnostic, Result, Span};

use crate::term::{Arm, Handler, Pat, Program, Term};

/// A runtime value. Aggregates own their elements; closures and the environment
/// share structure through `Rc`.
#[derive(Clone)]
pub enum Value {
    Int(i64),
    Real(f64),
    /// Byte string, also the representation of an `Array` (both are byte vectors).
    Str(Rc<Vec<u8>>),
    Bool(bool),
    Unit,
    Tuple(Vec<Value>),
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    Variant {
        ty: String,
        tag: String,
        fields: Vec<Value>,
    },
    /// A generic growable vector (`Vec `T`), distinct from the byte-vector `Str`.
    Vector(Rc<Vec<Value>>),
    Closure(Rc<Closure>),
    /// A partially applied built-in: it runs once `args` reaches `arity`.
    Builtin {
        name: Rc<str>,
        arity: usize,
        args: Vec<Value>,
    },
    /// An algebraic effect operation. Applying it performs the operation.
    Operation {
        effect: Option<String>,
        op: String,
    },
    /// A captured affine continuation. Applying it resumes the suspended
    /// computation once.
    Resumption(Continuation),
}

/// An opaque, one-shot continuation captured by an effect handler. Its interior
/// is the interpreter's private machinery; a value only ever gets one by
/// performing an operation, and consumes it by applying it.
#[derive(Clone)]
pub struct Continuation(Rc<RefCell<Option<Resume>>>);

type Eval = Result<Outcome>;
type Resume = Rc<dyn Fn(Value) -> Eval>;

enum Outcome {
    Value(Value),
    Perform {
        effect: Option<String>,
        op: String,
        arg: Value,
        resume: Resume,
    },
}

pub struct Closure {
    param: String,
    body: Arc<Term>,
    env: Env,
}

/// A lexical environment: a linked list of single-binding scopes. A slot is
/// shared and mutable so a recursive binding can be filled after the closure
/// that captures it is built.
type Env = Option<Rc<Scope>>;

struct Scope {
    name: String,
    slot: Rc<RefCell<Value>>,
    parent: Env,
}

fn extend(env: &Env, name: String, value: Value) -> Env {
    Some(Rc::new(Scope {
        name,
        slot: Rc::new(RefCell::new(value)),
        parent: env.clone(),
    }))
}

fn lookup(env: &Env, name: &str) -> Option<Rc<RefCell<Value>>> {
    let mut cur = env;
    while let Some(scope) = cur {
        if scope.name == name {
            return Some(scope.slot.clone());
        }
        cur = &scope.parent;
    }
    None
}

/// A global's evaluation state: unforced code, in-progress (to catch a value-level
/// self-reference), or its forced value.
enum GCell {
    Thunk(Arc<Term>),
    Forcing,
    Forced(Value),
}

#[derive(Clone)]
pub struct Interp {
    rt: Rc<Runtime>,
}

struct Runtime {
    /// Globals keyed canonically by `Module.name`.
    globals: HashMap<String, RefCell<GCell>>,
    /// Bare `name` to its canonical key, so an unqualified reference resolves.
    bare: HashMap<String, String>,
    /// Effect-qualified operations, keyed by effect name.
    ops_by_effect: HashMap<String, Vec<String>>,
    /// Operation names to the effects that declare them.
    ops_by_name: HashMap<String, Vec<String>>,
}

impl Interp {
    /// Build an interpreter over already-lowered modules. The first module is the
    /// root: its names win when an unqualified name is defined in several modules.
    /// Each global's term is cloned into an [`Arc`], so the interpreter owns its
    /// program and does not borrow the passed slice.
    pub fn new(modules: &[Program]) -> Interp {
        let mut globals = HashMap::new();
        let mut bare: HashMap<String, String> = HashMap::new();
        let mut ops_by_effect: HashMap<String, Vec<String>> = HashMap::new();
        let mut ops_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for program in modules {
            for effect in &program.effects {
                ops_by_effect
                    .entry(effect.effect.clone())
                    .or_default()
                    .push(effect.op.clone());
                ops_by_name
                    .entry(effect.op.clone())
                    .or_default()
                    .push(effect.effect.clone());
            }
            for (name, term) in &program.globals {
                let key = format!("{}.{}", program.module, name);
                globals.insert(
                    key.clone(),
                    RefCell::new(GCell::Thunk(Arc::new(term.clone()))),
                );
                bare.entry(name.clone()).or_insert(key);
            }
        }
        for effects in ops_by_name.values_mut() {
            effects.sort();
            effects.dedup();
        }
        Interp {
            rt: Rc::new(Runtime {
                globals,
                bare,
                ops_by_effect,
                ops_by_name,
            }),
        }
    }

    /// Force and return the value of a global, qualified `Module.name` or bare.
    pub fn eval_global(&self, name: &str) -> Result<Value> {
        let key = if self.rt.globals.contains_key(name) {
            name.to_string()
        } else {
            self.rt
                .bare
                .get(name)
                .cloned()
                .ok_or_else(|| fault(format!("no global `{name}`")))?
        };
        self.force(&key)
    }

    fn force(&self, key: &str) -> Result<Value> {
        let cell = self
            .rt
            .globals
            .get(key)
            .ok_or_else(|| fault(format!("no global `{key}`")))?;
        match &*cell.borrow() {
            GCell::Forced(v) => return Ok(v.clone()),
            GCell::Forcing => {
                return Err(fault(format!(
                    "`{key}` refers to itself while being defined"
                )))
            }
            GCell::Thunk(_) => {}
        }
        let term = match std::mem::replace(&mut *cell.borrow_mut(), GCell::Forcing) {
            GCell::Thunk(t) => t,
            _ => unreachable!("state checked as Thunk just above"),
        };
        let evaluated = self.eval(&term, &None);
        let value = match evaluated {
            Ok(Outcome::Value(v)) => v,
            Ok(Outcome::Perform { effect, op, .. }) => {
                *cell.borrow_mut() = GCell::Thunk(term);
                return Err(fault(format!(
                    "unhandled effect operation `{}`",
                    format_op(effect.as_deref(), &op)
                )));
            }
            Err(e) => {
                *cell.borrow_mut() = GCell::Thunk(term);
                return Err(e);
            }
        };
        *cell.borrow_mut() = GCell::Forced(value.clone());
        Ok(value)
    }

    fn eval(&self, term: &Term, env: &Env) -> Eval {
        match term {
            Term::Int(n) => Ok(Outcome::Value(Value::Int(*n))),
            Term::Real(r) => Ok(Outcome::Value(Value::Real(*r))),
            Term::Str(s) => Ok(Outcome::Value(Value::Str(Rc::new(s.clone())))),
            Term::Bool(b) => Ok(Outcome::Value(Value::Bool(*b))),
            Term::Unit => Ok(Outcome::Value(Value::Unit)),

            Term::Var { module, name } => self
                .resolve_var(module.as_deref(), name, env)
                .map(Outcome::Value),

            Term::App(f, x) => {
                let func = self.eval(f, env)?;
                let x = x.clone();
                let env_for_x = env.clone();
                let this = self.clone();
                self.bind(
                    func,
                    Rc::new(move |func| {
                        let arg = this.eval(&x, &env_for_x)?;
                        let inner = this.clone();
                        let func = func.clone();
                        this.bind(arg, Rc::new(move |arg| inner.apply(func.clone(), arg)))
                    }),
                )
            }

            Term::Lam { param, body } => Ok(Outcome::Value(Value::Closure(Rc::new(Closure {
                param: param.clone(),
                body: body.clone(),
                env: env.clone(),
            })))),

            Term::Let {
                name,
                rec,
                val,
                body,
            } => {
                if *rec {
                    let scope = extend(env, name.clone(), Value::Unit);
                    let out = self.eval(val, &scope)?;
                    let this = self.clone();
                    let body = body.clone();
                    self.bind(
                        out,
                        Rc::new(move |v| {
                            if let Some(s) = &scope {
                                *s.slot.borrow_mut() = v;
                            }
                            this.eval(&body, &scope)
                        }),
                    )
                } else {
                    let out = self.eval(val, env)?;
                    let this = self.clone();
                    let env = env.clone();
                    let name = name.clone();
                    let body = body.clone();
                    self.bind(
                        out,
                        Rc::new(move |v| {
                            let scope = extend(&env, name.clone(), v);
                            this.eval(&body, &scope)
                        }),
                    )
                }
            }

            Term::Case {
                scrut,
                arms,
                default,
            } => {
                let value = self.eval(scrut, env)?;
                let this = self.clone();
                let arms = arms.clone();
                let default = default.clone();
                let env = env.clone();
                self.bind(
                    value,
                    Rc::new(move |value| {
                        this.eval_case_from(value, arms.clone(), default.clone(), env.clone(), 0)
                    }),
                )
            }

            Term::Tuple(items) => self.eval_terms(items.clone(), 0, env.clone(), Vec::new()),

            Term::Struct { name, base, fields } => {
                self.eval_struct(name, base.clone(), fields.clone(), env)
            }

            Term::Variant { ty, tag, fields } => {
                let values = self.eval_terms(fields.clone(), 0, env.clone(), Vec::new())?;
                let ty = ty.clone();
                let tag = tag.clone();
                self.bind(
                    values,
                    Rc::new(move |values| {
                        let Value::Tuple(fields) = values else {
                            unreachable!("eval_terms returns a tuple")
                        };
                        Ok(Outcome::Value(Value::Variant {
                            ty: ty.clone(),
                            tag: tag.clone(),
                            fields,
                        }))
                    }),
                )
            }

            Term::Field(record, name) => {
                let rec = self.eval(record, env)?;
                let this = self.clone();
                let name = name.clone();
                self.bind(
                    rec,
                    Rc::new(move |rec| this.field(&rec, &name).map(Outcome::Value)),
                )
            }

            Term::With { subject, body } => {
                let subj = self.eval(subject, env)?;
                let this = self.clone();
                let env = env.clone();
                let body = body.clone();
                self.bind(
                    subj,
                    Rc::new(move |subj| match subj {
                        Value::Struct { fields, .. } => {
                            let mut scope = env.clone();
                            for (fname, fval) in fields {
                                scope = extend(&scope, fname, fval);
                            }
                            this.eval(&body, &scope)
                        }
                        _ => Err(fault("`with` on a non-struct value")),
                    }),
                )
            }

            Term::Handle { body, handler } => {
                let out = self.eval(body, env)?;
                self.handle_outcome(out, handler.clone(), env.clone())
            }

            Term::Fault(what) => Err(fault(format!("unsupported at runtime: {what}"))),
        }
    }

    fn bind(&self, out: Outcome, cont: Resume) -> Eval {
        match out {
            Outcome::Value(v) => cont(v),
            Outcome::Perform {
                effect,
                op,
                arg,
                resume,
            } => {
                let this = self.clone();
                Ok(Outcome::Perform {
                    effect,
                    op,
                    arg,
                    resume: Rc::new(move |v| {
                        let next = resume(v)?;
                        this.bind(next, cont.clone())
                    }),
                })
            }
        }
    }

    fn resolve_var(&self, module: Option<&str>, name: &str, env: &Env) -> Result<Value> {
        if module.is_none() {
            if let Some(slot) = lookup(env, name) {
                return Ok(slot.borrow().clone());
            }
        }
        // Globals: a qualified name tries `Module.name`, then the bare name.
        let qualified = module.map(|m| format!("{m}.{name}"));
        if let Some(key) = qualified.filter(|k| self.rt.globals.contains_key(k)) {
            return self.force(&key);
        }
        if let Some(key) = self.rt.bare.get(name) {
            return self.force(key);
        }
        if let Some(arity) = builtin_arity(name) {
            return Ok(Value::Builtin {
                name: name.into(),
                arity,
                args: Vec::new(),
            });
        }
        if let Some(module) = module {
            if self
                .rt
                .ops_by_effect
                .get(module)
                .is_some_and(|ops| ops.iter().any(|op| op == name))
            {
                return Ok(Value::Operation {
                    effect: Some(module.to_string()),
                    op: name.to_string(),
                });
            }
        } else if let Some(effects) = self.rt.ops_by_name.get(name) {
            let effect = if effects.len() == 1 {
                Some(effects[0].clone())
            } else {
                None
            };
            return Ok(Value::Operation {
                effect,
                op: name.to_string(),
            });
        }
        Err(fault(format!("unbound name `{name}`")))
    }

    fn apply(&self, func: Value, arg: Value) -> Eval {
        match func {
            Value::Closure(c) => {
                let scope = extend(&c.env, c.param.clone(), arg);
                self.eval(&c.body, &scope)
            }
            Value::Builtin {
                name,
                arity,
                mut args,
            } => {
                args.push(arg);
                if args.len() == arity {
                    run_builtin(&name, args).map(Outcome::Value)
                } else {
                    Ok(Outcome::Value(Value::Builtin { name, arity, args }))
                }
            }
            Value::Operation { effect, op } => Ok(Outcome::Perform {
                effect,
                op,
                arg,
                resume: Rc::new(|v| Ok(Outcome::Value(v))),
            }),
            Value::Resumption(k) => {
                let resume =
                    k.0.borrow_mut()
                        .take()
                        .ok_or_else(|| fault("continuation already resumed"))?;
                resume(arg)
            }
            _ => Err(fault("applied a non-function value")),
        }
    }

    fn eval_terms(&self, terms: Arc<[Term]>, start: usize, env: Env, acc: Vec<Value>) -> Eval {
        if start >= terms.len() {
            return Ok(Outcome::Value(Value::Tuple(acc)));
        }
        let out = self.eval(&terms[start], &env)?;
        let this = self.clone();
        self.bind(
            out,
            Rc::new(move |v| {
                let mut next = acc.clone();
                next.push(v);
                this.eval_terms(terms.clone(), start + 1, env.clone(), next)
            }),
        )
    }

    fn eval_case_from(
        &self,
        value: Value,
        arms: Arc<[Arm]>,
        default: Option<Arc<Term>>,
        env: Env,
        start: usize,
    ) -> Eval {
        for (offset, arm) in arms[start..].iter().enumerate() {
            let mut binds = Vec::new();
            if match_pat(&arm.pat, &value, &mut binds) {
                let mut scope = env.clone();
                for (n, v) in binds {
                    scope = extend(&scope, n, v);
                }
                if let Some(guard) = &arm.guard {
                    let guard_value = self.eval(guard, &scope)?;
                    let next = start + offset + 1;
                    let this = self.clone();
                    let body = arm.body.clone();
                    let body_scope = scope.clone();
                    let arms = arms.clone();
                    let default = default.clone();
                    let env = env.clone();
                    let value = value.clone();
                    return self.bind(
                        guard_value,
                        Rc::new(move |guard_value| match guard_value {
                            Value::Bool(true) => this.eval(&body, &body_scope),
                            _ => this.eval_case_from(
                                value.clone(),
                                arms.clone(),
                                default.clone(),
                                env.clone(),
                                next,
                            ),
                        }),
                    );
                }
                return self.eval(&arm.body, &scope);
            }
        }
        match default {
            Some(d) => self.eval(&d, &env),
            None => Err(fault("no pattern matched (non-exhaustive `when`)")),
        }
    }

    fn eval_struct(
        &self,
        name: &str,
        base: Option<Arc<Term>>,
        fields: Arc<[(String, Term)]>,
        env: &Env,
    ) -> Eval {
        match base {
            Some(b) => {
                let base_value = self.eval(&b, env)?;
                let this = self.clone();
                let env = env.clone();
                let name = name.to_string();
                self.bind(
                    base_value,
                    Rc::new(move |base_value| match base_value {
                        Value::Struct {
                            name: base_name,
                            fields: base_fields,
                        } => this.eval_struct_fields(
                            name.clone(),
                            fields.clone(),
                            0,
                            env.clone(),
                            base_fields,
                            base_name,
                        ),
                        _ => Err(fault("record update of a non-struct value")),
                    }),
                )
            }
            None => self.eval_struct_fields(
                name.to_string(),
                fields,
                0,
                env.clone(),
                Vec::new(),
                String::new(),
            ),
        }
    }

    fn eval_struct_fields(
        &self,
        name: String,
        fields: Arc<[(String, Term)]>,
        start: usize,
        env: Env,
        acc: Vec<(String, Value)>,
        base_name: String,
    ) -> Eval {
        if start >= fields.len() {
            return Ok(Outcome::Value(Value::Struct {
                name: if name.is_empty() { base_name } else { name },
                fields: acc,
            }));
        }
        let (fname, fexpr) = &fields[start];
        let field_value = self.eval(fexpr, &env)?;
        let this = self.clone();
        let fname = fname.clone();
        self.bind(
            field_value,
            Rc::new(move |fv| {
                let mut next = acc.clone();
                match next.iter_mut().find(|(n, _)| *n == fname) {
                    Some((_, slot)) => *slot = fv,
                    None => next.push((fname.clone(), fv)),
                }
                this.eval_struct_fields(
                    name.clone(),
                    fields.clone(),
                    start + 1,
                    env.clone(),
                    next,
                    base_name.clone(),
                )
            }),
        )
    }

    fn field(&self, record: &Value, name: &str) -> Result<Value> {
        match record {
            Value::Struct { fields, .. } => fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| fault(format!("no field `{name}`"))),
            Value::Tuple(items) => name
                .parse::<usize>()
                .ok()
                .and_then(|i| items.get(i).cloned())
                .ok_or_else(|| fault(format!("no tuple index `{name}`"))),
            _ => Err(fault(format!("field access `.{name}` on a non-record"))),
        }
    }

    fn handle_outcome(&self, outcome: Outcome, handler: Arc<Handler>, env: Env) -> Eval {
        match outcome {
            Outcome::Value(v) => match &handler.default {
                Some((name, body)) => {
                    let scope = extend(&env, name.clone(), v);
                    self.eval(body, &scope)
                }
                None => Ok(Outcome::Value(v)),
            },
            Outcome::Perform {
                effect,
                op,
                arg,
                resume,
            } => match self.match_clause(&handler, effect.as_deref(), &op)? {
                Some(idx) => {
                    let deep_resume: Resume = Rc::new({
                        let this = self.clone();
                        let handler = handler.clone();
                        let env = env.clone();
                        move |v| {
                            let next = resume(v)?;
                            this.handle_outcome(next, handler.clone(), env.clone())
                        }
                    });
                    let k =
                        Value::Resumption(Continuation(Rc::new(RefCell::new(Some(deep_resume)))));
                    let clause = &handler.clauses[idx];
                    let scope = extend(&env, clause.arg.clone(), arg);
                    let scope = extend(&scope, handler.continuation.clone(), k);
                    self.eval(&clause.body, &scope)
                }
                None => {
                    let this = self.clone();
                    let handler = handler.clone();
                    Ok(Outcome::Perform {
                        effect,
                        op,
                        arg,
                        resume: Rc::new(move |v| {
                            let next = resume(v)?;
                            this.handle_outcome(next, handler.clone(), env.clone())
                        }),
                    })
                }
            },
        }
    }

    fn match_clause(
        &self,
        handler: &Handler,
        effect: Option<&str>,
        op: &str,
    ) -> Result<Option<usize>> {
        let mut found = None;
        for (i, clause) in handler.clauses.iter().enumerate() {
            if clause_matches(clause.effect.as_deref(), &clause.op, effect, op) {
                if found.is_some() {
                    return Err(fault(format!(
                        "ambiguous handler clauses for `{}`",
                        format_op(effect, op)
                    )));
                }
                found = Some(i);
            }
        }
        Ok(found)
    }
}

fn clause_matches(
    clause_effect: Option<&str>,
    clause_op: &str,
    performed_effect: Option<&str>,
    performed_op: &str,
) -> bool {
    clause_op == performed_op
        && match (performed_effect, clause_effect) {
            (Some(performed), Some(clause)) => performed == clause,
            // A bare clause is the ambient effect for that operation.
            (Some(_), None) => true,
            // An unqualified operation is selected by a qualified handler clause.
            (None, Some(_)) => true,
            (None, None) => true,
        }
}

fn format_op(effect: Option<&str>, op: &str) -> String {
    match effect {
        Some(effect) => format!("{effect}.{op}"),
        None => op.to_string(),
    }
}

/// Match `value` against `pat`, appending any bindings to `out`. Returns whether
/// the pattern matched (bindings from a partial match are ignored by the caller).
fn match_pat(pat: &Pat, value: &Value, out: &mut Vec<(String, Value)>) -> bool {
    match pat {
        Pat::Wild => true,
        Pat::Var(name) => {
            out.push((name.clone(), value.clone()));
            true
        }
        Pat::Int(n) => matches!(value, Value::Int(v) if v == n),
        Pat::Real(r) => matches!(value, Value::Real(v) if v == r),
        Pat::Bool(b) => matches!(value, Value::Bool(v) if v == b),
        Pat::Str(s) => matches!(value, Value::Str(v) if v.as_slice() == s.as_slice()),
        Pat::Tuple(pats) => match value {
            Value::Tuple(items) if items.len() == pats.len() => {
                pats.iter().zip(items).all(|(p, v)| match_pat(p, v, out))
            }
            _ => false,
        },
        Pat::Variant { tag, fields } => match value {
            Value::Variant {
                tag: vtag,
                fields: vfields,
                ..
            } if vtag == tag && vfields.len() == fields.len() => fields
                .iter()
                .zip(vfields)
                .all(|(p, v)| match_pat(p, v, out)),
            _ => false,
        },
        Pat::Struct { fields } => match value {
            Value::Struct {
                fields: vfields, ..
            } => fields.iter().all(|(fname, p)| {
                vfields
                    .iter()
                    .find(|(n, _)| n == fname)
                    .is_some_and(|(_, v)| match_pat(p, v, out))
            }),
            _ => false,
        },
        Pat::StrPrefix { prefix, rest } => match value {
            Value::Str(bytes) if bytes.starts_with(prefix) => {
                let tail = Value::Str(Rc::new(bytes[prefix.len()..].to_vec()));
                match_pat(rest, &tail, out)
            }
            _ => false,
        },
    }
}

// -- built-ins --------------------------------------------------------------

/// The arity of a built-in operator, or `None` if the name is not a built-in.
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

fn run_builtin(name: &str, a: Vec<Value>) -> Result<Value> {
    match name {
        "+" | "-" | "*" | "/" | "%" => arith(name, &a[0], &a[1]),
        "neg" => match &a[0] {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Real(r) => Ok(Value::Real(-r)),
            _ => Err(fault("`neg` on a non-number")),
        },
        "not" => match &a[0] {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            _ => Err(fault("`not` on a non-boolean")),
        },
        "?=" => Ok(Value::Bool(value_eq(&a[0], &a[1]))),
        "?<" | "?>" | "<=" | ">=" => compare(name, &a[0], &a[1]),
        "++" => concat(&a[0], &a[1]),
        "array_alloc" => Ok(Value::Str(Rc::new(vec![0u8; as_len(&a[0])?]))),
        "array_len" => Ok(Value::Int(as_bytes(&a[0])?.len() as i64)),
        "array_get" => {
            let bytes = as_bytes(&a[0])?;
            let i = as_index(&a[1])?;
            bytes
                .get(i)
                .map(|b| Value::Int(*b as i64))
                .ok_or_else(|| fault("array index out of bounds"))
        }
        "array_push" => {
            let mut bytes = as_bytes(&a[0])?.clone();
            bytes.push(as_byte(&a[1])?);
            Ok(Value::Str(Rc::new(bytes)))
        }
        "array_set" => {
            let mut bytes = as_bytes(&a[0])?.clone();
            let i = as_index(&a[1])?;
            if i >= bytes.len() {
                return Err(fault("array index out of bounds"));
            }
            bytes[i] = as_byte(&a[2])?;
            Ok(Value::Str(Rc::new(bytes)))
        }
        "array_slice" => {
            let bytes = as_bytes(&a[0])?;
            let mut beg = as_index(&a[1])?;
            let mut end = as_index(&a[2])?;
            beg = beg.min(bytes.len());
            end = end.clamp(beg, bytes.len());
            Ok(Value::Str(Rc::new(bytes[beg..end].to_vec())))
        }
        "vec_new" => Ok(Value::Vector(Rc::new(Vec::new()))),
        "vec_fill" => {
            let n = as_len(&a[0])?;
            Ok(Value::Vector(Rc::new(vec![a[1].clone(); n])))
        }
        "vec_len" => Ok(Value::Int(as_vec(&a[0])?.len() as i64)),
        "vec_get" => {
            let v = as_vec(&a[0])?;
            let i = as_index(&a[1])?;
            v.get(i)
                .cloned()
                .ok_or_else(|| fault("vec index out of bounds"))
        }
        "vec_push" => {
            let mut v = as_vec(&a[0])?.clone();
            v.push(a[1].clone());
            Ok(Value::Vector(Rc::new(v)))
        }
        "vec_set" => {
            let mut v = as_vec(&a[0])?.clone();
            let i = as_index(&a[1])?;
            if i >= v.len() {
                return Err(fault("vec index out of bounds"));
            }
            v[i] = a[2].clone();
            Ok(Value::Vector(Rc::new(v)))
        }
        _ => Err(fault(format!("unknown built-in `{name}`"))),
    }
}

fn arith(op: &str, x: &Value, y: &Value) -> Result<Value> {
    if let (Value::Int(a), Value::Int(b)) = (x, y) {
        let r = match op {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" | "%" if *b == 0 => return Err(fault("division by zero")),
            "/" => a / b,
            "%" => a % b,
            _ => unreachable!("arith called with a non-arithmetic operator"),
        };
        return Ok(Value::Int(r));
    }
    let (a, b) = (as_f64(x)?, as_f64(y)?);
    let r = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "%" => a % b,
        _ => unreachable!("arith called with a non-arithmetic operator"),
    };
    Ok(Value::Real(r))
}

fn compare(op: &str, x: &Value, y: &Value) -> Result<Value> {
    use std::cmp::Ordering;
    let ord = match (x, y) {
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        _ => as_f64(x)?
            .partial_cmp(&as_f64(y)?)
            .ok_or_else(|| fault("comparison of incomparable values"))?,
    };
    let r = match op {
        "?<" => ord == Ordering::Less,
        "?>" => ord == Ordering::Greater,
        "<=" => ord != Ordering::Greater,
        ">=" => ord != Ordering::Less,
        _ => unreachable!("compare called with a non-comparison operator"),
    };
    Ok(Value::Bool(r))
}

fn concat(x: &Value, y: &Value) -> Result<Value> {
    match (x, y) {
        (Value::Str(a), Value::Str(b)) => {
            let mut bytes = a.as_ref().clone();
            bytes.extend_from_slice(b);
            Ok(Value::Str(Rc::new(bytes)))
        }
        // Lists concatenate by rebuilding the left spine onto the right.
        (Value::Variant { ty, .. }, _) if ty == "List" => Ok(list_append(x.clone(), y.clone())),
        _ => Err(fault("`++` on unsupported operands")),
    }
}

fn list_append(xs: Value, ys: Value) -> Value {
    match xs {
        Value::Variant { tag, fields, .. } if tag == "Cons" => {
            let head = fields[0].clone();
            let tail = list_append(fields[1].clone(), ys);
            Value::Variant {
                ty: "List".into(),
                tag: "Cons".into(),
                fields: vec![head, tail],
            }
        }
        // Nil (or any non-cons tail): the right list is the result.
        _ => ys,
    }
}

/// Structural equality (`?=`). Numbers compare across Int/Real; functions are not
/// comparable.
fn value_eq(x: &Value, y: &Value) -> bool {
    match (x, y) {
        (Value::Int(_) | Value::Real(_), Value::Int(_) | Value::Real(_)) => {
            as_f64(x).ok() == as_f64(y).ok()
        }
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Unit, Value::Unit) => true,
        (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(p, q)| value_eq(p, q))
        }
        (Value::Vector(a), Value::Vector(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(p, q)| value_eq(p, q))
        }
        (Value::Struct { fields: a, .. }, Value::Struct { fields: b, .. }) => {
            a.len() == b.len()
                && a.iter().all(|(n, v)| {
                    b.iter()
                        .find(|(m, _)| m == n)
                        .is_some_and(|(_, w)| value_eq(v, w))
                })
        }
        (
            Value::Variant {
                tag: ta,
                fields: fa,
                ..
            },
            Value::Variant {
                tag: tb,
                fields: fb,
                ..
            },
        ) => ta == tb && fa.len() == fb.len() && fa.iter().zip(fb).all(|(p, q)| value_eq(p, q)),
        _ => false,
    }
}

fn as_f64(v: &Value) -> Result<f64> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::Real(r) => Ok(*r),
        _ => Err(fault("expected a number")),
    }
}

fn as_bytes(v: &Value) -> Result<&Vec<u8>> {
    match v {
        Value::Str(b) => Ok(b),
        _ => Err(fault("expected a byte vector")),
    }
}

fn as_vec(v: &Value) -> Result<&Vec<Value>> {
    match v {
        Value::Vector(items) => Ok(items),
        _ => Err(fault("expected a vector")),
    }
}

fn as_index(v: &Value) -> Result<usize> {
    match v {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        Value::Int(_) => Err(fault("negative index")),
        _ => Err(fault("expected an integer index")),
    }
}

fn as_len(v: &Value) -> Result<usize> {
    match v {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        _ => Err(fault("expected a non-negative length")),
    }
}

fn as_byte(v: &Value) -> Result<u8> {
    match v {
        Value::Int(n) if (0..=255).contains(n) => Ok(*n as u8),
        _ => Err(fault("expected a byte value (0..255)")),
    }
}

fn fault(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Code::RuntimeFault, Span::at(0), 0, msg.into())
}

impl Value {
    /// A short human display of a value, for the `run` command.
    pub fn show(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "{}".into(),
            Value::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(Value::show).collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Struct { name, fields } => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(n, v)| format!(".{n} = {}", v.show()))
                    .collect();
                format!("{name}.{{ {} }}", inner.join(", "))
            }
            Value::Variant { tag, fields, .. } => {
                if fields.is_empty() {
                    format!(".{tag}")
                } else {
                    let inner: Vec<String> = fields.iter().map(Value::show).collect();
                    format!(".{tag}.{{ {} }}", inner.join(", "))
                }
            }
            Value::Vector(items) => {
                let inner: Vec<String> = items.iter().map(Value::show).collect();
                format!("vec[{}]", inner.join(", "))
            }
            Value::Closure(_) | Value::Builtin { .. } | Value::Operation { .. } => {
                "<function>".into()
            }
            Value::Resumption(_) => "<continuation>".into(),
        }
    }
}
