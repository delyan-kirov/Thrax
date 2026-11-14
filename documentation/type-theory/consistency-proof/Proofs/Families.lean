import Proofs.Universes

/-!
# Inductive families in sets

The set-theoretic meaning of one inductive declaration, independent of the
syntax (#496). A declaration is read through its uniform parameters `a`: each
constructor is a list of fields, each field either a type computed from `a`
and the fields before it, or a recursive occurrence of the family at indices
computed the same way (the kernel's restricted positivity: the family occurs
only as a whole field, at its own uniform parameters). A constructor's result
states its indices.

* The family is the least fixed point, built in stages: `MemberN n` holds the
  values of depth below `n`. A value has finitely many recursive fields, so
  `ω` stages reach every value.
* Its elements at indices `xs` form a set because they all lie in a bound,
  `Vω`, built from the field types alone. When those types lie in a
  Grothendieck universe, so do the bound and the family.
* The **label** of the family at `a` records each field's type as a function
  of the fields before it; at a recursive field, the family's elements at its
  indices. This is PLT25's label (Section 6.2) without its stream: the uniform
  parameters never change in a recursive occurrence, so one level suffices.
  Equal labels give the projections of Data-Eq-Projection, and the label
  never stores the parameters themselves, which may be too large for the
  universe (Note [Equality of two instances of one inductive type is stuck]).
* The recursor is defined by stages, reading a value's constructor and fields
  back from its encoding.

Environments are lists, the newest value first, as de Bruijn indices count.
-/

universe u

namespace ZFSet

/-! ## Tuples and finite unions -/

def tuple : List ZFSet.{u} → ZFSet.{u}
  | [] => empty
  | x :: xs => pair x (tuple xs)

theorem tuple_inj : {xs ys : List ZFSet.{u}} → tuple xs = tuple ys → xs = ys
  | [], [], _ => rfl
  | [], _ :: _, h => absurd h.symm pair_ne_empty
  | _ :: _, [], h => absurd h pair_ne_empty
  | _ :: _, _ :: _, h => by
    let ⟨e₁, e₂⟩ := pair_inj h
    rw [e₁, tuple_inj e₂]

noncomputable def untuple : Nat → ZFSet.{u} → List ZFSet.{u}
  | 0, _ => []
  | n + 1, t => fst t :: untuple n (snd t)

theorem untuple_tuple : (xs : List ZFSet.{u}) → untuple xs.length (tuple xs) = xs
  | [] => rfl
  | x :: xs => by
    simp only [List.length_cons, untuple, tuple, fst_pair, snd_pair, untuple_tuple xs]

theorem IsUniverse.tuple_mem {G : ZFSet.{u}} (hG : IsUniverse G) :
    (xs : List ZFSet.{u}) → (∀ x ∈ xs, x ∈ₛ G) → tuple xs ∈ₛ G
  | [], _ => hG.empty_mem
  | x :: xs, h => hG.pair_mem (h x List.mem_cons_self)
      (hG.tuple_mem xs fun y hy => h y (List.mem_cons_of_mem _ hy))

noncomputable def union (a b : ZFSet.{u}) : ZFSet.{u} := sUnion (upair a b)

theorem mem_union {a b z : ZFSet.{u}} : z ∈ₛ union a b ↔ z ∈ₛ a ∨ z ∈ₛ b := by
  rw [union, mem_sUnion]
  constructor
  · rintro ⟨y, hy, hz⟩
    rcases mem_upair.1 hy with e | e <;> subst e
    · exact Or.inl hz
    · exact Or.inr hz
  · rintro (h | h)
    · exact ⟨a, mem_upair.2 (Or.inl rfl), h⟩
    · exact ⟨b, mem_upair.2 (Or.inr rfl), h⟩

noncomputable def unions : List ZFSet.{u} → ZFSet.{u}
  | [] => empty
  | x :: xs => union x (unions xs)

theorem mem_unions {z : ZFSet.{u}} : {xs : List ZFSet.{u}} → (z ∈ₛ unions xs ↔ ∃ x ∈ xs, z ∈ₛ x)
  | [] => ⟨fun h => absurd h (not_mem_empty _), fun ⟨_, h, _⟩ => absurd h List.not_mem_nil⟩
  | x :: xs => by
    rw [unions, mem_union, mem_unions]
    constructor
    · rintro (h | ⟨y, hy, h⟩)
      · exact ⟨x, List.mem_cons_self, h⟩
      · exact ⟨y, List.mem_cons_of_mem _ hy, h⟩
    · rintro ⟨y, hy, h⟩
      rcases List.mem_cons.1 hy with e | hy
      · exact Or.inl (e ▸ h)
      · exact Or.inr ⟨y, hy, h⟩

theorem IsUniverse.unions_mem {G : ZFSet.{u}} (hG : IsUniverse G) :
    (xs : List ZFSet.{u}) → (∀ x ∈ xs, x ∈ₛ G) → unions xs ∈ₛ G
  | [], _ => hG.empty_mem
  | x :: xs, h => hG.sUnion_mem (hG.upair_mem _ _ (h x List.mem_cons_self)
      (hG.unions_mem xs fun y hy => h y (List.mem_cons_of_mem _ hy)))

/-- Equal graphs agree on the first one's domain. -/
theorem lam_eq_app {D D' : ZFSet.{u}} {F F' : ZFSet.{u} → ZFSet.{u}} (h : lam D F = lam D' F')
    {t : ZFSet.{u}} (ht : t ∈ₛ D) : F t = F' t := by
  have : pair t (F t) ∈ₛ lam D' F' := h ▸ mem_lam.2 ⟨t, ht, rfl⟩
  let ⟨_, _, e⟩ := mem_lam.1 this
  let ⟨e₁, e₂⟩ := pair_inj e
  subst e₁
  exact e₂

/-! ## Curried functions -/

/-- `f` applied to each argument in turn. -/
noncomputable def apps (f : ZFSet.{u}) (xs : List ZFSet.{u}) : ZFSet.{u} := xs.foldl app f

theorem apps_nil (f : ZFSet.{u}) : apps f [] = f := rfl

theorem apps_cons (f x : ZFSet.{u}) (xs : List ZFSet.{u}) : apps f (x :: xs) = apps (app f x) xs :=
  rfl

theorem apps_append (f : ZFSet.{u}) (xs ys : List ZFSet.{u}) :
    apps f (xs ++ ys) = apps (apps f xs) ys := List.foldl_append

/-- A function of `n` arguments, each from `D`, curried. -/
noncomputable def curry (D : ZFSet.{u}) : Nat → (List ZFSet.{u} → ZFSet.{u}) → ZFSet.{u}
  | 0, F => F []
  | n + 1, F => lam D fun x => curry D n fun xs => F (x :: xs)

theorem apps_curry (D : ZFSet.{u}) : (xs : List ZFSet.{u}) → (∀ x ∈ xs, x ∈ₛ D) →
    (F : List ZFSet.{u} → ZFSet.{u}) → apps (curry D xs.length F) xs = F xs
  | [], _, _ => rfl
  | x :: xs, h, F => by
    show apps (app (lam D _) x) xs = _
    rw [app_lam (h x List.mem_cons_self)]
    exact apps_curry D xs (fun y hy => h y (List.mem_cons_of_mem _ hy)) _

end ZFSet

namespace Family

open ZFSet

/-! ## Declarations -/

/-- A field of a constructor, given the environment of the uniform parameters
and the fields before it. -/
inductive SField where
  /-- A field of the type this code stands for, of sort `l`. -/
  | plain (A : List ZFSet.{u} → ZFSet.{u}) (l : Nat)
  /-- The family itself, at these indices. -/
  | recur (ys : List ZFSet.{u} → List ZFSet.{u})

structure SCtor where
  fields : List SField.{u}
  /-- The indices of the constructor's result, given all its fields. -/
  result : List ZFSet.{u} → List ZFSet.{u}

variable (cs : List SCtor.{u}) (a : List ZFSet.{u})

/-- `fs` fit the fields `Fs`, after the fields `pre` (newest first), with `R`
saying which values a recursive field may take at which indices. -/
def FieldsOk (R : List ZFSet.{u} → ZFSet.{u} → Prop) :
    List SField.{u} → List ZFSet.{u} → List ZFSet.{u} → Prop
  | [], _, [] => True
  | .plain A _ :: Fs, pre, x :: xs => x ∈ₛ El (A (pre ++ a.reverse)) ∧ FieldsOk R Fs (x :: pre) xs
  | .recur ys :: Fs, pre, x :: xs => R (ys (pre ++ a.reverse)) x ∧ FieldsOk R Fs (x :: pre) xs
  | _, _, _ => False

variable {a}

theorem FieldsOk.mono {R R' : List ZFSet.{u} → ZFSet.{u} → Prop} (h : ∀ ys x, R ys x → R' ys x) :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} → FieldsOk a R Fs pre fs → FieldsOk a R' Fs pre fs
  | [], _, [], _ => trivial
  | .plain _ _ :: _, _, _ :: _, ⟨hx, hs⟩ => ⟨hx, FieldsOk.mono h hs⟩
  | .recur _ :: _, _, _ :: _, ⟨hx, hs⟩ => ⟨h _ _ hx, FieldsOk.mono h hs⟩

