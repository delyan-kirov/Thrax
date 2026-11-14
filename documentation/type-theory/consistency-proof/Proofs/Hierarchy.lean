import Proofs.Universes

/-!
# A hierarchy of universes

`Hierarchy.{N} N` exists for every concrete `N ≥ 1` (#494). Lean cannot
iterate over its own universe levels, so the construction is a base case and
a step, `Hierarchy.{u} N → Hierarchy.{u+1} (N+1)`, applied by hand: the
consistency theorem is a schema, one instance per bound.

The step lifts every universe of the old hierarchy one Lean universe up and
puts on top `univ.{u}`, the set of all sets of `ZFSet.{u}`. Both are classes
of lifted sets closed under the constructions, which is what `ofLifts` shows.
-/

universe u

namespace ZFSet

/-! ## Lifting -/

theorem lift_mk (y : PSet.{u}) : lift (mk y) = mk (PSet.lift y) := rfl

theorem lift_inj {a b : ZFSet.{u}} (h : lift a = lift b) : a = b := by
  induction a using Quotient.inductionOn with
  | h a =>
  induction b using Quotient.inductionOn with
  | h b => exact Quotient.sound (PSet.equiv_of_lift (Quotient.exact h))

theorem lift_mem_lift {y x : ZFSet.{u}} (h : y ∈ₛ x) : lift y ∈ₛ lift x := by
  induction x using Quotient.inductionOn with
  | h x =>
  induction y using Quotient.inductionOn with
  | h y => exact PSet.lift_mem_lift h

theorem mem_lift_iff {z : ZFSet.{u + 1}} {x : ZFSet.{u}} :
    z ∈ₛ lift x ↔ ∃ y, y ∈ₛ x ∧ z = lift y :=
  ⟨mem_lift, fun ⟨_, hy, e⟩ => e ▸ lift_mem_lift hy⟩

theorem mem_of_lift_mem {y x : ZFSet.{u}} (h : lift y ∈ₛ lift x) : y ∈ₛ x := by
  let ⟨_, hy, e⟩ := mem_lift h
  exact lift_inj e ▸ hy

theorem mem_univ {x : ZFSet.{u + 1}} : x ∈ₛ univ.{u} ↔ ∃ y, x = lift y := by
  unfold univ
  rw [mem_mk_iff]
  exact ⟨fun ⟨b, e⟩ => ⟨mk b, e⟩, fun ⟨y, e⟩ => by
    induction y using Quotient.inductionOn with
    | h y => exact ⟨y, e⟩⟩

theorem lift_empty : lift empty.{u} = empty := ext fun _ =>
  ⟨fun h => let ⟨_, hy, _⟩ := mem_lift h; absurd hy (not_mem_empty _),
   fun h => absurd h (not_mem_empty _)⟩

theorem lift_upair (a b : ZFSet.{u}) : lift (upair a b) = upair (lift a) (lift b) := ext fun z => by
  rw [mem_lift_iff, mem_upair]
  constructor
  · rintro ⟨y, hy, rfl⟩
    rcases mem_upair.1 hy with e | e <;> subst e
    · exact Or.inl rfl
    · exact Or.inr rfl
  · rintro (e | e) <;> subst e
    · exact ⟨a, mem_upair.2 (Or.inl rfl), rfl⟩
    · exact ⟨b, mem_upair.2 (Or.inr rfl), rfl⟩

/-- A subset of a lifted set is lifted. -/
theorem subset_lift {s : ZFSet.{u + 1}} {a : ZFSet.{u}} (h : ∀ w, w ∈ₛ s → w ∈ₛ lift a) :
    s = lift (sep (fun z => lift z ∈ₛ s) a) := ext fun w => by
  rw [mem_lift_iff]
  constructor
  · intro hw
    let ⟨z, hz, e⟩ := mem_lift (h w hw)
    exact ⟨z, mem_sep.2 ⟨hz, e ▸ hw⟩, e⟩
  · rintro ⟨z, hz, rfl⟩
    exact (mem_sep.1 hz).2

theorem lift_powerset (a : ZFSet.{u}) : powerset (lift a) = lift (powerset a) := ext fun s => by
  rw [mem_powerset, mem_lift_iff]
  constructor
  · intro h
    refine ⟨_, mem_powerset.2 fun z hz => (mem_sep.1 hz).1, subset_lift h⟩
  · rintro ⟨t, ht, rfl⟩ w hw
    let ⟨z, hz, e⟩ := mem_lift hw
    exact e ▸ lift_mem_lift (mem_powerset.1 ht z hz)

open Classical in
/-- The set a lifted set came from. -/
noncomputable def unlift (x : ZFSet.{u + 1}) : ZFSet.{u} :=
  if h : ∃ y, x = lift y then Classical.choose h else empty

theorem lift_unlift {x : ZFSet.{u + 1}} (h : ∃ y, x = lift y) : lift (unlift x) = x := by
  unfold unlift
  split
  next h' => exact (Classical.choose_spec h').symm
  next h' => exact absurd h h'

