# 5. Indexed families

**Question:** are indexed inductive families primitive, or encoded?

## Comparison

| System | Indexed families | How they compute |
|---|---|---|
| TT^obs | not covered; "we conjecture that ... any indexed inductive types as defined in CIC can be described" (PT22 §4.4) | n/a |
| CC^obs | primitive, non-recursive, one index and one argument per constructor, as an extension (Puj22 §5.3) | Ind-comp, an iota rule (Puj22 §5.3) |
| CIC^obs | Fordism: each constructor takes an equality of indices, `c_F : B → y ∼ x → Ind_F a x`, and matching inserts a cast (PLT25 §2.2, §4.2) | the computation rule of matching holds "thanks to the rule Cast-Refl" (PLT25 §4.2) |
| Sirman et al. | first class, one index, fording (SLK24 §2.2) | as CIC^obs, with Cast-Refl in conversion (SLK24 §3.4) |
| Lean 4, Rocq, Agda | primitive, with indices in the recursor (Lean4/inductive.cpp) | iota on constructors |

## By the criteria

1. **Published with a proof.** Fordism is published with CIC^obs's scheme
   (PLT25 §4.2); its consistency is PLT25 Consistency. Primitive indexed
   families have no observational treatment: their equality and cast rules
   are TT^obs's open conjecture (PT22 §4.4), and Puj22 §5.3 treats only the
   non-recursive case.
2. **Computes.** Fordism computes only if Cast-Refl holds definitionally
   (PLT25 §4.2); without it the cast inserted in every branch is stuck on
   reflexivity.
3. **Fewest rules.** Fordism adds none: indexed families are sugar over the
   parameterised scheme.
4. **Efficient implementation.** Sirman et al. (SLK24 §2.2).
5. **Fits Ares.** Sized tensors (`@Tensor n T`) are the first indexed family Ares
   needs; fording keeps them in the existing scheme.

## Decision record (proposed)

**Fordism, as CIC^obs: indexed families elaborate to parameterised inductive
types with an equality field per constructor (#466).** It depends on decision
1: Cast-Refl must be a conversion rule first, or matching on a forded family
does not compute.

Update (2026-11-11): implemented (#466). A constructor states its result,
`| cons (m: Nat) (h: a) (t: Vec a m) : Vec a (succ m)`; the binders every
constructor passes through unchanged are parameters, the rest indices. The
kernel stores the forded type, and the recursor has the indexed motive with a
cast in iota (Note [Indexed families are forded]). Not yet: index types that
depend on earlier indices, whose equality fields would be heterogeneous (#490).
