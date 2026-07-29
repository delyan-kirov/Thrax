//! Handle-based storage: an alternative to the reference-returning bump
//! allocator ([`crate::Arena`]) that lets AST-style trees drop their lifetime
//! parameter entirely.
//!
//! Instead of a child being `&'a Node<'a>`, it is an [`Aol<Node>`] ("arena object
//! lookup"): an opaque, `Copy` handle carrying no lifetime. The data is reachable
//! only through the [`Store`] that produced the handle, so the store is the key:
//! `store.get(id)`. A handle from one store used against another fails the tag
//! check, which is the runtime stand-in for the borrow checker's compile-time
//! guarantee.
//!
//! Why this is sound without generational indices: an AST [`Store`] is
//! append-only. Nodes are pushed while building the tree and freed all at once
//! when the store drops; a slot is never released and reused, so a live handle
//! can never point at a repurposed slot. The per-store tag catches the only
//! remaining misuse, a handle applied to the wrong store.
//!
//! A [`Store`] is backed by a `Vec`, so `get` borrows it locally (that borrow has
//! a lifetime, but an inferred one at the call site, never a parameter on the
//! node types). `push` needs `&mut`, and the borrow checker therefore proves no
//! read is outstanding across a growth that could move the backing buffer.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Distinguishes stores so a handle carries the identity of its origin. Starts at
/// 1 so a zero tag is never a valid store (useful as a niche/sentinel).
static NEXT_TAG: AtomicUsize = AtomicUsize::new(1);

fn fresh_tag() -> usize {
    NEXT_TAG.fetch_add(1, Ordering::Relaxed)
}

/// A handle to one `T` stored in a [`Store`]: a `tag` (which store) and an
/// `index` (which slot). `Copy` and free of any lifetime. The
/// `PhantomData<fn() -> T>` makes the handle typed (an `Aol<Expr>` cannot be
/// passed where an `Aol<Ty>` is expected) without imposing `T`'s auto-traits or
/// variance on the handle.
pub struct Aol<T> {
    tag: usize,
    index: usize,
    _marker: PhantomData<fn() -> T>,
}

// Hand-written so the impls do not gain a spurious `T: Trait` bound (the derives
// would add one even though the handle stores no `T`).
impl<T> Clone for Aol<T> {
    fn clone(&self) -> Aol<T> {
        *self
    }
}
impl<T> Copy for Aol<T> {}
impl<T> PartialEq for Aol<T> {
    fn eq(&self, other: &Aol<T>) -> bool {
        self.tag == other.tag && self.index == other.index
    }
}
impl<T> Eq for Aol<T> {}
impl<T> Hash for Aol<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.index.hash(state);
    }
}
impl<T> fmt::Debug for Aol<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Aol({}#{})", self.index, self.tag)
    }
}

impl<T> Aol<T> {
    /// The store-local index, exposed for parallel [`SecondaryMap`]-style tables.
    pub fn index(self) -> usize {
        self.index
    }
}

/// A contiguous run of `T`s in a [`Store`], the handle form of `&[T]`. Building
/// one appends the elements to the store's backing buffer, so a slice's elements
/// live next to each other and are read back with [`Store::slice`].
pub struct Slice<T> {
    tag: usize,
    start: usize,
    len: usize,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Slice<T> {
    fn clone(&self) -> Slice<T> {
        *self
    }
}
impl<T> Copy for Slice<T> {}
impl<T> PartialEq for Slice<T> {
    fn eq(&self, other: &Slice<T>) -> bool {
        self.tag == other.tag && self.start == other.start && self.len == other.len
    }
}
impl<T> Eq for Slice<T> {}
impl<T> fmt::Debug for Slice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Slice({}..{}#{})",
            self.start,
            self.start + self.len,
            self.tag
        )
    }
}

