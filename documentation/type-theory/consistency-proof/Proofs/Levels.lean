/-!
# Levels

The kernel's level expressions and its decision procedure for `<=`
(`crates/kernel/src/level.rs`), with the procedure proved sound (#498, #479):
when it answers yes, `a <= b` holds under every assignment of the
parameters. Completeness, that it answers yes whenever that holds, is not
proved here.

The rest of the proofs read levels as closed numbers. A level-polymorphic
derivation stands for its instances, one per assignment (see
`Proofs/Consistency.lean`); what makes an instance's level premises hold is
this soundness: the kernel checks them with `Level.leq` once, for all
assignments.
-/

/-- `0`, successor, `max`, `imax` and parameters. -/
inductive Lvl where
  | zero
  | succ (l : Lvl)
  | max (a b : Lvl)
  | imax (a b : Lvl)
  | param (p : Nat)
  deriving DecidableEq

namespace Lvl

/-- A level's value under an assignment of its parameters. -/
def eval (σ : Nat → Nat) : Lvl → Nat
  | zero => 0
  | succ l => eval σ l + 1
  | max a b => Nat.max (eval σ a) (eval σ b)
  | imax a b => if eval σ b = 0 then 0 else Nat.max (eval σ a) (eval σ b)
  | param p => σ p

/-! ## Construction, as `Level::max` and `Level::imax` fold -/

def mkMax : Lvl → Lvl → Lvl
  | zero, b => b
  | a, zero => a
  | a, b => if a = b then a else match a, b with
    | succ x, succ y => succ (mkMax x y)
    | a, b => max a b

def mkImax (a b : Lvl) : Lvl :=
  match b with
  | zero => zero
  | succ _ => mkMax a b
  | b => if a = zero then b else imax a b

theorem eval_mkMax (σ : Nat → Nat) : (a b : Lvl) → eval σ (mkMax a b) = Nat.max (eval σ a) (eval σ b)
  | zero, b => by simp [mkMax, eval]
  | succ x, zero => by simp [mkMax, eval]
  | max x y, zero => by simp [mkMax, eval]
  | imax x y, zero => by simp [mkMax, eval]
  | param p, zero => by simp [mkMax, eval]
  | succ x, succ y => by
    unfold mkMax
    split
    next h => cases h <;> simp
    next => simp [eval, eval_mkMax σ x y, Nat.succ_max_succ]
  | succ _, max _ _ | succ _, imax _ _ | succ _, param _
  | max _ _, succ _ | max _ _, max _ _ | max _ _, imax _ _ | max _ _, param _
  | imax _ _, succ _ | imax _ _, max _ _ | imax _ _, imax _ _ | imax _ _, param _
  | param _, succ _ | param _, max _ _ | param _, imax _ _ | param _, param _ => by
    unfold mkMax
    split
    next h => cases h <;> simp
    next => simp [eval]

theorem eval_mkImax (σ : Nat → Nat) (a b : Lvl) :
    eval σ (mkImax a b) = eval σ (imax a b) := by
  unfold mkImax
  split
  next => simp [eval]
  next => simp [eval_mkMax, eval]
  next =>
    split
    next h => subst h; simp only [eval]; split <;> simp_all
    next => rfl

/-- Parameter `p` replaced by `by`, as `Level::substitute`. -/
def subst : Lvl → Nat → Lvl → Lvl
  | zero, _, _ => zero
  | succ l, p, by_ => succ (subst l p by_)
  | max a b, p, by_ => mkMax (subst a p by_) (subst b p by_)
  | imax a b, p, by_ => mkImax (subst a p by_) (subst b p by_)
  | param q, p, by_ => if q = p then by_ else param q

/-- `σ` with parameter `p` sent to `n`. -/
def update (σ : Nat → Nat) (p n : Nat) : Nat → Nat := fun q => if q = p then n else σ q

theorem eval_subst (σ : Nat → Nat) (p : Nat) (by_ : Lvl) :
    (l : Lvl) → eval σ (subst l p by_) = eval (update σ p (eval σ by_)) l
  | zero => rfl
  | succ l => by simp [subst, eval, eval_subst σ p by_ l]
  | max a b => by simp [subst, eval_mkMax, eval, eval_subst σ p by_ a, eval_subst σ p by_ b]
  | imax a b => by simp [subst, eval_mkImax, eval, eval_subst σ p by_ a, eval_subst σ p by_ b]
  | param q => by
    simp only [subst, update, eval]
    split <;> rfl

/-! ## Max-plus normal forms -/

/-- `max(constant, p + k for (p, k) in params)`, as `MaxPlus`; the kernel's
map is a list here, where a parameter listed twice counts at its largest
offset. -/
structure MaxPlus where
  constant : Nat
  params : List (Nat × Nat)

namespace MaxPlus

/-- One entry's value, `p + k`. -/
def term (σ : Nat → Nat) (pk : Nat × Nat) : Nat := σ pk.1 + pk.2

def eval (σ : Nat → Nat) (m : MaxPlus) : Nat :=
  m.params.foldl (fun acc pk => Nat.max acc (term σ pk)) m.constant

def join (m m' : MaxPlus) : MaxPlus := ⟨Nat.max m.constant m'.constant, m.params ++ m'.params⟩

def plusOne (m : MaxPlus) : MaxPlus := ⟨m.constant + 1, m.params.map fun (p, k) => (p, k + 1)⟩

def isZero (m : MaxPlus) : Bool := m.constant == 0 && m.params.isEmpty

def isNonzero (m : MaxPlus) : Bool := decide (1 ≤ m.constant) || m.params.any fun pk => decide (1 ≤ pk.2)

/-- `m <= m'` under every assignment, as `MaxPlus::leq`. -/
def leq (m m' : MaxPlus) : Bool :=
  decide (m.constant ≤ m'.params.foldl (fun acc pk => Nat.max acc pk.2) m'.constant) &&
    m.params.all fun pk => m'.params.any fun qj => qj.1 == pk.1 && decide (pk.2 ≤ qj.2)

theorem foldl_max_ge (f : Nat × Nat → Nat) :
    (l : List (Nat × Nat)) → (c : Nat) → c ≤ l.foldl (fun acc pk => Nat.max acc (f pk)) c
  | [], _ => Nat.le_refl _
  | x :: l, c => Nat.le_trans (Nat.le_max_left c (f x)) (foldl_max_ge f l _)

theorem foldl_max_mem (f : Nat × Nat → Nat) :
    (l : List (Nat × Nat)) → (c : Nat) → ∀ x ∈ l, f x ≤ l.foldl (fun acc pk => Nat.max acc (f pk)) c
  | [], _, _, h => absurd h List.not_mem_nil
  | y :: l, c, x, h => by
    rcases List.mem_cons.1 h with e | h
    · subst e
      exact Nat.le_trans (Nat.le_max_right c (f x)) (foldl_max_ge f l _)
    · exact foldl_max_mem f l _ x h

/-- The value is the largest of the constant and the entries. -/
theorem foldl_max_le (f : Nat × Nat → Nat) :
    (l : List (Nat × Nat)) → (c n : Nat) → c ≤ n → (∀ x ∈ l, f x ≤ n) →
    l.foldl (fun acc pk => Nat.max acc (f pk)) c ≤ n
  | [], _, _, hc, _ => hc
  | x :: l, _, n, hc, h =>
    foldl_max_le f l _ n (Nat.max_le.2 ⟨hc, h x List.mem_cons_self⟩)
      fun y hy => h y (List.mem_cons_of_mem _ hy)

theorem eval_le_iff (σ : Nat → Nat) (m : MaxPlus) (n : Nat) :
    m.eval σ ≤ n ↔ m.constant ≤ n ∧ ∀ pk ∈ m.params, term σ pk ≤ n :=
  ⟨fun h => ⟨Nat.le_trans (foldl_max_ge _ _ _) h,
      fun pk hpk => Nat.le_trans (foldl_max_mem (term σ) _ _ pk hpk) h⟩,
    fun ⟨hc, hp⟩ => foldl_max_le _ _ _ _ hc hp⟩

theorem le_eval_of_mem (σ : Nat → Nat) (m : MaxPlus) {pk : Nat × Nat} (h : pk ∈ m.params) :
    term σ pk ≤ m.eval σ := foldl_max_mem (term σ) _ _ pk h

theorem constant_le_eval (σ : Nat → Nat) (m : MaxPlus) : m.constant ≤ m.eval σ := foldl_max_ge _ _ _

theorem eval_join (σ : Nat → Nat) (m m' : MaxPlus) :
    (join m m').eval σ = Nat.max (m.eval σ) (m'.eval σ) := by
  apply Nat.le_antisymm
  · rw [eval_le_iff]
    refine ⟨Nat.max_le.2 ⟨Nat.le_trans (constant_le_eval σ m) (Nat.le_max_left _ _),
      Nat.le_trans (constant_le_eval σ m') (Nat.le_max_right _ _)⟩, fun pk hpk => ?_⟩
    rcases List.mem_append.1 hpk with h | h
    · exact Nat.le_trans (le_eval_of_mem σ m h) (Nat.le_max_left _ _)
    · exact Nat.le_trans (le_eval_of_mem σ m' h) (Nat.le_max_right _ _)
  · refine Nat.max_le.2 ⟨?_, ?_⟩ <;> rw [eval_le_iff]
    · exact ⟨Nat.le_trans (Nat.le_max_left m.constant m'.constant) (constant_le_eval σ (join m m')),
        fun pk hpk => le_eval_of_mem σ (join m m') (List.mem_append_left _ hpk)⟩
    · exact ⟨Nat.le_trans (Nat.le_max_right m.constant m'.constant) (constant_le_eval σ (join m m')),
        fun pk hpk => le_eval_of_mem σ (join m m') (List.mem_append_right _ hpk)⟩

theorem foldl_plusOne (σ : Nat → Nat) : (l : List (Nat × Nat)) → (c : Nat) →
    (l.map fun (p, k) => (p, k + 1)).foldl (fun acc pk => Nat.max acc (term σ pk)) (c + 1) =
      l.foldl (fun acc pk => Nat.max acc (term σ pk)) c + 1
  | [], _ => rfl
  | (p, k) :: l, c => by
    simp only [List.map_cons, List.foldl_cons]
    rw [show Nat.max (c + 1) (term σ (p, k + 1)) = Nat.max c (term σ (p, k)) + 1 by
      show Nat.max (c + 1) (σ p + (k + 1)) = Nat.max c (σ p + k) + 1
      simp only [Nat.max_def]
      split <;> split <;> omega]
    exact foldl_plusOne σ l _

theorem eval_plusOne (σ : Nat → Nat) (m : MaxPlus) : (plusOne m).eval σ = m.eval σ + 1 :=
  foldl_plusOne σ m.params m.constant

theorem eval_isZero {m : MaxPlus} (h : m.isZero = true) (σ : Nat → Nat) : m.eval σ = 0 := by
  simp only [isZero, Bool.and_eq_true, beq_iff_eq, List.isEmpty_iff] at h
  obtain ⟨hc, hp⟩ := h
  simp [eval, hc, hp]

theorem eval_isNonzero {m : MaxPlus} (h : m.isNonzero = true) (σ : Nat → Nat) : 1 ≤ m.eval σ := by
  simp only [isNonzero, Bool.or_eq_true, decide_eq_true_eq, List.any_eq_true] at h
  rcases h with h | ⟨pk, hpk, hk⟩
  · exact Nat.le_trans h (constant_le_eval σ m)
  · exact Nat.le_trans (by simp only [term]; omega) (le_eval_of_mem σ m hpk)

theorem leq_sound {m m' : MaxPlus} (h : m.leq m' = true) (σ : Nat → Nat) : m.eval σ ≤ m'.eval σ := by
  simp only [leq, Bool.and_eq_true, decide_eq_true_eq, List.all_eq_true, List.any_eq_true,
    Bool.and_eq_true, beq_iff_eq] at h
  obtain ⟨hc, hp⟩ := h
  rw [eval_le_iff]
  refine ⟨Nat.le_trans hc (foldl_max_le _ _ _ _ (constant_le_eval σ m') fun qj hqj =>
    Nat.le_trans (by simp only [term]; omega) (le_eval_of_mem σ m' hqj)), fun pk hpk => ?_⟩
  obtain ⟨qj, hqj, he, hk⟩ := hp pk hpk
  have := le_eval_of_mem σ m' hqj
  simp only [term] at this ⊢
  rw [he] at this
  omega

end MaxPlus

/-- The normal form, or the parameter to split on, as `Level::max_plus`. -/
def maxPlus : Lvl → Except Nat MaxPlus
  | zero => .ok ⟨0, []⟩
  | succ l => (maxPlus l).map MaxPlus.plusOne
  | max a b => do
    let a ← maxPlus a
    let b ← maxPlus b
    pure (a.join b)
  | imax a b => do
    let b ← maxPlus b
    if b.isZero then pure ⟨0, []⟩
    else if b.isNonzero then do
      let a ← maxPlus a
      pure (a.join b)
    else match b.params with
      | pk :: _ => .error pk.1
      | [] => pure ⟨0, []⟩
  | param p => .ok ⟨0, [(p, 0)]⟩

theorem maxPlus_eval (σ : Nat → Nat) : (l : Lvl) → (m : MaxPlus) → l.maxPlus = .ok m →
    eval σ l = m.eval σ
  | zero, m, h => by
    simp only [maxPlus, Except.ok.injEq] at h; subst h; rfl
  | succ l, m, h => by
    simp only [maxPlus] at h
    cases hl : l.maxPlus with
    | error p => rw [hl] at h; cases h
    | ok ml =>
      rw [hl] at h
      cases h
      rw [eval, maxPlus_eval σ l ml hl, MaxPlus.eval_plusOne]
  | max a b, m, h => by
    simp only [maxPlus] at h
    cases ha : a.maxPlus with
    | error p => rw [ha] at h; cases h
    | ok ma =>
      cases hb : b.maxPlus with
      | error p => rw [ha, hb] at h; cases h
      | ok mb =>
        rw [ha, hb] at h
        cases h
        rw [eval, maxPlus_eval σ a ma ha, maxPlus_eval σ b mb hb, MaxPlus.eval_join]
  | imax a b, m, h => by
    simp only [maxPlus] at h
    cases hb : b.maxPlus with
    | error p => rw [hb] at h; cases h
    | ok mb =>
      rw [hb] at h
      simp only [bind, Except.bind] at h
      have eb := maxPlus_eval σ b mb hb
      simp only [eval]
      by_cases hz : mb.isZero = true
      · simp only [hz, ↓reduceIte] at h
        cases h
        rw [eb, MaxPlus.eval_isZero hz σ]; rfl
      · have hz' : mb.isZero = false := by simpa using hz
        simp only [hz', Bool.false_eq_true, ↓reduceIte] at h
        by_cases hn : mb.isNonzero = true
        · simp only [hn, ↓reduceIte] at h
          cases ha : a.maxPlus with
          | error p => rw [ha] at h; cases h
          | ok ma =>
            rw [ha] at h
            cases h
            have := MaxPlus.eval_isNonzero hn σ
            rw [eb]
            simp only [show ¬ mb.eval σ = 0 by omega, ↓reduceIte]
            rw [maxPlus_eval σ a ma ha, MaxPlus.eval_join]
        · have hn' : mb.isNonzero = false := by simpa using hn
          simp only [hn', Bool.false_eq_true, ↓reduceIte] at h
          cases hp : mb.params with
          | cons pk _ => rw [hp] at h; cases h
          | nil =>
            simp [MaxPlus.isZero, hp] at hz
            simp [MaxPlus.isNonzero, hp] at hn
            omega
  | param p, m, h => by
    simp only [maxPlus, Except.ok.injEq] at h
    subst h
    simp [eval, MaxPlus.eval, MaxPlus.term]

/-! ## The comparison -/

/-- `a <= b` for every assignment, as `Level::leq`, with `fuel` bounding the
depth of case splits: the kernel's recursion is this with as much fuel as
its run takes, and it terminates. -/
def leq : Nat → Lvl → Lvl → Bool
  | 0, _, _ => false
  | n + 1, a, b =>
    match a.maxPlus, b.maxPlus with
    | .ok ma, .ok mb => ma.leq mb
    | .error p, _ | .ok _, .error p =>
      leq n (a.subst p zero) (b.subst p zero) &&
        leq n (a.subst p (succ (param p))) (b.subst p (succ (param p)))

-- Algorithm [Level comparison by case split]
/-- Soundness: a yes from the comparison holds under every assignment. -/
theorem leq_sound : (n : Nat) → (a b : Lvl) → leq n a b = true → ∀ σ, eval σ a ≤ eval σ b
  | 0, _, _, h => by simp [leq] at h
  | n + 1, a, b, h => fun σ => by
    have split : ∀ p, leq n (a.subst p zero) (b.subst p zero) = true →
        leq n (a.subst p (succ (param p))) (b.subst p (succ (param p))) = true →
        eval σ a ≤ eval σ b := fun p h0 h1 => by
      cases hp : σ p with
      | zero =>
        have := leq_sound n _ _ h0 σ
        rw [eval_subst, eval_subst] at this
        have e : update σ p (eval σ zero) = σ := by
          funext q; simp only [update, eval]; split <;> simp_all
        rwa [e] at this
      | succ k =>
        let σ' := update σ p k
        have := leq_sound n _ _ h1 σ'
        rw [eval_subst, eval_subst] at this
        have e : update σ' p (eval σ' (succ (param p))) = σ := by
          funext q; simp only [σ', update, eval]; split <;> simp_all
        rwa [e] at this
    simp only [leq] at h
    split at h
    next ma mb ha hb => rw [maxPlus_eval σ a ma ha, maxPlus_eval σ b mb hb]; exact MaxPlus.leq_sound h σ
    next p _ =>
      simp only [Bool.and_eq_true] at h
      exact split p h.1 h.2
    next p _ _ =>
      simp only [Bool.and_eq_true] at h
      exact split p h.1 h.2

/-- `Level::is_nonzero`: one or more under every assignment, the level of data. -/
theorem nonzero_sound {n : Nat} {l : Lvl} (h : leq n (succ zero) l = true) (σ : Nat → Nat) :
    eval σ l ≠ 0 := by
  have := leq_sound n _ _ h σ; simp [eval] at this; omega

/-- `Level::is_zero`: zero under every assignment, the level of propositions. -/
theorem zero_sound {n : Nat} {l : Lvl} (h : leq n l zero = true) (σ : Nat → Nat) : eval σ l = 0 := by
  have := leq_sound n _ _ h σ; simp [eval] at this; omega

end Lvl
