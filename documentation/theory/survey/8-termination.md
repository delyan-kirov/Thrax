# 8. Termination

**Question:** how are recursive definitions justified: by the kernel, or by
elaboration into eliminators?

## Comparison

| System | In the kernel | Recursion the user writes |
|---|---|---|
| TT^obs, CC^obs | eliminators only (PT22 Fig. 2, N-Elim) | n/a |
| CIC^obs | `match` and `fix` with Coq's guard condition (PLT25 §4.1) | Coq's |
| Sirman et al. | Mendler-style induction: the recursive call is typed at an opaque type, so the fixed point is structurally decreasing by typing (SLK24 §2.2) | the same |
| Lean 4 | recursors only (the kernel generates them, Lean4/inductive.cpp) | structural and well-founded recursion, compiled to recursors outside the kernel (*not vendored*) |
| Size-change termination | Lee, Jones, Ben-Amram, POPL 2001, *not vendored* | #406 names it for the frontend |

## By the criteria

1. **Published with a proof.** Eliminators are what every normalisation proof
   here covers (PT22, PT23, PLT25 for ℕ). A guard condition or size-change
   check in the kernel is outside every proof surveyed.
2. **Computes.** Recursors compute by iota. Well-founded recursion through a
   proof-irrelevant `Acc` cannot compute, and its large elimination breaks
   normalisation (AC20); decision 2 already rules it out. Well-founded
   recursion needs `Acc` as a relevant inductive type in `@Type 1`, which the
   scheme allows.
3. **Fewest rules.** Recursors are generated from the declaration; nothing
   extra is trusted.
4. **Efficient implementation.** Lean's split, recursors in the kernel and
   compilation outside it, is the efficient design in production.
5. **Fits Ares.** The frontend already owns the surface language, where
   size-change analysis belongs.

## Decision record (proposed)

**The kernel trusts recursors only. The frontend elaborates recursive
definitions into recursors, and checks termination itself (size-change, as
#406 plans) before it does; the kernel re-checks the elaborated term.**
Well-founded recursion, where needed, goes through a relevant `Acc` in
`@Type 1`. Mendler-style induction (SLK24 §2.2) is the alternative if
elaborating to recursors proves too costly.
