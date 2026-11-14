/-!
# Sets

Aczel's sets in type theory: the domain of the kernel's model (#481).

The model needs a cumulative hierarchy, `@Type i ⊆ @Type (i+1)` as an
inclusion, because the kernel's universe subtyping inserts no coercion term.
Lean's own universes are not cumulative (`ULift A` is not `A`), so types cannot
be interpreted as Lean types. Interpreting every term as one set, in one Lean
universe large enough for all the kernel's levels, makes the inclusion literal.

A pre-set is a family of pre-sets indexed by a type; two are equivalent when
each member of one is equivalent to some member of the other. `ZFSet` is the
quotient, so equal sets are equal in Lean and conversion can be read as `=`.
-/

universe u

/-- A set, given as the family of its members. -/
inductive PSet : Type (u + 1) where
  | mk (α : Type u) (f : α → PSet) : PSet

namespace PSet

/-- Extensional equivalence. -/
def Equiv : PSet.{u} → PSet.{u} → Prop
  | mk _ f, mk _ g => (∀ a, ∃ b, Equiv (f a) (g b)) ∧ (∀ b, ∃ a, Equiv (f a) (g b))

theorem Equiv.refl : (x : PSet.{u}) → Equiv x x
  | mk _ f => ⟨fun a => ⟨a, Equiv.refl (f a)⟩, fun b => ⟨b, Equiv.refl (f b)⟩⟩

theorem Equiv.symm : {x y : PSet.{u}} → Equiv x y → Equiv y x
  | mk _ _, mk _ _, ⟨h₁, h₂⟩ =>
    ⟨fun b => let ⟨a, e⟩ := h₂ b; ⟨a, e.symm⟩, fun a => let ⟨b, e⟩ := h₁ a; ⟨b, e.symm⟩⟩

theorem Equiv.trans : {x y z : PSet.{u}} → Equiv x y → Equiv y z → Equiv x z
  | mk _ _, mk _ _, mk _ _, ⟨xy₁, xy₂⟩, ⟨yz₁, yz₂⟩ =>
    ⟨fun a => let ⟨b, e₁⟩ := xy₁ a; let ⟨c, e₂⟩ := yz₁ b; ⟨c, e₁.trans e₂⟩,
     fun c => let ⟨b, e₂⟩ := yz₂ c; let ⟨a, e₁⟩ := xy₂ b; ⟨a, e₁.trans e₂⟩⟩

/-- `x` is a member of `y`, up to equivalence. -/
def Mem (x : PSet.{u}) : PSet.{u} → Prop
  | mk _ g => ∃ b, Equiv x (g b)

theorem mem_mk {α : Type u} (f : α → PSet.{u}) (a : α) : Mem (f a) (mk α f) :=
  ⟨a, Equiv.refl _⟩

theorem Mem.left {x x' y : PSet.{u}} (e : Equiv x x') : Mem x y → Mem x' y := by
  cases y with
  | mk _ g => exact fun ⟨b, h⟩ => ⟨b, e.symm.trans h⟩

theorem Mem.right {x y y' : PSet.{u}} (e : Equiv y y') : Mem x y → Mem x y' := by
  cases y with
  | mk _ g =>
    cases y' with
    | mk _ g' => exact fun ⟨b, h⟩ => let ⟨b', h'⟩ := e.1 b; ⟨b', h.trans h'⟩

/-- Sets with the same members are equivalent. -/
theorem ext {x y : PSet.{u}} (h : ∀ z, Mem z x ↔ Mem z y) : Equiv x y := by
  cases x with
  | mk _ f =>
    cases y with
    | mk _ g =>
      exact ⟨fun a => (h (f a)).1 (mem_mk f a),
             fun b => let ⟨a, e⟩ := (h (g b)).2 (mem_mk g b); ⟨a, e.symm⟩⟩

instance setoid : Setoid PSet.{u} := ⟨Equiv, ⟨Equiv.refl, Equiv.symm, Equiv.trans⟩⟩

/-- The same set one Lean universe up. -/
def lift : PSet.{u} → PSet.{u + 1}
  | mk α f => mk (ULift.{u + 1} α) (fun a => lift (f a.down))

theorem lift_equiv : {x y : PSet.{u}} → Equiv x y → Equiv (lift x) (lift y)
  | mk _ _, mk _ _, ⟨h₁, h₂⟩ => by
    unfold lift
    exact ⟨fun a => let ⟨b, e⟩ := h₁ a.down; ⟨⟨b⟩, lift_equiv e⟩,
           fun b => let ⟨a, e⟩ := h₂ b.down; ⟨⟨a⟩, lift_equiv e⟩⟩

/-- Lifting reflects equivalence. -/
theorem equiv_of_lift : {x y : PSet.{u}} → Equiv (lift x) (lift y) → Equiv x y
  | mk _ _, mk _ _, h => by
    unfold lift at h
    exact ⟨fun a => let ⟨b, e⟩ := h.1 ⟨a⟩; ⟨b.down, equiv_of_lift e⟩,
           fun b => let ⟨a, e⟩ := h.2 ⟨b⟩; ⟨a.down, equiv_of_lift e⟩⟩

theorem lift_mem_lift {y : PSet.{u}} : {x : PSet.{u}} → Mem y x → Mem (lift y) (lift x)
  | mk α f, ⟨b, e⟩ =>
    show Mem (lift y) (mk (ULift.{u + 1} α) fun a => lift (f a.down)) from ⟨⟨b⟩, lift_equiv e⟩

/-- Every set of the universe below, lifted: the next Grothendieck universe's
seed. It is a set one level up because `PSet.{u} : Type (u+1)`. -/
def univ : PSet.{u + 1} := mk PSet.{u} lift

theorem lift_mem_univ (x : PSet.{u}) : Mem (lift x) univ.{u} := mem_mk lift x

/-- A member of a lifted set is a lifted member. -/
theorem mem_lift {z : PSet.{u + 1}} : {x : PSet.{u}} → Mem z (lift x) →
    ∃ y, Mem y x ∧ Equiv z (lift y)
  | mk _ f, ⟨b, e⟩ => ⟨f b.down, mem_mk f b.down, e⟩

end PSet

/-- Sets: pre-sets up to extensional equivalence. -/
def ZFSet : Type (u + 1) := Quotient PSet.setoid.{u}

namespace ZFSet

def mk : PSet.{u} → ZFSet.{u} := Quotient.mk _

def Mem : ZFSet.{u} → ZFSet.{u} → Prop :=
  Quotient.lift₂ PSet.Mem fun _ _ _ _ ex ey =>
    propext ⟨fun h => (PSet.Mem.left ex h).right ey,
             fun h => (PSet.Mem.left ex.symm h).right ey.symm⟩

infix:50 " ∈ₛ " => Mem

theorem mem_mk (x y : PSet.{u}) : mk x ∈ₛ mk y ↔ PSet.Mem x y := Iff.rfl

theorem ext {x y : ZFSet.{u}} (h : ∀ z, z ∈ₛ x ↔ z ∈ₛ y) : x = y := by
  induction x using Quotient.inductionOn with
  | h x =>
    induction y using Quotient.inductionOn with
    | h y => exact Quotient.sound (PSet.ext fun z => h (mk z))

/-! ## Finite sets -/

def empty : ZFSet.{u} := mk (PSet.mk PEmpty PEmpty.elim)

theorem not_mem_empty (z : ZFSet.{u}) : ¬ z ∈ₛ empty := by
  induction z using Quotient.inductionOn with
  | h z => exact fun ⟨b, _⟩ => b.elim

/-- `{x, y}`. -/
def upair (x y : ZFSet.{u}) : ZFSet.{u} :=
  Quotient.liftOn₂ x y
    (fun x y => mk (PSet.mk (ULift Bool) fun b => if b.down then x else y))
    fun x y x' y' ex ey => by
      apply Quotient.sound
      exact ⟨fun b => ⟨b, by cases b with | up b => cases b <;> assumption⟩,
             fun b => ⟨b, by cases b with | up b => cases b <;> assumption⟩⟩

theorem mem_upair {x y z : ZFSet.{u}} : z ∈ₛ upair x y ↔ z = x ∨ z = y := by
  induction x using Quotient.inductionOn with
  | h x =>
  induction y using Quotient.inductionOn with
  | h y =>
  induction z using Quotient.inductionOn with
  | h z =>
    constructor
    · intro ⟨⟨b⟩, e⟩
      cases b with
      | true => exact Or.inl (Quotient.sound e)
      | false => exact Or.inr (Quotient.sound e)
    · intro h
      cases h with
      | inl e => exact ⟨⟨true⟩, Quotient.exact e⟩
      | inr e => exact ⟨⟨false⟩, Quotient.exact e⟩

def singleton (x : ZFSet.{u}) : ZFSet.{u} := upair x x

theorem mem_singleton {x z : ZFSet.{u}} : z ∈ₛ singleton x ↔ z = x := by
  unfold singleton; rw [mem_upair]; exact ⟨fun h => h.elim id id, Or.inl⟩

theorem singleton_inj {x y : ZFSet.{u}} (h : singleton x = singleton y) : x = y :=
  mem_singleton.1 (h ▸ mem_singleton.2 rfl)

theorem upair_eq_singleton {x y z : ZFSet.{u}} (h : upair x y = singleton z) :
    x = z ∧ y = z :=
  ⟨mem_singleton.1 (h ▸ mem_upair.2 (Or.inl rfl)),
   mem_singleton.1 (h ▸ mem_upair.2 (Or.inr rfl))⟩

/-! ## Ordered pairs

A type's code is a tuple whose first component names its former, so the type
formers are injective and distinct: what Eq-Pi and Eq-Univ-Neq need. -/

/-- Kuratowski's pair `{{x}, {x, y}}`. -/
def pair (x y : ZFSet.{u}) : ZFSet.{u} := upair (singleton x) (upair x y)

theorem pair_inj {a b c d : ZFSet.{u}} (h : pair a b = pair c d) : a = c ∧ b = d := by
  -- `{a}` is `{c}` or `{c, d}`; either way `a = c`.
  have hac : a = c := by
    have : singleton a ∈ₛ pair c d := h ▸ mem_upair.2 (Or.inl rfl)
    cases mem_upair.1 this with
    | inl e => exact singleton_inj e
    | inr e => exact ((upair_eq_singleton e.symm).1).symm
  subst hac
  refine ⟨rfl, ?_⟩
  -- `{a, d}` is `{a}` (so `d = a`) or `{a, b}`.
  have hd : d = a ∨ d = b := by
    have : upair a d ∈ₛ pair a b := h ▸ mem_upair.2 (Or.inr rfl)
    cases mem_upair.1 this with
    | inl e => exact Or.inl ((upair_eq_singleton e).2)
    | inr e => exact mem_upair.1 (e ▸ mem_upair.2 (Or.inr rfl))
  -- `{a, b}` is `{a}` (so `b = a`) or `{a, d}`.
  have hb : b = a ∨ b = d := by
    have : upair a b ∈ₛ pair a d := h ▸ mem_upair.2 (Or.inr rfl)
    cases mem_upair.1 this with
    | inl e => exact Or.inl ((upair_eq_singleton e).2)
    | inr e => exact mem_upair.1 (e ▸ mem_upair.2 (Or.inr rfl))
  cases hb with
  | inr e => exact e
  | inl e =>
    cases hd with
    | inl e' => exact e.trans e'.symm
    | inr e' => exact e'.symm

/-! ## Universes

`univ.{u}` lives in `ZFSet.{u+1}` and holds every set of `ZFSet.{u}`, lifted.
A hierarchy of `n` kernel universes is interpreted inside `ZFSet.{n}`, each
level's universe lifted to the top; the inclusion between them is
`lift_mem_univ`. -/

def lift : ZFSet.{u} → ZFSet.{u + 1} :=
  Quotient.lift (fun x => mk (PSet.lift x)) fun _ _ e => Quotient.sound (PSet.lift_equiv e)

def univ : ZFSet.{u + 1} := mk PSet.univ

theorem lift_mem_univ (x : ZFSet.{u}) : lift x ∈ₛ univ.{u} := by
  induction x using Quotient.inductionOn with
  | h x => exact PSet.lift_mem_univ x

theorem mem_lift {z : ZFSet.{u + 1}} {x : ZFSet.{u}} (h : z ∈ₛ lift x) :
    ∃ y, y ∈ₛ x ∧ z = lift y := by
  induction x using Quotient.inductionOn with
  | h x =>
  induction z using Quotient.inductionOn with
  | h z =>
    let ⟨y, m, e⟩ := PSet.mem_lift h
    exact ⟨mk y, m, Quotient.sound e⟩

/-! ## Propositions

`@Type 0` is read as the subsets of `{∅}`: a proposition is `{∅}` when it
holds and `∅` when it does not, so any two of its proofs are equal
(Proof-Irrelevance). -/

/-- `{∅ | p}`. -/
def ofProp (p : Prop) : ZFSet.{u} := mk (PSet.mk (ULift (PLift p)) fun _ => PSet.mk PEmpty PEmpty.elim)

theorem mem_ofProp {p : Prop} {z : ZFSet.{u}} : z ∈ₛ ofProp p ↔ p ∧ z = empty := by
  induction z using Quotient.inductionOn with
  | h z =>
    constructor
    · intro ⟨⟨⟨hp⟩⟩, e⟩; exact ⟨hp, Quotient.sound e⟩
    · intro ⟨hp, e⟩; exact ⟨⟨⟨hp⟩⟩, Quotient.exact e⟩

theorem proof_irrelevant {p : Prop} {x y : ZFSet.{u}}
    (hx : x ∈ₛ ofProp p) (hy : y ∈ₛ ofProp p) : x = y :=
  (mem_ofProp.1 hx).2.trans (mem_ofProp.1 hy).2.symm

end ZFSet
