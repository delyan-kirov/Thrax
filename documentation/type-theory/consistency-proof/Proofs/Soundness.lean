import Proofs.Signature

/-!
# Soundness and consistency

Every derivable judgment holds in the model (#494): a typed term is
interpreted in its type's elements, and convertible terms are interpreted
equally. `@Bottom` is the proposition with no elements, so no closed term has
type `@Bottom`.
-/

universe u

namespace Model

open ZFSet Family

variable {N : Nat} (H : Hierarchy.{u} N) (S : Glob → ZFSet.{u})

/-! ## Facts the cases share -/

theorem El_sort (l : Nat) (ρ : Nat → ZFSet.{u}) : El (interp H S (.sort l) ρ) = H.U l :=
  El_univCode _ _

theorem propCode_mem_U0 (p : Prop) : propCode.{u} p ∈ₛ H.U 0 :=
  Hierarchy.propCode_mem_props p

/-- A proposition's only element is `∅`. -/
theorem eq_empty {c v : ZFSet.{u}} (hc : c ∈ₛ H.U 0) (hv : v ∈ₛ El c) : v = empty :=
  Hierarchy.El_of_mem_props hc hv

theorem holds_of {c v : ZFSet.{u}} (hc : c ∈ₛ H.U 0) (hv : v ∈ₛ El c) : holds c := by
  have e := eq_empty H hc hv
  subst e
  exact hv

/-- Propositions are equal exactly when they hold together. -/
theorem prop_eq_iff {c d : ZFSet.{u}} (hc : c ∈ₛ H.U 0) (hd : d ∈ₛ H.U 0) :
    c = d ↔ (holds c ↔ holds d) := by
  let ⟨p, ep⟩ := Hierarchy.mem_props hc
  let ⟨q, eq⟩ := Hierarchy.mem_props hd
  subst ep eq
  rw [holds_propCode, holds_propCode]
  exact ⟨fun h => propCode_inj h, propCode_congr⟩

theorem interp_pi_zero (l : Nat) (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (.pi l 0 A B) ρ =
      propCode (∀ x, x ∈ₛ El (interp H S A ρ) → holds (interp H S B (cons x ρ))) := by
  simp only [interp, ↓reduceIte]

theorem interp_pi {l l' : Nat} (hl' : l' ≠ 0) (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (.pi l l' A B) ρ =
      piCode l l' (interp H S A ρ) (lam (El (interp H S A ρ)) fun x => interp H S B (cons x ρ)) := by
  simp only [interp, hl', ↓reduceIte]

theorem interp_and (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (.sigma 0 0 A B) ρ =
      propCode (holds (interp H S A ρ) ∧ holds (interp H S B (cons empty ρ))) := by
  simp only [interp, and_self, ↓reduceIte]

/-- The elements of a Pi type with a relevant codomain. -/
theorem El_pi {l l' : Nat} (hl' : l' ≠ 0) (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    El (interp H S (.pi l l' A B) ρ) =
      piSet (El (interp H S A ρ)) fun x => El (interp H S B (cons x ρ)) := by
  rw [interp_pi H S hl', El_piCode]
  exact piSet_congr fun x hx => by rw [app_lam hx]

theorem mem_sigmaSet {a z : ZFSet.{u}} {B : ZFSet.{u} → ZFSet.{u}} :
    z ∈ₛ sigmaSet a B ↔ ∃ x, x ∈ₛ a ∧ ∃ y, y ∈ₛ B x ∧ z = pair x y := by
  rw [sigmaSet, mem_sep]
  refine ⟨fun h => h.2, fun h => ⟨?_, h⟩⟩
  let ⟨x, hx, y, hy, e⟩ := h
  exact mem_prod.2 ⟨x, hx, y, mem_sUnion.2 ⟨B x, mem_image.2 ⟨x, hx, rfl⟩, hy⟩, e⟩

theorem sigmaSet_congr {a : ZFSet.{u}} {B B' : ZFSet.{u} → ZFSet.{u}}
    (h : ∀ x, x ∈ₛ a → B x = B' x) : sigmaSet a B = sigmaSet a B' :=
  ext fun z => by
    rw [mem_sigmaSet, mem_sigmaSet]
    exact ⟨fun ⟨x, hx, y, hy, e⟩ => ⟨x, hx, y, h x hx ▸ hy, e⟩,
           fun ⟨x, hx, y, hy, e⟩ => ⟨x, hx, y, (h x hx).symm ▸ hy, e⟩⟩

theorem interp_sigma {l l' : Nat} (h : ¬(l = 0 ∧ l' = 0)) (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    interp H S (.sigma l l' A B) ρ =
      sigmaCode l l' (interp H S A ρ) (lam (El (interp H S A ρ)) fun x => interp H S B (cons x ρ)) := by
  simp only [interp]
  split
  next h' => exact absurd h' h
  next => rfl

/-- The elements of a Sigma type that is not a proposition. -/
theorem El_sigma {l l' : Nat} (h : ¬(l = 0 ∧ l' = 0)) (A B : Tm) (ρ : Nat → ZFSet.{u}) :
    El (interp H S (.sigma l l' A B) ρ) =
      sigmaSet (El (interp H S A ρ)) fun x => El (interp H S B (cons x ρ)) := by
  rw [interp_sigma H S h, El_sigmaCode]
  exact sigmaSet_congr fun x hx => by rw [app_lam hx]

/-! ## Heads

Eq-Univ-Neq says types with different heads are unequal. A code's head is
read back from its label. -/

open Classical in
/-- The tag of a code and the levels in its label. -/
noncomputable def hd (c : ZFSet.{u}) : ZFSet.{u} :=
  pair (tag c) (if tag c = nat 1 then snd (fst c) else fst (snd (fst c)))

def enc : Tm.Head → ZFSet.{u}
  | .sort l => pair (nat 1) (nat l)
  | .pi l l' => pair (nat 2) (pair (nat l) (nat l'))
  | .sigma l l' => pair (nat 3) (pair (nat l) (nat l'))
  | .quot l => pair (nat 5) (nat l)

theorem enc_inj {a b : Tm.Head} (h : enc.{u} a = enc.{u} b) : a = b := by
  cases a <;> cases b <;> simp only [enc] at h <;>
    first
    | (have := nat_inj (pair_inj h).1; omega)
    | (have := nat_inj (pair_inj h).2; subst this; rfl)
    | (have := pair_inj (pair_inj h).2
       have h1 := nat_inj this.1
       have h2 := nat_inj this.2
       subst h1 h2; rfl)

theorem nat_ne {m n : Nat} (h : m ≠ n) : nat.{u} m ≠ nat.{u} n := fun e => h (nat_inj e)

theorem hd_interp {X : Tm} {h : Tm.Head} (hX : X.head = some h) (ρ : Nat → ZFSet.{u}) :
    hd (interp H S X ρ) = enc h := by
  cases X with
  | sort l =>
    simp only [Tm.head, Option.some.injEq] at hX
    subst hX
    show hd (univCode l (H.U l)) = _
    rw [hd, tag_univCode]
    split
    next => simp only [univCode, fst_pair, snd_pair, enc]
    next h => exact absurd rfl h
  | pi l l' A B =>
    by_cases hl' : l' = 0
    · simp [Tm.head, hl'] at hX
    · simp only [Tm.head, hl', ↓reduceIte, Option.some.injEq] at hX
      subst hX
      rw [interp_pi H S hl', hd, tag_piCode]
      split
      next h => exact absurd h (nat_ne (by omega))
      next => simp only [piCode, fst_pair, snd_pair, enc]
  | sigma l l' A B =>
    by_cases hl : l = 0 ∧ l' = 0
    · simp [Tm.head, hl] at hX
    · simp only [Tm.head, hl, ↓reduceIte, Option.some.injEq] at hX
      subst hX
      rw [interp_sigma H S hl, hd, tag_sigmaCode]
      split
      next h => exact absurd h (nat_ne (by omega))
      next => simp only [sigmaCode, fst_pair, snd_pair, enc]
  | quot l A R =>
    simp only [Tm.head, Option.some.injEq] at hX
    subst hX
    show hd (quotCode l _ _) = _
    rw [hd, tag_quotCode]
    split
    next h => exact absurd h (nat_ne (by omega))
    next => simp only [quotCode, fst_pair, snd_pair, enc]
  | _ => simp [Tm.head] at hX

/-! ## Declarations in the signature -/

theorem ArgsOk.split {pre vs : List ZFSet.{u}} {Δ : List (Tm × Nat)} (n : Nat)
    (h : ArgsOk H S pre vs Δ) :
    ArgsOk H S pre (vs.take n) (Δ.take n) ∧ ArgsOk H S (pre ++ vs.take n) (vs.drop n) (Δ.drop n) := by
  have hl := h.length
  rw [← List.take_append_drop n vs, ← List.take_append_drop n Δ] at h
  exact (ArgsOk.append (by simp [hl])).1 h

/-- What the soundness of a rule about declaration `k` may use. -/
theorem declAt {sig : Sig} (hG : Good H sig) {k : Nat} {d : Decl} (hk : sig[k]? = some d) :
    DeclWith (Valid H (semSig H (sig.take k))) k d ∧
    (∀ g, g.decl = k → semSig H sig g = meaning H (semSig H (sig.take k)) k d g) ∧
    (∀ g, g.decl < k → semSig H sig g = semSig H (sig.take k) g) := by
  have hlt : k < sig.length := (List.getElem?_eq_some_iff.1 hk).1
  refine ⟨hG k d hk, fun g hg => ?_, fun g hg => semSig_take H (Nat.le_of_lt hlt) hg⟩
  have h' : sig[g.decl]? = some d := hg ▸ hk
  rw [semSig_at H h', hg]

/-! ## Iterated Pi types -/

/-- Eliminating a Pi type, at any sorts. -/
theorem app_mem_pi {l l' : Nat} {A B : Tm} {ρ : Nat → ZFSet.{u}} {f a : ZFSet.{u}}
    (hf : f ∈ₛ El (interp H S (.pi l l' A B) ρ)) (ha : a ∈ₛ El (interp H S A ρ)) :
    ZFSet.app f a ∈ₛ El (interp H S B (cons a ρ)) := by
  by_cases hl' : l' = 0
  · subst hl'
    rw [interp_pi_zero, El_propCode] at hf
    obtain ⟨hall, hfe⟩ := mem_ofProp.1 hf
    rw [hfe, app_empty]; exact hall _ ha
  · rw [El_pi H S hl'] at hf; exact app_mem_piSet hf ha

/-- Values for the binders of a telescope, each in its type after the ones
before it, starting from the environment `L`. -/
def TeleFit : List ZFSet.{u} → List (Tm × Nat) → List ZFSet.{u} → Prop
  | _, [], [] => True
  | L, (A, _) :: Δ, v :: vs => v ∈ₛ El (interp H S A (envOf L)) ∧ TeleFit (v :: L) Δ vs
  | _, _, _ => False

theorem apps_mem_pis : {Δ : List (Tm × Nat)} → {B : Tm} → {l : Nat} → {L vs : List ZFSet.{u}} →
    {f : ZFSet.{u}} → f ∈ₛ El (interp H S (pis Δ B l) (envOf L)) → TeleFit H S L Δ vs →
    apps f vs ∈ₛ El (interp H S B (envOf (vs.reverse ++ L)))
  | [], _, _, _, [], _, hf, _ => by simpa [pis, apps_nil] using hf
  | (A, l₀) :: Δ, B, l, L, v :: vs, f, hf, ⟨hv, h⟩ => by
    have := app_mem_pi H S hf hv
    rw [cons_envOf] at this
    have := apps_mem_pis this h
    rw [apps_cons]; simpa using this

theorem TeleFit.append : {L vs ws : List ZFSet.{u}} → {Δ Δ' : List (Tm × Nat)} →
    vs.length = Δ.length → TeleFit H S L Δ vs → TeleFit H S (vs.reverse ++ L) Δ' ws →
    TeleFit H S L (Δ ++ Δ') (vs ++ ws)
  | _, [], _, [], _, _, _, h => by simpa using h
  | L, v :: vs, ws, (A, l) :: Δ, Δ', hl, ⟨hv, h⟩, h' => ⟨hv, TeleFit.append (by simpa using hl) h
      (by simpa using h')⟩

/-- One entry of a fitting telescope of arguments. -/
theorem ArgsOk.get : {pre vs : List ZFSet.{u}} → {Δ : List (Tm × Nat)} → ArgsOk H S pre vs Δ →
    (i : Nat) → (hi : i < vs.length) → (hΔ : i < Δ.length) →
    vs[i] ∈ₛ El (interp H S Δ[i].1 (envOf (pre ++ vs.take i).reverse))
  | _, _ :: _, (_, _) :: _, ⟨_, hv, _⟩, 0, _, _ => by simpa using hv
  | pre, v :: vs, (_, _) :: _, ⟨_, _, h⟩, i + 1, hi, hΔ => by
    have := h.get i (by simpa using hi) (by simpa using hΔ)
    simpa using this

/-! ## Quotients -/

theorem interp_shiftN (n : Nat) (t : Tm) (x : ZFSet.{u}) (ρ : Nat → ZFSet.{u}) :
    interp H S (Quot.shiftN (n + 1) t) (cons x ρ) = interp H S (Quot.shiftN n t) ρ :=
  interp_shift H S _ x ρ

theorem holds_pi_zero {l : Nat} {A B : Tm} {ρ : Nat → ZFSet.{u}} :
    holds (interp H S (.pi l 0 A B) ρ) ↔ ∀ x, x ∈ₛ El (interp H S A ρ) → holds (interp H S B (cons x ρ)) := by
  rw [interp_pi_zero, holds_propCode]

theorem El_shift {A : Tm} {x : ZFSet.{u}} {ρ : Nat → ZFSet.{u}} :
    El (interp H S A.shift (cons x ρ)) = El (interp H S A ρ) := by rw [interp_shift]

section QuotSem

variable {l : Nat} {A R : Tm} {ρ : Nat → ZFSet.{u}}

theorem refl_sem (h : holds (interp H S (Quot.reflTy l A R) ρ)) :
    ∀ x, x ∈ₛ El (interp H S A ρ) → holds (app (app (interp H S R ρ) x) x) := fun x hx => by
  have := (holds_pi_zero H S).1 h x hx
  change holds (app (app (interp H S (Quot.shiftN 1 R) (cons x ρ)) x) x) at this
  rwa [interp_shiftN] at this

theorem sym_sem (h : holds (interp H S (Quot.symTy l A R) ρ)) :
    ∀ x, x ∈ₛ El (interp H S A ρ) → ∀ y, y ∈ₛ El (interp H S A ρ) →
      holds (app (app (interp H S R ρ) x) y) → holds (app (app (interp H S R ρ) y) x) :=
  fun x hx y hy hxy => by
    have h1 := (holds_pi_zero H S).1 h x hx
    have h2 := (holds_pi_zero H S).1 h1 y (by rwa [El_shift])
    have h3 := (holds_pi_zero H S).1 h2 empty (by
      show holds (app (app (interp H S (Quot.shiftN 2 R) (cons y (cons x ρ))) x) y)
      rwa [interp_shiftN, interp_shiftN])
    change holds (app (app (interp H S (Quot.shiftN 3 R) (cons empty (cons y (cons x ρ)))) y) x) at h3
    rwa [interp_shiftN, interp_shiftN, interp_shiftN] at h3

theorem trans_sem (h : holds (interp H S (Quot.transTy l A R) ρ)) :
    ∀ x, x ∈ₛ El (interp H S A ρ) → ∀ y, y ∈ₛ El (interp H S A ρ) → ∀ z, z ∈ₛ El (interp H S A ρ) →
      holds (app (app (interp H S R ρ) x) y) → holds (app (app (interp H S R ρ) y) z) →
      holds (app (app (interp H S R ρ) x) z) :=
  fun x hx y hy z hz hxy hyz => by
    have h1 := (holds_pi_zero H S).1 h x hx
    have h2 := (holds_pi_zero H S).1 h1 y (by rwa [El_shift])
    have h3 := (holds_pi_zero H S).1 h2 z (by
      show z ∈ₛ El (interp H S (Quot.shiftN 2 A) (cons y (cons x ρ)))
      rwa [interp_shiftN, interp_shiftN])
    have h4 := (holds_pi_zero H S).1 h3 empty (by
      show holds (app (app (interp H S (Quot.shiftN 3 R) (cons z (cons y (cons x ρ)))) x) y)
      rwa [interp_shiftN, interp_shiftN, interp_shiftN])
    have h5 := (holds_pi_zero H S).1 h4 empty (by
      show holds (app (app (interp H S (Quot.shiftN 4 R) (cons empty (cons z (cons y (cons x ρ))))) y) z)
      rwa [interp_shiftN, interp_shiftN, interp_shiftN, interp_shiftN])
    change holds (app (app (interp H S (Quot.shiftN 5 R)
      (cons empty (cons empty (cons z (cons y (cons x ρ)))))) x) z) at h5
    rwa [interp_shiftN, interp_shiftN, interp_shiftN, interp_shiftN, interp_shiftN] at h5

theorem rel_sem (h : interp H S R ρ ∈ₛ El (interp H S (Quot.relTy l A) ρ)) :
    ∀ x, x ∈ₛ El (interp H S A ρ) → ∀ y, y ∈ₛ El (interp H S A ρ) →
      app (app (interp H S R ρ) x) y ∈ₛ H.U 0 := fun x hx y hy => by
  have h1 := app_mem_pi H S h hx
  have h2 := app_mem_pi H S h1 (by rwa [El_shift])
  rwa [El_sort] at h2

theorem resp_sem {m : Nat} {P f : Tm} (h : holds (interp H S (Quot.respTy l m A R P f) ρ)) :
    ∀ x, x ∈ₛ El (interp H S A ρ) → ∀ y, y ∈ₛ El (interp H S A ρ) →
      holds (app (app (interp H S R ρ) x) y) → app (interp H S f ρ) x = app (interp H S f ρ) y :=
  fun x hx y hy hxy => by
    simp only [Quot.respTy] at h
    have h1 := (holds_pi_zero H S).1 h x hx
    have h2 := (holds_pi_zero H S).1 h1 y (by rwa [El_shift])
    have h3 := (holds_pi_zero H S).1 h2 empty (by
      show holds (app (app (interp H S (Quot.shiftN 2 R) (cons y (cons x ρ))) x) y)
      rwa [interp_shiftN, interp_shiftN])
    change holds (propCode (app (interp H S (Quot.shiftN 3 f) (cons empty (cons y (cons x ρ)))) x =
      app (interp H S (Quot.shiftN 3 f) (cons empty (cons y (cons x ρ)))) y)) at h3
    rwa [holds_propCode, interp_shiftN, interp_shiftN, interp_shiftN] at h3

/-- With an equivalence, two elements have the same class exactly when related. -/
theorem cls_eq_iff {a r : ZFSet.{u}}
    (refl : ∀ x, x ∈ₛ El a → holds (app (app r x) x))
    (sym : ∀ x, x ∈ₛ El a → ∀ y, y ∈ₛ El a → holds (app (app r x) y) → holds (app (app r y) x))
    (trans : ∀ x, x ∈ₛ El a → ∀ y, y ∈ₛ El a → ∀ z, z ∈ₛ El a →
      holds (app (app r x) y) → holds (app (app r y) z) → holds (app (app r x) z))
    {x y : ZFSet.{u}} (hx : x ∈ₛ El a) (hy : y ∈ₛ El a) :
    cls a r x = cls a r y ↔ holds (app (app r x) y) := by
  constructor
  · intro e
    have : y ∈ₛ cls a r x := e ▸ mem_cls.2 ⟨hy, refl y hy⟩
    exact (mem_cls.1 this).2
  · intro hxy
    apply ext
    intro z
    rw [mem_cls, mem_cls]
    exact ⟨fun ⟨hz, h⟩ => ⟨hz, trans y hy x hx z hz (sym x hx y hy hxy) h⟩,
      fun ⟨hz, h⟩ => ⟨hz, trans x hx y hy z hz hxy h⟩⟩

end QuotSem

/-! ## Methods -/

section Method

variable {Sk : Glob → ZFSet.{u}} {k : Nat} {d : Decl}
  (hglob : ∀ g, g.decl = k → S g = meaning H Sk k d g)
  (hbelow : ∀ g, g.decl < k → S g = Sk g)
include hglob hbelow

omit hbelow in
/-- A value of the family is a value of its type. -/
theorem member_El (DW : DeclWith (Valid H Sk) k d) {a ys : List ZFSet.{u}} {x : ZFSet.{u}}
    (ha : ArgsOk H Sk [] a (d.params.take d.uniform)) (hys : ArgsOk H Sk a ys d.indices)
    (hm : Member (semCtors H Sk d) a ys x) : x ∈ₛ El (apps (S (.data k)) (a ++ ys)) := by
  have hal : a.length = d.uniform := by
    rw [ha.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
  have htop : ∀ v ∈ a ++ ys, v ∈ₛ top H := fun v hv => by
    rcases List.mem_append.1 hv with hv | hv
    · exact ha.top v hv
    · exact hys.top v hv
  have hlen : (a ++ ys).length = d.params.length := by
    rw [List.length_append, hal, hys.length, Decl.indices, List.length_drop]
    have := DW.uniform_le; omega
  rw [apps_data hglob htop hlen, Family.El_dataCode, List.take_left' hal, List.drop_left' hal]
  exact mem_famSet.2 hm

omit hglob in
/-- The method's field binders hold the fields. -/
theorem method_fields (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors)
    {a M : List ZFSet.{u}} (hal : a.length = d.uniform) :
    {Fs P : List Field} → {done fs : List ZFSet.{u}} → C.fields = P ++ Fs → done.length = P.length →
    FieldsOk a (Member (semCtors H Sk d) a) (Fs.map (sField H Sk)) done.reverse fs →
    (∀ (i : Nat) (ys : List Tm) (x : ZFSet.{u}), Fs[i]? = some (Field.recur ys) →
      Member (semCtors H Sk d) a
        (ys.map (interp H Sk · (envOf ((done ++ fs.take i).reverse ++ a.reverse)))) x →
      x ∈ₛ El (apps (S (.data k))
        (a ++ ys.map (interp H S · (envOf ((done ++ fs.take i).reverse ++ a.reverse)))))) →
    TeleFit H S (done.reverse ++ M ++ a.reverse)
      (Decl.liftTele M.length P.length (fieldsTele k d P.length Fs)) fs
  | [], _, _, [], _, _, _, _ => trivial
  | .plain A l :: Fs, P, done, x :: fs, hP, hd, ⟨hx, hfs⟩, hR => by
    have hA : A.globsBelow k := (DW.inScope.2 C hC).2 (.plain A l) (by rw [hP]; simp)
    have e := interp_lift H S (t := A) (L := done.reverse) (M := M) (R := a.reverse)
    rw [List.length_reverse, hd] at e
    refine ⟨?_, ?_⟩
    · show x ∈ₛ El (interp H S (Tm.lift P.length M.length A) (envOf (done.reverse ++ M ++ a.reverse)))
      rw [e, interp_decl hbelow hA]; exact hx
    · have := method_fields DW hC hal (M := M) (Fs := Fs) (P := P ++ [.plain A l]) (done := done ++ [x])
        (by simp [hP]) (by simp [hd]) (by simpa using hfs) (fun i ys y hi hy => by
          have := hR (i + 1) ys y (by simpa using hi) (by simpa using hy)
          simpa using this)
      simpa [fieldsTele, Decl.liftTele] using this
  | .recur ys :: Fs, P, done, x :: fs, hP, hd, ⟨hx, hfs⟩, hR => by
    have e := interp_lift H S (t := Field.ty k d.uniform P.length (.recur ys)) (L := done.reverse)
      (M := M) (R := a.reverse)
    rw [List.length_reverse, hd] at e
    refine ⟨?_, ?_⟩
    · show x ∈ₛ El (interp H S (Tm.lift P.length M.length (Field.ty k d.uniform P.length (.recur ys)))
        (envOf (done.reverse ++ M ++ a.reverse)))
      rw [e, Field.ty, interp_apps, List.map_append,
        paramVars_interp (H := H) (S := S) (by simp [hd]) hal]
      have := hR 0 ys x rfl (by simpa using hx)
      simp only [List.take_zero, List.append_nil] at this
      exact this
    · have := method_fields DW hC hal (M := M) (Fs := Fs) (P := P ++ [.recur ys]) (done := done ++ [x])
        (by simp [hP]) (by simp [hd]) (by simpa using hfs) (fun i ys' y hi hy => by
          have := hR (i + 1) ys' y (by simpa using hi) (by simpa using hy)
          simpa using this)
      simpa [fieldsTele, Decl.liftTele] using this
  | .plain _ _ :: _, _, _, [], _, _, h, _ => h.elim
  | .recur _ :: _, _, _, [], _, _, h, _ => h.elim
  | [], _, _, _ :: _, _, _, h, _ => h.elim

omit hglob hbelow in
theorem ihsOk_nil {a pre xs rs : List ZFSet.{u}} {P : ZFSet.{u}} (h : IhsOk a P [] pre xs rs) :
    rs = [] := by
  cases xs <;> cases rs <;> first | rfl | exact h.elim

omit hglob in
/-- The method's induction hypotheses' binders hold the recursor's results. -/
theorem method_ihs (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors) (m : Nat)
    {a M fs : List ZFSet.{u}} {P : ZFSet.{u}} (hM : M[M.length - 1]? = some P) (hM1 : 0 < M.length)
    (hfl : fs.length = C.fields.length) :
    {Fs Q : List Field} → {rsDone rs : List ZFSet.{u}} → C.fields = Q ++ Fs →
    IhsOk a P (Fs.map (sField H Sk)) (fs.take Q.length).reverse (fs.drop Q.length) rs →
    TeleFit H S (rsDone.reverse ++ fs.reverse ++ M ++ a.reverse)
      (Decl.ihTele m M.length fs.length Fs Q.length rsDone.length) rs
  | [], _, _, rs, _, h => by rw [ihsOk_nil h]; trivial
  | F :: Fs, Q, rsDone, rs, hQ, h => by
    have hlt : Q.length < fs.length := by rw [hfl, hQ]; simp
    have hdrop : fs.drop Q.length = fs[Q.length] :: fs.drop (Q.length + 1) := by
      rw [List.drop_eq_getElem_cons hlt]
    have htake : (fs.take (Q.length + 1)).reverse = fs[Q.length] :: (fs.take Q.length).reverse := by
      rw [List.take_add_one, List.getElem?_eq_getElem hlt]; simp
    rw [hdrop] at h
    cases F with
    | plain A l =>
      have h' : IhsOk a P (Fs.map (sField H Sk)) (fs[Q.length] :: (fs.take Q.length).reverse)
          (fs.drop (Q.length + 1)) rs := h
      rw [← htake] at h'
      have := method_ihs DW hC m hM hM1 hfl (Fs := Fs) (Q := Q ++ [.plain A l]) (rsDone := rsDone)
        (by simp [hQ]) (by rw [List.length_append, List.length_singleton]; exact h')
      rw [List.length_append, List.length_singleton] at this
      exact this
    | recur ys =>
      obtain ⟨r, rs, rfl⟩ : ∃ r rs', rs = r :: rs' := by
        cases rs with
        | nil => exact h.elim
        | cons r rs => exact ⟨r, rs, rfl⟩
      obtain ⟨hr, h⟩ := h
      refine ⟨?_, ?_⟩
      · have hys : ∀ y ∈ ys, y.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (by rw [hQ]; simp)
        have hP : envOf (rsDone.reverse ++ fs.reverse ++ M ++ a.reverse)
            (rsDone.length + fs.length + M.length - 1) = P := by
          simp only [List.append_assoc]
          rw [envOf_append_ge (by simp; omega), envOf_append_ge (by simp; omega),
            envOf_append_lt (by simp; omega)]
          unfold envOf
          have e : rsDone.length + fs.length + M.length - 1 - rsDone.reverse.length -
              fs.reverse.length = M.length - 1 := by simp; omega
          rw [e, hM]; rfl
        have hx : envOf (rsDone.reverse ++ fs.reverse ++ M ++ a.reverse)
            (rsDone.length + fs.length - 1 - Q.length) = fs[Q.length] := by
          simp only [List.append_assoc]
          rw [envOf_append_ge (by simp; omega), envOf_append_lt (by simp; omega),
            envOf_reverse (by simp; omega)]
          unfold envOf
          have e : fs.length - 1 - (rsDone.length + fs.length - 1 - Q.length - rsDone.reverse.length) =
              Q.length := by simp; omega
          rw [e, List.getElem?_eq_getElem hlt]; rfl
        have hren : ∀ y ∈ ys, interp H S (y.rename fun n => Tm.liftAt Q.length M.length n +
              (rsDone.length + fs.length - Q.length)) (envOf (rsDone.reverse ++ fs.reverse ++ M ++ a.reverse)) =
            interp H Sk y (envOf ((fs.take Q.length).reverse ++ a.reverse)) := fun y hy => by
          rw [interp_rename_env H S (ρ' := envOf ((fs.take Q.length).reverse ++ a.reverse)) fun n => by
            have := envOf_ihRename (R₁ := rsDone.reverse) (M := M) (a := a) (Nat.le_of_lt hlt) n
            simpa using this]
          exact interp_decl hbelow (hys y hy) _
        show r ∈ₛ El (interp H S (Tm.apps (.var (rsDone.length + fs.length + M.length - 1))
          (ys.map (fun y => y.rename fun n => Tm.liftAt Q.length M.length n +
            (rsDone.length + fs.length - Q.length)) ++
            [.var (rsDone.length + fs.length - 1 - Q.length)])) _)
        have hmap : (ys.map fun y => y.rename fun n => Tm.liftAt Q.length M.length n +
              (rsDone.length + fs.length - Q.length)).map
              (interp H S · (envOf (rsDone.reverse ++ fs.reverse ++ M ++ a.reverse))) =
            ys.map (interp H Sk · (envOf ((fs.take Q.length).reverse ++ a.reverse))) := by
          rw [List.map_map]; exact List.map_congr_left hren
        rw [interp_apps, List.map_append, hmap]
        show r ∈ₛ El (apps (envOf _ (rsDone.length + fs.length + M.length - 1))
          (_ ++ [envOf _ (rsDone.length + fs.length - 1 - Q.length)]))
        rw [hP, hx]
        exact hr
      · rw [← htake] at h
        have := method_ihs DW hC m hM hM1 hfl (Fs := Fs) (Q := Q ++ [.recur ys])
          (rsDone := rsDone ++ [r]) (by simp [hQ])
          (by rw [List.length_append, List.length_singleton]; exact h)
        rw [List.length_append, List.length_singleton, List.length_append, List.length_singleton]
          at this
        simpa using this

/-- The method's result is the motive at the constructor's indices and value. -/
theorem method_result (DW : DeclWith (Valid H Sk) k d) {c : Nat} {C : Ctor} (hc : d.ctors[c]? = some C)
    {a M fs rs : List ZFSet.{u}} {P : ZFSet.{u}} (hM : M[M.length - 1]? = some P)
    (hMl : M.length = 1 + c) (hal : a.length = d.uniform) (hfl : fs.length = C.fields.length)
    (hrl : rs.length = Decl.Ctor.recCount C) (htop : ∀ v ∈ a ++ fs, v ∈ₛ top H) :
    interp H S (Decl.methodResult k d c C) (envOf (rs.reverse ++ fs.reverse ++ M ++ a.reverse)) =
      apps P ((sCtor H Sk C).result (fs.reverse ++ a.reverse) ++ [val c fs]) := by
  have hC := List.mem_of_getElem? hc
  simp only [Decl.methodResult]
  rw [interp_apps, List.map_append, List.map_map]
  have hP : interp H S (.var (Decl.Ctor.recCount C + C.fields.length + (1 + c) - 1))
      (envOf (rs.reverse ++ fs.reverse ++ M ++ a.reverse)) = P := by
    show envOf _ _ = P
    simp only [List.append_assoc]
    rw [envOf_append_ge (by simp; omega), envOf_append_ge (by simp; omega),
      envOf_append_lt (by simp; omega)]
    unfold envOf
    have e : Decl.Ctor.recCount C + C.fields.length + (1 + c) - 1 - rs.reverse.length -
        fs.reverse.length = M.length - 1 := by simp; omega
    rw [e, hM]; rfl
  have hres : ∀ y ∈ C.result,
      ((fun x => interp H S x (envOf (rs.reverse ++ fs.reverse ++ M ++ a.reverse))) ∘
        fun y => y.rename fun n => Tm.liftAt C.fields.length (1 + c) n + Decl.Ctor.recCount C) y =
      interp H Sk y (envOf (fs.reverse ++ a.reverse)) := fun y hy => by
    simp only [Function.comp]
    rw [interp_rename_env H S (ρ' := envOf (fs.reverse ++ a.reverse)) fun n => by
      have := envOf_ihRename (R₁ := rs.reverse) (fs := fs) (M := M) (a := a) (Nat.le_refl _) n
      simp only [List.length_reverse, Nat.add_sub_cancel, List.take_length] at this
      rw [hfl, hMl, hrl] at this
      exact this]
    exact interp_decl hbelow ((DW.inScope.2 C hC).1 y hy) _
  rw [hP, List.map_congr_left hres]
  congr 2
  simp only [List.map_cons, List.map_nil, List.cons.injEq, and_true]
  rw [interp_apps, List.map_append]
  show apps (S (.ctor k c)) _ = _
  rw [show (paramVars d.uniform (Decl.Ctor.recCount C + C.fields.length + (1 + c))).map
      (interp H S · (envOf (rs.reverse ++ fs.reverse ++ M ++ a.reverse))) = a by
    have := paramVars_interp' (H := H) (S := S) (L := rs.reverse ++ fs.reverse ++ M) (a := a)
      (R := []) (j := Decl.Ctor.recCount C + C.fields.length + (1 + c)) (by simp; omega) hal
    simpa using this]
  rw [show (paramVars C.fields.length (Decl.Ctor.recCount C)).map
      (interp H S · (envOf (rs.reverse ++ fs.reverse ++ M ++ a.reverse))) = fs by
    have := paramVars_interp' (H := H) (S := S) (L := rs.reverse) (a := fs) (R := M ++ a.reverse)
      (j := Decl.Ctor.recCount C) (by simp [hrl]) hfl
    simpa [List.append_assoc] using this]
  exact apps_ctor hglob hc htop hal hfl

omit hglob hbelow in
theorem ihsOk_length {a : List ZFSet.{u}} {P : ZFSet.{u}} :
    {Fs : List Field} → {pre xs rs : List ZFSet.{u}} → IhsOk a P (Fs.map (sField H Sk)) pre xs rs →
    rs.length = (Fs.filter Field.isRec).length
  | [], _, xs, rs, h => by rw [ihsOk_nil h]; rfl
  | .plain _ _ :: _, _, _ :: _, _, h => by
    simp only [List.filter_cons, Field.isRec]; exact ihsOk_length h
  | .recur _ :: _, _, _ :: _, _ :: _, ⟨_, h⟩ => by
    simp only [List.filter_cons, Field.isRec, ↓reduceIte, List.length_cons]
    exact congrArg (· + 1) (ihsOk_length h)
  | .plain _ _ :: _, _, [], _, h => h.elim
  | .recur _ :: _, _, [], _, h => h.elim
  | .recur _ :: _, _, _ :: _, [], h => h.elim

/-- Data-Elim, in the model. -/
theorem elim_mem (DW : DeclWith (Valid H Sk) k d) (m : Nat) {ps ms xs : List ZFSet.{u}}
    {P x : ZFSet.{u}} (hpl : ps.length = d.uniform) (hml : ms.length = d.ctors.length)
    (hA : ArgsOk H S [] (ps ++ [P] ++ ms ++ xs ++ [x]) (d.elimTele k m)) :
    apps (S (.elim k m)) (ps ++ [P] ++ ms ++ xs ++ [x]) ∈ₛ El (apps P (xs ++ [x])) := by
  have hT1 : (d.params.take d.uniform).length = d.uniform := by
    rw [List.length_take]; exact Nat.min_eq_left DW.uniform_le
  have hT3 : (d.ctors.mapIdx (Decl.method k d m)).length = d.ctors.length := List.length_mapIdx
  have hT4 : (d.indices.mapIdx fun i A => (Tm.lift i (1 + d.ctors.length) A.1, A.2)).length =
      d.indices.length := List.length_mapIdx
  simp only [Decl.elimTele] at hA
  have hAll := hA
  have hxl : xs.length = d.indices.length := by
    have := hA.length
    simp only [List.length_append, List.length_singleton, hT1, hT3, hT4, hpl, hml] at this
    omega
  obtain ⟨hA, hA5⟩ := (ArgsOk.append (by simp [hpl, hml, hxl, hT1, hT3, hT4])).1 hA
  obtain ⟨hA, hA4⟩ := (ArgsOk.append (by simp [hpl, hml, hT1, hT3])).1 hA
  obtain ⟨hA, hA3⟩ := (ArgsOk.append (by simp [hpl, hT1])).1 hA
  obtain ⟨hA1, -⟩ := (ArgsOk.append (by simp [hpl, hT1])).1 hA
  have hA1' := (params_congr hbelow DW).1 hA1
  -- The value is in the family at its indices.
  obtain ⟨-, hmaj, -⟩ := hA5
  simp only [List.nil_append] at hmaj hA4
  rw [interp_apps, List.map_append] at hmaj
  have hpv : (paramVars d.uniform (d.indices.length + d.ctors.length + 1)).map
      (interp H S · (envOf (ps ++ [P] ++ ms ++ xs).reverse)) = ps := by
    have := paramVars_interp' (H := H) (S := S) (L := xs.reverse ++ ms.reverse ++ [P]) (a := ps)
      (R := []) (j := d.indices.length + d.ctors.length + 1) (by simp [hxl, hml]; omega) hpl
    simpa using this
  have hxv : (paramVars d.indices.length 0).map
      (interp H S · (envOf (ps ++ [P] ++ ms ++ xs).reverse)) = xs := by
    have := paramVars_interp' (H := H) (S := S) (L := []) (a := xs) (R := ms.reverse ++ [P] ++ ps.reverse)
      (j := 0) rfl hxl
    simpa using this
  rw [hpv, hxv] at hmaj
  have htop4 : ∀ v ∈ ps ++ xs, v ∈ₛ top H := fun v hv => by
    rcases List.mem_append.1 hv with hv | hv
    · exact hA1.top v hv
    · exact hA4.top v hv
  have hlen : (ps ++ xs).length = d.params.length := by
    rw [List.length_append, hpl, hxl, Decl.indices, List.length_drop]; have := DW.uniform_le; omega
  change x ∈ₛ El (apps (S (.data k)) (ps ++ xs)) at hmaj
  rw [apps_data hglob htop4 hlen, Family.El_dataCode, List.take_left' hpl, List.drop_left' hpl] at hmaj
  have hx := mem_famSet.1 hmaj
  -- The recursor.
  have hlenAll : (ps ++ [P] ++ ms ++ xs ++ [x]).length =
      d.uniform + 1 + d.ctors.length + d.indices.length + 1 := by
    rw [List.length_append, List.length_append, List.length_append, List.length_append,
      List.length_singleton, List.length_singleton, hpl, hml, hxl]
  rw [hglob _ rfl]
  simp only [meaning]
  rw [← hlenAll, apps_curry _ _ hAll.top]
  have e1 : (ps ++ [P] ++ ms ++ xs ++ [x]).take d.uniform = ps := by
    simp only [List.append_assoc]; exact List.take_left' hpl
  have e2 : ((ps ++ [P] ++ ms ++ xs ++ [x]).drop (d.uniform + 1)).take d.ctors.length = ms := by
    have : (ps ++ [P]).length = d.uniform + 1 := by simp [hpl]
    rw [show ps ++ [P] ++ ms ++ xs ++ [x] = (ps ++ [P]) ++ (ms ++ (xs ++ [x])) by
        simp only [List.append_assoc],
      List.drop_left' this, List.take_left' hml]
  have e3 : (ps ++ [P] ++ ms ++ xs ++ [x]).getLastD empty = x := by
    rw [List.getLastD_eq_getLast?, List.getLast?_concat]; rfl
  rw [e1, e2, e3]
  refine recV_mem (fun c C' fs hc' hF rs hI => ?_) hx
  obtain ⟨C, hc, rfl⟩ := semCtors_get hc'
  have hC := List.mem_of_getElem? hc
  have hcl : c < d.ctors.length := (List.getElem?_eq_some_iff.1 hc).1
  have hcm : c < ms.length := by omega
  have hfl : fs.length = C.fields.length := by simpa [sCtor] using hF.length
  -- The method's type.
  have hmeth := ArgsOk.get H S hA3 c hcm (by rw [hT3]; exact hcl)
  simp only [List.getElem_mapIdx, List.nil_append] at hmeth
  have eC : d.ctors[c] = C := (List.getElem?_eq_some_iff.1 hc).2
  rw [eC] at hmeth
  simp only [Decl.method] at hmeth
  have eM : (ps ++ [P] ++ ms.take c).reverse = ((ms.take c).reverse ++ [P]) ++ ps.reverse := by simp
  rw [eM] at hmeth
  have hMl : ((ms.take c).reverse ++ [P]).length = 1 + c := by
    simp [List.length_take]; omega
  have hM : ((ms.take c).reverse ++ [P])[((ms.take c).reverse ++ [P]).length - 1]? = some P := by
    simp
  have hM1 : 0 < ((ms.take c).reverse ++ [P]).length := by rw [hMl]; omega
  -- The fields' binders.
  have hpl' : ps.length = d.uniform := hpl
  have w := walk DW hC (P := []) (done := []) (R := Member (semCtors H Sk d) ps) (Rest := [])
    (DW.fields C hC) (by simp) rfl
    (by simpa [zeroRec_nil] using envOk_params hA1') hpl' (by simpa [sCtor] using hF)
  have part1 := method_fields H S hbelow DW hC hpl (M := (ms.take c).reverse ++ [P]) (Fs := C.fields)
    (P := []) (done := []) (fs := fs) rfl rfl (by simpa [sCtor] using hF)
    (fun i ys y hi hy => by
      have hfit := w.2 i ys hi
      have hys : ∀ t ∈ ys, t.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (List.mem_of_getElem? hi)
      rw [List.map_congr_left fun t ht => interp_decl hbelow (hys t ht) _]
      exact member_El H S hglob DW hA1' hfit hy)
  have part2 := method_ihs H S hbelow DW hC m (M := (ms.take c).reverse ++ [P]) (fs := fs) hM hM1 hfl
    (Fs := C.fields) (Q := []) (rsDone := []) rfl (by simpa [sCtor] using hI)
  have hrl : rs.length = Decl.Ctor.recCount C := by
    have := ihsOk_length H (Sk := Sk) (Fs := C.fields) (by simpa [sCtor] using hI)
    simpa [Decl.Ctor.recCount] using this
  have htopfs : ∀ v ∈ ps ++ fs, v ∈ₛ top H := fun v hv => by
    rcases List.mem_append.1 hv with hv | hv
    · exact hA1.top v hv
    · exact member_fields_top DW hA1' hc' hF v hv
  have hfit : TeleFit H S ((ms.take c).reverse ++ [P] ++ ps.reverse)
      (Decl.methodTele k d m c C) (fs ++ rs) := by
    unfold Decl.methodTele Decl.fieldTele
    refine TeleFit.append H S ?_ ?_ ?_
    · rw [Decl.liftTele_length, Ctor.tele, fieldsTele_length, hfl]
    · rw [← hMl]; simpa [Ctor.tele] using part1
    · rw [← hMl, ← hfl]; simpa using part2
  have := apps_mem_pis H S hmeth hfit
  rw [show (fs ++ rs).reverse ++ ((ms.take c).reverse ++ [P] ++ ps.reverse) =
      rs.reverse ++ fs.reverse ++ ((ms.take c).reverse ++ [P]) ++ ps.reverse by simp,
    method_result H S hglob hbelow DW hc hM hMl hpl hfl hrl htopfs] at this
  rw [List.getElem?_eq_getElem hcm]
  exact this
omit hbelow in
/-- The recursor, applied to all its arguments. -/
theorem apps_elim (m : Nat) {ps ms xs : List ZFSet.{u}} {P x : ZFSet.{u}}
    (htop : ∀ v ∈ ps ++ [P] ++ ms ++ xs ++ [x], v ∈ₛ top H) (hpl : ps.length = d.uniform)
    (hml : ms.length = d.ctors.length) (hxl : xs.length = d.indices.length) :
    apps (S (.elim k m)) (ps ++ [P] ++ ms ++ xs ++ [x]) = recV (semCtors H Sk d) ps ms x := by
  have hlenAll : (ps ++ [P] ++ ms ++ xs ++ [x]).length =
      d.uniform + 1 + d.ctors.length + d.indices.length + 1 := by
    rw [List.length_append, List.length_append, List.length_append, List.length_append,
      List.length_singleton, List.length_singleton, hpl, hml, hxl]
  rw [hglob _ rfl]
  simp only [meaning]
  rw [← hlenAll, apps_curry _ _ htop]
  have e1 : (ps ++ [P] ++ ms ++ xs ++ [x]).take d.uniform = ps := by
    simp only [List.append_assoc]; exact List.take_left' hpl
  have e2 : ((ps ++ [P] ++ ms ++ xs ++ [x]).drop (d.uniform + 1)).take d.ctors.length = ms := by
    have : (ps ++ [P]).length = d.uniform + 1 := by simp [hpl]
    rw [show ps ++ [P] ++ ms ++ xs ++ [x] = (ps ++ [P]) ++ (ms ++ (xs ++ [x])) by
        simp only [List.append_assoc],
      List.drop_left' this, List.take_left' hml]
  have e3 : (ps ++ [P] ++ ms ++ xs ++ [x]).getLastD empty = x := by
    rw [List.getLastD_eq_getLast?, List.getLast?_concat]; rfl
  rw [e1, e2, e3]

omit hglob hbelow in
theorem interp_getD (ts : List Tm) (c : Nat) (ρ : Nat → ZFSet.{u}) :
    interp H S (ts.getD c .irr) ρ = (ts.map (interp H S · ρ))[c]?.getD empty := by
  rw [List.getD_eq_getElem?_getD, List.getElem?_map]
  cases ts[c]? <;> rfl

/-- The induction hypotheses Data-Iota passes are the recursor's results. -/
theorem interp_ihArgs (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors) (m : Nat)
    {ps ms fs : List Tm} {P : Tm} {ρ : Nat → ZFSet.{u}}
    (hpl : ps.length = d.uniform) (hml : ms.length = d.ctors.length) (hfl : fs.length = C.fields.length)
    (htop : ∀ v ∈ (ps ++ [P] ++ ms ++ fs).map (interp H S · ρ), v ∈ₛ top H)
    (hfit : ∀ (i : Nat) (ys : List Tm), C.fields[i]? = some (Field.recur ys) →
      ArgsOk H Sk (ps.map (interp H S · ρ))
        (ys.map (interp H Sk · (envOf (((fs.map (interp H S · ρ)).take i).reverse ++
          (ps.map (interp H S · ρ)).reverse)))) d.indices) :
    {Fs Q : List Field} → C.fields = Q ++ Fs →
    (Decl.ihArgs k m ps P ms fs Fs Q.length).map (interp H S · ρ) =
      ihs (recV (semCtors H Sk d) (ps.map (interp H S · ρ)) (ms.map (interp H S · ρ)))
        (Fs.map (sField H Sk)) ((fs.map (interp H S · ρ)).drop Q.length)
  | [], Q, hQ => by
    have : Q.length = fs.length := by rw [hfl, hQ]; simp
    simp [Decl.ihArgs, ihs, this]
  | F :: Fs, Q, hQ => by
    have hlt : Q.length < fs.length := by rw [hfl, hQ]; simp
    have hdrop : (fs.map (interp H S · ρ)).drop Q.length =
        interp H S fs[Q.length] ρ :: (fs.map (interp H S · ρ)).drop (Q.length + 1) := by
      rw [List.drop_eq_getElem_cons (by simpa using hlt)]; simp
    rw [hdrop]
    cases F with
    | plain A l =>
      have := interp_ihArgs DW hC m hpl hml hfl htop hfit (Fs := Fs) (Q := Q ++ [.plain A l])
        (by simp [hQ])
      simp only [List.length_append, List.length_singleton] at this
      simpa [Decl.ihArgs, ihs, sField] using this
    | recur ys =>
      have := interp_ihArgs DW hC m hpl hml hfl htop hfit (Fs := Fs) (Q := Q ++ [.recur ys])
        (by simp [hQ])
      simp only [List.length_append, List.length_singleton] at this
      simp only [Decl.ihArgs, List.map_cons, sField, ihs, this, List.cons.injEq, and_true]
      have hget : C.fields[Q.length]? = some (.recur ys) := by
        rw [hQ, List.getElem?_append_right (Nat.le_refl _)]; simp
      have hf := hfit Q.length ys hget
      have hys : ∀ y ∈ ys, y.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (List.mem_of_getElem? hget)
      rw [interp_apps, List.map_append, List.map_append, List.map_append, List.map_append, List.map_map]
      have eys : ys.map ((interp H S · ρ) ∘ Tm.substL (ps ++ fs.take Q.length)) =
          ys.map (interp H Sk · (envOf (((fs.map (interp H S · ρ)).take Q.length).reverse ++
            (ps.map (interp H S · ρ)).reverse))) := by
        apply List.map_congr_left
        intro y hy
        simp only [Function.comp, interp_substL, List.map_append, List.reverse_append, List.map_take]
        exact interp_decl hbelow (hys y hy) _
      rw [eys]
      have hx : interp H S (fs.getD Q.length .irr) ρ = interp H S fs[Q.length] ρ := by
        rw [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem hlt]; rfl
      simp only [List.map_cons, List.map_nil, hx]
      show apps (S (.elim k m)) _ = _
      apply apps_elim H S hglob m _ (by simpa using hpl) (by simpa using hml) hf.length
      intro v hv
      simp only [List.mem_append, List.mem_singleton] at hv
      rcases hv with ((((hv | hv) | hv) | hv) | hv)
      · exact htop v (by simp_all)
      · exact htop v (by simp_all)
      · exact htop v (by simp_all)
      · exact hf.top v hv
      · subst hv
        exact htop _ (List.mem_map.2 ⟨fs[Q.length], List.mem_append.2 (Or.inr (List.getElem_mem hlt)), rfl⟩)

end Method

/-! ## Inductive types in the signature -/

theorem interp_data {sig : Sig} (hG : Good H sig) {k : Nat} {d : Decl} (hk : sig[k]? = some d)
    {as : List Tm} {ρ : Nat → ZFSet.{u}}
    (has : ArgsOk H (semSig H sig) [] (as.map (interp H (semSig H sig) · ρ)) d.params) :
    interp H (semSig H sig) (Tm.apps (.glob (.data k)) as) ρ =
      dataCode (semCtors H (semSig H (sig.take k)) d) ((as.map (interp H (semSig H sig) · ρ)).take d.uniform) k
        ((as.map (interp H (semSig H sig) · ρ)).drop d.uniform) := by
  obtain ⟨_, hglob, _⟩ := declAt H hG hk
  rw [interp_apps]
  exact apps_data hglob has.top has.length

theorem interp_ctorApp {sig : Sig} (hG : Good H sig) {k c : Nat} {d : Decl} {C : Ctor}
    (hk : sig[k]? = some d) (hc : d.ctors[c]? = some C) {ps fs : List Tm} {ρ : Nat → ZFSet.{u}}
    (hps : ArgsOk H (semSig H sig) [] (ps.map (interp H (semSig H sig) · ρ)) (d.params.take d.uniform))
    (hfs : ArgsOk H (semSig H sig) (ps.map (interp H (semSig H sig) · ρ))
      (fs.map (interp H (semSig H sig) · ρ)) (C.tele k d)) :
    interp H (semSig H sig) (Tm.apps (.glob (.ctor k c)) (ps ++ fs)) ρ =
      val c (fs.map (interp H (semSig H sig) · ρ)) := by
  obtain ⟨DW, hglob, _⟩ := declAt H hG hk
  rw [interp_apps, List.map_append]
  refine apps_ctor hglob hc (fun v hv => ?_) ?_ ?_
  · rcases List.mem_append.1 hv with hv | hv
    · exact hps.top v hv
    · exact hfs.top v hv
  · rw [hps.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
  · rw [hfs.length, Ctor.tele, fieldsTele_length]

/-- Casting fields is the identity in the model. -/
theorem interp_castFields (k : Nat) (d : Decl) (ps ps' fs : List Tm) (ρ : Nat → ZFSet.{u}) :
    (Fs : List Field) → (j : Nat) → (done : List Tm) → j + Fs.length ≤ fs.length →
    (d.castFields k ps ps' fs Fs j done).map (interp H S · ρ) =
      done.map (interp H S · ρ) ++ ((fs.drop j).take Fs.length).map (interp H S · ρ)
  | [], _, done, _ => by simp [Decl.castFields]
  | F :: Fs, j, done, h => by
    simp only [Decl.castFields]
    rw [interp_castFields k d ps ps' fs ρ Fs (j + 1) _ (by simp at h; omega)]
    have hj : j < fs.length := by simp at h; omega
    rw [List.drop_eq_getElem_cons hj]
    simp only [List.map_append, List.map_cons, List.map_nil, List.append_assoc, List.take_succ_cons,
      List.singleton_append, List.length_cons]
    congr 2
    show interp H S (fs.getD j .irr) ρ = _
    rw [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem hj]; rfl

theorem interp_andAll (ρ : Nat → ZFSet.{u}) :
    (Ps : List Tm) → (∀ P ∈ Ps, ∃ q, interp H S P ρ = propCode q) →
    interp H S (andAll Ps) ρ = propCode (∀ P ∈ Ps, holds (interp H S P ρ))
  | [], _ => propCode_congr ⟨fun _ _ h => absurd h List.not_mem_nil, fun _ => trivial⟩
  | [P], h => by
    obtain ⟨q, e⟩ := h P List.mem_cons_self
    show interp H S P ρ = _
    rw [e]
    refine propCode_congr ⟨fun hq P' hP' => ?_, fun h => ?_⟩
    · rw [List.mem_singleton.1 hP', e, holds_propCode]; exact hq
    · have := h P List.mem_cons_self; rwa [e, holds_propCode] at this
  | P :: Q :: Ps, h => by
    show interp H S (Tm.and P (andAll (Q :: Ps))) ρ = _
    rw [Tm.and, interp_and, interp_shift,
      interp_andAll ρ (Q :: Ps) fun P' hP' => h P' (List.mem_cons_of_mem _ hP'), holds_propCode]
    refine propCode_congr ⟨fun ⟨hP, hr⟩ P' hP' => ?_, fun hall => ⟨hall P List.mem_cons_self,
      fun P' hP' => hall P' (List.mem_cons_of_mem _ hP')⟩⟩
    rcases List.mem_cons.1 hP' with e | hP'
    · exact e ▸ hP
    · exact hr P' hP'

/-- Eq-Ctor's conjunction holds when the fields that are not proofs agree. -/
theorem eqFields_iff (k : Nat) (d : Decl) (ps fs fs' : List Tm) (ρ : Nat → ZFSet.{u}) :
    (Fs : List Field) → (j : Nat) →
    (∀ P ∈ d.eqFields k ps fs fs' Fs j, ∃ q, interp H S P ρ = propCode q) ∧
    ((∀ P ∈ d.eqFields k ps fs fs' Fs j, holds (interp H S P ρ)) ↔
      ∀ i (hi : i < Fs.length), Fs[i].level d.level ≠ 0 →
        interp H S (fs.getD (j + i) .irr) ρ = interp H S (fs'.getD (j + i) .irr) ρ)
  | [], _ => ⟨fun _ h => absurd h List.not_mem_nil,
      ⟨fun _ _ hi => absurd hi (Nat.not_lt_zero _), fun _ _ h => absurd h List.not_mem_nil⟩⟩
  | F :: Fs, j => by
    have ih := eqFields_iff k d ps fs fs' ρ Fs (j + 1)
    simp only [Decl.eqFields]
    split
    next h0 =>
      refine ⟨ih.1, ih.2.trans ⟨fun h i hi hl => ?_, fun h i hi hl => ?_⟩⟩
      · cases i with
        | zero => exact absurd h0 hl
        | succ i => rw [show j + (i + 1) = j + 1 + i by omega]; exact h i (by simpa using hi) hl
      · rw [show j + 1 + i = j + (i + 1) by omega]; exact h (i + 1) (by simpa using hi) hl
    next h0 =>
      refine ⟨fun P hP => ?_, ⟨fun h i hi hl => ?_, fun h P hP => ?_⟩⟩
      · rcases List.mem_cons.1 hP with e | hP
        · subst e; exact ⟨_, rfl⟩
        · exact ih.1 P hP
      · cases i with
        | zero =>
          have := h _ List.mem_cons_self
          rwa [show interp H S (Tm.eq _ _ _ _) ρ = propCode _ from rfl, holds_propCode] at this
        | succ i =>
          rw [show j + (i + 1) = j + 1 + i by omega]
          exact ih.2.1 (fun P hP => h P (List.mem_cons_of_mem _ hP)) i (by simpa using hi) hl
      · rcases List.mem_cons.1 hP with e | hP
        · subst e
          rw [show interp H S (Tm.eq _ _ _ _) ρ = propCode _ from rfl, holds_propCode]
          exact h 0 (by simp) h0
        · exact ih.2.2 (fun i hi hl => by
            rw [show j + 1 + i = j + (i + 1) by omega]; exact h (i + 1) (by simpa using hi) hl) P hP

/-- A proof field's value is `∅`. -/
theorem ArgsOk.zero {k : Nat} {d : Decl} : {pre vs : List ZFSet.{u}} → {Fs : List Field} → {j : Nat} →
    ArgsOk H S pre vs (fieldsTele k d j Fs) → ∀ i (hi : i < Fs.length) (hv : i < vs.length),
    Fs[i].level d.level = 0 → vs[i] = empty
  | _, _ :: _, _ :: _, _, ⟨hA, hv, _⟩, 0, _, _, hl => by
    simp only [List.getElem_cons_zero] at hl ⊢
    rw [hl] at hA
    exact eq_empty H hA hv
  | _, _ :: _, _ :: _, _, ⟨_, _, h⟩, i + 1, hi, hv, hl => by
    simp only [List.getElem_cons_succ] at hl ⊢
    exact h.zero i (by simpa using hi) (by simpa using hv) hl
  | _, [], _, _, _, _, _, hv, _ => absurd hv (Nat.not_lt_zero _)
  | _, _ :: _, [], _, h, _, _, _, _ => h.elim

/-! ## Soundness -/

theorem sound {sig : Sig} (hS : S = semSig H sig) (hG : Good H sig) {j : Jdg} (d : Deriv N sig j) :
    Valid H S j := by
  induction d with
  | wfNil => trivial
  | wfCons _ _ => trivial
  | @var Γ i A _ hget _ =>
    intro ρ hρ
    show ρ i ∈ₛ El (interp H S (A.rename (· + (i + 1))) ρ)
    rw [interp_rename]; exact envOk_get H S hρ hget
  | @univ Γ l _ hl _ =>
    intro ρ _
    rw [El_sort]; exact H.univCode_mem hl
  | @piForm Γ A B l l' _ _ ihA ihB =>
    intro ρ hρ
    have hA : interp H S A ρ ∈ₛ H.U l := by have := ihA ρ hρ; rwa [El_sort] at this
    have hB : ∀ x, x ∈ₛ El (interp H S A ρ) → interp H S B (cons x ρ) ∈ₛ H.U l' := fun x hx => by
      have := ihB (cons x ρ) ⟨hρ, hx⟩; rwa [El_sort] at this
    rw [El_sort]
    by_cases hl' : l' = 0
    · subst hl'
      rw [interp_pi_zero]; exact propCode_mem_U0 H _
    · rw [interp_pi H S hl']
      have := H.piCode_mem hl' hA hB
      simp only [Tm.imax, hl', ↓reduceIte]; exact this
  | @lam Γ A B t l l' hl' _ _ _ _ _ iht =>
    intro ρ hρ
    rw [El_pi H S hl']
    exact lam_mem_piSet fun x hx => iht (cons x ρ) ⟨hρ, hx⟩
  | @lamProof Γ A B t l _ _ _ _ ihB iht =>
    intro ρ hρ
    rw [interp_pi_zero]
    show holds (propCode _)
    rw [holds_propCode]
    intro x hx
    have hB := ihB (cons x ρ) ⟨hρ, hx⟩
    rw [El_sort] at hB
    exact holds_of H hB (iht (cons x ρ) ⟨hρ, hx⟩)
  | @app Γ f a A B l l' _ _ ihf iha =>
    intro ρ hρ
    show ZFSet.app (interp H S f ρ) (interp H S a ρ) ∈ₛ El (interp H S (B.inst a) ρ)
    rw [interp_inst]
    have hf := ihf ρ hρ
    have ha := iha ρ hρ
    by_cases hl' : l' = 0
    · subst hl'
      rw [interp_pi_zero, El_propCode] at hf
      obtain ⟨hall, hfe⟩ := mem_ofProp.1 hf
      rw [hfe, app_empty]
      exact hall _ ha
    · rw [El_pi H S hl'] at hf
      exact app_mem_piSet hf ha
  | @sigmaForm Γ A B l l' _ _ ihA ihB =>
    intro ρ hρ
    rw [El_sort]
    by_cases h : l = 0 ∧ l' = 0
    · obtain ⟨rfl, rfl⟩ := h
      rw [interp_and]; exact propCode_mem_U0 H _
    · have hA : interp H S A ρ ∈ₛ H.U l := by have := ihA ρ hρ; rwa [El_sort] at this
      have hB : ∀ x, x ∈ₛ El (interp H S A ρ) → interp H S B (cons x ρ) ∈ₛ H.U l' := fun x hx => by
        have := ihB (cons x ρ) ⟨hρ, hx⟩; rwa [El_sort] at this
      rw [interp_sigma H S h]; exact H.sigmaCode_mem h hA hB
  | @pair Γ A B a b l l' h _ _ _ _ _ _ iha ihb =>
    intro ρ hρ
    have hb := ihb ρ hρ
    rw [interp_inst] at hb
    rw [El_sigma H S h]
    exact mem_sigmaSet.2 ⟨_, iha ρ hρ, _, hb, rfl⟩
  | @andIntro Γ A B a b _ _ _ _ ihA ihB iha ihb =>
    intro ρ hρ
    show holds (interp H S (.sigma 0 0 A B) ρ)
    rw [interp_and, holds_propCode]
    have hA := ihA ρ hρ
    rw [El_sort] at hA
    have ha := iha ρ hρ
    have hAh : holds (interp H S A ρ) := holds_of H hA ha
    refine ⟨hAh, ?_⟩
    have hb := ihb ρ hρ
    rw [interp_inst, eq_empty H hA ha] at hb
    have hB := ihB (cons empty ρ) ⟨hρ, hAh⟩
    rw [El_sort] at hB
    exact holds_of H hB hb
  | @fst Γ p A B l l' _ ihp =>
    intro ρ hρ
    have hp := ihp ρ hρ
    show ZFSet.fst (interp H S p ρ) ∈ₛ _
    by_cases h : l = 0 ∧ l' = 0
    · obtain ⟨rfl, rfl⟩ := h
      rw [interp_and, El_propCode] at hp
      obtain ⟨⟨hA, _⟩, he⟩ := mem_ofProp.1 hp
      rw [he, fst_empty]; exact hA
    · rw [El_sigma H S h] at hp
      obtain ⟨x, hx, y, _, e⟩ := mem_sigmaSet.1 hp
      rw [e, fst_pair]; exact hx
  | @snd Γ p A B l l' _ ihp =>
    intro ρ hρ
    have hp := ihp ρ hρ
    show ZFSet.snd (interp H S p ρ) ∈ₛ El (interp H S (B.inst (.fst p)) ρ)
    rw [interp_inst]
    show _ ∈ₛ El (interp H S B (cons (ZFSet.fst (interp H S p ρ)) ρ))
    by_cases h : l = 0 ∧ l' = 0
    · obtain ⟨rfl, rfl⟩ := h
      rw [interp_and, El_propCode] at hp
      obtain ⟨⟨_, hB⟩, he⟩ := mem_ofProp.1 hp
      rw [he, fst_empty, snd_empty]; exact hB
    · rw [El_sigma H S h] at hp
      obtain ⟨x, _, y, hy, e⟩ := mem_sigmaSet.1 hp
      rw [e, fst_pair, snd_pair]; exact hy
  | @letIn Γ A v t C l _ _ _ _ _ iht =>
    intro ρ hρ
    show interp H S t (cons (interp H S v ρ) ρ) ∈ₛ _
    rw [← interp_inst]; exact iht ρ hρ
  | @top Γ _ _ =>
    intro ρ _
    rw [El_sort]; exact propCode_mem_U0 H _
  | @tt Γ _ _ =>
    intro ρ _
    show holds (propCode True)
    rw [holds_propCode]; trivial
  | @bot Γ _ _ =>
    intro ρ _
    rw [El_sort]; exact propCode_mem_U0 H _
  | @absurd Γ A p l _ _ _ ihp =>
    intro ρ hρ
    have hp : _ ∈ₛ El (propCode False) := ihp ρ hρ
    rw [El_propCode] at hp
    exact (mem_ofProp.1 hp).1.elim
  | @eqForm Γ A a b l _ _ _ _ _ _ _ =>
    intro ρ _
    rw [El_sort]; exact propCode_mem_U0 H _
  | @refl Γ A a l _ _ _ _ _ =>
    intro ρ _
    show holds (propCode _)
    rw [holds_propCode]
  | @transport Γ A t P p u e l _ _ _ _ _ _ _ _ iht ihP ihp _ ihe =>
    intro ρ hρ
    have he : _ ∈ₛ El (propCode (interp H S t ρ = interp H S u ρ)) := ihe ρ hρ
    rw [El_propCode] at he
    have htu := (mem_ofProp.1 he).1
    have hP := ihP ρ hρ
    rw [El_pi H S (by decide : (1 : Nat) ≠ 0)] at hP
    have hPt := app_mem_piSet hP (iht ρ hρ)
    rw [El_sort] at hPt
    have hp : _ ∈ₛ El (ZFSet.app (interp H S P ρ) (interp H S t ρ)) := ihp ρ hρ
    show holds (ZFSet.app (interp H S P ρ) (interp H S u ρ))
    rw [← htu]
    exact holds_of H hPt hp
  | @cast Γ A B e t l _ _ _ _ _ _ ihe iht =>
    intro ρ hρ
    have he : _ ∈ₛ El (propCode (interp H S A ρ = interp H S B ρ)) := ihe ρ hρ
    rw [El_propCode] at he
    show interp H S t ρ ∈ₛ _
    rw [← (mem_ofProp.1 he).1]; exact iht ρ hρ
  | @proof Γ P t _ _ ihP iht =>
    intro ρ hρ
    have hP := ihP ρ hρ
    rw [El_sort] at hP
    exact holds_of H hP (iht ρ hρ)
  | @conv Γ t A B l _ _ iht ihc =>
    intro ρ hρ
    rw [← ihc ρ hρ]; exact iht ρ hρ
  | convRefl _ _ => intro ρ _; rfl
  | convSymm _ ih => intro ρ hρ; exact (ih ρ hρ).symm
  | convTrans _ _ ih ih' => intro ρ hρ; exact (ih ρ hρ).trans (ih' ρ hρ)
  | convType _ _ ih _ => exact ih
  | @piCong Γ A A' B B' l l' _ _ _ ihAA _ ihBB =>
    intro ρ hρ
    have hAA := ihAA ρ hρ
    have hB : ∀ x, x ∈ₛ El (interp H S A ρ) → interp H S B (cons x ρ) = interp H S B' (cons x ρ) :=
      fun x hx => ihBB (cons x ρ) ⟨hρ, hx⟩
    by_cases hl' : l' = 0
    · subst hl'
      rw [interp_pi_zero, interp_pi_zero, ← hAA]
      apply propCode_congr
      exact ⟨fun h x hx => by rw [← hB x hx]; exact h x hx,
             fun h x hx => by rw [hB x hx]; exact h x hx⟩
    · rw [interp_pi H S hl', interp_pi H S hl', ← hAA, lam_congr hB]
  | @lamCong Γ A A' B t t' l l' _ _ _ _ ihAA _ ihtt =>
    intro ρ hρ
    show lam (El (interp H S A ρ)) _ = lam (El (interp H S A' ρ)) _
    rw [← ihAA ρ hρ]
    exact lam_congr fun x hx => ihtt (cons x ρ) ⟨hρ, hx⟩
  | appCong _ _ ihff ihaa =>
    intro ρ hρ
    show ZFSet.app _ _ = ZFSet.app _ _
    rw [ihff ρ hρ, ihaa ρ hρ]
  | @sigmaCong Γ A A' B B' l l' _ _ _ ihAA _ ihBB =>
    intro ρ hρ
    by_cases h : l = 0 ∧ l' = 0
    · obtain ⟨rfl, rfl⟩ := h
      rw [interp_and, interp_and, ← ihAA ρ hρ]
      apply propCode_congr
      constructor
      · rintro ⟨hA, hB⟩; exact ⟨hA, ihBB (cons empty ρ) ⟨hρ, hA⟩ ▸ hB⟩
      · rintro ⟨hA, hB⟩; exact ⟨hA, (ihBB (cons empty ρ) ⟨hρ, hA⟩).symm ▸ hB⟩
    · have hB : ∀ x, x ∈ₛ El (interp H S A ρ) →
          interp H S B (cons x ρ) = interp H S B' (cons x ρ) :=
        fun x hx => ihBB (cons x ρ) ⟨hρ, hx⟩
      rw [interp_sigma H S h, interp_sigma H S h, ← ihAA ρ hρ, lam_congr hB]
  | pairCong _ _ iha ihb =>
    intro ρ hρ
    show ZFSet.pair _ _ = ZFSet.pair _ _
    rw [iha ρ hρ, ihb ρ hρ]
  | fstCong _ ih =>
    intro ρ hρ
    show ZFSet.fst _ = ZFSet.fst _
    rw [ih ρ hρ]
  | sndCong _ ih =>
    intro ρ hρ
    show ZFSet.snd _ = ZFSet.snd _
    rw [ih ρ hρ]
  | @letCong Γ A A' v v' t t' B l _ _ _ _ _ ihv ihvv ihtt =>
    intro ρ hρ
    show interp H S t (cons (interp H S v ρ) ρ) = interp H S t' (cons (interp H S v' ρ) ρ)
    rw [← ihvv ρ hρ]
    exact ihtt _ ⟨hρ, ihv ρ hρ⟩
  | absurdCong _ _ _ _ => intro ρ _; rfl
  | eqCong _ _ _ _ iha ihb =>
    intro ρ hρ
    show propCode _ = propCode _
    rw [iha ρ hρ, ihb ρ hρ]
  | castCong _ _ _ _ _ _ _ iht => exact iht
  | @beta Γ A B t a l l' _ _ _ _ _ _ iha =>
    intro ρ hρ
    show ZFSet.app (lam (El (interp H S A ρ)) _) (interp H S a ρ) = _
    rw [interp_inst, app_lam (iha ρ hρ)]
  | @etaPi Γ f A B l l' hl' _ ihf =>
    intro ρ hρ
    have hf := ihf ρ hρ
    rw [El_pi H S hl'] at hf
    show interp H S f ρ = lam (El (interp H S A ρ)) fun x => ZFSet.app (interp H S f.shift (cons x ρ)) x
    simp only [interp_shift]
    exact (lam_app hf).symm
  | fstPair _ _ => intro ρ _; exact fst_pair _ _
  | sndPair _ _ => intro ρ _; exact snd_pair _ _
  | @etaSigma Γ p q A B l l' h _ _ _ _ ihp ihq ihf ihs =>
    intro ρ hρ
    have hp := ihp ρ hρ
    have hq := ihq ρ hρ
    rw [El_sigma H S h] at hp hq
    obtain ⟨x, _, y, _, ep⟩ := mem_sigmaSet.1 hp
    obtain ⟨x', _, y', _, eq⟩ := mem_sigmaSet.1 hq
    have hf : ZFSet.fst (interp H S p ρ) = ZFSet.fst (interp H S q ρ) := ihf ρ hρ
    have hs : ZFSet.snd (interp H S p ρ) = ZFSet.snd (interp H S q ρ) := ihs ρ hρ
    rw [ep, eq, fst_pair, fst_pair] at hf
    rw [ep, eq, snd_pair, snd_pair] at hs
    rw [ep, eq, hf, hs]
  | @zeta Γ A v t C _ _ =>
    intro ρ _
    show interp H S t (cons (interp H S v ρ) ρ) = _
    rw [interp_inst]
  | @proofIrrelevance Γ P p q _ _ _ ihP ihp ihq =>
    intro ρ hρ
    have hP := ihP ρ hρ
    rw [El_sort] at hP
    rw [eq_empty H hP (ihp ρ hρ), eq_empty H hP (ihq ρ hρ)]
  | @eqFun Γ A B f g l l' hl' _ _ _ _ _ _ ihf ihg =>
    intro ρ hρ
    have hf := ihf ρ hρ
    have hg := ihg ρ hρ
    rw [El_pi H S hl'] at hf hg
    show propCode (interp H S f ρ = interp H S g ρ) = _
    rw [interp_pi_zero]
    apply propCode_congr
    constructor
    · intro e x _
      show holds (propCode (ZFSet.app (interp H S f.shift (cons x ρ)) x =
        ZFSet.app (interp H S g.shift (cons x ρ)) x))
      rw [holds_propCode, interp_shift, interp_shift, e]
    · intro h
      apply ZFSet.funext hf hg
      intro x hx
      have := h x hx
      change holds (propCode (ZFSet.app (interp H S f.shift (cons x ρ)) x =
        ZFSet.app (interp H S g.shift (cons x ρ)) x)) at this
      rwa [holds_propCode, interp_shift, interp_shift] at this
  | @eqPair Γ A B p q l l' h _ _ _ _ _ _ ihp ihq =>
    intro ρ hρ
    have hp := ihp ρ hρ
    have hq := ihq ρ hρ
    rw [El_sigma H S h] at hp hq
    obtain ⟨x, _, y, _, ep⟩ := mem_sigmaSet.1 hp
    obtain ⟨x', _, y', _, eq⟩ := mem_sigmaSet.1 hq
    show propCode (interp H S p ρ = interp H S q ρ) = _
    simp only [Tm.and, interp_and, interp_shift]
    show _ = propCode (holds (propCode (ZFSet.fst (interp H S p ρ) = ZFSet.fst (interp H S q ρ))) ∧
      holds (propCode (ZFSet.snd (interp H S p ρ) = ZFSet.snd (interp H S q ρ))))
    rw [holds_propCode, holds_propCode, ep, eq, fst_pair, fst_pair, snd_pair, snd_pair]
    exact propCode_congr ⟨fun e => pair_inj e, fun ⟨e₁, e₂⟩ => by rw [e₁, e₂]⟩
  | @eqBox Γ P a b _ _ _ ihP iha ihb =>
    intro ρ hρ
    have hP := ihP ρ hρ
    rw [El_sort] at hP
    show propCode _ = propCode True
    rw [eq_empty H hP (iha ρ hρ), eq_empty H hP (ihb ρ hρ)]
    exact propCode_congr ⟨fun _ => trivial, fun _ => rfl⟩
  | @eqOmega Γ P Q _ _ ihP ihQ =>
    intro ρ hρ
    have hP := ihP ρ hρ
    have hQ := ihQ ρ hρ
    rw [El_sort] at hP hQ
    show propCode (interp H S P ρ = interp H S Q ρ) = _
    simp only [Tm.and, Tm.imp, interp_and, interp_shift, interp_pi_zero, holds_propCode]
    apply propCode_congr
    rw [prop_eq_iff H hP hQ]
    exact ⟨fun h => ⟨fun _ hx => h.1 (holds_of H hP hx), fun _ hx => h.2 (holds_of H hQ hx)⟩,
           fun ⟨h₁, h₂⟩ => ⟨fun h => h₁ _ h, fun h => h₂ _ h⟩⟩
  | eqUniv _ _ =>
    intro ρ _
    show propCode _ = propCode True
    exact propCode_congr ⟨fun _ => trivial, fun _ => rfl⟩
  | @eqPi Γ A A' B B' l l' k _ hl' _ _ _ _ _ _ _ _ =>
    intro ρ hρ
    show propCode (interp H S (.pi l l' A B) ρ = interp H S (.pi l l' A' B') ρ) = _
    have inner : ∀ x, holds (interp H S (.eq (l' + 1) (.sort l')
        (B.subst (Tm.under (.cast A'.shift A.shift .irr (.var 0)))) B') (cons x ρ)) ↔
        interp H S B (cons x ρ) = interp H S B' (cons x ρ) := by
      intro x
      show holds (propCode _) ↔ _
      rw [holds_propCode, interp_under]
      rfl
    simp only [Tm.and, interp_and, interp_shift, interp_pi_zero, holds_propCode, inner]
    rw [interp_pi H S hl', interp_pi H S hl']
    apply propCode_congr
    show _ ↔ holds (propCode _) ∧ _
    rw [holds_propCode]
    constructor
    · intro e
      obtain ⟨_, _, hA, hB⟩ := piCode_inj e
      refine ⟨hA.symm, fun x hx => ?_⟩
      have := congrArg (fun g => ZFSet.app g x) hB
      rw [app_lam (hA ▸ hx), app_lam hx] at this
      exact this
    · rintro ⟨hA, hB⟩
      rw [hA] at hB ⊢
      rw [lam_congr hB]
  | @eqSigma Γ A A' B B' l l' k _ h _ _ _ _ _ _ _ _ =>
    intro ρ hρ
    show propCode (interp H S (.sigma l l' A B) ρ = interp H S (.sigma l l' A' B') ρ) = _
    have inner : ∀ x, holds (interp H S (.eq (l' + 1) (.sort l') B
        (B'.subst (Tm.under (.cast A.shift A'.shift .irr (.var 0))))) (cons x ρ)) ↔
        interp H S B (cons x ρ) = interp H S B' (cons x ρ) := by
      intro x
      show holds (propCode _) ↔ _
      rw [holds_propCode, interp_under]
      rfl
    simp only [Tm.and, interp_and, interp_shift, interp_pi_zero, holds_propCode, inner]
    rw [interp_sigma H S h, interp_sigma H S h]
    apply propCode_congr
    show _ ↔ holds (propCode _) ∧ _
    rw [holds_propCode]
    constructor
    · intro e
      obtain ⟨_, _, hA, hB⟩ := sigmaCode_inj e
      refine ⟨hA, fun x hx => ?_⟩
      have := congrArg (fun g => ZFSet.app g x) hB
      rw [app_lam hx, app_lam (hA ▸ hx)] at this
      exact this
    · rintro ⟨hA, hB⟩
      rw [hA] at hB ⊢
      rw [lam_congr hB]
  | @eqUnivNeq Γ X Y k hx hy _ _ _ hX hY hne _ _ =>
    intro ρ _
    show propCode _ = propCode False
    refine propCode_congr ⟨fun e => hne (enc_inj ?_), False.elim⟩
    rw [← hd_interp H S hX ρ, ← hd_interp H S hY ρ, e]
  | @castPi Γ A A' B B' e f l l' m m' k hl' hm' _ _ _ _ _ _ _ _ _ _ ihe ihf =>
    intro ρ hρ
    have he : _ ∈ₛ El (propCode (interp H S (.pi l l' A B) ρ = interp H S (.pi m m' A' B') ρ)) :=
      ihe ρ hρ
    rw [El_propCode] at he
    have e := (mem_ofProp.1 he).1
    rw [interp_pi H S hl', interp_pi H S hm'] at e
    have hA := (piCode_inj e).2.2.1
    have hf := ihf ρ hρ
    rw [El_pi H S hl'] at hf
    show interp H S f ρ = lam (El (interp H S A' ρ)) fun x => ZFSet.app (interp H S f.shift (cons x ρ)) x
    simp only [interp_shift]
    rw [← hA]; exact (lam_app hf).symm
  | @castSigma Γ A A' B B' e p l l' m m' k h _ _ _ _ _ _ _ _ _ _ _ _ ihp =>
    intro ρ hρ
    have hp := ihp ρ hρ
    rw [El_sigma H S h] at hp
    obtain ⟨x, _, y, _, ep⟩ := mem_sigmaSet.1 hp
    show interp H S p ρ = ZFSet.pair (ZFSet.fst (interp H S p ρ)) (ZFSet.snd (interp H S p ρ))
    rw [ep, fst_pair, snd_pair]
  | castUniv _ _ _ _ _ _ => intro ρ _; rfl
  | castRefl _ _ _ _ _ _ => intro ρ _; rfl
  | argsNil _ _ => intro ρ _; trivial
  | @quotForm Γ A R l hl _ _ _ _ _ ihA _ _ _ _ =>
    intro ρ hρ
    have hA := ihA ρ hρ
    rw [El_sort] at hA ⊢
    exact H.quotCode_mem hl hA _
  | @qin Γ A R a l _ _ _ iha =>
    intro ρ hρ
    show cls _ _ _ ∈ₛ El (quotCode _ _ _)
    rw [El_quotCode]
    exact mem_image.2 ⟨_, iha ρ hρ, rfl⟩
  | @qelim Γ A R P f q l m _ _ _ _ _ _ _ _ _ _ _ ihR ihrefl ihsym ihtrans _ ihf _ ihq =>
    intro ρ hρ
    have refl := refl_sem H S (ihrefl ρ hρ)
    have sym := sym_sem H S (ihsym ρ hρ)
    have trans := trans_sem H S (ihtrans ρ hρ)
    have hq : interp H S q ρ ∈ₛ El (quotCode l (interp H S A ρ) (interp H S R ρ)) := ihq ρ hρ
    rw [El_quotCode] at hq
    obtain ⟨x₀, hx₀, e⟩ := mem_image.1 hq
    have hrep : rep (interp H S q ρ) ∈ₛ interp H S q ρ :=
      rep_mem ⟨x₀, e ▸ mem_cls.2 ⟨hx₀, refl x₀ hx₀⟩⟩
    rw [e] at hrep
    obtain ⟨hy, hxy⟩ := mem_cls.1 hrep
    have hf := app_mem_pi H S (ihf ρ hρ) hy
    change _ ∈ₛ El (app (interp H S P.shift (cons _ ρ))
      (cls (interp H S A.shift (cons _ ρ)) (interp H S R.shift (cons _ ρ))
        (rep (cls (interp H S A ρ) (interp H S R ρ) x₀)))) at hf
    rw [interp_shift, interp_shift, interp_shift,
      ← (cls_eq_iff refl sym trans hx₀ hy).2 hxy] at hf
    show app (interp H S f ρ) (rep (interp H S q ρ)) ∈ₛ El (app (interp H S P ρ) (interp H S q ρ))
    rw [e]
    exact hf
  | @quotBeta Γ A R P f a l m _ _ _ _ _ _ _ _ ihrefl _ ihresp iha =>
    intro ρ hρ
    have ha := iha ρ hρ
    have hrep : rep (cls (interp H S A ρ) (interp H S R ρ) (interp H S a ρ)) ∈ₛ
        cls (interp H S A ρ) (interp H S R ρ) (interp H S a ρ) :=
      rep_mem ⟨_, mem_cls.2 ⟨ha, refl_sem H S (ihrefl ρ hρ) _ ha⟩⟩
    obtain ⟨hy, hay⟩ := mem_cls.1 hrep
    show app (interp H S f ρ) (rep _) = app (interp H S f ρ) (interp H S a ρ)
    exact (resp_sem H S (ihresp ρ hρ) _ ha _ hy hay).symm
  | @eqQuotClass Γ A R a b l _ _ _ _ _ _ _ _ ihR ihrefl ihsym ihtrans iha ihb =>
    intro ρ hρ
    have ha := iha ρ hρ
    have hb := ihb ρ hρ
    show propCode (cls _ _ (interp H S a ρ) = cls _ _ (interp H S b ρ)) =
      app (app (interp H S R ρ) (interp H S a ρ)) (interp H S b ρ)
    rw [prop_eq_iff H (propCode_mem_U0 H _) (rel_sem H S (ihR ρ hρ) _ ha _ hb), holds_propCode]
    exact cls_eq_iff (refl_sem H S (ihrefl ρ hρ)) (sym_sem H S (ihsym ρ hρ))
      (trans_sem H S (ihtrans ρ hρ)) ha hb
  | @eqQuot Γ A R A' R' l _ _ _ _ _ ihR _ ihR' =>
    intro ρ hρ
    have hr := rel_sem H S (ihR ρ hρ)
    have hr' := rel_sem H S (ihR' ρ hρ)
    have inner : ∀ x y, holds (interp H S (.eq 1 (.sort 0) (Quot.rel 2 R 1 0)
        (.app (.app (Quot.shiftN 2 R') (.cast (Quot.shiftN 2 A) (Quot.shiftN 2 A') .irr (.var 1)))
          (.cast (Quot.shiftN 2 A) (Quot.shiftN 2 A') .irr (.var 0)))) (cons y (cons x ρ))) ↔
        app (app (interp H S R ρ) x) y = app (app (interp H S R' ρ) x) y := by
      intro x y
      show holds (propCode (app (app (interp H S (Quot.shiftN 2 R) (cons y (cons x ρ))) x) y =
        app (app (interp H S (Quot.shiftN 2 R') (cons y (cons x ρ))) x) y)) ↔ _
      rw [holds_propCode, interp_shiftN, interp_shiftN, interp_shiftN, interp_shiftN]
      rfl
    show propCode (quotCode l _ _ = quotCode l _ _) = _
    simp only [Tm.and, interp_and, interp_shift, interp_pi_zero, holds_propCode, inner]
    apply propCode_congr
    show _ ↔ holds (propCode _) ∧ _
    rw [holds_propCode]
    constructor
    · intro e
      obtain ⟨-, hA, hrel⟩ := quotCode_inj e
      refine ⟨hA, fun x hx y hy => ?_⟩
      rw [prop_eq_iff H (hr x hx y hy) (hr' x (hA ▸ hx) y (hA ▸ hy))]
      have h1 : pair x y ∈ₛ relSet (interp H S A ρ) (interp H S R ρ) ↔
          pair x y ∈ₛ relSet (interp H S A' ρ) (interp H S R' ρ) := by rw [hrel]
      rw [mem_relSet, mem_relSet, ← hA] at h1
      exact ⟨fun h => (h1.1 ⟨hx, hy, h⟩).2.2, fun h => (h1.2 ⟨hx, hy, h⟩).2.2⟩
    · rintro ⟨hA, hrel⟩
      have hpt : ∀ x, x ∈ₛ El (interp H S A ρ) → ∀ y, y ∈ₛ El (interp H S A ρ) →
          app (app (interp H S R ρ) x) y = app (app (interp H S R' ρ) x) y :=
        fun x hx y hy => hrel x hx y hy
      rw [← hA]
      have hrs : relSet (interp H S A ρ) (interp H S R ρ) = relSet (interp H S A ρ) (interp H S R' ρ) :=
        ext fun z => by
          rw [relSet, relSet, mem_sep, mem_sep]
          constructor
          · rintro ⟨hz, h⟩
            obtain ⟨x, hx, y, hy, rfl⟩ := mem_prod.1 hz
            rw [fst_pair, snd_pair] at h ⊢
            exact ⟨hz, hpt x hx y hy ▸ h⟩
          · rintro ⟨hz, h⟩
            obtain ⟨x, hx, y, hy, rfl⟩ := mem_prod.1 hz
            rw [fst_pair, snd_pair] at h ⊢
            exact ⟨hz, (hpt x hx y hy).symm ▸ h⟩
      have hcl : ∀ x, x ∈ₛ El (interp H S A ρ) →
          cls (interp H S A ρ) (interp H S R ρ) x = cls (interp H S A ρ) (interp H S R' ρ) x :=
        fun x hx => ext fun z => by
          rw [mem_cls, mem_cls]
          exact ⟨fun ⟨hz, h⟩ => ⟨hz, hpt x hx z hz ▸ h⟩, fun ⟨hz, h⟩ => ⟨hz, (hpt x hx z hz).symm ▸ h⟩⟩
      unfold quotCode
      rw [hrs]
      congr 1
      apply ext
      intro z
      rw [mem_image, mem_image]
      exact ⟨fun ⟨x, hx, e⟩ => ⟨x, hx, e.trans (hcl x hx)⟩,
        fun ⟨x, hx, e⟩ => ⟨x, hx, e.trans (hcl x hx).symm⟩⟩
  | @castQuot Γ A R A' R' e a l _ _ ihe iha =>
    intro ρ hρ
    have ha := iha ρ hρ
    have he : interp H S e ρ ∈ₛ El (propCode (quotCode l (interp H S A ρ) (interp H S R ρ) =
      quotCode l (interp H S A' ρ) (interp H S R' ρ))) := ihe ρ hρ
    rw [El_propCode] at he
    obtain ⟨-, hA, hrel⟩ := quotCode_inj (mem_ofProp.1 he).1
    show cls _ _ (interp H S a ρ) = cls _ _ (interp H S a ρ)
    apply ext
    intro z
    rw [mem_cls, mem_cls]
    have h1 : pair (interp H S a ρ) z ∈ₛ relSet (interp H S A ρ) (interp H S R ρ) ↔
        pair (interp H S a ρ) z ∈ₛ relSet (interp H S A' ρ) (interp H S R' ρ) := by rw [hrel]
    rw [mem_relSet, mem_relSet] at h1
    constructor
    · rintro ⟨hz, h⟩; exact ⟨(h1.1 ⟨ha, hz, h⟩).2.1, (h1.1 ⟨ha, hz, h⟩).2.2⟩
    · rintro ⟨hz, h⟩
      have := h1.2 ⟨hA ▸ ha, hz, h⟩
      exact ⟨this.2.1, this.2.2⟩
  | quotCong _ _ ihA ihR =>
    intro ρ hρ
    show quotCode _ _ _ = quotCode _ _ _
    rw [ihA ρ hρ, ihR ρ hρ]
  | qinCong _ _ _ ihA ihR iha =>
    intro ρ hρ
    show cls _ _ _ = cls _ _ _
    rw [ihA ρ hρ, ihR ρ hρ, iha ρ hρ]
  | qelimCong _ _ ihf ihq =>
    intro ρ hρ
    show app _ (rep _) = app _ (rep _)
    rw [ihf ρ hρ, ihq ρ hρ]
  | @argsCons Γ pre t ts A Δ l _ _ _ ihA iht ihs =>
    intro ρ hρ
    have hA := ihA ρ hρ
    have ht := iht ρ hρ
    rw [interp_substL] at hA ht
    rw [El_sort] at hA
    have hs := ihs ρ hρ
    rw [List.map_append] at hs
    exact ⟨hA, ht, hs⟩
  | @dataForm Γ as k d hk _ ihas =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, hglob, hbelow⟩ := declAt H hG hk
    have has := ihas ρ hρ
    simp only [List.map_nil] at has
    rw [interp_apps, El_sort]
    show apps (semSig H sig (.data k)) _ ∈ₛ _
    rw [apps_data hglob has.top (by rw [has.length])]
    have ha := (ArgsOk.split H _ d.uniform has).1
    exact dataCode_mem DW ((params_congr hbelow DW).1 ha) _
  | @ctor Γ ps fs k c d C hk hc _ _ ihps ihfs =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, hglob, hbelow⟩ := declAt H hG hk
    have hC := List.mem_of_getElem? hc
    have hps := ihps ρ hρ
    simp only [List.map_nil] at hps
    have hfs := ihfs ρ hρ
    obtain ⟨hF, hR⟩ := ctorFacts hglob hbelow DW hC hps hfs
    have hpl : (ps.map (interp H (semSig H sig) · ρ)).length = d.uniform := by
      rw [hps.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
    have hfl : (fs.map (interp H (semSig H sig) · ρ)).length = C.fields.length := by
      rw [hfs.length, Ctor.tele, fieldsTele_length]
    rw [interp_apps, interp_apps, List.map_append, List.map_append]
    show apps (semSig H sig (.ctor k c)) _ ∈ₛ El (apps (semSig H sig (.data k)) _)
    have htop : ∀ v ∈ ps.map (interp H (semSig H sig) · ρ) ++ fs.map (interp H (semSig H sig) · ρ),
        v ∈ₛ top H := fun v hv => by
      rcases List.mem_append.1 hv with hv | hv
      · exact hps.top v hv
      · exact hfs.top v hv
    rw [apps_ctor hglob hc htop hpl hfl]
    have hres : (C.result.map (Tm.substL (ps ++ fs))).map (interp H (semSig H sig) · ρ) =
        (sCtor H (semSig H (sig.take k)) C).result
          ((fs.map (interp H (semSig H sig) · ρ)).reverse ++ (ps.map (interp H (semSig H sig) · ρ)).reverse) := by
      simp only [List.map_map, sCtor]
      apply List.map_congr_left
      intro y hy
      simp only [Function.comp, interp_substL, List.map_append, List.reverse_append]
      exact interp_decl hbelow ((DW.inScope.2 C hC).1 y hy) _
    rw [hres]
    have htop' : ∀ v ∈ ps.map (interp H (semSig H sig) · ρ) ++ (sCtor H (semSig H (sig.take k)) C).result
        ((fs.map (interp H (semSig H sig) · ρ)).reverse ++ (ps.map (interp H (semSig H sig) · ρ)).reverse),
        v ∈ₛ top H := fun v hv => by
      rcases List.mem_append.1 hv with hv | hv
      · exact hps.top v hv
      · exact hR.top v hv
    have hlen : (ps.map (interp H (semSig H sig) · ρ) ++ (sCtor H (semSig H (sig.take k)) C).result
        ((fs.map (interp H (semSig H sig) · ρ)).reverse ++
          (ps.map (interp H (semSig H sig) · ρ)).reverse)).length = d.params.length := by
      rw [List.length_append, hpl, hR.length, Decl.indices, List.length_drop]
      have := DW.uniform_le
      omega
    rw [apps_data hglob htop' hlen, Family.El_dataCode, List.take_left' hpl, List.drop_left' hpl]
    refine mem_famSet.2 (Member.intro ?_ hF)
    simp [semCtors, hc]
  | @elim Γ ps P ms xs x k m d hk hpl hml _ ihA =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, hglob, hbelow⟩ := declAt H hG hk
    have hA := ihA ρ hρ
    simp only [List.map_nil, List.map_append, List.map_cons] at hA
    rw [interp_apps, interp_apps]
    simp only [List.map_append, List.map_cons, List.map_nil]
    exact elim_mem H _ hglob hbelow DW m (by simpa using hpl) (by simpa using hml) hA
  | @iota Γ ps P ms xs fs k m c d C hk hc hpl hml _ _ ihfs ihA =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, hglob, hbelow⟩ := declAt H hG hk
    have hC := List.mem_of_getElem? hc
    have hA := ihA ρ hρ
    simp only [List.map_nil, List.map_append, List.map_cons] at hA
    have hfs := ihfs ρ hρ
    simp only [Decl.elimTele] at hA
    have hT1 : (d.params.take d.uniform).length = d.uniform := by
      rw [List.length_take]; exact Nat.min_eq_left DW.uniform_le
    have hpl' : (ps.map (interp H (semSig H sig) · ρ)).length = d.uniform := by simpa using hpl
    have hml' : (ms.map (interp H (semSig H sig) · ρ)).length = d.ctors.length := by simpa using hml
    have hxl : (xs.map (interp H (semSig H sig) · ρ)).length = d.indices.length := by
      have := hA.length
      simp only [List.length_append, List.length_singleton, hT1, List.length_mapIdx, hpl', hml'] at this
      omega
    have hA1 : ArgsOk H (semSig H sig) [] (ps.map (interp H (semSig H sig) · ρ))
        (d.params.take d.uniform) := by
      have := (ArgsOk.split H _ d.uniform hA).1
      simp only [List.append_assoc] at this
      rwa [List.take_left' hpl', List.take_left' hT1] at this
    obtain ⟨hF, -⟩ := ctorFacts hglob hbelow DW hC hA1 hfs
    have hA1' := (params_congr hbelow DW).1 hA1
    have w := walk DW hC (P := []) (done := []) (R := Member (semCtors H (semSig H (sig.take k)) d) _)
      (Rest := []) (DW.fields C hC) (by simp) rfl (by simpa [zeroRec_nil] using envOk_params hA1') hpl'
      (by simpa [sCtor] using hF)
    have hfl : (fs.map (interp H (semSig H sig) · ρ)).length = C.fields.length := by
      rw [hfs.length, Ctor.tele, fieldsTele_length]
    have htopc : ∀ v ∈ ps.map (interp H (semSig H sig) · ρ) ++ fs.map (interp H (semSig H sig) · ρ),
        v ∈ₛ top H := fun v hv => by
      rcases List.mem_append.1 hv with hv | hv
      · exact hA1.top v hv
      · exact hfs.top v hv
    have hcv : interp H (semSig H sig) (Tm.apps (.glob (.ctor k c)) (ps ++ fs)) ρ =
        val c (fs.map (interp H (semSig H sig) · ρ)) := by
      rw [interp_apps, List.map_append]
      exact apps_ctor hglob hc htopc hpl' hfl
    rw [interp_apps]
    simp only [List.map_append, List.map_cons, List.map_nil] at hA ⊢
    rw [hcv] at hA ⊢
    show apps (semSig H sig (.elim k m)) _ = interp H (semSig H sig) (Tm.apps (ms.getD c .irr) _) ρ
    rw [apps_elim H _ hglob m hA.top hpl' hml' hxl, recV_val (by simp [semCtors, hc]) hF,
      interp_apps, List.map_append, interp_getD]
    congr 2
    have := interp_ihArgs H _ hglob hbelow DW hC m (ρ := ρ) (P := P) (ms := ms) hpl hml (by simpa using hfl)
      (fun v hv => by
        simp only [List.map_append, List.map_cons, List.map_nil, List.mem_append, List.mem_singleton] at hv
        rcases hv with ((hv | hv) | hv) | hv
        · exact hA1.top v hv
        · exact hA.top v (by simp [hv])
        · exact hA.top v (by simp [hv])
        · exact hfs.top v hv)
      (fun i ys hi => by simpa using w.2 i ys hi) (Fs := C.fields) (Q := []) rfl
    simp only [List.length_nil, List.drop_zero] at this
    exact this.symm
  | eqUnivData _ _ _ =>
    intro ρ _
    show propCode _ = propCode True
    exact propCode_congr ⟨fun _ => trivial, fun _ => rfl⟩
  | @eqDataNeq Γ as as' k k' l d d' hk hk' hne _ _ ihas ihas' =>
    intro ρ hρ
    subst hS
    show propCode (interp H (semSig H sig) (Tm.apps _ as) ρ = interp H (semSig H sig) (Tm.apps _ as') ρ) = propCode False
    have ha := ihas ρ hρ
    have ha' := ihas' ρ hρ
    simp only [List.map_nil] at ha ha'
    rw [interp_data H hG hk ha, interp_data H hG hk' ha']
    exact propCode_congr ⟨fun e => hne (nat_inj (pair_inj (pair_inj (pair_inj e).1).2).1), False.elim⟩
  | @eqDataHead Γ as Y h k l d hk _ hY ihas =>
    intro ρ hρ
    subst hS
    show propCode (interp H (semSig H sig) (Tm.apps _ as) ρ = interp H (semSig H sig) Y ρ) = propCode False
    have ha := ihas ρ hρ
    simp only [List.map_nil] at ha
    rw [interp_data H hG hk ha]
    refine propCode_congr ⟨fun e => ?_, False.elim⟩
    have ht := congrArg tag e
    rw [Family.tag_dataCode] at ht
    have hy : tag (interp H (semSig H sig) Y ρ) = fst (enc h) := by
      rw [← hd_interp H (semSig H sig) hY ρ, hd, fst_pair]
    rw [hy] at ht
    cases h <;> simp only [enc, fst_pair] at ht <;> exact absurd (nat_inj ht) (by omega)
  | @eqHeadData Γ as Y h k l d hk _ hY ihas =>
    intro ρ hρ
    subst hS
    show propCode (interp H (semSig H sig) Y ρ = interp H (semSig H sig) (Tm.apps _ as) ρ) = propCode False
    have ha := ihas ρ hρ
    simp only [List.map_nil] at ha
    rw [interp_data H hG hk ha]
    refine propCode_congr ⟨fun e => ?_, False.elim⟩
    have ht := congrArg tag e.symm
    rw [Family.tag_dataCode] at ht
    have hy : tag (interp H (semSig H sig) Y ρ) = fst (enc h) := by
      rw [← hd_interp H (semSig H sig) hY ρ, hd, fst_pair]
    rw [hy] at ht
    cases h <;> simp only [enc, fst_pair] at ht <;> exact absurd (nat_inj ht) (by omega)
  | @eqCtorNeq Γ ps fs ps' fs' as k c c' d C C' hk hne hc hc' _ _ _ _ ihps ihfs ihps' ihfs' =>
    intro ρ hρ
    subst hS
    show propCode (interp H (semSig H sig) (Tm.apps _ (ps ++ fs)) ρ = interp H (semSig H sig) (Tm.apps _ (ps' ++ fs')) ρ) =
      propCode False
    have hps := ihps ρ hρ
    have hps' := ihps' ρ hρ
    simp only [List.map_nil] at hps hps'
    rw [interp_ctorApp H hG hk hc hps (ihfs ρ hρ), interp_ctorApp H hG hk hc' hps' (ihfs' ρ hρ)]
    exact propCode_congr ⟨fun e => hne (val_inj e).1, False.elim⟩
  | @castCtor Γ ps fs as as' e k c d C hk hc _ _ _ _ ihps ihfs ihas' _ =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, -, -⟩ := declAt H hG hk
    have hps := ihps ρ hρ
    have ha' := ihas' ρ hρ
    have hfs := ihfs ρ hρ
    simp only [List.map_nil] at hps ha'
    show interp H (semSig H sig) (Tm.apps _ (ps ++ fs)) ρ = interp H (semSig H sig) (Tm.apps _ (as'.take d.uniform ++ _)) ρ
    have hfl : fs.length = C.fields.length := by
      have := hfs.length; rw [List.length_map, Ctor.tele, fieldsTele_length] at this; exact this
    have ecf := interp_castFields H (semSig H sig) k d ps (as'.take d.uniform) fs ρ C.fields 0 []
      (by omega)
    simp only [List.map_nil, List.nil_append, List.drop_zero, ← hfl, List.take_length] at ecf
    obtain ⟨_, hglob, _⟩ := declAt H hG hk
    rw [interp_ctorApp H hG hk hc hps hfs, interp_apps, List.map_append, ecf]
    have ha'u := (ArgsOk.split H _ d.uniform ha').1
    refine (apps_ctor hglob hc (fun v hv => ?_) ?_ (by simpa using hfl)).symm
    · rcases List.mem_append.1 hv with hv | hv
      · rw [List.map_take] at hv; exact ha'u.top v hv
      · exact hfs.top v hv
    · rw [List.map_take, ha'u.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
  | @eqCtor Γ ps fs ps' fs' as k c d C hk hc _ _ _ _ ihps ihfs ihps' ihfs' =>
    intro ρ hρ
    subst hS
    show propCode (interp H (semSig H sig) (Tm.apps _ (ps ++ fs)) ρ =
      interp H (semSig H sig) (Tm.apps _ (ps' ++ fs')) ρ) = _
    have hps := ihps ρ hρ
    have hps' := ihps' ρ hρ
    have hfs := ihfs ρ hρ
    have hfs' := ihfs' ρ hρ
    simp only [List.map_nil] at hps hps'
    rw [interp_ctorApp H hG hk hc hps hfs, interp_ctorApp H hG hk hc hps' hfs']
    have E := eqFields_iff H (semSig H sig) k d ps fs fs' ρ C.fields 0
    rw [interp_andAll H _ ρ _ E.1]
    refine propCode_congr ⟨fun e => E.2.2 fun i hi _ => ?_, fun h => ?_⟩
    · have e' := congrArg (·[i]?) (val_inj e).2
      simp only [List.getElem?_map] at e'
      rw [List.getD_eq_getElem?_getD, List.getD_eq_getElem?_getD, Nat.zero_add]
      revert e'
      cases fs[i]? <;> cases fs'[i]? <;> intro e' <;> simp at e' <;> first | rfl | exact e'
    · have hfl : fs.length = C.fields.length := by
        have := hfs.length; rw [List.length_map, Ctor.tele, fieldsTele_length] at this; exact this
      have hfl' : fs'.length = C.fields.length := by
        have := hfs'.length; rw [List.length_map, Ctor.tele, fieldsTele_length] at this; exact this
      have h' := E.2.1 h
      congr 1
      apply List.ext_getElem (by simp [hfl, hfl'])
      intro i hi hi'
      simp only [List.length_map] at hi hi'
      by_cases hl : C.fields[i].level d.level = 0
      · rw [ArgsOk.zero H _ hfs i (by omega) (by simpa using hi) hl,
          ArgsOk.zero H _ hfs' i (by omega) (by simpa using hi') hl]
      · have := h' i (by omega) hl
        simp only [Nat.zero_add, List.getD_eq_getElem?_getD, List.getElem?_eq_getElem hi,
          List.getElem?_eq_getElem hi', Option.getD_some] at this
        simpa using this
  | @dataProj Γ as as' e bs k c j d C F hk hc hF _ _ _ _ _ ihas ihas' ihe ihbs ihcbs =>
    intro ρ hρ
    subst hS
    obtain ⟨DW, hglob, hbelow⟩ := declAt H hG hk
    have hC := List.mem_of_getElem? hc
    have hj : j < C.fields.length := (List.getElem?_eq_some_iff.1 hF).1
    have ha := ihas ρ hρ
    have ha' := ihas' ρ hρ
    simp only [List.map_nil] at ha ha'
    have he := ihe ρ hρ
    have hbs := ihbs ρ hρ
    have hcbs := ihcbs ρ hρ
    have hau := (ArgsOk.split H _ d.uniform ha).1
    have hau' := (ArgsOk.split H _ d.uniform ha').1
    rw [List.map_take] at hbs hcbs
    have hbl : bs.length = j := by
      have := hbs.length
      rw [List.length_map, List.length_take, Ctor.tele, fieldsTele_length] at this; omega
    have ecf := interp_castFields H (semSig H sig) k d (as.take d.uniform) (as'.take d.uniform) bs ρ
      (C.fields.take j) 0 [] (by simp; omega)
    have hbt : (bs.drop 0).take (C.fields.take j).length = bs := by
      simp only [List.drop_zero, List.length_take]; rw [List.take_of_length_le (by omega)]
    rw [hbt] at ecf
    simp only [List.map_nil, List.nil_append] at ecf
    rw [ecf] at hcbs
    have heq : interp H (semSig H sig) (Tm.apps (.glob (.data k)) as) ρ =
        interp H (semSig H sig) (Tm.apps (.glob (.data k)) as') ρ := by
      change _ ∈ₛ El (propCode _) at he
      rw [El_propCode] at he
      exact (mem_ofProp.1 he).1
    rw [interp_data H hG hk ha, interp_data H hG hk ha'] at heq
    have hlabel := dataCode_label heq
    obtain ⟨hpre, hysa⟩ := prefixFacts hglob hbelow DW hC hj hau hbs
    obtain ⟨-, hysa'⟩ := prefixFacts hglob hbelow DW hC hj hau' hcbs
    have hc' : (semCtors H (semSig H (sig.take k)) d)[c]? = some (sCtor H (semSig H (sig.take k)) C) := by
      simp [semCtors, hc]
    have hfc := fieldCode_eq hlabel hc' (by simpa [sCtor] using hj) (by simpa [sCtor] using hpre)
    simp only [sCtor, List.getElem?_map, hF, Option.map_some] at hfc
    show empty ∈ₛ El (propCode _)
    rw [El_propCode, mem_ofProp]
    refine ⟨?_, rfl⟩
    rw [interp_substL, interp_substL, List.map_append, List.map_append, ecf, List.map_take,
      List.map_take, List.reverse_append, List.reverse_append]
    have hal : ((as.map (interp H (semSig H sig) · ρ)).take d.uniform).length = d.uniform := by
      rw [hau.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
    have hal' : ((as'.map (interp H (semSig H sig) · ρ)).take d.uniform).length = d.uniform := by
      rw [hau'.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
    cases F with
    | plain A l =>
      have hA : A.globsBelow k := (DW.inScope.2 C hC).2 (.plain A l) (List.mem_of_getElem? hF)
      simp only [Field.ty]
      rw [interp_decl hbelow hA, interp_decl hbelow hA]
      simpa only [fieldCode, sField] using hfc
    | recur ys =>
      have hys : ∀ y ∈ ys, y.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (List.mem_of_getElem? hF)
      simp only [Field.ty]
      rw [interp_apps, interp_apps, List.map_append, List.map_append,
        paramVars_interp (H := H) (S := semSig H sig) (by simp [hbl]) hal,
        paramVars_interp (H := H) (S := semSig H sig) (by simp [hbl]) hal',
        List.map_congr_left fun y hy => interp_decl hbelow (hys y hy) _,
        List.map_congr_left fun y hy => interp_decl hbelow (hys y hy) _]
      have fa := hysa ys hF
      have fa' := hysa' ys hF
      show apps (semSig H sig (.data k)) _ = apps (semSig H sig (.data k)) _
      have htop : ∀ (a ys' : List ZFSet.{u}), ArgsOk H (semSig H sig) [] a (d.params.take d.uniform) →
          ArgsOk H (semSig H (sig.take k)) a ys' d.indices → ∀ v ∈ a ++ ys', v ∈ₛ top H :=
        fun a ys' h1 h2 v hv => by
          rcases List.mem_append.1 hv with hv | hv
          · exact h1.top v hv
          · exact h2.top v hv
      have hlen : ∀ (a ys' : List ZFSet.{u}), a.length = d.uniform →
          ArgsOk H (semSig H (sig.take k)) a ys' d.indices → (a ++ ys').length = d.params.length :=
        fun a ys' h1 h2 => by
          rw [List.length_append, h1, h2.length, Decl.indices, List.length_drop]
          have := DW.uniform_le; omega
      rw [apps_data hglob (htop _ _ hau fa) (hlen _ _ hal fa),
        apps_data hglob (htop _ _ hau' fa') (hlen _ _ hal' fa'),
        List.take_left' hal, List.drop_left' hal, List.take_left' hal', List.drop_left' hal']
      simp only [fieldCode, sField] at hfc
      unfold dataCode
      rw [hlabel, hfc]

/-! ## Consistency -/

/-- A well-formed signature has a model: each declaration's premises hold
with the meanings of the ones before it. -/
theorem good_of_sigOk {sig : Sig} (h : SigOk N sig) : Good H sig := by
  induction h with
  | nil => intro k d hk; simp at hk
  | @snoc sig d _ hd ih =>
    intro k d' hk
    by_cases hlt : k < sig.length
    · rw [List.getElem?_append_left hlt] at hk
      rw [List.take_append_of_le_length (Nat.le_of_lt hlt)]
      exact ih k d' hk
    · have hk' : k = sig.length := by
        have := (List.getElem?_eq_some_iff.1 hk).1
        simp at this; omega
      subst hk'
      rw [List.getElem?_append_right (Nat.le_refl _)] at hk
      simp only [Nat.sub_self, List.getElem?_cons_zero, Option.some.injEq] at hk
      subst hk
      rw [List.take_append_of_le_length (Nat.le_refl _), List.take_length]
      exact hd.map fun j hj => sound H _ rfl ih hj

include H in
/-- No closed term has type `@Bottom`, in any well-formed signature, for any
bound `N` on the universes that has a hierarchy. -/
theorem consistent {sig : Sig} (hsig : SigOk N sig) (t : Tm) : ¬ Deriv N sig (.ty [] t .bot) := fun d => by
  have h : _ ∈ₛ El (propCode False) := sound H _ rfl (good_of_sigOk H hsig) d (fun _ => empty) trivial
  rw [El_propCode] at h
  exact (mem_ofProp.1 h).1

end Model
