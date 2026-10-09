//! Static exhaustiveness and redundancy checking for `is` matches, after
//! Maranget, "Warnings for pattern matching" (JFP 2007).
//!
//! The checker translates each arm's patterns into a [`DPat`], a pattern over a
//! small constructor algebra: union variants, booleans, the two steps of a
//! sequence view, products, and literals of an infinite domain. [`useful`] then
//! answers "is there a value this vector matches that no earlier row matches?",
//! producing a witness when there is. A match is exhaustive when a wildcard is not
//! useful against its unguarded rows; an arm is unreachable when none of its
//! alternatives is useful against the unguarded rows before it.
//!
//! [`DPat::Opaque`] stands for a refutable pattern the checker does not model
//! (a range, a string prefix, a literal matched through a user type's equality).
//! As a row it covers nothing, and as the vector under test it matches anything,
//! so it can neither make a match look exhaustive nor make a later arm look dead.

use std::rc::Rc;

/// The one union whose variants line up with the sequence view's two steps:
/// CORE's `impl_ISeqView_for_List` views `Nil` as `Empty` and `Cons` as `More`,
/// so its variant and sequence patterns share a column. Any other union's view is
/// an arbitrary function, and mixing the two there is reported instead.
const SEQ_UNION: (&str, &str, &str) = ("List", "Nil", "Cons");

/// A pattern constructor. Two constructors are the same when [`Ctor::same`] says so.
#[derive(Clone, Debug)]
pub enum Ctor {
    /// Variant `index` of `union`. `siblings` lists every variant of the union as
    /// `(tag, payload arity)`, in declaration order.
    Variant {
        union: Rc<str>,
        index: usize,
        siblings: Rc<[(String, usize)]>,
    },
    Bool(bool),
    /// The end of a sequence (`[]`, the view's `Empty`).
    SeqEmpty,
    /// One step of a sequence (`h :: t`, the view's `More`): the element, then the
    /// rest of the sequence.
    SeqMore,
    /// A literal of a type with infinitely many values, keyed by its source form.
    Lit(String),
    /// The integers `lo ..= hi` of a type whose values are `dom.0 ..= dom.1`. A
    /// literal is a one-element range. Columns of ranges are split into disjoint
    /// segments (see [`segments`]) rather than compared by equality.
    Int { lo: i128, hi: i128, dom: (i128, i128) },
    /// The single constructor of a product, its fields in `fields` order.
    Product {
        kind: ProductKind,
        fields: Rc<[String]>,
    },
}

/// How a product pattern was written, for showing a witness.
#[derive(Clone, Debug)]
pub enum ProductKind {
    Struct(Rc<str>),
    Tuple,
    Record,
}

#[derive(Clone, Debug)]
pub enum DPat {
    Wild,
    Opaque,
    Ctor(Ctor, Vec<DPat>),
    /// A product matched by field name; fields it does not list are wildcards.
    /// Turned into a positional [`Ctor::Product`] per column, once every row's
    /// field names are known.
    Product(ProductKind, Vec<(String, DPat)>),
}

impl Ctor {
    fn arity(&self) -> usize {
        match self {
            Ctor::Variant { index, siblings, .. } => siblings[*index].1,
            Ctor::Bool(_) | Ctor::SeqEmpty | Ctor::Lit(_) | Ctor::Int { .. } => 0,
            Ctor::SeqMore => 2,
            Ctor::Product { fields, .. } => fields.len(),
        }
    }

    fn same(&self, other: &Ctor) -> bool {
        match (self, other) {
            (Ctor::Variant { union: u, index: i, .. }, Ctor::Variant { union: v, index: j, .. }) => {
                u == v && i == j
            }
            (Ctor::Bool(a), Ctor::Bool(b)) => a == b,
            (Ctor::SeqEmpty, Ctor::SeqEmpty) | (Ctor::SeqMore, Ctor::SeqMore) => true,
            (Ctor::Lit(a), Ctor::Lit(b)) => a == b,
            (Ctor::Int { lo: a, hi: b, .. }, Ctor::Int { lo: c, hi: d, .. }) => a == c && b == d,
            (Ctor::Product { .. }, Ctor::Product { .. }) => true,
            _ => false,
        }
    }

