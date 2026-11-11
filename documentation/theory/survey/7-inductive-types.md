# 7. Inductive types

**Question:** which inductive declarations does the kernel accept, and how do
equality and cast treat them?

## Comparison

| System | Scheme | Equality of values | Equality of types | Cast | Nested, infinitely branching |
|---|---|---|---|---|---|
| TT^obs | ℕ only in the core; Σ and others as extensions (PT22 §4.4) | Eq-zero, Eq-Suc, Eq-zero-suc, Eq-suc-zero (PT22 Fig. 4) | by head (PT22 Fig. 4, Eq-Univ) | Cast-Zero, Cast-Suc (PT22 Fig. 4) | n/a |
| CIC^obs | parameters, no indices, outside Ω, recursive occurrences strictly positive, non-uniform parameters allowed (PLT25 §4.1) | no rule: characterised by the eliminator (PLT25 §4.1) | no computation; projections `eq_c` to each constructor's field types (PLT25 §4.1) | constructor-wise, along `eq_c` (PLT25 §4.1) | nested "mathematically straightforward but heavy on notation" (PLT25 §4); only ℕ formalised (PLT25 §5) |
| Sirman et al. | first class, one index, no positivity check (SLK24 §2.2) | like ℕ: computes (SLK24 §2.2) | `(µF) a ∼ (µF) a'` is `a ∼ a'` for one definition; stuck otherwise, and `Vec A n ∼ Vec A' n` deliberately does not give `A ∼ A'` (SLK24 §2.2) | constructor-wise (SLK24code/Eval.hs:301-312) | n/a |
| Lean 4 | mutual, nested, indexed (Lean4/inductive.cpp) | n/a | n/a | n/a | nested by translation to mutual types (Lean4/inductive.cpp:874-890) |
| Dybjer's schema | Dyb94, *not vendored* | | | | |

## By the criteria

1. **Published with a proof.** CIC^obs's scheme has a consistency proof
   (PLT25 Theorem 6.5); its decidability covers only ℕ. TT^obs's ℕ rules are
   covered by PT22 Normalisation; their generalisation to every constructor
   (the kernel's Eq-Ctor) is not. Injectivity of type formers (the kernel's
   Eq-Data) is in no source, and SLK24 §2.2 rejects it on purpose.
2. **Computes.** Eq-Ctor makes equalities of constructor values compute (the
   rationals' wrong-claim test relies on it). CIC^obs's `eq_c` projections are
   irrelevant constants: they never block computation, because casts compute
   by the constructor without inspecting the proof.
3. **Fewest rules.** CIC^obs has none for value equality and one projection
   per constructor field for type equality.
4. **Efficient implementation.** Lean's translation of nested types keeps the
   kernel's scheme small.
5. **Fits Ares.** The kernel passes the irrelevant witness where a proof is
   needed, so `eq_c` needs no proof terms to be built.

## Decision record (proposed)

*Update 2026-11-11:* the stuck equality is in the kernel. Its injectivity
rule was unsound by CIC^obs §2.3, and the kernel accepted the counterexample's
precondition (#477). The `eq_c` projections are still to do.

**CIC^obs's scheme. Equality of two instances of one inductive type no longer
computes to the equality of their parameters; it stays stuck, with CIC^obs's
`eq_c` projections as the way to use it (#477).** Eq-Ctor stays: it is
TT^obs's published rule for ℕ, generalised, and its status remains
`published-uncovered`. Strict positivity grows to infinitely branching fields
(allowed by PLT25 §4.1), and nested types are translated as Lean does (#467).
