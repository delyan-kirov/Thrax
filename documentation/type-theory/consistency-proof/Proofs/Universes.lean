import Proofs.Constructions

/-!
# Universes and codes

The model's universes and the codes of its types (#494).

A kernel type is interpreted as a **code**: an ordered pair of a label and
the set of the type's elements, `El`. Labels name the type former and its
arguments, so formers are injective and distinct, as Eq-Pi and Eq-Univ-Neq
need. These are PLT25's labelled sets (Section 6).

Kernel levels are closed numbers in the model. `@Type 0` is the propositions,
whose codes are `propCode p` with `El` the subset `{∅ | p}` of `{∅}`, so a
proposition's proofs are all `∅`. `@Type k` for `k ≥ 1` is a Grothendieck
universe from a `Hierarchy` of them. A hierarchy of `N` universes lives in
`ZFSet.{N}` (see the construction), so the model covers the kernel's levels up
to a bound `N` fixed in advance: a theory cannot model its own universes all at
once.
-/

universe u

namespace ZFSet

/-! ## Numerals, for labels -/

/-- Zermelo's numerals: `0 = ∅`, `n + 1 = {n}`. -/
def nat : Nat → ZFSet.{u}
  | 0 => empty
  | n + 1 => singleton (nat n)

theorem nat_inj : {m n : Nat} → nat.{u} m = nat.{u} n → m = n
  | 0, 0, _ => rfl
  | 0, n + 1, h => by
    have : nat n ∈ₛ nat.{u} (n + 1) := mem_singleton.2 rfl
    rw [← h] at this
    exact absurd this (not_mem_empty _)
  | m + 1, 0, h => by
    have : nat m ∈ₛ nat.{u} (m + 1) := mem_singleton.2 rfl
    rw [h] at this
    exact absurd this (not_mem_empty _)
  | _ + 1, _ + 1, h => congrArg Nat.succ (nat_inj (singleton_inj h))

/-- `{F n | n : Nat}`. -/
noncomputable def range (F : Nat → ZFSet.{u}) : ZFSet.{u} :=
  mk (PSet.mk (ULift Nat) fun n => out (F n.down))

theorem mem_range {F : Nat → ZFSet.{u}} {z : ZFSet.{u}} : z ∈ₛ range F ↔ ∃ n, z = F n := by
  rw [range, mem_mk_iff]
  exact ⟨fun ⟨⟨n⟩, e⟩ => ⟨n, e.trans (mk_out _)⟩, fun ⟨n, e⟩ => ⟨⟨n⟩, e.trans (mk_out _).symm⟩⟩

/-- The numerals: the axiom of infinity. -/
noncomputable def omegaSet : ZFSet.{u} := range nat

open Classical in
/-- The number a numeral stands for. -/
noncomputable def natOf (z : ZFSet.{u}) : Nat := if h : ∃ n, z = nat n then Classical.choose h else 0

theorem natOf_nat (n : Nat) : natOf (nat.{u} n) = n := by
  unfold natOf
  split
  next h => exact nat_inj (Classical.choose_spec h).symm
  next h => exact absurd ⟨n, rfl⟩ h

/-! ## Grothendieck universes -/

/-- A set closed under the constructions the model uses. -/
structure IsUniverse (G : ZFSet.{u}) : Prop where
  trans : ∀ x y, x ∈ₛ G → y ∈ₛ x → y ∈ₛ G
  empty_mem : empty ∈ₛ G
  upair_mem : ∀ x y, x ∈ₛ G → y ∈ₛ G → upair x y ∈ₛ G
  powerset_mem : ∀ x, x ∈ₛ G → powerset x ∈ₛ G
  union_mem : ∀ (I : ZFSet.{u}) (F : ZFSet.{u} → ZFSet.{u}),
    I ∈ₛ G → (∀ i, i ∈ₛ I → F i ∈ₛ G) → sUnion (image F I) ∈ₛ G
  omega_mem : omegaSet ∈ₛ G

namespace IsUniverse

variable {G : ZFSet.{u}} (hG : IsUniverse G)
include hG

theorem subset_mem {x y : ZFSet.{u}} (hy : y ∈ₛ G) (h : ∀ z, z ∈ₛ x → z ∈ₛ y) : x ∈ₛ G :=
  hG.trans _ _ (hG.powerset_mem y hy) (mem_powerset.2 h)