    /// Whether `self` and `other` construct values of the same type, so they can
    /// share a column.
    fn same_family(&self, other: &Ctor) -> bool {
        match (self, other) {
            (Ctor::Variant { union: u, .. }, Ctor::Variant { union: v, .. }) => u == v,
            (Ctor::Bool(_), Ctor::Bool(_)) | (Ctor::Lit(_), Ctor::Lit(_)) => true,
            (Ctor::Int { .. }, Ctor::Int { .. }) => true,
            (Ctor::SeqEmpty | Ctor::SeqMore, Ctor::SeqEmpty | Ctor::SeqMore) => true,
            (Ctor::Product { .. }, Ctor::Product { .. }) => true,
            _ => false,
        }
    }

    /// Every constructor of this constructor's type, or `None` for an infinite
    /// domain.
    fn signature(&self) -> Option<Vec<Ctor>> {
        Some(match self {
            Ctor::Variant { union, siblings, .. } => (0..siblings.len())
                .map(|index| Ctor::Variant {
                    union: union.clone(),
                    index,
                    siblings: siblings.clone(),
                })
                .collect(),
            Ctor::Bool(_) => vec![Ctor::Bool(true), Ctor::Bool(false)],
            Ctor::SeqEmpty | Ctor::SeqMore => vec![Ctor::SeqEmpty, Ctor::SeqMore],
            Ctor::Product { .. } => vec![self.clone()],
            Ctor::Lit(_) | Ctor::Int { .. } => return None,
        })
    }
}

/// A witness of `v` that no row of `rows` matches, one pattern per column of `v`,
/// or `None` when every value `v` matches is already matched by some row. Every
/// row has `v.len()` columns. When a column mixes variants of a union with
/// sequence steps, the minority family is dropped and `mixed` names the union,
/// since a witness found after that may be one the dropped rows do cover.
fn useful(rows: &[Vec<DPat>], v: &[DPat], mixed: &mut Option<Rc<str>>) -> Option<Vec<DPat>> {
    let Some((head, rest)) = v.split_first() else {
        return rows.is_empty().then(Vec::new);
    };
    let (rows, head) = normalize_column(rows, head, mixed);
    if let Some(dom) = int_domain(&rows, &head) {
        return useful_ints(&rows, &head, rest, dom, mixed);
    }
    match &head {
        DPat::Ctor(c, args) => {
            let spec = specialize(&rows, c);
            let mut vv = args.clone();
            vv.extend_from_slice(rest);
            useful(&spec, &vv, mixed).map(|w| rebuild(c, w))
        }
        DPat::Wild | DPat::Opaque => {
            let heads: Vec<&Ctor> = rows
                .iter()
                .filter_map(|r| match &r[0] {
                    DPat::Ctor(c, _) => Some(c),
                    _ => None,
                })
                .collect();
            let signature = heads.first().and_then(|c| c.signature());
            let missing: Vec<Ctor> = signature
                .iter()
                .flatten()
                .filter(|c| !heads.iter().any(|h| h.same(c)))
                .cloned()
                .collect();
            if signature.is_some() && missing.is_empty() {
                for c in signature.into_iter().flatten() {
                    let spec = specialize(&rows, &c);
                    let mut vv = vec![DPat::Wild; c.arity()];
                    vv.extend_from_slice(rest);
                    if let Some(w) = useful(&spec, &vv, mixed) {
                        return Some(rebuild(&c, w));
                    }
                }
                return None;
            }
            let default: Vec<Vec<DPat>> = rows
                .iter()
                .filter(|r| matches!(r[0], DPat::Wild))
                .map(|r| r[1..].to_vec())
                .collect();
            useful(&default, rest, mixed).map(|w| {
                let head = match missing.into_iter().next() {
                    Some(c) => DPat::Ctor(c.clone(), vec![DPat::Wild; c.arity()]),
                    None => DPat::Wild,
                };
                std::iter::once(head).chain(w).collect()
            })
        }
        DPat::Product(..) => unreachable!("normalize_column turns products into constructors"),
    }
}

