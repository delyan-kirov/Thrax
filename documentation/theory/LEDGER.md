# Theory ledger

**Status:** the registry of where the kernel's theory comes from, issue #472.
**Date:** 2026-11-11.
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

The test reads the tables below and checks:

- every tag and every `See` resolves to an entry;
- every rule and algorithm marked `yes` in its Code column is tagged somewhere,
  and every one marked `planned` is tagged nowhere;
- a `published` rule names a theorem that covers it; a `deviation` or
  `conjecture` names an issue, and so does every assumption;
- every source, theorem and issue a row names exists, and every vendored file
  is in [`references/`](references/README.md) with the SHA-256 written here.

The test cannot see whether an issue is still open. Closing one means
updating its rows here.

## Statuses

| Status | Meaning |
|---|---|
| `published` | in a source, and covered by a theorem whose fragment contains it |
| `published-uncovered` | in a source, but no theorem covers it in the form the kernel uses |
| `deviation` | differs from every source; the row says how, and names an issue |
| `conjecture` | no source; names an issue |

Each `published` rule is covered by *some* theorem, each for its own fragment.
No theorem covers their union, which is the kernel; see Assume [Composition].

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
| PT24 | Pujet, Tabareau. Observational Equality Meets CIC. ESOP 2024, LNCS 14576 | https://doi.org/10.1007/978-3-031-57262-3_12 | - | - | CC-BY-4.0, not fetched: the publisher serves a bot check to scripts |
| MU21 | de Moura, Ullrich. The Lean 4 Theorem Prover and Programming Language. CADE 2021 | https://doi.org/10.1007/978-3-030-79876-5_37 | - | - | CC-BY-4.0, not fetched: the publisher serves a bot check to scripts |
| AMS07 | Altenkirch, McBride, Swierstra. Observational Equality, Now! PLPV 2007 | https://doi.org/10.1145/1292597.1292608 | - | - | ACM copyright, link only |
| ST14 | Sozeau, Tabareau. Universe Polymorphism in Coq. ITP 2014 | https://doi.org/10.1007/978-3-319-08970-6_32 | - | - | Springer copyright, link only |
| BS91 | Berger, Schwichtenberg. An Inverse of the Evaluation Functional for Typed λ-calculus. LICS 1991 | https://doi.org/10.1109/LICS.1991.151645 | - | - | IEEE copyright, link only |
| Coq96 | Coquand. An Algorithm for Type-Checking Dependent Types. Science of Computer Programming 26, 1996 | https://doi.org/10.1016/0167-6423(95)00021-6 | - | - | Elsevier copyright, link only |
| AC05 | Abel, Coquand. Untyped Algorithmic Equality for Martin-Löf's Logical Framework with Surjective Pairs. TLCA 2005 | https://doi.org/10.1007/11417170_4 | - | - | Springer copyright, link only |
| PTu00 | Pierce, Turner. Local Type Inference. TOPLAS 22(1) 2000 | https://doi.org/10.1145/345099.345100 | - | - | ACM copyright, link only |
| DK21 | Dunfield, Krishnaswami. Bidirectional Typing. ACM Computing Surveys 54(5) 2021 | https://doi.org/10.1145/3450952 | - | - | ACM copyright, link only |
| Dyb94 | Dybjer. Inductive Families. Formal Aspects of Computing 6(4) 1994 | https://doi.org/10.1007/BF01211308 | - | - | Springer copyright, link only |

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
| AC20 Non-normalisation | AC20 | Introduction and Section 2 | an impredicative Prop, proof-irrelevant equality, and a cast that reduces when its two types are definitionally equal: normalisation fails for open terms | paper |

## Algorithms

| Algorithm | Source | Where in source | Status | Issue | Code |
|---|---|---|---|---|---|
| Normalisation by evaluation | BS91, Coq96 | not checked: not vendored | published-uncovered | - | yes |
| Bidirectional checking | PTu00, DK21 | not checked: not vendored | published-uncovered | - | yes |
| Typed conversion | AC05, PLT25 | PLT25 Fig. 7, algorithmic conversion; AC05 not checked: not vendored | published-uncovered | - | yes |
| Level comparison by case split | - | - | conjecture | #472 | yes |
| Recursor generation | Dyb94 | not checked: not vendored | published-uncovered | - | yes |
| Transitive axioms | - | - | conjecture | #472 | yes |

The level comparison was attributed to Lean 4 in the code, but Lean's kernel
does not split cases: `is_geq_core` (Lean4, `level.cpp:483`) is a syntactic
check, sound but incomplete. The case split needs its own source or proof.
Transitive axioms is graph reachability over what the checker resolved; it is
a conjecture only in that nothing proves the recorded edges complete.

