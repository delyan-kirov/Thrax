//! The reified-K abstract machine: the port of `engines/IT.cpp`. It executes the
//! IR (closure-converted Core) with an EXPLICIT heap continuation stack, so deep
//! non-tail recursion grows the heap rather than the host stack, and a tail call
//! reuses the activation (constant stack). This is the substrate the effect
//! machinery (handlers, first-class one-shot resumptions) extends.
//!
//! It is strict: data constructors are eager. A value is the reference-counted
//! [`PVal`]; a closure is a [`Value::Code`] (an IR code index plus its captured
//! environment); globals are lazy-memoized CAFs. Built-ins dispatch dynamically
//! by name on the value kinds, exactly as the tree-walker does, so no resolved
//! overload keys are needed.
//!
//! The machine coexists with the tree-walker ([`crate::eval`]); a test diffs the
//! two so this port can be validated against the proven interpreter before it
//! takes over.

pub mod data;
pub mod ffi;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ir::data::{Atom, AltKind, Expr, Program};
use utilities::Result;

use data::{builtin_arity, fault, mk, run_builtin, run_extern, PVal, Resumption, Value};

/// One activation: the local slot array (params + let/case binders) and the
/// current closure's captured record.
struct Frame<'p> {
    locals: Vec<PVal<'p>>,
    env: Vec<PVal<'p>>,
}
type FrameP<'p> = Rc<RefCell<Frame<'p>>>;

/// A return continuation: on a value `v`, back-patch the let's placeholder box
/// with `v`, bind the box into the saved frame's slot, and resume `cont`. Binding
/// the box (not `v`) lets a recursive binding's closure, which captured the box
/// weakly, observe the finished value.
pub(crate) struct KRet<'p> {
    boxed: PVal<'p>,
    slot: usize,
    cont: &'p Expr,
    frame: FrameP<'p>,
}

/// A handler's evaluated clauses and value clause `els`, installed by a prompt.
struct Handler<'p> {
    clauses: Vec<HClause<'p>>,
    els: PVal<'p>,
}

/// One evaluated handler clause: its operation (optionally effect-qualified) and
/// the 2-parameter closure `\arg = \k = body`.
struct HClause<'p> {
    effect: Option<String>,
    op: String,
    fun: PVal<'p>,
}

/// A prompt delimiter: where a handler is installed. `perform` searches down for
/// the nearest one with a clause for the operation; a value reaching it runs `els`.
pub(crate) struct KPrompt<'p> {
    handler: Handler<'p>,
}

/// A `defer` marker. A value returning through it runs `cleanup` then continues;
/// a cleanup captured in a discarded continuation runs at the clause boundary
/// instead (see [`KAfterClause`]).
pub(crate) struct KDefer<'p> {
    cleanup: PVal<'p>,
}

/// "Deliver `saved`, ignoring the incoming value." Pushed under a `KDefer` so the
/// cleanup's own result is discarded and the protected value continues.
pub(crate) struct KThunkRet<'p> {
    saved: PVal<'p>,
}

/// Marks a handler clause boundary, holding the clause's resumption `kval`. When
/// the clause finishes, this decides the fate of any `defer` cleanups captured in
/// `kval`: if it was resumed or stored they run elsewhere; if it was discarded
/// (the abort case) they run HERE, with the enclosing handlers still installed.
pub(crate) struct KAfterClause<'p> {
    kval: PVal<'p>,
}

