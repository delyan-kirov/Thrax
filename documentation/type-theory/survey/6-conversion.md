# 6. Conversion

**Question:** how does the kernel decide definitional equality, and how does it
control unfolding?

## Comparison

| System | Algorithm | Proof irrelevance | Cast-Refl | Unfolding |
|---|---|---|---|---|
| TT^obs, CC^obs | typed whnf comparison, proved complete by a logical relation (PT22 §3.4) | typed: at a type in Ω (PT22 Fig. 3) | none | n/a |
| CIC^obs | algorithmic conversion: whnf, congruence, η, and Cast-refl-L/R with bounded backtracking (PLT25 Fig. 7, §5.2, §5.4) | Proof-Irr on neutrals (PLT25 §5.2) | Cast-refl-L, Cast-refl-R (PLT25 §5.2) | n/a |
| Sirman et al. | untyped NbE, η when one side is a constructor (SLK24 §3.4) | applications annotated with the argument's sort; irrelevant arguments skipped (SLK24 §3.4) | Cast-Eq-Left/Right/Conv with backtracking (SLK24code/Conversion.hs:216-243) | eager |
| Lean 4 | whnf with lazy delta reduction: unfold the side with the greater definition height first (Lean4/type_checker.cpp:974-1063) | by type | n/a | lazy, by height (Lean4/type_checker.cpp:1001) |
| smalltt | NbE with glued evaluation: a global is evaluated lazily to both its unfolding and its name (smalltt/README.md:223-250) | n/a | n/a | lazy, both branches shared (smalltt/README.md:236-250) |

## By the criteria

1. **Published with a proof.** CIC^obs's algorithmic conversion is proved
   decidable and complete with Cast-Refl (PLT25 Theorems 5.3, 5.5, 5.6), for
   its fragment.
2. **Computes.** All compute; the difference is cost.
3. **Fewest rules.** NbE folds reduction and comparison into one evaluator,
   which is what the kernel already has.
4. **Efficient implementation.** Glued evaluation (smalltt) keeps terms small
   for error messages and avoids unfolding; Lean's definition heights pick
   which side to unfold.
5. **Fits Ares.** Error messages showing folded definitions is #469.

## Decision record (proposed)

**Keep typed conversion over NbE values, and add Cast-refl-L and Cast-refl-R
as in PLT25 Fig. 7 (with SLK24code/Conversion.hs:216-243 as the
implementation to follow), with the bounded backtracking of PLT25 §5.4.**
Then glued evaluation of globals as smalltt does it (#469). Lean's lazy delta
by definition height is the next step if glued evaluation alone is too slow.
Typed conversion stays because proof irrelevance is decided by the type (Note
[Proof irrelevance is decided by the type]); SLK24's sort annotations are the
alternative if the kernel goes untyped.
