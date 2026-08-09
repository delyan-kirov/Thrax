# Negative testing (needed, design deferred)

Every suite we have is a *positive* suite. `tests/MAIN.thx` imports each example,
runs its `test`, and counts faults. The C-backend suite checks that the emitted
program prints the same bytes as the interpreter. Both only ever exercise
programs that are *supposed to compile and run*. Nothing checks that a program we
expect to reject is actually rejected.

That gap is how the struct-literal bug shipped. A positional `.{1.1, 1, 1.1}`
against a 2-parameter struct over-applied (`Weirdtype Int Int Int`), and the
literal type-checked vacuously because:

1. Struct literals were inference-only. In checking position the expected type
   was ignored, so a bare `.{..}` could not learn which struct it built.
2. When the struct name could not be resolved, `infer_struct_lit` returned
   `self.eng.fresh()`. A fresh unification variable unifies with anything, so the
   compile error was silently deferred to a runtime fault.
3. Type application arity was never checked, so `Weirdtype Int Int Int` (three
   args to a two-param type) passed.

The fixes (bidirectional struct literals, an error instead of `fresh()` on
unresolved names, and `check_type_arity` for over-application) close *this* hole,
but they were found by hand. A positive suite structurally cannot catch the next
one: a soundness hole makes a bad program *pass*, and a passing program is
invisible to a suite that only asserts good programs pass.

## What we need

A negative suite: a set of programs each paired with the diagnostic (or at least
the error *code*) they must produce. A run fails if such a program compiles, or
fails with the wrong error. Candidates to seed it from the bug above:

- positional literal with wrong arity / wrong field count,
- bare `.{..}` where the struct cannot be inferred (no annotation, no qualifier),
- over-applied and under-applied type constructors,
- scalar/record promotion that should *not* fire (mismatched inner type).

## Deferred

The mechanism (file format, how expected diagnostics are pinned, how stable the
match has to be against message churn, whether it lives in the corpus or beside
the Rust tests) is **left to the user to design**. This note exists so the gap is
recorded, not so it is resolved here.
