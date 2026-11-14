/-!
# Syntax

The kernel's terms (`crates/kernel/src/term.rs`), with de Bruijn indices.

Differences from the Rust, all because the rules here are declarative:

* Levels are closed numbers. The kernel's level expressions, with `max`,
  `imax` and parameters, are `Proofs/Levels.lean`: a level-polymorphic
  derivation stands for its closed instances (see `consistent_poly`).
* A lambda carries its domain. The kernel's `Lam` omits it because a lambda is
  only ever checked against a known Pi type, whose domain this is.
* A class `@qin a` carries its quotient's carrier and relation, as Lean's
  `Quot.mk` does; the kernel's is checked against a known quotient type.
* A global of an inductive declaration is used applied to all its arguments:
  the kernel's partial application is a lambda (`inductive::curry`), so it is
  the eta-expansion of a full one.

Proofs follow the kernel: every term checked against a proposition elaborates
to `irr`, the kernel's `Irrelevant` (see Rule [Proof-Witness]).
-/

/-- A global an inductive declaration introduces; `k` numbers the declaration
in the signature. -/
inductive Glob where
  /-- The type former. -/
  | data (k : Nat)
  /-- Constructor `c`. -/
  | ctor (k c : Nat)
  /-- The recursor, with its motive in `@Type m`. -/
  | elim (k m : Nat)
  deriving DecidableEq

/-- The declaration a global belongs to. -/
def Glob.decl : Glob → Nat
  | data k => k
  | ctor k _ => k
  | elim k _ => k

