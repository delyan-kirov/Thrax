import Proofs.Syntax

/-!
# Typing

The kernel's rules as one inductive judgment, for universes, Pi, Sigma,
`let`, propositions (`@Top`, `@Bottom`), equality, transport, cast (#494,
#495), inductive types (#496) and quotients (#497), over a signature of
declarations.

Each constructor carries the `Rule [X]` tag of the row of
`documentation/type-theory/LEDGER.md` it states, the convention the Rust kernel
uses. Where the kernel computes (equality and cast reduce by the heads of
their types), the rule here is a conversion `≡`; where it decides a level
question, the premise is about numbers.

`N` bounds the universes: `@Type l` is a term only when `l + 1 ≤ N`. The
consistency theorem is for each `N` (see `Proofs/Model.lean`).
-/

open Tm

/-- The judgment forms. -/
inductive Jdg where
  /-- `Γ` is a context. -/
  | wf (Γ : List Tm)
  /-- `Γ ⊢ t : A`. -/
  | ty (Γ : List Tm) (t A : Tm)
  /-- `Γ ⊢ t ≡ u : A`. -/
  | conv (Γ : List Tm) (t u A : Tm)
  /-- `ts` fit the telescope `Δ`, whose entries are over `pre` and the terms
  before them. -/
  | args (Γ : List Tm) (pre ts : List Tm) (Δ : List (Tm × Nat))

open Jdg

inductive Deriv (N : Nat) (sig : Sig) : Jdg → Prop where
  -- Contexts
  | wfNil : Deriv N sig (wf [])
  | wfCons {Γ A l} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (wf (A :: Γ))

  -- Typing
  | var {Γ i A} : Deriv N sig (wf Γ) → Γ[i]? = some A →
      Deriv N sig (ty Γ (var i) (rename (· + (i + 1)) A))
  -- Rule [Univ]
  | univ {Γ l} : Deriv N sig (wf Γ) → l + 1 ≤ N → Deriv N sig (ty Γ (sort l) (sort (l + 1)))
  -- Rule [Pi-Form]
  | piForm {Γ A B l l'} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ (pi l l' A B) (sort (imax l l')))
  -- Rule [Fun]
  | lam {Γ A B t l l'} : l' ≠ 0 → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) B (sort l')) → Deriv N sig (ty (A :: Γ) t B) →
      Deriv N sig (ty Γ (lam A t) (pi l l' A B))
  -- Rule [Fun]
  -- Rule [Proof-Witness]: a function into a proposition is a proof.
  | lamProof {Γ A B t l} : Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) B (sort 0)) → Deriv N sig (ty (A :: Γ) t B) →
      Deriv N sig (ty Γ irr (pi l 0 A B))
  -- Rule [App]
  | app {Γ f a A B l l'} : Deriv N sig (ty Γ f (pi l l' A B)) → Deriv N sig (ty Γ a A) →
      Deriv N sig (ty Γ (app f a) (inst B a))
  -- Rule [Sigma-Form]: a proposition, `&`, when both sides are
  | sigmaForm {Γ A B l l'} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ (sigma l l' A B) (sort (max l l')))
  -- Rule [Sigma-Pair]
  | pair {Γ A B a b l l'} : ¬(l = 0 ∧ l' = 0) → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) B (sort l')) → Deriv N sig (ty Γ a A) → Deriv N sig (ty Γ b (inst B a)) →
      Deriv N sig (ty Γ (pair a b) (sigma l l' A B))
  -- Rule [Sigma-Pair], for propositions
  | andIntro {Γ A B a b} : Deriv N sig (ty Γ A (sort 0)) → Deriv N sig (ty (A :: Γ) B (sort 0)) →
      Deriv N sig (ty Γ a A) → Deriv N sig (ty Γ b (inst B a)) → Deriv N sig (ty Γ irr (sigma 0 0 A B))
  -- Rule [Sigma-Pair], the projections
  | fst {Γ p A B l l'} : Deriv N sig (ty Γ p (sigma l l' A B)) → Deriv N sig (ty Γ (fst p) A)
  | snd {Γ p A B l l'} : Deriv N sig (ty Γ p (sigma l l' A B)) →
      Deriv N sig (ty Γ (snd p) (inst B (fst p)))
  -- Rule [Let]: the variable is transparent, so the body is checked with the
  -- value in its place
  | letIn {Γ A v t C l} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ v A) →
      Deriv N sig (ty Γ (inst t v) C) → Deriv N sig (ty Γ (letIn A v t) C)
  -- Rule [Top]
  | top {Γ} : Deriv N sig (wf Γ) → Deriv N sig (ty Γ top (sort 0))
  | tt {Γ} : Deriv N sig (wf Γ) → Deriv N sig (ty Γ tt top)
  -- Rule [Bottom]
  | bot {Γ} : Deriv N sig (wf Γ) → Deriv N sig (ty Γ bot (sort 0))
  | absurd {Γ A p l} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ p bot) →
      Deriv N sig (ty Γ (absurd A p) A)
  -- Rule [Eq-Form]: only at a type that is not a proposition
  | eqForm {Γ A a b l} : l ≠ 0 → Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ a A) →
      Deriv N sig (ty Γ b A) → Deriv N sig (ty Γ (eq l A a b) (sort 0))
  -- Rule [Refl]
  | refl {Γ A a l} : l ≠ 0 → Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ a A) →
      Deriv N sig (ty Γ irr (eq l A a a))
  -- Rule [Transport]: with the non-dependent motive the kernel takes
  | transport {Γ A t P p u e l} : l ≠ 0 → Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ t A) →
      Deriv N sig (ty Γ P (pi l 1 A (sort 0))) → Deriv N sig (ty Γ p (app P t)) →
      Deriv N sig (ty Γ u A) → Deriv N sig (ty Γ e (eq l A t u)) → Deriv N sig (ty Γ irr (app P u))
  -- Rule [Cast]
  | cast {Γ A B e t l} : Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty Γ B (sort l)) →
      Deriv N sig (ty Γ e (eq (l + 1) (sort l) A B)) → Deriv N sig (ty Γ t A) →
      Deriv N sig (ty Γ (cast A B e t) B)
  -- Rule [Proof-Witness]
  | proof {Γ P t} : Deriv N sig (ty Γ P (sort 0)) → Deriv N sig (ty Γ t P) → Deriv N sig (ty Γ irr P)
  | conv {Γ t A B l} : Deriv N sig (ty Γ t A) → Deriv N sig (conv Γ A B (sort l)) → Deriv N sig (ty Γ t B)

  -- Telescopes
  | argsNil {Γ pre} : Deriv N sig (wf Γ) → Deriv N sig (args Γ pre [] [])
  | argsCons {Γ pre t ts A Δ} {l : Nat} : Deriv N sig (ty Γ (substL pre A) (sort l)) →
      Deriv N sig (ty Γ t (substL pre A)) → Deriv N sig (args Γ (pre ++ [t]) ts Δ) →
      Deriv N sig (args Γ pre (t :: ts) ((A, l) :: Δ))

  -- Inductive types, applied to all their arguments (a partial application
  -- is its eta-expansion, as the kernel's `curry` makes it)
  -- Rule [Data-Form]
  | dataForm {Γ as} {k : Nat} {d : Decl} : sig[k]? = some d → Deriv N sig (args Γ [] as d.params) →
      Deriv N sig (ty Γ (apps (glob (.data k)) as) (sort d.level))
  -- Rule [Data-Ctor]
  | ctor {Γ ps fs} {k c : Nat} {d : Decl} {C : Ctor} : sig[k]? = some d → d.ctors[c]? = some C →
      Deriv N sig (args Γ [] ps (d.params.take d.uniform)) →
      Deriv N sig (args Γ ps fs (C.tele k d)) →
      Deriv N sig (ty Γ (apps (glob (.ctor k c)) (ps ++ fs))
        (apps (glob (.data k)) (ps ++ C.result.map (substL (ps ++ fs)))))
  -- Rule [Data-Elim]
  | elim {Γ ps P ms xs x} {k m : Nat} {d : Decl} : sig[k]? = some d → ps.length = d.uniform →
      ms.length = d.ctors.length →
      Deriv N sig (args Γ [] (ps ++ [P] ++ ms ++ xs ++ [x]) (d.elimTele k m)) →
      Deriv N sig (ty Γ (apps (glob (.elim k m)) (ps ++ [P] ++ ms ++ xs ++ [x])) (apps P (xs ++ [x])))

  -- Quotients. A class carries its quotient's carrier and relation, as
  -- Lean's `Quot.mk` does: the kernel's `@qin a` is checked against a known
  -- quotient type, whose carrier and relation these are.
  -- Rule [Quot-Form]
  | quotForm {Γ A R} {l : Nat} : l ≠ 0 → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ R (Quot.relTy l A)) → Deriv N sig (ty Γ irr (Quot.reflTy l A R)) →
      Deriv N sig (ty Γ irr (Quot.symTy l A R)) → Deriv N sig (ty Γ irr (Quot.transTy l A R)) →
      Deriv N sig (ty Γ (quot l A R) (sort l))
  -- Rule [Quot-Intro]
  | qin {Γ A R a} {l : Nat} : Deriv N sig (ty Γ (quot l A R) (sort l)) → Deriv N sig (ty Γ a A) →
      Deriv N sig (ty Γ (qin A R a) (quot l A R))
  -- Rule [Quot-Elim], with the quotient's premises restated: its classes are
  -- classes of an equivalence
  | qelim {Γ A R P f q} {l m : Nat} : l ≠ 0 → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ R (Quot.relTy l A)) → Deriv N sig (ty Γ irr (Quot.reflTy l A R)) →
      Deriv N sig (ty Γ irr (Quot.symTy l A R)) → Deriv N sig (ty Γ irr (Quot.transTy l A R)) →
      Deriv N sig (ty Γ P (pi l (m + 1) (quot l A R) (sort m))) →
      Deriv N sig (ty Γ f (pi l m A (app (shift P) (qin (shift A) (shift R) (var 0))))) →
      Deriv N sig (ty Γ irr (Quot.respTy l m A R P f)) → Deriv N sig (ty Γ q (quot l A R)) →
      Deriv N sig (ty Γ (qelim m P f q) (app P q))

  -- Conversion: an equivalence
  | convRefl {Γ t A} : Deriv N sig (ty Γ t A) → Deriv N sig (conv Γ t t A)
  | convSymm {Γ t u A} : Deriv N sig (conv Γ t u A) → Deriv N sig (conv Γ u t A)
  | convTrans {Γ t u v A} : Deriv N sig (conv Γ t u A) → Deriv N sig (conv Γ u v A) →
      Deriv N sig (conv Γ t v A)
  | convType {Γ t u A B l} : Deriv N sig (conv Γ t u A) → Deriv N sig (conv Γ A B (sort l)) →
      Deriv N sig (conv Γ t u B)

  -- Conversion: congruence
  | piCong {Γ A A' B B' l l'} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (conv (A :: Γ) B B' (sort l')) →
      Deriv N sig (conv Γ (pi l l' A B) (pi l l' A' B') (sort (imax l l')))
  | lamCong {Γ A A' B t t' l l'} : l' ≠ 0 → Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (conv (A :: Γ) t t' B) →
      Deriv N sig (conv Γ (lam A t) (lam A' t') (pi l l' A B))
  | appCong {Γ f f' a a' A B l l'} : Deriv N sig (conv Γ f f' (pi l l' A B)) →
      Deriv N sig (conv Γ a a' A) → Deriv N sig (conv Γ (app f a) (app f' a') (inst B a))
  | sigmaCong {Γ A A' B B' l l'} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (conv (A :: Γ) B B' (sort l')) →
      Deriv N sig (conv Γ (sigma l l' A B) (sigma l l' A' B') (sort (max l l')))
  | pairCong {Γ a a' b b' A B l l'} : Deriv N sig (conv Γ a a' A) →
      Deriv N sig (conv Γ b b' (inst B a)) → Deriv N sig (conv Γ (pair a b) (pair a' b') (sigma l l' A B))
  | fstCong {Γ p p' A B l l'} : Deriv N sig (conv Γ p p' (sigma l l' A B)) →
      Deriv N sig (conv Γ (fst p) (fst p') A)
  | sndCong {Γ p p' A B l l'} : Deriv N sig (conv Γ p p' (sigma l l' A B)) →
      Deriv N sig (conv Γ (snd p) (snd p') (inst B (fst p)))
  | letCong {Γ A A' v v' t t' B l} : Deriv N sig (conv Γ A A' (sort l)) → Deriv N sig (ty Γ v A) →
      Deriv N sig (conv Γ v v' A) → Deriv N sig (conv (A :: Γ) t t' B) →
      Deriv N sig (conv Γ (letIn A v t) (letIn A' v' t') (inst B v))
  | absurdCong {Γ A A' p p' l} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (conv Γ p p' bot) → Deriv N sig (conv Γ (absurd A p) (absurd A' p') A)
  | eqCong {Γ A A' a a' b b' l} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (conv Γ a a' A) → Deriv N sig (conv Γ b b' A) →
      Deriv N sig (conv Γ (eq l A a b) (eq l A' a' b') (sort 0))
  | quotCong {Γ A A' R R'} {l : Nat} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (conv Γ R R' (Quot.relTy l A)) →
      Deriv N sig (conv Γ (quot l A R) (quot l A' R') (sort l))
  | qinCong {Γ A A' R R' a a'} {l : Nat} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (conv Γ R R' (Quot.relTy l A)) → Deriv N sig (conv Γ a a' A) →
      Deriv N sig (conv Γ (qin A R a) (qin A' R' a') (quot l A R))
  | qelimCong {Γ P P' f f' q q' F Q} {m : Nat} : Deriv N sig (conv Γ f f' F) →
      Deriv N sig (conv Γ q q' Q) →
      Deriv N sig (conv Γ (qelim m P f q) (qelim m P' f' q') (app P q))
  | castCong {Γ A A' B B' e e' t t' l} : Deriv N sig (conv Γ A A' (sort l)) →
      Deriv N sig (conv Γ B B' (sort l)) → Deriv N sig (conv Γ e e' (eq (l + 1) (sort l) A B)) →
      Deriv N sig (conv Γ t t' A) → Deriv N sig (conv Γ (cast A B e t) (cast A' B' e' t') B)

  -- Conversion: computation
  -- Rule [Beta]
  | beta {Γ A B t a l l'} : l' ≠ 0 → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) t B) → Deriv N sig (ty Γ a A) →
      Deriv N sig (conv Γ (app (lam A t) a) (inst t a) (inst B a))
  -- Rule [Eta-Pi]
  | etaPi {Γ f A B l l'} : l' ≠ 0 → Deriv N sig (ty Γ f (pi l l' A B)) →
      Deriv N sig (conv Γ f (lam A (app (shift f) (var 0))) (pi l l' A B))
  -- Rule [Sigma-Pair]: the projections of a pair
  | fstPair {Γ a b A B l l'} : Deriv N sig (ty Γ (pair a b) (sigma l l' A B)) →
      Deriv N sig (conv Γ (fst (pair a b)) a A)
  | sndPair {Γ a b A B l l'} : Deriv N sig (ty Γ (pair a b) (sigma l l' A B)) →
      Deriv N sig (conv Γ (snd (pair a b)) b (inst B a))
  -- Rule [Eta-Sigma]
  | etaSigma {Γ p q A B l l'} : ¬(l = 0 ∧ l' = 0) → Deriv N sig (ty Γ p (sigma l l' A B)) →
      Deriv N sig (ty Γ q (sigma l l' A B)) → Deriv N sig (conv Γ (fst p) (fst q) A) →
      Deriv N sig (conv Γ (snd p) (snd q) (inst B (fst p))) → Deriv N sig (conv Γ p q (sigma l l' A B))
  -- Rule [Let]
  | zeta {Γ A v t C} : Deriv N sig (ty Γ (letIn A v t) C) → Deriv N sig (conv Γ (letIn A v t) (inst t v) C)
  -- Rule [Proof-Irrelevance]
  | proofIrrelevance {Γ P p q} : Deriv N sig (ty Γ P (sort 0)) → Deriv N sig (ty Γ p P) →
      Deriv N sig (ty Γ q P) → Deriv N sig (conv Γ p q P)
  -- Rule [Eq-Fun]
  | eqFun {Γ A B f g l l'} : l' ≠ 0 → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ f (pi l l' A B)) → Deriv N sig (ty Γ g (pi l l' A B)) →
      Deriv N sig (conv Γ (eq (max l l') (pi l l' A B) f g)
        (pi l 0 A (eq l' B (app (shift f) (var 0)) (app (shift g) (var 0)))) (sort 0))
  -- Rule [Eq-Pair]
  | eqPair {Γ A B p q l l'} : ¬(l = 0 ∧ l' = 0) → Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty (A :: Γ) B (sort l')) → Deriv N sig (ty Γ p (sigma l l' A B)) →
      Deriv N sig (ty Γ q (sigma l l' A B)) →
      Deriv N sig (conv Γ (eq (max l l') (sigma l l' A B) p q)
        (and (eq l A (fst p) (fst q))
          (eq l' (inst B (fst q)) (cast (inst B (fst p)) (inst B (fst q)) irr (snd p)) (snd q)))
        (sort 0))
  -- Rule [Eq-Box]: the equality of two proofs, as Eq-Pair forms it
  | eqBox {Γ P a b} : Deriv N sig (ty Γ P (sort 0)) → Deriv N sig (ty Γ a P) → Deriv N sig (ty Γ b P) →
      Deriv N sig (conv Γ (eq 0 P a b) top (sort 0))
  -- Rule [Eq-Omega]
  | eqOmega {Γ P Q} : Deriv N sig (ty Γ P (sort 0)) → Deriv N sig (ty Γ Q (sort 0)) →
      Deriv N sig (conv Γ (eq 1 (sort 0) P Q) (and (imp P Q) (imp Q P)) (sort 0))
  -- Rule [Eq-Univ]
  | eqUniv {Γ j k} : Deriv N sig (ty Γ (sort j) (sort k)) →
      Deriv N sig (conv Γ (eq (k + 1) (sort k) (sort j) (sort j)) top (sort 0))
  -- Rule [Eq-Pi]
  | eqPi {Γ A A' B B' l l' k} : k ≠ 0 → l' ≠ 0 →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ A' (sort l)) → Deriv N sig (ty (A' :: Γ) B' (sort l')) →
      Deriv N sig (conv Γ (eq (k + 1) (sort k) (pi l l' A B) (pi l l' A' B'))
        (and (eq (l + 1) (sort l) A' A)
          (pi l 0 A' (eq (l' + 1) (sort l')
            (subst (under (cast (shift A') (shift A) irr (var 0))) B) B')))
        (sort 0))
  -- Rule [Eq-Sigma]
  | eqSigma {Γ A A' B B' l l' k} : k ≠ 0 → ¬(l = 0 ∧ l' = 0) →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ A' (sort l)) → Deriv N sig (ty (A' :: Γ) B' (sort l')) →
      Deriv N sig (conv Γ (eq (k + 1) (sort k) (sigma l l' A B) (sigma l l' A' B'))
        (and (eq (l + 1) (sort l) A A')
          (pi l 0 A (eq (l' + 1) (sort l') B
            (subst (under (cast (shift A) (shift A') irr (var 0))) B'))))
        (sort 0))
  -- Rule [Eq-Univ-Neq]
  | eqUnivNeq {Γ X Y k hx hy} : k ≠ 0 → Deriv N sig (ty Γ X (sort k)) → Deriv N sig (ty Γ Y (sort k)) →
      head X = some hx → head Y = some hy → hx ≠ hy →
      Deriv N sig (conv Γ (eq (k + 1) (sort k) X Y) bot (sort 0))
  -- Rule [Cast-Pi]
  | castPi {Γ A A' B B' e f l l' m m' k} : l' ≠ 0 → m' ≠ 0 →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ A' (sort m)) → Deriv N sig (ty (A' :: Γ) B' (sort m')) →
      Deriv N sig (ty Γ e (eq (k + 1) (sort k) (pi l l' A B) (pi m m' A' B'))) →
      Deriv N sig (ty Γ f (pi l l' A B)) →
      Deriv N sig (conv Γ (cast (pi l l' A B) (pi m m' A' B') e f)
        (lam A' (cast (subst (under (cast (shift A') (shift A) irr (var 0))) B) B' irr
          (app (shift f) (cast (shift A') (shift A) irr (var 0)))))
        (pi m m' A' B'))
  -- Rule [Cast-Sigma]
  | castSigma {Γ A A' B B' e p l l' m m' k} : ¬(l = 0 ∧ l' = 0) → ¬(m = 0 ∧ m' = 0) →
      Deriv N sig (ty Γ A (sort l)) → Deriv N sig (ty (A :: Γ) B (sort l')) →
      Deriv N sig (ty Γ A' (sort m)) → Deriv N sig (ty (A' :: Γ) B' (sort m')) →
      Deriv N sig (ty Γ e (eq (k + 1) (sort k) (sigma l l' A B) (sigma m m' A' B'))) →
      Deriv N sig (ty Γ p (sigma l l' A B)) →
      Deriv N sig (conv Γ (cast (sigma l l' A B) (sigma m m' A' B') e p)
        (pair (cast A A' irr (fst p))
          (cast (inst B (fst p)) (inst B' (cast A A' irr (fst p))) irr (snd p)))
        (sigma m m' A' B'))
  -- Rule [Data-Iota]: for an indexed family, the method's result is cast from
  -- the constructor's own indices to the value's
  | iota {Γ ps P ms xs fs} {k m c : Nat} {d : Decl} {C : Ctor} : sig[k]? = some d → d.ctors[c]? = some C →
      ps.length = d.uniform → ms.length = d.ctors.length →
      Deriv N sig (args Γ ps fs (C.tele k d)) →
      Deriv N sig (args Γ [] (ps ++ [P] ++ ms ++ xs ++ [apps (glob (.ctor k c)) (ps ++ fs)])
        (d.elimTele k m)) →
      Deriv N sig (conv Γ
        (apps (glob (.elim k m)) (ps ++ [P] ++ ms ++ xs ++ [apps (glob (.ctor k c)) (ps ++ fs)]))
        (Tm.cast (apps P (C.result.map (substL (ps ++ fs)) ++ [apps (glob (.ctor k c)) (ps ++ fs)]))
          (apps P (xs ++ [apps (glob (.ctor k c)) (ps ++ fs)])) irr
          (apps (ms.getD c irr) (fs ++ Decl.ihArgs k m ps P ms fs C.fields 0)))
        (apps P (xs ++ [apps (glob (.ctor k c)) (ps ++ fs)])))
  -- Rule [Eq-Ctor]: different constructors
  | eqCtorNeq {Γ ps fs ps' fs' as} {k c c' : Nat} {d : Decl} {C C' : Ctor} : sig[k]? = some d → c ≠ c' →
      d.ctors[c]? = some C → d.ctors[c']? = some C' →
      Deriv N sig (args Γ [] ps (d.params.take d.uniform)) → Deriv N sig (args Γ ps fs (C.tele k d)) →
      Deriv N sig (args Γ [] ps' (d.params.take d.uniform)) →
      Deriv N sig (args Γ ps' fs' (C'.tele k d)) →
      Deriv N sig (conv Γ (eq d.level (apps (glob (.data k)) as) (apps (glob (.ctor k c)) (ps ++ fs))
        (apps (glob (.ctor k c')) (ps' ++ fs'))) bot (sort 0))
  -- Rule [Eq-Ctor]: the same constructor
  | eqCtor {Γ ps fs ps' fs' as} {k c : Nat} {d : Decl} {C : Ctor} : sig[k]? = some d → d.ctors[c]? = some C →
      Deriv N sig (args Γ [] ps (d.params.take d.uniform)) → Deriv N sig (args Γ ps fs (C.tele k d)) →
      Deriv N sig (args Γ [] ps' (d.params.take d.uniform)) →
      Deriv N sig (args Γ ps' fs' (C.tele k d)) →
      Deriv N sig (conv Γ (eq d.level (apps (glob (.data k)) as) (apps (glob (.ctor k c)) (ps ++ fs))
        (apps (glob (.ctor k c)) (ps' ++ fs'))) (andAll (d.eqFields k ps fs fs' C.fields 0)) (sort 0))
  -- Rule [Eq-Univ-Data]
  | eqUnivData {Γ} {k : Nat} {d : Decl} : sig[k]? = some d → d.params = [] → Deriv N sig (wf Γ) →
      Deriv N sig (conv Γ (eq (d.level + 1) (sort d.level) (glob (.data k)) (glob (.data k))) top
        (sort 0))
  -- Rule [Eq-Univ-Neq]: two inductive types
  | eqDataNeq {Γ as as'} {k k' l : Nat} {d d' : Decl} : sig[k]? = some d → sig[k']? = some d' → k ≠ k' →
      Deriv N sig (args Γ [] as d.params) → Deriv N sig (args Γ [] as' d'.params) →
      Deriv N sig (conv Γ (eq (l + 1) (sort l) (apps (glob (.data k)) as) (apps (glob (.data k')) as'))
        bot (sort 0))
  -- Rule [Eq-Univ-Neq]: an inductive type and another former, either way round
  | eqDataHead {Γ as Y h} {k l : Nat} {d : Decl} : sig[k]? = some d → Deriv N sig (args Γ [] as d.params) →
      head Y = some h →
      Deriv N sig (conv Γ (eq (l + 1) (sort l) (apps (glob (.data k)) as) Y) bot (sort 0))
  | eqHeadData {Γ as Y h} {k l : Nat} {d : Decl} : sig[k]? = some d → Deriv N sig (args Γ [] as d.params) →
      head Y = some h →
      Deriv N sig (conv Γ (eq (l + 1) (sort l) Y (apps (glob (.data k)) as)) bot (sort 0))
  -- Rule [Cast-Ctor]: into a constructor at the target's parameters, cast
  -- again from its own indices to the target's
  | castCtor {Γ ps fs as as' e} {k c : Nat} {d : Decl} {C : Ctor} : sig[k]? = some d → d.ctors[c]? = some C →
      Deriv N sig (args Γ [] ps (d.params.take d.uniform)) → Deriv N sig (args Γ ps fs (C.tele k d)) →
      Deriv N sig (args Γ [] as' d.params) →
      Deriv N sig (ty Γ e (eq (d.level + 1) (sort d.level) (apps (glob (.data k)) as)
        (apps (glob (.data k)) as'))) →
      Deriv N sig (conv Γ
        (Tm.cast (apps (glob (.data k)) as) (apps (glob (.data k)) as') e
          (apps (glob (.ctor k c)) (ps ++ fs)))
        (Tm.cast (apps (glob (.data k)) (as'.take d.uniform ++ C.result.map
            (substL (as'.take d.uniform ++ d.castFields k ps (as'.take d.uniform) fs C.fields 0 []))))
          (apps (glob (.data k)) as') irr
          (apps (glob (.ctor k c))
            (as'.take d.uniform ++ d.castFields k ps (as'.take d.uniform) fs C.fields 0 [])))
        (apps (glob (.data k)) as'))
  -- Rule [Eq-Data-Stuck]: no rule reduces `D as ~ D as'`; the projections below
  -- are its only use
  -- Rule [Data-Eq-Projection]: `D as ~ D as'` makes field `j`'s types equal,
  -- after the fields before it, cast (which must fit at `as'`, for the
  -- statement to be well typed)
  | dataProj {Γ as as' e bs} {k c j : Nat} {d : Decl} {C : Ctor} {F : Field} : sig[k]? = some d → d.ctors[c]? = some C →
      C.fields[j]? = some F →
      Deriv N sig (args Γ [] as d.params) → Deriv N sig (args Γ [] as' d.params) →
      Deriv N sig (ty Γ e (eq (d.level + 1) (sort d.level) (apps (glob (.data k)) as)
        (apps (glob (.data k)) as'))) →
      Deriv N sig (args Γ (as.take d.uniform) bs ((C.tele k d).take j)) →
      Deriv N sig (args Γ (as'.take d.uniform)
        (d.castFields k (as.take d.uniform) (as'.take d.uniform) bs (C.fields.take j) 0 [])
        ((C.tele k d).take j)) →
      Deriv N sig (ty Γ irr (eq (F.level d.level + 1) (sort (F.level d.level))
        (substL (as.take d.uniform ++ bs) (F.ty k d.uniform j))
        (substL (as'.take d.uniform ++ d.castFields k (as.take d.uniform) (as'.take d.uniform) bs
          (C.fields.take j) 0 []) (F.ty k d.uniform j))))
  -- Rule [Quot-Beta]
  | quotBeta {Γ A R P f a} {l m : Nat} : Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ R (Quot.relTy l A)) → Deriv N sig (ty Γ irr (Quot.reflTy l A R)) →
      Deriv N sig (ty Γ f (pi l m A (app (shift P) (qin (shift A) (shift R) (var 0))))) →
      Deriv N sig (ty Γ irr (Quot.respTy l m A R P f)) → Deriv N sig (ty Γ a A) →
      Deriv N sig (conv Γ (qelim m P f (qin A R a)) (app f a) (app P (qin A R a)))
  -- Rule [Eq-Quot-Class]: equality of classes is the relation
  | eqQuotClass {Γ A R a b} {l : Nat} : Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ R (Quot.relTy l A)) → Deriv N sig (ty Γ irr (Quot.reflTy l A R)) →
      Deriv N sig (ty Γ irr (Quot.symTy l A R)) → Deriv N sig (ty Γ irr (Quot.transTy l A R)) →
      Deriv N sig (ty Γ a A) → Deriv N sig (ty Γ b A) →
      Deriv N sig (conv Γ (eq l (quot l A R) (qin A R a) (qin A R b)) (app (app R a) b) (sort 0))
  -- Rule [Eq-Quot]: the carriers are equal, and the relations, along the cast
  | eqQuot {Γ A R A' R'} {l : Nat} : Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ R (Quot.relTy l A)) → Deriv N sig (ty Γ A' (sort l)) →
      Deriv N sig (ty Γ R' (Quot.relTy l A')) →
      Deriv N sig (conv Γ (eq (l + 1) (sort l) (quot l A R) (quot l A' R'))
        (and (eq (l + 1) (sort l) A A')
          (pi l 0 A (pi l 0 (shift A) (eq 1 (sort 0) (Quot.rel 2 R 1 0)
            (app (app (Quot.shiftN 2 R') (cast (Quot.shiftN 2 A) (Quot.shiftN 2 A') irr (var 1)))
              (cast (Quot.shiftN 2 A) (Quot.shiftN 2 A') irr (var 0)))))))
        (sort 0))
  -- Rule [Cast-Quot]: through the representative
  | castQuot {Γ A R A' R' e a} {l : Nat} :
      Deriv N sig (ty Γ e (eq (l + 1) (sort l) (quot l A R) (quot l A' R'))) → Deriv N sig (ty Γ a A) →
      Deriv N sig (conv Γ (cast (quot l A R) (quot l A' R') e (qin A R a))
        (qin A' R' (cast A A' irr a)) (quot l A' R'))
  -- Rule [Cast-Univ]
  | castUniv {Γ j e X l} : Deriv N sig (ty Γ (sort j) (sort l)) →
      Deriv N sig (ty Γ e (eq (l + 1) (sort l) (sort j) (sort j))) → Deriv N sig (ty Γ X (sort j)) →
      Deriv N sig (conv Γ (cast (sort j) (sort j) e X) X (sort j))
  -- Rule [Cast-Refl]
  | castRefl {Γ A e t l} : Deriv N sig (ty Γ A (sort l)) →
      Deriv N sig (ty Γ e (eq (l + 1) (sort l) A A)) → Deriv N sig (ty Γ t A) →
      Deriv N sig (conv Γ (cast A A e t) t A)

/-! ## Declarations

The premises of a declaration, over any reading `J` of the judgments: `J` is
derivability for the rules, and validity in the model for soundness. -/

/-- The telescope `Δ` is well formed after `Γ`. -/
def TeleOk (J : Jdg → Prop) : List Tm → List (Tm × Nat) → Prop
  | Γ, [] => J (.wf Γ)
  | Γ, (A, l) :: Δ => J (.ty Γ A (.sort l)) ∧ TeleOk J (A :: Γ) Δ

/-- The fields from number `j` on are well formed in context `Γ`, which holds
the uniform parameters and the fields before them. -/
def FieldsTyped (J : Jdg → Prop) (d : Decl) : List Tm → Nat → List Field → Prop
  | _, _, [] => True
  | Γ, j, .plain A l :: Fs => J (.ty Γ A (.sort l)) ∧ l ≤ d.level ∧ FieldsTyped J d (A :: Γ) (j + 1) Fs
  | Γ, j, .recur ys :: Fs =>
    J (.args Γ (paramVars d.uniform j) ys d.indices) ∧ FieldsTyped J d (.top :: Γ) (j + 1) Fs

/-- The parameters' context. -/
def Decl.paramCtx (d : Decl) : List Tm := ((d.params.take d.uniform).map Prod.fst).reverse

-- Rule [Data-Form]
-- Rule [Positivity-Restricted]: a field mentions the type only as a whole
-- recursive field, since its other terms name only earlier globals
/-- Declaration `d` is well formed as declaration number `k`. -/
structure DeclWith (J : Jdg → Prop) (k : Nat) (d : Decl) : Prop where
  level_pos : 0 < d.level
  uniform_le : d.uniform ≤ d.params.length
  params : TeleOk J [] d.params
  fields : ∀ C ∈ d.ctors, FieldsTyped J d d.paramCtx 0 C.fields
  result : ∀ C ∈ d.ctors, J (.args (fieldCtx d.paramCtx C.fields)
    (paramVars d.uniform C.fields.length) C.result d.indices)
  unused : ∀ C ∈ d.ctors, C.recursiveUnused
  inScope : d.scoped k

theorem TeleOk.map {J J' : Jdg → Prop} (h : ∀ j, J j → J' j) :
    {Γ : List Tm} → {Δ : List (Tm × Nat)} → TeleOk J Γ Δ → TeleOk J' Γ Δ
  | _, [], t => h _ t
  | _, _ :: _, ⟨t, ts⟩ => ⟨h _ t, TeleOk.map h ts⟩

theorem FieldsTyped.append {J : Jdg → Prop} {d : Decl} :
    {Γ : List Tm} → {i : Nat} → {Fs R : List Field} → FieldsTyped J d Γ i (Fs ++ R) →
    FieldsTyped J d Γ i Fs ∧ FieldsTyped J d (fieldCtx Γ Fs) (i + Fs.length) R
  | _, _, [], _, h => ⟨trivial, by simpa [fieldCtx] using h⟩
  | _, _, .plain _ _ :: _, _, ⟨t, hl, h⟩ =>
    let ih := FieldsTyped.append h
    ⟨⟨t, hl, ih.1⟩, by simpa [fieldCtx, Nat.add_assoc, Nat.add_comm 1] using ih.2⟩
  | _, _, .recur _ :: _, _, ⟨t, h⟩ =>
    let ih := FieldsTyped.append h
    ⟨⟨t, ih.1⟩, by simpa [fieldCtx, Nat.add_assoc, Nat.add_comm 1] using ih.2⟩

theorem FieldsTyped.map {J J' : Jdg → Prop} {d : Decl} (h : ∀ j, J j → J' j) :
    {Γ : List Tm} → {i : Nat} → {Fs : List Field} → FieldsTyped J d Γ i Fs → FieldsTyped J' d Γ i Fs
  | _, _, [], t => t
  | _, _, .plain _ _ :: _, ⟨t, hl, ts⟩ => ⟨h _ t, hl, FieldsTyped.map h ts⟩
  | _, _, .recur _ :: _, ⟨t, ts⟩ => ⟨h _ t, FieldsTyped.map h ts⟩

theorem DeclWith.map {J J' : Jdg → Prop} {k : Nat} {d : Decl} (h : ∀ j, J j → J' j)
    (w : DeclWith J k d) : DeclWith J' k d :=
  ⟨w.level_pos, w.uniform_le, w.params.map h, fun C hC => (w.fields C hC).map h,
   fun C hC => h _ (w.result C hC), w.unused, w.inScope⟩

/-- Declaration `d` is well formed as the next one after `sig`. -/
abbrev DeclOk (N : Nat) (sig : Sig) (d : Decl) : Prop := DeclWith (Deriv N sig) sig.length d

/-- Every declaration is well formed after the ones before it. -/
inductive SigOk (N : Nat) : Sig → Prop where
  | nil : SigOk N []
  | snoc {sig : Sig} {d : Decl} : SigOk N sig → DeclOk N sig d → SigOk N (sig ++ [d])
