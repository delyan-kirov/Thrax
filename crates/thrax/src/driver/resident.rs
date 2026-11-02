//! The checked modules a process keeps between compiles.
//!
//! A compile type-checks its whole import closure, yet most of that closure is
//! the same from one compile to the next: the standard library under every
//! shell entry, every `@eval` fragment and every playground run, and every
//! module a metaprogram-expansion round did not splice. So each module that
//! checks cleanly is kept with its checker, and a later compile reuses it when
//! its source is the same and every module it imported from was itself
//! reused. Only the rest is parsed and checked.
//!
//! The kept checkers share one tree and one type store. A compile parses its
//! new modules onto a copy of the tree (the copy's stores keep their tags, so
//! every handle into the original resolves the same in it) and checks them,
//! importing from the kept checkers. When the [`ModuleRun`] ends it keeps what
//! checked cleanly: the copy becomes the shared tree and every kept checker is
//! rebound to it. The types built for modules that are not kept are dropped by
//! rewinding the store to the watermark taken after the last kept module, the
//! same discipline as the editor's base (`lsp::analysis`).
//!
//! Input: the loaded sources of one compile. Output: a checker per module, its
//! definitions' types, and its diagnostics, as [`super::check_modules_over`]
//! reports them.
//!
//! Entry point: [`ModuleRun::check`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::typing::types::TypesMark;
use frontend::core::CoreProgram;
use frontend::{Ast, Checker, Program, Resolved, Type, Types};
use utilities::StrId;

use super::{check_modules_over, import_graph, CheckedOver, ModuleSet, SessionRoot, Setup};

/// One module checked by an earlier compile.
struct Kept {
    /// Unique in the process: what a lowered prefix is keyed by.
    id: u64,
    name: String,
    src: String,
    /// Checked leniently with something deferred: only a lenient check may
    /// reuse it. See Note [A lenient check that deferred nothing is strict].
    deferred: bool,
    /// The modules its checker imported from, `CORE` and `C` included.
    deps: Vec<String>,
    program: Program,
    results: Vec<(StrId, Type)>,
    /// Its Core, once a compile has desugared it.
    /// See Note [A kept module desugars the same].
    lowered: Option<CoreProgram>,
    checker: Checker<'static>,
}

/// Every kept module, over the tree and the type store they share.
///
/// The checkers borrow `ast`, erased to `'static`: the tree is boxed (so it
/// never moves), is never mutated while they exist, and `modules` is declared
/// first so it drops first.
struct Resident {
    modules: Vec<Kept>,
    ast: Box<Ast>,
    types: Rc<Types>,
    /// Kept modules since replaced. Their types stay in the store as garbage,
    /// so past a bound the whole set is dropped and rebuilt.
    stale: usize,
    next_id: u64,
}

impl Resident {
    fn new() -> Resident {
        Resident { modules: Vec::new(), ast: Box::default(), types: Rc::new(Types::new()), stale: 0, next_id: 0 }
    }
}

thread_local! {
    static RESIDENT: RefCell<Option<Resident>> = const { RefCell::new(None) };
}

// Note [A lenient check that deferred nothing is strict]
//
// A lenient (expansion-round) check differs from the strict one only where a
// name, an instance or an overload is missing: the strict check reports it,
// the lenient one defers it. A lenient check that deferred nothing therefore
// took exactly the strict check's path and produced the same checker, so the
// final strict round may reuse it. A strict check that succeeded deferred
// nothing either, so a lenient round may always reuse it.

// Note [A kept module desugars the same]
//
// Desugaring a module reads what its own checker resolved (keyed by its own
// nodes) and the declarations of the modules it imports; a lookup that falls
// back to "any module" only happens for a name the checker did not resolve,
// which a clean module does not have. A kept module is reused only while its
// source and every module it imports are unchanged, so its Core is the same
// in every compile that reuses it and is desugared once. The C layouts each
// module's Core carries are the whole program's, but the IR merges them by
// name across modules, so a kept module's older list loses nothing.

