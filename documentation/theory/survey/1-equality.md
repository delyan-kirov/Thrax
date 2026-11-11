# 1. Equality

**Question:** what is the kernel's equality, and how does transport along it
compute?

## Comparison

| System | Equality | Transport into data | Cast on equal types | Extensionality |
|---|---|---|---|---|
| OTT, Epigram 2 | computes by type | coerce | *not vendored* (AMS07) | funext by computation |
| TT^obs | `∼` computes by type (PT22 Fig. 4) | `cast` by the types' heads (PT22 Fig. 4, Cast-Π) | only propositional: `castrefl` (PT22 Fig. 2, Cast-Refl) | funext and propext by computation (PT22 Fig. 4, Eq-Fun, Eq-Ω) |
| CC^obs | as TT^obs (PT23 Fig. 4) | as TT^obs | propositional (PT23 Fig. 2, Cast-Refl); in conversion as an option (Puj22 §5.1.4, Theorem 5.1.2) | as TT^obs |
| CIC^obs | as TT^obs; on inductive values characterised by the eliminator (PLT25 §4.1) | as TT^obs | conversion rule (PLT25 Fig. 3, Cast-Refl), decidable (PLT25 Theorem 5.6); the Coq implementation reduces neutral casts (PLT25 §7.2) | as TT^obs |
| sott | does not reduce; proofs built and projected (sott README, "Simplified") | `coerce`, stuck on neutral types (sott `core/checker.ml:128-163`) | none | `funext` as a proof former (sott README) |
| Sirman et al. | as CC^obs (SLK24 §2.1) | `cast` (SLK24code/Eval.hs:258-315) | in conversion, Cast-Eq-Left/Right with backtracking (SLK24 §3.4; SLK24code/Conversion.hs:216-243) | by computation |
| Lean 4 | inductive `Eq` | `Eq.rec`, computes on `rfl` | by `Eq.rec` on `rfl` | axioms: `propext` (Lean4/Core.lean:1605), `Quot.sound` (Lean4/Prelude.lean:543); funext a theorem from `Quot.sound` (Lean4/Prelude.lean:578) |
| Rocq, Agda | inductive `eq`, with `SProp`/`Prop` (GCST19) | J | by J on refl | axioms |

## By the criteria

1. **Published with a proof.** The observational systems have decidability
   proofs for their fragments (PT22 §3.4, PT23 Theorem 3.2, PLT25 Theorem
   5.6). Lean's `propext` and `Quot.sound` are axioms: they hold in a model but
   block evaluation.
2. **Computes.** Observational equality gives funext and propext without
   axioms, so closed terms keep canonicity (PLT25 Theorem 6.7). In Lean a
   transport along `propext` gets stuck. sott computes casts but not equality.
3. **Fewest rules.** sott has the fewest (no equality computation), at the
   cost of proofs written by hand. CIC^obs adds one rule, Cast-Refl, to
   TT^obs.
4. **Efficient implementation.** Sirman et al. implement CC^obs with
   Cast-Refl in conversion by NbE (SLK24 §3.4).
5. **Fits Ares.** Proofs are irrelevant and erasable in all observational
   systems.

## Decision record (proposed)

**Observational equality as in CIC^obs, with Cast-Refl as a conversion rule
(PLT25 Fig. 3), decided as in PLT25 Fig. 7 (Cast-refl-L, Cast-refl-R) and
SLK24code/Conversion.hs:216-243.** The kernel's evaluation rule
`Cast-Refl-Eval` is removed (#464). It is published only as Puj22's
Conjecture 5.1.1, and AC20 shows the hazard of the shape.

Consequence: the respect obligation of a quotient operation, which needed the
evaluation rule (Note [A cast between identical types is the identity]), is
met by conversion instead, since conversion is where the checker compares the
expected and the synthesised type.