## Rules

| Rule | Source | Where in source | Status | Covered by | Issue | Code |
|---|---|---|---|---|---|---|
| Univ | PT23 | Fig. 2, Univ with 𝒜(Ω, 𝒰₀) and 𝒜(𝒰ᵢ, 𝒰ᵢ₊₁) | published | PT23 Decidability, PT23 Consistency | - | yes |
| Level-Polymorphism | ST14 | not checked: not vendored | published-uncovered | - | - | yes |
| Pi-Form | PT23 | Fig. 2, Π-Form with sort ℛ(s, s′), imax for constant levels | published | PT23 Decidability, PT23 Consistency | - | yes |
| Fun | PT22 | Fig. 2, Fun | published | PT22 Normalisation, PT22 Decidability | - | yes |
| App | PT22 | Fig. 2, App | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Beta | PT22 | Fig. 4, β-red | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eta-Pi | PT22 | Fig. 3, η-Eq | published | PT22 Decidability | - | yes |
| Let | PT22 | Fig. 4, β-red; a let is a typed redex whose variable stays transparent | published-uncovered | - | - | yes |
| Proof-Irrelevance | PT22 | Fig. 3, Proof-Irrelevance | published | PT22 Decidability | - | yes |
| Cumulativity | PT22 | Fig. 2, Π-Form, whose annotations are the only cumulativity | deviation | - | #472 | yes |
| Sigma-Form | PT22 | Section 4.4, Σ-types; for propositions PT23 Fig. 2, &-Form | published-uncovered | PT22 Sigma | - | yes |
| Sigma-Pair | PT22 | Section 4.4, pairs and projections | published-uncovered | PT22 Sigma | - | yes |
| Eta-Sigma | AC05 | not checked: not vendored | published-uncovered | - | - | yes |
| Top | PT22 | Fig. 2, ⊤-Form and ⊤-Intro | published | PT22 Normalisation | - | yes |
| Bottom | PT22 | Fig. 2, ⊥-Form and ⊥-Elim | published | PT22 Normalisation | - | yes |
| Eq-Form | PT22 | Fig. 2, Eq-Form | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Prop | PT22 | Fig. 2, Eq-Form, which forms ∼ only at a type in 𝒰 | deviation | - | #472 | yes |
| Eq-Stuck | PT22 | Fig. 2, Eq-Form, which needs a type in 𝒰 | deviation | - | #468 | yes |
| Refl | PT22 | Fig. 2, Refl | published | PT22 Normalisation | - | yes |
| Transport | PT22 | Fig. 2, Transport-Ω, with the non-dependent motive the paper says suffices | published | PT22 Normalisation | - | yes |
| Cast | PT22 | Fig. 2, Cast | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Fun | PT22 | Fig. 4, Eq-Fun | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Pair | PT22 | Section 4.4, Eq-Pair | published-uncovered | PT22 Sigma | - | yes |
| Eq-Omega | PT22 | Fig. 4, Eq-Ω | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Univ | PT22 | Fig. 4, Eq-Univ | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Univ-Neq | PT22 | Fig. 4, Eq-Univ-≠, with hd extended to Σ, inductive types, quotients and the sorts of a binder's sides | published-uncovered | - | - | yes |
| Eq-Pi | PT22 | Fig. 4, Eq-Π | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Eq-Sigma | PT22 | Section 4.4, Eq-Σ | published-uncovered | PT22 Sigma | - | yes |
| Eq-Ctor | PT22 | Fig. 4, Eq-zero, Eq-Suc, Eq-zero-suc, Eq-suc-zero, generalised to every constructor | published-uncovered | - | - | yes |
| Eq-Data | PLT25 | Section 4.1, the eq_c projections | deviation | - | #472 | yes |
| Eq-Quot-Class | PT22 | Section 4.1, π(t) ∼ π(u) reduces to R t u | published-uncovered | PT22 Quotients | - | yes |
| Eq-Quot | PT22 | Section 4.1, equality of two quotient types | published-uncovered | PT22 Quotients | - | yes |
| Cast-Pi | PT22 | Fig. 4, Cast-Π | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Cast-Sigma | PT22 | Section 4.4, Cast-Σ | published-uncovered | PT22 Sigma | - | yes |
| Cast-Univ | PT22 | Fig. 4, Cast-Univ | published | PT22 Normalisation, PT22 Decidability | - | yes |
| Cast-Ctor | PLT25 | Section 4.1, the cast rule for a constructor | published-uncovered | PLT25 Consistency | - | yes |
| Cast-Quot | PT22 | Section 4.1, cast of π(t) | published-uncovered | PT22 Quotients | - | yes |
| Cast-Refl | PLT25 | Fig. 3, Cast-Refl, a conversion rule | published | PLT25 Decidability, Puj22 Cast-Refl decidability | - | planned |
| Cast-Refl-Eval | Puj22, PLT25 | Puj22 Section 5.1.3, Cast-Refl-Neutral; PLT25 Section 7.2, Cast-refl-red | deviation | - | #464 | yes |
| Proof-Witness | PT22 | Fig. 2, Refl and Transport-Ω, whose proof terms the kernel drops | deviation | - | #465 | yes |
| Data-Form | Dyb94, PLT25 | PLT25 Section 4.1, inductive definitions without indices, outside Ω; Dyb94 not checked | published-uncovered | PLT25 Consistency | - | yes |
| Data-Ctor | Dyb94, PLT25 | PLT25 Section 4.1; Dyb94 not checked | published-uncovered | PLT25 Consistency | - | yes |
| Data-Elim | Dyb94 | not checked: not vendored | published-uncovered | - | - | yes |
| Data-Iota | Dyb94 | not checked: not vendored | published-uncovered | - | - | yes |
| Positivity-Restricted | Dyb94 | not checked: not vendored; the kernel admits only whole recursive fields | deviation | - | #467 | yes |
| Quot-Form | PT22 | Section 4.1, A/(R, Rr, Rs, Rt) | published-uncovered | PT22 Quotients | - | yes |
| Quot-Intro | PT22 | Section 4.1, π(t) | published-uncovered | PT22 Quotients | - | yes |
| Quot-Elim | PT22, PLT25 | PT22 Section 4.1, Q-elim; the respect premise in PLT25 Section 7.6's direction | published-uncovered | PT22 Quotients | - | yes |
| Quot-Beta | PT22 | Section 4.1, Q-elim on π(u) | published-uncovered | PT22 Quotients | - | yes |