theorem unlift_lift (y : ZFSet.{u}) : unlift (lift y) = y :=
  lift_inj (lift_unlift ⟨y, rfl⟩)

theorem lift_union (I : ZFSet.{u}) (F : ZFSet.{u + 1} → ZFSet.{u + 1})
    (hF : ∀ i, i ∈ₛ I → ∃ y, F (lift i) = lift y) :
    sUnion (image F (lift I)) = lift (sUnion (image (fun i => unlift (F (lift i))) I)) :=
  ext fun z => by
    rw [mem_sUnion, mem_lift_iff]
    constructor
    · rintro ⟨s, hs, hz⟩
      let ⟨i', hi', e⟩ := mem_image.1 hs
      let ⟨i, hi, e'⟩ := mem_lift hi'
      subst e e'
      rw [← lift_unlift (hF i hi)] at hz
      let ⟨w, hw, e⟩ := mem_lift hz
      exact ⟨w, mem_sUnion.2 ⟨_, mem_image.2 ⟨i, hi, rfl⟩, hw⟩, e⟩
    · rintro ⟨w, hw, rfl⟩
      let ⟨s, hs, hw⟩ := mem_sUnion.1 hw
      let ⟨i, hi, e⟩ := mem_image.1 hs
      subst e
      refine ⟨F (lift i), mem_image.2 ⟨lift i, lift_mem_lift hi, rfl⟩, ?_⟩
      rw [← lift_unlift (hF i hi)]
      exact lift_mem_lift hw

theorem lift_nat : (n : Nat) → lift (nat.{u} n) = nat n
  | 0 => lift_empty
  | n + 1 => by
    show lift (upair (nat n) (nat n)) = upair (nat (n + 1 - 1)) (nat n)
    rw [lift_upair, lift_nat n]; rfl

theorem lift_omegaSet : lift omegaSet.{u} = omegaSet := ext fun z => by
  rw [mem_lift_iff]; unfold omegaSet; rw [mem_range]
  constructor
  · rintro ⟨y, hy, rfl⟩
    let ⟨n, e⟩ := mem_range.1 hy
    exact ⟨n, by rw [e, lift_nat]⟩
  · rintro ⟨n, rfl⟩
    exact ⟨nat n, mem_range.2 ⟨n, rfl⟩, (lift_nat n).symm⟩

/-! ## Classes of lifted sets -/

/-- A predicate closed under the constructions of `IsUniverse`. -/
structure Closed (P : ZFSet.{u} → Prop) : Prop where
  trans : ∀ x y, P x → y ∈ₛ x → P y
  empty_mem : P empty
  upair_mem : ∀ x y, P x → P y → P (upair x y)
  powerset_mem : ∀ x, P x → P (powerset x)
  union_mem : ∀ (I : ZFSet.{u}) (F : ZFSet.{u} → ZFSet.{u}),
    P I → (∀ i, i ∈ₛ I → P (F i)) → P (sUnion (image F I))
  omega_mem : P omegaSet

theorem IsUniverse.closed {G : ZFSet.{u}} (hG : IsUniverse G) : Closed (· ∈ₛ G) :=
  ⟨hG.trans, hG.empty_mem, hG.upair_mem, hG.powerset_mem, hG.union_mem, hG.omega_mem⟩

