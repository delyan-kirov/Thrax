/*-------------------------------------------------------------------------------
 *\file EX.hpp
 *\info Header file for the Parser.
 *
 * EX is a Pratt (precedence-climbing) parser that pulls flat tokens from a
 * streaming LX::Lexer and builds the Expr tree below. Operator precedence and
 * structure (groups, let, if, lambda) interleave through one recursion, so the
 * C++ call stack mirrors the grammar -- which is what lets errors carry a
 * meaningful context chain (see ER::Diagnostic). The Expr types are unchanged
 * from before; only the parsing engine is new.
 *-----------------------------------------------------------------------------*/

#ifndef EX_HEADER
#define EX_HEADER

#include "ER.hpp"
#include "LX.hpp"
#include "UT.hpp"

namespace EX
{

/*------------------------------------------------------------------------------
 *\TYPES
 *-----------------------------------------------------------------------------*/

struct Expr;
using Exprs  = UT::Vec<Expr>;
using ExPair = UT::Pair<Expr>;

/*------------------------------------------------------------------------------
 *\TYPE SYNTAX
 *
 * The syntactic type written in a signature: `Int`, `Str` (TyCon), `T (TyVar),
 * and `T -> U` (TyArrow). This is what the parser builds; the type checker (TC)
 * lowers it into its own representation with unification variables.
 *-----------------------------------------------------------------------------*/

struct Ty;

struct TyCon
{
  UT::String name; // Int, Str
};
struct TyVar
{
  UT::String name; // type-variable name, without the leading backtick
};
struct TyArrow
{
  Ty *from;
  Ty *to;
};

#define EX_TY_VARIANTS                                                         \
  X(Con, TyCon)                                                                \
  X(Var, TyVar)                                                                \
  X(Arrow, TyArrow)

enum class TyTag
{
#define X(tag, type) tag,
  EX_TY_VARIANTS
#undef X
};

using TyData =
#define X(tag, type) type,
  std::variant<EX_TY_VARIANTS std::monostate>
#undef X
  ;

struct Ty
{
  TyTag  tag;
  TyData as;
};

/*------------------------------------------------------------------------------
 *\PATTERNS
 *
 * Patterns appear in lambda parameters, `let` bindings, and (soon) `if .. is`
 * match arms. They are lowered away by the LL pass (see LL.hpp) before type
 * checking, so neither TC nor IT ever sees a Pattern node.
 *-----------------------------------------------------------------------------*/

struct Pattern;

struct PatWild // `_` -- matches anything, binds nothing
{
};
struct PatVar // a lowercase name -- matches anything, binds it
{
  UT::String name;
};
struct PatInt // an integer literal -- matches by equality (refutable)
{
  ssize_t    value;
  UT::String anchor;
  size_t     line;
};
struct PatReal // a real literal (refutable)
{
  double     value;
  UT::String anchor;
  size_t     line;
};
struct PatStr // a string literal (refutable)
{
  UT::String value;
  UT::String anchor;
  size_t     line;
};
// One entry in a struct pattern. `name` is the field name; it is empty for a
// positional slot. `pat` is the sub-pattern matched against that field.
struct FieldPat
{
  UT::String name;
  Pattern   *pat;
};
// `Type.{ ... }` -- matches a struct of the named type and destructures its
// fields. Named when any entry carries a field name (out-of-order and partial;
// a bare entry puns `name` as `name = name`); positional otherwise (one slot
// per field, in declaration order). `anchor`/`line` locate the type name for
// diagnostics.
struct PatStruct
{
  UT::String        type_name;
  UT::Vec<FieldPat> fields;
  UT::String        anchor;
  size_t            line;
};

#define EX_PAT_VARIANTS                                                        \
  X(Wild, PatWild)                                                             \
  X(Var, PatVar)                                                               \
  X(Int, PatInt)                                                               \
  X(Real, PatReal)                                                             \
  X(Str, PatStr)                                                               \
  X(Struct, PatStruct)

enum class PatTag
{
#define X(tag, type) tag,
  EX_PAT_VARIANTS
#undef X
};

using PatData =
#define X(tag, type) type,
  std::variant<EX_PAT_VARIANTS std::monostate>
#undef X
  ;

struct Pattern
{
  PatTag  tag;
  PatData as;
};

struct ExUnknown
{
};
// A global definition: `$ name [: sig] = def`. `sig` is null when omitted (the
// type checker must then infer a ground, non-arrow type for it).
struct ExDef
{
  UT::String name;
  Ty        *sig;
  Expr      *def;
};
struct ExInt
{
  ssize_t value;
};
struct ExReal
{
  double value;
};
struct ExVar
{
  UT::String name;
};
struct ExStr
{
  UT::String value;
};
// `@extern ("symbol", "lib")` -- a foreign binding. Only valid as a global
// body; its type comes from the enclosing global's signature.
struct ExExtern
{
  UT::String symbol;
  UT::String lib;
};

struct ExFnDef
{
  UT::String param;
  Expr      *body;
  // Set when the parameter is a pattern (e.g. `\Person.{x,y} = ...`); the LL
  // pass lowers it into a plain `param` plus destructuring lets, then clears
  // this back to null. Null for an ordinary `\x = ...`.
  Pattern *param_pat = nullptr;

