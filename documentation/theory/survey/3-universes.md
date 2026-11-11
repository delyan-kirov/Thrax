# 3. Universes

**Question:** how are universes levelled, is there cumulativity, and how are
level constraints decided?

## Comparison

| System | Hierarchy | Polymorphism | Cumulativity | Deciding `<=` |
|---|---|---|---|---|
| TT^obs | 𝒰ᵢ and Ωᵢ (PT22 Fig. 2, Univ) | none | in Π-Form's annotations only; needed to type Eq-Ω and Eq-Π (PT22 §2.3) | constants |
| CC^obs | 𝒰ᵢ and one Ω : 𝒰₀ (PT23 §2.2) | none | none: 𝒜(𝒰ᵢ, 𝒰ⱼ) := j = i + 1 (PT23 §2.2) | constants |
| CIC^obs | as CIC; universe subtleties in PLT25 §7.5 | Coq's | Coq's | Coq's |
| Sirman et al. | `U : U`, inconsistent by choice (SLK24 §2.1) | none | n/a | n/a |
| Lean 4 | `Sort u`, `Prop = Sort 0` | level parameters, `imax` (Lean4/level.cpp:105-116) | none | syntactic and incomplete (Lean4/level.cpp:483-504) |
| Rocq | `Type@{i}`, `SProp`, `Prop` | universe polymorphism (ST14, *not vendored*) | yes, by subtyping | a constraint graph (Rocq/uGraph.ml:72-123) |
| Agda | `Set ℓ`, `Setω` | level expressions | optional | *not surveyed* |

## By the criteria

1. **Published with a proof.** CC^obs has a decidability proof with no
   cumulativity (PT23 Theorem 3.2). Its single impredicative Ω is what makes
   cumulativity unnecessary: `A ∼ B` for `A B : 𝒰ᵢ` is a proposition in Ω,
   not in a higher universe, so Eq-Π types without lifting. TT^obs needs
   cumulativity only because its Ωᵢ are levelled (PT22 §2.3). No observational
   system with a proof has level polymorphism.
2. **Computes.** Universes do not affect computation.
3. **Fewest rules.** No subtyping removes the kernel's Cumulativity rule and
   the `subtype` path of conversion (#475).
4. **Efficient implementation.** Lean's level check is fast and incomplete;
   Rocq's graph is complete for its constraints.
5. **Fits Ares.** #406 settled level binders `@ctx {u: @l}`; that needs
   polymorphism and `imax` (for `@Type 0`).

## Decision record (proposed)

**CC^obs's universes: `@Type 0` impredicative, `@Type (n + 1) : @Type
(n + 2)`, and no cumulativity. Level polymorphism with `imax` as Lean has it.**
Cumulativity is removed (#475); a value moves up a universe by an explicit
lift where a program needs it. The kernel keeps its complete case-split
comparison of levels, which needs the proof #479 asks for; Lean's incomplete
check is the fallback if no proof is found.

Open: level polymorphism over an observational theory has no published
proof. It is part of Assume [Composition] (#478).