theorem pair_mem {x y : ZFSet.{u}} (hx : x ∈ₛ G) (hy : y ∈ₛ G) : pair x y ∈ₛ G :=
  hG.upair_mem _ _ (hG.upair_mem _ _ hx hx) (hG.upair_mem _ _ hx hy)

theorem singleton_mem {x : ZFSet.{u}} (hx : x ∈ₛ G) : singleton x ∈ₛ G := hG.upair_mem _ _ hx hx

theorem image_mem {I : ZFSet.{u}} {F : ZFSet.{u} → ZFSet.{u}} (hI : I ∈ₛ G)
    (hF : ∀ i, i ∈ₛ I → F i ∈ₛ G) : image F I ∈ₛ G := by
  have e : sUnion (image (fun i => singleton (F i)) I) = image F I := ext fun z => by
    rw [mem_sUnion, mem_image]
    constructor
    · rintro ⟨s, hs, hz⟩
      let ⟨i, hi, e⟩ := mem_image.1 hs
      subst e
      exact ⟨i, hi, mem_singleton.1 hz⟩
    · rintro ⟨i, hi, rfl⟩
      exact ⟨_, mem_image.2 ⟨i, hi, rfl⟩, mem_singleton.2 rfl⟩
  rw [← e]
  exact hG.union_mem _ _ hI fun i hi => hG.singleton_mem (hF i hi)

theorem sUnion_mem {x : ZFSet.{u}} (hx : x ∈ₛ G) : sUnion x ∈ₛ G := by
  have e : image (fun i => i) x = x := ext fun z => by
    rw [mem_image]
    constructor
    · rintro ⟨y, hy, rfl⟩; exact hy
    · intro h; exact ⟨z, h, rfl⟩
  rw [← e]
  exact hG.union_mem _ _ hx fun i hi => hG.trans _ _ hx hi

theorem prod_mem {a c : ZFSet.{u}} (ha : a ∈ₛ G) (hc : c ∈ₛ G) : prod a c ∈ₛ G :=
  hG.sUnion_mem <| hG.image_mem ha fun _ hx =>
    hG.image_mem hc fun _ hy => hG.pair_mem (hG.trans _ _ ha hx) (hG.trans _ _ hc hy)

theorem sep_mem {p : ZFSet.{u} → Prop} {x : ZFSet.{u}} (hx : x ∈ₛ G) : sep p x ∈ₛ G :=
  hG.subset_mem hx fun _ hz => (mem_sep.1 hz).1

theorem lam_mem {a : ZFSet.{u}} {F : ZFSet.{u} → ZFSet.{u}} (ha : a ∈ₛ G)
    (hF : ∀ x, x ∈ₛ a → F x ∈ₛ G) : lam a F ∈ₛ G :=
  hG.image_mem ha fun x hx => hG.pair_mem (hG.trans _ _ ha hx) (hF x hx)

theorem piSet_mem {a : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}} (ha : a ∈ₛ G)
    (hB : ∀ x, x ∈ₛ a → B x ∈ₛ G) : piSet a B ∈ₛ G :=
  hG.sep_mem <| hG.powerset_mem _ <| hG.prod_mem ha <| hG.sUnion_mem <| hG.image_mem ha hB

theorem ofProp_mem (p : Prop) : ofProp.{u} p ∈ₛ G :=
  hG.subset_mem (hG.singleton_mem hG.empty_mem) fun _ hz =>
    mem_singleton.2 (mem_ofProp.1 hz).2

theorem nat_mem : (n : Nat) → nat n ∈ₛ G
  | 0 => hG.empty_mem
  | n + 1 => hG.singleton_mem (nat_mem n)

theorem range_mem {F : Nat → ZFSet.{u}} (hF : ∀ n, F n ∈ₛ G) : range F ∈ₛ G := by
  have e : image (fun z => F (natOf z)) omegaSet = range F := ext fun z => by
    rw [mem_image, mem_range]
    constructor
    · rintro ⟨y, hy, rfl⟩; exact ⟨_, rfl⟩
    · rintro ⟨n, rfl⟩; exact ⟨nat n, mem_range.2 ⟨n, rfl⟩, by rw [natOf_nat]⟩
  rw [← e]
  exact hG.image_mem hG.omega_mem fun _ _ => hF _

