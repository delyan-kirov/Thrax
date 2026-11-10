//! The signature: every global the kernel has checked, in declaration order.
//!
//! A global has a name, its level parameters, its type (a closed term over
//! those parameters) and what it is: a transparent definition, an axiom with
//! no body, or one of the three globals an inductive declaration introduces
//! (the type former, its constructors and its recursor). Later globals may
//! refer to earlier ones only, so the signature is append-only and a
//! [`GlobalId`] never dangles.
//!
//! Entry point: [`Signature`].

use std::collections::HashMap;
use std::rc::Rc;

use crate::level::Level;
use crate::term::{Name, Term};

/// A global's position in the signature.
pub type GlobalId = u32;

/// An inductive declaration's position in the signature.
pub type DataId = u32;

#[derive(Clone, Debug)]
pub enum GlobalKind {
    /// Unfolds to its body during evaluation.
    Definition(Rc<Term>),
    /// A postulate: a neutral constant of its type.
    Axiom,
    /// An inductive type former, applied to its parameters.
    Data(DataId),
    /// The `index`th constructor of an inductive type.
    Constructor(DataId, usize),
    /// The recursor (dependent eliminator) of an inductive type.
    Recursor(DataId),
}

/// An inductive type `D (params) : @Type level` and its constructors. Its
/// level parameters are the type former's; the recursor adds one more, the
/// sort of its motive.
#[derive(Clone, Debug)]
pub struct DataDecl {
    pub global: GlobalId,
    pub params: Vec<Param>,
    pub level: Level,
    pub constructors: Vec<ConstructorDecl>,
    pub recursor: GlobalId,
}

/// A parameter or a constructor field: its type is a term over the
/// parameters and fields before it, and `level` is its sort.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: Name,
    pub ty: Rc<Term>,
    pub level: Level,
}

#[derive(Clone, Debug)]
pub struct ConstructorDecl {
    pub name: Name,
    pub global: GlobalId,
    pub fields: Vec<Param>,
    /// Which fields are `D params` itself.
    pub recursive: Vec<bool>,
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
    datas: Vec<DataDecl>,
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

    pub fn data(&self, id: DataId) -> &DataDecl {
        &self.datas[id as usize]
    }

    pub fn data_mut(&mut self, id: DataId) -> &mut DataDecl {
        &mut self.datas[id as usize]
    }

    pub fn add_data(&mut self, decl: DataDecl) -> DataId {
        self.datas.push(decl);
        (self.datas.len() - 1) as DataId
    }

    /// The id the next [`Signature::add`] returns.
    pub fn next_global(&self) -> GlobalId {
        self.globals.len() as GlobalId
    }

    pub fn add(&mut self, global: Global) -> GlobalId {
        assert!(!self.by_name.contains_key(&global.name), "global `{}` is declared twice", global.name);
        let id = self.globals.len() as GlobalId;
        self.by_name.insert(global.name.clone(), id);
        self.globals.push(global);
        id
    }
}
