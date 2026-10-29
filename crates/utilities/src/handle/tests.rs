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