How the deviations differ from their sources:

- **Cumulativity.** The kernel has a subtyping judgement, `@Type a <= @Type b`
  from level 1, used wherever a synthesised type meets an expected one.
  TT^obs has cumulativity only inside Π-Form's annotations, and CC^obs has
  none.
- **Eq-Prop.** `a ~ b` at a proposition is `{}`. TT^obs and CC^obs form
  equality only at relevant types. It holds in their models by proof
  irrelevance, but no source has the rule.
- **Eq-Stuck.** Equality at a type whose sort may be `0` is left stuck, and so
  is equality between instances of an inductive type whose parameters depend
  on earlier ones.
- **Eq-Data.** `D a ~ D a'` reduces to the equality of the parameters, which
  makes type formers injective. CIC^obs only provides projections from it to
  the equality of each constructor's field types.
- **Cast-Refl-Eval.** `@cast A B e t` evaluates to `t` when `A` and `B` are
  syntactically identical values, before any rule by head. CIC^obs puts
  Cast-Refl in conversion, and its Coq implementation reduces only neutral
  casts (PLT25 Section 7.2); Puj22 conjectures normalisation for the version
  on neutral types with a conversion premise (Conjecture 5.1.1). AC20 shows that
  such a rule breaks normalisation when it computes in an impredicative
  universe; the kernel only meets proofs there, which it never reduces (see
  Proof-Witness), the situation Puj22 Section 5.1.3 expects to be safe.
- **Proof-Witness.** Proofs evaluate to one irrelevant witness, and `@refl`
  and `@transp` elaborate to it, so an elaborated term cannot be re-checked.
- **Positivity-Restricted.** A restriction of the schema, not an extension:
  no `A -> D params` fields and no nested occurrences.

## Assumptions

| Assumption | Why it is believed | What would discharge it | Issue |
|---|---|---|---|
| Composition | each published rule is covered for its own fragment, and the fragments' models (setoids, sets) look compatible | a logical relation for the whole kernel, or a translation into a system that has one | #472 |
| Cast-Refl as an evaluation rule | `@cast A A e t ~ t` holds in the setoid model, and cast never computes on proofs | moving the rule into conversion, as Cast-Refl (PLT25 Fig. 3) | #464 |
| Level comparison is complete | it decides `<=` on max-plus normal forms and splits every undecided `imax` on both values of a parameter | a proof, or a published algorithm to cite | #472 |
| Proofs need not be kept | every proof the evaluator drops was type-checked, and the published systems contain the proof terms the rules would build | keeping proof terms | #465 |
