/* Bison grammar for Thrax's surface syntax: a written standard and an LALR(1)
 * ambiguity check. NOT wired into the compiler -- the real parser is the
 * hand-written Pratt parser in compiler/EX.cpp. A Pratt parser resolves
 * ambiguity silently through binding powers; Bison reports it.
 *
 * Check with `build grammar-check` (silent pass; fails if the conflict count
 * changes) or `build grammar-check -v` (counterexamples). Pure grammar
 * analysis, no lexer/codegen; uses bison off PATH, else `nix shell`.
 *
 * TOKEN CONVENTIONS. This models the token stream, so two spellings the lexer
 * folds together are split, matching how EX.cpp branches on a token's first
 * character:
 *   - Word  -> UIDENT (uppercase-initial) / LIDENT (lowercase) / UNDERSCORE (_)
 *   - At (@name) and Op (a maximal-munch run) -> one terminal per significant
 *     spelling (AT_STRUCT, PLUS, CONS, PIPE_BACK, ...).
 * Operator precedence/associativity is transcribed from infix_db in
 * compiler/EXxDATA.hpp; the %left/%right/%precedence block below is that spec.
 *
 * CONFLICTS. `%expect 9`: nine shift/reduce, all resolve by the default
 * (shift), all matching EX.cpp. Not LALR(1) but unambiguous under maximal munch
 * -- EXPECTED, keep as is:
 *   - `f x.y`         postfix `.` binds tighter than application -> `f (x.y)`
 *   - `E.Tag.{...}`   the `.{...}` is the variant payload, not a struct literal
 *   - `A.B.C`         qualified-name chains shift greedily
 *   - `do ... ctl`    `ctl` attaches to the nearest `do`
 *   - `if`/`when`     `else` attaches to the nearest opener
 *   - nested `when`   arms belong to the innermost `when`
 * The one brace overlap tuples introduced -- `Tag: {A, B}` is the variant's
 * positional PAYLOAD fields, never a bare tuple type is resolved
 * structurally (payload_decl's bare-type alternative is non-brace-initial,
 * type_nb), matching EX.cpp's context rule; see payload_decl.
 *
 * DIVERGENCES from EX.cpp (this grammar is intentionally stricter):
 *   - SHOULD WARN (future work): EX.cpp lets a control form (let/if/when/\)
 *     be a bare operand/argument (operand_starters), body extending maximally
 *     right, so `f let x = 1 in x + 100` parses as `f (let x = 1 in (x+100))`
 *     -- the `+ 100` is swallowed. Legal, but a footgun that should emit a
 *     compiler warning. Here a control form in operand position needs parens.
 *   - LOWER PRIORITY: `defer` cleanup is `op_expr` here (EX.cpp allows a full
 *     expr up to `do`); `ext` and semantic well-formedness (field-case, mixed
 *     named/positional, arity) are omitted -- those are checks, not grammar. */

%define parse.error verbose
%expect 9

%token INT REAL STR
%token UIDENT       /* Word, uppercase-initial */
%token LIDENT       /* Word, lowercase-initial */
%token UNDERSCORE   /* _ */
%token TYVAR        /* `a */

%token AT_MOD AT_STRUCT AT_UNION AT_ALIAS AT_EFFECT AT_OPERATOR AT_ASSERT
%token AT_PRIVATE AT_PUBLIC AT_EXTERN AT_ARRAY
%token AT_TYCON     /* @int64 / @float64 / @str ... */

%token LPAREN RPAREN LBRACE RBRACE LBRACK RBRACK
%token LAMBDA       /* \ */
%token EQ ARROW DOLLAR COLON COMMA DOT

%token KW_LET KW_IN KW_IF KW_WHEN KW_IS KW_THEN KW_ELSE KW_WITH
%token KW_DO KW_CTL KW_DEFER

%right SEMI                         /* ; */
%right PIPE_BACK                    /* <| */
%left  PIPE_FWD                     /* |> */
%left  EQEQ LT GT LE GE             /* == < > <= >= */
%right CONS                         /* :: */
%left  CONCAT                       /* ++ */
%left  PLUS MINUS
%left  STAR SLASH PERCENT
%precedence NEG                     /* unary - ! */
%precedence APP                     /* application */
%left  DOT                          /* postfix . */