  ExFnDef() = default;
  ExFnDef(UT::String param, AR::Arena &arena);
};

struct ExIf
{
  Expr *cond;
  Expr *then;
  Expr *alt;
};

// One `is pat then body` arm of a match.
struct MatchArm
{
  Pattern *pat;
  Expr    *body;
};
// A match: `if scrut is pat then e ... is pat then e else alt`. Distinguished
// from ExIf by the `is` after the scrutinee. The LL pass lowers it into nested
// boolean `if`s with equality guards and binding lets, so TC/IT never see it.
struct ExMatch
{
  Expr               *scrut;
  UT::Vec<MatchArm>   arms;
  Expr               *alt;
};

struct ExApp
{
  Expr *fn;
  Expr *arg;
};

struct ExLet
{
  UT::String var;
  Expr      *val;
  Expr      *body;
  // Set when the binder is a pattern (`let Person.{x,y} = e in ...`); the LL
  // pass lowers it into nested plain lets, then clears this to null. `sig`, when
  // set, pins the bound value's type (used by lowering to make the destructured
  // field accesses statically typed); TC unifies the value with it.
  Pattern *pat = nullptr;
  Ty      *sig = nullptr;
};

// One `field : Type` entry in a struct declaration.
struct FieldDecl
{
  UT::String name;
  Ty        *ty;
};
// One `field = expr` entry in a struct literal.
struct FieldInit
{
  UT::String name;
  Expr      *val;
};

// A struct type declaration: `$ Name : Struct = field: Ty, ...`. Defines a
// nominal record type; it produces no runtime value.
struct ExStructDecl
{
  UT::String         name;
  UT::Vec<FieldDecl> fields;
};
// A struct literal: `Type.{ field = expr, ... }`. Qualified form only, so
// `type_name` is always set (bare `.{...}` is a later increment).
struct ExStructLit
{
  UT::String         type_name;
  UT::Vec<FieldInit> fields;
};
// Field access: `record.field`.
struct ExField
{
  Expr      *record;
  UT::String field;
};

#define EX_EXPR_VARIANTS                                                       \
  X(Unknown, ExUnknown)                                                        \
  X(Def, ExDef)                                                                \
  X(Int, ExInt)                                                                \
  X(Real, ExReal)                                                              \
  X(Let, ExLet)                                                                \
  X(FnDef, ExFnDef)                                                            \
  X(App, ExApp)                                                                \
  X(Var, ExVar)                                                                \
  X(If, ExIf)                                                                  \
  X(Match, ExMatch)                                                            \
  X(Str, ExStr)                                                                \
  X(Extern, ExExtern)                                                          \
  X(StructDecl, ExStructDecl)                                                  \
  X(StructLit, ExStructLit)                                                    \
  X(Field, ExField)

enum class ExprTag
{
#define X(tag, type) tag,
  EX_EXPR_VARIANTS
#undef X
};

using ExprData =
#define X(tag, type) type,
  std::variant<EX_EXPR_VARIANTS std::monostate>
#undef X
  ;

struct Expr
{
  ExprTag  tag;
  ExprData as;

  Expr() = default;
  Expr(ExprTag tag);
  Expr(ExprTag tag, AR::Arena &arena);
};

using R           = ER::Result<Expr *>;
using Diagnostics = std::vector<ER::Diagnostic>;

/*-------------------------------------------------------------------------------
 *\PARSER
 *------------------------------------------------------------------------------*/

class Parser
{
public:
  AR::Arena  &m_arena;
  LX::Lexer  &m_lex;
  Exprs       m_exprs;
  Diagnostics m_diags;

  Parser(LX::Lexer &lex);

  void operator()();

private:
  UT_NODISCARD R parse_global();
  UT_NODISCARD R parse_extern();
  UT_NODISCARD R parse_struct_decl(const LX::Token &name);
  UT_NODISCARD R parse_struct_lit(UT::String type_name);
  UT_NODISCARD ER::Result<Ty *> parse_type();
  UT_NODISCARD ER::Result<Ty *> parse_type_atom();
  UT_NODISCARD R                parse_expr(int min_bp);
  UT_NODISCARD R                parse_prefix();
  UT_NODISCARD R                parse_primary();
  UT_NODISCARD R                parse_group();
  UT_NODISCARD R                parse_let();
  UT_NODISCARD R                parse_if();
  UT_NODISCARD R                parse_closure();
  UT_NODISCARD ER::Result<Pattern *> parse_pattern();
  UT_NODISCARD ER::Result<Pattern *> parse_struct_pattern(UT::String type_name,
                                                          const LX::Token &tn);
  UT_NODISCARD LX::R expect(LX::TokenTag tag, const char *what);
  void               recover();

  UT_NODISCARD Expr *alloc(Expr e);
  UT_NODISCARD Expr *mk_int(const LX::Token &t);
  UT_NODISCARD Expr *mk_real(const LX::Token &t);
  UT_NODISCARD Expr *mk_str(const LX::Token &t);
  UT_NODISCARD Expr *mk_var(const LX::Token &t);
  UT_NODISCARD Expr *mk_app(Expr *fn, Expr *arg);
  UT_NODISCARD Expr *mk_op_var(UT::String name);
  UT_NODISCARD Expr *mk_unop(UT::String op, Expr *operand);
  UT_NODISCARD Expr *mk_binop(UT::String op, Expr *lhs, Expr *rhs);
  UT_NODISCARD Expr *mk_if(Expr *cond, Expr *then, Expr *alt);
  UT_NODISCARD Expr *mk_let(UT::String var, Expr *val, Expr *body);
  UT_NODISCARD Expr *mk_fndef(UT::String param, Expr *body);
  UT_NODISCARD Expr *mk_def(UT::String name, Ty *sig, Expr *def);
  UT_NODISCARD Expr *mk_extern(UT::String symbol, UT::String lib);
  UT_NODISCARD Expr    *mk_field(Expr *record, UT::String field);
  UT_NODISCARD Ty      *mk_ty(Ty t);
  UT_NODISCARD Pattern *alloc_pat(Pattern p);
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(ExprTag t, int level = 0);
std::string pprint(Expr *e, int level = 0);
std::string pprint(Pattern *p);

} // namespace EX

#endif // EX_HEADER
