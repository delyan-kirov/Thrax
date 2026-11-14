/* Bison grammar for Ares's surface syntax: a written standard and an LALR(1)
 * ambiguity check. NOT wired into the compiler -- the real parser is the
 * hand-written Pratt parser in crates/frontend/src/parser.rs. A Pratt parser resolves
 * ambiguity silently through binding powers; Bison reports it.
 *
 * Check with `thrax grammar` (silent pass; fails if the conflict count changes)
 * or `thrax grammar -v` (counterexamples). Pure grammar analysis, no
 * lexer/codegen; bison comes from the dev shell (flake.nix).
 *
 * TOKEN CONVENTIONS. This models the token stream, so two spellings the lexer
 * folds together are split, matching how parser.rs branches on a token's first
 * character:
 *   - Word  -> UIDENT (uppercase-initial) / LIDENT (lowercase) / UNDERSCORE (_)
 *   - At (@name) and Op (a maximal-munch run) -> one terminal per significant
 *     spelling (AT_STRUCT, PLUS, CONS, PIPE_BACK, ...).
 * Operator precedence/associativity is transcribed from the OpRole table in
 * crates/frontend/src/lexer/data.rs; the %left/%right/%precedence block below
 * is that spec.
 *
 * CONFLICTS. `%expect 3`: bison's summary count, all resolving by the default
 * (shift), all matching parser.rs. Each is the dangling-else kind: nothing
 * closes a run of arms, so the grammar has to pick, and it picks the
 * innermost -- EXPECTED, keep as is.
 *   - nested arms     arms belong to the innermost multi-arm `\ |` or handler
 *                     (parenthesize to nest); the real parser also ends the
 *                     arms at a `| op x` operation clause, which this grammar
 *                     does not model; two
 *   - `do ... ctl`    `ctl` attaches to the nearest `do`
 * Everything else that would conflict is ruled out by the grammar instead: a
 * splice is a prefix operator, never an argument (see app); an argument and a
 * later lambda parameter do not start with `.` (see app, params); a qualified
 * name, a variant and its payload are postfix `.` steps (see postfix).
 * The one brace overlap tuples introduced -- `Tag: {A, B}` is the variant's
 * positional PAYLOAD fields, never a bare tuple type is resolved
 * structurally (payload_decl's bare-type alternative is non-brace-initial,
 * app_nb), matching parser.rs's context rule; see payload_decl.
 *
 * ONE GRAMMAR FOR TYPES AND EXPRESSIONS (#406 step 2). A type is an `expr`:
 * signatures, annotations, fields, payloads, row arguments. The merge kept
 * the meaning of every corpus program; it admits more than parser.rs: an operator,
 * a lowercase head (`m a`) or a control form in a type position parses here
 * and is rejected by parser.rs, whose type parser stays separate until the
 * elaborator takes types as expressions (step 3).
 *
 * DIVERGENCES from parser.rs (this grammar is intentionally stricter):
 *   - SHOULD WARN (future work): parser.rs lets a control form (let/if/\)
 *     be a bare operand/argument (starts_operand), body extending maximally
 *     right, so `f let x = 1 in x + 100` parses as `f (let x = 1 in (x+100))`
 *     -- the `+ 100` is swallowed. Legal, but a footgun that should emit a
 *     compiler warning. Here a control form in operand position needs parens.
 *   - LOWER PRIORITY: semantic well-formedness (field-case, mixed
 *     named/positional, arity, the one size of `@Array.{ n }`) are omitted --
 *     those are checks, not grammar. */

%define parse.error verbose
%expect 3

%token INT REAL IMAG STR   /* IMAG: an `i`-suffixed number, `3i` / `1.2i` / `2.5e-2i` */
%token UIDENT       /* Word, uppercase-initial */
%token LIDENT       /* Word, lowercase-initial */
%token UNDERSCORE   /* _ */
%token HOLE         /* ??? a typed hole: any type, always a compile error naming it */