inductive Tm where
  | var (i : Nat)
  /-- `@Type l`; `@Type 0` is the propositions. -/
  | sort (l : Nat)
  /-- `(x : A) -> B`, with `A : @Type l` and `B : @Type l'`. -/
  | pi (l l' : Nat) (A B : Tm)
  | lam (A t : Tm)
  | app (f a : Tm)
  /-- `let x : A = v in t`. -/
  | letIn (A v t : Tm)
  /-- `(x : A) * B`, with `A : @Type l` and `B : @Type l'`. -/
  | sigma (l l' : Nat) (A B : Tm)
  | pair (a b : Tm)
  | fst (p : Tm)
  | snd (p : Tm)
  | top
  | tt
  | bot
  /-- `@absurd A p`, with `p : @Bottom`. -/
  | absurd (A p : Tm)
  /-- `a ~ b` at type `A`, where `A : @Type l`. -/
  | eq (l : Nat) (A a b : Tm)
  /-- `@cast A B e t`. -/
  | cast (A B e t : Tm)
  /-- A proof, whose value is irrelevant. -/
  | irr
  /-- A global, applied to its arguments with `app`. -/
  | glob (g : Glob)
  /-- `@Quot A R`, with `A : @Type l`. -/
  | quot (l : Nat) (A R : Tm)
  /-- `@qin a`, the class of `a` in `@Quot A R`. -/
  | qin (A R a : Tm)
  /-- `@qelim P f q`, with the motive `P` into `@Type m`; the proof that `f`
  respects the relation is erased, as in the kernel. -/
  | qelim (m : Nat) (P f q : Tm)
  deriving Inhabited

namespace Tm

/-- A renaming, lifted under a binder. -/
def up (ξ : Nat → Nat) : Nat → Nat
  | 0 => 0
  | n + 1 => ξ n + 1

def rename (ξ : Nat → Nat) : Tm → Tm
  | var i => var (ξ i)
  | sort l => sort l
  | pi l l' A B => pi l l' (rename ξ A) (rename (up ξ) B)
  | lam A t => lam (rename ξ A) (rename (up ξ) t)
  | app f a => app (rename ξ f) (rename ξ a)
  | letIn A v t => letIn (rename ξ A) (rename ξ v) (rename (up ξ) t)
  | sigma l l' A B => sigma l l' (rename ξ A) (rename (up ξ) B)
  | pair a b => pair (rename ξ a) (rename ξ b)
  | fst p => fst (rename ξ p)
  | snd p => snd (rename ξ p)
  | top => top
  | tt => tt
  | bot => bot
  | absurd A p => absurd (rename ξ A) (rename ξ p)
  | eq l A a b => eq l (rename ξ A) (rename ξ a) (rename ξ b)
  | cast A B e t => cast (rename ξ A) (rename ξ B) (rename ξ e) (rename ξ t)
  | irr => irr
  | glob g => glob g
  | quot l A R => quot l (rename ξ A) (rename ξ R)
  | qin A R a => qin (rename ξ A) (rename ξ R) (rename ξ a)
  | qelim m P f q => qelim m (rename ξ P) (rename ξ f) (rename ξ q)

/-- A term in one more variable. -/
def shift (t : Tm) : Tm := rename Nat.succ t

/-- A substitution, lifted under a binder. -/
def upS (σ : Nat → Tm) : Nat → Tm
  | 0 => var 0
  | n + 1 => shift (σ n)

def subst (σ : Nat → Tm) : Tm → Tm
  | var i => σ i
  | sort l => sort l
  | pi l l' A B => pi l l' (subst σ A) (subst (upS σ) B)
  | lam A t => lam (subst σ A) (subst (upS σ) t)
  | app f a => app (subst σ f) (subst σ a)
  | letIn A v t => letIn (subst σ A) (subst σ v) (subst (upS σ) t)
  | sigma l l' A B => sigma l l' (subst σ A) (subst (upS σ) B)
  | pair a b => pair (subst σ a) (subst σ b)
  | fst p => fst (subst σ p)
  | snd p => snd (subst σ p)
  | top => top
  | tt => tt
  | bot => bot
  | absurd A p => absurd (subst σ A) (subst σ p)
  | eq l A a b => eq l (subst σ A) (subst σ a) (subst σ b)
  | cast A B e t => cast (subst σ A) (subst σ B) (subst σ e) (subst σ t)
  | irr => irr
  | glob g => glob g
  | quot l A R => quot l (subst σ A) (subst σ R)
  | qin A R a => qin (subst σ A) (subst σ R) (subst σ a)
  | qelim m P f q => qelim m (subst σ P) (subst σ f) (subst σ q)

/-- `a` for variable 0, the rest moved down. -/
def single (a : Tm) : Nat → Tm
  | 0 => a
  | n + 1 => var n

/-- `B[x := a]`. -/
def inst (B a : Tm) : Tm := subst (single a) B

/-- Variable 0 replaced by `t`, whose own variables are those of the binder's
scope: `B[x := t]` without leaving the binder. -/
def under (t : Tm) : Nat → Tm
  | 0 => t
  | n + 1 => var (n + 1)

/-- `P -> Q` for propositions. -/
def imp (P Q : Tm) : Tm := pi 0 0 P (shift Q)

/-- `P * Q` for propositions. -/
def and (P Q : Tm) : Tm := sigma 0 0 P (shift Q)

/-- The sort of `(x : A) -> B`. -/
def imax (l l' : Nat) : Nat := if l' = 0 then 0 else max l l'

/-- What Eq-Univ-Neq compares: the former of a type and its levels. -/
inductive Head where
  | sort (l : Nat)
  | pi (l l' : Nat)
  | sigma (l l' : Nat)
  | quot (l : Nat)
  deriving DecidableEq

/-- The head of a relevant type, when it has one. -/
def head : Tm → Option Head
  | sort l => some (.sort l)
  | pi l l' _ _ => if l' = 0 then none else some (.pi l l')
  | sigma l l' _ _ => if l = 0 ∧ l' = 0 then none else some (.sigma l l')
  | quot l _ _ => some (.quot l)
  | _ => none

/-! ## Applications, telescopes and scope -/

/-- `f` applied to each argument in turn. -/
def apps (f : Tm) (ts : List Tm) : Tm := ts.foldl app f

/-- A telescope entry, written over the terms before it (outermost first),
instantiated with them; a variable past them is a proof. -/
def substL (ts : List Tm) (A : Tm) : Tm := subst (fun i => ts.reverse[i]?.getD irr) A

/-- `c` more variables inserted at index `c'`: `B` moved under `c` new
binders that sit outside its first `c'` variables. -/
def liftAt (c' c : Nat) (n : Nat) : Nat := if n < c' then n else n + c

def lift (c' c : Nat) (t : Tm) : Tm := rename (liftAt c' c) t

/-- Variable `i` occurs in `t`. -/
def occurs : Nat → Tm → Bool
  | i, var j => i == j
  | _, sort _ => false
  | i, pi _ _ A B => occurs i A || occurs (i + 1) B
  | i, lam A t => occurs i A || occurs (i + 1) t
  | i, app f a => occurs i f || occurs i a
  | i, letIn A v t => occurs i A || occurs i v || occurs (i + 1) t
  | i, sigma _ _ A B => occurs i A || occurs (i + 1) B
  | i, pair a b => occurs i a || occurs i b
  | i, fst p => occurs i p
  | i, snd p => occurs i p
  | _, top => false
  | _, tt => false
  | _, bot => false
  | i, absurd A p => occurs i A || occurs i p
  | i, eq _ A a b => occurs i A || occurs i a || occurs i b
  | i, cast A B e t => occurs i A || occurs i B || occurs i e || occurs i t
  | _, irr => false
  | _, glob _ => false
  | i, quot _ A R => occurs i A || occurs i R
  | i, qin A R a => occurs i A || occurs i R || occurs i a
  | i, qelim _ P f q => occurs i P || occurs i f || occurs i q

/-- Every global `t` names belongs to a declaration before `k`. -/
def globsBelow (k : Nat) : Tm → Prop
  | var _ => True
  | sort _ => True
  | pi _ _ A B => globsBelow k A ∧ globsBelow k B
  | lam A t => globsBelow k A ∧ globsBelow k t
  | app f a => globsBelow k f ∧ globsBelow k a
  | letIn A v t => globsBelow k A ∧ globsBelow k v ∧ globsBelow k t
  | sigma _ _ A B => globsBelow k A ∧ globsBelow k B
  | pair a b => globsBelow k a ∧ globsBelow k b
  | fst p => globsBelow k p
  | snd p => globsBelow k p
  | top => True
  | tt => True
  | bot => True
  | absurd A p => globsBelow k A ∧ globsBelow k p
  | eq _ A a b => globsBelow k A ∧ globsBelow k a ∧ globsBelow k b
  | cast A B e t => globsBelow k A ∧ globsBelow k B ∧ globsBelow k e ∧ globsBelow k t
  | irr => True
  | glob g => g.decl < k
  | quot _ A R => globsBelow k A ∧ globsBelow k R
  | qin A R a => globsBelow k A ∧ globsBelow k R ∧ globsBelow k a
  | qelim _ P f q => globsBelow k P ∧ globsBelow k f ∧ globsBelow k q

/-- The inductive type at the head of an application. -/
def dataHead : Tm → Option Nat
  | glob (.data k) => some k
  | app f _ => dataHead f
  | _ => none

end Tm

/-! ## Inductive declarations

As the kernel stores them (`crates/kernel/src/signature.rs`): an indexed
family is forded, its indices are parameters after the uniform ones, and each
constructor states the indices of its result (see Note [Indexed families are
forded]). -/

/-- A constructor field, over the uniform parameters and the fields before it. -/
inductive Field where
  /-- A field of type `A : @Type l`. -/
  | plain (A : Tm) (l : Nat)
  /-- The type being declared, at its own uniform parameters and indices `ys`. -/
  | recur (ys : List Tm)

structure Ctor where
  fields : List Field
  /-- The indices of the result, over the uniform parameters and the fields. -/
  result : List Tm

-- Rule [Data-Ford]: the indices are parameters after the uniform ones, and each
-- constructor states the indices of its result
structure Decl where
  /-- The uniform parameters, then the indices, each with its sort, each over
  the ones before it. -/
  params : List (Tm × Nat)
  uniform : Nat
  /-- The type's sort, `@Type level`. -/
  level : Nat
  ctors : List Ctor

/-- The signature, oldest declaration first; declaration `k` is entry `k`. -/
abbrev Sig := List Decl

open Tm

/-- The variables for the `u` uniform parameters, under `j` binders. -/
def paramVars (u j : Nat) : List Tm := (List.range u).map fun i => var (j + (u - 1 - i))

/-- A field's type, under the `j` fields before it, in declaration `k`. -/
def Field.ty (k u j : Nat) : Field → Tm
  | .plain A _ => A
  | .recur ys => apps (glob (.data k)) (paramVars u j ++ ys)

def Field.isRec : Field → Bool
  | .recur _ => true
  | .plain _ _ => false

def Field.level (level : Nat) : Field → Nat
  | .plain _ l => l
  | .recur _ => level

/-- Fields from number `j` on as a telescope. -/
def fieldsTele (k : Nat) (d : Decl) : Nat → List Field → List (Tm × Nat)
  | _, [] => []
  | j, F :: Fs => (F.ty k d.uniform j, F.level d.level) :: fieldsTele k d (j + 1) Fs

theorem fieldsTele_length (k : Nat) (d : Decl) :
    (j : Nat) → (Fs : List Field) → (fieldsTele k d j Fs).length = Fs.length
  | _, [] => rfl
  | j, _ :: Fs => congrArg (· + 1) (fieldsTele_length k d (j + 1) Fs)

theorem fieldsTele_take (k : Nat) (d : Decl) :
    (i j : Nat) → (Fs : List Field) → (fieldsTele k d i Fs).take j = fieldsTele k d i (Fs.take j)
  | _, _, [] => by simp [fieldsTele]
  | _, 0, _ :: _ => by simp [fieldsTele]
  | i, j + 1, _ :: Fs => by simp [fieldsTele, fieldsTele_take k d (i + 1) j Fs]

/-- The constructor's fields as a telescope. -/
def Ctor.tele (k : Nat) (d : Decl) (C : Ctor) : List (Tm × Nat) := fieldsTele k d 0 C.fields

/-! ## The recursor's telescope

`inductive.rs` builds the recursor's type; here it is the telescope of its
arguments, each entry over the ones before it: the uniform parameters, the
motive, one method per constructor, the indices, and the value. -/

/-- The sort of `pis Δ B` when `B : @Type l`. -/
def pisSort : List (Tm × Nat) → Nat → Nat
  | [], l => l
  | (_, l) :: Δ, l' => imax l (pisSort Δ l')

/-- `(x₁ : A₁) -> ... -> B`, with `B : @Type l`. -/
def pis : List (Tm × Nat) → Tm → Nat → Tm
  | [], B, _ => B
  | (A, l) :: Δ, B, l' => pi l (pisSort Δ l') A (pis Δ B l')

namespace Decl

variable (k : Nat) (d : Decl) (m : Nat)

def indices : List (Tm × Nat) := d.params.drop d.uniform

/-- `(indices) -> D params indices -> @Type m`, over the parameters. -/
def motive : Tm :=
  let ni := d.indices.length
  pis d.indices
    (pi d.level (m + 1) (apps (glob (.data k)) (paramVars d.uniform ni ++ paramVars ni 0)) (sort m))
    (imax d.level (m + 1))

def motiveSort : Nat := pisSort d.indices (imax d.level (m + 1))

/-- The induction hypotheses' binders for the fields from number `f` on,
`h` of them already bound, in a method for constructor number `o - 1`. -/
def ihTele (o nf : Nat) : List Field → Nat → Nat → List (Tm × Nat)
  | [], _, _ => []
  | .plain _ _ :: Fs, f, h => ihTele o nf Fs (f + 1) h
  | .recur ys :: Fs, f, h =>
    (apps (var (h + nf + o - 1))
        (ys.map (fun y => rename (fun n => liftAt f o n + (h + nf - f)) y) ++ [var (h + nf - 1 - f)]),
      m) :: ihTele o nf Fs (f + 1) (h + 1)

/-- A telescope from entry `j` on, under `o` more binders outside it. -/
def liftTele (o : Nat) : Nat → List (Tm × Nat) → List (Tm × Nat)
  | _, [] => []
  | j, (A, l) :: Δ => (lift j o A, l) :: liftTele o (j + 1) Δ

theorem liftTele_length (o : Nat) : (j : Nat) → (Δ : List (Tm × Nat)) → (liftTele o j Δ).length = Δ.length
  | _, [] => rfl
  | j, _ :: Δ => congrArg (· + 1) (liftTele_length o (j + 1) Δ)

/-- The fields' binders in the method for constructor `c`. -/
def fieldTele (c : Nat) (C : Ctor) : List (Tm × Nat) := liftTele (1 + c) 0 (C.tele k d)

/-- How many fields of `C` are recursive. -/
def Ctor.recCount (C : Ctor) : Nat := (C.fields.filter Field.isRec).length

/-- The method for constructor `c`, over the parameters, the motive and the
methods before it: `(fields) -> (P ys f for each recursive field f) -> P
indices (c params fields)`. -/
def methodTele (c : Nat) (C : Ctor) : List (Tm × Nat) :=
  fieldTele k d c C ++ ihTele m (1 + c) C.fields.length C.fields 0 0

def methodResult (c : Nat) (C : Ctor) : Tm :=
  let (o, nf, nr) := (1 + c, C.fields.length, Ctor.recCount C)
  apps (var (nr + nf + o - 1))
    (C.result.map (fun y => rename (fun n => liftAt nf o n + nr) y) ++
      [apps (glob (.ctor k c)) (paramVars d.uniform (nr + nf + o) ++ paramVars nf nr)])

def method (c : Nat) (C : Ctor) : Tm × Nat :=
  (pis (methodTele k d m c C) (methodResult k d c C) m, pisSort (methodTele k d m c C) m)

/-- The recursor's arguments as a telescope. -/
def elimTele : List (Tm × Nat) :=
  let (u, ni, nc) := (d.uniform, d.indices.length, d.ctors.length)
  d.params.take u ++ [(motive k d m, motiveSort d m)] ++ d.ctors.mapIdx (method k d m) ++
    d.indices.mapIdx (fun i A => (lift i (1 + nc) A.1, A.2)) ++
    [(apps (glob (.data k)) (paramVars u (ni + nc + 1) ++ paramVars ni 0), d.level)]

end Decl

/-! ## Terms the computation rules build -/

namespace Decl

variable (k : Nat) (d : Decl) (m : Nat)

/-- The recursor's results on the recursive fields among `Fs`, the first of
them field number `j`: Data-Iota's induction hypotheses. -/
def ihArgs (ps : List Tm) (P : Tm) (ms fs : List Tm) : List Field → Nat → List Tm
  | [], _ => []
  | .plain _ _ :: Fs, j => ihArgs ps P ms fs Fs (j + 1)
  | .recur ys :: Fs, j =>
    apps (glob (.elim k m)) (ps ++ [P] ++ ms ++ ys.map (substL (ps ++ fs.take j)) ++ [fs.getD j irr]) ::
      ihArgs ps P ms fs Fs (j + 1)

/-- Fields `fs` at parameters `ps`, each cast to its type at parameters `ps'`
after the fields already cast (`done`): Cast-Ctor's fields. -/
def castFields (ps ps' : List Tm) : List Tm → List Field → Nat → List Tm → List Tm
  | _, [], _, done => done
  | fs, F :: Fs, j, done =>
    let A := F.ty k d.uniform j
    castFields ps ps' fs Fs (j + 1)
      (done ++ [cast (substL (ps ++ fs.take j) A) (substL (ps' ++ done) A) irr (fs.getD j irr)])

/-- The equalities of the fields that are not proofs: Eq-Ctor. -/
def eqFields (ps fs fs' : List Tm) : List Field → Nat → List Tm
  | [], _ => []
  | F :: Fs, j =>
    let A := F.ty k d.uniform j
    let rest := eqFields ps fs fs' Fs (j + 1)
    if F.level d.level = 0 then rest
    else eq (F.level d.level) (substL (ps ++ fs'.take j) A)
      (cast (substL (ps ++ fs.take j) A) (substL (ps ++ fs'.take j) A) irr (fs.getD j irr))
      (fs'.getD j irr) :: rest

end Decl

/-- `P₁ * (P₂ * ... Pₙ)`, or `@Top`. -/
def andAll : List Tm → Tm
  | [] => .top
  | [P] => P
  | P :: Ps => Tm.and P (andAll Ps)

/-- The context of a constructor's fields, after `Γ`: a recursive field is
bound as `@Top`, since nothing after it may use it. -/
def fieldCtx : List Tm → List Field → List Tm
  | Γ, [] => Γ
  | Γ, .plain A _ :: Fs => fieldCtx (A :: Γ) Fs
  | Γ, .recur _ :: Fs => fieldCtx (.top :: Γ) Fs

/-- The terms at field position `j` (a field's type or its indices), or the
result's indices at the end. -/
def Ctor.termsAt (C : Ctor) (j : Nat) : List Tm :=
  match C.fields[j]? with
  | some (.plain A _) => [A]
  | some (.recur ys) => ys
  | none => C.result

/-- No term after a recursive field mentions it. -/
def Ctor.recursiveUnused (C : Ctor) : Prop :=
  ∀ j ys, C.fields[j]? = some (.recur ys) → ∀ j', j < j' → j' ≤ C.fields.length →
    ∀ t ∈ C.termsAt j', t.occurs (j' - 1 - j) = false

/-- Every term the declaration stores names only globals before `k`. -/
def Decl.scoped (k : Nat) (d : Decl) : Prop :=
  (∀ p ∈ d.params, p.1.globsBelow k) ∧
  ∀ C ∈ d.ctors, (∀ t ∈ C.result, t.globsBelow k) ∧
    ∀ F ∈ C.fields, match F with
      | .plain A _ => A.globsBelow k
      | .recur ys => ∀ y ∈ ys, y.globsBelow k

/-! ## Quotients

The types the kernel checks a quotient's arguments against
(`check.rs`, `infer_quotient` and `respect_type`). -/

namespace Quot

open Tm

/-- `t` under `n` more binders. -/
def shiftN : Nat → Tm → Tm
  | 0, t => t
  | n + 1, t => shift (shiftN n t)

/-- `R x y`, with `R` under `n` binders and `x`, `y` variables. -/
def rel (n : Nat) (R : Tm) (x y : Nat) : Tm := app (app (shiftN n R) (var x)) (var y)

/-- `A -> A -> @Type 0`. -/
def relTy (l : Nat) (A : Tm) : Tm := pi l l A (pi l 1 (shift A) (sort 0))

/-- `(x: A) -> R x x`. -/
def reflTy (l : Nat) (A R : Tm) : Tm := pi l 0 A (rel 1 R 0 0)

/-- `(x y: A) -> R x y -> R y x`. -/
def symTy (l : Nat) (A R : Tm) : Tm :=
  pi l 0 A (pi l 0 (shift A) (pi 0 0 (rel 2 R 1 0) (rel 3 R 1 2)))

/-- `(x y z: A) -> R x y -> R y z -> R x z`. -/
def transTy (l : Nat) (A R : Tm) : Tm :=
  pi l 0 A (pi l 0 (shift A) (pi l 0 (shiftN 2 A)
    (pi 0 0 (rel 3 R 2 1) (pi 0 0 (rel 4 R 2 1) (rel 5 R 4 2)))))

/-- `(x y: A) -> R x y -> @cast (P (@qin x)) (P (@qin y)) _ (f x) ~ f y`. -/
def respTy (l m : Nat) (A R P f : Tm) : Tm :=
  let (A₃, R₃, P₃, f₃) := (shiftN 3 A, shiftN 3 R, shiftN 3 P, shiftN 3 f)
  pi l 0 A (pi l 0 (shift A) (pi 0 0 (rel 2 R 1 0)
    (eq m (app P₃ (qin A₃ R₃ (var 1)))
      (cast (app P₃ (qin A₃ R₃ (var 2))) (app P₃ (qin A₃ R₃ (var 1))) irr (app f₃ (var 2)))
      (app f₃ (var 1)))))

end Quot
