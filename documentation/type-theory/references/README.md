# Ares references

The publications that the Ares kernel's rules, algorithms and theorems come
from, vendored so a citation in the theory ledger always points at the exact
text it was checked against.

The ledger is [`../LEDGER.md`](../LEDGER.md). Each `Source` row there names a
directory here and the SHA-256 of its file, and
`crates/kernel/tests/ledger.rs` checks that the file exists and the hash
matches.

## Layout

One directory per source, named by its ledger key:

```
<KEY>/paper.pdf   the publication (for source code: the file itself, and its LICENSE)
<KEY>/entry.bib   BibTeX, with the DOI, the URL the copy came from, and its licence
```

The hashes live in the ledger only, where the test checks them.

## Adding a source

1. Check the licence. Vendor only what may be redistributed (CC-BY and
   similar). Anything else gets a ledger row with its URL and no file.
2. Create `<KEY>/paper.pdf` and `<KEY>/entry.bib`, the BibTeX `note` giving
   the licence and the date fetched.
3. Put the file's SHA-256 (`sha256sum <KEY>/paper.pdf`) in the ledger row.

## Vendored

| Key | Publication | Licence |
|---|---|---|
| PT22 | Pujet, Tabareau. Observational Equality: Now for Good. POPL 2022 | CC-BY 4.0 |
| PT23 | Pujet, Tabareau. Impredicative Observational Equality. POPL 2023 | CC-BY 4.0 |
| PLT25 | Pujet, Leray, Tabareau. Observational Equality Meets CIC. TOPLAS 2025 | CC-BY 4.0 |
| Puj22 | Pujet. Computing with Extensionality Principles in Type Theory. PhD thesis, 2022 | CC-BY-NC-ND 4.0 |
| AC20 | Abel, Coquand. Failure of Normalization in Impredicative Type Theory with Proof-Irrelevant Propositional Equality. LMCS 2020 | CC-BY 4.0 |
| Lean4 | Lean 4 kernel, `src/kernel/level.cpp`, commit 84c7f508 | Apache-2.0 |
| PT24 | Pujet, Tabareau. Observational Equality Meets CIC. ESOP 2024 | CC-BY 4.0 |
| MU21 | de Moura, Ullrich. The Lean 4 Theorem Prover and Programming Language. CADE 2021 | CC-BY 4.0 |
| SLK24 | Sirman, Lennon-Bertrand, Krishnaswami. Implementing a Type Theory with Observational Equality, Using NbE. TYPES 2024 | CC-BY 4.0 |
| SLK24code | Sirman's implementation, `src/Conversion.hs` and `src/Eval.hs` | BSD-3-Clause |
| smalltt | Kovács, smalltt `README.md` (glued evaluation) | MIT |
| Lean4 | also `quot.cpp`, `inductive.cpp`, `type_checker.cpp`, `Init/Prelude.lean`, `Init/Core.lean` | Apache-2.0 |
| Rocq | Rocq kernel, `kernel/uGraph.ml` | LGPL-2.1 |
| GCST19 | Gilbert, Cockx, Sozeau, Tabareau. Definitional Proof-Irrelevance without K. POPL 2019 | CC-BY-NC-SA 4.0 |
| nanoda | nanoda_lib, a Lean 4 kernel in Rust: `src/level.rs`, `src/tc.rs` | Apache-2.0 |

Code is pinned to the commit in its `entry.bib`, so a `file:line` citation
stays valid. Not vendored: sott, whose repository has no licence; the ledger links it.

PT24 and MU21 are their chapters' pages cut from the open-access proceedings
volumes, which the publisher only serves to a browser.