%token AT_MOD AT_STRUCT AT_UNION AT_ALIAS AT_EFFECT AT_MAIN AT_BUILD
%token AT_USE AT_IFACE  /* AT_USE: `@use`; `@ctx` is reserved for
                          the unified binder of #406 */
%token AT_PRIVATE AT_EXTERN AT_ARRAY AT_IMPORT
%token AT_TRUE AT_FALSE /* the two `@Bool` literals (there is no `true`/`false` alias) */
%token AT_TYCON     /* any other @name: a built-in type (@Int64, @Str, @Vec) or
                       effect (@IO) in a type, an intrinsic (@vec_len) as a value */

%token LPAREN RPAREN LBRACE RBRACE LBRACK RBRACK
%token LAMBDA       /* \ */
%token EQ ARROW FATARROW DOLLAR COLON COMMA DOT  /* FATARROW is `=>` */

%token KW_LET KW_IN KW_IF KW_ELSE
%token KW_DO KW_CTL

%right SEMI                         /* ; */
%right PIPE_BACK                    /* <| */
%left  PIPE_FWD                     /* |> */
%left  OR_OR                        /* || (short-circuit, desugars to a lazy if) */
%left  AND_AND                      /* && (short-circuit, desugars to a lazy if) */
%left  EQEQ NEQ LT GT LE GE         /* == != < > <= >= */
%right CONS                         /* :: */
%left  CONCAT                       /* ++ */
%left  PLUS MINUS BAR3 CARET3      /* ||| ^^^ bitwise or, xor */
%left  STAR SLASH PERCENT AMP3 SHL3 SHR3  /* &&& <<< >>> bitwise and, shifts */
%right CARET                        /* ^ exponentiation */
%right COMPOSE                      /* <|> composition (desugars to `\x => f (g x)`) */
%precedence NEG                     /* unary - ! ~~~ and the splice & */
%precedence APP                     /* application */
%left  DOT                          /* postfix . */

%token BANG                         /* ! */
%token TILDE3                       /* ~~~ bitwise complement */
%token AMP                          /* & build-time splice */
%token ANGLE_EMPTY                  /* <> */
%token BAR                          /* | */
%token ELLIPSIS                     /* ... inclusive range (patterns + `t.[p...q]` slices) */

%start program
%%

program   : mod_decl globals ;
mod_decl  : DOLLAR AT_MOD UIDENT ;

globals   : /* empty */ | globals global ;

global
  : DOLLAR LIDENT opt_sig EQ body
  | DOLLAR UIDENT COLON AT_STRUCT opt_type_params EQ struct_body
  | DOLLAR UIDENT COLON AT_STRUCT AT_EXTERN STR opt_type_params EQ struct_body  /* C layout */
  | DOLLAR UIDENT COLON AT_UNION  opt_type_params EQ union_body
  | DOLLAR UIDENT COLON AT_UNION  opt_type_params EQ  /* no variants: uninhabited */
  | DOLLAR UIDENT COLON AT_UNION  AT_EXTERN STR opt_type_params EQ union_body  /* C layout */
  | DOLLAR UIDENT COLON AT_ALIAS  opt_type_params EQ expr
  | DOLLAR UIDENT COLON AT_EFFECT opt_type_params EQ effect_body
  | DOLLAR AT_IMPORT import
  | DOLLAR AT_PRIVATE
  | DOLLAR AT_MAIN COLON expr EQ expr  /* the entry: `@Vec @Str -> <@IO> @Int` */
  /* The compile-time entry, run during compilation; its `@Code` result adds
   * items to this module. Signature: `{} -> <@Meta, @IO> @Code`. */
  | DOLLAR AT_BUILD COLON expr EQ expr
  /* A blessed interface TYPE, declarable only in CORE: the compiler resolves a
   * value of it at a desugar site and calls its single field. AT_IFACE is one of
   * `@IIntLit @IRealLit @IImagLit @IStrLit @ISeqLit @IRange @IRangeFrom @IIndex
   * @ISlice @ISeqView @ISeqViewBack @IInterp @IInterpText @IInterpHole`. */
  | DOLLAR AT_IFACE COLON AT_STRUCT opt_type_params EQ struct_body
  /* A builtin, declarable only in CORE: the name is the runtime primitive
   * (`$ @iadd : t -> t -> t = @extern "ares"`). */
  | DOLLAR AT_TYCON COLON expr EQ builtin_lit
  | DOLLAR splice  /* `$ &assert ..`: the items the code holds replace it */
  | DOLLAR LPAREN operator_name RPAREN COLON expr EQ expr  /* `$ (+) : T = e` */
  ;