/// One frame of the reified continuation stack.
pub(crate) enum KFrame<'p> {
    Ret(KRet<'p>),
    Prompt(KPrompt<'p>),
    Defer(KDefer<'p>),
    ThunkRet(KThunkRet<'p>),
    AfterClause(KAfterClause<'p>),
}

/// The machine: the program plus the global (CAF) memo table. One instance runs a
/// whole program and is reused across nested CAF evaluations.
pub struct Machine<'p> {
    prog: &'p Program,
    /// Canonical global name (`Module.name`) -> its CAF code index.
    globals: HashMap<String, usize>,
    /// Bare name (a global's last segment) -> its CAF, first definition winning.
    /// The fallback for an unqualified reference or the entry point.
    bare: HashMap<String, usize>,
    /// CAF memo keyed by code index, so a global reached by its canonical and its
    /// bare name is still evaluated once.
    memo: RefCell<HashMap<usize, PVal<'p>>>,
    in_progress: RefCell<HashSet<usize>>,
    ops_by_effect: HashMap<String, Vec<String>>,
    ops_by_name: HashMap<String, Vec<String>>,
}

/// The mutable state of one `run`: the current activation, the reified
/// continuation, and the recursive-closure keep-alive list.
struct Exec<'p> {
    frame: FrameP<'p>,
    ctrl: &'p Expr,
    kont: Vec<KFrame<'p>>,
    /// Recursive-let closures are reachable from their own env only weakly. With
    /// calls trampolined (no host-stack pinning), keep each one alive here for the
    /// run's duration. Dedup against the back keeps self-recursion O(1).
    live: Vec<PVal<'p>>,
}

/// Collapse a chain of recursive placeholder cells to the underlying value.
pub fn deref<'p>(mut v: PVal<'p>) -> PVal<'p> {
    loop {
        let up = match &*v.borrow() {
            Value::Rec(w) => Some(w.upgrade().expect("machine: recursive cell dropped")),
            _ => None,
        };
        match up {
            Some(next) => v = next,
            None => return v,
        }
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
            _ => true,
        }
}

fn format_op(effect: Option<&str>, op: &str) -> String {
    match effect {
        Some(effect) => format!("{effect}.{op}"),
        None => op.to_string(),
    }
}

impl<'p> Machine<'p> {
    pub fn new(prog: &'p Program) -> Machine<'p> {
        let mut globals = HashMap::new();
        let mut bare = HashMap::new();
        for (name, code) in &prog.globals {
            globals.entry(name.clone()).or_insert(*code);
            let last = name.rsplit('.').next().unwrap_or(name);
            bare.entry(last.to_string()).or_insert(*code);
        }
        let mut ops_by_effect: HashMap<String, Vec<String>> = HashMap::new();
        let mut ops_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for e in &prog.effects {
            ops_by_effect
                .entry(e.effect.clone())
                .or_default()
                .push(e.op.clone());
            ops_by_name
                .entry(e.op.clone())
                .or_default()
                .push(e.effect.clone());
        }
        for effects in ops_by_name.values_mut() {
            effects.sort();
            effects.dedup();
        }
        Machine {
            prog,
            globals,
            bare,
            memo: RefCell::new(HashMap::new()),
            in_progress: RefCell::new(HashSet::new()),
            ops_by_effect,
            ops_by_name,
        }
    }

    /// Force and return a global (by canonical or bare name), then collapse
    /// recursive cells.
    pub fn eval_global(&self, name: &str) -> Result<PVal<'p>> {
        Ok(deref(self.glob(name)?))
    }

