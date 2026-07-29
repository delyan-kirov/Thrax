//! A hand-rolled bump (arena) allocator, no external crates.
//!
//! This is the classic *bump allocator*: allocation is a pointer increment, and
//! nothing is freed individually. Memory is handed out from fixed-size
//! [`Block`]s; when a block fills, a fresh one is allocated. [`Arena::reset`]
//! rewinds every block's cursor so the whole region can be reused without
//! re-`malloc`ing, and dropping the arena frees the blocks.
//!
//! It is *dropless*: `T::drop` is never run for allocated values. That is the
//! right trade for a compiler whose AST nodes borrow the source and own no heap
//! resources. Do not arena-allocate a `T` that must run a destructor.
//!
//! # Aliasing soundness
//!
//! Each block's storage is a *separate* heap allocation held behind a
//! [`NonNull`], so growing the internal `Vec<Block>` never moves live bytes:
//! references handed out earlier stay valid. Every allocation returns a fresh,
//! non-overlapping region, so returning `&mut T` from `&self` cannot alias.

use std::alloc::{self, Layout};
use std::cell::UnsafeCell;
use std::ptr::NonNull;
use std::{mem, ptr, slice, str};

pub mod handle;
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};

/// Default block capacity in bytes (matches the C++ arena's 1 KiB block).
const BLOCK_DEFAULT: usize = 1 << 10;

/// Alignment every block base is guaranteed to satisfy. Individual allocations
/// may request up to this; a stronger alignment trips an assert (see
/// [`Arena::raw_alloc`]).
const MAX_ALIGN: usize = 16;

/// A single contiguous run of bytes with a bump cursor.
struct Block {
    base: NonNull<u8>,
    cap: usize,
    len: usize,
}

impl Block {
    fn new(min_cap: usize) -> Block {
        let cap = min_cap.max(BLOCK_DEFAULT);
        let layout = Layout::from_size_align(cap, MAX_ALIGN).expect("valid block layout");
        // SAFETY: cap > 0, so the layout is non-zero-sized.
        let raw = unsafe { alloc::alloc(layout) };
        let base = NonNull::new(raw).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Block { base, cap, len: 0 }
    }

    /// Try to carve `size` bytes at `align` out of this block's remaining space.
    fn try_bump(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        let start = self.base.as_ptr() as usize;
        let cursor = start + self.len;
        let aligned = (cursor + align - 1) & !(align - 1);
        let new_len = (aligned - start).checked_add(size)?;
        if new_len <= self.cap {
            self.len = new_len;
            Some(aligned as *mut u8)
        } else {
            None
        }
    }

    fn layout(&self) -> Layout {
        Layout::from_size_align(self.cap, MAX_ALIGN).expect("block layout was valid at creation")
    }
}

/// Interior state, mutated through `&self` via the [`UnsafeCell`].
struct State {
    /// Blocks before `active` are full; `active` is the current fill target;
    /// blocks after `active` are empty spares left behind by [`Arena::reset`].
    blocks: Vec<Block>,
    active: usize,
}

/// A bump allocator. Allocate with `&self`; the returned references live as long
/// as the borrow of the arena, so allocation can continue freely.
pub struct Arena {
    state: UnsafeCell<State>,
}

impl Default for Arena {
    fn default() -> Arena {
        Arena::new()
    }
}

impl Arena {
    pub fn new() -> Arena {
        Arena {
            state: UnsafeCell::new(State {
                blocks: Vec::new(),
                active: 0,
            }),
        }
    }

    /// The core primitive: hand back `size` aligned bytes.
    fn raw_alloc(&self, size: usize, align: usize) -> *mut u8 {
        debug_assert!(align.is_power_of_two(), "alignment must be a power of two");
        assert!(
            align <= MAX_ALIGN,
            "arena guarantees alignment up to {MAX_ALIGN}, got {align}"
        );

        // Zero-sized requests never touch a block; a dangling-but-aligned pointer
        // is all a ZST read/write needs.
        if size == 0 {
            return align as *mut u8;
        }

        // SAFETY: no `&mut State` outlives this function, and no returned byte
        // pointer aliases the `State` struct itself.
        let state = unsafe { &mut *self.state.get() };

        loop {
            if state.active < state.blocks.len() {
                if let Some(p) = state.blocks[state.active].try_bump(size, align) {
                    return p;
                }
                // Current block is full; a spare may follow it after a reset.
                if state.active + 1 < state.blocks.len() {
                    state.active += 1;
                    continue;
                }
            }
            // Need a brand-new block. Size it to hold this request outright.
            let mut block = Block::new(size + align);
            let p = block
                .try_bump(size, align)
                .expect("a freshly sized block must satisfy the request");
            state.blocks.push(block);
            state.active = state.blocks.len() - 1;
            return p;
        }
    }