theorem iUnion_mem {F : Nat → ZFSet.{u}} (hF : ∀ n, F n ∈ₛ G) : sUnion (range F) ∈ₛ G :=
  hG.sUnion_mem (hG.range_mem hF)

theorem snd_mem {c : ZFSet.{u}} (hc : c ∈ₛ G) : snd c ∈ₛ G := by
  unfold snd
  split
  next h =>
    have e := Classical.choose_spec (Classical.choose_spec h)
    have hab : upair _ _ ∈ₛ G := hG.trans _ _ (e ▸ hc) (mem_upair.2 (Or.inr rfl))
    exact hG.trans _ _ hab (mem_upair.2 (Or.inr rfl))
  next => exact hG.empty_mem

end IsUniverse

/-! ## Codes -/

/-- A type's elements: the second component of its code. -/
noncomputable def El (c : ZFSet.{u}) : ZFSet.{u} := snd c

/-- A proposition holds when `∅`, its only possible proof, is an element. -/
def holds (c : ZFSet.{u}) : Prop := empty ∈ₛ El c

/-- A proposition, by its truth: `{∅ | p}`. -/
def propCode (p : Prop) : ZFSet.{u} := pair (pair (nat 0) empty) (ofProp p)

/-- `@Type k`, given the set `U` of its elements. -/
def univCode (k : Nat) (U : ZFSet.{u}) : ZFSet.{u} := pair (pair (nat 1) (nat k)) U

