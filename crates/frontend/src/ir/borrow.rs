//! Borrow inference: which parameters a code only reads.
//!
//! The algorithm follows Ullrich and de Moura, "Counting Immutable Beans:
//! Reference Counting Optimized for Purely Functional Programming" (IFL 2019),
//! as used by Perceus (Reinking, Xie, de Moura and Leijen, PLDI 2021): a
//! parameter that the body only inspects is borrowed, so a caller that keeps
//! its own reference lends the value instead of handing one over, and neither
//! side counts. See Note [Borrowed parameters].
//!
//! Every use of a parameter, or of a part of one (a `Case` binder or a field
//! read out of it), is classified as a borrow (a `Case` scrutinee, a
//! field read, an operand a builtin only reads, an argument to another
//! borrowed parameter) or an own (anything that stores, returns, applies or
//! hands the value to code that is not known). Starting from "every
//! parameter of a code with a known call site is borrowed", a greatest
//! fixpoint over the call graph demotes each parameter with an owning use,
//! and each one passed to a parameter that was demoted. Then every call to a
//! borrowed parameter whose argument outlives the callee is marked as lending
//! it.
//!
//! Input: freshly closure-converted codes. Output: the same codes with
//! [`super::data::Code::borrowed`] and the lending flag of each `App` filled in, for
//! [`super::ownership`] to honour.
//!
//! Entry point: [`infer_borrowed_params`].

use std::collections::HashMap;

use utilities::StrId;

use super::data::{builtin, Atom, IrExpr, IrProgram, Prim};

// Note [Borrowed parameters]
//
// Every activation owns one reference per local slot, so passing a value the caller still needs costs an increment at the call and
// a decrement when the callee's parameter dies, which is wasted work for a
// callee that only reads its argument. See Note [Last use moves].
//
// A borrowed parameter breaks that pairing. The callee promises never to move
// or drop it, so it holds no reference of its own; the caller promises the
// value stays alive until the callee is done. The two promises are recorded
// separately, which is what keeps the convention safe when one side does not
// know the other:
//
// - `Code::borrowed[i]` is the callee's promise. The ownership pass pins a
//   borrowed parameter, so the body reads it but never moves or drops it.
// - `App::lend` is the caller's promise, made only when the argument outlives
//   the call: a local or captured field of an activation that waits for the
//   result, a global (its cache lives as long as the program), or, for a tail
//   call that ends the caller, a parameter the caller itself borrowed.
//
// An engine binds a parameter without counting only when both hold. A call
// that lends to an owned parameter counts as usual, and a borrowed parameter
// entered by a call that did not lend (a closure applied by unknown code, a
// handler clause run by the engine) takes a counted reference that dies with
// the activation, since the body never drops it. A tail call that passes on a
// parameter it borrowed lends it again when the activation borrowed it too,
// and otherwise hands its counted reference over.
//
// Three rules keep the promise to read only:
//
// - A value the callee stores (in a constructor, a closure, a handler) or
//   returns is owned, so that the store or the caller can be its sole owner.
//   So is one with a part (a `Case` binder or a field read out of it) used
//   that way: while the caller holds the whole, no part of it is unique, and
//   a resumption or a vector reached through it would be copied rather than
//   moved or updated in place.
// - A builtin operand is a borrow only when the builtin reads it. One that may
//   update its operand in place (`@vec_push`) needs it uniquely owned, which a
//   borrowed value never is.
// - Nothing in a handler body borrows or lends. A resumption can re-enter the
//   body after the activation that lent the value has finished.

