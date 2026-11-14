import Proofs.Soundness
import Proofs.Hierarchy
import Proofs.Levels

/-!
# Consistency

The kernel's rules (#494, #495, #496, #497, #498) prove no `@Bottom` in the empty
context, over any well-formed signature of inductive declarations, for each
bound `N` on the universes. `Model.consistent` is the
theorem for any `N` that has a `Hierarchy`; the hierarchies are built one
Lean universe at a time, so each bound is its own instance. Four are given;
another is one more `.step`.

Levels here are numbers. A level-polymorphic declaration stands for its
instances, one per assignment of numbers to its prenex level parameters, and
`consistent_poly` is the theorem for every assignment.
-/

open ZFSet

theorem consistent₁ (sig : Sig) (hsig : SigOk 1 sig) (t : Tm) :
    ¬ Deriv 1 sig (.ty [] t .bot) :=
  Model.consistent hierarchy₁ hsig t

theorem consistent₂ (sig : Sig) (hsig : SigOk 2 sig) (t : Tm) :
    ¬ Deriv 2 sig (.ty [] t .bot) :=
  Model.consistent hierarchy₁.step hsig t

theorem consistent₃ (sig : Sig) (hsig : SigOk 3 sig) (t : Tm) :
    ¬ Deriv 3 sig (.ty [] t .bot) :=
  Model.consistent hierarchy₁.step.step hsig t

theorem consistent₄ (sig : Sig) (hsig : SigOk 4 sig) (t : Tm) :
    ¬ Deriv 4 sig (.ty [] t .bot) :=
  Model.consistent hierarchy₁.step.step.step hsig t

universe u in
-- Rule [Level-Polymorphism]
-- Rule [Eq-Stuck]: in an instance a level is a number, so Eq-Form or Eq-Box applies
/-- A level-polymorphic signature and term, given by their instance under each
assignment `σ` of numbers to the level parameters (`Lvl.eval σ` on each
level): no instance proves `@Bottom`. The kernel checks the polymorphic
declaration once, deciding each level premise with `Lvl.leq`, and
`Lvl.leq_sound` makes that premise true under every assignment, so it holds
in each instance as the number premise of a rule here. An equality the kernel
leaves stuck on a level parameter needs no rule: no instance has one. -/
theorem consistent_poly {N : Nat} (H : Hierarchy.{u} N) (sig : (Nat → Nat) → Sig)
    (t : (Nat → Nat) → Tm) : ∀ σ, SigOk N (sig σ) → ¬ Deriv N (sig σ) (.ty [] (t σ) .bot) :=
  fun σ hsig => Model.consistent H hsig (t σ)

/-- info: 'consistent₄' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms consistent₄

/-- info: 'Lvl.leq_sound' depends on axioms: [propext, Quot.sound] -/
#guard_msgs in
#print axioms Lvl.leq_sound