%token BANG QEQ QGT QLT             /* ! ?= ?> ?< */
%token ANGLE_EMPTY                  /* <> */
%token BAR                          /* | */

%start program
%%

program   : mod_decl globals ;
mod_decl  : AT_MOD UIDENT ;

globals   : /* empty */ | globals global ;

global
  : DOLLAR LIDENT opt_sig EQ body
  | DOLLAR LIDENT COLON AT_STRUCT EQ struct_body
  | DOLLAR LIDENT COLON AT_UNION  EQ union_body
  | DOLLAR LIDENT COLON AT_ALIAS  EQ type
  | DOLLAR LIDENT COLON AT_EFFECT EQ effect_body
  | DOLLAR KW_WITH import
  | DOLLAR AT_PRIVATE
  | DOLLAR AT_PUBLIC
  | DOLLAR AT_ASSERT expr
  | DOLLAR AT_OPERATOR DOT LBRACE overloadable_op RBRACE COLON type EQ expr
  ;

opt_sig   : /* empty */ | COLON type ;
body      : expr | extern_lit ;

overloadable_op
  : PLUS | MINUS | STAR | SLASH | PERCENT | CONCAT
  | EQEQ | LT | GT | LE | GE
  | QEQ  | QGT | QLT
  ;

import       : dotted_name | dotted_name EQ dotted_name ;
dotted_name  : any_name | any_name DOT any_name ;
any_name     : UIDENT | LIDENT ;

struct_body   : field_decls opt_comma ;
field_decls   : field_decl | field_decls COMMA field_decl ;
field_decl    : LIDENT COLON type ;

union_body    : variant_decls opt_comma ;
variant_decls : variant_decl | variant_decls COMMA variant_decl ;
variant_decl  : UIDENT COLON payload_decl ;

/* After `Tag:` a '{' ALWAYS opens the payload's field braces (`{}` empty,
 * `{A, B}` two positional fields) never a bare tuple type; that is how
 * EX.cpp reads it. A single tuple-typed field is written nested
 * (`Tag: {{A, B}}`) or named (`Tag: {t: {A, B}}`), so the bare-type
 * alternative is restricted to non-brace-initial types (type_nb). */
payload_decl
  : LBRACE RBRACE
  | LBRACE payload_fields opt_comma RBRACE
  | type_nb
  ;

payload_fields : payload_field | payload_fields COMMA payload_field ;
payload_field  : type | LIDENT COLON type ;

type_nb      : type_app_nb | type_app_nb ARROW type | type_app_nb ARROW eff_row type ;
type_app_nb  : type_atom_nb | type_app_head type_args ;
type_atom_nb
  : UIDENT
  | UIDENT DOT UIDENT
  | AT_TYCON
  | TYVAR
  | LPAREN type RPAREN
  ;

effect_body : op_decls opt_comma ;
op_decls    : op_decl | op_decls COMMA op_decl ;
op_decl     : LIDENT COLON type ;

opt_comma   : /* empty */ | COMMA ;

type
  : type_app
  | type_app ARROW type
  | type_app ARROW eff_row type
  ;

type_app      : type_atom | type_app_head type_args ;
type_app_head : UIDENT | UIDENT DOT UIDENT ;
type_args     : type_atom | type_args type_atom ;

type_atom
  : UIDENT
  | UIDENT DOT UIDENT
  | AT_TYCON
  | TYVAR
  | LBRACE RBRACE
  | LBRACE type_list opt_comma RBRACE  /* tuple type; n >= 1 ({} is unit) */
  | LPAREN type RPAREN
  ;

type_list : type | type_list COMMA type ;

eff_row
  : ANGLE_EMPTY
  | LT TYVAR GT
  | LT eff_names GT
  | LT eff_names BAR TYVAR GT
  | PIPE_BACK TYVAR GT
  ;

eff_names : UIDENT | eff_names COMMA UIDENT ;

expr      : ctrl_expr | op_expr ;

ctrl_expr
  : KW_LET let_binder EQ expr KW_IN expr
  | KW_IF expr KW_THEN expr KW_ELSE expr
  | KW_WHEN expr arms KW_ELSE expr
  | LAMBDA params EQ expr
  | KW_DEFER op_expr handle
  | handle
  ;

