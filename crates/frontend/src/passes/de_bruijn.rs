//! De Bruijn indexing: every local variable gets the distance to its binder.
//!
//! After match compilation and ANF, every [`Term::Var`] gets its De Bruijn
//! index against the binder stack: `0` marks a global, resolved by module and
//! name; a positive index counts outward through the binders to a local. The
//! indices go in a side table, [`CoreProgram::de_bruijn`], keyed by the
//! variable's node. One node can sit under different binders when a subtree is
//! shared, so the pass gives each occurrence a node of its own. The closure
//! converter reads the table to split a variable into an activation slot or an
//! environment capture, replacing a name search with array indexing.
//!
//! Which nodes bind which names is defined once, by the Core traversal.
//! See Note [Core binder discipline].
//!
//! Input: an A-normal Core program. Output: the same program, with every
//! variable occurrence its own node and indexed.
//!
//! Entry point: [`assign_de_bruijn_indices`].


use utilities::{Aol, SecondaryMap, StrId};

use crate::core::{CoreProgram, Term, Terms};

/// Assign De-Bruijn indices to every variable in each of `program`'s globals. A
/// global's own name is resolved through the global table (index `0`), so each
/// definition starts from an empty binder stack.
pub fn assign_de_bruijn_indices(program: &mut CoreProgram) {
    let mut indices = SecondaryMap::new();
    program.rewrite_globals(|terms, term| assign(terms, term, &mut Vec::new(), &mut indices));
    program.de_bruijn = indices;
}

fn assign(t: &mut Terms, term: Aol<Term>, names: &mut Vec<StrId>, indices: &mut SecondaryMap<Term, usize>) -> Aol<Term> {
    if let Term::Var { module, name } = *t.get(term) {
        let occurrence = t.add(Term::Var { module, name });
        indices.insert(occurrence, if module.is_some() { 0 } else { lookup(names, name) });
        return occurrence;
    }
    t.map_children(term, &mut |t, child, bound| {
        names.extend_from_slice(bound);
        let out = assign(t, child, names, indices);
        names.truncate(names.len() - bound.len());
        out
    })
}

/// The De-Bruijn index of `name` in the binder stack (innermost last): the
/// 1-based distance from the top, or `0` if unbound (a global).
fn lookup(names: &[StrId], name: StrId) -> usize {
    for (i, n) in names.iter().enumerate().rev() {
        if *n == name {
            return names.len() - i;
        }
    }
    0
}

#[cfg(test)]
mod tests;