/// Fill in [`super::data::Code::borrowed`] and each `App`'s `lend` flag.
/// Run once, on freshly closure-converted codes, before the ownership pass.
/// See Note [Borrowed parameters].
pub fn infer_borrowed_params(program: &mut IrProgram) {
    let resolver = Resolver::new(program);
    let ncodes = program.codes.len();

    let mut facts: Vec<Vec<(usize, Need)>> = vec![Vec::new(); ncodes];
    let mut known_calls = vec![false; ncodes];
    for (c, code) in program.codes.iter_mut().enumerate() {
        let nparams = code.nparams;
        let mut parts = vec![Vec::new(); code.nlocals];
        parts_of_params(&code.body, nparams, &mut parts);
        let mut known = vec![None; code.nlocals];
        walk(&mut code.body, Position::TAIL, &mut known, &resolver, &mut |atom, how, at| {
            if let How::Arg { callee: Some(Callee::Code(d)), .. } = how {
                known_calls[d] = true;
            }
            let Atom::Local(s) = atom else { return };
            let params: &[usize] = if *s < nparams { std::slice::from_ref(s) } else { &parts[*s] };
            if params.is_empty() {
                return;
            }
            let need = match how {
                _ if at.handled => Need::Own,
                How::Borrow => return,
                How::Own => Need::Own,
                How::Arg { callee: Some(Callee::Code(d)), .. } => Need::Borrowed(d),
                How::Arg { callee: Some(Callee::Prim(prim, i)), .. } if reads_operand(prim, i) => return,
                How::Arg { .. } => Need::Own,
            };
            facts[c].extend(params.iter().map(|p| (*p, need)));
        });
    }

    let mut borrowed: Vec<Vec<bool>> = program
        .codes
        .iter()
        .enumerate()
        .map(|(c, code)| vec![known_calls[c]; code.nparams])
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for c in 0..ncodes {
            for &(p, need) in &facts[c] {
                let keep = match need {
                    Need::Own => false,
                    Need::Borrowed(d) => borrowed[d].first().copied().unwrap_or(false),
                };
                if !keep && borrowed[c][p] {
                    borrowed[c][p] = false;
                    changed = true;
                }
            }
        }
    }

    for (c, code) in program.codes.iter_mut().enumerate() {
        let mine = &borrowed[c];
        let mut known = vec![None; code.nlocals];
        walk(&mut code.body, Position::TAIL, &mut known, &resolver, &mut |atom, how, at| {
            let How::Arg { callee: Some(Callee::Code(d)), lend } = how else { return };
            if !borrowed[d].first().copied().unwrap_or(false) || at.handled {
                return;
            }
            *lend = match atom {
                Atom::Local(s) if at.tail => mine.get(*s).copied().unwrap_or(false),
                Atom::Local(_) | Atom::Env(_) => !at.tail,
                Atom::Glob { name } => matches!(resolver.glob(*name), Some(Callee::Code(_))),
                _ => false,
            };
        });
        code.borrowed = borrowed[c].clone();
    }
}

/// Record in `parts[s]` the parameters whose part local `s` may hold: a `Case`
/// binder or a field read out of a parameter, or out of such a part. A slot
/// reused for other values keeps the union, which only owns more.
fn parts_of_params(e: &IrExpr, nparams: usize, parts: &mut [Vec<usize>]) {
    let sources = |x: &Atom, parts: &[Vec<usize>]| match x {
        Atom::Local(x) if *x < nparams => vec![*x],
        Atom::Local(x) => parts[*x].clone(),
        _ => Vec::new(),
    };
    match e {
        IrExpr::Let { slot, rhs, body } => {
            parts_of_params(rhs, nparams, parts);
            if let IrExpr::Field { rec, .. } = rhs.as_ref() {
                let from = sources(rec, parts);
                parts[*slot].extend(from);
            }
            parts_of_params(body, nparams, parts);
        }
        IrExpr::Case { scrut, alts, default } => {
            let from = sources(scrut, parts);
            for alt in alts {
                for b in alt.binder_base..alt.binder_base + alt.binders.len() {
                    parts[b].extend(from.iter().copied());
                }
                parts_of_params(&alt.body, nparams, parts);
            }
            parts_of_params(default, nparams, parts);
        }
        e => e.for_each_child(&mut |child| parts_of_params(child, nparams, parts)),
    }
}

/// What a parameter's use requires of it.
#[derive(Clone, Copy, Debug)]
enum Need {
    /// The use owns it.
    Own,
    /// It is passed to this code's parameter, so it may stay borrowed only
    /// while that parameter does.
    Borrowed(usize),
}

