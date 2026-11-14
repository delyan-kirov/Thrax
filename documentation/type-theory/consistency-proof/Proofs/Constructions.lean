import Proofs.Sets

/-!
# Set constructions

Separation, union, power set and image on `ZFSet`, then what the model
builds from them: ordered-pair projections, function graphs with application,
and dependent function sets.

Each construction works on a representative chosen by `out`, so it
needs no proof that it respects equivalence; the price is `Classical.choice`.
The model uses Lean's three standard axioms: `propext`, `Quot.sound` and
`Classical.choice`.
-/

universe u

namespace PSet

def Index : PSet.{u} → Type u
  | mk α _ => α

def member : (x : PSet.{u}) → x.Index → PSet.{u}
  | mk _ f => f

/-- The members satisfying `p`. -/
def sep (p : PSet.{u} → Prop) : PSet.{u} → PSet.{u}
  | mk _ f => mk {a // p (f a)} fun a => f a.1

/-- The members of the members. -/
def sUnion : PSet.{u} → PSet.{u}
  | mk _ f => mk (Σ a, (f a).Index) fun p => (f p.1).member p.2

/-- Every subset, one per predicate on the index. -/
def powerset : PSet.{u} → PSet.{u}
  | mk α f => mk (α → Prop) fun s => mk {a // s a} fun a => f a.1

end PSet

namespace ZFSet

/-- A representative, chosen. -/
noncomputable def out (x : ZFSet.{u}) : PSet.{u} := Classical.choose (Quotient.exists_rep x)

theorem mk_out (x : ZFSet.{u}) : mk (out x) = x := Classical.choose_spec (Quotient.exists_rep x)

theorem mem_mk_iff {y : PSet.{u}} {z : ZFSet.{u}} :
    z ∈ₛ mk y ↔ ∃ b, z = mk (y.member b) := by
  cases y with
  | mk α f =>
    induction z using Quotient.inductionOn with
    | h z => exact ⟨fun ⟨a, e⟩ => ⟨a, Quotient.sound e⟩, fun ⟨a, e⟩ => ⟨a, Quotient.exact e⟩⟩

/-- Membership through the chosen representative. -/
theorem mem_iff {x z : ZFSet.{u}} : z ∈ₛ x ↔ ∃ b, z = mk ((out x).member b) := by
  conv => lhs; rw [← mk_out x]
  exact mem_mk_iff

/-! ## Separation, union, power set, image -/

noncomputable def sep (p : ZFSet.{u} → Prop) (x : ZFSet.{u}) : ZFSet.{u} :=
  mk (PSet.sep (fun z => p (mk z)) (out x))

theorem mem_sep {p : ZFSet.{u} → Prop} {x z : ZFSet.{u}} : z ∈ₛ sep p x ↔ z ∈ₛ x ∧ p z := by
  rw [sep, mem_mk_iff, mem_iff]
  generalize out x = y
  cases y with
  | mk α f =>
    constructor
    · rintro ⟨⟨a, ha⟩, rfl⟩; exact ⟨⟨a, rfl⟩, ha⟩
    · rintro ⟨⟨a, rfl⟩, ha⟩; exact ⟨⟨a, ha⟩, rfl⟩

noncomputable def sUnion (x : ZFSet.{u}) : ZFSet.{u} := mk (PSet.sUnion (out x))

theorem mem_sUnion {x z : ZFSet.{u}} : z ∈ₛ sUnion x ↔ ∃ y, y ∈ₛ x ∧ z ∈ₛ y := by
  rw [sUnion, mem_mk_iff]
  conv => rhs; enter [1, y]; rw [mem_iff (x := x)]
  generalize out x = w
  cases w with
  | mk α f =>
    constructor
    · rintro ⟨⟨a, b⟩, rfl⟩
      exact ⟨mk (f a), ⟨a, rfl⟩, mem_mk_iff.2 ⟨b, rfl⟩⟩
    · rintro ⟨y, ⟨a, rfl⟩, hz⟩
      let ⟨b, e⟩ := mem_mk_iff.1 hz
      exact ⟨⟨a, b⟩, e⟩

noncomputable def powerset (x : ZFSet.{u}) : ZFSet.{u} := mk (PSet.powerset (out x))

theorem mem_powerset {x z : ZFSet.{u}} : z ∈ₛ powerset x ↔ ∀ w, w ∈ₛ z → w ∈ₛ x := by
  rw [powerset, mem_mk_iff]
  conv => rhs; enter [w]; rw [mem_iff (x := x)]
  generalize out x = y
  cases y with
  | mk α f =>
    constructor
    · rintro ⟨s, rfl⟩ w hw
      let ⟨⟨a, _⟩, e⟩ := mem_mk_iff.1 hw
      exact ⟨a, e⟩
    · intro h
      refine ⟨fun a => mk (f a) ∈ₛ z, ext fun w => ⟨fun hw => ?_, fun hw => ?_⟩⟩
      · let ⟨a, e⟩ := h w hw
        have e' : w = mk (f a) := e
        subst e'
        exact mem_mk_iff.2 ⟨⟨a, hw⟩, rfl⟩
      · let ⟨⟨a, ha⟩, e⟩ := mem_mk_iff.1 hw
        exact e ▸ ha

/-- `{F y | y ∈ x}`. -/
noncomputable def image (F : ZFSet.{u} → ZFSet.{u}) (x : ZFSet.{u}) : ZFSet.{u} :=
  mk (PSet.mk (out x).Index fun b => out (F (mk ((out x).member b))))

theorem mem_image {F : ZFSet.{u} → ZFSet.{u}} {x z : ZFSet.{u}} :
    z ∈ₛ image F x ↔ ∃ y, y ∈ₛ x ∧ z = F y := by
  rw [image, mem_mk_iff]
  conv => rhs; enter [1, y]; rw [mem_iff (x := x)]
  constructor
  · rintro ⟨b, rfl⟩
    exact ⟨mk ((out x).member b), ⟨b, rfl⟩, mk_out _⟩
  · rintro ⟨y, ⟨b, rfl⟩, rfl⟩
    exact ⟨b, (mk_out _).symm⟩

/-! ## Facts about the finite sets -/

theorem upair_ne_empty {x y : ZFSet.{u}} : upair x y ≠ empty := fun h =>
  not_mem_empty x (h ▸ mem_upair.2 (Or.inl rfl))

theorem pair_ne_empty {x y : ZFSet.{u}} : pair x y ≠ empty := upair_ne_empty

theorem mem_ofProp_iff {p : Prop} : empty ∈ₛ ofProp.{u} p ↔ p :=
  ⟨fun h => (mem_ofProp.1 h).1, fun h => mem_ofProp.2 ⟨h, rfl⟩⟩

theorem ofProp_inj {p q : Prop} (h : ofProp.{u} p = ofProp.{u} q) : p ↔ q :=
  ⟨fun hp => mem_ofProp_iff.1 (h ▸ mem_ofProp_iff.2 hp),
   fun hq => mem_ofProp_iff.1 (h.symm ▸ mem_ofProp_iff.2 hq)⟩

theorem ofProp_congr {p q : Prop} (h : p ↔ q) : ofProp.{u} p = ofProp.{u} q := by
  rw [propext h]

/-! ## Projections

Total: a set that is not an ordered pair projects to `∅`. -/

open Classical in
noncomputable def fst (v : ZFSet.{u}) : ZFSet.{u} :=
  if h : ∃ a b, v = pair a b then Classical.choose h else empty

open Classical in
noncomputable def snd (v : ZFSet.{u}) : ZFSet.{u} :=
  if h : ∃ a b, v = pair a b then Classical.choose (Classical.choose_spec h) else empty

theorem fst_pair (a b : ZFSet.{u}) : fst (pair a b) = a := by
  unfold fst
  split
  next h => exact (pair_inj (Classical.choose_spec (Classical.choose_spec h))).1.symm
  next h => exact absurd ⟨a, b, rfl⟩ h

theorem snd_pair (a b : ZFSet.{u}) : snd (pair a b) = b := by
  unfold snd
  split
  next h => exact (pair_inj (Classical.choose_spec (Classical.choose_spec h))).2.symm
  next h => exact absurd ⟨a, b, rfl⟩ h

theorem not_pair_empty : ¬ ∃ a b, empty.{u} = pair a b := fun ⟨_, _, h⟩ => pair_ne_empty h.symm

theorem fst_empty : fst empty.{u} = empty := by
  unfold fst
  split
  next h => exact absurd h not_pair_empty
  next => rfl

theorem snd_empty : snd empty.{u} = empty := by
  unfold snd
  split
  next h => exact absurd h not_pair_empty
  next => rfl

/-! ## Products and functions -/

/-- `A × C`. -/
noncomputable def prod (a c : ZFSet.{u}) : ZFSet.{u} :=
  sUnion (image (fun x => image (fun y => pair x y) c) a)

theorem mem_prod {a c z : ZFSet.{u}} :
    z ∈ₛ prod a c ↔ ∃ x, x ∈ₛ a ∧ ∃ y, y ∈ₛ c ∧ z = pair x y := by
  rw [prod, mem_sUnion]
  constructor
  · rintro ⟨s, hs, hz⟩
    let ⟨x, hx, e⟩ := mem_image.1 hs
    subst e
    let ⟨y, hy, e⟩ := mem_image.1 hz
    exact ⟨x, hx, y, hy, e⟩
  · rintro ⟨x, hx, y, hy, rfl⟩
    exact ⟨_, mem_image.2 ⟨x, hx, rfl⟩, mem_image.2 ⟨y, hy, rfl⟩⟩

/-- The graph of `F` on `A`. -/
noncomputable def lam (a : ZFSet.{u}) (F : ZFSet.{u} → ZFSet.{u}) : ZFSet.{u} :=
  image (fun x => pair x (F x)) a

theorem mem_lam {a z : ZFSet.{u}} {F : ZFSet.{u} → ZFSet.{u}} :
    z ∈ₛ lam a F ↔ ∃ x, x ∈ₛ a ∧ z = pair x (F x) := mem_image

open Classical in
/-- Application of a graph: the `y` paired with `x`, or `∅`. -/
noncomputable def app (f x : ZFSet.{u}) : ZFSet.{u} :=
  if h : ∃ y, pair x y ∈ₛ f then Classical.choose h else empty

theorem app_lam {a x : ZFSet.{u}} {F : ZFSet.{u} → ZFSet.{u}} (hx : x ∈ₛ a) :
    app (lam a F) x = F x := by
  unfold app
  split
  next h =>
    let ⟨x', _, e⟩ := mem_lam.1 (Classical.choose_spec h)
    let ⟨e₁, e₂⟩ := pair_inj e
    rw [e₂, ← e₁]
  next h => exact absurd ⟨F x, mem_lam.2 ⟨x, hx, rfl⟩⟩ h

theorem app_empty (x : ZFSet.{u}) : app empty x = empty := by
  unfold app
  split
  next h => exact absurd h fun ⟨_, h⟩ => not_mem_empty _ h
  next => rfl

theorem lam_congr {a : ZFSet.{u}} {F G : ZFSet.{u} → ZFSet.{u}}
    (h : ∀ x, x ∈ₛ a → F x = G x) : lam a F = lam a G :=
  ext fun z => by
    rw [mem_lam, mem_lam]
    exact ⟨fun ⟨x, hx, e⟩ => ⟨x, hx, h x hx ▸ e⟩, fun ⟨x, hx, e⟩ => ⟨x, hx, (h x hx).symm ▸ e⟩⟩

/-- The dependent functions from `A`, each value `F x` in `B x`. -/
noncomputable def piSet (a : ZFSet.{u}) (B : ZFSet.{u} → ZFSet.{u}) : ZFSet.{u} :=
  sep (fun f => ∃ F : ZFSet.{u} → ZFSet.{u}, (∀ x, x ∈ₛ a → F x ∈ₛ B x) ∧ f = lam a F)
    (powerset (prod a (sUnion (image B a))))

theorem mem_piSet {a f : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}} :
    f ∈ₛ piSet a B ↔ ∃ F : ZFSet.{u} → ZFSet.{u}, (∀ x, x ∈ₛ a → F x ∈ₛ B x) ∧ f = lam a F := by
  rw [piSet, mem_sep]
  refine ⟨fun h => h.2, fun h => ⟨?_, h⟩⟩
  let ⟨F, hF, e⟩ := h
  subst e
  refine mem_powerset.2 fun z hz => ?_
  let ⟨x, hx, e⟩ := mem_lam.1 hz
  exact mem_prod.2 ⟨x, hx, F x, mem_sUnion.2 ⟨B x, mem_image.2 ⟨x, hx, rfl⟩, hF x hx⟩, e⟩

theorem lam_mem_piSet {a : ZFSet.{u}} {B F : ZFSet.{u} → ZFSet.{u}}
    (h : ∀ x, x ∈ₛ a → F x ∈ₛ B x) : lam a F ∈ₛ piSet a B :=
  mem_piSet.2 ⟨F, h, rfl⟩

theorem app_mem_piSet {a f x : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}}
    (hf : f ∈ₛ piSet a B) (hx : x ∈ₛ a) : app f x ∈ₛ B x := by
  let ⟨F, hF, e⟩ := mem_piSet.1 hf
  subst e
  rw [app_lam hx]
  exact hF x hx

/-- A function is the graph of its own application (eta). -/
theorem lam_app {a f : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}} (hf : f ∈ₛ piSet a B) :
    lam a (app f) = f := by
  let ⟨F, _, e⟩ := mem_piSet.1 hf
  subst e
  exact lam_congr fun x hx => app_lam hx

theorem piSet_congr {a : ZFSet.{u}} {B B' : ZFSet.{u} → ZFSet.{u}}
    (h : ∀ x, x ∈ₛ a → B x = B' x) : piSet a B = piSet a B' :=
  ext fun f => by
    rw [mem_piSet, mem_piSet]
    exact ⟨fun ⟨F, hF, e⟩ => ⟨F, fun x hx => h x hx ▸ hF x hx, e⟩,
           fun ⟨F, hF, e⟩ => ⟨F, fun x hx => (h x hx).symm ▸ hF x hx, e⟩⟩

/-- Functions are equal when they agree pointwise. -/
theorem funext {a f g : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}}
    (hf : f ∈ₛ piSet a B) (hg : g ∈ₛ piSet a B) (h : ∀ x, x ∈ₛ a → app f x = app g x) :
    f = g := by
  rw [← lam_app hf, ← lam_app hg]
  exact lam_congr h

end ZFSet
