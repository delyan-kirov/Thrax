import Proofs.Families
import Proofs.Typing

/-!
# The interpretation

Every term is interpreted as one set, in an environment giving a set for each
de Bruijn variable (#494). The interpretation is total: a term that does not
type-check still means some set, so the substitution lemmas below need no
side conditions. Typing is what makes the meaning the intended one
(`Proofs/Soundness.lean`).

* A type is a code (`Proofs/Universes.lean`). A Pi or Sigma type into a
  proposition is a proposition, so it is `propCode` of the formula it states.
* A proof is `∅`, the kernel's `Irrelevant`; applying or projecting `∅` gives
  `∅` again, which is why `app` and the projections need no sort annotation.
* `@cast` is the identity: a cast is only well typed along an equality, and
  equal types are the same code.
* A `let` is its body with the variable bound to the value's meaning (#495).
-/

universe u

namespace Model

open ZFSet

/-- `x` for variable 0, the environment `ρ` for the rest. -/
def cons (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) : Nat → ZFSet.{u}
  | 0 => x
  | n + 1 => ρ n

variable {N : Nat} (H : Hierarchy.{u} N) (S : Glob → ZFSet.{u})

noncomputable def interp : Tm → (Nat → ZFSet.{u}) → ZFSet.{u}
  | .var i, ρ => ρ i
  | .sort l, _ => univCode l (H.U l)
  | .pi l l' A B, ρ =>
    if l' = 0 then propCode (∀ x, x ∈ₛ El (interp A ρ) → holds (interp B (cons x ρ)))
    else piCode l l' (interp A ρ) (lam (El (interp A ρ)) fun x => interp B (cons x ρ))
  | .lam A t, ρ => lam (El (interp A ρ)) fun x => interp t (cons x ρ)
  | .app f a, ρ => app (interp f ρ) (interp a ρ)
  | .letIn _ v t, ρ => interp t (cons (interp v ρ) ρ)
  | .sigma l l' A B, ρ =>
    if l = 0 ∧ l' = 0 then propCode (holds (interp A ρ) ∧ holds (interp B (cons empty ρ)))
    else sigmaCode l l' (interp A ρ) (lam (El (interp A ρ)) fun x => interp B (cons x ρ))
  | .pair a b, ρ => pair (interp a ρ) (interp b ρ)
  | .fst p, ρ => fst (interp p ρ)
  | .snd p, ρ => snd (interp p ρ)
  | .top, _ => propCode True
  | .tt, _ => empty
  | .bot, _ => propCode False
  | .absurd _ _, _ => empty
  | .eq _ _ a b, ρ => propCode (interp a ρ = interp b ρ)
  | .cast _ _ _ t, ρ => interp t ρ
  | .irr, _ => empty
  | .glob g, _ => S g
  | .quot l A R, ρ => quotCode l (interp A ρ) (interp R ρ)
  | .qin A R a, ρ => cls (interp A ρ) (interp R ρ) (interp a ρ)
  | .qelim _ _ f q, ρ => app (interp f ρ) (rep (interp q ρ))

/-! ## Renaming and substitution -/

theorem cons_up (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) (ξ : Nat → Nat) :
    (fun n => cons x ρ (Tm.up ξ n)) = cons x (fun n => ρ (ξ n)) := by
  funext n; cases n <;> rfl

theorem interp_rename (t : Tm) (ξ : Nat → Nat) (ρ : Nat → ZFSet.{u}) :
    interp H S (t.rename ξ) ρ = interp H S t (fun n => ρ (ξ n)) := by
  induction t generalizing ξ ρ with
  | var i => rfl
  | sort l => rfl
  | pi l l' A B ihA ihB => simp only [Tm.rename, interp, ihA, ihB, cons_up]
  | lam A t ihA iht => simp only [Tm.rename, interp, ihA, iht, cons_up]
  | app f a ihf iha => simp only [Tm.rename, interp, ihf, iha]
  | letIn A v t _ ihv iht => simp only [Tm.rename, interp, ihv, iht, cons_up]
  | sigma l l' A B ihA ihB => simp only [Tm.rename, interp, ihA, ihB, cons_up]
  | pair a b iha ihb => simp only [Tm.rename, interp, iha, ihb]
  | fst p ih => simp only [Tm.rename, interp, ih]
  | snd p ih => simp only [Tm.rename, interp, ih]
  | top => rfl
  | tt => rfl
  | bot => rfl
  | absurd A p _ _ => rfl
  | eq l A a b _ iha ihb => simp only [Tm.rename, interp, iha, ihb]
  | cast A B e t _ _ _ iht => simp only [Tm.rename, interp, iht]
  | irr => rfl
  | glob g => rfl
  | quot l A R ihA ihR => simp only [Tm.rename, interp, ihA, ihR]
  | qin A R a ihA ihR iha => simp only [Tm.rename, interp, ihA, ihR, iha]
  | qelim m P f q _ ihf ihq => simp only [Tm.rename, interp, ihf, ihq]

theorem interp_shift (t : Tm) (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) :
    interp H S t.shift (cons x ρ) = interp H S t ρ :=
  interp_rename H S t _ _

theorem cons_upS (σ : Nat → Tm) (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) :
    (fun n => interp H S (Tm.upS σ n) (cons x ρ)) = cons x (fun n => interp H S (σ n) ρ) := by
  funext n
  cases n with
  | zero => rfl
  | succ n => exact interp_shift H S (σ n) x ρ

theorem interp_subst (t : Tm) (σ : Nat → Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (t.subst σ) ρ = interp H S t (fun n => interp H S (σ n) ρ) := by
  induction t generalizing σ ρ with
  | var i => rfl
  | sort l => rfl
  | pi l l' A B ihA ihB => simp only [Tm.subst, interp, ihA, ihB, cons_upS]
  | lam A t ihA iht => simp only [Tm.subst, interp, ihA, iht, cons_upS]
  | app f a ihf iha => simp only [Tm.subst, interp, ihf, iha]
  | letIn A v t _ ihv iht => simp only [Tm.subst, interp, ihv, iht, cons_upS]
  | sigma l l' A B ihA ihB => simp only [Tm.subst, interp, ihA, ihB, cons_upS]
  | pair a b iha ihb => simp only [Tm.subst, interp, iha, ihb]
  | fst p ih => simp only [Tm.subst, interp, ih]
  | snd p ih => simp only [Tm.subst, interp, ih]
  | top => rfl
  | tt => rfl
  | bot => rfl
  | absurd A p _ _ => rfl
  | eq l A a b _ iha ihb => simp only [Tm.subst, interp, iha, ihb]
  | cast A B e t _ _ _ iht => simp only [Tm.subst, interp, iht]
  | irr => rfl
  | glob g => rfl
  | quot l A R ihA ihR => simp only [Tm.subst, interp, ihA, ihR]
  | qin A R a ihA ihR iha => simp only [Tm.subst, interp, ihA, ihR, iha]
  | qelim m P f q _ ihf ihq => simp only [Tm.subst, interp, ihf, ihq]

theorem interp_inst (B a : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (B.inst a) ρ = interp H S B (cons (interp H S a ρ) ρ) := by
  rw [Tm.inst, interp_subst]
  congr 1; funext n; cases n <;> rfl

theorem interp_under (B t : Tm) (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) :
    interp H S (B.subst (Tm.under t)) (cons x ρ) = interp H S B (cons (interp H S t (cons x ρ)) ρ) := by
  rw [interp_subst]
  congr 1; funext n; cases n <;> rfl

/-! ## Environments -/

/-- `ρ` gives each variable of `Γ` an element of its type. -/
def EnvOk : List Tm → (Nat → ZFSet.{u}) → Prop
  | [], _ => True
  | A :: Γ, ρ => EnvOk Γ (fun n => ρ (n + 1)) ∧ ρ 0 ∈ₛ El (interp H S A fun n => ρ (n + 1))

theorem envOk_cons {Γ : List Tm} {A : Tm} {x : ZFSet.{u}} {ρ : Nat → ZFSet.{u}} :
    EnvOk H S (A :: Γ) (cons x ρ) ↔ EnvOk H S Γ ρ ∧ x ∈ₛ El (interp H S A ρ) := Iff.rfl

theorem envOk_get : {Γ : List Tm} → {ρ : Nat → ZFSet.{u}} → {i : Nat} → {A : Tm} →
    EnvOk H S Γ ρ → Γ[i]? = some A → ρ i ∈ₛ El (interp H S A fun n => ρ (n + (i + 1)))
  | [], _, _, _, _, h => by simp at h
  | B :: Γ, ρ, 0, A, ⟨_, hx⟩, h => by
    simp only [List.getElem?_cons_zero, Option.some.injEq] at h
    subst h; exact hx
  | B :: Γ, ρ, i + 1, A, ⟨hΓ, _⟩, h => by
    simp only [List.getElem?_cons_succ] at h
    have := envOk_get hΓ h
    simp only [Nat.add_assoc] at this ⊢
    exact this

/-! ## Lists of values -/

/-- The environment of a list of values, the newest first; `∅` past them,
the meaning of `irr`. -/
def envOf (vs : List ZFSet.{u}) (i : Nat) : ZFSet.{u} := vs[i]?.getD empty

theorem cons_envOf (v : ZFSet.{u}) (vs : List ZFSet.{u}) : cons v (envOf vs) = envOf (v :: vs) := by
  funext n; cases n <;> rfl

theorem interp_apps (f : Tm) (ts : List Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (Tm.apps f ts) ρ = ZFSet.apps (interp H S f ρ) (ts.map (interp H S · ρ)) := by
  induction ts generalizing f with
  | nil => rfl
  | cons t ts ih => exact ih (.app f t)

theorem interp_substL (ts : List Tm) (A : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (Tm.substL ts A) ρ = interp H S A (envOf (ts.map (interp H S · ρ)).reverse) := by
  rw [Tm.substL, interp_subst]
  congr 1; funext i
  simp only [envOf, ← List.map_reverse, List.getElem?_map]
  cases ts.reverse[i]? <;> rfl

/-- A renaming that moves an environment to another, exactly. -/
theorem interp_rename_env {t : Tm} {ξ : Nat → Nat} {ρ ρ' : Nat → ZFSet.{u}}
    (h : ∀ n, ρ (ξ n) = ρ' n) : interp H S (t.rename ξ) ρ = interp H S t ρ' := by
  rw [interp_rename]; congr 1; funext n; exact h n

theorem envOf_liftAt {L M R : List ZFSet.{u}} (n : Nat) :
    envOf (L ++ M ++ R) (Tm.liftAt L.length M.length n) = envOf (L ++ R) n := by
  unfold Tm.liftAt envOf
  split
  next h => simp [List.getElem?_append_left h]
  next h =>
    rw [List.getElem?_append_right (by simp; omega), List.getElem?_append_right (by omega)]
    congr 2; simp; omega

theorem interp_lift {t : Tm} {L M R : List ZFSet.{u}} :
    interp H S (Tm.lift L.length M.length t) (envOf (L ++ M ++ R)) = interp H S t (envOf (L ++ R)) :=
  interp_rename_env H S envOf_liftAt

/-! ## Irrelevant variables -/

theorem interp_occurs (t : Tm) {ρ ρ' : Nat → ZFSet.{u}} (h : ∀ i, t.occurs i → ρ i = ρ' i) :
    interp H S t ρ = interp H S t ρ' := by
  have under : ∀ {t : Tm} {ρ ρ' : Nat → ZFSet.{u}}, (∀ i, t.occurs (i + 1) → ρ i = ρ' i) →
      ∀ x, ∀ i, t.occurs i → cons x ρ i = cons x ρ' i := by
    intro t ρ ρ' h x i hi
    cases i with
    | zero => rfl
    | succ i => exact h i hi
  induction t generalizing ρ ρ' with
  | var j => exact h j (by simp [Tm.occurs])
  | sort l => rfl
  | pi l l' A B ihA ihB =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    have hA := ihA fun i hi => h i (Or.inl hi)
    have hB := fun x => ihB (under (fun i hi => h i (Or.inr hi)) x)
    simp only [interp, hA, hB]
  | lam A t ihA iht =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    have hA := ihA fun i hi => h i (Or.inl hi)
    have ht := fun x => iht (under (fun i hi => h i (Or.inr hi)) x)
    simp only [interp, hA, ht]
  | app f a ihf iha =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, ihf fun i hi => h i (Or.inl hi), iha fun i hi => h i (Or.inr hi)]
  | letIn A v t _ ihv iht =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    have hv := ihv fun i hi => h i (Or.inl (Or.inr hi))
    show interp H S t (cons (interp H S v ρ) ρ) = interp H S t (cons (interp H S v ρ') ρ')
    rw [hv]
    exact iht (under (fun i hi => h i (Or.inr hi)) _)
  | sigma l l' A B ihA ihB =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    have hA := ihA fun i hi => h i (Or.inl hi)
    have hB := fun x => ihB (under (fun i hi => h i (Or.inr hi)) x)
    simp only [interp, hA, hB]
  | pair a b iha ihb =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, iha fun i hi => h i (Or.inl hi), ihb fun i hi => h i (Or.inr hi)]
  | fst p ih => simp only [interp, ih fun i hi => h i (by simpa [Tm.occurs] using hi)]
  | snd p ih => simp only [interp, ih fun i hi => h i (by simpa [Tm.occurs] using hi)]
  | top => rfl
  | tt => rfl
  | bot => rfl
  | absurd A p _ _ => rfl
  | eq l A a b _ iha ihb =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, iha fun i hi => h i (Or.inl (Or.inr hi)), ihb fun i hi => h i (Or.inr hi)]
  | cast A B e t _ _ _ iht =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    exact iht fun i hi => h i (Or.inr hi)
  | irr => rfl
  | glob g => rfl
  | quot l A R ihA ihR =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, ihA fun i hi => h i (Or.inl hi), ihR fun i hi => h i (Or.inr hi)]
  | qin A R a ihA ihR iha =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, ihA fun i hi => h i (Or.inl (Or.inl hi)),
      ihR fun i hi => h i (Or.inl (Or.inr hi)), iha fun i hi => h i (Or.inr hi)]
  | qelim m P f q _ ihf ihq =>
    simp only [Tm.occurs, Bool.or_eq_true] at h
    simp only [interp, ihf fun i hi => h i (Or.inl (Or.inr hi)), ihq fun i hi => h i (Or.inr hi)]

/-! ## Earlier globals -/

theorem interp_globs {k : Nat} {S' : Glob → ZFSet.{u}} (hS : ∀ g, g.decl < k → S g = S' g)
    (t : Tm) (ht : t.globsBelow k) (ρ : Nat → ZFSet.{u}) : interp H S t ρ = interp H S' t ρ := by
  induction t generalizing ρ with
  | var j => rfl
  | sort l => rfl
  | pi l l' A B ihA ihB => simp only [interp, ihA ht.1, ihB ht.2]
  | lam A t ihA iht => simp only [interp, ihA ht.1, iht ht.2]
  | app f a ihf iha => simp only [interp, ihf ht.1, iha ht.2]
  | letIn A v t _ ihv iht => simp only [interp, ihv ht.2.1, iht ht.2.2]
  | sigma l l' A B ihA ihB => simp only [interp, ihA ht.1, ihB ht.2]
  | pair a b iha ihb => simp only [interp, iha ht.1, ihb ht.2]
  | fst p ih => simp only [interp, ih ht]
  | snd p ih => simp only [interp, ih ht]
  | top => rfl
  | tt => rfl
  | bot => rfl
  | absurd A p _ _ => rfl
  | eq l A a b _ iha ihb => simp only [interp, iha ht.2.1, ihb ht.2.2]
  | cast A B e t _ _ _ iht => exact iht ht.2.2.2 ρ
  | irr => rfl
  | glob g => exact hS g ht
  | quot l A R ihA ihR => simp only [interp, ihA ht.1, ihR ht.2]
  | qin A R a ihA ihR iha => simp only [interp, ihA ht.1, ihR ht.2.1, iha ht.2.2]
  | qelim m P f q _ ihf ihq => simp only [interp, ihf ht.2.1, ihq ht.2.2]

end Model