    /// Move `val` into the arena and borrow it for the arena's lifetime.
    ///
    /// Returns `&mut` from `&self` on purpose (the typed-arena pattern): every
    /// allocation is a fresh, non-overlapping region, so the mutable borrow
    /// cannot alias another allocation.
    #[allow(clippy::mut_from_ref)]
    pub fn alloc<T>(&self, val: T) -> &mut T {
        let ptr = self.raw_alloc(mem::size_of::<T>(), mem::align_of::<T>()) as *mut T;
        // SAFETY: `ptr` is a fresh, aligned, non-overlapping region for one `T`.
        unsafe {
            ptr::write(ptr, val);
            &mut *ptr
        }
    }

    /// Copy `s` into the arena and borrow it as a `str`.
    pub fn alloc_str(&self, s: &str) -> &str {
        let bytes = s.as_bytes();
        let ptr = self.raw_alloc(bytes.len(), 1);
        // SAFETY: `ptr` owns `bytes.len()` fresh bytes; copying valid UTF-8 in
        // keeps it valid UTF-8.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            str::from_utf8_unchecked(slice::from_raw_parts(ptr, bytes.len()))
        }
    }

    /// Copy a `Copy` slice into the arena and borrow it.
    #[allow(clippy::mut_from_ref)] // fresh non-overlapping region; see `alloc`
    pub fn alloc_slice_copy<T: Copy>(&self, src: &[T]) -> &mut [T] {
        let bytes = mem::size_of::<T>()
            .checked_mul(src.len())
            .expect("slice byte size overflows usize");
        let ptr = self.raw_alloc(bytes, mem::align_of::<T>()) as *mut T;
        // SAFETY: `ptr` owns `src.len()` fresh, aligned `T` slots; `T: Copy`, so
        // a bytewise copy is a valid initialization.
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), ptr, src.len());
            slice::from_raw_parts_mut(ptr, src.len())
        }
    }

    /// Rewind every block so all handed-out bytes can be reused. Takes `&mut
    /// self`: the borrow checker thus proves no allocation is still borrowed,
    /// which is exactly the safety condition for reuse.
    pub fn reset(&mut self) {
        let state = self.state.get_mut();
        for block in &mut state.blocks {
            block.len = 0;
        }
        state.active = 0;
    }

    /// Total bytes currently reserved across all blocks (for tests/introspection).
    pub fn reserved_bytes(&self) -> usize {
        // SAFETY: shared read, no outstanding `&mut State`.
        let state = unsafe { &*self.state.get() };
        state.blocks.iter().map(|b| b.cap).sum()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        for block in &state.blocks {
            // SAFETY: each block base came from `alloc::alloc` with this layout
            // and is freed exactly once here.
            unsafe {
                alloc::dealloc(block.base.as_ptr(), block.layout());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_returns_distinct_writable_regions() {
        let arena = Arena::new();
        let a = arena.alloc(1u64);
        let b = arena.alloc(2u64);
        *a += 40;
        *b += 40;
        assert_eq!(*a, 41);
        assert_eq!(*b, 42);
        assert!(!std::ptr::eq(a, b));
    }

    #[test]
    fn many_allocations_survive_block_growth() {
        let arena = Arena::new();
        let refs: Vec<&mut usize> = (0..10_000).map(|i| arena.alloc(i)).collect();
        // Earlier references must still be valid after many later blocks.
        for (i, r) in refs.iter().enumerate() {
            assert_eq!(**r, i);
        }
        assert!(arena.reserved_bytes() >= 10_000 * std::mem::size_of::<usize>());
    }

    #[test]
    fn alloc_str_and_slice_copy() {
        let arena = Arena::new();
        let s = arena.alloc_str("thrax");
        let xs = arena.alloc_slice_copy(&[1u8, 2, 3]);
        assert_eq!(s, "thrax");
        assert_eq!(xs, &[1, 2, 3]);
    }

    #[test]
    fn reset_reuses_blocks_without_growing() {
        let mut arena = Arena::new();
        for i in 0..2_000usize {
            arena.alloc(i);
        }
        let before = arena.reserved_bytes();
        arena.reset();
        for i in 0..2_000usize {
            arena.alloc(i);
        }
        assert_eq!(
            arena.reserved_bytes(),
            before,
            "reset must reuse existing blocks"
        );
    }

    #[test]
    fn honors_large_alignment_up_to_max() {
        #[repr(align(16))]
        struct Aligned16(u128);
        let arena = Arena::new();
        let p = arena.alloc(Aligned16(7));
        assert_eq!((p as *const _ as usize) % 16, 0);
        assert_eq!(p.0, 7);
    }

    #[test]
    fn zero_sized_types_do_not_allocate() {
        let arena = Arena::new();
        let _u: &mut () = arena.alloc(());
        assert_eq!(arena.reserved_bytes(), 0);
    }
}