/// One compile's check: the modules it reused and the checkers of the rest.
///
/// The checkers borrow `ast` and the resident's tree, erased to `'static` like
/// the resident's own, and drop first: `fresh` is declared before both.
pub(crate) struct ModuleRun {
    fresh: Vec<Option<Checker<'static>>>,
    resident: Option<Resident>,
    /// The resident's tree with this compile's new modules parsed onto it, or
    /// `None` when every module was reused.
    ast: Option<Box<Ast>>,
    /// Per module, the kept module it reuses.
    reused: Vec<Option<usize>>,
    pub programs: Vec<Program>,
    pub results: Vec<Vec<(StrId, Type)>>,
    pub errors: Vec<Vec<utilities::Diagnostic>>,
    pub warnings: Vec<Vec<utilities::Diagnostic>>,
    /// The order the modules were checked in, dependencies first.
    pub order: Vec<usize>,
    /// The Core of each module this run desugared, for those it keeps.
    lowered: Vec<Option<CoreProgram>>,
    /// The store's watermark right after each module this run checked.
    marks: Vec<Option<TypesMark>>,
    /// The store's watermark before this run checked anything.
    start: TypesMark,
    graph: Vec<Vec<usize>>,
    sources: Vec<(String, String)>,
    lenient: bool,
    /// The session root, which is never kept: its setup is its own.
    session_root: Option<usize>,
}

impl ModuleRun {
    /// Check every module of `sources` (indexed by `index`), reusing the kept
    /// modules that still hold. Fails, rendered, only when a module's `$ @mod`
    /// header is unreadable.
    pub(crate) fn check(
        sources: &[(String, String, String)],
        index: &HashMap<String, usize>,
        lenient: bool,
        session: Option<SessionRoot<'_>>,
    ) -> Result<ModuleRun, String> {
        let mut resident = RESIDENT.with(|r| r.borrow_mut().take()).unwrap_or_else(Resident::new);
        if resident.stale > 2 * resident.modules.len() + 64 {
            resident = Resident::new();
        }
        let session_root = session.and_then(|s| index.get(s.module).copied());
        let reused = reusable(&resident, sources, index, lenient, session_root);

        let mut ast = reused.iter().any(Option::is_none).then(|| Box::new((*resident.ast).clone()));
        let mut programs = Vec::with_capacity(sources.len());
        let mut parse_errors = Vec::with_capacity(sources.len());
        for (i, (_, src_path, src)) in sources.iter().enumerate() {
            if let Some(k) = reused[i] {
                programs.push(resident.modules[k].program.clone());
                parse_errors.push(Vec::new());
                continue;
            }
            let tree = ast.as_mut().expect("a module to parse has a tree to parse onto");
            let (next, program, errs) = frontend::parse_recovering(std::mem::take(&mut **tree), src);
            **tree = next;
            match program {
                Ok(p) => programs.push(p),
                Err(d) => {
                    RESIDENT.with(|r| *r.borrow_mut() = Some(resident));
                    return Err(d.render(src, src_path));
                }
            }
            parse_errors.push(errs);
        }

        // SAFETY: see `ModuleRun`: both trees are boxed and frozen from here on,
        // and the run holds them until its checkers are gone.
        let tree: &'static Ast = match &ast {
            Some(a) => unsafe { &*(a.as_ref() as *const Ast) },
            None => unsafe { &*(resident.ast.as_ref() as *const Ast) },
        };
        let graph = import_graph(tree, &programs, index);
        let types = Rc::clone(&resident.types);
        let start = types.mark();
        let mut marks: Vec<Option<TypesMark>> = (0..sources.len()).map(|_| None).collect();
        let kept: &[Kept] = &resident.modules;
        // SAFETY: as above; the kept checkers live in the resident the run holds.
        let kept: &'static [Kept] = unsafe { &*(kept as *const [Kept]) };
        let modules = ModuleSet { ast: tree, programs: &programs, graph: &graph, sources };
        let setup = Setup { lenient, open_effects: false, record_types: false, probe: false };
        let over: CheckedOver<'static> = check_modules_over(
            modules,
            &types,
            setup,
            session,
            parse_errors,
            &|i| reused[i].map(|k| (&kept[k].checker, kept[k].results.as_slice())),
            &mut |i| marks[i] = Some(types.mark()),
        );
        Ok(ModuleRun {
            fresh: over.checkers,
            resident: Some(resident),
            ast,
            reused,
            programs,
            results: over.results,
            errors: over.errors,
            warnings: over.warnings,
            order: over.order,
            lowered: (0..sources.len()).map(|_| None).collect(),
            marks,
            start,
            graph,
            sources: sources.iter().map(|(n, _, s)| (n.clone(), s.clone())).collect(),
            lenient,
            session_root,
        })
    }

    /// The tree every module's handles resolve in.
    pub(crate) fn ast(&self) -> &Ast {
        match &self.ast {
            Some(a) => a,
            None => &self.resident.as_ref().expect("held until the run ends").ast,
        }
    }

    /// Module `i`'s checker, reused or not.
    pub(crate) fn checker(&self, i: usize) -> &Checker<'_> {
        match self.reused[i] {
            Some(k) => &self.resident.as_ref().expect("held until the run ends").modules[k].checker,
            None => self.fresh[i].as_ref().expect("every module is checked or reused"),
        }
    }

    /// Module `i`'s checker, which this run must have checked itself.
    pub(crate) fn checker_mut(&mut self, i: usize) -> &mut Checker<'static> {
        self.fresh[i].as_mut().expect("only a module this run checked is mutable")
    }

    /// Every module's checker, in module order.
    pub(crate) fn checkers(&self) -> impl Iterator<Item = &Checker<'_>> {
        (0..self.reused.len()).map(|i| self.checker(i))
    }

    /// The Core of each module in `order`, desugared over `resolved` unless a
    /// kept module already has it.
    pub(crate) fn desugar(&mut self, order: &[usize], resolved: &Resolved) -> Vec<CoreProgram> {
        let mut out = Vec::with_capacity(order.len());
        for &i in order {
            if let Some(k) = self.reused[i] {
                if let Some(core) = &self.resident.as_ref().expect("held until the run ends").modules[k].lowered {
                    out.push(core.clone());
                    continue;
                }
            }
            let core = frontend::desugar_program(self.ast(), &self.programs[i], resolved);
            if Some(i) != self.session_root {
                self.lowered[i] = Some(core.clone());
            }
            out.push(core);
        }
        out
    }

    /// Per module in `order`, the id of the kept module it reuses.
    pub(crate) fn kept_ids(&self, order: &[usize]) -> Vec<Option<u64>> {
        let resident = self.resident.as_ref().expect("held until the run ends");
        order.iter().map(|&i| self.reused[i].map(|k| resident.modules[k].id)).collect()
    }

    /// The modules this run checked that may be kept: clean, not the session
    /// root, and importing only from modules that are reused or kept themselves.
    fn keepable(&self) -> Vec<bool> {
        let (c_idx, core_idx) = self.prelude();
        let mut keep = vec![false; self.reused.len()];
        for &i in &self.order {
            if self.reused[i].is_some() || Some(i) == self.session_root {
                continue;
            }
            let clean = self.errors[i].is_empty() && self.warnings[i].is_empty();
            let deps_hold = self.deps(i, c_idx, core_idx).all(|d| self.reused[d].is_some() || keep[d]);
            keep[i] = clean && deps_hold;
        }
        keep
    }

    fn prelude(&self) -> (Option<usize>, Option<usize>) {
        (
            self.sources.iter().position(|(n, _)| n == "C"),
            self.sources.iter().position(|(n, _)| n == "CORE"),
        )
    }

    /// The modules module `i`'s checker imported from: its imports, plus `C`
    /// and `CORE`, which every other module imports implicitly.
    fn deps(&self, i: usize, c_idx: Option<usize>, core_idx: Option<usize>) -> impl Iterator<Item = usize> + '_ {
        let prelude = if Some(i) == c_idx || Some(i) == core_idx { [None, None] } else { [c_idx, core_idx] };
        self.graph[i].iter().copied().chain(prelude.into_iter().flatten())
    }
}

