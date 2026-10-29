//! De Bruijn indexing: every local variable gets the distance to its binder.
//!
//! After match compilation and ANF, every [`Term::Var`] gets its De Bruijn
//! index against the binder stack: `0` marks a global, resolved by module and
//! name; a positive index counts outward through the binders to a local. The
//! closure converter reads it to split a variable into an activation slot or
//! an environment capture, replacing a name search with array indexing.
//!
//! Which nodes bind which names is defined once, by the Core traversal.
//! See Note [Core binder discipline].
//!
//! Input: an A-normal Core program. Output: the same program, every `idx`
//! filled.
//!
//! Entry point: [`assign_de_bruijn_indices`].


use crate::core::{CoreProgram, Term};

/// Assign De-Bruijn indices to every variable in each of `program`'s globals. A
/// global's own name is resolved through the global table (index `0`), so each
/// definition starts from an empty binder stack.
pub fn assign_de_bruijn_indices(program: &mut CoreProgram) {
    for (_, term) in program.globals.iter_mut() {
        let mut names = Vec::new();
        *term = assign(term, &mut names);
    }
}

fn assign(t: &Term, names: &mut Vec<String>) -> Term {
    if let Term::Var { module, name, .. } = t {
        return Term::Var {
            module: module.clone(),
            name: name.clone(),
            idx: if module.is_some() { 0 } else { lookup(names, name) },
        };
    }
    t.map_children(&mut |child, bound| {
        names.extend(bound.iter().map(|b| b.to_string()));
        let out = assign(child, names);
        names.truncate(names.len() - bound.len());
        out
    })
}

/// The De-Bruijn index of `name` in the binder stack (innermost last): the
/// 1-based distance from the top, or `0` if unbound (a global).
fn lookup(names: &[String], name: &str) -> usize {
    for (i, n) in names.iter().enumerate().rev() {
        if n == name {
            return names.len() - i;
        }
    }
    0
}

#[cfg(test)]
mod tests;