let_binder
  : LIDENT opt_sig
  | LBRACE pat_elem_list opt_comma RBRACE  /* tuple pattern; n >= 1 */
  | UIDENT DOT LBRACE field_pats RBRACE
  | UIDENT DOT UIDENT pat_payload
  | UIDENT DOT UIDENT DOT UIDENT pat_payload
  ;

op_expr
  : op_expr SEMI op_expr
  | op_expr PIPE_BACK op_expr
  | op_expr PIPE_FWD op_expr
  | op_expr EQEQ op_expr
  | op_expr LT op_expr
  | op_expr GT op_expr
  | op_expr LE op_expr
  | op_expr GE op_expr
  | op_expr CONS op_expr
  | op_expr CONCAT op_expr
  | op_expr PLUS op_expr
  | op_expr MINUS op_expr
  | op_expr STAR op_expr
  | op_expr SLASH op_expr
  | op_expr PERCENT op_expr
  | MINUS op_expr %prec NEG
  | BANG  op_expr %prec NEG
  | app
  ;

app  : atom | app atom %prec APP ;

atom
  : INT
  | REAL
  | STR
  | LIDENT
  | UIDENT
  | LPAREN expr RPAREN
  | LBRACE RBRACE
  | LBRACE elem_list opt_comma RBRACE  /* tuple literal; n >= 1 ({} is unit) */
  | seq_lit
  | array_lit
  | DOT LBRACE field_inits RBRACE
  | DOT UIDENT variant_payload
  | atom DOT LIDENT
  | atom DOT INT   /* positional tuple access `t.0`; a chained `t.0.1`  */
  | atom DOT REAL  /* arrives as one REAL token and is split at its '.' */
  | atom DOT UIDENT variant_payload
  | atom DOT LBRACE field_inits RBRACE
  ;

field_inits : /* empty */ | init_list opt_comma ;
init_list   : field_init | init_list COMMA field_init ;
field_init  : DOT LIDENT EQ expr | expr ;

variant_payload : /* empty */ | DOT LBRACE field_inits RBRACE ;

seq_lit   : LBRACK RBRACK | LBRACK elem_list opt_comma RBRACK ;
elem_list : expr | elem_list COMMA expr ;

array_lit
  : AT_ARRAY DOT LBRACE expr opt_comma RBRACE
  | AT_ARRAY DOT LBRACE DOT LIDENT EQ expr opt_comma RBRACE
  ;

/* `@extern "C" "symbol" "lib"`: ABI, symbol, then SYMBOLIC library name --
 * never a path or soname (resolution is the consumer's job: dlopen table in
 * the interpreter, link line in the native backend). */
extern_lit : AT_EXTERN STR STR STR ;

handle
  : KW_DO expr
  | KW_DO expr KW_CTL LIDENT clauses opt_else_clause
  ;

clauses         : clause | clauses clause ;
clause          : KW_IS op_ref LIDENT EQ expr ;
op_ref          : LIDENT | UIDENT DOT LIDENT ;
opt_else_clause : /* empty */ | KW_ELSE LIDENT EQ expr ;

arms : arm | arms arm ;
arm
  : KW_IS pattern KW_THEN expr
  | KW_IS pattern KW_IF expr KW_THEN expr
  ;

params : pattern | params pattern ;

pattern
  : pat_atom
  | pat_atom CONS pattern
  | STR CONCAT pattern
  ;

pat_atom
  : UNDERSCORE
  | LIDENT
  | INT
  | REAL
  | STR
  | list_pattern
  | LBRACE pat_elem_list opt_comma RBRACE  /* tuple pattern; n >= 1 */
  | DOT UIDENT pat_payload
  | UIDENT DOT LBRACE field_pats RBRACE
  | UIDENT DOT UIDENT pat_payload
  | UIDENT DOT UIDENT DOT UIDENT pat_payload
  ;

pat_payload  : /* empty */ | DOT LBRACE field_pats RBRACE ;

list_pattern : LBRACK RBRACK | LBRACK pat_elems RBRACK ;
pat_elems
  : pat_elem_list opt_comma
  | pat_elem_list COMMA DOT DOT pattern
  | DOT DOT pattern
  ;
pat_elem_list : pattern | pat_elem_list COMMA pattern ;

field_pats     : /* empty */ | field_pat_list opt_comma ;
field_pat_list : field_pat | field_pat_list COMMA field_pat ;
field_pat
  : DOT LIDENT EQ pattern
  | DOT LIDENT
  | pattern
  ;

%%