/* A signature is an expression, like every type; `@use` parameters stand on
 * its arrows (see use_param). `=` is never an operator, so it ends the type. */
opt_sig   : /* empty */ | COLON expr ;
body      : expr | extern_lit ;

/* Every operator that may stand alone in parens: the name a `$ (op) : T = e`
 * definition binds, and the function value `(op)` refers to. The grammatical
 * delimiters (`|`, `<>`) are excluded: they are not operators. */
operator_name
  : PLUS | MINUS | STAR | SLASH | PERCENT | CARET | BANG | CONCAT | CONS
  | AMP3 | BAR3 | CARET3 | SHL3 | SHR3 | TILDE3
  | EQEQ | NEQ | LT | GT | LE | GE
  | AND_AND | OR_OR | SEMI | PIPE_FWD | PIPE_BACK | COMPOSE
  ;

import       : dotted_name | any_name EQ dotted_name ;  /* `$ @import V = VEC`: qualified only */
dotted_name  : any_name | any_name DOT any_name ;
any_name     : UIDENT | LIDENT ;

/* Type parameters after `@struct`/`@union`/`@alias`: `@struct a b = ...`.
 * They are mandatory: every type variable used in the body must be declared here.
 * `opt_` only because a non-generic type declares none (an empty list). */
opt_type_params : /* empty */ | opt_type_params LIDENT ;

struct_body   : field_decls opt_comma ;
field_decls   : field_decl | field_decls COMMA field_decl ;
field_decl    : LIDENT COLON expr | entry_splice ;

union_body    : variant_decls opt_comma ;
variant_decls : variant_decl | variant_decls COMMA variant_decl ;
variant_decl  : UIDENT COLON payload_decl | entry_splice ;
/* `&X`: X builds the `@Code` of fields or variants, checked and spliced in
 * place of the entry before the final compile. */
entry_splice  : splice ;

/* `&X`, the build-time splice, valid in every position: X runs at compile time
 * and its result replaces the site, `@Code` spliced in as source and
 * first-order data inserted as a literal. Syntax, not a function: no section
 * `(&)` and no `$ (&)` definition. The operand binds like a prefix operator's,
 * so it is an application and stops before an infix operator, a `,`, `=>` or a
 * row's `>`. The lexer splits a trailing `&` off an operator run that is
 * otherwise unknown, so `<&Logging>` lexes as `<` `&`. */
splice : AMP app ;

/* After `Tag:` a '{' ALWAYS opens the payload's field braces (`{}` empty,
 * `{A, B}` two positional fields) never a bare tuple type; that is how
 * parser.rs reads it. A single tuple-typed field is written nested
 * (`Tag: {{A, B}}`) or named (`Tag: {t: {A, B}}`), so the bare-type
 * alternative is an application or arrow whose head is not brace-initial
 * (app_nb, atom_nb). */
payload_decl
  : LBRACE RBRACE
  | LBRACE payload_fields opt_comma RBRACE
  | app_nb
  | app_nb ARROW expr
  | app_nb ARROW eff_row expr
  ;

payload_fields : payload_field | payload_fields COMMA payload_field ;
payload_field  : expr | LIDENT COLON expr ;

effect_body : op_decls opt_comma ;
op_decls    : op_decl | op_decls COMMA op_decl ;
op_decl     : LIDENT COLON expr ;

opt_comma   : /* empty */ | COMMA ;

