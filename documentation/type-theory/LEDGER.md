# Theory ledger

**Status:** the registry of where the kernel's theory comes from, issue #472.
**Date:** 2026-11-14.
**Scope:** every source, algorithm, rule, theorem and assumption behind
`crates/kernel`, with a status, enforced by `crates/kernel/tests/ledger.rs`.

---

The kernel implements a type theory that nobody has proved sound as a whole.
This file says, for each piece, who published it, what proves it, and where
the kernel goes past or away from the proof. A piece of code that implements
a rule says so with a tag on its own line:

```rust
// Rule [Eq-Fun]
// Algorithm [Normalisation by evaluation]
```

Prose may point at an entry with `See Rule [Name]` or `See Algorithm [Name]`.
The Lean proofs in [`consistency-proof/`](consistency-proof) tag the constructor that states a
rule the same way, as `-- Rule [Name]`, with prose allowed after it.

The test reads the tables below and checks:

- every tag and every `See` resolves to an entry;
- every rule and algorithm marked `yes` in its Code column is tagged somewhere,
  and every one marked `planned` is tagged nowhere;
- a `published` rule names a theorem that covers it; a `deviation` names an
  issue or a theorem that covers it; a `conjecture` names an issue, and so
  does every assumption;
- every rule covered by a theorem proved in Lean is tagged `-- Rule [Name]`
  in `consistency-proof/`, and every Lean tag names a rule such a theorem covers;
- every source, theorem and issue a row names exists, and every vendored file
  is in [`references/`](references/README.md) with the SHA-256 written here.

The test cannot see whether an issue is still open. Closing one means
updating its rows here.

The design questions behind the rules, compared across systems, are the
[survey](survey/README.md).

## Statuses

| Status | Meaning |
|---|---|
| `published` | in a source, and covered by a theorem whose fragment contains it |
| `published-uncovered` | in a source, but no theorem covers it in the form the kernel uses |
| `deviation` | differs from every source; the row says how, and names an issue |
| `conjecture` | no source; names an issue |

