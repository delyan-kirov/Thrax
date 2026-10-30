use super::*;

fn f(name: &str) -> String {
    name.to_string()
}

#[test]
fn vector2_two_floats() {
    let l = CLayout::of(vec![(f("x"), CKind::F32), (f("y"), CKind::F32)]);
    assert_eq!(l.size, 8);
    assert_eq!(l.align, 4);
    assert_eq!(l.fields[0].offset, 0);
    assert_eq!(l.fields[1].offset, 4);
}

#[test]
fn color_four_bytes_is_packed() {
    let l = CLayout::of(vec![
        (f("r"), CKind::U8),
        (f("g"), CKind::U8),
        (f("b"), CKind::U8),
        (f("a"), CKind::U8),
    ]);
    assert_eq!(l.size, 4);
    assert_eq!(l.align, 1);
    assert_eq!(
        l.fields.iter().map(|x| x.offset).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn mixed_field_gets_padded() {
    // A `u8` then an 8-byte field: the second aligns to 8, so the struct is
    // 16 bytes with 8-byte alignment (7 bytes of padding after the u8).
    let l = CLayout::of(vec![(f("tag"), CKind::U8), (f("n"), CKind::S64)]);
    assert_eq!(l.fields[0].offset, 0);
    assert_eq!(l.fields[1].offset, 8);
    assert_eq!(l.size, 16);
    assert_eq!(l.align, 8);
}

#[test]
fn union_overlaps_at_zero() {
    // A union of an i32 and a double: all at offset 0, size 8, align 8.
    let l = CLayout::of_union(vec![(f("i"), CKind::S32), (f("d"), CKind::F64)]);
    assert!(l.is_union);
    assert_eq!(l.fields[0].offset, 0);
    assert_eq!(l.fields[1].offset, 0);
    assert_eq!(l.size, 8);
    assert_eq!(l.align, 8);
}

#[test]
fn nested_struct_field() {
    let inner = CLayout::of(vec![(f("x"), CKind::F32), (f("y"), CKind::F32)]);
    let outer = CLayout::of(vec![
        (f("pos"), CKind::Struct("Vec2".into(), inner)),
        (f("id"), CKind::S32),
    ]);
    assert_eq!(outer.fields[0].offset, 0);
    assert_eq!(outer.fields[1].offset, 8);
    assert_eq!(outer.size, 12);
    assert_eq!(outer.align, 4);
}