/* A searched parameter: `@use {d: IOrd t} -> t -> t -> t`. A closed record of
 * named fields, each an interface instance (or, with dependent types, a
 * proposition) that call sites do not write: the compiler finds each field by
 * its type in scope. The fields are in scope by name in the body, like a
 * named-record parameter's, so the body's lambda starts at the first explicit
 * parameter. Any number, anywhere in a signature; field names are unique
 * across a signature's `@use` parameters, and there is at least one (`@use {}`
 * is an error). Legal only on the spine of a global's signature, never
 * inside a parameter's type, a lambda or ascription annotation, or a field
 * type, which the parser enforces: the grammar admits `(@use {d: I t} -> t) -> t`,
 * parser.rs rejects it. A local `let` signature does not take `@use` yet. */
use_param  : AT_USE LBRACE use_fields opt_comma RBRACE ;
use_fields : LIDENT COLON expr | use_fields COMMA LIDENT COLON expr ;

rec_field_list
  : rec_field
  | rec_field_list COMMA rec_field
  ;

rec_field : LIDENT COLON expr ;

eff_row
  : ANGLE_EMPTY
  | LT LIDENT GT
  | LT eff_names GT
  | LT eff_names BAR LIDENT GT
  | PIPE_BACK LIDENT GT
  ;

/* An entry is an effect applied to its arguments (`Chan @Int e`), or
 * entries a generator produces (`&X`, a whole entry). The arguments are
 * atoms, so a `>` inside one is parenthesized. A row has no alias. */
eff_names : row_entry | eff_names COMMA row_entry ;
row_entry : eff_entry | splice ;
eff_entry : UIDENT | AT_TYCON | eff_entry arg ;

/* Types are expressions (#406): there is no separate type grammar, and a
 * signature, an annotation, a field or a payload is an `expr` like any other.
 * Which expressions denote types is the checker's business. */
expr
  : ctrl_expr
  | op_expr
  /* The lowest, right-associative operators take a whole expression on
   * their right, so a control form may follow and run to the end:
   * `a; let x = 1 in b`, `f <| g <| \x => ..`. `|>` is left-associative, so
   * only its last operand may be a control form: `xs |> f |> \v => ..`, and a
   * match is `s |> \ | p => e | ...`. */
  | op_expr SEMI expr
  | op_expr PIPE_BACK expr
  | op_expr PIPE_FWD ctrl_expr
  /* The function type, at that same lowest level: `a -> b -> c` is
   * `a -> (b -> c)`, and `a |> f -> b` is `(a |> f) -> b`. An effect row may
   * follow the arrow (`a -> <@IO> b`); `<` and `<|` cannot start an operand,
   * so the row is unambiguous. A `@use` parameter stands only left of an
   * arrow. An ascribed name left of an arrow names the parameter,
   * `(x: A) -> B`; in a signature the body sees `x` bound. */
  | op_expr ARROW expr
  | op_expr ARROW eff_row expr
  | use_param ARROW expr
  | use_param ARROW eff_row expr
  ;

ctrl_expr
  /* Each form takes ONE binding, subject or arm; several nest:
   * `let x = a in let y = b in e`, `if a => b else if c => d else e`. */
  : KW_LET let_binder EQ expr KW_IN expr
  | KW_IF expr FATARROW expr KW_ELSE expr
  | LAMBDA params FATARROW expr
  /* The multi-arm lambda: its arguments matched against the arms, top to
   * bottom, one argument per comma-separated pattern (`\ | 0, xs => ...` takes
   * two). One pattern is one argument, so a match is `s |> \ | p => a | q => b`,
   * which the checker elaborates as a match on `s`. An empty match is
   * `x |> @absurd` (a CORE function), not a lambda with no arms. */
  | LAMBDA arms
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
  : op_expr PIPE_FWD op_expr
  | op_expr OR_OR op_expr            /* || short-circuit -> lazy if */
  | op_expr AND_AND op_expr          /* && short-circuit -> lazy if */
  | op_expr EQEQ op_expr
  | op_expr NEQ op_expr
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
  | op_expr AMP3 op_expr
  | op_expr SHL3 op_expr
  | op_expr SHR3 op_expr
  | op_expr BAR3 op_expr
  | op_expr CARET3 op_expr
  | op_expr CARET op_expr
  | op_expr COMPOSE op_expr
  | MINUS op_expr %prec NEG
  | BANG  op_expr %prec NEG
  | TILDE3 op_expr %prec NEG
  | AMP   op_expr %prec NEG
  | app
  ;

