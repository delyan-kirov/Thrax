//! The signature: every global the kernel has checked, in declaration order.
//!
//! A global has a name, its level parameters, its type (a closed term over
//! those parameters) and what it is: a transparent definition, or an axiom
//! with no body. Later globals may refer to earlier ones only, so the
//! signature is append-only and a [`GlobalId`] never dangles.
//!
//! Entry point: [`Signature`].

use std::collections::HashMap;
use std::rc::Rc;

use crate::term::{Name, Term};

/// A global's position in the signature.
pub type GlobalId = u32;

#[derive(Clone, Debug)]
pub enum GlobalKind {
    /// Unfolds to its body during evaluation.
    Definition(Rc<Term>),
    /// A postulate: a neutral constant of its type.
    Axiom,
}

#[derive(Clone, Debug)]
pub struct Global {
    pub name: Name,
    pub level_params: Vec<Name>,
    pub ty: Rc<Term>,
    pub kind: GlobalKind,
}

#[derive(Default)]
pub struct Signature {
    globals: Vec<Global>,
    by_name: HashMap<Name, GlobalId>,
}

impl Signature {
    pub fn new() -> Signature {
        Signature::default()
    }

    pub fn global(&self, id: GlobalId) -> &Global {
        &self.globals[id as usize]
    }

    pub fn lookup(&self, name: &str) -> Option<GlobalId> {
        self.by_name.get(name).copied()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.globals.iter().map(|g| &*g.name)
    }

    pub fn add(&mut self, global: Global) -> GlobalId {
        assert!(!self.by_name.contains_key(&global.name), "global `{}` is declared twice", global.name);
        let id = self.globals.len() as GlobalId;
        self.by_name.insert(global.name.clone(), id);
        self.globals.push(global);
        id
    }
}
