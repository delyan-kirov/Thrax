//! The intermediate representation, and the middle end that produces it.
//!
//! [`data`] is the IR itself: closure-converted, A-normal Core with shallow
//! patterns, which the `interpreter`'s abstract machine and the `ccg` backend
//! both consume. [`lower_modules`] is the whole middle end: it merges the
//! desugared modules into one Core program, runs the [`crate::passes`] over it,
//! closure-converts it ([`closure_conversion`]), and marks where each local's
//! reference ends ([`ownership`]).
//!
//! Input: every module's Core, root first. Output: one [`IrProgram`].
//!
//! Entry points: [`lower_modules`], and [`lower_prefix`] with [`lower_onto`]
//! to lower a program's first modules once and the rest onto them.

pub mod closure_conversion;
pub mod data;
pub mod ownership;

pub use data::IrProgram;

use crate::core::{CoreEffect, CoreProgram};
use crate::passes;
use utilities::StrId;

/// Lower a whole program (root module first) to the IR: give every global the
/// canonical key `Module.name`, copy the modules into one Core, run the
/// [`passes`], then closure-convert. A qualified reference resolves to its canonical
/// key; a bare reference resolves through the runtime's bare fallback (see
/// `machine::glob`). Root first, so a bare name collision resolves to the root.
pub fn lower_modules(modules: &[CoreProgram]) -> IrProgram {
    let mut merged = merge(modules);
    passes::eta_fold::fold_primitive_methods(&mut merged);
    run_passes(&mut merged);
    lower(&merged)
}

/// Copy `modules` into one Core program, each global keyed `Module.name`.
fn merge(modules: &[CoreProgram]) -> CoreProgram {
    let mut merged = CoreProgram::new(modules.first().map_or_else(|| StrId::intern(""), |m| m.module));
    for m in modules {
        for e in &m.effects {
            merged.effects.push(CoreEffect {
                effect: e.effect,
                op: e.op,
            });
        }
        for &(name, term) in &m.globals {
            let term = merged.terms.import(&m.terms, term);
            merged
                .globals
                .push((StrId::intern(&format!("{}.{}", m.module, name)), term));
        }
        for (name, layout) in &m.crepr_layouts {
            if !merged.crepr_layouts.iter().any(|(n, _)| n == name) {
                merged.crepr_layouts.push((*name, layout.clone()));
            }
        }
    }
    merged
}

/// The passes after the eta fold, each of which rewrites one global at a time.
fn run_passes(merged: &mut CoreProgram) {
    passes::match_compilation::compile_matches(merged);
    passes::a_normal_form::to_a_normal_form(merged);
    passes::de_bruijn::assign_de_bruijn_indices(merged);
}

// Note [Lowering onto a lowered prefix]
//
// Every pass after merging rewrites one global at a time, so lowering a
// program in two parts gives the same code as lowering it whole, given three
// things the first part must hand the second: the eta-expanded primitive
// methods of its globals (the eta fold reads them by name), the names of its
// globals (closure conversion tells a global from a built-in by them), and its
// code count (code ids are positions in one pool). The first part must not
// name any global of the second, which holds when it is a set of modules
// closed under imports. The shell lowers the standard library once and each
// entry onto it.

/// The IR of the first modules of a program, ready for the rest to be lowered
/// onto it with [`lower_onto`]. See Note [Lowering onto a lowered prefix].
pub struct LoweredPrefix {
    ir: IrProgram,
    prims: passes::eta_fold::Prims,
    names: std::collections::HashSet<StrId>,
}

/// Lower `modules`, which import nothing outside themselves, as a prefix.
pub fn lower_prefix(modules: &[CoreProgram]) -> LoweredPrefix {
    let mut merged = merge(modules);
    let prims = passes::eta_fold::primitive_methods(&merged);
    passes::eta_fold::fold_primitive_methods_with(&mut merged, &prims);
    run_passes(&mut merged);
    let ir = lower(&merged);
    let names = ir.globals.iter().map(|(n, _)| *n).collect();
    LoweredPrefix { ir, prims, names }
}

/// Lower a program whose first modules are `prefix` and whose other modules
/// are `rest` (root first): the same program [`lower_modules`] makes of them
/// all, with the rest's globals ahead, so a bare name still resolves to the root.
pub fn lower_onto(prefix: &LoweredPrefix, rest: &[CoreProgram]) -> IrProgram {
    let mut merged = merge(rest);
    let mut prims = passes::eta_fold::primitive_methods(&merged);
    for (k, v) in &prefix.prims {
        prims.entry(*k).or_insert(*v);
    }
    passes::eta_fold::fold_primitive_methods_with(&mut merged, &prims);
    run_passes(&mut merged);
    let mut own = closure_conversion::convert_closures_after(&merged, prefix.ir.codes.len(), &prefix.names);
    ownership::insert_moves_and_drops(&mut own.codes);
    let mut codes = Vec::with_capacity(prefix.ir.codes.len() + own.codes.len());
    codes.extend(prefix.ir.codes.iter().cloned());
    codes.append(&mut own.codes);
    let mut globals = own.globals;
    globals.extend(prefix.ir.globals.iter().copied());
    let mut effects = own.effects;
    effects.extend(prefix.ir.effects.iter().cloned());
    let mut crepr_layouts = own.crepr_layouts;
    for (name, layout) in &prefix.ir.crepr_layouts {
        if !crepr_layouts.iter().any(|(n, _)| n == name) {
            crepr_layouts.push((*name, layout.clone()));
        }
    }
    IrProgram { codes, globals, effects, crepr_layouts }
}

/// Closure-convert a Core program that has been through the passes, then mark
/// its moves and drops.
pub fn lower(core: &CoreProgram) -> IrProgram {
    let mut program = closure_conversion::convert_closures(core);
    ownership::insert_moves_and_drops(&mut program.codes);
    program
}