/* Application. The head of an application is any atom (`m a` with a
 * variable `m`, `p x` with `p : A -> @Type`), not only a type name; a sized
 * tensor is the builtin `@Tensor` applied, `@Tensor n a`. An argument is an
 * `arg`: an atom that does not start with `.` (`f x .Tag` would otherwise
 * read as `f (x.Tag)` or as two arguments), so `f (.Tag)` parenthesizes. A
 * splice is never an argument: `&` is a prefix operator (see op_expr), so
 * `@Vec (&f a)`. */
app    : atom | app arg %prec APP ;
app_nb : atom_nb | app_nb arg %prec APP ;

/* An atom is a primary followed by a postfix `.` chain. Two restricted
 * copies differ only in the head: `atom_nb` is not brace-initial (a variant's
 * bare payload type, see payload_decl) and `arg` is not dot-initial. */
atom    : primary    | atom DOT postfix ;
atom_nb : primary_nb | atom_nb DOT postfix ;
arg     : arg_primary | arg DOT postfix ;

primary      : plain | dot_primary | brace_atom ;
primary_nb   : plain | dot_primary ;
arg_primary  : plain | brace_atom ;

plain
  : INT
  | REAL
  | IMAG   /* resolves `@IImagLit <type>` and calls its field on the magnitude;
            * deliberately absent from `pat_atom` (a pattern holds no call) */
  | STR
  | AT_TRUE   /* @Bool literals; the ONLY spelling (no bare `true`/`false`) */
  | AT_FALSE
  | LIDENT    /* a variable: a value, a type or anything else in scope */
  | UIDENT
  | HOLE
  | AT_TYCON  /* a builtin type (`@Int`) or effect (`@IO`), or an intrinsic
               * as a value: `@vec_len v`, `@iadd a b` */
  | AT_IFACE  /* a blessed interface type: `@IIndex (@Tensor n a) @Int a` */
  | LPAREN expr RPAREN
  | LPAREN operator_name RPAREN  /* an operator as a function value: `(+) 1 1` */
  | LPAREN expr COLON expr RPAREN  /* a type ascription: `([] : List @Int)` */
  /* An explicit `@use` argument, matched to the signature's `@use` fields by
   * name; omitted fields are still searched: `max_of @use {.d = flip} 3 7`.
   * At least one field: an empty `@use {}` supplies nothing, so it is an error.
   * It precedes the explicit argument that follows its parameter. */
  | AT_USE LBRACE use_inits opt_comma RBRACE
  | seq_lit
  /* The byte-array type. `@Array.{ n }` is its literal, the typed struct
   * literal postfix on the type like `Person.{ ... }`; parser.rs checks that
   * it holds one size (`n` or `.size = n`). */
  | AT_ARRAY
  ;

dot_primary
  : DOT LBRACE struct_lit_body RBRACE
  | DOT UIDENT   /* a bare variant `.Tag`; its payload is the postfix `.{ .. }` */
  ;

