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