/// The domain of column 0 when it holds integer ranges.
fn int_domain(rows: &[Vec<DPat>], head: &DPat) -> Option<(i128, i128)> {
    rows.iter().map(|r| &r[0]).chain(std::iter::once(head)).find_map(|p| match p {
        DPat::Ctor(Ctor::Int { dom, .. }, _) => Some(*dom),
        _ => None,
    })
}

/// [`useful`] for a column of integer ranges: split the part of the domain the
/// head matches into segments no range boundary crosses, so each segment lies
/// wholly inside or wholly outside every row's range, and try each in turn.
fn useful_ints(
    rows: &[Vec<DPat>],
    head: &DPat,
    rest: &[DPat],
    dom: (i128, i128),
    mixed: &mut Option<Rc<str>>,
) -> Option<Vec<DPat>> {
    let (lo, hi) = match head {
        DPat::Ctor(Ctor::Int { lo, hi, .. }, _) => (*lo, *hi),
        _ => dom,
    };
    let ranges = rows.iter().filter_map(|r| match &r[0] {
        DPat::Ctor(Ctor::Int { lo, hi, .. }, _) => Some((*lo, *hi)),
        _ => None,
    });
    // Any uncovered segment is a witness. Trying interior gaps first, then the
    // top, then the bottom of the domain names the gap a reader expects.
    let mut segs = segments((lo, hi), ranges);
    segs.sort_by_key(|&(a, b)| match (a == dom.0, b == dom.1) {
        (false, false) => 0,
        (false, true) => 1,
        _ => 2,
    });
    for (a, b) in segs {
        let spec: Vec<Vec<DPat>> = rows
            .iter()
            .filter(|r| match &r[0] {
                DPat::Ctor(Ctor::Int { lo, hi, .. }, _) => *lo <= a && b <= *hi,
                _ => true,
            })
            .map(|r| r[1..].to_vec())
            .collect();
        if let Some(w) = useful(&spec, rest, mixed) {
            let seg = DPat::Ctor(Ctor::Int { lo: a, hi: b, dom }, Vec::new());
            return Some(std::iter::once(seg).chain(w).collect());
        }
    }
    None
}

/// Split `span` at every boundary of `ranges` into consecutive inclusive
/// segments, each wholly inside or wholly outside every range.
fn segments(span: (i128, i128), ranges: impl Iterator<Item = (i128, i128)>) -> Vec<(i128, i128)> {
    let (lo, hi) = span;
    let mut cuts: Vec<i128> = vec![lo, hi + 1];
    for (a, b) in ranges {
        cuts.extend([a, b + 1].into_iter().filter(|c| lo < *c && *c <= hi));
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2).map(|w| (w[0], w[1] - 1)).collect()
}