    /// Force a lazy-memoized CAF (keyed by code index, so the canonical and bare
    /// names of one global share a single evaluation).
    fn force(&self, code: usize) -> Result<PVal<'p>> {
        if let Some(v) = self.memo.borrow().get(&code) {
            return Ok(v.clone());
        }
        if self.in_progress.borrow().contains(&code) {
            return Err(fault("a value global refers to itself while being defined"));
        }
        self.in_progress.borrow_mut().insert(code);
        let result = self.run(code, Vec::new(), Vec::new());
        self.in_progress.borrow_mut().remove(&code);
        let v = result?;
        self.memo.borrow_mut().insert(code, v.clone());
        Ok(v)
    }

    /// Resolve a single canonical name (the C++ `IR::Glob` scheme). The order
    /// matters: an EXACT canonical global (`Module.name`, the form the checker
    /// mangles every resolved reference to) wins first, then a built-in, then a
    /// `TARGET.` member, then an effect operation, and only LAST the unqualified
    /// fallback. Effect ops precede that fallback so an unrelated imported global
    /// of the same name (whose last segment aliases into `bare`) cannot shadow a
    /// same-named operation. The `C` libc namespace resolves as exact `@extern`
    /// globals (`C.sqrt`, ...).
    fn glob(&self, name: &str) -> Result<PVal<'p>> {
        if let Some(&code) = self.globals.get(name) {
            return self.force(code);
        }
        if let Some(arity) = builtin_arity(name) {
            return Ok(mk(Value::Builtin {
                name: name.into(),
                arity,
                args: Vec::new(),
            }));
        }
        if let Some(suffix) = name.strip_prefix("TARGET.") {
            if let Some(v) = clib::target_value(suffix) {
                return Ok(mk(v));
            }
        }
        // An effect operation: `Effect.op` names the effect, a bare op resolves to
        // the sole effect declaring it (else stays ambient, matched by the handler).
        if let Some((eff, op)) = name.split_once('.') {
            if self
                .ops_by_effect
                .get(eff)
                .is_some_and(|ops| ops.iter().any(|o| o == op))
            {
                return Ok(mk(Value::Op {
                    effect: Some(eff.to_string()),
                    op: op.to_string(),
                }));
            }
        }
        if let Some(effects) = self.ops_by_name.get(name) {
            let effect = if effects.len() == 1 {
                Some(effects[0].clone())
            } else {
                None
            };
            return Ok(mk(Value::Op {
                effect,
                op: name.to_string(),
            }));
        }
        // The unqualified fallback: a reference the checker left bare (the entry
        // point, or a not-module-resolved name), matched by last name-segment.
        if let Some(&code) = self.bare.get(name) {
            return self.force(code);
        }
        Err(fault(format!("unbound name `{name}`")))
    }

    /// Read an atom in `frame` WITHOUT forcing a recursive cell (so a closure
    /// capture keeps its weak self-reference); callers `deref` where a real value
    /// is needed.
    fn eval_atom(&self, a: &Atom, frame: &FrameP<'p>) -> Result<PVal<'p>> {
        match a {
            Atom::Local(i) => frame
                .borrow()
                .locals
                .get(*i)
                .cloned()
                .ok_or_else(|| fault("machine: local index out of range")),
            Atom::Env(i) => frame
                .borrow()
                .env
                .get(*i)
                .cloned()
                .ok_or_else(|| fault("machine: env index out of range")),
            Atom::Glob { name } => self.glob(name),
            Atom::LitI(n) => Ok(mk(Value::Int(*n))),
            Atom::LitR(r) => Ok(mk(Value::Real(*r))),
            Atom::LitS(s) => Ok(mk(Value::Str(Rc::new(s.clone())))),
            Atom::LitB(b) => Ok(mk(Value::Bool(*b))),
            Atom::Unit => Ok(mk(Value::Unit)),
            Atom::Clos { code, captures } => {
                let mut env = Vec::with_capacity(captures.len());
                for c in captures {
                    env.push(self.eval_atom(c, frame)?);
                }
                Ok(mk(Value::Code { code: *code, env }))
            }
            Atom::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
            } => Ok(mk(Value::Extern {
                abi: Rc::from(abi.as_str()),
                symbol: Rc::from(symbol.as_str()),
                lib: Rc::from(lib.as_str()),
                arg_types: Rc::from(arg_types.as_slice()),
                ret_type: Rc::from(ret_type.as_str()),
                args: Vec::new(),
            })),
        }
    }

    /// Apply a value to one argument to completion, via a nested `run`. Used for
    /// the program entry point; inside `run`, application is inlined (no nesting).
    pub fn apply(&self, callee: PVal<'p>, arg: PVal<'p>) -> Result<PVal<'p>> {
        let callee = deref(callee);
        let (code, env) = match &*callee.borrow() {
            Value::Code { code, env } => (*code, env.clone()),
            _ => return Err(fault("machine: applying a non-function value")),
        };
        self.run(code, vec![arg], env)
    }

    /// Run `code` (with the given initial locals and captured env) to a value,
    /// driving the machine loop until the continuation stack empties.
    fn run(&self, code: usize, mut locals: Vec<PVal<'p>>, env: Vec<PVal<'p>>) -> Result<PVal<'p>> {
        let entry = &self.prog.codes[code];
        locals.resize(entry.nlocals, mk(Value::Unk));
        let mut ex = Exec {
            frame: Rc::new(RefCell::new(Frame { locals, env })),
            ctrl: &entry.body,
            kont: Vec::new(),
            live: Vec::new(),
        };

        loop {
            match ex.ctrl {
                Expr::Ret(a) => {
                    let v = deref(self.eval_atom(a, &ex.frame)?);
                    if let Some(r) = ex.ret(self, v)? {
                        return Ok(r);
                    }
                }

                Expr::Let { slot, rhs, body } => {
                    let boxed = mk(Value::Unk);
                    let self_ = mk(Value::Rec(Rc::downgrade(&boxed)));
                    ex.frame.borrow_mut().locals[*slot] = self_;
                    ex.kont.push(KFrame::Ret(KRet {
                        boxed,
                        slot: *slot,
                        cont: body.as_ref(),
                        frame: ex.frame.clone(),
                    }));
                    ex.ctrl = rhs.as_ref();
                }

                Expr::App { fun, arg, .. } => {
                    let callee = deref(self.eval_atom(fun, &ex.frame)?);
                    let argv = deref(self.eval_atom(arg, &ex.frame)?);
                    if let Some(r) = self.apply_inline(&mut ex, callee, argv)? {
                        return Ok(r);
                    }
                }

                Expr::Case {
                    scrut,
                    alts,
                    default,
                } => {
                    let sv = deref(self.eval_atom(scrut, &ex.frame)?);
                    let mut next: Option<&'p Expr> = None;
                    for alt in alts {
                        let matched = {
                            let b = sv.borrow();
                            match (&alt.kind, &*b) {
                                (AltKind::Int(k), Value::Int(v)) => v == k,
                                (AltKind::Real(r), Value::Real(v)) => v == r,
                                (AltKind::Bool(x), Value::Bool(v)) => v == x,
                                (AltKind::Con(c), Value::Variant { tag, .. }) => tag == c,
                                _ => false,
                            }
                        };
                        if matched {
                            if let AltKind::Con(_) = &alt.kind {
                                let fields = match &*sv.borrow() {
                                    Value::Variant { fields, .. } => fields.clone(),
                                    _ => unreachable!("Con alt matched a non-variant"),
                                };
                                let mut f = ex.frame.borrow_mut();
                                for (i, val) in fields.into_iter().enumerate() {
                                    f.locals[alt.binder_base + i] = val;
                                }
                            }
                            next = Some(&alt.body);
                            break;
                        }
                    }
                    ex.ctrl = next.unwrap_or_else(|| default.as_ref());
                }

                Expr::MkStruct { name, base, fields } => {
                    let (mut out, base_name): (Vec<(String, PVal<'p>)>, String) = match base {
                        Some(b) => {
                            let bv = deref(self.eval_atom(b, &ex.frame)?);
                            let bb = bv.borrow();
                            match &*bb {
                                Value::Struct { name, fields } => (fields.clone(), name.clone()),
                                _ => return Err(fault("record update of a non-struct value")),
                            }
                        }
                        None => (Vec::new(), String::new()),
                    };
                    for (fname, fa) in fields {
                        let fv = deref(self.eval_atom(fa, &ex.frame)?);
                        match out.iter_mut().find(|(n, _)| n == fname) {
                            Some(slot) => slot.1 = fv,
                            None => out.push((fname.clone(), fv)),
                        }
                    }
                    let sname = if name.is_empty() {
                        base_name
                    } else {
                        name.clone()
                    };
                    let v = mk(Value::Struct {
                        name: sname,
                        fields: out,
                    });
                    if let Some(r) = ex.ret(self, v)? {
                        return Ok(r);
                    }
                }

                Expr::Field { rec, name } => {
                    let rv = deref(self.eval_atom(rec, &ex.frame)?);
                    let v = {
                        let b = rv.borrow();
                        match &*b {
                            Value::Struct { fields, .. } => fields
                                .iter()
                                .find(|(n, _)| n == name)
                                .map(|(_, v)| v.clone()),
                            Value::Tuple(items) => {
                                name.parse::<usize>().ok().and_then(|i| items.get(i).cloned())
                            }
                            _ => None,
                        }
                    }
                    .ok_or_else(|| fault(format!("no field `{name}`")))?;
                    if let Some(r) = ex.ret(self, v)? {
                        return Ok(r);
                    }
                }

                Expr::MkVariant { ty, tag, fields } => {
                    let mut fs = Vec::with_capacity(fields.len());
                    for a in fields {
                        fs.push(deref(self.eval_atom(a, &ex.frame)?));
                    }
                    let v = mk(Value::Variant {
                        ty: ty.clone(),
                        tag: tag.clone(),
                        fields: fs,
                    });
                    if let Some(r) = ex.ret(self, v)? {
                        return Ok(r);
                    }
                }

                Expr::MkTuple(items) => {
                    let mut fs = Vec::with_capacity(items.len());
                    for a in items {
                        fs.push(deref(self.eval_atom(a, &ex.frame)?));
                    }
                    let v = mk(Value::Tuple(fs));
                    if let Some(r) = ex.ret(self, v)? {
                        return Ok(r);
                    }
                }

                Expr::Handle {
                    body,
                    clauses,
                    els,
                } => {
                    let mut hclauses = Vec::with_capacity(clauses.len());
                    for c in clauses {
                        let fun = deref(self.eval_atom(&c.fun, &ex.frame)?);
                        hclauses.push(HClause {
                            effect: c.effect.clone(),
                            op: c.op.clone(),
                            fun,
                        });
                    }
                    let els_v = deref(self.eval_atom(els, &ex.frame)?);
                    ex.kont.push(KFrame::Prompt(KPrompt {
                        handler: Handler {
                            clauses: hclauses,
                            els: els_v,
                        },
                    }));
                    ex.ctrl = body.as_ref();
                }

                Expr::Defer { cleanup, body } => {
                    let cl = deref(self.eval_atom(cleanup, &ex.frame)?);
                    ex.kont.push(KFrame::Defer(KDefer { cleanup: cl }));
                    ex.ctrl = body.as_ref();
                }

                Expr::Fault(s) => return Err(fault(s.clone())),
            }
        }
    }

    /// Dispatch an application inside the run loop: a closure jumps in place (a
    /// tail call reusing the loop), a builtin runs or accumulates, an operation
    /// performs, a resumption splices its captured slice back on. Returns
    /// `Some(result)` only if the continuation stack emptied.
    fn apply_inline(
        &self,
        ex: &mut Exec<'p>,
        callee: PVal<'p>,
        argv: PVal<'p>,
    ) -> Result<Option<PVal<'p>>> {
        enum Kind<'p> {
            Code(usize, Vec<PVal<'p>>),
            Builtin(Rc<str>, usize, Vec<PVal<'p>>),
            Extern(Rc<str>, Rc<str>, Rc<str>, Rc<[String]>, Rc<str>, Vec<PVal<'p>>),
            Op(Option<String>, String),
            Resump(Rc<RefCell<Resumption<'p>>>),
            Bad,
        }
        let kind = match &*callee.borrow() {
            Value::Code { code, env } => Kind::Code(*code, env.clone()),
            Value::Builtin { name, arity, args } => {
                Kind::Builtin(name.clone(), *arity, args.clone())
            }
            Value::Extern {
                abi,
                symbol,
                lib,
                arg_types,
                ret_type,
                args,
            } => Kind::Extern(
                abi.clone(),
                symbol.clone(),
                lib.clone(),
                arg_types.clone(),
                ret_type.clone(),
                args.clone(),
            ),
            Value::Op { effect, op } => Kind::Op(effect.clone(), op.clone()),
            Value::Resump(r) => Kind::Resump(r.clone()),
            _ => Kind::Bad,
        };

        match kind {
            Kind::Code(ccode, cenv) => {
                // Pin any recursive closures reachable from this one's captures, so
                // a tail call doesn't free them out from under their own weak self-
                // reference.
                for cap in &cenv {
                    let tgt = match &*cap.borrow() {
                        Value::Rec(w) => w.upgrade(),
                        _ => None,
                    };
                    if let Some(t) = tgt {
                        if ex.live.last().is_none_or(|l| !Rc::ptr_eq(l, &t)) {
                            ex.live.push(t);
                        }
                    }
                }
                let cc = &self.prog.codes[ccode];
                let mut locals = vec![mk(Value::Unk); cc.nlocals];
                locals[0] = argv;
                ex.frame = Rc::new(RefCell::new(Frame {
                    locals,
                    env: cenv,
                }));
                ex.ctrl = &cc.body;
                Ok(None)
            }

            Kind::Builtin(name, arity, mut args) => {
                args.push(argv);
                let v = if args.len() >= arity {
                    // `generate template f` is HIGHER-ORDER: it applies the closure
                    // `f` to each index, so it runs here (where `self.apply` can drive
                    // the machine) rather than in the leaf `run_builtin`.
                    if &*name == "@tensor_create" {
                        // Build the elements by applying `f` at each index of the
                        // template's leading axis, then stack them into a tensor.
                        let (_, _, shape, _) = data::tensor_fields(&args[0])?;
                        let len = shape.first().copied().unwrap_or(0);
                        let f = args[1].clone();
                        let mut out = Vec::with_capacity(len);
                        for i in 0..len {
                            out.push(self.apply(f.clone(), mk(Value::Int(i as i64)))?);
                        }
                        mk(data::tensor_stack(&out)?)
                    } else {
                        mk(run_builtin(&name, &args)?)
                    }
                } else {
                    mk(Value::Builtin { name, arity, args })
                };
                ex.ret(self, v)
            }

            Kind::Extern(abi, symbol, lib, arg_types, ret_type, mut args) => {
                args.push(argv);
                let v = if args.len() >= arg_types.len() {
                    mk(run_extern(&abi, &symbol, &lib, &arg_types, &ret_type, &args)?)
                } else {
                    mk(Value::Extern {
                        abi,
                        symbol,
                        lib,
                        arg_types,
                        ret_type,
                        args,
                    })
                };
                ex.ret(self, v)
            }

            Kind::Op(effect, op) => {
                let mut found: Option<(usize, PVal<'p>)> = None;
                for i in (0..ex.kont.len()).rev() {
                    if let KFrame::Prompt(kp) = &ex.kont[i] {
                        for cl in &kp.handler.clauses {
                            if clause_matches(
                                cl.effect.as_deref(),
                                &cl.op,
                                effect.as_deref(),
                                &op,
                            ) {
                                found = Some((i, cl.fun.clone()));
                                break;
                            }
                        }
                        if found.is_some() {
                            break;
                        }
                    }
                }
                let (p, clause) = found.ok_or_else(|| {
                    fault(format!(
                        "unhandled effect operation `{}`",
                        format_op(effect.as_deref(), &op)
                    ))
                })?;
                // Capture the slice from the prompt (inclusive: a deep handler) up
                // to here; the clause runs below the prompt (outside it).
                let seg = ex.kont.split_off(p);
                let res = Rc::new(RefCell::new(Resumption { seg, used: false }));
                let kval = mk(Value::Resump(res));
                ex.kont
                    .push(KFrame::AfterClause(KAfterClause { kval: kval.clone() }));
                // Enter the clause inline: a 2-slot code with the operation argument
                // in slot 0 and the resumption in slot 1. Its body runs on the now-
                // truncated stack, so perform/resume chains stay constant-stack.
                let cl = deref(clause);
                ex.enter(self, &cl, &[argv, kval])?;
                Ok(None)
            }

            Kind::Resump(res) => {
                {
                    let r = res.borrow();
                    if r.used {
                        return Err(fault("continuation already resumed"));
                    }
                }
                let seg = {
                    let mut r = res.borrow_mut();
                    r.used = true;
                    std::mem::take(&mut r.seg)
                };
                // Splice the captured slice back on (re-installing its prompt: deep)
                // and deliver the value to the suspended point.
                ex.kont.extend(seg);
                ex.ret(self, argv)
            }

            Kind::Bad => Err(fault("applied a non-function value")),
        }
    }
}

impl<'p> Exec<'p> {
    /// Enter a closure: set the activation to a fresh frame with `args` in the
    /// leading local slots (and the rest `Unk`), and point control at its body.
    fn enter(&mut self, m: &Machine<'p>, clo: &PVal<'p>, args: &[PVal<'p>]) -> Result<()> {
        let (ccode, cenv) = match &*clo.borrow() {
            Value::Code { code, env } => (*code, env.clone()),
            _ => return Err(fault("machine: entering a non-closure")),
        };
        let cc = &m.prog.codes[ccode];
        let mut locals = vec![mk(Value::Unk); cc.nlocals];
        for (i, a) in args.iter().enumerate() {
            locals[i] = a.clone();
        }
        self.frame = Rc::new(RefCell::new(Frame {
            locals,
            env: cenv,
        }));
        self.ctrl = &cc.body;
        Ok(())
    }

    /// Hand a finished value to the top continuation frame (or finish the run,
    /// returning `Some`). Loops for the frames that resolve without a control jump
    /// (`ThunkRet`, a resumed/stored `AfterClause`).
    fn ret(&mut self, m: &Machine<'p>, mut v: PVal<'p>) -> Result<Option<PVal<'p>>> {
        loop {
            let Some(kf) = self.kont.pop() else {
                return Ok(Some(v));
            };
            match kf {
                KFrame::Ret(kr) => {
                    let filled = v.borrow().clone_shallow();
                    *kr.boxed.borrow_mut() = filled;
                    kr.frame.borrow_mut().locals[kr.slot] = kr.boxed.clone();
                    self.frame = kr.frame;
                    self.ctrl = kr.cont;
                    return Ok(None);
                }
                KFrame::Prompt(kp) => {
                    let els = deref(kp.handler.els.clone());
                    self.enter(m, &els, &[v])?;
                    return Ok(None);
                }
                KFrame::Defer(kd) => {
                    let cleanup = deref(kd.cleanup);
                    self.kont.push(KFrame::ThunkRet(KThunkRet { saved: v }));
                    self.enter(m, &cleanup, &[])?;
                    return Ok(None);
                }
                KFrame::ThunkRet(kt) => {
                    v = kt.saved;
                    continue;
                }
                KFrame::AfterClause(ka) => {
                    // Baseline references to kval: this popped frame's, and the
                    // clause's slot 1 (the clause frame is still current). Any beyond
                    // that means the clause stashed `k` (a generator / scheduler).
                    let (used, seg_defers) = {
                        let b = ka.kval.borrow();
                        let Value::Resump(res) = &*b else {
                            return Err(fault("machine: KAfterClause without a resumption"));
                        };
                        let r = res.borrow();
                        let defers: Vec<PVal<'p>> = r
                            .seg
                            .iter()
                            .filter_map(|f| match f {
                                KFrame::Defer(d) => Some(d.cleanup.clone()),
                                _ => None,
                            })
                            .collect();
                        (r.used, defers)
                    };
                    let stored = Rc::strong_count(&ka.kval) > 2;
                    if used || stored {
                        continue;
                    }
                    // Discarded (abort): run the captured cleanups now, innermost
                    // first (LIFO: seg order pushed => topmost runs first), then re-
                    // deliver the clause's value `v`.
                    self.kont.push(KFrame::ThunkRet(KThunkRet { saved: v }));
                    for cleanup in seg_defers {
                        self.kont.push(KFrame::Defer(KDefer { cleanup }));
                    }
                    v = mk(Value::Unk);
                    continue;
                }
            }
        }
    }
}

/// Evaluate a global of `prog` on the machine and render it, for diffing against
/// the tree-walker.
pub fn eval(prog: &Program, name: &str) -> Result<String> {
    let m = Machine::new(prog);
    let v = m.eval_global(name)?;
    let s = v.borrow().show();
    Ok(s)
}

/// `TARGET` host reflection, ported from the tree-walker so the machine matches
/// it byte-for-byte. The `C` libc namespace is no longer served here: it flows
/// through the single `@extern` FFI path (`run_extern`).
mod clib {
    use std::rc::Rc;

    use super::data::Value;

    /// A `TARGET.<field>` reflection constant. The interpreter is defined to run
    /// programs for the host, so the reflected target is `Target::host()`.
    pub(super) fn target_value<'p>(name: &str) -> Option<Value<'p>> {
        let t = utilities::Target::host();
        let bytes = |s: &str| Value::Str(Rc::new(s.as_bytes().to_vec()));
        Some(match name {
            "int_bits" | "ptr_bits" => Value::Int(t.ptr_bits() as i64),
            "int_max" => Value::Int(t.int_max() as i64),
            "int_min" => Value::Int(t.int_min() as i64),
            "arch" => bytes(t.arch_name()),
            "os" => bytes(t.os_name()),
            "name" => Value::Str(Rc::new(t.name().into_bytes())),
            _ => return None,
        })
    }
}
