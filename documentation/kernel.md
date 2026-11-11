# The observational kernel

**Status:** prototype, plan step 1 of issue #406. A separate crate,
`crates/kernel`, that the frontend does not use yet.
**Date:** 2026-11-10.
**Scope:** what the kernel checks, the script language it reads, how it
computes, and where it departs from the published system.

---

## 1. Purpose

Issue #406 makes Ares dependently typed with observational equality, after
Pujet and Tabareau's TT^obs ("Observational Equality: Now for Good", POPL
2022; "Impredicative Observational Equality", POPL 2023). Step 1 of its plan is
a kernel that implements the theory on its own, so the rules can be tried and
tested before the frontend's checker grows into them (steps 2 to 4).

The milestone the plan sets is rationals over a user-defined binary `Integer`
with respectful quotient elimination. It is `crates/kernel/scripts/rationals.ott`.

## 2. Layout

| Module | Job |
|---|---|
| `level` | universe levels, `imax`, the case split that decides `<=` |
| `term` | core terms with de Bruijn indices, printing, shifting |
| `signature` | the checked globals and inductive declarations, and the axioms each rests on |
| `nbe` | evaluation to values and read-back (normalisation by evaluation) |
| `inductive` | constructor and recursor types, iota reduction |
| `observational` | the computation of `a ~ b` and `@cast` |
| `conversion` | typed definitional equality, cumulative subtyping |
| `syntax` | lexer and parser of the script language |
| `check` | bidirectional elaboration of scripts into core terms |

No external crates. Values are `Rc`-shared because closures capture the terms
they evaluate.

## 3. The script language

Scripts use the spelling #406 settled on, with every argument explicit:

```
$ id : @ctx {u: @l} -> (t: @Type u) -> t -> t = \t x => x
$ Nat : @Type 1 = @data | zero | succ Nat
$ add : Nat -> Nat -> Nat = \m n => Nat.elim @ctx {.m = 1} (\_ => Nat) n (\_ r => succ r) m
$ four : add (succ (succ zero)) (succ (succ zero)) ~ succ (succ (succ (succ zero))) = @refl four
```

- `$ name : T = e` defines, `$ name : T` postulates, `$ D : (params) -> @Type l = @data | c fields | ...` declares an inductive type.
- `@ctx {u: @l, ...} ->` at the start of a signature binds levels; a use passes them as `name @ctx {.u = l}`. Kernel levels range over every level, `0` included; the surface default `n >= 1` is a kernel `k + 1`.
- Terms: `\x => e`, `\(x: A) => e`, `(x: A) -> B`, `A -> B`, `(x: A) * B`, `(a, b)`, `p.1`, `p.2`, `let x: T = e in b`, `(e : T)`, `a ~ b`.
- Primitives: `@Type l`, `@Top`, `@tt`, `@Bottom`, `@absurd A e`, `@Eq A a b`, `@refl a`, `@transp t B u t' e`, `@cast A B e t`, `@Quot A R rr rs rt`, `@qin a`, `@qelim P f resp q`.
- `#` starts a comment.

## 4. The theory as implemented

- **Universes.** `@Type 0` is the impredicative universe of propositions; the sort of a Pi type is `imax`. Cumulativity starts at level 1: `@Type 1 <= @Type 2`, but `@Type 0` is not below `@Type 1`.
- **Proof irrelevance.** Conversion is typed. Two values of a proposition are equal, decided from the type. Proofs evaluate to one irrelevant witness (Note [Proofs evaluate to an irrelevant witness]).
- **Pi and Sigma** with eta. A Sigma of data and a proof is data (the shape a refinement desugars to).
- **Inductive types**, parameterised, in data universes, with fields that may depend on earlier fields. Recursive fields are whole `D params` only. Each declaration adds the type former, its constructors and `D.elim`.
- **Equality** `a ~ b` computes by its type: pointwise at Pi types (function extensionality), componentwise at Sigma types, constructor-wise at inductive types, structurally at universes (contravariant Pi domains), as the relation at quotients, as logical equivalence at `@Type 0`, and as `@Top` at a proposition.
- **Cast** computes by the heads of its two types, through lambdas, pairs, constructors and classes.
- **Quotients** of data by a relation proved reflexive, symmetric and transitive. `@qelim` needs the method to respect the relation.

## 5. Departures and gaps

Every rule the kernel implements, where it comes from, what theorem covers it
and where it departs, is in the theory ledger,
[`documentation/theory/LEDGER.md`](theory/LEDGER.md) (#472). The code tags
each rule's implementation with `// Rule [Name]`, and
`crates/kernel/tests/ledger.rs` keeps the two in step. The main departures:

- `@cast A B e t` reduces to `t` when `A` and `B` are the same value (Note [A cast between identical types is the identity]). TT^obs reaches `t` only through its structure; without the rule the outer respect proof of a binary quotient operation meets a stuck cast. CIC^obs has the rule in conversion, not evaluation (#464).
- Equality at an inductive type whose parameters depend on earlier parameters is left stuck.
- Indexed families are not primitive (TT^obs encodes them by fording). Nested and infinitely branching recursive fields are rejected.
- Definitions unfold eagerly, so error messages show unfolded terms. Glued evaluation would keep the names.
- No metavariables or implicit arguments: the kernel checks elaborated terms, and every argument is written.

## 5.1 Axioms

`kernel::axioms` lists the postulates a global rests on, transitively and
through proofs (Lean's `#print axioms`). A script pins them with a
`# axioms: a b` line above each declaration that rests on any, which
`tests/scripts.rs` checks, so a new dependence on a postulate fails a test. `rationals.ott` rests on `mul_comm`,
`mul_assoc` and `mul_cancel` (#463).

## 6. Next steps

Plan step 2 merges the type and expression grammars of `documentation/ares.y`
without changing meaning. Step 3 grows the frontend's Algorithm W into a
bidirectional elaborator with NbE conversion, for which this kernel is the
reference.