/// Bring column 0 of `rows` and `head` to one constructor family. Name-keyed
/// products become positional over the union of the column's field names; a
/// sequence step facing the `List` union becomes the matching `List` variant; a
/// row whose head is opaque or of another family is dropped (it covers nothing
/// here), and such a `head` is tested as a wildcard. A variant row dropped for
/// facing sequence steps, or the reverse, sets `mixed` to the union's name.
fn normalize_column(
    rows: &[Vec<DPat>],
    head: &DPat,
    mixed: &mut Option<Rc<str>>,
) -> (Vec<Vec<DPat>>, DPat) {
    let all = || rows.iter().map(|r| &r[0]).chain(std::iter::once(head));
    let mut names: Vec<String> = Vec::new();
    let mut kind: Option<ProductKind> = None;
    let mut list: Option<Ctor> = None;
    for p in all() {
        match p {
            DPat::Product(k, fields) => {
                kind.get_or_insert_with(|| k.clone());
                for (n, _) in fields {
                    if !names.contains(n) {
                        names.push(n.clone());
                    }
                }
            }
            DPat::Ctor(c @ Ctor::Variant { union, .. }, _) if &**union == SEQ_UNION.0 => {
                list.get_or_insert_with(|| c.clone());
            }
            _ => {}
        }
    }
    let names: Rc<[String]> = names.into();
    let fix = |p: &DPat| -> DPat {
        match p {
            DPat::Product(_, fields) => {
                let kind = kind.clone().expect("a product sets the column's kind");
                let args = names
                    .iter()
                    .map(|n| {
                        fields
                            .iter()
                            .find(|(f, _)| f == n)
                            .map_or(DPat::Wild, |(_, p)| p.clone())
                    })
                    .collect();
                DPat::Ctor(Ctor::Product { kind, fields: names.clone() }, args)
            }
            DPat::Ctor(c @ (Ctor::SeqEmpty | Ctor::SeqMore), args) => match &list {
                Some(Ctor::Variant { union, siblings, .. }) => {
                    let tag = if matches!(c, Ctor::SeqEmpty) { SEQ_UNION.1 } else { SEQ_UNION.2 };
                    match siblings.iter().position(|(t, n)| t == tag && *n == args.len()) {
                        Some(index) => DPat::Ctor(
                            Ctor::Variant {
                                union: union.clone(),
                                index,
                                siblings: siblings.clone(),
                            },
                            args.clone(),
                        ),
                        None => DPat::Opaque,
                    }
                }
                _ => p.clone(),
            },
            _ => p.clone(),
        }
    };
    let rows: Vec<Vec<DPat>> = rows
        .iter()
        .map(|r| std::iter::once(fix(&r[0])).chain(r[1..].iter().cloned()).collect())
        .collect();
    let head = fix(head);
    let family: Option<Ctor> = rows
        .iter()
        .map(|r| &r[0])
        .chain(std::iter::once(&head))
        .find_map(|p| match p {
            DPat::Ctor(c, _) => Some(c.clone()),
            _ => None,
        });
    let mut fits = |p: &DPat| match (p, &family) {
        (DPat::Ctor(c, _), Some(f)) => {
            let ok = c.same_family(f);
            if !ok {
                if let Some(u) = seq_variant_clash(c, f) {
                    mixed.get_or_insert(u);
                }
            }
            ok
        }
        (DPat::Opaque, _) => false,
        _ => true,
    };
    let rows = rows.into_iter().filter(|r| fits(&r[0])).collect();
    let head = if fits(&head) { head } else { DPat::Wild };
    (rows, head)
}

/// The union whose variants meet sequence steps when `a` and `b` are one of each.
fn seq_variant_clash(a: &Ctor, b: &Ctor) -> Option<Rc<str>> {
    match (a, b) {
        (Ctor::Variant { union, .. }, Ctor::SeqEmpty | Ctor::SeqMore)
        | (Ctor::SeqEmpty | Ctor::SeqMore, Ctor::Variant { union, .. }) => Some(union.clone()),
        _ => None,
    }
}

/// The rows that match constructor `c` in column 0, with that column replaced by
/// `c`'s arguments.
fn specialize(rows: &[Vec<DPat>], c: &Ctor) -> Vec<Vec<DPat>> {
    rows.iter()
        .filter_map(|r| {
            let args = match &r[0] {
                DPat::Ctor(d, args) if d.same(c) => args.clone(),
                DPat::Ctor(..) => return None,
                _ => vec![DPat::Wild; c.arity()],
            };
            Some(args.into_iter().chain(r[1..].iter().cloned()).collect())
        })
        .collect()
}

/// Fold the first `c.arity()` patterns of a witness back under `c`.
fn rebuild(c: &Ctor, mut w: Vec<DPat>) -> Vec<DPat> {
    let rest = w.split_off(c.arity());
    std::iter::once(DPat::Ctor(c.clone(), w)).chain(rest).collect()
}