impl<T> Slice<T> {
    pub fn len(self) -> usize {
        self.len
    }
    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// An append-only, `Vec`-backed store of `T`s addressed by [`Aol`] handles.
pub struct Store<T> {
    tag: usize,
    items: Vec<T>,
}

impl<T> Default for Store<T> {
    fn default() -> Store<T> {
        Store::new()
    }
}

impl<T> Store<T> {
    pub fn new() -> Store<T> {
        Store {
            tag: fresh_tag(),
            items: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Store `val` and return its handle (the table-insert half of the lookup
    /// analogy: `create` puts a row in, `lookup` reads it back).
    pub fn create(&mut self, val: T) -> Aol<T> {
        let index = self.items.len();
        self.items.push(val);
        Aol {
            tag: self.tag,
            index,
            _marker: PhantomData,
        }
    }

    /// Store a run of values contiguously and return a slice handle.
    pub fn create_slice(&mut self, vals: impl IntoIterator<Item = T>) -> Slice<T> {
        let start = self.items.len();
        self.items.extend(vals);
        Slice {
            tag: self.tag,
            start,
            len: self.items.len() - start,
            _marker: PhantomData,
        }
    }

    /// Read a handle's value, panicking if it did not come from this store.
    pub fn lookup(&self, id: Aol<T>) -> &T {
        self.check(id.tag);
        &self.items[id.index]
    }

    pub fn lookup_mut(&mut self, id: Aol<T>) -> &mut T {
        self.check(id.tag);
        &mut self.items[id.index]
    }

    /// Overwrite a handle's value in place (genuine tree rewrites; annotations
    /// usually belong in a [`SecondaryMap`] instead).
    pub fn commit(&mut self, id: Aol<T>, val: T) {
        self.check(id.tag);
        self.items[id.index] = val;
    }

    /// Read a slice handle's elements.
    pub fn lookup_slice(&self, s: Slice<T>) -> &[T] {
        self.check(s.tag);
        &self.items[s.start..s.start + s.len]
    }

    fn check(&self, tag: usize) {
        assert_eq!(
            tag, self.tag,
            "handle used with the wrong store (tag {tag} vs {})",
            self.tag
        );
    }
}

/// A parallel table keyed by an [`Aol`]'s index: the "write annotations onto
/// nodes without mutating them" structure (Cranelift's `SecondaryMap`). The
/// checker records, say, a resolved overload for an expression handle here,
/// leaving the [`Store`] read-only.
pub struct SecondaryMap<T, V> {
    entries: Vec<Option<V>>,
    _marker: PhantomData<fn() -> T>,
}

impl<T, V> Default for SecondaryMap<T, V> {
    fn default() -> SecondaryMap<T, V> {
        SecondaryMap::new()
    }
}

impl<T, V> SecondaryMap<T, V> {
    pub fn new() -> SecondaryMap<T, V> {
        SecondaryMap {
            entries: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn insert(&mut self, id: Aol<T>, value: V) {
        let i = id.index;
        if i >= self.entries.len() {
            self.entries.resize_with(i + 1, || None);
        }
        self.entries[i] = Some(value);
    }

    pub fn get(&self, id: Aol<T>) -> Option<&V> {
        self.entries.get(id.index).and_then(Option::as_ref)
    }
}

/// Identifier of an interned byte string, the handle form of `&str` / `&[u8]`.
/// An `(offset, len)` pair into an [`Interner`]'s buffer: `offset` and `len` are
/// indices, not a borrowed slice, so a `StrId` carries no lifetime and the AST it
/// sits in loses its source lifetime too. The pair slices the buffer directly.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StrId {
    offset: usize,
    len: usize,
}

/// A string interner: one growable byte buffer plus a dedup index, so equal
/// identifiers share a single [`StrId`].
#[derive(Default)]
pub struct Interner {
    buf: Vec<u8>,
    dedup: HashMap<Box<[u8]>, StrId>,
}

impl Interner {
    pub fn new() -> Interner {
        Interner::default()
    }

    /// Intern UTF-8 text, returning a handle (deduplicated).
    pub fn intern(&mut self, s: &str) -> StrId {
        self.intern_bytes(s.as_bytes())
    }

    /// Intern raw bytes (string literals are byte vectors, not necessarily UTF-8).
    pub fn intern_bytes(&mut self, bytes: &[u8]) -> StrId {
        if let Some(&id) = self.dedup.get(bytes) {
            return id;
        }
        let id = StrId {
            offset: self.buf.len(),
            len: bytes.len(),
        };
        self.buf.extend_from_slice(bytes);
        self.dedup.insert(bytes.into(), id);
        id
    }

    pub fn bytes(&self, id: StrId) -> &[u8] {
        &self.buf[id.offset..id.offset + id.len]
    }

    /// Resolve to `&str`. The interned text came in as UTF-8 via [`Interner::intern`];
    /// callers that interned raw bytes should use [`Interner::bytes`].
    pub fn resolve(&self, id: StrId) -> &str {
        std::str::from_utf8(self.bytes(id)).expect("interned as UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrips_handles() {
        let mut s: Store<u64> = Store::new();
        let a = s.create(10);
        let b = s.create(20);
        assert_eq!(*s.lookup(a), 10);
        assert_eq!(*s.lookup(b), 20);
        s.commit(a, 11);
        assert_eq!(*s.lookup(a), 11);
    }

    #[test]
    fn slices_are_contiguous() {
        let mut s: Store<i32> = Store::new();
        let head = s.create(0);
        let run = s.create_slice([1, 2, 3]);
        assert_eq!(s.lookup_slice(run), &[1, 2, 3]);
        assert_eq!(*s.lookup(head), 0);
        assert_eq!(run.len(), 3);
    }

    #[test]
    #[should_panic(expected = "wrong store")]
    fn wrong_store_is_caught() {
        let mut a: Store<u8> = Store::new();
        let b: Store<u8> = Store::new();
        let id = a.create(1);
        // `id` belongs to `a`; using it against `b` trips the tag check.
        let _ = b.lookup(id);
    }

    #[test]
    fn secondary_map_annotates_by_handle() {
        let mut s: Store<&str> = Store::new();
        let x = s.create("x");
        let y = s.create("y");
        let mut ann: SecondaryMap<&str, i32> = SecondaryMap::new();
        ann.insert(y, 42);
        assert_eq!(ann.get(x), None);
        assert_eq!(ann.get(y), Some(&42));
    }

    #[test]
    fn interner_dedups() {
        let mut i = Interner::new();
        let a = i.intern("hello");
        let b = i.intern("hello");
        let c = i.intern("world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "hello");
        assert_eq!(i.resolve(c), "world");
    }
}
