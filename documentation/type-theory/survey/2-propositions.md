# 2. Propositions

**Question:** which universe holds proofs, is it definitionally proof
irrelevant, and what may a proof be eliminated into?

## Comparison

| System | Universe of proofs | Definitional irrelevance | Elimination into data | Inductive propositions |
|---|---|---|---|---|
| TT^obs | a hierarchy Ωᵢ (PT22 Fig. 2, Univ) | yes (PT22 Fig. 3, Proof-Irrelevance) | only `⊥-elim` and `cast` (PT22 §2.2) | none |
| CC^obs | one impredicative Ω (PT23 §2.2, 𝒜(Ω, 𝒰₀)) | yes | only `⊥-elim` and `cast`; impredicativity confined to Ω (Puj22 §5.4.1) | by impredicative encoding |
| CIC^obs | `SProp` in the Coq implementation (PLT25 §7) | yes | as CC^obs | not in Ω (PLT25 §4.1) |
| Sirman et al. | Ω (SLK24 §2.1) | yes, by not comparing irrelevant terms (SLK24 §3.4) | as CC^obs | none |
| Lean 4 | impredicative `Prop` | yes | subsingleton elimination: one constructor with propositional or index fields (Lean4/inductive.cpp:510-575), so `Eq` and `Acc` eliminate into data | yes |
| Rocq, Agda | `SProp` / `Prop` (GCST19) | yes | restricted by GCST19's criterion | yes |

## By the criteria

1. **Published with a proof.** CC^obs's confinement is what its
   normalisation proof relies on (Puj22 §5.4.1: "CC^obs cannot define more
   functions in the proof-relevant layer than MLTT"). Subsingleton
   elimination of a proof-irrelevant `Acc` breaks normalisation (AC20, which
   cites Carneiro's counterexample for Lean), and Puj22 §5.4.1 calls large elimination with
   irrelevance "a serious obstacle".
2. **Computes.** With only `⊥-elim` and `cast` crossing the layers, no
   computation ever inspects a proof, so proofs can be erased.
3. **Fewest rules.** No inductive propositions means no elimination
   criterion at all.
4. **Efficient implementation.** Sirman et al. skip irrelevant terms in
   conversion (SLK24 §3.4).
5. **Fits Ares.** Erasure of every proof is what the backends want.

## Decision record (proposed)

**One impredicative universe of definitionally irrelevant propositions,
`@Type 0`, as CC^obs's Ω. No inductive propositions and no subsingleton
elimination; a proof reaches data only through `@absurd` and `@cast`.** `Acc`
has no large elimination (already agreed on #471; AC20). This is the kernel's
current design, so the only change is #465: keep proof terms in elaborated
output, so that it can be re-checked, while still never evaluating them.

Update (2026-11-11): `a ~ b`, `@refl` and `@transp` are rejected at a
proposition, as Eq-Form has it (#476). Inside data, equality of a proof
component is `{}`, read as the Box type of PT22 Section 4.3.