/// Show a witness in Thrax pattern syntax.
pub fn show(p: &DPat) -> String {
    match p {
        DPat::Wild | DPat::Opaque | DPat::Product(..) => "_".to_string(),
        DPat::Ctor(c, args) => match c {
            Ctor::Variant { union, index, siblings } => {
                let tag = &siblings[*index].0;
                if args.is_empty() {
                    format!("{union}.{tag}")
                } else {
                    let args: Vec<String> = args.iter().map(show).collect();
                    format!("{union}.{tag}.{{ {} }}", args.join(", "))
                }
            }
            Ctor::Bool(b) => b.to_string(),
            Ctor::Lit(k) => k.clone(),
            // A gap reaching down to a 64-bit type's minimum is named by its top
            // value alone: any uncovered value is a witness, and that bound is noise.
            Ctor::Int { lo, hi, dom } => match (lo == hi, hi == &dom.1) {
                (true, _) => lo.to_string(),
                _ if *lo == i64::MIN as i128 => hi.to_string(),
                (false, true) => format!("{lo} ..."),
                (false, false) => format!("{lo} ... {hi}"),
            },
            Ctor::SeqEmpty => "[]".to_string(),
            Ctor::SeqMore => {
                let mut elems = Vec::new();
                let mut cur = p;
                while let DPat::Ctor(Ctor::SeqMore, args) = cur {
                    elems.push(show(&args[0]));
                    cur = &args[1];
                }
                if matches!(cur, DPat::Ctor(Ctor::SeqEmpty, _)) {
                    format!("[{}]", elems.join(", "))
                } else {
                    elems.push(show(cur));
                    elems.join(" :: ")
                }
            }
            Ctor::Product { kind, fields } => {
                if args.iter().all(|a| matches!(a, DPat::Wild)) {
                    return "_".to_string();
                }
                match kind {
                    ProductKind::Tuple => {
                        let mut slots: Vec<(usize, String)> = fields
                            .iter()
                            .zip(args)
                            .map(|(f, a)| (f.parse().unwrap_or(0), show(a)))
                            .collect();
                        slots.sort_by_key(|(i, _)| *i);
                        let slots: Vec<String> = slots.into_iter().map(|(_, s)| s).collect();
                        format!("{{ {} }}", slots.join(", "))
                    }
                    ProductKind::Struct(_) | ProductKind::Record => {
                        let named: Vec<String> = fields
                            .iter()
                            .zip(args)
                            .filter(|(_, a)| !matches!(a, DPat::Wild))
                            .map(|(f, a)| format!(".{f} = {}", show(a)))
                            .collect();
                        let prefix = match kind {
                            ProductKind::Struct(name) => format!("{name}."),
                            _ => String::new(),
                        };
                        format!("{prefix}{{ {} }}", named.join(", "))
                    }
                }
            }
        },
    }
}

/// A dead part of a match: a whole arm, or one alternative of an or-pattern arm
/// whose other alternatives are live.
#[derive(Debug, PartialEq)]
pub enum Dead {
    Arm(usize),
    Alt(usize, usize),
}

/// The outcome of checking one match: a value no arm covers, and the parts of
/// the match no value can reach, in source order. When `mixed` names a union,
/// the arms match it with both variant and sequence patterns in one position,
/// and `missing` may be a value the dropped arms do cover.
pub struct Report {
    pub missing: Option<DPat>,
    pub mixed: Option<Rc<str>>,
    pub unreachable: Vec<Dead>,
}

/// Check a match whose arm `i` has alternatives `arms[i].0` and is guarded when
/// `arms[i].1`. A guard can fail, so a guarded arm covers nothing. An alternative
/// is tested against every unguarded row before it, including the earlier
/// alternatives of its own arm.
pub fn check(arms: &[(Vec<DPat>, bool)]) -> Report {
    let mut rows: Vec<Vec<DPat>> = Vec::new();
    let mut unreachable = Vec::new();
    for (i, (alts, guarded)) in arms.iter().enumerate() {
        let mut seen = rows.clone();
        let mut dead = Vec::new();
        for (j, p) in alts.iter().enumerate() {
            if useful(&seen, std::slice::from_ref(p), &mut None).is_none() {
                dead.push(j);
            }
            seen.push(vec![p.clone()]);
        }
        if dead.len() == alts.len() {
            unreachable.push(Dead::Arm(i));
        } else {
            unreachable.extend(dead.into_iter().map(|j| Dead::Alt(i, j)));
        }
        if !guarded {
            rows = seen;
        }
    }
    let mut mixed = None;
    let missing = useful(&rows, &[DPat::Wild], &mut mixed).map(|mut w| w.remove(0));
    Report { missing, mixed, unreachable }
}
