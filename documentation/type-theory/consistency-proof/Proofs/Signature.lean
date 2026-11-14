import Proofs.Model

/-!
# The signature in the model

What each global means (#496). Declaration `k` is interpreted with the
meanings of the declarations before it (`semSig`), which is well founded
because its terms name only earlier globals (`Decl.scoped`).

* Its type former, applied to parameters `a` and indices `xs`, is the code
  `Family.dataCode` of the family of `Proofs/Families.lean`.
* A constructor, applied to the parameters and fields `fs`, is the value
  `Family.val c fs`: the parameters are not stored, as Cast-Ctor needs.
* The recursor is `Family.recV`.

Each is a curried function whose arguments come from `top`, the largest
universe, which holds every value of every type.
-/

universe u

namespace Model

open ZFSet Family

variable {N : Nat} (H : Hierarchy.{u} N)

/-- The largest universe. -/
def top : ZFSet.{u} := H.G (N - 1)

theorem mem_top {c x : ZFSet.{u}} {l : Nat} (hc : c ∈ₛ H.U l) (hx : x ∈ₛ El c) : x ∈ₛ top H := by
  have hN : N - 1 < N := by have := H.pos; omega
  unfold Hierarchy.U at hc
  split at hc
  · rw [Hierarchy.El_of_mem_props hc hx]; exact (H.isUniverse _ hN).empty_mem
  · have hi := H.indexLt l
    have hG := H.isUniverse _ hi
    exact H.mono (by unfold Hierarchy.index; omega) hN (hG.trans _ _ (hG.snd_mem hc) hx)

variable (S : Glob → ZFSet.{u})

/-! ## Declarations in sets -/

noncomputable def sField : Field → SField.{u}
  | .plain A l => .plain (fun env => interp H S A (envOf env)) l
  | .recur ys => .recur fun env => ys.map (interp H S · (envOf env))

noncomputable def sCtor (C : Ctor) : SCtor.{u} :=
  ⟨C.fields.map (sField H S), fun env => C.result.map (interp H S · (envOf env))⟩

noncomputable def semCtors (d : Decl) : List SCtor.{u} := d.ctors.map (sCtor H S)

/-- The globals of declaration `k`, given the meanings `S` of the earlier ones. -/
noncomputable def meaning (k : Nat) (d : Decl) : Glob → ZFSet.{u}
  | .data _ => curry (top H) d.params.length fun as =>
      dataCode (semCtors H S d) (as.take d.uniform) k (as.drop d.uniform)
  | .ctor _ c => curry (top H) (d.uniform + ((d.ctors[c]?.map fun C => C.fields.length).getD 0))
      fun as => val c (as.drop d.uniform)
  | .elim _ _ => curry (top H) (d.uniform + 1 + d.ctors.length + d.indices.length + 1) fun as =>
      recV (semCtors H S d) (as.take d.uniform) ((as.drop (d.uniform + 1)).take d.ctors.length)
        (as.getLastD empty)

/-- Every global of the signature. -/
noncomputable def semSig (sig : Sig) (g : Glob) : ZFSet.{u} :=
  if h : g.decl < sig.length then meaning H (semSig (sig.take g.decl)) g.decl sig[g.decl] g
  else empty
termination_by sig.length
decreasing_by simp only [List.length_take]; omega

theorem semSig_at {sig : Sig} {g : Glob} {d : Decl} (h : sig[g.decl]? = some d) :
    semSig H sig g = meaning H (semSig H (sig.take g.decl)) g.decl d g := by
  have hlt : g.decl < sig.length := (List.getElem?_eq_some_iff.1 h).1
  have hd : sig[g.decl] = d := (List.getElem?_eq_some_iff.1 h).2
  rw [semSig]; simp only [hlt, ↓reduceDIte, hd]

theorem semSig_append {sig : Sig} (more : Sig) {g : Glob} (hg : g.decl < sig.length) :
    semSig H (sig ++ more) g = semSig H sig g := by
  have hd : (sig ++ more)[g.decl]? = some sig[g.decl] := by
    rw [List.getElem?_append_left hg, List.getElem?_eq_getElem hg]
  rw [semSig_at H hd, semSig_at H (List.getElem?_eq_getElem hg),
    List.take_append_of_le_length (Nat.le_of_lt hg)]

/-- A declaration's earlier globals mean the same in the whole signature. -/
theorem semSig_take {sig : Sig} {k : Nat} (hk : k ≤ sig.length) {g : Glob} (hg : g.decl < k) :
    semSig H sig g = semSig H (sig.take k) g := by
  conv => lhs; rw [← List.take_append_drop k sig]
  exact semSig_append H _ (by simp only [List.length_take]; omega)

/-! ## What judgments mean -/

/-- Values `vs` fit the telescope `Δ` after the values `pre`; each type is
in its universe. -/
def ArgsOk (pre : List ZFSet.{u}) : List ZFSet.{u} → List (Tm × Nat) → Prop
  | [], [] => True
  | v :: vs, (A, l) :: Δ => interp H S A (envOf pre.reverse) ∈ₛ H.U l ∧
      v ∈ₛ El (interp H S A (envOf pre.reverse)) ∧ ArgsOk (pre ++ [v]) vs Δ
  | _, _ => False

/-- What a judgment means in the model. -/
def Valid : Jdg → Prop
  | .wf _ => True
  | .ty Γ t A => ∀ ρ, EnvOk H S Γ ρ → interp H S t ρ ∈ₛ El (interp H S A ρ)
  | .conv Γ t u _ => ∀ ρ, EnvOk H S Γ ρ → interp H S t ρ = interp H S u ρ
  | .args Γ pre ts Δ => ∀ ρ, EnvOk H S Γ ρ →
      ArgsOk H S (pre.map (interp H S · ρ)) (ts.map (interp H S · ρ)) Δ

/-- Every declaration's premises hold, with the meanings of the ones before. -/
def Good (sig : Sig) : Prop :=
  ∀ k d, sig[k]? = some d → DeclWith (Valid H (semSig H (sig.take k))) k d

/-! ## Telescopes of values -/

variable {H S}

theorem ArgsOk.length : {pre vs : List ZFSet.{u}} → {Δ : List (Tm × Nat)} →
    ArgsOk H S pre vs Δ → vs.length = Δ.length
  | _, [], [], _ => rfl
  | _, _ :: _, _ :: _, ⟨_, _, h⟩ => congrArg (· + 1) h.length

theorem ArgsOk.top : {pre vs : List ZFSet.{u}} → {Δ : List (Tm × Nat)} →
    ArgsOk H S pre vs Δ → ∀ v ∈ vs, v ∈ₛ top H
  | _, [], [], _ => fun _ h => absurd h List.not_mem_nil
  | _, _ :: _, _ :: _, ⟨hA, hv, h⟩ => fun w hw => by
    rcases List.mem_cons.1 hw with e | hw
    · exact e ▸ mem_top H hA hv
    · exact h.top w hw

theorem ArgsOk.append : {pre vs ws : List ZFSet.{u}} → {Δ Δ' : List (Tm × Nat)} →
    vs.length = Δ.length → (ArgsOk H S pre (vs ++ ws) (Δ ++ Δ') ↔
      ArgsOk H S pre vs Δ ∧ ArgsOk H S (pre ++ vs) ws Δ')
  | _, [], _, [], _, _ => by simp [ArgsOk]
  | pre, v :: vs, ws, (A, l) :: Δ, Δ', h => by
    simp only [List.cons_append, ArgsOk]
    rw [ArgsOk.append (by simpa using h)]
    simp only [List.append_assoc, List.singleton_append, and_assoc]

/-- Telescope entries that mean the same under two signatures. -/
theorem ArgsOk.congr {S' : Glob → ZFSet.{u}} :
    {pre vs : List ZFSet.{u}} → {Δ : List (Tm × Nat)} →
    (∀ p ∈ Δ, ∀ ρ, interp H S p.1 ρ = interp H S' p.1 ρ) →
    (ArgsOk H S pre vs Δ ↔ ArgsOk H S' pre vs Δ)
  | _, [], [], _ => Iff.rfl
  | _, [], _ :: _, _ => Iff.rfl
  | _, _ :: _, [], _ => Iff.rfl
  | _, _ :: _, (A, l) :: Δ, h => by
    simp only [ArgsOk, h (A, l) List.mem_cons_self]
    rw [ArgsOk.congr fun p hp => h p (List.mem_cons_of_mem _ hp)]

/-- Values fitting a telescope extend an environment of its context. -/
theorem envOk_of_args : {Γ : List Tm} → {pre vs : List ZFSet.{u}} → {Δ : List (Tm × Nat)} →
    EnvOk H S Γ (envOf pre.reverse) → ArgsOk H S pre vs Δ →
    EnvOk H S ((Δ.map Prod.fst).reverse ++ Γ) (envOf (pre ++ vs).reverse)
  | _, _, [], [], hΓ, _ => by simpa using hΓ
  | Γ, pre, v :: vs, (A, l) :: Δ, hΓ, ⟨_, hv, h⟩ => by
    have hA : EnvOk H S (A :: Γ) (envOf (pre ++ [v]).reverse) := by
      rw [List.reverse_append, List.reverse_singleton, List.singleton_append, ← cons_envOf]
      exact ⟨hΓ, hv⟩
    have := envOk_of_args hA h
    simpa using this

/-! ## Recursive fields are unused -/

/-- The fields with each recursive one's value replaced by `∅`: what the
context of a declaration's premises, which binds a recursive field as
`@Top`, sees. -/
def zeroRec : List Field → List ZFSet.{u} → List ZFSet.{u}
  | .plain _ _ :: Fs, x :: xs => x :: zeroRec Fs xs
  | .recur _ :: Fs, _ :: xs => empty :: zeroRec Fs xs
  | _, xs => xs

theorem zeroRec_length : (Fs : List Field) → (fs : List ZFSet.{u}) → (zeroRec Fs fs).length = fs.length
  | .plain _ _ :: Fs, _ :: xs => congrArg (· + 1) (zeroRec_length Fs xs)
  | .recur _ :: Fs, _ :: xs => congrArg (· + 1) (zeroRec_length Fs xs)
  | [], _ => rfl
  | _ :: _, [] => by cases ‹Field› <;> rfl

theorem zeroRec_plain : {Fs : List Field} → {fs : List ZFSet.{u}} → {n : Nat} → {A : Tm} → {l : Nat} →
    Fs[n]? = some (.plain A l) → (zeroRec Fs fs)[n]? = fs[n]?
  | .plain _ _ :: _, _ :: _, 0, _, _, _ => rfl
  | .plain _ _ :: _, _ :: _, _ + 1, _, _, h => by
    simp only [zeroRec, List.getElem?_cons_succ]; exact zeroRec_plain (by simpa using h)
  | .recur _ :: _, _ :: _, 0, _, _, h => by simp at h
  | .recur _ :: _, _ :: _, _ + 1, _, _, h => by
    simp only [zeroRec, List.getElem?_cons_succ]; exact zeroRec_plain (by simpa using h)
  | [], _, _, _, _, h => by simp at h
  | .plain _ _ :: _, [], _, _, _, _ => rfl
  | .recur _ :: _, [], _, _, _, _ => rfl

theorem zeroRec_recur : {Fs : List Field} → {fs : List ZFSet.{u}} → {n : Nat} → {ys : List Tm} →
    Fs[n]? = some (.recur ys) → n < fs.length → (zeroRec Fs fs)[n]? = some empty
  | .plain _ _ :: _, _ :: _, 0, _, h, _ => by simp at h
  | .plain _ _ :: _, _ :: _, _ + 1, _, h, hn => by
    simp only [zeroRec, List.getElem?_cons_succ]; exact zeroRec_recur (by simpa using h) (by simpa using hn)
  | .recur _ :: _, _ :: _, 0, _, _, _ => rfl
  | .recur _ :: _, _ :: _, _ + 1, _, h, hn => by
    simp only [zeroRec, List.getElem?_cons_succ]; exact zeroRec_recur (by simpa using h) (by simpa using hn)
  | [], _, _, _, h, _ => by simp at h
  | _ :: _, [], _, _, _, hn => absurd hn (Nat.not_lt_zero _)

/-- A term at field position `j` does not see the recursive fields before it. -/
theorem interp_unused {C : Ctor} (hu : C.recursiveUnused) {j : Nat} (hj : j ≤ C.fields.length)
    {t : Tm} (ht : t ∈ C.termsAt j) {fs : List ZFSet.{u}} (hl : fs.length = j) (R : List ZFSet.{u}) :
    interp H S t (envOf (fs.reverse ++ R)) = interp H S t (envOf ((zeroRec C.fields fs).reverse ++ R)) := by
  have hz := zeroRec_length C.fields fs
  apply interp_occurs
  intro i hi
  unfold envOf
  by_cases hij : i < j
  · rw [List.getElem?_append_left (by simp only [List.length_reverse]; omega),
      List.getElem?_append_left (by simp only [List.length_reverse, hz]; omega),
      List.getElem?_reverse (by omega), List.getElem?_reverse (by omega)]
    simp only [hz]
    have hn : fs.length - 1 - i < C.fields.length := by omega
    cases hF : C.fields[fs.length - 1 - i]? with
    | none => simp at hF; omega
    | some F =>
      cases F with
      | plain A l => rw [zeroRec_plain hF]
      | recur ys =>
        have := hu _ ys hF j (by omega) hj t ht
        have e : j - 1 - (fs.length - 1 - i) = i := by omega
        rw [e] at this
        rw [this] at hi
        cases hi
  · rw [List.getElem?_append_right (by simp only [List.length_reverse]; omega),
      List.getElem?_append_right (by simp only [List.length_reverse, hz]; omega)]
    simp only [List.length_reverse, hz]

/-- What `zeroRec` keeps of a field's value. -/
def fieldVal : Field → ZFSet.{u} → ZFSet.{u}
  | .plain _ _, x => x
  | .recur _, _ => empty

theorem zeroRec_nil : (Fs : List Field) → zeroRec Fs ([] : List ZFSet.{u}) = []
  | [] => rfl
  | .plain _ _ :: _ => rfl
  | .recur _ :: _ => rfl

theorem zeroRec_snoc {x : ZFSet.{u}} : {Fs : List Field} → {done : List ZFSet.{u}} → {F : Field} →
    Fs[done.length]? = some F → zeroRec Fs (done ++ [x]) = zeroRec Fs done ++ [fieldVal F x]
  | F' :: Fs, [], F, h => by
    simp only [List.length_nil, List.getElem?_cons_zero, Option.some.injEq] at h
    subst h
    cases F' <;> simp only [List.nil_append, zeroRec, zeroRec_nil, fieldVal]
  | F' :: Fs, y :: done, F, h => by
    simp only [List.length_cons, List.getElem?_cons_succ] at h
    cases F' <;> simp only [List.cons_append, zeroRec, zeroRec_snoc h]
  | [], _, _, h => by simp at h

theorem paramVars_interp {L a : List ZFSet.{u}} (hL : L.length = j) (ha : a.length = u) :
    (paramVars u j).map (interp H S · (envOf (L ++ a.reverse))) = a := by
  apply List.ext_getElem
  · simp [paramVars, ha]
  · intro i h₁ h₂
    simp only [paramVars, List.map_map, List.getElem_map, List.getElem_range, Function.comp]
    show envOf (L ++ a.reverse) (j + (u - 1 - i)) = a[i]
    unfold envOf
    rw [List.getElem?_append_right (by omega), List.getElem?_reverse (by simp at h₁ h₂; omega)]
    simp only [hL, ha]
    have e : u - 1 - (j + (u - 1 - i) - j) = i := by simp at h₁ h₂; omega
    rw [e, List.getElem?_eq_getElem h₂]; rfl

theorem paramVars_interp' {L a R : List ZFSet.{u}} (hL : L.length = j) (ha : a.length = u) :
    (paramVars u j).map (interp H S · (envOf (L ++ a.reverse ++ R))) = a := by
  apply List.ext_getElem
  · simp [paramVars, ha]
  · intro i h₁ h₂
    simp only [paramVars, List.map_map, List.getElem_map, List.getElem_range, Function.comp]
    show envOf (L ++ a.reverse ++ R) (j + (u - 1 - i)) = a[i]
    unfold envOf
    rw [List.append_assoc, List.getElem?_append_right (by omega),
      List.getElem?_append_left (by simp at h₁ h₂ ⊢; omega), List.getElem?_reverse (by simp at h₁ h₂; omega)]
    simp only [hL, ha]
    have e : u - 1 - (j + (u - 1 - i) - j) = i := by simp at h₁ h₂; omega
    rw [e, List.getElem?_eq_getElem h₂]; rfl

theorem envOf_append_lt {X Y : List ZFSet.{u}} {i : Nat} (h : i < X.length) :
    envOf (X ++ Y) i = envOf X i := by
  unfold envOf; rw [List.getElem?_append_left h]

theorem envOf_append_ge {X Y : List ZFSet.{u}} {i : Nat} (h : X.length ≤ i) :
    envOf (X ++ Y) i = envOf Y (i - X.length) := by
  unfold envOf; rw [List.getElem?_append_right h]

theorem envOf_reverse {X : List ZFSet.{u}} {i : Nat} (h : i < X.length) :
    envOf X.reverse i = envOf X (X.length - 1 - i) := by
  unfold envOf; rw [List.getElem?_reverse h]

/-- The renaming that moves the indices of a recursive field (over the
parameters and the fields before number `f`) under the method's binders. -/
theorem envOf_ihRename {R₁ fs M a : List ZFSet.{u}} {f : Nat} (hf : f ≤ fs.length) (n : Nat) :
    envOf (R₁ ++ fs.reverse ++ M ++ a.reverse)
        (Tm.liftAt f M.length n + (R₁.length + fs.length - f)) =
      envOf ((fs.take f).reverse ++ a.reverse) n := by
  simp only [List.append_assoc]
  have hlt : (fs.take f).length = f := by simp; omega
  unfold Tm.liftAt
  split
  next h =>
    have h1 : R₁.length ≤ n + (R₁.length + fs.length - f) := by omega
    have h2 : n + (R₁.length + fs.length - f) - R₁.length < fs.reverse.length := by simp; omega
    have h3 : n < (fs.take f).reverse.length := by simp; omega
    conv => lhs; rw [envOf_append_ge h1, envOf_append_lt h2, envOf_reverse (by simpa using h2)]
    conv => rhs; rw [envOf_append_lt h3, envOf_reverse (by simpa using h3)]
    unfold envOf
    rw [List.getElem?_take]
    split
    · congr 2; omega
    · next h' => exact (h' (by omega)).elim
  next h =>
    have h1 : R₁.length ≤ n + M.length + (R₁.length + fs.length - f) := by omega
    have h2 : fs.reverse.length ≤ n + M.length + (R₁.length + fs.length - f) - R₁.length := by
      simp; omega
    have h3 : M.length ≤ n + M.length + (R₁.length + fs.length - f) - R₁.length - fs.reverse.length := by
      simp; omega
    have h4 : (fs.take f).reverse.length ≤ n := by simp; omega
    conv => lhs; rw [envOf_append_ge h1, envOf_append_ge h2, envOf_append_ge h3]
    conv => rhs; rw [envOf_append_ge h4]
    simp only [List.length_reverse, hlt]
    congr 1; omega

section Decl

variable {S Sk : Glob → ZFSet.{u}} {k : Nat} {d : Decl}

/-- The parameters' values extend an environment of their context. -/
theorem envOk_params {a : List ZFSet.{u}} (ha : ArgsOk H Sk [] a (d.params.take d.uniform)) :
    EnvOk H Sk d.paramCtx (envOf a.reverse) := by
  have := envOk_of_args (Γ := []) (pre := []) (S := Sk) (H := H) trivial ha
  simpa [Decl.paramCtx] using this

/-- Walking a constructor's premises along fitting fields: the context stays
satisfied (with the recursive fields as `∅`), each plain field's type is in
its universe, and each recursive field's indices fit. -/
theorem walk (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors) {a : List ZFSet.{u}}
    {R : List ZFSet.{u} → ZFSet.{u} → Prop} {Rest : List Field} :
    {Fs P : List Field} → {Γ : List Tm} → {done fs : List ZFSet.{u}} →
    FieldsTyped (Valid H Sk) d Γ P.length Fs → C.fields = P ++ Fs ++ Rest → done.length = P.length →
    EnvOk H Sk Γ (envOf ((zeroRec C.fields done).reverse ++ a.reverse)) → a.length = d.uniform →
    FieldsOk a R (Fs.map (sField H Sk)) done.reverse fs →
    EnvOk H Sk (fieldCtx Γ Fs) (envOf ((zeroRec C.fields (done ++ fs)).reverse ++ a.reverse)) ∧
    ∀ (i : Nat) (ys : List Tm), Fs[i]? = some (Field.recur ys) →
      ArgsOk H Sk a (ys.map (interp H Sk · (envOf ((done ++ fs.take i).reverse ++ a.reverse))))
        d.indices
  | [], _, _, _, [], _, _, _, hΓ, _, _ => ⟨by simpa [fieldCtx] using hΓ, fun _ _ h => by simp at h⟩
  | .plain A l :: Fs, P, Γ, done, x :: fs, ⟨hA, _, hFs⟩, hP, hd, hΓ, ha, ⟨hx, hfs⟩ => by
    have hget : C.fields[done.length]? = some (.plain A l) := by
      rw [hP, hd, List.append_assoc, List.getElem?_append_right (Nat.le_refl _)]; simp
    have hA' : A ∈ C.termsAt done.length := by simp [Ctor.termsAt, hget]
    have hj : done.length ≤ C.fields.length := by rw [hP, hd]; simp
    have eA := interp_unused (S := Sk) (H := H) (DW.unused C hC) hj hA' rfl a.reverse
    have hx' : x ∈ₛ El (interp H Sk A (envOf ((zeroRec C.fields done).reverse ++ a.reverse))) := by
      rw [← eA]; exact hx
    have hΓ' : EnvOk H Sk (A :: Γ) (envOf ((zeroRec C.fields (done ++ [x])).reverse ++ a.reverse)) := by
      rw [zeroRec_snoc hget]
      simp only [fieldVal, List.reverse_append, List.reverse_singleton, List.cons_append,
        ← cons_envOf]
      exact ⟨hΓ, hx'⟩
    have ih := walk DW hC (Rest := Rest) (P := P ++ [.plain A l]) (done := done ++ [x])
      (by simpa using hFs) (by simp [hP]) (by simp [hd]) hΓ' ha (by simpa using hfs)
    refine ⟨by simpa [fieldCtx] using ih.1, fun i ys h => ?_⟩
    cases i with
    | zero => simp at h
    | succ i =>
      have := ih.2 i ys (by simpa using h)
      simpa using this
  | .recur ys :: Fs, P, Γ, done, x :: fs, ⟨hys, hFs⟩, hP, hd, hΓ, ha, ⟨_, hfs⟩ => by
    have hget : C.fields[done.length]? = some (.recur ys) := by
      rw [hP, hd, List.append_assoc, List.getElem?_append_right (Nat.le_refl _)]; simp
    have hΓ' : EnvOk H Sk (.top :: Γ) (envOf ((zeroRec C.fields (done ++ [x])).reverse ++ a.reverse)) := by
      rw [zeroRec_snoc hget]
      simp only [fieldVal, List.reverse_append, List.reverse_singleton, List.cons_append,
        ← cons_envOf]
      exact ⟨hΓ, (holds_propCode (p := True)).2 trivial⟩
    have ih := walk DW hC (Rest := Rest) (P := P ++ [.recur ys]) (done := done ++ [x])
      (by simpa using hFs) (by simp [hP]) (by simp [hd]) hΓ' ha (by simpa using hfs)
    refine ⟨by simpa [fieldCtx] using ih.1, fun i ys' h => ?_⟩
    cases i with
    | zero =>
      simp only [List.getElem?_cons_zero, Option.some.injEq, Field.recur.injEq] at h
      subst h
      have hv := hys _ hΓ
      rw [paramVars_interp (H := H) (S := Sk) (by rw [List.length_reverse, zeroRec_length, hd]) ha] at hv
      have hj : done.length ≤ C.fields.length := by rw [hP, hd]; simp
      have e : ∀ y ∈ ys, interp H Sk y (envOf ((done ++ List.take 0 (x :: fs)).reverse ++ a.reverse)) =
          interp H Sk y (envOf ((zeroRec C.fields done).reverse ++ a.reverse)) := fun y hy => by
        simp only [List.take_zero, List.append_nil]
        exact interp_unused (DW.unused C hC) hj (by simp [Ctor.termsAt, hget, hy]) rfl _
      rw [List.map_congr_left e]
      exact hv
    | succ i =>
      have := ih.2 i ys' (by simpa using h)
      simpa using this
  | .plain _ _ :: _, _, _, _, [], _, _, _, _, _, h => h.elim
  | .recur _ :: _, _, _, _, [], _, _, _, _, _, h => h.elim
  | [], _, _, _, _ :: _, _, _, _, _, _, h => h.elim

/-- Every plain field's type is in the type's universe. -/
theorem small_of (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors) {a : List ZFSet.{u}} :
    {Fs P : List Field} → {Γ : List Tm} → {done : List ZFSet.{u}} →
    FieldsTyped (Valid H Sk) d Γ P.length Fs → C.fields = P ++ Fs → done.length = P.length →
    EnvOk H Sk Γ (envOf ((zeroRec C.fields done).reverse ++ a.reverse)) →
    Small a (H.G (Hierarchy.index N d.level)) (Fs.map (sField H Sk)) done.reverse
  | [], _, _, _, _, _, _, _ => trivial
  | .plain A l :: Fs, P, Γ, done, ⟨hA, hl, hFs⟩, hP, hd, hΓ => by
    have hget : C.fields[done.length]? = some (.plain A l) := by
      rw [hP, hd, List.getElem?_append_right (Nat.le_refl _)]; simp
    have hj : done.length ≤ C.fields.length := by rw [hP, hd]; simp
    have eA := interp_unused (S := Sk) (H := H) (DW.unused C hC) hj
      (by simp [Ctor.termsAt, hget] : A ∈ C.termsAt done.length) rfl a.reverse
    have hAU : interp H Sk A (envOf ((zeroRec C.fields done).reverse ++ a.reverse)) ∈ₛ H.U l := by
      have := hA _ hΓ
      rwa [show interp H Sk (.sort l) _ = univCode l (H.U l) from rfl, El_univCode] at this
    refine ⟨?_, fun x hx => ?_⟩
    · show interp H Sk A (envOf (done.reverse ++ a.reverse)) ∈ₛ _
      rw [eA]; exact H.U_sub hl hAU
    · have hx' : x ∈ₛ El (interp H Sk A (envOf ((zeroRec C.fields done).reverse ++ a.reverse))) := by
        rw [← eA]; exact hx
      have hΓ' : EnvOk H Sk (A :: Γ) (envOf ((zeroRec C.fields (done ++ [x])).reverse ++ a.reverse)) := by
        rw [zeroRec_snoc hget]
        simp only [fieldVal, List.reverse_append, List.reverse_singleton, List.cons_append,
          ← cons_envOf]
        exact ⟨hΓ, hx'⟩
      have := small_of DW hC (P := P ++ [.plain A l]) (done := done ++ [x])
        (by simpa using hFs) (by simp [hP]) (by simp [hd]) hΓ'
      simpa using this
  | .recur ys :: Fs, P, Γ, done, ⟨_, hFs⟩, hP, hd, hΓ => fun x => by
    have hget : C.fields[done.length]? = some (.recur ys) := by
      rw [hP, hd, List.getElem?_append_right (Nat.le_refl _)]; simp
    have hΓ' : EnvOk H Sk (.top :: Γ) (envOf ((zeroRec C.fields (done ++ [x])).reverse ++ a.reverse)) := by
      rw [zeroRec_snoc hget]
      simp only [fieldVal, List.reverse_append, List.reverse_singleton, List.cons_append,
        ← cons_envOf]
      exact ⟨hΓ, (holds_propCode (p := True)).2 trivial⟩
    have := small_of DW hC (P := P ++ [.recur ys]) (done := done ++ [x])
      (by simpa using hFs) (by simp [hP]) (by simp [hd]) hΓ'
    simpa using this

theorem semCtors_get {S : Glob → ZFSet.{u}} {c : Nat} {C' : SCtor.{u}}
    (h : (semCtors H S d)[c]? = some C') : ∃ C, d.ctors[c]? = some C ∧ C' = sCtor H S C := by
  simp only [semCtors, List.getElem?_map] at h
  cases hc : d.ctors[c]? with
  | none => simp [hc] at h
  | some C => simp only [hc, Option.map_some, Option.some.injEq] at h; exact ⟨C, rfl, h.symm⟩

/-- At parameters that fit, every constructor's plain fields are small. -/
theorem smalls (DW : DeclWith (Valid H Sk) k d) {a : List ZFSet.{u}}
    (ha : ArgsOk H Sk [] a (d.params.take d.uniform)) :
    ∀ (c : Nat) (C : SCtor.{u}), (semCtors H Sk d)[c]? = some C →
      Small a (H.G (Hierarchy.index N d.level)) C.fields [] := by
  intro c C' h
  let ⟨C, hc, e⟩ := semCtors_get h
  subst e
  have hC := List.mem_of_getElem? hc
  have := small_of DW hC (P := []) (done := []) (DW.fields C hC) rfl rfl
    (by simpa [zeroRec_nil] using envOk_params ha)
  simpa [sCtor] using this

/-- Data-Form, in sets. -/
theorem dataCode_mem (DW : DeclWith (Valid H Sk) k d) {a : List ZFSet.{u}}
    (ha : ArgsOk H Sk [] a (d.params.take d.uniform)) (xs : List ZFSet.{u}) :
    dataCode (semCtors H Sk d) a k xs ∈ₛ H.U d.level := by
  have hU : H.U d.level = H.G (Hierarchy.index N d.level) := by
    unfold Hierarchy.U; simp [Nat.pos_iff_ne_zero.1 DW.level_pos]
  rw [hU]
  have hG := H.isUniverse _ (H.indexLt d.level)
  have hs := smalls DW ha
  exact hG.pair_mem (hG.pair_mem (hG.nat_mem 4) (hG.pair_mem (hG.nat_mem k) (label_mem hG hs)))
    (famSet_mem hG hs xs)

/-! ### The whole signature around a declaration -/

variable (hglob : ∀ g, g.decl = k → S g = meaning H Sk k d g)
  (hbelow : ∀ g, g.decl < k → S g = Sk g)
include hglob hbelow

omit hglob in
theorem interp_decl {t : Tm} (ht : t.globsBelow k) (ρ : Nat → ZFSet.{u}) :
    interp H S t ρ = interp H Sk t ρ :=
  interp_globs H S hbelow t ht ρ

omit hbelow in
theorem apps_data {vs : List ZFSet.{u}} (htop : ∀ v ∈ vs, v ∈ₛ top H) (hl : vs.length = d.params.length) :
    apps (S (.data k)) vs = dataCode (semCtors H Sk d) (vs.take d.uniform) k (vs.drop d.uniform) := by
  rw [hglob _ rfl, meaning, ← hl, apps_curry _ vs htop]

omit hbelow in
theorem apps_ctor {c : Nat} {C : Ctor} (hc : d.ctors[c]? = some C) {ps fs : List ZFSet.{u}}
    (htop : ∀ v ∈ ps ++ fs, v ∈ₛ top H) (hps : ps.length = d.uniform)
    (hfs : fs.length = C.fields.length) : apps (S (.ctor k c)) (ps ++ fs) = val c fs := by
  have hl : (ps ++ fs).length = d.uniform + ((d.ctors[c]?.map fun C => C.fields.length).getD 0) := by
    simp [hc, hps, hfs]
  rw [hglob _ rfl, meaning, ← hl, apps_curry _ _ htop]
  congr 1
  rw [← hps, List.drop_left]

omit hglob in
/-- Fields fitting a constructor's telescope fit its fields in sets, with `R`
holding of each recursive field that is in the family at its indices. -/
theorem fieldsFit (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors)
    {a : List ZFSet.{u}} (ha : a.length = d.uniform) {R : List ZFSet.{u} → ZFSet.{u} → Prop} {Rest : List Field} :
    {Fs P : List Field} → {done fs : List ZFSet.{u}} → C.fields = P ++ Fs ++ Rest → done.length = P.length →
    ArgsOk H S (a ++ done) fs (fieldsTele k d P.length Fs) →
    (∀ (i : Nat) (ys : List Tm) (x : ZFSet.{u}), Fs[i]? = some (Field.recur ys) →
      x ∈ₛ El (apps (S (.data k)) (a ++ ys.map (interp H S · (envOf ((done ++ fs.take i).reverse ++ a.reverse))))) →
      R (ys.map (interp H Sk · (envOf ((done ++ fs.take i).reverse ++ a.reverse)))) x) →
    FieldsOk a R (Fs.map (sField H Sk)) done.reverse fs
  | [], _, _, [], _, _, _, _ => trivial
  | .plain A l :: Fs, P, done, x :: fs, hP, hd, ⟨_, hx, h⟩, hR => by
    have hA : A.globsBelow k := by
      have := (DW.inScope.2 C hC).2 (.plain A l) (by rw [hP]; simp)
      exact this
    refine ⟨?_, ?_⟩
    · show x ∈ₛ El (interp H Sk A (envOf (done.reverse ++ a.reverse)))
      rw [← interp_decl hbelow hA, ← List.reverse_append]
      exact hx
    · have := fieldsFit DW hC ha (Rest := Rest) (Fs := Fs) (P := P ++ [.plain A l]) (done := done ++ [x]) (by simp [hP])
        (by simp [hd]) (by simpa [fieldsTele] using h) (fun i ys y hi hy => by
          have := hR (i + 1) ys y (by simpa using hi) (by simpa using hy)
          simpa using this)
      simpa using this
  | .recur ys :: Fs, P, done, x :: fs, hP, hd, ⟨_, hx, h⟩, hR => by
    refine ⟨?_, ?_⟩
    · have hy : ∀ y ∈ ys, y.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (by rw [hP]; simp)
      have := hR 0 ys x rfl (by
        simp only [List.take_zero, List.append_nil]
        simp only [Field.ty] at hx
        rw [interp_apps, List.map_append, List.reverse_append,
          paramVars_interp (H := H) (S := S) (by simp [hd]) ha] at hx
        exact hx)
      simp only [List.take_zero, List.append_nil] at this
      exact this
    · have := fieldsFit DW hC ha (Rest := Rest) (Fs := Fs) (P := P ++ [.recur ys]) (done := done ++ [x]) (by simp [hP])
        (by simp [hd]) (by simpa [fieldsTele] using h) (fun i ys' y hi hy => by
          have := hR (i + 1) ys' y (by simpa using hi) (by simpa using hy)
          simpa using this)
      simpa using this
  | .plain _ _ :: _, _, _, [], _, _, h, _ => h.elim
  | .recur _ :: _, _, _, [], _, _, h, _ => h.elim
  | [], _, _, _ :: _, _, _, h, _ => h.elim

omit hglob in
theorem params_congr (DW : DeclWith (Valid H Sk) k d) {pre a : List ZFSet.{u}} {n : Nat} :
    ArgsOk H S pre a (d.params.take n) ↔ ArgsOk H Sk pre a (d.params.take n) :=
  ArgsOk.congr fun p hp ρ => interp_decl hbelow (DW.inScope.1 p (List.mem_of_mem_take hp)) ρ

omit hglob in
theorem indices_congr (DW : DeclWith (Valid H Sk) k d) {pre a : List ZFSet.{u}} :
    ArgsOk H S pre a d.indices ↔ ArgsOk H Sk pre a d.indices :=
  ArgsOk.congr fun p hp ρ => interp_decl hbelow (DW.inScope.1 p (List.mem_of_mem_drop hp)) ρ

/-- Data-Ctor, in sets: fields fitting a constructor are the family's
fields, and its result's indices fit. -/
theorem ctorFacts (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors)
    {a fs : List ZFSet.{u}} (ha : ArgsOk H S [] a (d.params.take d.uniform))
    (hfs : ArgsOk H S a fs (C.tele k d)) :
    FieldsOk a (Member (semCtors H Sk d) a) (sCtor H Sk C).fields [] fs ∧
      ArgsOk H Sk a ((sCtor H Sk C).result (fs.reverse ++ a.reverse)) d.indices := by
  have hal : a.length = d.uniform := by
    rw [ha.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
  have ha' := (params_congr hbelow DW).1 ha
  have hfs' : ArgsOk H S (a ++ []) fs (fieldsTele k d ([] : List Field).length C.fields) := by
    simpa [Ctor.tele] using hfs
  have plain := fieldsFit hbelow DW hC hal (R := fun _ _ => True) (Rest := []) (P := []) (done := [])
    (by simp) rfl hfs'
    (fun _ _ _ _ _ => trivial)
  have w := walk DW hC (P := []) (done := []) (R := fun _ _ => True) (Rest := []) (DW.fields C hC)
    (by simp) rfl
    (by simpa [zeroRec_nil] using envOk_params ha') hal (by simpa [sCtor] using plain)
  refine ⟨?_, ?_⟩
  · have := fieldsFit hbelow DW hC hal (R := Member (semCtors H Sk d) a) (Rest := []) (P := [])
      (done := []) (by simp) rfl
      hfs' (fun i ys x hi hx => by
        have hfit := w.2 i ys (by simpa using hi)
        simp only [List.nil_append] at hfit hx ⊢
        have hy : ∀ y ∈ ys, y.globsBelow k := (DW.inScope.2 C hC).2 (.recur ys) (List.mem_of_getElem? hi)
        rw [List.map_congr_left fun y hy' => interp_decl hbelow (hy y hy') _] at hx
        have htop : ∀ v ∈ a ++ ys.map (interp H Sk · (envOf ((fs.take i).reverse ++ a.reverse))),
            v ∈ₛ top H := by
          intro v hv
          rcases List.mem_append.1 hv with hv | hv
          · exact ha.top v hv
          · exact hfit.top v hv
        have hlen : (a ++ ys.map (interp H Sk · (envOf ((fs.take i).reverse ++ a.reverse)))).length =
            d.params.length := by
          rw [List.length_append, hal, hfit.length, Decl.indices, List.length_drop]
          have := DW.uniform_le; omega
        rw [apps_data hglob htop hlen, El_dataCode, List.take_left' hal, List.drop_left' hal] at hx
        exact mem_famSet.1 hx)
    simpa [sCtor] using this
  · have hres := DW.result C hC _ w.1
    rw [paramVars_interp (H := H) (S := Sk) (by simp [zeroRec_length, plain.length]) hal] at hres
    have e : ∀ y ∈ C.result, interp H Sk y (envOf ((zeroRec C.fields fs).reverse ++ a.reverse)) =
        interp H Sk y (envOf (fs.reverse ++ a.reverse)) := fun y hy =>
      (interp_unused (DW.unused C hC) (Nat.le_refl _) (by simp [Ctor.termsAt, hy])
        (by simpa using plain.length) _).symm
    simp only [List.nil_append] at hres
    rw [List.map_congr_left e] at hres
    exact hres

omit hglob hbelow in
/-- Every field that fits lies in the largest universe. -/
theorem fields_top {G : ZFSet.{u}} (hGi : ∀ x, x ∈ₛ G → x ∈ₛ top H) (hG : IsUniverse G)
    {a : List ZFSet.{u}} {R : List ZFSet.{u} → ZFSet.{u} → Prop} (hR : ∀ ys x, R ys x → x ∈ₛ top H) :
    {Fs : List SField.{u}} → {pre fs : List ZFSet.{u}} → Small a G Fs pre → FieldsOk a R Fs pre fs →
    ∀ v ∈ fs, v ∈ₛ top H
  | [], _, [], _, _ => fun _ h => absurd h List.not_mem_nil
  | .plain _ _ :: _, _, x :: _, ⟨hA, hs⟩, ⟨hx, hf⟩ => fun v hv => by
    rcases List.mem_cons.1 hv with e | hv
    · subst e; exact hGi _ (hG.trans _ _ (hG.snd_mem hA) hx)
    · exact fields_top hGi hG hR (hs x hx) hf v hv
  | .recur _ :: _, _, x :: _, hs, ⟨hx, hf⟩ => fun v hv => by
    rcases List.mem_cons.1 hv with e | hv
    · subst e; exact hR _ _ hx
    · exact fields_top hGi hG hR (hs x) hf v hv

omit hglob hbelow in
theorem level_top {x : ZFSet.{u}}
    (hx : x ∈ₛ H.G (Hierarchy.index N d.level)) : x ∈ₛ top H := by
  have hN : N - 1 < N := by have := H.pos; omega
  exact H.mono (by unfold Hierarchy.index; omega) hN hx

omit hglob hbelow in
/-- The fields of a value of the family lie in the largest universe. -/
theorem member_fields_top (DW : DeclWith (Valid H Sk) k d) {a fs : List ZFSet.{u}}
    (ha : ArgsOk H Sk [] a (d.params.take d.uniform)) {c : Nat} {C : SCtor.{u}}
    (hc : (semCtors H Sk d)[c]? = some C) (hf : FieldsOk a (Member (semCtors H Sk d) a) C.fields [] fs) :
    ∀ v ∈ fs, v ∈ₛ top H := by
  have hG := H.isUniverse _ (H.indexLt d.level)
  have hs := smalls DW ha
  exact fields_top (fun _ hx => level_top hx) hG
    (fun _ _ hx => level_top (hG.trans _ _ (Vω_mem hG hs) (member_mem_Vω hx))) (hs c C hc) hf

/-- Fields that fit the first `j` of a constructor's: they lie in the bound,
and a recursive field number `j`'s indices fit after them. -/
theorem prefixFacts (DW : DeclWith (Valid H Sk) k d) {C : Ctor} (hC : C ∈ d.ctors) {j : Nat}
    (hj : j < C.fields.length) {a bs : List ZFSet.{u}}
    (ha : ArgsOk H S [] a (d.params.take d.uniform)) (hbs : ArgsOk H S a bs ((C.tele k d).take j)) :
    FieldsOk a (fun _ x => x ∈ₛ Vω (semCtors H Sk d) a) ((sCtor H Sk C).fields.take j) [] bs ∧
    ∀ ys, C.fields[j]? = some (Field.recur ys) →
      ArgsOk H Sk a (ys.map (interp H Sk · (envOf (bs.reverse ++ a.reverse)))) d.indices := by
  have hal : a.length = d.uniform := by
    rw [ha.length, List.length_take]; exact Nat.min_eq_left DW.uniform_le
  have ha' := (params_congr hbelow DW).1 ha
  have hbs' : ArgsOk H S (a ++ []) bs (fieldsTele k d ([] : List Field).length (C.fields.take j)) := by
    rw [Ctor.tele, fieldsTele_take] at hbs; simpa using hbs
  have hsplit : C.fields = [] ++ C.fields.take j ++ C.fields.drop j := by simp
  have plain := fieldsFit hbelow DW hC hal (R := fun _ _ => True) (Rest := C.fields.drop j) (P := [])
    (done := []) hsplit rfl hbs' (fun _ _ _ _ _ => trivial)
  have FT := FieldsTyped.append (Fs := C.fields.take j) (R := C.fields.drop j)
    (by rw [List.take_append_drop]; exact DW.fields C hC)
  have w := walk DW hC (R := fun _ _ => True) (Rest := C.fields.drop j) (P := []) (done := []) FT.1
    hsplit rfl (by simpa [zeroRec_nil] using envOk_params ha') hal (by simpa using plain)
  have hbl : bs.length = j := by
    rw [hbs.length, List.length_take, Ctor.tele, fieldsTele_length]; omega
  refine ⟨?_, fun ys hF => ?_⟩
  · have := fieldsFit hbelow DW hC hal (R := fun _ x => x ∈ₛ Vω (semCtors H Sk d) a)
      (Rest := C.fields.drop j) (P := []) (done := []) hsplit rfl hbs' (fun i ys x hi hx => by
        have hfit := w.2 i ys hi
        simp only [List.nil_append] at hfit hx
        have hy : ∀ y ∈ ys, y.globsBelow k :=
          (DW.inScope.2 C hC).2 (.recur ys) (List.mem_of_mem_take (List.mem_of_getElem? hi))
        rw [List.map_congr_left fun y hy' => interp_decl hbelow (hy y hy') _] at hx
        have htop : ∀ v ∈ a ++ ys.map (interp H Sk · (envOf ((bs.take i).reverse ++ a.reverse))),
            v ∈ₛ top H := fun v hv => by
          rcases List.mem_append.1 hv with hv | hv
          · exact ha.top v hv
          · exact hfit.top v hv
        have hlen : (a ++ ys.map (interp H Sk · (envOf ((bs.take i).reverse ++ a.reverse)))).length =
            d.params.length := by
          rw [List.length_append, hal, hfit.length, Decl.indices, List.length_drop]
          have := DW.uniform_le; omega
        rw [apps_data hglob htop hlen, El_dataCode, List.take_left' hal, List.drop_left' hal] at hx
        exact member_mem_Vω (mem_famSet.1 hx))
    simpa [sCtor, List.map_take] using this
  · have hdrop : C.fields.drop j = .recur ys :: C.fields.drop (j + 1) := by
      rw [List.drop_eq_getElem_cons hj]
      congr 1
      exact Option.some.inj ((List.getElem?_eq_getElem hj).symm.trans hF)
    have h2 := FT.2
    rw [hdrop] at h2
    have hlt : (C.fields.take j).length = j := by simp; omega
    simp only [List.length_take, Nat.zero_add, Nat.min_eq_left (Nat.le_of_lt hj)] at h2
    have hv := h2.1 _ w.1
    simp only [List.nil_append] at hv
    rw [paramVars_interp (H := H) (S := Sk) (by simp [zeroRec_length, hbl]) hal] at hv
    have hj' : j ≤ C.fields.length := Nat.le_of_lt hj
    rw [List.map_congr_left fun y hy =>
      (interp_unused (DW.unused C hC) hj' (by simp [Ctor.termsAt, hF, hy]) hbl _).symm] at hv
    exact hv

end Decl

end Model