brace_atom
  : LBRACE dot_inits opt_comma RBRACE  /* a record or codata literal `{ .f = e }` */
  | LBRACE dot_inits BAR expr RBRACE   /* a record update `{ .f = e | base }` */
  | LBRACE dot_inits COMMA DOT DOT op_expr RBRACE  /* extension `{ .tag = 1, ..p }` */
  | LBRACE DOT DOT op_expr RBRACE      /* `{ ..p }`: p's fields, as a record */
  | LBRACE RBRACE                      /* the unit value, and its type */
  | LBRACE elem_list opt_comma RBRACE  /* a tuple, or a tuple type `{A, B}`; n >= 1 */
  /* A record type `{x: T, y: U}`: as a parameter it is the tuple `{T, U}`
   * whose field names bind the argument; a one-field `{x: T}` collapses to
   * `T` (see parse_global). */
  | LBRACE rec_field_list opt_comma RBRACE
  | LBRACE rec_field_list BAR LIDENT RBRACE  /* open record type `{ x: T | r }` */
  ;

postfix
  : LIDENT
  | INT   /* `.n`: positional tuple/struct field `t.0`; chained `t.0.1` */
  | REAL  /* arrives as one REAL token and is split at its '.' */
  | UIDENT   /* a qualified name `M.x`, a variant `E.Tag`, a chain `M.E.Tag` */
  | LBRACE struct_lit_body RBRACE  /* `T.{ .. }`, a variant's `E.Tag.{ .. }` */
  | LBRACK index_list RBRACK   /* `t.[i]` / `t.[i, j]`: modular indexing */
  ;

/* `t.[i, j, ...]`: a comma-list of axis indices, folded to nested single-axis
 * indexing (`t.[i].[j]`). */
/* Index slots: an `expr` reduces its axis (modular indexing); `p ... q` keeps it,
 * narrowed to the inclusive range; `..` keeps it whole. An all-`expr` access is
 * the `@IIndex` interface; any range/`..` slot makes it a shape-checked tensor slice
 * (a view). `..` is two `DOT`s (there is no `..` token). */
index_list : index_slot | index_list COMMA index_slot ;
index_slot : expr | expr ELLIPSIS expr | DOT DOT ;

field_inits : /* empty */ | init_list opt_comma ;
init_list   : field_init | init_list COMMA field_init ;
field_init  : DOT LIDENT EQ expr | expr ;

/* A struct literal may end with a record update `| base`: it copies every
   unlisted field from `base` (an expression of the same struct type) and must be
   the final entry. Works for both the qualified `Person.{ .f = e | base }` and the
   bare `.{ | base }` form; the bare form's type is settled at its lit site from
   context, like any bare literal. */
struct_lit_body
  : field_inits
  | init_list BAR expr
  | BAR expr
  ;

seq_lit
  : LBRACK RBRACK
  | LBRACK elem_list opt_comma RBRACK
  | LBRACK expr ELLIPSIS expr RBRACK   /* range builder `[lo ... hi]` -> `range lo hi` */
  | LBRACK expr ELLIPSIS RBRACK        /* open range `[lo ...]` -> `count_from lo` (Stream) */
  ;
elem_list : expr | elem_list COMMA expr ;
dot_inits
  : DOT LIDENT EQ expr
  | dot_inits COMMA DOT LIDENT EQ expr
  ;

use_inits : DOT LIDENT EQ expr | use_inits COMMA DOT LIDENT EQ expr ;

/* `@extern "C" "symbol" "lib"`: ABI, symbol, then SYMBOLIC library name --
 * never a path or soname (resolution is the consumer's job: dlopen table in
 * the interpreter, link line in the native backend). */
extern_lit : AT_EXTERN STR STR STR | builtin_lit ;

/* `@extern "ares"`: the definition is the runtime builtin of its own name, and
 * only CORE may write it. `@extern "load"`: a load from C memory, typed by the
 * binding (`{ p: @Ptr, i: @Int } -> T`). The one STR must be one of the two. */
builtin_lit : AT_EXTERN STR ;

handle
  : KW_DO expr
  | KW_DO expr KW_CTL LIDENT handler_arms
  ;

/* A handler mixes operation clauses `| op x => e` with value arms `| pat => e`
 * over the body's final value. A pattern never juxtaposes two names and a
 * qualified pattern's second segment is an uppercase tag, so `BAR LIDENT LIDENT`
 * and `BAR UIDENT DOT LIDENT` always open a clause. */
