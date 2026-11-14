# Kernel design survey

**Status:** step 4 of issue #472. Every decision below is **proposed**; none is
taken until the owner accepts it.
**Date:** 2026-11-11.
**Scope:** eight design questions for the kernel of #406, each compared across
the observational systems and the mainstream proof assistants, ending in a
decision record.

---

Each question has a comparison table. A cell cites a section of a vendored
paper (`PT22 §4.1`) or a line of vendored source (`Lean4/quot.cpp:88`), both
under [`../references/`](../references/README.md), whose keys are the
`Sources` table of the [ledger](../LEDGER.md). A cell marked *not vendored*
cites a source that could not be checked against its text here.

"Best" is decided in this order (#472):

1. published, with a proof covering our fragment;
2. computes: closed terms reduce to values, and no axiom blocks evaluation;
3. fewest kernel rules;
4. a known efficient implementation;
5. fits Ares (machine integers, proof erasure, effects).

| # | Question | Proposed decision | Issues |
|---|---|---|---|
| 1 | [Equality](1-equality.md) | observational, Cast-Refl in conversion | #464 |
| 2 | [Propositions](2-propositions.md) | definitionally irrelevant, no subsingleton elimination | #465 |
| 3 | [Universes](3-universes.md) | level parameters with `imax`, no subtyping | #475, #479 |
| 4 | [Quotients](4-quotients.md) | TT^obs quotients, equality computes to the relation | |
| 5 | [Indexed families](5-indexed-families.md) | Fordism, over Cast-Refl in conversion | #466 |
| 6 | [Conversion](6-conversion.md) | NbE with glued globals, Cast-refl-L/R | #469, #464 |
| 7 | [Inductive types](7-inductive-types.md) | CIC^obs scheme, equality of types stuck, nested by translation | #467, #477 |
| 8 | [Termination](8-termination.md) | recursors only in the kernel | |

## Candidates

| Candidate | Sources here |
|---|---|
| OTT (Altenkirch, McBride, Swierstra 2007) and Epigram 2 | AMS07, *not vendored* (ACM copyright) |
| TT^obs | PT22 |
| CC^obs | PT23, Puj22 |
| CIC^obs and its Coq implementation | PT24, PLT25 |
| sott (Atkey, OCaml) | github.com/bobatkey/sott at 65f6844, *not vendored*: the repository has no licence |
| Sirman, Lennon-Bertrand, Krishnaswami (Haskell, TYPES 2024) | SLK24, SLK24code |
| Lean 4 | MU21, Lean4 |
| Rocq | GCST19, Rocq |
| Agda | GCST19; Agda's source was not surveyed |
| smalltt (a performance reference, not a theory) | smalltt |

## When a decision is accepted

Change its status to **accepted** and the date, add a `Source` row for the
decision record in the ledger, and update the rows of the rules it changes.