Each published theorem covers its own fragment. Lean Consistency covers every
rule at once: it is the kernel's rules together, proved consistent in Lean
(#481). It proves consistency only; normalisation and decidability of the
whole kernel are not proved, and the published theorems that give them each
cover a fragment.

Paper rule names are written as the paper prints them, so `Eq-Π` is the rule
the kernel calls `Eq-Pi`. "Not checked" in a Where column means the location
was not verified against the text, because the source is not vendored.

## Sources

[`references/`](references/README.md) holds the vendored files. A source that may not be
redistributed, or that is not fetched yet, has `-` for its path and hash.

| Key | Citation | DOI or URL | Vendored | SHA-256 | Licence |
|---|---|---|---|---|---|
| PT22 | Pujet, Tabareau. Observational Equality: Now for Good. POPL 2022 | https://doi.org/10.1145/3498693 | PT22/paper.pdf | c10ae7c3ad9779e7215d21c0576e7f881242f818b074126d9b5531de74153351 | CC-BY-4.0 |
| PT23 | Pujet, Tabareau. Impredicative Observational Equality. POPL 2023 | https://doi.org/10.1145/3571739 | PT23/paper.pdf | 7a40dfcb0c56a3c7c08826e3a914bb0cbc0f5022b5b2ddb9e18e31004114665d | CC-BY-4.0 |
| PLT25 | Pujet, Leray, Tabareau. Observational Equality Meets CIC. TOPLAS 47(2) 2025, the journal version of PT24 | https://doi.org/10.1145/3719342 | PLT25/paper.pdf | 15ffdd3de370ce7cb4f57565c57ceb22c8aba8560190b34816d11f42ee059e0d | CC-BY-4.0 |
| Puj22 | Pujet. Computing with Extensionality Principles in Type Theory. PhD thesis, Nantes Université 2022 | https://hal.science/tel-03923041 | Puj22/paper.pdf | 11cd792f3dd0c5e8ce7dab6b274262475ba6ff369342ff00331832e95d055c2b | CC-BY-NC-ND-4.0 |
| AC20 | Abel, Coquand. Failure of Normalization in Impredicative Type Theory with Proof-Irrelevant Propositional Equality. LMCS 16(2) 2020 | https://doi.org/10.23638/LMCS-16(2:14)2020 | AC20/paper.pdf | a7279158c5d217cf2fe1657a27856d2aec60337123d9b58eb77d3dfd18bc3de5 | CC-BY-4.0 |
| Lean4 | The Lean 4 kernel, src/kernel/level.cpp, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/kernel/level.cpp | Lean4/level.cpp | 34526f1009b9c9a08c9944950976bb6b6036d279e38a35eaaf1221ed9f5ceb3b | Apache-2.0 |
| PT24 | Pujet, Tabareau. Observational Equality Meets CIC. ESOP 2024, LNCS 14576, pp. 275-301 | https://doi.org/10.1007/978-3-031-57262-3_12 | PT24/paper.pdf | dfca4534381a6bf4649c41f26a416ad562d6b772441c07676c0d37fcc5cad819 | CC-BY-4.0 |
| MU21 | de Moura, Ullrich. The Lean 4 Theorem Prover and Programming Language. CADE 2021, LNCS 12699, pp. 625-635 | https://doi.org/10.1007/978-3-030-79876-5_37 | MU21/paper.pdf | 501866908f06b30a165662e5ca6a94e89a988dc09fa94426ff6259714612e938 | CC-BY-4.0 |
| SLK24 | Sirman, Lennon-Bertrand, Krishnaswami. Implementing a Type Theory with Observational Equality, Using Normalisation by Evaluation. TYPES 2024, LIPIcs 336 | https://doi.org/10.4230/LIPIcs.TYPES.2024.5 | SLK24/paper.pdf | b880a8614914151a3f022c7b7f132e8f0a539841aa84629a31358e6c4a6ed171 | CC-BY-4.0 |
| SLK24-Conversion | Sirman. observational-type-theory, src/Conversion.hs, commit 53982dc5093f69db83a157367c2d40aadbdcd039 | https://github.com/matthew-sirman/observational-type-theory/blob/53982dc5093f69db83a157367c2d40aadbdcd039/src/Conversion.hs | SLK24code/Conversion.hs | 2d100d5dde8dfba0e2457aa270ae8f7f38556211f83c05ba24e84ee2abf9ebbf | BSD-3-Clause |
| SLK24-Eval | Sirman. observational-type-theory, src/Eval.hs, commit 53982dc5093f69db83a157367c2d40aadbdcd039 | https://github.com/matthew-sirman/observational-type-theory/blob/53982dc5093f69db83a157367c2d40aadbdcd039/src/Eval.hs | SLK24code/Eval.hs | 72b38d728be06a5b5c3776ddd5ff38479f3323080e403d41890d60b392b28a93 | BSD-3-Clause |
| smalltt | Kovács. smalltt, README.md, commit ea99b0f478e50dcb81ea19e40bfb4262339b22aa | https://github.com/AndrasKovacs/smalltt/blob/ea99b0f478e50dcb81ea19e40bfb4262339b22aa/README.md | smalltt/README.md | 0ae84e3f07972c8678f1e6168bf9f6b4d989b64f75e0d25b5e028a96b228f2e4 | MIT |
| Lean4-quot | The Lean 4 kernel, src/kernel/quot.cpp, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/kernel/quot.cpp | Lean4/quot.cpp | 500621538bfa97cd63ebf0c4b0e111b69232f5d4fa34aa56b5f6482599782627 | Apache-2.0 |
| Lean4-inductive | The Lean 4 kernel, src/kernel/inductive.cpp, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/kernel/inductive.cpp | Lean4/inductive.cpp | 6f0d3c5d31fc67ae58d727b12dc989d524fadc1bddc79e45d0e7589a6a207c79 | Apache-2.0 |
| Lean4-type-checker | The Lean 4 kernel, src/kernel/type_checker.cpp, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/kernel/type_checker.cpp | Lean4/type_checker.cpp | 8bfd17c68b0abf76efdaa8c5ad250e7b9f5c87f35ca59a570df7acbcfea7785d | Apache-2.0 |
| Lean4-Prelude | Lean 4, src/Init/Prelude.lean, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/Init/Prelude.lean | Lean4/Prelude.lean | fa2c883cf69beadc203c1d9c33a72b750bf11959d9f460d7c2febfb390c63acd | Apache-2.0 |
| Lean4-Core | Lean 4, src/Init/Core.lean, commit 84c7f508f95c62aaa5049cf220f7279d3fcfde1a | https://github.com/leanprover/lean4/blob/84c7f508f95c62aaa5049cf220f7279d3fcfde1a/src/Init/Core.lean | Lean4/Core.lean | c59ce725905c3878be557a5569ccbda9fa7affedfb28b2b930429eb245c1263c | Apache-2.0 |
| Rocq-uGraph | The Rocq kernel, kernel/uGraph.ml, commit dd62e3cfb7999a10b091000ec33b5c8a185b2326 | https://github.com/rocq-prover/rocq/blob/dd62e3cfb7999a10b091000ec33b5c8a185b2326/kernel/uGraph.ml | Rocq/uGraph.ml | 6bf3f594e16ca686f7144ff946f983cd264de6b299b71c32fc6a6de24d97425f | LGPL-2.1 |
| nanoda-level | nanoda_lib, an independent Lean 4 type checker in Rust, src/level.rs, commit 0da9f98feadd3a0a2ef4b3dbcf52a298a6fef929 | https://github.com/ammkrn/nanoda_lib/blob/0da9f98feadd3a0a2ef4b3dbcf52a298a6fef929/src/level.rs | nanoda/level.rs | a1afb0638fd9f427c6e7693ea44498b9775bd619bb6fff3bdc44dc52f1d587ad | Apache-2.0 |
| nanoda-tc | nanoda_lib, an independent Lean 4 type checker in Rust, src/tc.rs, commit 0da9f98feadd3a0a2ef4b3dbcf52a298a6fef929 | https://github.com/ammkrn/nanoda_lib/blob/0da9f98feadd3a0a2ef4b3dbcf52a298a6fef929/src/tc.rs | nanoda/tc.rs | 1c74274ec2d979796f2dfa73e93d63db54cbb4ac388d283fda28e472dcd96781 | Apache-2.0 |
| AMS07 | Altenkirch, McBride, Swierstra. Observational Equality, Now! PLPV 2007 | https://doi.org/10.1145/1292597.1292608 | - | - | ACM copyright, link only |
| ST14 | Sozeau, Tabareau. Universe Polymorphism in Coq. ITP 2014 | https://doi.org/10.1007/978-3-319-08970-6_32 | - | - | Springer copyright, link only |
| BS91 | Berger, Schwichtenberg. An Inverse of the Evaluation Functional for Typed λ-calculus. LICS 1991 | https://doi.org/10.1109/LICS.1991.151645 | - | - | IEEE copyright, link only |
| Coq96 | Coquand. An Algorithm for Type-Checking Dependent Types. Science of Computer Programming 26, 1996 | https://doi.org/10.1016/0167-6423(95)00021-6 | - | - | Elsevier copyright, link only |
| AC05 | Abel, Coquand. Untyped Algorithmic Equality for Martin-Löf's Logical Framework with Surjective Pairs. TLCA 2005 | https://doi.org/10.1007/11417170_4 | - | - | Springer copyright, link only |
| PTu00 | Pierce, Turner. Local Type Inference. TOPLAS 22(1) 2000 | https://doi.org/10.1145/345099.345100 | - | - | ACM copyright, link only |
| DK21 | Dunfield, Krishnaswami. Bidirectional Typing. ACM Computing Surveys 54(5) 2021 | https://doi.org/10.1145/3450952 | - | - | ACM copyright, link only |
| Dyb94 | Dybjer. Inductive Families. Formal Aspects of Computing 6(4) 1994 | https://doi.org/10.1007/BF01211308 | - | - | Springer copyright, link only |
| GCST19 | Gilbert, Cockx, Sozeau, Tabareau. Definitional Proof-Irrelevance without K. POPL 2019 | https://doi.org/10.1145/3290316 | GCST19/paper.pdf | 3b14f8d2c37705def11232369b9c0da7c3680f3a463e5b06b5d156ed77387e8e | CC-BY-NC-SA-4.0 |
| sott | Atkey. sott, Simplified Observational Type Theory, commit 65f684453910d6988f1e5c825ac8e6ee27927469 | https://github.com/bobatkey/sott/tree/65f684453910d6988f1e5c825ac8e6ee27927469 | - | - | no licence, link only |
| LJB01 | Lee, Jones, Ben-Amram. The Size-Change Principle for Program Termination. POPL 2001 | https://doi.org/10.1145/360204.360210 | - | - | ACM copyright, link only |

## Theorems

| Theorem | Source | Where in source | Fragment | Proof |
|---|---|---|---|---|
| PT22 Normalisation | PT22 | Corollary 3.6 | TT^obs of Figs. 1 to 4: Π with cumulative annotations, ∃ in Ω, N, ⊤, ⊥, ∼, transp, cast; hierarchies 𝒰ᵢ and Ωᵢ | Agda |
| PT22 Decidability | PT22 | Section 3.4 | as PT22 Normalisation | Agda |
| PT22 Consistency | PT22 | Theorem 3.7, by the setoid model of Theorem 3.10 | as PT22 Normalisation | paper |
| PT22 Quotients | PT22 | Section 4.1 | TT^obs with quotients by a proof-irrelevant equivalence; "not formalized" | paper-sketch |
| PT22 Sigma | PT22 | Section 4.4 | TT^obs with relevant Σ-types; rules given, metatheory not replayed | paper-sketch |
| PT23 Decidability | PT23 | Theorem 3.2 and Section 3.4 | CC^obs: one impredicative Ω, finitely many 𝒰ᵢ (no cumulativity), Π with sort ℛ(s, s′), & in Ω, N, ⊤, ⊥, ∼, transp, cast | Agda |
| PT23 Consistency | PT23 | Theorem 5.4, by the standard model of Theorem 5.3 | as PT23 Decidability | paper |
| PLT25 Decidability | PLT25 | Theorem 5.6 | CIC^obs restricted to Π, N and universes, with Cast-Refl in conversion | Agda |
| PLT25 Consistency | PLT25 | Theorem 6.5 | CIC^obs with its scheme of parameterised inductive types (Section 4.1) | paper |
| Puj22 Cast-Refl decidability | Puj22 | Theorem 5.1.2, Section 5.1.4 | CC^obs with Cast-Refl decided during conversion | Agda |
| Lean Consistency | - | `consistency-proof/Proofs/Consistency.lean`, `consistent₁` to `consistent₄` and `consistent_poly` | every rule below at once, as `Deriv` in `consistency-proof/Proofs/Typing.lean`, over any signature of inductive declarations (`SigOk`), for each bound on the universes with a model (four are built); levels are numbers, and a level-polymorphic derivation is its instances; the differences from the kernel are listed in `consistency-proof/Proofs/Syntax.lean` | Lean |
| Lean Level comparison soundness | - | `consistency-proof/Proofs/Levels.lean`, `Lvl.leq_sound` | Algorithm [Level comparison by case split], with fuel bounding its depth: a yes holds under every assignment | Lean |
| AC20 Non-normalisation | AC20 | Introduction and Section 2 | an impredicative Prop, proof-irrelevant equality, and a cast that reduces when its two types are definitionally equal: normalisation fails for open terms | paper |

## Algorithms

| Algorithm | Source | Where in source | Status | Issue | Code |
|---|---|---|---|---|---|
| Normalisation by evaluation | BS91, Coq96 | not checked: not vendored | published-uncovered | - | yes |
| Bidirectional checking | PTu00, DK21 | not checked: not vendored | published-uncovered | - | yes |
| Typed conversion | AC05, PLT25 | PLT25 Fig. 7, algorithmic conversion; AC05 not checked: not vendored | published-uncovered | - | yes |
| Level comparison by case split | nanoda-level | level.rs:158-173, leq_imax_by_cases, the same split on a parameter being zero or not; soundness is Lean Level comparison soundness | published-uncovered | #479 | yes |
| Recursor generation | Dyb94 | not checked: not vendored | published-uncovered | - | yes |
| Transitive axioms | - | - | conjecture | #480 | yes |

The level comparison was attributed to Lean 4 in the code, but Lean's C++
kernel does not split cases: `is_geq_core` (Lean4, `level.cpp:483`) is a
syntactic check, sound but incomplete. nanoda_lib, an independent Lean kernel
in Rust, does split (`nanoda/level.rs:158-173`), which is the kernel's
algorithm; neither has a proof. Its soundness is proved in Lean
(`consistency-proof/Proofs/Levels.lean`); completeness is open (#479).
Transitive axioms is graph reachability over what the checker resolved; it is
a conjecture only in that nothing proves the recorded edges complete.

## Rules

| Rule | Source | Where in source | Status | Covered by | Issue | Code |
|---|---|---|---|---|---|---|
| Univ | PT23 | Fig. 2, Univ with 𝒜(Ω, 𝒰₀) and 𝒜(𝒰ᵢ, 𝒰ᵢ₊₁) | published | PT23 Decidability, PT23 Consistency, Lean Consistency | - | yes |
| Level-Polymorphism | ST14 | not checked: not vendored | published | Lean Consistency | - | yes |
| Pi-Form | PT23 | Fig. 2, Π-Form with sort ℛ(s, s′), imax for constant levels | published | PT23 Decidability, PT23 Consistency, Lean Consistency | - | yes |
| Fun | PT22 | Fig. 2, Fun | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| App | PT22 | Fig. 2, App | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Beta | PT22 | Fig. 4, β-red | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eta-Pi | PT22 | Fig. 3, η-Eq | published | PT22 Decidability, Lean Consistency | - | yes |
| Let | PT22 | Fig. 4, β-red; a let is a typed redex whose variable stays transparent | published | Lean Consistency | - | yes |
| Proof-Irrelevance | PT22 | Fig. 3, Proof-Irrelevance | published | PT22 Decidability, Lean Consistency | - | yes |
| Sigma-Form | PT22 | Section 4.4, Σ-types; for propositions PT23 Fig. 2, &-Form | published | PT22 Sigma, Lean Consistency | - | yes |
| Sigma-Pair | PT22 | Section 4.4, pairs and projections | published | PT22 Sigma, Lean Consistency | - | yes |
| Eta-Sigma | AC05 | not checked: not vendored | published | Lean Consistency | - | yes |
| Top | PT22 | Fig. 2, ⊤-Form and ⊤-Intro | published | PT22 Normalisation, Lean Consistency | - | yes |
| Bottom | PT22 | Fig. 2, ⊥-Form and ⊥-Elim | published | PT22 Normalisation, Lean Consistency | - | yes |
| Eq-Form | PT22 | Fig. 2, Eq-Form | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Box | PT22, Puj22 | PT22 Section 4.3, the equality of □A; Puj22 Section 3.2.10, Box-Proof-Eq; a proof inside data is read as boxed | published | Lean Consistency | - | yes |
| Eq-Stuck | PT22 | Fig. 2, Eq-Form, which needs a type in 𝒰; a level parameter can leave that undecided | deviation | Lean Consistency | - | yes |
| Eq-Data-Stuck | PLT25 | Section 2.3, no computation for `Ind a ∼ Ind a'`; Section 4.1, only the eq_c projections use it | published | PLT25 Consistency, Lean Consistency | - | yes |
| Refl | PT22 | Fig. 2, Refl | published | PT22 Normalisation, Lean Consistency | - | yes |
| Transport | PT22 | Fig. 2, Transport-Ω, with the non-dependent motive the paper says suffices | published | PT22 Normalisation, Lean Consistency | - | yes |
| Cast | PT22 | Fig. 2, Cast | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Fun | PT22 | Fig. 4, Eq-Fun | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Pair | PT22 | Section 4.4, Eq-Pair | published | PT22 Sigma, Lean Consistency | - | yes |
| Eq-Omega | PT22 | Fig. 4, Eq-Ω | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Univ | PT22 | Fig. 4, Eq-Univ | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Univ-Neq | PT22 | Fig. 4, Eq-Univ-≠, with hd extended to Σ, inductive types, quotients and the sorts of a binder's sides | published | Lean Consistency | - | yes |
| Eq-Pi | PT22 | Fig. 4, Eq-Π | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Eq-Sigma | PT22 | Section 4.4, Eq-Σ | published | PT22 Sigma, Lean Consistency | - | yes |
| Eq-Ctor | PT22 | Fig. 4, Eq-zero, Eq-Suc, Eq-zero-suc, Eq-suc-zero, generalised to every constructor | published | Lean Consistency | - | yes |
| Eq-Univ-Data | PT22 | Fig. 4, Eq-Univ for ℕ, extended to every inductive type without parameters | published | Lean Consistency | - | yes |
| Data-Eq-Projection | PLT25 | Section 4.1, the eq_c projections from `Ind a ∼ Ind a'` to the equality of each constructor's field types | published | PLT25 Consistency, Lean Consistency | - | yes |
| Eq-Quot-Class | PT22 | Section 4.1, π(t) ∼ π(u) reduces to R t u | published | PT22 Quotients, Lean Consistency | - | yes |
| Eq-Quot | PT22 | Section 4.1, equality of two quotient types | published | PT22 Quotients, Lean Consistency | - | yes |
| Cast-Pi | PT22 | Fig. 4, Cast-Π | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Cast-Sigma | PT22 | Section 4.4, Cast-Σ | published | PT22 Sigma, Lean Consistency | - | yes |
| Cast-Univ | PT22 | Fig. 4, Cast-Univ | published | PT22 Normalisation, PT22 Decidability, Lean Consistency | - | yes |
| Cast-Ctor | PLT25 | Section 4.1, the cast rule for a constructor | published | PLT25 Consistency, Lean Consistency | - | yes |
| Cast-Quot | PT22 | Section 4.1, cast of π(t) | published | PT22 Quotients, Lean Consistency | - | yes |
| Cast-Refl | PLT25 | Fig. 3, Cast-Refl, a conversion rule | published | PLT25 Decidability, Puj22 Cast-Refl decidability, Lean Consistency | - | yes |
| Proof-Witness | PT22, AC20 | PT22 Fig. 2, the proof terms of Refl, Transport-Ω and every other proof, which the kernel drops; AC20 Section 2, the loops that dropping them avoids | deviation | Lean Consistency | #465 | yes |
| Data-Form | Dyb94, PLT25 | PLT25 Section 4.1, inductive definitions without indices, outside Ω (with indices, see Data-Ford); Dyb94 not checked | published | PLT25 Consistency, Lean Consistency | - | yes |
| Data-Ford | PLT25 | Section 4.2, Fordism: indices as parameters with one equality field per index, and a cast in each branch of a match; index types restricted to depend on the parameters only | published | PLT25 Consistency, Lean Consistency | - | yes |
| Data-Ctor | Dyb94, PLT25 | PLT25 Section 4.1; Dyb94 not checked | published | PLT25 Consistency, Lean Consistency | - | yes |
| Data-Elim | Dyb94 | not checked: not vendored | published | Lean Consistency | - | yes |
| Data-Iota | Dyb94 | not checked: not vendored | published | Lean Consistency | - | yes |
| Positivity-Restricted | Dyb94 | not checked: not vendored; the kernel admits only whole recursive fields | deviation | Lean Consistency | #467 | yes |
| Quot-Form | PT22 | Section 4.1, A/(R, Rr, Rs, Rt) | published | PT22 Quotients, Lean Consistency | - | yes |
| Quot-Intro | PT22 | Section 4.1, π(t) | published | PT22 Quotients, Lean Consistency | - | yes |
| Quot-Elim | PT22, PLT25 | PT22 Section 4.1, Q-elim; the respect premise in PLT25 Section 7.6's direction | published | PT22 Quotients, Lean Consistency | - | yes |
| Quot-Beta | PT22 | Section 4.1, Q-elim on π(u) | published | PT22 Quotients, Lean Consistency | - | yes |

How the deviations differ from their sources:

- **Eq-Stuck.** Equality at a type whose sort is a level that may or may
  not be `0` is left stuck, since neither Eq-Form nor Eq-Box applies until the
  level is known. No source has level parameters over an observational
  theory. Lean Consistency covers it: in each instance of a level-polymorphic
  derivation the level is a number, so the stuck equality is never formed
  (#478). (`D a ~ D a'` is
  stuck too, but that is CIC^obs's rule, Eq-Data-Stuck; until 2026-11-11 it
  reduced to `a ~ a'`, which was unsound, #477.)
- **Proof-Witness.** Every term checked against a proposition elaborates to
  one irrelevant witness, so an elaborated term cannot be re-checked. Keeping
  proofs would need another way to stop evaluation from looping on AC20's
  terms.
- **Positivity-Restricted.** A restriction of the schema, not an extension:
  no `A -> D params` fields and no nested occurrences.

## Assumptions

| Assumption | Why it is believed | What would discharge it | Issue |
|---|---|---|---|
| Level comparison is complete | it decides `<=` on max-plus normal forms and splits every undecided `imax` on both values of a parameter, as nanoda_lib does | a proof | #479 |
| Proofs need not be kept | every proof the evaluator drops was type-checked, and the published systems contain the proof terms the rules would build | keeping proof terms | #465 |
