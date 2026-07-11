#ifndef EXxDATA_HEADER_
#define EXxDATA_HEADER_

#include "ER.hpp"
#include "LX.hpp"
#include "OP.hpp"
#include "UT.hpp"

/*------------------------------------------------------------------------------
 *\MACROS
 *
 * EX_TRY / EX_CTX / EX_ERR are the only error primitives. EX_TRY propagates a
 * failure unchanged; EX_CTX propagates it but first appends a context frame
 * anchored at a token; EX_ERR starts a fresh root cause. All three rely on
 * ER::Fail converting to whatever Result<T> the enclosing method returns. They
 * use the GNU statement-expression form so they can yield the unwrapped value
 * inline.
 *-----------------------------------------------------------------------------*/

#define EX_TRY(EXPR)                                                           \
  ({                                                                           \
    auto r_ = (EXPR);                                                          \
    if (!r_.ok) return ER::Fail{ r_.err };                                     \
    r_.value;                                                                  \
  })

#define EX_CTX(EXPR, TOK, ...)                                                 \
  ({                                                                           \
    auto r_ = (EXPR);                                                          \
    if (!r_.ok)                                                                \
    {                                                                          \
      ER::push_ctx(m_arena,                                                    \
                   r_.err,                                                     \
                   (TOK).str,                                                  \
                   (TOK).line,                                                 \
                   ER::mk_msg(m_arena, __VA_ARGS__));                          \
      return ER::Fail{ r_.err };                                               \
    }                                                                          \
    r_.value;                                                                  \
  })

