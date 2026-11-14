# 4. Quotients

**Question:** how are quotient types formed, eliminated, and compared?

## Comparison

| System | Relation | Equality of classes | Elimination premise | Equality of quotient types |
|---|---|---|---|---|
| TT^obs | proof-irrelevant, proved an equivalence (PT22 §4.1) | `π(t) ∼ π(u)` computes to `R t u` (PT22 §4.1) | `t_π x ∼ cast(B π(y), B π(x), _, t_π y)` (PT22 §4.1) | carrier and relation (PT22 §4.1) |
| CIC^obs (Coq) | `A → A → SProp`, any relation (PLT25 §7.6) | characterised by the eliminator: the equivalence closure of `R` (PLT25 §7.6) | `cast _ _ (ap P (quo_eq a b H)) (Pquo a) ∼ Pquo b` (PLT25 §7.6) | carrier only, injective (PLT25 §7.6, `obseq_Quotient`) |
| sott | `A → A → Set` (sott README) | `same-class(e)` proof former (sott README) | a proof `e2` that the result does not depend on the class (sott README) | carrier and relation (sott `core/checker.ml:143-153`) |
| Sirman et al. | an equivalence (SLK24 §4.1) | computes to the relation (SLK24code/Eval.hs:217-218, Quotient-Proj-Eq) | as TT^obs (SLK24 §4.1) | carrier and relation (SLK24code/Eval.hs:195-198, Eq-Quotient) |
| Lean 4 | any `r` | `Quot.sound`, an axiom (Lean4/Prelude.lean:543) | `∀ a b, r a b → f a = f b` (Lean4/quot.cpp:83-91) | none |

## By the criteria

1. **Published with a proof.** TT^obs's quotients have a sketched proof
   (PT22 §4.1, "not formalized"); none of the others has more.
2. **Computes.** In the observational systems the equality of two classes is
   not an axiom, so casts along it compute. Lean's `Quot.sound` is an axiom,
   and funext is proved from it (Lean4/Prelude.lean:578), so a transport
   along a funext proof gets stuck: quotients are what break canonicity in
   Lean (PLT25 §7.6).
3. **Fewest rules.** Requiring an equivalence makes `π(t) ∼ π(u)` a single
   computation rule; CIC^obs instead needs no proof of equivalence but no
   computation rule either.
4. **Efficient implementation.** Sirman et al. implement TT^obs's form.
5. **Fits Ares.** The rationals milestone uses the computation: `1/2 ~ 2/4`
   holds by computation in `rationals.ott`.

## Decision record (proposed)

**TT^obs's quotients, as the kernel has them: the relation must be an
equivalence, `@qin a ~ @qin b` computes to `R a b`, and the eliminator's
respect premise is stated with a cast.** The kernel's premise casts from the
`x` side to the `y` side (PLT25 §7.6's direction), where TT^obs casts back;
the two are interderivable by symmetry. Equality of two quotient types stays
as TT^obs has it (carrier and relation). No change to the kernel; the ledger
rows stay `published-uncovered` under PT22 Quotients.
