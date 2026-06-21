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

#ifndef EX_HEADER_
#define EX_HEADER_

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
using ExPair = std::pair<Expr, Expr>;

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
  UT::Vu name; // Int, Str
};
struct TyVar
{
  UT::Vu name; // type-variable name, without the leading backtick
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
  UT::Vu name;
};
struct PatInt // an integer literal -- matches by equality (refutable)
{
  ssize_t value;
  UT::Vu  anchor;
  size_t  line;
};
struct PatReal // a real literal (refutable)
{
  double value;
  UT::Vu anchor;
  size_t line;
};
struct PatStr // a string literal (refutable)
{
  UT::Vu value;
  UT::Vu anchor;
  size_t line;
};
// One entry in a struct pattern. `name` is the field name; it is empty for a
// positional slot. `pat` is the sub-pattern matched against that field.
struct FieldPat
{
  UT::Vu   name;
  Pattern *pat;
};
// `Type.{ ... }` -- matches a struct of the named type and destructures its
// fields. Named when any entry carries a field name (out-of-order and partial;
// a bare entry puns `name` as `name = name`); positional otherwise (one slot
// per field, in declaration order). `anchor`/`line` locate the type name for
// diagnostics.
struct PatStruct
{
  UT::Vu            type_name;
  UT::Vec<FieldPat> fields;
  UT::Vu            anchor;
  size_t            line;
};
// `Type.Tag.{ ... }` (or `Type.Tag` for a unit payload) -- matches a variant of
// the named union and destructures its payload, reusing FieldPat with the same
// named/positional rule as PatStruct. Refutable: it tests the constructor tag,
// so it may appear only in `if .. is` arms, never a lambda / `let`.
struct PatVariant
{
  UT::Vu            type_name;
  UT::Vu            tag;
  UT::Vec<FieldPat> fields;
  UT::Vu            anchor;
  size_t            line;
};

#define EX_PAT_VARIANTS                                                        \
  X(Wild, PatWild)                                                             \
  X(Var, PatVar)                                                               \
  X(Int, PatInt)                                                               \
  X(Real, PatReal)                                                             \
  X(Str, PatStr)                                                               \
  X(Struct, PatStruct)                                                         \
  X(Variant, PatVariant)

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
  UT::Vu name;
  Ty    *sig;
  Expr  *def;
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
  UT::Vu name;
};
struct ExStr
{
  UT::Vu value;
};
// `@extern ("symbol", "lib")` -- a foreign binding. Only valid as a global
// body; its type comes from the enclosing global's signature.
struct ExExtern
{
  UT::Vu symbol;
  UT::Vu lib;
};

struct ExFnDef
{
  UT::Vu param;
  Expr  *body;
  // Set when the parameter is a pattern (e.g. `\Person.{x,y} = ...`); the LL
  // pass lowers it into a plain `param` plus destructuring lets, then clears
  // this back to null. Null for an ordinary `\x = ...`.
  Pattern *param_pat = nullptr;