theorem closed_true : Closed fun _ : ZFSet.{u} => True :=
  ⟨fun _ _ _ _ => trivial, trivial, fun _ _ _ _ => trivial, fun _ _ => trivial,
   fun _ _ _ _ => trivial, trivial⟩

/-- The lifts of a closed class form a universe one level up. -/
theorem ofLifts {S : ZFSet.{u + 1}} {P : ZFSet.{u} → Prop} (hP : Closed P)
    (hS : ∀ x, x ∈ₛ S ↔ ∃ y, P y ∧ x = lift y) : IsUniverse S where
  trans x y hx hy := by
    let ⟨a, ha, e⟩ := (hS x).1 hx
    subst e
    let ⟨b, hb, e⟩ := mem_lift hy
    exact (hS y).2 ⟨b, hP.trans a b ha hb, e⟩
  empty_mem := (hS _).2 ⟨empty, hP.empty_mem, lift_empty.symm⟩
  upair_mem x y hx hy := by
    let ⟨a, ha, e⟩ := (hS x).1 hx
    let ⟨b, hb, e'⟩ := (hS y).1 hy
    subst e e'
    exact (hS _).2 ⟨_, hP.upair_mem a b ha hb, (lift_upair a b).symm⟩
  powerset_mem x hx := by
    let ⟨a, ha, e⟩ := (hS x).1 hx
    subst e
    exact (hS _).2 ⟨_, hP.powerset_mem a ha, lift_powerset a⟩
  union_mem I F hI hF := by
    let ⟨I₀, hI₀, e⟩ := (hS I).1 hI
    subst e
    have hF' : ∀ i, i ∈ₛ I₀ → ∃ y, F (lift i) = lift y := fun i hi =>
      let ⟨y, _, e⟩ := (hS _).1 (hF _ (lift_mem_lift hi)); ⟨y, e⟩
    rw [lift_union I₀ F hF']
    refine (hS _).2 ⟨_, hP.union_mem _ _ hI₀ fun i hi => ?_, rfl⟩
    let ⟨y, hy, e⟩ := (hS _).1 (hF _ (lift_mem_lift hi))
    rw [e, unlift_lift]
    exact hy
  omega_mem := (hS _).2 ⟨_, hP.omega_mem, lift_omegaSet.symm⟩

/-! ## The hierarchy -/

/-- One universe: every set of `ZFSet.{0}`. -/
noncomputable def hierarchy₁ : Hierarchy.{1} 1 where
  G _ := univ.{0}
  pos := by decide
  isUniverse _ _ := ofLifts closed_true fun _ => by
    rw [mem_univ]; exact ⟨fun ⟨y, e⟩ => ⟨y, trivial, e⟩, fun ⟨y, _, e⟩ => ⟨y, e⟩⟩
  chain i h := absurd h (by omega)

/-- The old universes lifted, with every set of the old level on top. -/
noncomputable def Hierarchy.step {N : Nat} (H : Hierarchy.{u} N) : Hierarchy.{u + 1} (N + 1) where
  G i := if i < N then lift (H.G i) else univ.{u}
  pos := by omega
  isUniverse i hi := by
    by_cases h : i < N
    · simp only [h, ↓reduceIte]
      exact ofLifts (H.isUniverse i h).closed fun _ => mem_lift_iff.trans
        ⟨fun ⟨y, hy, e⟩ => ⟨y, hy, e⟩, fun ⟨y, hy, e⟩ => ⟨y, hy, e⟩⟩
    · simp only [h, ↓reduceIte]
      exact ofLifts closed_true fun _ => by
        rw [mem_univ]; exact ⟨fun ⟨y, e⟩ => ⟨y, trivial, e⟩, fun ⟨y, _, e⟩ => ⟨y, e⟩⟩
  chain i hi := by
    by_cases h : i + 1 < N
    · simp only [h, Nat.lt_of_succ_lt h, ↓reduceIte]
      exact lift_mem_lift (H.chain i h)
    · have hi' : i < N := by omega
      simp only [h, hi', ↓reduceIte]
      exact mem_univ.2 ⟨_, rfl⟩

end ZFSet
