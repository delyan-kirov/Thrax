/**
 * \file TCxDATA.hpp
 * \brief The type checker's data schema: the vocabulary TC::Checker (in TC.cpp)
 *
 * The checker lowers an EX::Expr tree into a typed \ref Core IR (desugar) and
 * runs Algorithm W over it. The data falls into five groups:
 *
 *   - \ref Type   -- an inferred type: a union-find graph of Con / Var / Arrow.
 *   - \ref Scheme -- a generalized (polymorphic) type, `forall vars . type`.
 *   - \ref Core   -- the desugared term language that inference walks.
 *   - \ref Overload (and \ref overload_db) -- how overloaded names type.
 *   - \ref GlobalEntry / \ref Globals plus the nominal-type tables -- the
 *     top-level symbol environment.
 *
 * Nodes are pool-allocated in std::deque stores (\ref TypeStore, \ref
 * CoreStore,
 * \ref GlobalStore) so raw `Type *` / `Core *` stay valid as the pools grow.
 */

#ifndef TCXDATA_HEADER_
#define TCXDATA_HEADER_

#include "EX.hpp"
#include "UT.hpp"

namespace TC
{

/* -- Types ------------------------------------------------------------------
 */

struct Scheme;
using Env = std::unordered_map<std::string, Scheme>;

struct Type;

/// A nominal type constructor: `Int`, `Str`, or a declared struct/union name.
/// `args` are the type arguments of a generic type (e.g. `Int` in `Maybe Int`);
/// empty for a nullary constructor.
struct TCon
{
  std::string         name;
  std::vector<Type *> args;
};
/// A unification variable. `ref` is its union-find link once bound; a `rigid`
/// variable is a signature skolem that unifies only with itself, with `name`
/// its written name for diagnostics. `id` is its stable identity.
struct TVar
{
  int         id;
  Type       *ref   = nullptr;
  bool        rigid = false;
  std::string name;
};
/// A function type `from -> to`.
struct TArrow
{
  Type *from;
  Type *to;
};

#define TC_TYPE_VARIANTS                                                       \
  X(Con, TCon)                                                                 \
  X(Var, TVar)                                                                 \
  X(Arrow, TArrow)

enum class Kind
{
#define X(tag, type) tag,
  TC_TYPE_VARIANTS
#undef X
};

using TypeData =
#define X(tag, type) type,
  std::variant<TC_TYPE_VARIANTS std::monostate>
#undef X
  ;

/// An inferred type. \see Kind for the active-variant tag.
struct Type
{
  TypeData as;
  Kind
  kind() const
  {
    return static_cast<Kind>(as.index());
  }
};

using TypeStore = std::deque<Type>;
using TyVarEnv
  = std::unordered_map<std::string, Type *>; ///< sig type vars by name

/// Type-variable identities (\ref TVar::id), as quantified by a \ref Scheme or
/// gathered by free_vars; \ref Subst maps them to types when instantiating.
using VarIds = std::vector<int>;
using Subst  = std::unordered_map<int, Type *>;

/// A type scheme: `forall vars . type`.
struct Scheme
{
  VarIds vars;
  Type  *type;
};

/* -- Overloads --------------------------------------------------------------
 */

/// One typed overload of an overloaded name. \ref sig is the signature laid out
/// flat -- the operand types followed by the result type -- so arity is
/// `sig.size() - 1` and the form is uniform across unary and binary uses. \ref
/// mono is the monomorphic implementation key a resolved use is rewritten to
/// (e.g. "+@Int"). The resolver matches on operands only; the result is stored
/// for later return-type overloading.
struct Overload
{
  std::vector<const char *> sig;
  std::string               mono;
};
/// Overloaded name -> its overloads. Defined in TCxDATA.cpp (it alone needs
/// OP's type-name constants, keeping this header free of OP).
using OverloadTable = std::unordered_map<std::string, std::vector<Overload>>;

extern const OverloadTable overload_db;

/* -- Core -------------------------------------------------------------------
 */

struct Core;

/// One alternative of a Case. The tag is EX::AltKind: a Con alt matches the
/// `tag`-th constructor of `type_name`, binding its payload positionally to
/// `binders`; an Int/Real alt matches a literal (value irrelevant to typing).
/// `body` is the arm's result, present on every kind.
struct AltCon
{
  std::string              type_name;
  size_t                   tag = 0;
  std::vector<std::string> binders;
  Core                    *body = nullptr;
};
struct AltInt
{
  Core *body = nullptr;
};
struct AltReal
{
  Core *body = nullptr;
};

#define TC_ALT_VARIANTS                                                        \
  X(Con, AltCon)                                                               \
  X(Int, AltInt)                                                               \
  X(Real, AltReal)

using AltData =
#define X(tag, type) type,
  std::variant<TC_ALT_VARIANTS std::monostate>
#undef X
  ;

/// A Case alternative. Its tag is EX::AltKind rather than a private enum, so it
/// stays in step with the interpreter; AltData lists payloads in that same
/// order, making the active variant index the tag.
struct CoreAlt
{
  AltData as;
  EX::AltKind
  kind() const
  {
    return static_cast<EX::AltKind>(as.index());
  }
};
using CoreAlts = std::vector<CoreAlt>;

/// Struct/variant literal field initializers, (name, value) in source order; a
/// name is empty for a positional variant payload.
using FieldInits = std::vector<std::pair<std::string, Core *>>;

/// \name Core payloads
/// One struct per \ref CKind, holding only that kind's fields. `anchor` is the
/// diagnostic source slice; CVar's `slot` is the EX name field rewritten when
/// the use resolves to an overload.
///@{
struct CVar
{
  std::string name;
  UT::Vu      anchor;
  UT::Vu     *slot = nullptr;
  /// Non-null for a use that MR left as an overload set (an EX::ExOverload):
  /// the mangled candidate globals. The use is typed as a fresh variable and
  /// the fit candidate chosen by resolve_user_sites, which writes its name to
  /// `slot`.
  const UT::Vec<UT::Vu> *overload = nullptr;
};
struct CLitInt
{
};
struct CLitReal
{
};
struct CLitStr
{
};
struct CExtern
{
};
struct CLam
{
  std::string param;
  Core       *body;
};
struct CApp
{
  Core *fn;
  Core *arg;
};
struct CLet
{
  std::string name;
  Core       *val;
  Core       *body;
  Type       *sig = nullptr; ///< when set, pins the bound value's type
};
struct CStructLit
{
  std::string type_name; ///< empty for a bare `.{...}` literal (type inferred)
  UT::Vu      anchor;
  FieldInits  fields;
  EX::ExStructLit *ex = nullptr; ///< back-link to patch a resolved bare literal
};
struct CField
{
  Core       *record;
  std::string field;
  UT::Vu      anchor;
};
struct CCase
{
  Core    *scrut;
  Core    *deflt;
  CoreAlts alts;
};
struct CVariantLit
{
  std::string type_name; ///< empty for a bare `.Tag...` literal (type inferred)
  std::string vtag;
  UT::Vu      anchor;
  FieldInits  fields;
  EX::ExVariantLit *ex
    = nullptr; ///< back-link to patch a resolved bare literal
};
///@}

#define TC_CORE_VARIANTS                                                       \
  X(Var, CVar)                                                                 \
  X(LitInt, CLitInt)                                                           \
  X(LitReal, CLitReal)                                                         \
  X(LitStr, CLitStr)                                                           \
  X(Lam, CLam)                                                                 \
  X(App, CApp)                                                                 \
  X(Let, CLet)                                                                 \
  X(Extern, CExtern)                                                           \
  X(StructLit, CStructLit)                                                     \
  X(Field, CField)                                                             \
  X(Case, CCase)                                                               \
  X(VariantLit, CVariantLit)

enum class CKind
{
#define X(tag, type) tag,
  TC_CORE_VARIANTS
#undef X
};

using CoreData =
#define X(tag, type) type,
  std::variant<TC_CORE_VARIANTS std::monostate>
#undef X
  ;

/// A desugared term node. \see CKind for the active-variant tag.
struct Core
{
  CoreData as;
  CKind
  kind() const
  {
    return static_cast<CKind>(as.index());
  }
};

using CoreStore = std::deque<Core>;

/// A deferred overload resolution. An overloaded use is typed as a fresh
/// variable `use` and resolved once its operands settle: the matching overload
/// is chosen and `slot` (the EX Var name) rewritten to its impl key. The whole
/// `use` type is kept (not split per operand) so resolution is arity-uniform.
struct ResolveSite
{
  UT::Vu     *slot;
  Type       *use;
  std::string base; ///< the overloaded name, looked up in \ref overload_db
  UT::Vu      anchor;
};
using ResolveSites = std::vector<ResolveSite>;

/// The accumulated diagnostics; empty iff the program is well typed.
using Diagnostics = std::vector<ER::Diagnostic>;

/* -- Globals & nominal types ------------------------------------------------
 */

/// A top-level binding: its EX definition, desugared `body`, and -- once
/// resolved -- its `scheme`. `state` guards the on-demand, cycle-checked
/// resolution (see resolve()).
struct GlobalEntry
{
  EX::ExDef *def;
  Core      *body;
  enum
  {
    Unresolved,
    Resolving,
    Resolved
  } state
    = Unresolved;
  Scheme scheme{};
};

using GlobalStore = std::deque<GlobalEntry>;
using NameIndex   = std::unordered_map<std::string, size_t>;

/// The top-level symbol table: entries in source order plus a by-name index.
/// One `add` keeps the two in step, so order (for deterministic diagnostics)
/// and O(1) lookup (for cross-references) never desync.
class Globals
{
public:
  void                  add(GlobalEntry e);
  GlobalEntry          *find(UT::Vu name);
  GlobalStore::iterator begin();
  GlobalStore::iterator end();

private:
  GlobalStore m_entries;
  NameIndex   m_index;
};

/// A declared struct/variant's shape: its fields in order, as (name, type). A
/// field name is empty for a positional variant payload.
using StructFields = std::vector<std::pair<std::string, Type *>>;
/// One variant of a union: its `tag` and payload shape. Its index in the
/// union's \ref Variants is its runtime constructor tag.
struct VariantShape
{
  std::string  tag;
  StructFields fields;
};
using Variants = std::vector<VariantShape>;

/// A declared nominal type's polymorphic shape. `params` are the type-variable
/// ids it is generic over (empty for a monomorphic type), referenced by the
/// field/variant types; a use site instantiates them with fresh vars and forms
/// `con(name, args)`. \see TC::instantiate_struct / instantiate_union.
struct StructDef
{
  VarIds       params;
  StructFields fields;
};
struct UnionDef
{
  VarIds   params;
  Variants variants;
};

/// Declared nominal types, keyed by type name.
using StructTable = std::unordered_map<std::string, StructDef>;
using UnionTable  = std::unordered_map<std::string, UnionDef>;

} // namespace TC

#endif // TCXDATA_HEADER_