/// Whether `prim` only reads its operand `i`, so the operand may be borrowed.
/// One it stores, or may update in place, must be owned.
/// See Note [Borrowed parameters].
fn reads_operand(prim: Prim, i: usize) -> bool {
    use Prim::*;
    match prim {
        Not | Neg | Iadd | Isub | Imul | Idiv | Imod | Udiv | Umod | Fadd | Fsub | Fmul | Fdiv | Fmod | Fpow
        | F32add | F32sub | F32mul | F32div | F32mod | F32pow | Ieq | Ilt | Ult | Feq | Flt | Seq | Slt
        | Swrap | Uwrap | Eq | Lt | Gt | Le | Ge | Concat | Acat | ArrayAlloc | ArrayLen | ArrayGet
        | ArraySlice | ArrayScan | ArrayMatch | PtrBytes | RecordWithout | VecNew | VecLen | VecGet | VecSlice | TensorLength | TensorStack
        | TensorTranspose | TensorConcat | TensorIndex | TensorSlice | TensorIndexAxis | TensorSliceAxis
        | Lex | TokenKind | TokenText | Parse | Unparse | ParseStr | ParseItems | ParseType | ParseRow | Format | TypeKind | TypeFields
        | TypeVariants | TypeParams | TypeOf | Lookup | Scope | Here | Check | Fail | Warn | Iand | Ior | Ixor | Inot | Ishl | Ishr | Ushr | F64Bits | BitsF64
        | F32Bits | BitsF32 => true,
        ArrayPush | ArraySet => i > 0,
        VecFill => i == 0,
        VecPush | VecSet | TensorCreate | TensorDot | TensorMatmul | Delay | Par | Spawn | Await | ChanNew | ChanSend | ChanRecv
        | TvarNew | TvarRead | StmBegin | StmRead | StmWrite | StmCommit | StmWait | StmSave | StmRollback
        | Eval | Abort | Emit | Fresh | Link | LinkPath | ImportPath | SetOption | ShellQuit | ShellReset
        | ShellList => false,
    }
}

// ## Known callees

/// What an application is known to call.
#[derive(Clone, Copy, Debug)]
enum Callee {
    /// A closure of this code: the argument becomes its parameter 0.
    Code(usize),
    /// A builtin holding this many operands already: the argument is the next.
    Prim(Prim, usize),
}

/// Resolves a global name the way the engines do (`machine::glob`): an exact
/// canonical global, then a builtin, a `TARGET.` member, an effect operation,
/// and last the bare last segment.
struct Resolver {
    globals: HashMap<StrId, usize>,
    bare: HashMap<String, usize>,
    ops: Vec<(StrId, StrId)>,
    /// Per code: the code of the closure its body returns outright, if any.
    /// A global function's CAF returns its lambda; a curried function's outer
    /// lambda returns its inner one.
    returns: Vec<Option<usize>>,
}

impl Resolver {
    fn new(program: &IrProgram) -> Resolver {
        let mut globals = HashMap::new();
        let mut bare = HashMap::new();
        for (name, code) in &program.globals {
            globals.entry(*name).or_insert(*code);
            let text = name.as_str();
            bare.entry(text.rsplit('.').next().unwrap_or(text).to_string()).or_insert(*code);
        }
        let returns = program
            .codes
            .iter()
            .map(|code| match &code.body {
                IrExpr::Ret(Atom::Clos { code, .. }) => Some(*code),
                _ => None,
            })
            .collect();
        Resolver {
            globals,
            bare,
            ops: program.effects.iter().map(|e| (e.effect, e.op)).collect(),
            returns,
        }
    }

    /// The value a global name holds: a CAF's code, or a builtin. `None` for
    /// anything an engine builds afresh at each reference.
    fn glob(&self, name: StrId) -> Option<Callee> {
        if let Some(&code) = self.globals.get(&name) {
            return Some(Callee::Code(code));
        }
        if let Some(b) = builtin(name.as_str()) {
            return Some(Callee::Prim(b.prim, 0));
        }
        let text = name.as_str();
        if text.starts_with("TARGET.") {
            return None;
        }
        let is_op = match text.split_once('.') {
            Some((eff, op)) if self.ops.iter().any(|(e, o)| e.as_str() == eff && o.as_str() == op) => true,
            _ => self.ops.iter().any(|(_, o)| *o == name),
        };
        if is_op {
            return None;
        }
        self.bare.get(text).map(|&code| Callee::Code(code))
    }

    /// What applying `fun` calls. A global names its CAF, whose value is the
    /// closure the CAF returns.
    fn callee(&self, fun: &Atom, known: &[Option<Callee>]) -> Option<Callee> {
        match fun {
            Atom::Glob { name } => match self.glob(*name)? {
                Callee::Code(caf) => self.returns[caf].map(Callee::Code),
                prim => Some(prim),
            },
            Atom::Local(s) => known.get(*s).copied().flatten(),
            Atom::Clos { code, .. } => Some(Callee::Code(*code)),
            _ => None,
        }
    }

