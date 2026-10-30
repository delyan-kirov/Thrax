//! Handle-based storage, which lets AST-style trees drop their lifetime
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
use std::sync::Mutex;

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
/// live next to each other and are read back with [`Store::lookup_slice`].
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

/// A clone keeps the original's tag: its handles address the clone too, which
/// is what lets an editor extend a copy of an already-checked tree.
impl<T: Clone> Clone for Store<T> {
    fn clone(&self) -> Store<T> {
        Store {
            tag: self.tag,
            items: self.items.clone(),
        }
    }
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

    /// The handle of the value at `index` (for walking a run of the store).
    pub fn handle(&self, index: usize) -> Aol<T> {
        assert!(index < self.items.len(), "handle index {index} out of range");
        Aol {
            tag: self.tag,
            index,
            _marker: PhantomData,
        }
    }

    /// Drop every value from `len` on. Sound only when no handle at or past
    /// `len` is used again: the arena-reset discipline, for a store whose tail
    /// belongs to a computation that is over.
    pub fn truncate(&mut self, len: usize) {
        self.items.truncate(len);
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

impl<T, V: Clone> Clone for SecondaryMap<T, V> {
    fn clone(&self) -> SecondaryMap<T, V> {
        SecondaryMap {
            entries: self.entries.clone(),
            _marker: PhantomData,
        }
    }
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

// Note [One process-wide interner]
//
// Every name the compiler handles, from the AST through the checker and the
// Core to the IR, is a `StrId` into one interner that lives as long as the
// process. One table means a `StrId` means the same text everywhere: a type
// constructor the checker builds from an AST name reuses the AST's handle, and
// a backend reads an IR name without being handed a table to read it from.
//
// The text is never freed. Interned bytes go into chunks that are leaked, so a
// lookup returns `&'static str` with no `unsafe`. Interning takes a lock;
// reading usually does not, since each thread mirrors the append-only table.
// Nothing is lost by never freeing: a compiler interns a bounded vocabulary,
// and an editor that rechecks a file on every keystroke interns only the
// strings it has not seen before.

/// An interned byte string, the handle form of `&str` / `&[u8]`: an index into
/// the process-wide interner, `Copy` and free of any lifetime. Equal text has
/// an equal `StrId`. See Note [One process-wide interner].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(u32);

/// The size of the first chunk; each later chunk doubles, up to [`CHUNK_MAX`].
const CHUNK_MIN: usize = 4 << 10;
const CHUNK_MAX: usize = 1 << 20;

#[derive(Default)]
struct Strings {
    texts: Vec<&'static [u8]>,
    dedup: HashMap<&'static [u8], StrId>,
    /// The unused tail of the current chunk.
    free: &'static mut [u8],
    chunk: usize,
}

static STRINGS: Mutex<Option<Strings>> = Mutex::new(None);

thread_local! {
    /// This thread's copy of a prefix of `STRINGS.texts`, so reading a `StrId`
    /// takes the lock only for a string interned since the thread last looked.
    /// Valid because the table is append-only and its entries are `'static`.
    static SEEN: std::cell::RefCell<Vec<&'static [u8]>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn with_strings<R>(f: impl FnOnce(&mut Strings) -> R) -> R {
    let mut guard = STRINGS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(Strings::default))
}

impl Strings {
    fn store(&mut self, bytes: &[u8]) -> &'static [u8] {
        if bytes.len() > self.free.len() {
            self.chunk = (self.chunk * 2).clamp(CHUNK_MIN, CHUNK_MAX);
            self.free = vec![0u8; self.chunk.max(bytes.len())].leak();
        }
        let (head, tail) = std::mem::take(&mut self.free).split_at_mut(bytes.len());
        head.copy_from_slice(bytes);
        self.free = tail;
        head
    }
}

impl StrId {
    /// Intern UTF-8 text, returning its handle (deduplicated).
    pub fn intern(s: &str) -> StrId {
        StrId::intern_bytes(s.as_bytes())
    }

    /// Intern raw bytes (string literals are byte vectors, not necessarily UTF-8).
    pub fn intern_bytes(bytes: &[u8]) -> StrId {
        with_strings(|t| {
            if let Some(&id) = t.dedup.get(bytes) {
                return id;
            }
            let id = StrId(u32::try_from(t.texts.len()).expect("fewer than 2^32 interned strings"));
            let stored = t.store(bytes);
            t.texts.push(stored);
            t.dedup.insert(stored, id);
            id
        })
    }

    pub fn bytes(self) -> &'static [u8] {
        let i = self.0 as usize;
        SEEN.with(|seen| {
            if let Some(&text) = seen.borrow().get(i) {
                return text;
            }
            let mut seen = seen.borrow_mut();
            let known = seen.len();
            with_strings(|t| seen.extend_from_slice(&t.texts[known..]));
            seen[i]
        })
    }

    /// The text. Text interned with [`StrId::intern`] is UTF-8; read raw bytes
    /// with [`StrId::bytes`].
    pub fn as_str(self) -> &'static str {
        std::str::from_utf8(self.bytes()).expect("interned as UTF-8")
    }
}

/// Interns the text.
impl From<&str> for StrId {
    fn from(s: &str) -> StrId {
        StrId::intern(s)
    }
}

impl From<&String> for StrId {
    fn from(s: &String) -> StrId {
        StrId::intern(s)
    }
}

impl From<String> for StrId {
    fn from(s: String) -> StrId {
        StrId::intern(&s)
    }
}

impl PartialEq<str> for StrId {
    fn eq(&self, other: &str) -> bool {
        self.bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for StrId {
    fn eq(&self, other: &&str) -> bool {
        self.bytes() == other.as_bytes()
    }
}

/// Prints as the text it stands for, quoted like a `String`.
impl fmt::Debug for StrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match std::str::from_utf8(self.bytes()) {
            Ok(s) => write!(f, "{s:?}"),
            Err(_) => write!(f, "{:?}", self.bytes()),
        }
    }
}

impl fmt::Display for StrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(self.bytes()))
    }
}

#[cfg(test)]
mod tests;