handler_arms : handler_arm | handler_arms handler_arm ;
handler_arm  : clause | arm ;
clause       : BAR op_ref LIDENT FATARROW expr ;
op_ref       : LIDENT | UIDENT DOT LIDENT ;

/* There is no default branch: a catch-all is an ordinary `| _ => e` arm. */
arms : arm | arms arm ;

/* An arm's `| pat` alternatives (or-patterns) share the body; the parser
   emits one arm per alternative. */
alts : BAR arm_pats | alts BAR arm_pats ;

/* One pattern per argument. The comma SEPARATES arguments and never builds a
   tuple (a tuple pattern is `{ a, b }`, one argument): a bare comma builds
   nothing anywhere in Ares, so it stays free to separate. Every alternative
   of every arm lists the same number of patterns, the lambda's arity; a
   handler's value arms take exactly one. parser.rs checks both counts. */
arm_pats : pattern | arm_pats COMMA pattern ;

arm
  : alts FATARROW expr
  | alts KW_IF expr FATARROW expr
  ;

/* A lambda's parameters, juxtaposed. An annotated parameter is ALWAYS
   parenthesized, `\(x: T) => e`, even when it is the only one: the annotation
   then ends at `)`, never at a `=>` (types become expressions with #406, and an
   `if` arm inside a type has its own `=>`). `\x: T => e` is an error.
   Only the first parameter may start with `.` (`\.Tag .{ x }` would read as
   one parameter or two), and a splice parameter is parenthesized, since its
   operand would take the parameters after it: `\(&f a) b => e`. */
params : param | pat_dot | params param ;

param : pat_plain | LPAREN pattern RPAREN | LPAREN pattern COLON expr RPAREN ;

/* `++` splits a sequence into fixed pieces (string literals, `[..]` lists)
 * around at most one part of unknown length: `"a" ++ r`, `r ++ [z]`,
 * `[a] ++ m ++ "z"`. `h :: t` is `[h] ++ t`. The one-hole rule is checked by
 * the parser, not encoded here. */
pattern
  : pat_atom
  | pat_atom CONS pattern
  | pat_atom CONCAT pattern
  | num_lit ELLIPSIS num_lit   /* inclusive numeric range `lo ... hi` */
  | num_lit ELLIPSIS           /* open range `lo ...` (matches `lo <= x`) */
  ;

num_lit : INT | REAL | MINUS INT | MINUS REAL ;  /* `-` only directly before a literal */

pat_atom : pat_plain | pat_dot | splice ;  /* splice: a constant (`&answer`)
                                            or a pattern a generator produces */
pat_dot  : DOT UIDENT pat_payload ;

pat_plain
  : UNDERSCORE
  | LIDENT
  | num_lit
  | STR
  | AT_TRUE   /* @Bool literal patterns; lowercase true/false would bind */
  | AT_FALSE
  | list_pattern
  | LBRACE pat_elem_list opt_comma RBRACE  /* tuple pattern; n >= 1 */
  | LBRACE rec_pats RBRACE                 /* record pattern `{ .x = a, .y, .._ }` */
  | UIDENT DOT LBRACE field_pats RBRACE
  | UIDENT DOT UIDENT pat_payload
  | UIDENT DOT UIDENT DOT UIDENT pat_payload
  ;

pat_payload  : /* empty */ | DOT LBRACE field_pats RBRACE ;

rec_pats : rec_pat | rec_pats COMMA rec_pat ;
rec_pat  : DOT LIDENT EQ pattern | DOT LIDENT | DOT DOT pattern ;

list_pattern : LBRACK RBRACK | LBRACK pat_elem_list opt_comma RBRACK ;
pat_elem_list : pattern | pat_elem_list COMMA pattern ;

field_pats     : /* empty */ | field_pat_list opt_comma ;
field_pat_list : field_pat | field_pat_list COMMA field_pat ;
field_pat
  : DOT LIDENT EQ pattern
  | DOT LIDENT
  | pattern
  ;

%%