#define EX_ERR(CODE, TOK, ...)                                                 \
  return ER::Fail                                                              \
  {                                                                            \
    ER::mk_root(m_arena,                                                       \
                (CODE),                                                        \
                (TOK).str,                                                     \
                (TOK).line,                                                    \
                ER::mk_msg(m_arena, __VA_ARGS__))                              \
  }

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
  UT::Vu        name; // Int, Str, or a struct/union name
  UT::Vec<Ty *> args; // type arguments: empty for a nullary con, set for an
                      // applied generic type like `Maybe Int`
};
struct TyVar
{
  UT::Vu name; // type-variable name, without the leading backtick
};
// A function type `from -> to`, optionally carrying an effect-row annotation
// written `from -> <E1, E2 | `e> to` (M3 effect rows). `eff_labels` are the
// effect names; `eff_tail` is the row-variable name (without backtick) for an
// open row, empty for a closed one. No annotation (both empty, `eff` false)
// means the pure/total empty row; an explicit `<>` is also pure with eff set.
struct TyArrow
{
  Ty             *from;
  Ty             *to;
  UT::Vec<UT::Vu> eff_labels;
  UT::Vu          eff_tail{};
  bool            eff = false; // an effect row was written (even if `<>`)
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
// The unit value `{}` (empty record). Its type is the unit type, also written
// `{}`. The type checker keeps it distinct from Int; the runtime represents it
// as 0 (CR lowers it to the integer literal 0).
struct ExUnit
{
};
// A global definition: `$ name [: sig] = def`. `sig` is null when omitted (the
// type checker must then infer a ground, non-arrow type for it). `name` is
// rewritten to the mangled `MOD/name` by MR; `origin` keeps the original
// source-name view (for diagnostics: a caret location and an un-mangled label).
struct ExDef
{
  UT::Vu name;
  Ty    *sig;
  Expr  *def;
  UT::Vu origin{};
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
  UT::Vu qualifier{}; // module prefix from `MOD.name`; empty when unqualified
};
struct ExStr
{
  UT::Vu value;
};
// `@extern.{ "symbol", "lib" }` -- a foreign binding. Only valid as a global
// body; its type comes from the enclosing global's signature.
struct ExExtern
{
  UT::Vu symbol;
  UT::Vu lib;
};

// `@mod NAME` -- declares the file's module. The first top-level node of a
// file.
struct ExModDecl
{
  UT::Vu name;
};

// `$ with <lhs> [= <rhs>]` -- one import directive, captured literally as
// parsed; the MR pass interprets it. A name part is a (prefix, name) pair:
// `MOD.sym` is (MOD, sym), a bare `MOD` or `name` is ("", MOD/name). `has_eq`
// records whether
// `= rhs` was written. The five forms:
//   with MOD                 lhs=(_,MOD)                 has_eq=0
//   with ALIAS = MOD         lhs=(_,ALIAS) rhs=(_,MOD)   has_eq=1
//   with MOD = MOD           lhs=(_,MOD)   rhs=(_,MOD)   has_eq=1
//   with name = MOD.sym      lhs=(_,name)  rhs=(MOD,sym) has_eq=1
//   with MOD.sym = MOD.sym   lhs=(MOD,sym) rhs=(MOD,sym) has_eq=1
struct ExImport
{
  UT::Vu lhs_prefix;
  UT::Vu lhs_name;
  UT::Vu rhs_prefix;
  UT::Vu rhs_name;
  bool   has_eq = false;
};

// `$ @private` / `$ @public` -- toggles export visibility of the symbols that
// follow it in the file (public is the default; reset at end of file).
struct ExVis
{
  bool is_private;
};

// An unresolved overload set: a use whose name (bare or qualified) matched
// several definitions. Produced by MR, resolved by TC (by type). `name` is the
// original source name (diagnostics); `candidates` are the mangled globals;
// `anchor` is the source slice of the use. `chosen` is empty until TC picks the
// candidate whose type fits the use, after which IT reads it as the resolved
// global name.
struct ExOverload
{
  UT::Vu          name;
  UT::Vec<UT::Vu> candidates;
  UT::Vu          anchor{};
  UT::Vu          chosen{};
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

// One `is pat [if guard] then body` arm of a match. `guard`, when non-null, is
// an extra boolean test evaluated in the scope of the pattern's bindings; if it
// fails, the match falls through to the next arm (LL threads a shared
// fallthrough).
struct MatchArm
{
  Pattern *pat;
  Expr    *body;
  Expr    *guard = nullptr;
};
// A match: `when scrut is pat then e ... is pat then e else alt`. The LL pass
// lowers it into an ExCase / if-chain, so TC/IT never see an ExMatch.
struct ExMatch
{
  Expr             *scrut;
  UT::Vec<MatchArm> arms;
  Expr             *alt;
};

// One `is op a = body` clause of a handler: operation `op`, its single argument
// bound to `arg`, and the clause body (in which both `arg` and the handler's
// continuation `k` are in scope). To resume, the body applies `k` (a
// first-class value); to abort (exceptions), it ignores `k`.
struct HandlerClause
{
  UT::Vu op;
  UT::Vu arg;
  Expr  *body;
  UT::Vu qualifier{}; // effect prefix from `is Effect.op a`; empty when bare.
                      // MR rewrites `op` to the canonical `Effect.op` identity
                      // and clears this.
};
// A handler: `do <body> ctl k  is op a = e ...  [else x = e]`. Runs `body`; an
// operation performed within it dispatches to the matching clause (binding the
// operation's argument and the continuation `k`). The optional `else x = e` is
// the value clause -- it transforms the body's normal result, defaulting to
// identity when omitted. It installs a handler; it has no runtime value of its
// own.
struct ExHandle
{
  Expr                  *body;
  UT::Vu                 k;
  UT::Vec<HandlerClause> clauses;
  UT::Vu                 else_var{}; // empty when there is no `else`
  Expr                  *else_body = nullptr;
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
// An effect declaration: `$ Name : @effect = op : A -> B, ...`. Declares a set
// of OPERATIONS (reusing FieldDecl: `name` is the operation, `ty` its `A -> B`
// signature, whose codomain is the type a `perform` of it yields back).
// Produces no runtime value; its operations are registered as typed names. The
// effect name is a TypeName (uppercase), each operation a variable (lowercase).
struct ExEffectDecl
{
  UT::Vu             name;
  UT::Vec<FieldDecl> ops;
};
// A type alias declaration: `$ Name : @alias = target`. Fully transparent --
// the type checker resolves `Name` to `target` wherever it is written, so the
// two are interchangeable (like a C typedef). Produces no runtime value.
struct ExAliasDecl
{
  UT::Vu name;
  Ty    *target;
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
  X(Unit, ExUnit)                                                              \
  X(Def, ExDef)                                                                \
  X(Int, ExInt)                                                                \
  X(Real, ExReal)                                                              \
  X(Let, ExLet)                                                                \
  X(FnDef, ExFnDef)                                                            \
  X(App, ExApp)                                                                \
  X(Var, ExVar)                                                                \
  X(If, ExIf)                                                                  \
  X(Match, ExMatch)                                                            \
  X(Handle, ExHandle)                                                          \
  X(Case, ExCase)                                                              \
  X(Str, ExStr)                                                                \
  X(Extern, ExExtern)                                                          \
  X(StructDecl, ExStructDecl)                                                  \
  X(StructLit, ExStructLit)                                                    \
  X(Field, ExField)                                                            \
  X(UnionDecl, ExUnionDecl)                                                    \
  X(AliasDecl, ExAliasDecl)                                                    \
  X(EffectDecl, ExEffectDecl)                                                  \
  X(VariantLit, ExVariantLit)                                                  \
  X(ModDecl, ExModDecl)                                                        \
  X(Import, ExImport)                                                          \
  X(Vis, ExVis)                                                                \
  X(Overload, ExOverload)

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
 *\PRECEDENCE
 *
 * (lbp, rbp) binding powers. A left-associative operator of precedence p gets
 * lbp = 10p, rbp = 10p + 1, so equal precedence folds left. Application
 * (juxtaposition) binds tightest; unary - and ! are prefix.
 *------------------------------------------------------------------------------*/

struct Bp
{
  int l;
  int r;
};

using InfixTable = std::unordered_map<std::string, Bp>;
using UnopTable  = std::unordered_map<std::string, const char *>;
using OperandSet = std::unordered_set<LX::TokenTag>;

const Bp  APP_BP{ 50, 51 };
const int PREFIX_BP = 40; // unary - and !

// Binary operators, keyed by lexeme. Presence is exactly "is an infix
// operator"; the value is its binding power. The name stored on the node is the
// lexeme itself, which doubles as the OP key (see UTxOP).
const InfixTable infix_db{
  // Sequencing / pipes -- parser-desugared (see parse_expr): `a ; b` = run a,
  // then b; `x |> f` = `f x`; `f <| x` = `f x`. Lowest precedences so they bind
  // looser than arithmetic/comparison and application. `;` is right-associative
  // (l>r), `|>` left-associative (l<r), `<|` right-associative (l>r).
  { ";", { 2, 1 } },        //
  { "<|", { 5, 4 } },       //
  { "|>", { 6, 7 } },       //
  { OP::ISEQ, { 10, 11 } }, //
  { OP::GEQ, { 10, 11 } },  //
  { OP::LEQ, { 10, 11 } },  //
  { OP::MORE, { 10, 11 } }, //
  { OP::LESS, { 10, 11 } }, //
  { OP::ADD, { 20, 21 } },  //
  { OP::SUB, { 20, 21 } },  //
  { OP::MUL, { 30, 31 } },  //
  { OP::DIV, { 30, 31 } },  //
  { OP::MOD, { 30, 31 } },  //
};

// Tokens that can begin an operand -> juxtaposition is application. `do` is NOT
// here: a `do` block is only ever a primary (parse_prefix), never an argument
// by juxtaposition -- so `defer <cleanup> do <body>` reads `do` as the body
// opener, not as applying the cleanup to a do-block. (Use `f (do ...)` to pass
// one.)
const OperandSet operand_starters{
  LX::TokenTag::Int,  LX::TokenTag::Real,   LX::TokenTag::Str,
  LX::TokenTag::Word, LX::TokenTag::LParen, LX::TokenTag::KwLet,
  LX::TokenTag::KwIf,   LX::TokenTag::Lambda, LX::TokenTag::LBrace,
  LX::TokenTag::KwWhen,
};

// Tokens that end an expression. `do` ends one so `defer <cleanup> do ...`
// stops the cleanup at the `do`.
const OperandSet expr_terminators{
  LX::TokenTag::Eof,    LX::TokenTag::Dollar, LX::TokenTag::RParen,
  LX::TokenTag::Comma,  LX::TokenTag::KwIn,   LX::TokenTag::KwThen,
  LX::TokenTag::KwElse, LX::TokenTag::RBrace, LX::TokenTag::KwIs,
  LX::TokenTag::KwCtl,  LX::TokenTag::KwDo,
};

// Prefix (unary) operators, keyed by lexeme -> canonical OP name. The name is
// distinct from the lexeme so unary '-' (neg) never aliases binary '-'.
const UnopTable unop_db{
  { OP::SUB, OP::NEG },
  { OP::BANG, OP::NOT },
};

const char *tok_desc(const LX::Token &t);

} // namespace EX

#endif // EXxDATA_HEADER_
