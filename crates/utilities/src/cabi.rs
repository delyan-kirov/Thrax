//! The C ABI data model shared by the frontend (which computes a layout from
//! Thrax types) and both engines (which marshal values across the `@extern`
//! boundary). A [`CLayout`] is a struct's flat, unboxed C memory image: the
//! total size, the alignment, and each field's byte offset and leaf kind.
//!
//! Target-dependent leaves (`Int`, `Nat`, `Ptr`) are resolved to fixed-width
//! kinds by the frontend before they reach here, so the layout algorithm and
//! the sizes below are pure data with no platform knowledge.

/// A C scalar leaf, or a nested struct. Fixed-width: the frontend has already
/// lowered `Int`/`Nat`/`Ptr` to the target's concrete word width.
#[derive(Clone, Debug, PartialEq)]
pub enum CKind {
    S8,
    S16,
    S32,
    S64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    /// A nested struct field: its declared type name (so a backend can name the
    /// emitted C type) and its layout.
    Struct(String, CLayout),
}

impl CKind {
    /// The size in bytes.
    pub fn size(&self) -> usize {
        match self {
            CKind::S8 | CKind::U8 => 1,
            CKind::S16 | CKind::U16 => 2,
            CKind::S32 | CKind::U32 | CKind::F32 => 4,
            CKind::S64 | CKind::U64 | CKind::F64 => 8,
            CKind::Struct(_, l) => l.size,
        }
    }

    /// The alignment in bytes. A scalar aligns to its size; a struct to the
    /// maximum alignment of its fields.
    pub fn align(&self) -> usize {
        match self {
            CKind::Struct(_, l) => l.align,
            other => other.size(),
        }
    }
}

/// One field of a C struct: its name, byte offset from the struct start, and
/// kind.
#[derive(Clone, Debug, PartialEq)]
pub struct CField {
    pub name: String,
    pub offset: usize,
    pub kind: CKind,
}

/// How one positional C argument of an `@extern` is sourced from the single
/// Thrax value the extern is applied to. A C function has no first-class
/// closures to curry, so a Thrax extern takes exactly ONE argument: unit (no C
/// arguments), an anonymous record (each field is a positional C argument), or a
/// single value (used directly). A record field is pulled BY NAME so a reordered
/// call site still marshals in the declared C order.
#[derive(Clone, Debug, PartialEq)]
pub enum ExternArg {
    /// The one applied value IS this positional C argument.
    Whole,
    /// Pull this named field from the one applied (record) value.
    Field(String),
    /// Take this positional element of the one applied (tuple) value. A closed
    /// record parameter surfaces as a positional tuple in this language, so a
    /// multi-argument C function reached by position expands its elements here.
    Elem(usize),
}

/// A struct's (or C union's) flat C memory image.
#[derive(Clone, Debug, PartialEq)]
pub struct CLayout {
    pub size: usize,
    pub align: usize,
    pub fields: Vec<CField>,
    /// A C `union`: every member starts at offset 0 and the size is the largest
    /// member. A value carries just one active member when built; a union result
    /// is read by reinterpreting the shared bytes as each member.
    pub is_union: bool,
}

/// Round `n` up to the next multiple of `align` (a power of two).
fn align_up(n: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (n + align - 1) & !(align - 1)
}

impl CLayout {
    /// Lay out `fields` (in declaration order) by the standard C rule: each
    /// field starts at the next offset aligned to its own alignment; the struct
    /// aligns to its largest field and its size is padded up to that alignment.
    pub fn of(fields: Vec<(String, CKind)>) -> CLayout {
        let mut offset = 0usize;
        let mut align = 1usize;
        let mut out = Vec::with_capacity(fields.len());
        for (name, kind) in fields {
            let (fsize, falign) = (kind.size(), kind.align());
            offset = align_up(offset, falign.max(1));
            out.push(CField {
                name,
                offset,
                kind,
            });
            offset += fsize;
            align = align.max(falign);
        }
        let size = align_up(offset, align.max(1));
        CLayout {
            size,
            align,
            fields: out,
            is_union: false,
        }
    }

    /// Lay out `fields` as a C `union`: every member starts at offset 0, the
    /// alignment is the largest member's, and the size is the largest member's
    /// size rounded up to that alignment.
    pub fn of_union(fields: Vec<(String, CKind)>) -> CLayout {
        let mut align = 1usize;
        let mut max = 0usize;
        let out: Vec<CField> = fields
            .into_iter()
            .map(|(name, kind)| {
                align = align.max(kind.align().max(1));
                max = max.max(kind.size());
                CField {
                    name,
                    offset: 0,
                    kind,
                }
            })
            .collect();
        CLayout {
            size: align_up(max, align.max(1)),
            align,
            fields: out,
            is_union: true,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