impl Drop for ModuleRun {
    fn drop(&mut self) {
        let Some(mut resident) = self.resident.take() else { return };
        // Before anything is removed: `reused` indexes the modules as they are.
        for (i, core) in self.lowered.iter_mut().enumerate() {
            if let (Some(k), Some(core)) = (self.reused[i], core.take()) {
                resident.modules[k].lowered.get_or_insert(core);
            }
        }
        let keep = self.keepable();
        let (c_idx, core_idx) = self.prelude();
        let last_kept = self.order.iter().rev().find(|&&i| keep[i]).copied();
        let rewind = match last_kept {
            Some(i) => self.marks[i].take().expect("a kept module was checked here"),
            None => std::mem::replace(&mut self.start, resident.types.mark()),
        };
        let mut fresh = std::mem::take(&mut self.fresh);
        if last_kept.is_some() {
            // A kept module replaced by a new check of it goes, and so does
            // every kept module that imported from it: it was checked against
            // the old one.
            let mut gone: HashSet<String> =
                (0..keep.len()).filter(|&i| keep[i]).map(|i| self.sources[i].0.clone()).collect();
            loop {
                let dropped: Vec<String> = resident
                    .modules
                    .iter()
                    .filter(|k| gone.contains(&k.name) || k.deps.iter().any(|d| gone.contains(d)))
                    .map(|k| k.name.clone())
                    .collect();
                if dropped.is_empty() {
                    break;
                }
                resident.stale += dropped.len();
                resident.modules.retain(|k| !dropped.contains(&k.name));
                gone.extend(dropped);
            }
            for i in 0..keep.len() {
                if !keep[i] {
                    continue;
                }
                let deps = self.deps(i, c_idx, core_idx).map(|d| self.sources[d].0.clone()).collect();
                let checker = fresh[i].take().expect("a kept module was checked here");
                resident.next_id += 1;
                resident.modules.push(Kept {
                    id: resident.next_id,
                    name: self.sources[i].0.clone(),
                    src: self.sources[i].1.clone(),
                    deferred: self.lenient && checker.deferred(),
                    deps,
                    program: self.programs[i].clone(),
                    results: std::mem::take(&mut self.results[i]),
                    lowered: self.lowered[i].take(),
                    checker,
                });
            }
            resident.ast = self.ast.take().expect("a kept module was parsed onto this run's tree");
            // SAFETY: see `Resident`: the new tree is boxed, frozen, and owned
            // beside the checkers, and it extends every tree they were checked over.
            let tree: &'static Ast = unsafe { &*(resident.ast.as_ref() as *const Ast) };
            for k in &mut resident.modules {
                k.checker.rebind_ast(tree);
            }
        }
        drop(fresh);
        resident.types.truncate(&rewind);
        RESIDENT.with(|r| *r.borrow_mut() = Some(resident));
    }
}