theorem FieldsOk.length {R : List ZFSet.{u} → ZFSet.{u} → Prop} :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} → FieldsOk a R Fs pre fs → fs.length = Fs.length
  | [], _, [], _ => rfl
  | .plain _ _ :: _, _, _ :: _, ⟨_, hs⟩ => congrArg (· + 1) (FieldsOk.length hs)
  | .recur _ :: _, _, _ :: _, ⟨_, hs⟩ => congrArg (· + 1) (FieldsOk.length hs)

/-- A stage-indexed predicate, increasing, holding of each recursive field
at some stage, holds of them all at one stage. -/
theorem FieldsOk.bound {Q : Nat → List ZFSet.{u} → ZFSet.{u} → Prop}
    (hQ : ∀ n ys x, Q n ys x → Q (n + 1) ys x) :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} →
    FieldsOk a (fun ys x => ∃ n, Q n ys x) Fs pre fs → ∃ n, FieldsOk a (Q n) Fs pre fs
  | [], _, [], _ => ⟨0, trivial⟩
  | .plain _ _ :: _, _, _ :: _, ⟨hx, hs⟩ =>
    let ⟨n, hs⟩ := FieldsOk.bound hQ hs
    ⟨n, hx, hs⟩
  | .recur _ :: _, _, _ :: _, ⟨⟨m, hx⟩, hs⟩ => by
    let ⟨n, hs⟩ := FieldsOk.bound hQ hs
    have up : ∀ k i ys x, Q i ys x → Q (i + k) ys x := by
      intro k; induction k with
      | zero => intro _ _ _ h; exact h
      | succ k ih => intro i ys x h; exact hQ _ _ _ (ih i ys x h)
    refine ⟨max m n, ?_, hs.mono fun ys x h => ?_⟩
    · have := up (max m n - m) m _ _ hx; rwa [Nat.add_sub_cancel' (Nat.le_max_left m n)] at this
    · have := up (max m n - n) n _ _ h; rwa [Nat.add_sub_cancel' (Nat.le_max_right m n)] at this

/-! ## The family -/

/-- A value: its constructor's number and its fields. -/
def val (c : Nat) (fs : List ZFSet.{u}) : ZFSet.{u} := pair (nat c) (tuple fs)

theorem val_inj {c c' : Nat} {fs fs' : List ZFSet.{u}} (h : val c fs = val c' fs') :
    c = c' ∧ fs = fs' :=
  let ⟨e₁, e₂⟩ := pair_inj h
  ⟨nat_inj e₁, tuple_inj e₂⟩

variable (a)

/-- The values of depth below `n`, at their indices. -/
def MemberN : Nat → List ZFSet.{u} → ZFSet.{u} → Prop
  | 0, _, _ => False
  | n + 1, xs, v => ∃ c C fs, cs[c]? = some C ∧ FieldsOk a (MemberN n) C.fields [] fs ∧
      xs = C.result (fs.reverse ++ a.reverse) ∧ v = val c fs

/-- The values of the family at indices `xs`. -/
def Member (xs : List ZFSet.{u}) (v : ZFSet.{u}) : Prop := ∃ n, MemberN cs a n xs v

variable {cs a}

theorem MemberN.succ : {n : Nat} → {xs : List ZFSet.{u}} → {v : ZFSet.{u}} →
    MemberN cs a n xs v → MemberN cs a (n + 1) xs v
  | _ + 1, _, _, ⟨c, C, fs, hc, hf, hx, hv⟩ => ⟨c, C, fs, hc, hf.mono fun _ _ h => h.succ, hx, hv⟩

theorem Member.intro {c : Nat} {C : SCtor.{u}} {fs : List ZFSet.{u}} (hc : cs[c]? = some C)
    (hf : FieldsOk a (Member cs a) C.fields [] fs) :
    Member cs a (C.result (fs.reverse ++ a.reverse)) (val c fs) :=
  let ⟨n, hf⟩ := FieldsOk.bound (fun _ _ _ h => MemberN.succ h) hf
  ⟨n + 1, c, C, fs, hc, hf, rfl, rfl⟩

theorem Member.cases {xs : List ZFSet.{u}} {v : ZFSet.{u}} (h : Member cs a xs v) :
    ∃ c C fs, cs[c]? = some C ∧ FieldsOk a (Member cs a) C.fields [] fs ∧
      xs = C.result (fs.reverse ++ a.reverse) ∧ v = val c fs := by
  obtain ⟨n, h⟩ := h
  cases n with
  | zero => exact h.elim
  | succ n =>
    let ⟨c, C, fs, hc, hf, hx, hv⟩ := h
    exact ⟨c, C, fs, hc, hf.mono fun _ _ h => ⟨n, h⟩, hx, hv⟩

/-- Induction on the family. -/
theorem Member.induction {Q : List ZFSet.{u} → ZFSet.{u} → Prop}
    (step : ∀ c C fs, cs[c]? = some C →
      FieldsOk a (fun ys x => Member cs a ys x ∧ Q ys x) C.fields [] fs →
      Q (C.result (fs.reverse ++ a.reverse)) (val c fs))
    {xs : List ZFSet.{u}} {v : ZFSet.{u}} (h : Member cs a xs v) : Q xs v := by
  obtain ⟨n, h⟩ := h
  induction n generalizing xs v with
  | zero => exact h.elim
  | succ n ih =>
    let ⟨c, C, fs, hc, hf, hx, hv⟩ := h
    subst hx hv
    exact step c C fs hc (hf.mono fun _ _ h => ⟨⟨n, h⟩, ih h⟩)

/-! ## A bound -/

variable (a)

/-- Where a field's values lie: its type, or `X` for a recursive field. -/
noncomputable def dom (X : ZFSet.{u}) (pre : List ZFSet.{u}) : SField.{u} → ZFSet.{u}
  | .plain A _ => El (A (pre ++ a.reverse))
  | .recur _ => X

/-- The tuples of fields `Fs` after `pre`, recursive ones from `X`. -/
noncomputable def tuples (X : ZFSet.{u}) : List SField.{u} → List ZFSet.{u} → ZFSet.{u}
  | [], _ => singleton empty
  | F :: Fs, pre =>
    sUnion (image (fun x => image (fun t => pair x t) (tuples X Fs (x :: pre))) (dom a X pre F))

variable {a}

theorem mem_tuples {X : ZFSet.{u}} : {Fs : List SField.{u}} → {pre : List ZFSet.{u}} → {t : ZFSet.{u}} →
    (t ∈ₛ tuples a X Fs pre ↔ ∃ fs, t = tuple fs ∧ FieldsOk a (fun _ x => x ∈ₛ X) Fs pre fs)
  | [], pre, t => by
    rw [tuples, mem_singleton]
    exact ⟨fun h => ⟨[], h, trivial⟩, fun ⟨fs, e, h⟩ => by
      cases fs with
      | nil => exact e
      | cons _ _ => exact h.elim⟩
  | F :: Fs, pre, t => by
    rw [tuples, mem_sUnion]
    constructor
    · rintro ⟨s, hs, ht⟩
      let ⟨x, hx, e⟩ := mem_image.1 hs
      subst e
      let ⟨t', ht', e⟩ := mem_image.1 ht
      subst e
      let ⟨fs, e, hfs⟩ := mem_tuples.1 ht'
      subst e
      refine ⟨x :: fs, rfl, ?_⟩
      cases F with
      | plain A l => exact ⟨hx, hfs⟩
      | recur ys => exact ⟨hx, hfs⟩
    · rintro ⟨fs, rfl, h⟩
      cases fs with
      | nil => cases F <;> exact h.elim
      | cons x fs =>
        have hx : x ∈ₛ dom a X pre F ∧ FieldsOk a (fun _ x => x ∈ₛ X) Fs (x :: pre) fs := by
          cases F with
          | plain A l => exact h
          | recur ys => exact h
        exact ⟨_, mem_image.2 ⟨x, hx.1, rfl⟩, mem_image.2 ⟨tuple fs, mem_tuples.2 ⟨fs, rfl, hx.2⟩, rfl⟩⟩

/-- The fields of constructor `c`. -/
def fieldsOf (cs : List SCtor.{u}) (c : Nat) : List SField.{u} := (cs[c]?.map SCtor.fields).getD []

variable (cs a)

/-- Values of depth below `n`, at any indices. -/
noncomputable def V : Nat → ZFSet.{u}
  | 0 => empty
  | n + 1 => unions ((List.range cs.length).map fun c =>
      image (fun t => pair (nat c) t) (tuples a (V n) (fieldsOf cs c) []))

noncomputable def Vω : ZFSet.{u} := sUnion (range (V cs a))

/-- The family at indices `xs`. -/
noncomputable def famSet (xs : List ZFSet.{u}) : ZFSet.{u} := sep (Member cs a xs) (Vω cs a)

variable {cs a}

theorem memberN_mem_V : {n : Nat} → {xs : List ZFSet.{u}} → {v : ZFSet.{u}} →
    MemberN cs a n xs v → v ∈ₛ V cs a n
  | n + 1, _, _, ⟨c, C, fs, hc, hf, _, hv⟩ => by
    subst hv
    have hlt : c < cs.length := by
      rcases Nat.lt_or_ge c cs.length with h | h
      · exact h
      · rw [List.getElem?_eq_none h] at hc; cases hc
    rw [V, mem_unions]
    refine ⟨_, List.mem_map.2 ⟨c, List.mem_range.2 hlt, rfl⟩, mem_image.2 ⟨tuple fs, ?_, rfl⟩⟩
    have : fieldsOf cs c = C.fields := by simp [fieldsOf, hc]
    rw [this]
    exact mem_tuples.2 ⟨fs, rfl, hf.mono fun _ _ h => memberN_mem_V h⟩

theorem member_mem_Vω {xs : List ZFSet.{u}} {v : ZFSet.{u}} (h : Member cs a xs v) :
    v ∈ₛ Vω cs a :=
  let ⟨n, h⟩ := h
  mem_sUnion.2 ⟨_, mem_range.2 ⟨n, rfl⟩, memberN_mem_V h⟩

theorem mem_famSet {xs : List ZFSet.{u}} {v : ZFSet.{u}} : v ∈ₛ famSet cs a xs ↔ Member cs a xs v := by
  rw [famSet, mem_sep]
  exact ⟨fun h => h.2, fun h => ⟨member_mem_Vω h, h⟩⟩

/-! ## In a universe -/

variable (G : ZFSet.{u}) (a)

/-- Every plain field's type is in `G`, after any earlier fields that fit
their types (with anything for the recursive ones). -/
def Small : List SField.{u} → List ZFSet.{u} → Prop
  | [], _ => True
  | .plain A _ :: Fs, pre => A (pre ++ a.reverse) ∈ₛ G ∧
      ∀ x, x ∈ₛ El (A (pre ++ a.reverse)) → Small Fs (x :: pre)
  | .recur _ :: Fs, pre => ∀ x, Small Fs (x :: pre)

variable {G a}

theorem tuples_mem {G : ZFSet.{u}} (hG : IsUniverse G) {X : ZFSet.{u}} (hX : X ∈ₛ G) :
    {Fs : List SField.{u}} → {pre : List ZFSet.{u}} → Small a G Fs pre → tuples a X Fs pre ∈ₛ G
  | [], _, _ => hG.singleton_mem hG.empty_mem
  | .plain A _ :: Fs, pre, ⟨hA, hs⟩ => by
    have hEl := hG.snd_mem hA
    exact hG.sUnion_mem (hG.image_mem hEl fun x hx =>
      hG.image_mem (tuples_mem hG hX (hs x hx)) fun t ht =>
        hG.pair_mem (hG.trans _ _ hEl hx) (hG.trans _ _ (tuples_mem hG hX (hs x hx)) ht))
  | .recur _ :: Fs, pre, hs =>
    hG.sUnion_mem (hG.image_mem hX fun x hx =>
      hG.image_mem (tuples_mem hG hX (hs x)) fun t ht =>
        hG.pair_mem (hG.trans _ _ hX hx) (hG.trans _ _ (tuples_mem hG hX (hs x)) ht))

/-- The plain fields along a fitting prefix stay small. -/
theorem Small.after {G X : ZFSet.{u}} :
    {Fs Gs : List SField.{u}} → {pre fs : List ZFSet.{u}} → Small a G (Fs ++ Gs) pre →
    FieldsOk a (fun _ x => x ∈ₛ X) Fs pre fs → Small a G Gs (fs.reverse ++ pre)
  | [], _, _, [], h, _ => h
  | .plain _ _ :: _, _, _, x :: _, ⟨_, h⟩, ⟨hx, hs⟩ => by
    have := Small.after (h x hx) hs
    simpa using this
  | .recur _ :: _, _, _, x :: _, h, ⟨_, hs⟩ => by
    have := Small.after (h x) hs
    simpa using this

theorem V_mem (hG : IsUniverse G) (hs : ∀ (c : Nat) (C : SCtor.{u}), cs[c]? = some C → Small a G C.fields []) :
    (n : Nat) → V cs a n ∈ₛ G
  | 0 => hG.empty_mem
  | n + 1 => by
    refine hG.unions_mem _ fun s hs' => ?_
    let ⟨c, hc, e⟩ := List.mem_map.1 hs'
    subst e
    have hc' : c < cs.length := List.mem_range.1 hc
    have hsm : Small a G (fieldsOf cs c) [] := by
      unfold fieldsOf
      rw [List.getElem?_eq_getElem hc']
      exact hs c _ (List.getElem?_eq_getElem hc')
    have ht := tuples_mem hG (V_mem hG hs n) hsm
    exact hG.image_mem ht fun t htm => hG.pair_mem (hG.nat_mem c) (hG.trans _ _ ht htm)

theorem Vω_mem (hG : IsUniverse G) (hs : ∀ (c : Nat) (C : SCtor.{u}), cs[c]? = some C → Small a G C.fields []) :
    Vω cs a ∈ₛ G :=
  hG.iUnion_mem (V_mem hG hs)

theorem famSet_mem (hG : IsUniverse G) (hs : ∀ (c : Nat) (C : SCtor.{u}), cs[c]? = some C → Small a G C.fields [])
    (xs : List ZFSet.{u}) : famSet cs a xs ∈ₛ G :=
  hG.sep_mem (Vω_mem hG hs)

/-! ## The label -/

variable (cs a)

/-- What the label records for a field, in an environment. -/
noncomputable def fieldCode (env : List ZFSet.{u}) : Option SField.{u} → ZFSet.{u}
  | some (.plain A _) => A env
  | some (.recur ys) => famSet cs a (ys env)
  | none => empty

/-- Field `j` of `C` as a function of the fields before it. -/
noncomputable def fieldLabel (C : SCtor.{u}) (j : Nat) : ZFSet.{u} :=
  lam (tuples a (Vω cs a) (C.fields.take j) [])
    fun t => fieldCode cs a ((untuple j t).reverse ++ a.reverse) C.fields[j]?

noncomputable def ctorLabel (C : SCtor.{u}) : ZFSet.{u} :=
  tuple ((List.range C.fields.length).map (fieldLabel cs a C))

noncomputable def label : ZFSet.{u} := tuple (cs.map (ctorLabel cs a))

/-- The code of the family at `a` and `xs`, for declaration number `k`. -/
noncomputable def dataCode (k : Nat) (xs : List ZFSet.{u}) : ZFSet.{u} :=
  pair (pair (nat 4) (pair (nat k) (label cs a))) (famSet cs a xs)

variable {cs a}

theorem El_dataCode (k : Nat) (xs : List ZFSet.{u}) : El (dataCode cs a k xs) = famSet cs a xs :=
  El_code _ _

theorem tag_dataCode (k : Nat) (xs : List ZFSet.{u}) : tag (dataCode cs a k xs) = nat 4 := by
  simp only [tag, dataCode, fst_pair]

theorem dataCode_label {k k' : Nat} {a' xs xs' : List ZFSet.{u}}
    (h : dataCode cs a k xs = dataCode cs a' k' xs') : label cs a = label cs a' :=
  (pair_inj (pair_inj (pair_inj h).1).2).2

/-- Equal labels give equal field codes, at every fitting prefix. -/
theorem fieldCode_eq {a' : List ZFSet.{u}} (h : label cs a = label cs a') {c : Nat} {C : SCtor.{u}}
    (hc : cs[c]? = some C) {j : Nat} (hj : j < C.fields.length) {pre : List ZFSet.{u}}
    (hpre : FieldsOk a (fun _ x => x ∈ₛ Vω cs a) (C.fields.take j) [] pre) :
    fieldCode cs a (pre.reverse ++ a.reverse) C.fields[j]? =
      fieldCode cs a' (pre.reverse ++ a'.reverse) C.fields[j]? := by
  have hl := tuple_inj h
  have hc' : c < cs.length := by
    rcases Nat.lt_or_ge c cs.length with h | h
    · exact h
    · rw [List.getElem?_eq_none h] at hc; cases hc
  have eC : cs[c] = C := by rw [List.getElem?_eq_getElem hc'] at hc; exact Option.some.inj hc
  have e1 : ctorLabel cs a C = ctorLabel cs a' C := by
    have := congrArg (fun l => l[c]?) hl
    simp only [List.getElem?_map, List.getElem?_eq_getElem hc', eC, Option.map_some,
      Option.some.injEq] at this
    exact this
  have e2 : fieldLabel cs a C j = fieldLabel cs a' C j := by
    have := congrArg (fun l => l[j]?) (tuple_inj e1)
    simp only [List.getElem?_map, List.getElem?_range hj, Option.map_some, Option.some.injEq] at this
    exact this
  have ht : tuple pre ∈ₛ tuples a (Vω cs a) (C.fields.take j) [] := mem_tuples.2 ⟨pre, rfl, hpre⟩
  have hlen : pre.length = j := by
    rw [hpre.length, List.length_take]; omega
  unfold fieldLabel at e2
  have := lam_eq_app e2 ht
  have e3 : untuple j (tuple pre) = pre := hlen ▸ untuple_tuple pre
  rw [e3] at this
  exact this

theorem Small.take {G : ZFSet.{u}} : {Fs : List SField.{u}} → {pre : List ZFSet.{u}} → {j : Nat} →
    Small a G Fs pre → Small a G (Fs.take j) pre
  | [], _, _, _ => by simp [Small]
  | _ :: _, _, 0, _ => by simp [Small]
  | .plain _ _ :: _, _, _ + 1, h => ⟨h.1, fun x hx => Small.take (h.2 x hx)⟩
  | .recur _ :: _, _, _ + 1, h => fun x => Small.take (h x)

theorem label_mem (hG : IsUniverse G) (hs : ∀ (c : Nat) (C : SCtor.{u}), cs[c]? = some C → Small a G C.fields []) :
    label cs a ∈ₛ G := by
  refine hG.tuple_mem _ fun l hl => ?_
  let ⟨C, hC, e⟩ := List.mem_map.1 hl
  subst e
  let ⟨c, hc⟩ := List.getElem?_of_mem hC
  refine hG.tuple_mem _ fun f hf => ?_
  let ⟨j, hj, e⟩ := List.mem_map.1 hf
  subst e
  have hj : j < C.fields.length := List.mem_range.1 hj
  have hVω := Vω_mem hG hs
  have hsm : Small a G (C.fields.take j ++ C.fields.drop j) [] := by
    rw [List.take_append_drop]; exact hs c C hc
  refine hG.lam_mem (tuples_mem hG hVω (Small.take (hs c C hc))) fun t ht => ?_
  let ⟨pre, e, hpre⟩ := mem_tuples.1 ht
  subst e
  have hlen : pre.length = j := by rw [hpre.length, List.length_take]; omega
  have e3 : untuple j (tuple pre) = pre := hlen ▸ untuple_tuple pre
  rw [e3]
  have hrest := Small.after hsm hpre
  rw [List.getElem?_eq_getElem hj]
  rw [List.drop_eq_getElem_cons hj] at hrest
  simp only [List.append_nil] at hrest
  cases hF : C.fields[j] with
  | plain A l =>
    rw [hF] at hrest
    exact hrest.1
  | recur ys => exact famSet_mem hG hs _

/-! ## The recursor -/

/-- The recursor's results for the recursive fields, in order: the induction
hypotheses a method takes after the fields. -/
def ihs (f : ZFSet.{u} → ZFSet.{u}) : List SField.{u} → List ZFSet.{u} → List ZFSet.{u}
  | .plain _ _ :: Fs, _ :: xs => ihs f Fs xs
  | .recur _ :: Fs, x :: xs => f x :: ihs f Fs xs
  | _, _ => []

theorem ihs_congr {R : List ZFSet.{u} → ZFSet.{u} → Prop} {f g : ZFSet.{u} → ZFSet.{u}}
    (h : ∀ ys x, R ys x → f x = g x) :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} → FieldsOk a R Fs pre fs →
    ihs f Fs fs = ihs g Fs fs
  | [], _, [], _ => rfl
  | .plain _ _ :: _, _, _ :: _, ⟨_, hs⟩ => by simp only [ihs]; exact ihs_congr h hs
  | .recur _ :: _, _, _ :: _, ⟨hx, hs⟩ => by
    simp only [ihs, h _ _ hx, ihs_congr h hs]

variable (cs)

/-- The recursor at depth below `n`, with methods `ms`, reading the
constructor and fields back from the value. -/
noncomputable def recN (ms : List ZFSet.{u}) : Nat → ZFSet.{u} → ZFSet.{u}
  | 0, _ => empty
  | n + 1, v =>
    match cs[natOf (fst v)]? with
    | some C =>
      let fs := untuple C.fields.length (snd v)
      apps (ms[natOf (fst v)]?.getD empty) (fs ++ ihs (recN ms n) C.fields fs)
    | none => empty

variable {cs}

theorem recN_val {ms : List ZFSet.{u}} {n c : Nat} {C : SCtor.{u}} {fs : List ZFSet.{u}}
    (hc : cs[c]? = some C) (hl : fs.length = C.fields.length) :
    recN cs ms (n + 1) (val c fs) = apps (ms[c]?.getD empty) (fs ++ ihs (recN cs ms n) C.fields fs) := by
  simp only [recN, val, fst_pair, snd_pair, natOf_nat, hc]
  rw [← hl, untuple_tuple]

theorem recN_stable {ms : List ZFSet.{u}} : {n : Nat} → {xs : List ZFSet.{u}} → {v : ZFSet.{u}} →
    MemberN cs a n xs v → ∀ m, n ≤ m → recN cs ms m v = recN cs ms n v
  | n + 1, _, _, ⟨c, C, fs, hc, hf, _, hv⟩, m, hm => by
    subst hv
    obtain ⟨m, rfl⟩ : ∃ m', m = m' + 1 := ⟨m - 1, by omega⟩
    rw [recN_val hc hf.length, recN_val hc hf.length,
      ihs_congr (f := recN cs ms m) (g := recN cs ms n) (fun _ _ h => recN_stable h m (by omega)) hf]

variable (cs a)

/-- The recursor with methods `ms`. -/
noncomputable def recV (ms : List ZFSet.{u}) (v : ZFSet.{u}) : ZFSet.{u} :=
  open Classical in
  if h : ∃ n xs, MemberN cs a n xs v then recN cs ms (Classical.choose h) v else empty

variable {cs a}

theorem recV_eq {ms : List ZFSet.{u}} {n : Nat} {xs : List ZFSet.{u}} {v : ZFSet.{u}}
    (h : MemberN cs a n xs v) : recV cs a ms v = recN cs ms n v := by
  unfold recV
  split
  next h' =>
    let ⟨xs', h'n⟩ := Classical.choose_spec h'
    rw [← recN_stable h'n (max (Classical.choose h') n) (Nat.le_max_left _ _),
      recN_stable h (max (Classical.choose h') n) (Nat.le_max_right _ _)]
  next h' => exact absurd ⟨n, xs, h⟩ h'

/-- Data-Iota, in sets. -/
theorem recV_val {ms : List ZFSet.{u}} {c : Nat} {C : SCtor.{u}} {fs : List ZFSet.{u}}
    (hc : cs[c]? = some C) (hf : FieldsOk a (Member cs a) C.fields [] fs) :
    recV cs a ms (val c fs) = apps (ms[c]?.getD empty) (fs ++ ihs (recV cs a ms) C.fields fs) := by
  let ⟨n, hn⟩ := FieldsOk.bound (fun _ _ _ h => MemberN.succ h) hf
  have hv : MemberN cs a (n + 1) _ (val c fs) := ⟨c, C, fs, hc, hn, rfl, rfl⟩
  rw [recV_eq hv, recN_val hc hn.length,
    ihs_congr (f := recN cs ms n) (g := recV cs a ms) (fun _ _ h => (recV_eq h).symm) hn]

variable (a)

/-- `rs` are induction hypotheses for the fields `fs`: each in the motive `P`
at its field's indices and value. -/
def IhsOk (P : ZFSet.{u}) : List SField.{u} → List ZFSet.{u} → List ZFSet.{u} → List ZFSet.{u} → Prop
  | [], _, [], [] => True
  | .plain _ _ :: Fs, pre, x :: xs, rs => IhsOk P Fs (x :: pre) xs rs
  | .recur ys :: Fs, pre, x :: xs, r :: rs =>
    r ∈ₛ El (apps P (ys (pre ++ a.reverse) ++ [x])) ∧ IhsOk P Fs (x :: pre) xs rs
  | _, _, _, _ => False

variable {a}

theorem ihsOk_of {P : ZFSet.{u}} {f : ZFSet.{u} → ZFSet.{u}} :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} →
    FieldsOk a (fun ys x => f x ∈ₛ El (apps P (ys ++ [x]))) Fs pre fs → IhsOk a P Fs pre fs (ihs f Fs fs)
  | [], _, [], _ => trivial
  | .plain _ _ :: _, _, _ :: _, ⟨_, hs⟩ => by simp only [IhsOk, ihs]; exact ihsOk_of hs
  | .recur _ :: _, _, _ :: _, ⟨hx, hs⟩ => ⟨hx, ihsOk_of hs⟩

/-- Data-Elim, in sets: methods that take fitting fields and induction
hypotheses into the motive make the recursor land in the motive. -/
theorem recV_mem {ms : List ZFSet.{u}} {P : ZFSet.{u}}
    (hm : ∀ (c : Nat) (C : SCtor.{u}) (fs : List ZFSet.{u}), cs[c]? = some C →
      FieldsOk a (Member cs a) C.fields [] fs → ∀ rs, IhsOk a P C.fields [] fs rs →
      apps (ms[c]?.getD empty) (fs ++ rs) ∈ₛ
        El (apps P (C.result (fs.reverse ++ a.reverse) ++ [val c fs])))
    {xs : List ZFSet.{u}} {v : ZFSet.{u}} (h : Member cs a xs v) :
    recV cs a ms v ∈ₛ El (apps P (xs ++ [v])) := by
  refine Member.induction (Q := fun xs v => recV cs a ms v ∈ₛ El (apps P (xs ++ [v])))
    (fun c C fs hc hf => ?_) h
  rw [recV_val hc (hf.mono fun _ _ h => h.1)]
  exact hm c C fs hc (hf.mono fun _ _ h => h.1) _ (ihsOk_of (hf.mono fun _ _ h => h.2))

end Family