    /// What a `let` binds when its right-hand side is `rhs`, as a callee.
    fn binds(&self, rhs: &IrExpr, known: &[Option<Callee>]) -> Option<Callee> {
        match rhs {
            IrExpr::Ret(a) => self.callee(a, known),
            IrExpr::App { fun, .. } => match self.callee(fun, known)? {
                Callee::Code(d) => self.returns[d].map(Callee::Code),
                Callee::Prim(prim, n) if n + 1 < prim.builtin().arity => Some(Callee::Prim(prim, n + 1)),
                Callee::Prim(..) => None,
            },
            _ => None,
        }
    }
}

// ## The walk

/// Where an atom is read.
#[derive(Clone, Copy, Debug)]
struct Position {
    /// The read is part of a tail call: the activation ends before the callee
    /// returns. A handler or `defer` body counts as tail, since its last call
    /// may end the activation that installed it.
    tail: bool,
    /// The read is inside a handler body.
    handled: bool,
}

impl Position {
    const TAIL: Position = Position { tail: true, handled: false };
}

/// How an atom is read.
enum How<'a> {
    Borrow,
    Own,
    /// The argument of an application, with what it calls and its lending flag.
    Arg { callee: Option<Callee>, lend: &'a mut bool },
}

/// Report every atom read in `e` to `f`, with how and where it is read.
/// `known` maps each local slot to the callee its `let` bound, if known.
fn walk(
    e: &mut IrExpr,
    at: Position,
    known: &mut [Option<Callee>],
    resolver: &Resolver,
    f: &mut impl FnMut(&Atom, How<'_>, Position),
) {
    match e {
        IrExpr::Ret(a) => read(a, How::Own, at, f),
        IrExpr::App { fun, arg, lend, .. } => {
            let callee = resolver.callee(fun, known);
            read(fun, How::Own, at, f);
            read(arg, How::Arg { callee, lend }, at, f);
        }
        IrExpr::Let { slot, rhs, body } => {
            walk(rhs, Position { tail: false, ..at }, known, resolver, f);
            let bound = resolver.binds(rhs, known);
            let saved = std::mem::replace(&mut known[*slot], bound);
            walk(body, at, known, resolver, f);
            known[*slot] = saved;
        }
        IrExpr::Case { scrut, alts, default } => {
            read(scrut, How::Borrow, at, f);
            for alt in alts {
                let slots = alt.binder_base..alt.binder_base + alt.binders.len();
                let saved: Vec<_> = known[slots.clone()].iter_mut().map(Option::take).collect();
                walk(&mut alt.body, at, known, resolver, f);
                known[slots].copy_from_slice(&saved);
            }
            walk(default, at, known, resolver, f);
        }
        IrExpr::MkStruct { base, fields, .. } => {
            if let Some(b) = base {
                read(b, How::Borrow, at, f);
            }
            for (_, a) in fields {
                read(a, How::Own, at, f);
            }
        }
        IrExpr::Field { rec, .. } => read(rec, How::Borrow, at, f),
        IrExpr::MkVariant { fields: items, .. } | IrExpr::MkTuple(items) => {
            for a in items {
                read(a, How::Own, at, f);
            }
        }
        IrExpr::Handle { body, clauses, els } => {
            for c in clauses {
                read(&c.fun, How::Own, at, f);
            }
            read(els, How::Own, at, f);
            walk(body, Position { tail: true, handled: true }, known, resolver, f);
        }
        IrExpr::Defer { cleanup, body } => {
            read(cleanup, How::Own, at, f);
            walk(body, Position { tail: true, ..at }, known, resolver, f);
        }
        IrExpr::Fault(_) => {}
        IrExpr::Drop { .. } => unreachable!("borrow inference runs before the ownership pass"),
    }
}

/// Report `a`, or for a closure built here, each atom it captures (a capture
/// is a store).
fn read(a: &Atom, how: How<'_>, at: Position, f: &mut impl FnMut(&Atom, How<'_>, Position)) {
    match a {
        Atom::Clos { captures, .. } => {
            for c in captures {
                read(c, How::Own, at, f);
            }
        }
        Atom::Move(_) | Atom::MoveEnv(_) => unreachable!("borrow inference runs before the ownership pass"),
        _ => f(a, how, at),
    }
}

#[cfg(test)]
mod tests;
