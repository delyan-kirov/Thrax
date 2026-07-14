/**
 * \file TCxDATA.hpp
 * \brief The type checker's data schema: the vocabulary TC::Checker (in TC.cpp)
 *
 * The checker runs Algorithm W. The data falls into four groups:
 *
 *   - \ref Type   -- an inferred type: a union-find graph of Con / Var / Arrow.
 *   - \ref Scheme -- a generalized (polymorphic) type, `forall vars . type`.
 *   - \ref Overload (and \ref overload_db) -- how overloaded names type.
 *   - \ref GlobalEntry / \ref Globals plus the nominal-type tables -- the
 *     top-level symbol environment.
 *
 * Nodes are pool-allocated in std::deque stores (\ref TypeStore, \ref
 * GlobalStore) so raw `Type *` stay valid as the pools grow.
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
/// A function type `from -[eff]-> to`. `eff` is the function's latent effect
/// row -- the effects performed by calling it (\see TRowEmpty / TRowExtend). A
/// pure function's `eff` is the empty closed row.
struct TArrow
{
  Type *from;
  Type *to;
  Type *eff;
};
/// The empty, closed effect row `<>` -- a pure computation performs no effect.
struct TRowEmpty
{
};
/// An effect-row extension `<label | rest>`: the row contains effect `label`
/// over the row `rest` (another extension, a row variable, or the empty row).
/// Rows are unordered up to reordering and may carry duplicate labels (scoped
/// labels, a la Leijen); a row variable in tail position is an ordinary TVar.
struct TRowExtend
{
  std::string label;
  Type       *rest;
};

#define TC_TYPE_VARIANTS                                                       \
  X(Con, TCon)                                                                 \
  X(Var, TVar)                                                                 \
  X(Arrow, TArrow)                                                             \
  X(RowEmpty, TRowEmpty)                                                       \
  X(RowExtend, TRowExtend)

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

/// Top-level binding
struct GlobalEntry
{
  EX::ExDef *def;
  EX::Expr  *body;
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