thread_local! {
    static PREFIX: RefCell<Option<(Vec<u64>, Rc<frontend::ir::LoweredPrefix>)>> = const { RefCell::new(None) };
}

/// Lower `cores` (root first) to the IR, where `kept[i]` is the id of the kept
/// module `cores[i]` is, if it is one. The kept modules are lowered once as a
/// prefix and the rest onto them, so a compile that reuses the same library
/// lowers only its own modules. See Note [Lowering onto a lowered prefix].
pub(crate) fn lower_reusing(cores: &[CoreProgram], kept: &[Option<u64>]) -> frontend::ir::IrProgram {
    assert_eq!(cores.len(), kept.len(), "one id slot per module");
    let ids: Vec<u64> = kept.iter().flatten().copied().collect();
    if ids.is_empty() {
        return frontend::ir::lower_modules(cores);
    }
    let cached = PREFIX.with(|p| p.borrow().as_ref().filter(|(k, _)| *k == ids).map(|(_, l)| Rc::clone(l)));
    let prefix = cached.unwrap_or_else(|| {
        let modules: Vec<CoreProgram> =
            cores.iter().zip(kept).filter(|(_, k)| k.is_some()).map(|(c, _)| c.clone()).collect();
        let lowered = Rc::new(frontend::ir::lower_prefix(&modules));
        PREFIX.with(|p| *p.borrow_mut() = Some((ids, Rc::clone(&lowered))));
        lowered
    });
    let rest: Vec<CoreProgram> = cores.iter().zip(kept).filter(|(_, k)| k.is_none()).map(|(c, _)| c.clone()).collect();
    frontend::ir::lower_onto(&prefix, &rest)
}

/// Per module of `sources`, the kept module it reuses: one of the same name
/// and source, checked strictly enough, every module it imported from reused in
/// turn. The session root is never reused.
fn reusable(
    resident: &Resident,
    sources: &[(String, String, String)],
    index: &HashMap<String, usize>,
    lenient: bool,
    session_root: Option<usize>,
) -> Vec<Option<usize>> {
    let mut reused: Vec<Option<usize>> = sources
        .iter()
        .enumerate()
        .map(|(i, (name, _, src))| {
            if Some(i) == session_root {
                return None;
            }
            resident.modules.iter().position(|k| k.name == *name && k.src == *src && (lenient || !k.deferred))
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..reused.len() {
            let Some(k) = reused[i] else { continue };
            let holds = resident.modules[k]
                .deps
                .iter()
                .all(|d| index.get(d).is_some_and(|&j| reused[j].is_some()));
            if !holds {
                reused[i] = None;
                changed = true;
            }
        }
        if !changed {
            return reused;
        }
    }
}

/// The names of the kept modules, for tests.
#[cfg(test)]
fn kept() -> Vec<String> {
    RESIDENT.with(|r| r.borrow().as_ref().map(|res| res.modules.iter().map(|k| k.name.clone()).collect()).unwrap_or_default())
}

#[cfg(test)]
mod tests;
