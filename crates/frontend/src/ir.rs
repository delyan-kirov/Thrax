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
//! Entry point: [`lower_modules`].

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
    passes::eta_fold::fold_primitive_methods(&mut merged);
    passes::match_compilation::compile_matches(&mut merged);
    passes::a_normal_form::to_a_normal_form(&mut merged);
    passes::de_bruijn::assign_de_bruijn_indices(&mut merged);
    lower(&merged)
}

/// Closure-convert a Core program that has been through the passes, then mark
/// its moves and drops.
pub fn lower(core: &CoreProgram) -> IrProgram {
    let mut program = closure_conversion::convert_closures(core);
    ownership::insert_moves_and_drops(&mut program.codes);
    program
}