  ExFnDef() = default;
  ExFnDef(UT::Vu param, AR::Arena &arena);
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
// from ExIf by the `is` after the scrutinee. The LL pass lowers it into an
// ExCase, so TC/IT never see an ExMatch.
struct ExMatch
{
  Expr             *scrut;
  UT::Vec<MatchArm> arms;
  Expr             *alt;
};

// The head an alternative matches on (see ExCase). Shared with the interpreter
// (IT aliases this), so it lives here.
enum class AltKind
{
  Con,
  Int,
  Real,
};

// One alternative of a lowered `Case`. Selected when the forced scrutinee's
// head matches `kind`: a constructor (`type_name`.`ctor`, the `tag`-th of its
// type, binding the payload positionally to `binders`; an empty binder ignores
// a slot) or an Int/Real literal (`ival`/`rval`). Built only by LL.
struct CaseAlt
{
  AltKind         kind;
  UT::Vu          type_name{}; // Con: the scrutinee's nominal type
  UT::Vu          ctor{};      // Con: the constructor / variant name
  size_t          tag  = 0;    // Con: constructor index within its type
  ssize_t         ival = 0;    // Int
  double          rval = 0;    // Real
  UT::Vec<UT::Vu> binders;     // Con: payload binder names, positional
  Expr           *body = nullptr;
};

// `case scrut of alt... else deflt` -- the one branching form after lowering.
// `if` and every match compile to this; TC types it directly and IT runs it as
// the lazy `Case` node.
struct ExCase
{
  Expr            *scrut;
  UT::Vec<CaseAlt> alts;
  Expr            *deflt;
};

struct ExApp
{
  Expr *fn;
  Expr *arg;
};

struct ExLet
{
  UT::Vu var;
  Expr  *val;
  Expr  *body;
  // Set when the binder is a pattern (`let Person.{x,y} = e in ...`); the LL
  // pass lowers it into nested plain lets, then clears this to null. `sig`,
  // when set, pins the bound value's type (used by lowering to make the
  // destructured field accesses statically typed); TC unifies the value with
  // it.
  Pattern *pat = nullptr;
  Ty      *sig = nullptr;
};

// One `field : Type` entry in a struct declaration.
struct FieldDecl
{
  UT::Vu name;
  Ty    *ty;
};
// One `field = expr` entry in a struct literal.
struct FieldInit
{
  UT::Vu name;
  Expr  *val;
};

// A struct type declaration: `$ Name : Struct = field: Ty, ...`. Defines a
// nominal record type; it produces no runtime value.
struct ExStructDecl
{
  UT::Vu             name;
  UT::Vec<FieldDecl> fields;
};
// A struct literal: `Type.{ field = expr, ... }`. Qualified form only, so
// `type_name` is always set (bare `.{...}` is a later increment).
struct ExStructLit
{
  UT::Vu             type_name;
  UT::Vec<FieldInit> fields;
};
// Field access: `record.field`.
struct ExField
{
  Expr  *record;
  UT::Vu field;
};

// One variant of a union declaration: `Tag: payload`. The payload is a list of
// fields (reusing FieldDecl); a field name is empty for an anonymous positional
// payload (`{T, U}`) and set for a named one (`{lhs: T, rhs: U}`). A bare type
// `Tag: T` is one anonymous field; `Tag: {}` is the empty (unit) payload.
struct VariantDecl
{
  UT::Vu             tag;
  UT::Vec<FieldDecl> fields;
  UT::Vu             anchor;
  size_t             line = 0;
};
// A union type declaration: `$ Name : Union = Tag: payload, ...`. A nominal sum
// type; it produces no runtime value.
struct ExUnionDecl
{
  UT::Vu               name;
  UT::Vec<VariantDecl> variants;
};
// A variant construction `Type.Tag.{ ... }` (or `Type.Tag` for a unit payload).
// `fields` are the payload values (reusing FieldInit): named when any carries a
// field name, positional otherwise -- same rule as a struct literal/pattern.
struct ExVariantLit
{
  UT::Vu             type_name;
  UT::Vu             tag;
  UT::Vec<FieldInit> fields;
  UT::Vu             anchor;
  size_t             line = 0;
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
  X(Case, ExCase)                                                              \
  X(Str, ExStr)                                                                \
  X(Extern, ExExtern)                                                          \
  X(StructDecl, ExStructDecl)                                                  \
  X(StructLit, ExStructLit)                                                    \
  X(Field, ExField)                                                            \
  X(UnionDecl, ExUnionDecl)                                                    \
  X(VariantLit, ExVariantLit)

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

using RExpr       = ER::Result<Expr *>;
using RTy         = ER::Result<Ty *>;
using RPattern    = ER::Result<Pattern *>;
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
  UT_NODISCARD RExpr    parse_global();
  UT_NODISCARD RExpr    parse_extern();
  UT_NODISCARD RExpr    parse_struct_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_struct_lit(UT::Vu type_name);
  UT_NODISCARD RExpr    parse_union_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_variant_lit(UT::Vu           type_name,
                                          UT::Vu           tag,
                                          const LX::Token &tok);
  UT_NODISCARD RTy      parse_type();
  UT_NODISCARD RTy      parse_type_atom();
  UT_NODISCARD RExpr    parse_expr(int min_bp);
  UT_NODISCARD RExpr    parse_prefix();
  UT_NODISCARD RExpr    parse_primary();
  UT_NODISCARD RExpr    parse_group();
  UT_NODISCARD RExpr    parse_let();
  UT_NODISCARD RExpr    parse_if();
  UT_NODISCARD RExpr    parse_closure();
  UT_NODISCARD RPattern parse_pattern();
  // Parses a qualified pattern after an uppercase type name: dispatches to a
  // `Type.{ ... }` struct pattern or a `Type.Tag[.{ ... }]` variant pattern.
  UT_NODISCARD RPattern parse_struct_pattern(UT::Vu           type_name,
                                             const LX::Token &tn);
  // Parses the `{ field-patterns }` body shared by struct patterns and variant
  // payloads. Assumes the opening `{` is the next token.
  UT_NODISCARD ER::Result<UT::Vec<FieldPat>>
               parse_field_pats(UT::Vu type_name, const LX::Token &tn);
  UT_NODISCARD LX::RToken expect(LX::TokenTag tag, const char *what);
  void                    recover();

  UT_NODISCARD Expr    *alloc(Expr e);
  UT_NODISCARD Expr    *mk_int(const LX::Token &t);
  UT_NODISCARD Expr    *mk_real(const LX::Token &t);
  UT_NODISCARD Expr    *mk_str(const LX::Token &t);
  UT_NODISCARD Expr    *mk_var(const LX::Token &t);
  UT_NODISCARD Expr    *mk_app(Expr *fn, Expr *arg);
  UT_NODISCARD Expr    *mk_op_var(UT::Vu name);
  UT_NODISCARD Expr    *mk_unop(UT::Vu op, Expr *operand);
  UT_NODISCARD Expr    *mk_binop(UT::Vu op, Expr *lhs, Expr *rhs);
  UT_NODISCARD Expr    *mk_if(Expr *cond, Expr *then, Expr *alt);
  UT_NODISCARD Expr    *mk_let(UT::Vu var, Expr *val, Expr *body);
  UT_NODISCARD Expr    *mk_fndef(UT::Vu param, Expr *body);
  UT_NODISCARD Expr    *mk_def(UT::Vu name, Ty *sig, Expr *def);
  UT_NODISCARD Expr    *mk_extern(UT::Vu symbol, UT::Vu lib);
  UT_NODISCARD Expr    *mk_field(Expr *record, UT::Vu field);
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

#endif // EX_HEADER_