/-- `(x : A) -> B x` with the sorts of both sides, `B` given by its graph. -/
noncomputable def piCode (l l' : Nat) (a b : ZFSet.{u}) : ZFSet.{u} :=
  pair (pair (nat 2) (pair (pair (nat l) (nat l')) (pair a b)))
    (piSet (El a) fun x => El (app b x))

/-- The pairs `(x, y)` with `x` in `A` and `y` in `B x`. -/
noncomputable def sigmaSet (a : ZFSet.{u}) (B : ZFSet.{u} → ZFSet.{u}) : ZFSet.{u} :=
  sep (fun z => ∃ x, x ∈ₛ a ∧ ∃ y, y ∈ₛ B x ∧ z = pair x y) (prod a (sUnion (image B a)))

/-- `(x : A) * B x`, as `piCode`. -/
noncomputable def sigmaCode (l l' : Nat) (a b : ZFSet.{u}) : ZFSet.{u} :=
  pair (pair (nat 3) (pair (pair (nat l) (nat l')) (pair a b)))
    (sigmaSet (El a) fun x => El (app b x))

/-- The pairs a relation `r` (a curried function into propositions) relates. -/
noncomputable def relSet (a r : ZFSet.{u}) : ZFSet.{u} :=
  sep (fun p => holds (app (app r (fst p)) (snd p))) (prod (El a) (El a))

/-- The class of `x`: what `r` relates it to. -/
noncomputable def cls (a r x : ZFSet.{u}) : ZFSet.{u} := sep (fun y => holds (app (app r x) y)) (El a)

/-- `A / R`: its elements are the classes; its label records the carrier
and the relation as a set of pairs, which is what Eq-Quot compares. -/
noncomputable def quotCode (l : Nat) (a r : ZFSet.{u}) : ZFSet.{u} :=
  pair (pair (nat 5) (pair (nat l) (pair a (relSet a r)))) (image (cls a r) (El a))

open Classical in
/-- Some element of `q`, or `∅`: a class's representative. -/
noncomputable def rep (q : ZFSet.{u}) : ZFSet.{u} := if h : ∃ x, x ∈ₛ q then Classical.choose h else empty

theorem rep_mem {q : ZFSet.{u}} (h : ∃ x, x ∈ₛ q) : rep q ∈ₛ q := by
  unfold rep; simp only [h, ↓reduceDIte]; exact Classical.choose_spec h

theorem mem_cls {a r x y : ZFSet.{u}} : y ∈ₛ cls a r x ↔ y ∈ₛ El a ∧ holds (app (app r x) y) := mem_sep

theorem mem_relSet {a r x y : ZFSet.{u}} :
    pair x y ∈ₛ relSet a r ↔ x ∈ₛ El a ∧ y ∈ₛ El a ∧ holds (app (app r x) y) := by
  rw [relSet, mem_sep, mem_prod, fst_pair, snd_pair]
  constructor
  · rintro ⟨⟨x', hx, y', hy, e⟩, h⟩
    let ⟨e₁, e₂⟩ := pair_inj e
    subst e₁ e₂
    exact ⟨hx, hy, h⟩
  · rintro ⟨hx, hy, h⟩
    exact ⟨⟨x, hx, y, hy, rfl⟩, h⟩

theorem El_code (label ext : ZFSet.{u}) : El (pair label ext) = ext := snd_pair _ _

theorem El_propCode (p : Prop) : El (propCode.{u} p) = ofProp p := El_code _ _

theorem holds_propCode {p : Prop} : holds (propCode.{u} p) ↔ p := by
  rw [holds, El_propCode]; exact mem_ofProp_iff

theorem El_univCode (k : Nat) (U : ZFSet.{u}) : El (univCode k U) = U := El_code _ _

theorem El_piCode (l l' : Nat) (a b : ZFSet.{u}) :
    El (piCode l l' a b) = piSet (El a) fun x => El (app b x) := El_code _ _

theorem El_sigmaCode (l l' : Nat) (a b : ZFSet.{u}) :
    El (sigmaCode l l' a b) = sigmaSet (El a) fun x => El (app b x) := El_code _ _

theorem propCode_inj {p q : Prop} (h : propCode.{u} p = propCode.{u} q) : p ↔ q :=
  ofProp_inj (pair_inj h).2

theorem propCode_congr {p q : Prop} (h : p ↔ q) : propCode.{u} p = propCode.{u} q := by
  rw [propext h]

theorem univCode_inj {k k' : Nat} {U U' : ZFSet.{u}} (h : univCode k U = univCode k' U') :
    k = k' :=
  nat_inj (pair_inj (pair_inj h).1).2

theorem piCode_inj {l l' m m' : Nat} {a b c d : ZFSet.{u}}
    (h : piCode l l' a b = piCode m m' c d) : l = m ∧ l' = m' ∧ a = c ∧ b = d := by
  let ⟨_, h⟩ := pair_inj (pair_inj h).1
  let ⟨hl, hab⟩ := pair_inj h
  let ⟨hl, hl'⟩ := pair_inj hl
  let ⟨ha, hb⟩ := pair_inj hab
  exact ⟨nat_inj hl, nat_inj hl', ha, hb⟩

theorem sigmaCode_inj {l l' m m' : Nat} {a b c d : ZFSet.{u}}
    (h : sigmaCode l l' a b = sigmaCode m m' c d) : l = m ∧ l' = m' ∧ a = c ∧ b = d := by
  let ⟨_, h⟩ := pair_inj (pair_inj h).1
  let ⟨hl, hab⟩ := pair_inj h
  let ⟨hl, hl'⟩ := pair_inj hl
  let ⟨ha, hb⟩ := pair_inj hab
  exact ⟨nat_inj hl, nat_inj hl', ha, hb⟩

theorem El_quotCode (l : Nat) (a r : ZFSet.{u}) : El (quotCode l a r) = image (cls a r) (El a) :=
  El_code _ _

theorem quotCode_inj {l l' : Nat} {a r a' r' : ZFSet.{u}} (h : quotCode l a r = quotCode l' a' r') :
    l = l' ∧ a = a' ∧ relSet a r = relSet a' r' := by
  let ⟨_, h⟩ := pair_inj (pair_inj h).1
  let ⟨hl, h⟩ := pair_inj h
  let ⟨ha, hr⟩ := pair_inj h
  exact ⟨nat_inj hl, ha, hr⟩

/-- The tag of a code: what Eq-Univ-Neq compares. -/
noncomputable def tag (c : ZFSet.{u}) : ZFSet.{u} := fst (fst c)

theorem tag_propCode (p : Prop) : tag (propCode.{u} p) = nat 0 := by
  simp only [tag, propCode, fst_pair]

theorem tag_univCode (k : Nat) (U : ZFSet.{u}) : tag (univCode k U) = nat 1 := by
  simp only [tag, univCode, fst_pair]

theorem tag_piCode (l l' : Nat) (a b : ZFSet.{u}) : tag (piCode l l' a b) = nat 2 := by
  simp only [tag, piCode, fst_pair]

theorem tag_sigmaCode (l l' : Nat) (a b : ZFSet.{u}) : tag (sigmaCode l l' a b) = nat 3 := by
  simp only [tag, sigmaCode, fst_pair]

theorem tag_quotCode (l : Nat) (a r : ZFSet.{u}) : tag (quotCode l a r) = nat 5 := by
  simp only [tag, quotCode, fst_pair]

/-- Codes with different tags differ. -/
theorem ne_of_tag {c d : ZFSet.{u}} {m n : Nat} (hc : tag c = nat m) (hd : tag d = nat n)
    (h : m ≠ n) : c ≠ d := fun e => h (nat_inj (hc.symm.trans (e ▸ hd)))

/-! ## The hierarchy -/

/-- `N` Grothendieck universes, each an element of the next. -/
structure Hierarchy (N : Nat) where
  G : Nat → ZFSet.{u}
  pos : 0 < N
  isUniverse : ∀ i, i < N → IsUniverse (G i)
  chain : ∀ i, i + 1 < N → G i ∈ₛ G (i + 1)

namespace Hierarchy

variable {N : Nat} (H : Hierarchy.{u} N)

theorem mono {i j : Nat} (hij : i ≤ j) (hj : j < N) {x : ZFSet.{u}} (hx : x ∈ₛ H.G i) :
    x ∈ₛ H.G j := by
  induction hij with
  | refl => exact hx
  | step hij ih =>
    rename_i k
    exact (H.isUniverse (k + 1) hj).trans _ _ (H.chain k hj) (ih (Nat.lt_of_succ_lt hj))

/-- Which universe of the hierarchy holds level `k ≥ 1`; levels past the
hierarchy share its top. -/
def index (N k : Nat) : Nat := min (k - 1) (N - 1)

include H in
theorem indexLt (k : Nat) : index N k < N := by
  unfold index; have := H.pos; omega

/-- The propositions: the two truth values' codes. -/
def props : ZFSet.{u} := upair (propCode False) (propCode True)

theorem propCode_mem_props (p : Prop) : propCode.{u} p ∈ₛ props := by
  rcases Classical.em p with hp | hp
  · rw [propCode_congr (iff_true_intro hp)]; exact mem_upair.2 (Or.inr rfl)
  · rw [propCode_congr (iff_false_intro hp)]; exact mem_upair.2 (Or.inl rfl)

theorem mem_props {c : ZFSet.{u}} (h : c ∈ₛ props) : ∃ p, c = propCode p := by
  rcases mem_upair.1 h with e | e
  · exact ⟨_, e⟩
  · exact ⟨_, e⟩

/-- The elements of `@Type k`. -/
def U (k : Nat) : ZFSet.{u} := if k = 0 then props else H.G (index N k)

theorem U_zero : H.U 0 = props := rfl

theorem U_succ (k : Nat) : H.U (k + 1) = H.G (index N (k + 1)) := rfl

theorem props_mem {i : Nat} (hi : i < N) : props ∈ₛ H.G i :=
  have hG := H.isUniverse i hi
  hG.upair_mem _ _ (hG.pair_mem (hG.pair_mem (hG.nat_mem 0) hG.empty_mem) (hG.ofProp_mem _))
    (hG.pair_mem (hG.pair_mem (hG.nat_mem 0) hG.empty_mem) (hG.ofProp_mem _))

/-- Every type of level `k` is a set of the universe for a level `m ≥ k`. -/
theorem U_sub {k m : Nat} (hkm : k ≤ m) {c : ZFSet.{u}} (hc : c ∈ₛ H.U k) :
    c ∈ₛ H.G (index N m) := by
  have hm := H.indexLt m
  unfold U at hc
  split at hc
  · exact (H.isUniverse _ hm).trans _ _ (H.props_mem hm) hc
  · exact H.mono (by unfold index; omega) hm hc

/-- A proposition's elements are among `{∅}`. -/
theorem El_of_mem_props {c x : ZFSet.{u}} (hc : c ∈ₛ props) (hx : x ∈ₛ El c) : x = empty := by
  let ⟨p, e⟩ := mem_props hc
  subst e
  rw [El_propCode] at hx
  exact (mem_ofProp.1 hx).2

/-- `@Type k : @Type (k + 1)` (Univ), within the hierarchy. -/
theorem univCode_mem {k : Nat} (hk : k + 1 ≤ N) :
    univCode k (H.U k) ∈ₛ H.U (k + 1) := by
  rw [U_succ]
  have hi : index N (k + 1) = k := by unfold index; omega
  rw [hi]
  have hG := H.isUniverse k (by omega)
  refine hG.pair_mem (hG.pair_mem (hG.nat_mem 1) (hG.nat_mem k)) ?_
  cases k with
  | zero => exact H.props_mem (by omega)
  | succ k =>
    rw [U_succ]
    have : index N (k + 1) = k := by unfold index; omega
    rw [this]
    exact H.chain k (by omega)

/-- `(x : A) -> B x` with `B` not a proposition (Pi-Form). -/
theorem piCode_mem {l l' : Nat} (hl' : l' ≠ 0) {a : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}}
    (ha : a ∈ₛ H.U l) (hB : ∀ x, x ∈ₛ El a → B x ∈ₛ H.U l') :
    piCode l l' a (lam (El a) B) ∈ₛ H.U (max l l') := by
  have hm : max l l' ≠ 0 := by omega
  have hU : H.U (max l l') = H.G (index N (max l l')) := by unfold U; simp [hm]
  rw [hU]
  have hG := H.isUniverse _ (H.indexLt (max l l'))
  have ha' := H.U_sub (Nat.le_max_left l l') ha
  have hB' : ∀ x, x ∈ₛ El a → B x ∈ₛ H.G (index N (max l l')) :=
    fun x hx => H.U_sub (Nat.le_max_right l l') (hB x hx)
  have hEl := hG.snd_mem ha'
  have hb := hG.lam_mem hEl hB'
  refine hG.pair_mem (hG.pair_mem (hG.nat_mem 2) (hG.pair_mem
    (hG.pair_mem (hG.nat_mem l) (hG.nat_mem l')) (hG.pair_mem ha' hb))) ?_
  refine hG.piSet_mem hEl fun x hx => ?_
  rw [app_lam hx]
  exact hG.snd_mem (hB' x hx)

/-- `(x : A) * B x` when it is not a proposition (Sigma-Form). -/
theorem sigmaCode_mem {l l' : Nat} (h : ¬(l = 0 ∧ l' = 0)) {a : ZFSet.{u}}
    {B : ZFSet.{u} → ZFSet.{u}} (ha : a ∈ₛ H.U l) (hB : ∀ x, x ∈ₛ El a → B x ∈ₛ H.U l') :
    sigmaCode l l' a (lam (El a) B) ∈ₛ H.U (max l l') := by
  have hm : max l l' ≠ 0 := by omega
  have hU : H.U (max l l') = H.G (index N (max l l')) := by unfold U; simp [hm]
  rw [hU]
  have hG := H.isUniverse _ (H.indexLt (max l l'))
  have ha' := H.U_sub (Nat.le_max_left l l') ha
  have hB' : ∀ x, x ∈ₛ El a → B x ∈ₛ H.G (index N (max l l')) :=
    fun x hx => H.U_sub (Nat.le_max_right l l') (hB x hx)
  have hEl := hG.snd_mem ha'
  have hb := hG.lam_mem hEl hB'
  refine hG.pair_mem (hG.pair_mem (hG.nat_mem 3) (hG.pair_mem
    (hG.pair_mem (hG.nat_mem l) (hG.nat_mem l')) (hG.pair_mem ha' hb))) ?_
  refine hG.sep_mem (hG.prod_mem hEl (hG.sUnion_mem (hG.image_mem hEl fun x hx => ?_)))
  rw [app_lam hx]
  exact hG.snd_mem (hB' x hx)

/-- `A / R` is in `A`'s universe (Quot-Form). -/
theorem quotCode_mem {l : Nat} (hl : l ≠ 0) {a : ZFSet.{u}} (ha : a ∈ₛ H.U l) (r : ZFSet.{u}) :
    quotCode l a r ∈ₛ H.U l := by
  have hU : H.U l = H.G (index N l) := by unfold U; simp [hl]
  rw [hU] at ha ⊢
  have hG := H.isUniverse _ (H.indexLt l)
  have hEl := hG.snd_mem ha
  refine hG.pair_mem (hG.pair_mem (hG.nat_mem 5) (hG.pair_mem (hG.nat_mem l)
    (hG.pair_mem ha (hG.sep_mem (hG.prod_mem hEl hEl))))) ?_
  exact hG.image_mem hEl fun _ _ => hG.sep_mem hEl

end Hierarchy

end ZFSet
