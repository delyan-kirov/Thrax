# Thrax Language Reference

Every language feature, one entry each, with a simple example and a more
involved one. The examples use the language's real, current surface syntax
(branches are `if c => t else e`, matches are `is scrut | pat => e ... else d`).
Where a feature is only partly built, the entry says so.

Every code block below is self-contained and type-checks on its own (verified
with `thrax check`); a block that needs a library imports it (`$ with LA`, etc.).

Conventions used throughout: a global is `$ name : Type = expr`; a lambda is
`\x = e` (curried: `\a b = e`); a branch is `if c => t else e`; a match is
`is scrut | pat => e ...`. The comparison operators are `==` (equal),
`!=` (not equal), `<` (less), `>` (greater), `<=` (at most), `>=` (at least),
each of type `a -> a -> @Bool`.

---

## Table of contents

1. Lexical structure
2. Program structure (modules, globals, imports, entry point)
3. Types
4. Expressions and syntactic sugar
5. Pattern matching
6. Algebraic data types (structs, unions, recursion)
7. Functions (currying, higher-order, interfaces, TCO)
8. Algebraic effects and handlers
9. Sized tensors and linear algebra
10. Foreign function interface
11. Compile-time evaluation
12. Intrinsic types and primitives
13. The `@`-sigil catalogue
14. Compiler CLI
15. Standard library modules

---

# 1. Lexical structure

## 1.1 Comments
A `#` starts a comment to end of line. There is no block comment.

```thrax
$ x : @Int = 1   # trailing comment
# whole-line comment
```

## 1.2 Integer literals
Decimal, hexadecimal (`0x`), and binary (`0b`).

```thrax
$ a = 255
$ b = 0xFF
$ c = 0b1010
$ mix = 0x10 + 0b10 + 8      # bases mix freely in arithmetic
```

## 1.3 Real literals
Decimal point or scientific notation.

```thrax
$ pi   : Real = 3.14159
$ tiny : Real = 12e-3        # 0.012
$ mixed = 1 + 1.0            # a literal takes the type its context wants
```

## 1.4 Imaginary literals
An `i` suffix on any numeric literal form makes it imaginary. The suffixed
literal is not a base type: it desugars to the `@IImagLit` interface, whose `CORE`
instance builds the `Cpx` struct (see 15). The `i` counts as a suffix only when no
identifier character follows, so `3if` is still `3` then `if`.

```thrax
$ unit : Cpx = 1.0i
$ whole: Cpx = 3i              # the magnitude is a Real either way
$ tiny : Cpx = 2.5e-2i         # scientific notation suffixes too
$ z    : Cpx = 3.4 + 1.2i      # the canonical spelling: real plus imaginary
```

An operator takes ONE type for both operands, so a real joins a complex only by
being made one: `CPX.of_real 3.4 + 1.2i`. Nothing is promoted silently, in either
direction. There is no imaginary PATTERN; match the fields (`is z | Cpx.{re, im}`)
or compare with `==` in a guard.

## 1.5 String literals and escapes
A string is a block of bytes; source text must be well-formed UTF-8. Escapes:
`\n \t \r \0 \\ \" \' \a \b \f \v`, `\xHH` (one raw byte), `\u{...}` (a Unicode
scalar, UTF-8 encoded).

```thrax
$ hi   : Str = "hello\n"
$ raw  : Str = "\x41\x42"            # "AB" via hex bytes
$ emoji: Str = "\u{1F600} café"      # code point + raw UTF-8
```

## 1.6 String interpolation
`"... {expr} ..."` splices an expression, stringified through `to_string` over its
`Ito_string` instance. Surface sugar for `chunk ++ to_string expr ++ chunk`. `\{`
and `\}` are literal braces.

Simple:
```thrax
$ greet : Str -> Str = \who = "hello {who}!"
```

Involved (a user type interpolates by defining an `Ito_string` instance):
```thrax
$ Point : @struct = x: @Int, y: @Int
$ impl_IToString_for_Point : Ito_string Point = .{ .show = \p = "({p.x}, {p.y})" }
$ msg : Str = "at {Point.{ .x = 3, .y = 4 }}, n={40 + 2}"   # "at (3, 4), n=42"
```

---

# 2. Program structure

## 2.1 Module header `@mod`
Every file begins with `$ @mod NAME`. A module name is uppercase with digits, `_`,
and the inert lowercase `x` allowed (so `MAIN`, `LIST_UTIL`, `MODxA`). A module
may span several files (repeat the header).

```thrax
$ @mod MAIN
```

## 2.2 Global definitions `$`
`$ name : Type = expr`. The annotation is required unless inference gives the
body a ground, non-arrow type (a plain `@Int`/`Str` constant). Functions and
anything polymorphic must be annotated. `$a` and `$ a` are the same.

Simple:
```thrax
$ answer = 42                 # inferred @Int, no annotation needed
$ id : t -> t = \x = x        # annotation required (polymorphic)
```

Involved (definitions may appear in any order and reference each other):
```thrax
$ even : @Int -> @Bool = \n = if n == 0 => @true else odd (n - 1)
$ odd  : @Int -> @Bool = \n = if n == 0 => @false else even (n - 1)
```

## 2.3 Imports `$ with`
`$ with MOD` brings a module's public names into scope, bare or qualified.
A qualified `MOD.symbol` needs the import too. An import cannot be renamed, and
it brings in the whole module, not one symbol.

```thrax
$ with MATH                    # MATH.sqrt, or bare sqrt
$ with STR
```

When two imports export the same name, the bare name is ambiguous and the
qualified form picks one (`VEC.len`, `LIST.len`).

## 2.4 Visibility `$ @private`
Symbols are public by default. `$ @private` hides every symbol below it, to the
end of the file, from importers. There is no `@public` counterpart, so a
module's public interface sits above the marker.

```thrax
$ api : @Int -> @Int = \x = helper x + 1
$ @private
$ helper : @Int -> @Int = \x = x * x    # module-private
```

## 2.5 Entry point
The program entry is `@main`, the one name the compiler knows, and it has exactly
one accepted signature: it takes the argument vector and returns the `@Int` exit
code, performing `@IO`. Together with `@build` (section 11.3), the compile-time
entry, those are the only `@`-names a program may define, besides the blessed
`@I*` interface types, which only `CORE` declares.

```thrax
$ @main : @Vec @Str -> <@IO> @Int = \args = 0      # args[0] is the program path
```

A module without `@main` is a library: `thrax check` type-checks it and runs its
compile-time assertions, and `thrax run`/`build` reject it. Nothing else is
special, so a test harness is ordinary code: a check under `$ @e` (section 11)
or a global some tool evaluates by name.

---

# 3. Types

## 3.1 Base numeric and string types
The word-size numbers are `@`-spelled like every other primitive: `@Int`
(signed word), `@Nat` (unsigned word). `Real` and `Str` keep friendly names.
They are distinct nullary constructors.

```thrax
$ n : @Int = -5
$ u : @Nat = 5
$ r : Real = 2.5
$ s : Str  = "hi"
```

Complex numbers are NOT a base type: `Cpx` is an ordinary `CORE` struct
(`re`/`im`), with the `i` literal suffix (1.4) and its operators supplied as
library instances. `CPX` holds the math (15).

## 3.2 Sized numerics
Fixed widths are `@`-spelled: `@Int8/@Int16/@Int32/@Int64`,
`@Nat8/@Nat16/@Nat32/@Nat64`, `@Float32/@Float64`. They are distinct from `@Int`
etc. (`@Int` and `@Int32` do not unify). Each width has its own arithmetic
instances; a literal takes the width expected of it. An operator takes ONE type for
both operands, so crossing a width is explicit (`@cast`, `f32_to_f64`).

```thrax
$ w : @Int32 = 1000
$ z : @Int32 = w * 2          # @Int32 + @Int32 : @Int32
```

## 3.3 Other `@`-typed built-ins
`@Ptr` (raw pointer), `@Bool` (values `@true`/`@false`), `@Array` (byte block),
`@Vec` (the growable vector, and the default sequence).

```thrax
$ flag : @Bool  = @true
$ bytes: @Array = @Array.{ 8 }      # 8 zeroed bytes
$ xs   : @Vec @Int = [1, 2, 3]
```

## 3.4 Unit `{}`
The empty record type is unit; its sole value is also `{}`. Disambiguated by
position (type vs term).

```thrax
$ nothing : {} = {}
$ ignore : @Int -> {} = \x = {}
```

## 3.5 Function types and effect rows
`A -> B` is a function. A row between the arrow and result lists the effects a
call may perform: `A -> <E> B`. A bare arrow is pure (empty row). `<| e>` is an
open row (a row variable), which is what makes a function effect-polymorphic.

```thrax
$ State : @effect = get : {} -> @Int, put : @Int -> {},
$ pure : @Int -> @Int = \x = x                       # pure (empty row)
$ act  : {} -> <State> @Int = \u = get {}           # may perform State
$ poly : (a -> <e> b) -> a -> <e> b = \f x = f x   # effect-polymorphic
```

## 3.6 Type variables
A lowercase name free in a signature is a type variable, universally quantified
over the whole signature. The same lowercase name repeated is the same variable;
there is no separate backtick spelling.

```thrax
$ fst : {a, b} -> a = \t = t.0
$ const : t -> u -> t = \x y = x
```

## 3.7 Tuples
`{A, B, ...}` is a structural product of any arity. `{}` is unit, `{A}` a
one-tuple. Elements read positionally with `.0`, `.1`.

Simple:
```thrax
$ p : {@Int, Str} = {42, "answer"}
$ n : @Int = p.0
```

Involved (generic, nested, first-class):
```thrax
$ swap : {a, b} -> {b, a} = \t = {t.1, t.0}
$ nested : {@Int, {@Int, @Int}} = {1, {2, 3}}
$ deep : @Int = nested.1.0 + nested.1.1        # 5
```

## 3.8 Records and row polymorphism
A record type may leave its tail open with a row variable:
`{ x: @Int, y: @Int | r }` accepts any record (nominal struct or anonymous) with at
least those fields. Records are structural (unify by field name).

Simple:
```thrax
$ area : { x: @Int, y: @Int | r } -> @Int = \p = p.x * p.y
```

Involved (update preserves the open tail; `with` stacks a field):
```thrax
$ shift : { x: @Int | r } -> { x: @Int | r } = \p = { .x = p.x + 10 | p }
$ tag   : { x: @Int | r } -> { x: @Int, t: @Int | r } = \p = { .t = 99, with p }
```

## 3.9 Sized tensors
`[n]T` is a vector of `n` elements; `n` is a type-level natural (its own kind).
`[m, n]T` is sugar for nested `[m][n]T`. Sizes may be arithmetic
(`[n+m]a`), decided by a canonical form. See section 9.

```thrax
$ v : [3]@Int = [10, 20, 30]
$ g : [2][2]@Int = [ [1,2], [3,4] ]
```

## 3.10 Type aliases `@alias`
`$ Name : @alias params = Type` names a type. It may take parameters and
partially instantiate another generic type. An alias is transparent (unifies
with its expansion).

Simple:
```thrax
$ Count : @alias = @Int
```

Involved (partial instantiation):
```thrax
$ Pair : @struct a b = fst: a, snd: b,
$ IntPair : @alias b = Pair @Int b               # fix the first arg, keep the second open
$ ip : IntPair Str = .{ .fst = 3, .snd = "z" }  # same as Pair @Int Str
```

An alias whose body is an effect row is a row alias. It names effects, not a
type, so it is used inside a row (section 8.6).

---

# 4. Expressions and syntactic sugar

## 4.1 Lambdas
`\pat ... = e`. Parameters are juxtaposed and curried sugar. A parameter may
be an irrefutable pattern, and may carry a `: Type` annotation inside parens,
`(x: T)`. The parens are needed because a lowercase name in a type is a type
variable, so a bare annotation would swallow the next parameter. A lone
parameter may drop them (`\x: T = e`), since `=` ends its type.

```thrax
$ Person : @struct = name: Str, age: @Int,
$ inc : @Int -> @Int = \x = x + 1
$ add : @Int -> @Int -> @Int = \a b = a + b            # curried
$ name : Person -> Str = \Person.{ name, _ } = name # destructuring parameter
$ dec = \x: @Int = x - 1                            # annotated parameter
$ sub = \(a: @Int) (b: @Int) = a - b               # one annotation per parameter
```

## 4.2 Application
Juxtaposition, left-associative, binds tighter than any operator.

```thrax
$ inc : @Int -> @Int = \x = x + 1
$ add : @Int -> @Int -> @Int = \a b = a + b
$ r = add (inc 2) 3          # (add (inc 2)) 3
```

## 4.3 Arithmetic and unary operators
`+ - * / %`, unary `-` (negation) and `!` (boolean not). Standard precedence,
`( )` groups.

```thrax
$ a = 1 + 2 * 3 - 4          # 3
$ b = (1 + 2) * (3 - 4)      # -3
$ c = -42
$ d = !@true                 # @false
```

## 4.4 Comparisons
`==` `!=` `<` `>` `<=` `>=`, all `a -> a -> @Bool`.

```thrax
$ lt : @Bool = 3 < 8
$ le : @Bool = 8 <= 8
$ ne : @Bool = 3 != 4
```

Like arithmetic, these are `CORE` functions over interfaces: `==` and `!=` over
`IEq`, the four orderings over `IOrd`, whose instances bottom out at the
comparison intrinsics (`@ieq`, `@ilt`, `@ult`, `@feq`, `@flt`, `@seq`, `@slt`).
A type of your own joins them with an instance:

```thrax
$ Money : @struct = cents: @Int

$ impl_IOrd_for_Money : IOrd Money = .{ .lt = \a b = a.cents < b.cents }
$ cheaper : @Bool = Money.{ .cents = 150 } < Money.{ .cents = 900 }
```

There is NO structural fallback: a type with no `IEq` instance cannot be compared
with `==`, which is an error at the comparison rather than a surprise at run time.
`DERIVE.derive_eq` / `derive_ord` generate the instances for a declared type.

`<` and `>` are also the effect-row brackets (§3.5), but a row only ever appears
in type position, so the two never collide.

## 4.5 Concatenation `++` and cons `::`
`++` joins strings/arrays/vectors of the same type; `::` prepends to a `@Vec`. Both
are `CORE` functions over interfaces (`ICat`, `ICons`), so a sequence of your own
joins them with instances of its own.

```thrax
$ s  : Str = "Hello" ++ " " ++ "world"
$ xs : @Vec @Int = 1 :: 2 :: 3 :: []
```

`::` builds a fresh vector, so it is O(n) and a `::`-recursion is quadratic;
`VEC.push` is the idiomatic way to build one up.

## 4.6 Short-circuit `&&` / `||`
Desugar to a lazy `if`, so the right operand runs only when needed.

```thrax
$ safe : @Int -> @Bool = \n = n > 0 && 100 / n > 5
```

## 4.7 Conditional `if ... => ... else`
`if c => t else e`. The condition is a `@Bool`. Chains with `else if`, or with a
comma between arms (one `if`, one `else`); a trailing comma before `else` is
allowed. It is lazy (only the taken branch runs) and is not a pattern match.

Simple:
```thrax
$ abs : @Int -> @Int = \n = if n > 0 => n else -n
```

Involved (else-if chain):
```thrax
$ sign : @Int -> @Int = \n =
	if n == 0 => 0
	else if n > 0 => 1
	else -1
```

The same chain with comma-separated arms:
```thrax
$ sign2 : @Int -> @Int = \n =
	if
		n == 0 => 0,
		n > 0  => 1,
	else -1
```

## 4.8 Local bindings `let ... in`
`let pat = e in body`. Multiple bindings chain with `,` (one `in`; each scopes
the next). A binding is recursive exactly when its name occurs in its own body
(no `let rec`). A binder may carry a type annotation.

Simple:
```thrax
$ x = let a = 6 in a * 7
```

Involved (comma chain, destructuring, recursion, annotation):
```thrax
$ r = let {a, b} = {3, 4}, s = a + b in s * 2
$ len : @Vec @Int -> @Int =
	let go : @Vec @Int -> @Int -> @Int = \l n =
		is l | _ :: t => go t (n + 1) | _ => n
	 in \l = go l 0
```

## 4.9 Sequencing `;`, pipes `|>` `<|`, composition `<|>`
Parser sugar, lowest precedence. `a ; b` = `let _ = a in b`. `x |> f` = `f x`
(left-assoc). `f <| x` = `f x` (right-assoc).

`f <|> g` composes, and is the lambda it means: `\x = f (g x)`. It is
right-associative and the tightest infix operator, so `f <|> g <|> h` is
`f <|> (g <|> h)`, and application still binds tighter (`f <|> g x` is
`f <|> (g x)`). Because it expands to a lambda, it is typed per use site, and the
composed function's effect row is the UNION of the two steps' rows: `shout <|> show`
where only `shout` performs `@IO` has type `@Int -> <@IO | r> {}`, and two different
effects give `<@IO, St | r>`.

The FUNCTION form `(<|>)` is CORE's `compose`, which is an ordinary definition and
so has one written type, `(b -> <e> c) -> (a -> <e> b) -> a -> <e> c`: one row for
both steps, so it composes two functions that agree on their effects. Taking two
different rows is a property of the infix form, which has no signature to share,
the way short-circuiting is a property of infix `&&` (§4.10). A single row that is
the union of two variables is not expressible today (issue #200).

```thrax
$ inc : @Int -> @Int = \x = x + 1
$ double : @Int -> @Int = \x = x * 2
$ step1 : {} -> @Int = \u = 0
$ step2 : {} -> @Int = \u = 0
$ eff = step1 {} ; step2 {} ; 0            # run for effect, return 0
$ y   = 5 |> inc |> double                 # double (inc 5)
$ z   = double <| inc <| 5                 # same
$ f   : @Int -> @Int = double <|> inc      # \x = double (inc x)
$ w   = (double <|> inc) 5                 # 12
$ shout : @Str -> <@IO> {} = \s = IO.println s
$ show  : @Int -> <@IO> {} = shout <|> to_string   # the row is the union
$ h2    : @Int -> @Int = (<|>) double inc          # = compose double inc
```

## 4.10 Operators as functions `(op)`
An operator in parentheses is a plain function value, the same binding a
`$ (op) : ... = ...` definition binds. It curries and passes as an argument; since
it takes a context parameter, the type it is used at decides which instance is
injected, so a bare `(+)` with nothing to pin its type is an error that says so.

```thrax
$ two   = (+) 1 1                          # 1 + 1
$ add1  : @Int -> @Int = (+) 1             # curried
$ plus  : @Int -> @Int -> @Int = (+)       # the instance comes from the type
$ total = VEC.foldl (+) 0 [1, 2, 3]        # as an argument
```

The parens must hold exactly one operator, so `(a + b)` and `(-1)` still group.
`|` and `<>` are grammatical delimiters, not operators, and have no function
form. An operator the parser rewrites rather than calls (`&&`, `||`, `;`, `|>`,
`<|`) has no binding of its own, so `(op)` is the function `\l r = l op r`;
`(&&)` therefore evaluates both sides, since short-circuiting is a property of
the infix form. `<|>` is rewritten too, but its function form names a binding:
`(<|>)` is CORE's `compose` (§4.9).

## 4.11 `@cast`
Converts an integer to another integer type, wrapping it into the target's
range: the value is truncated to the target's width, then sign-extended for
`@intN` or zero-extended for `@natN` (so `@cast 250` as an `@Int8` is `-6`). The
target type comes from context (annotate if unknown). Integer widths only, not
int/real/ptr (those use `C.i2f` / `C.i2p`).

Every value of a sized type stays in its range: arithmetic on `@Int8` .. `@Int32`
and `@Nat8` .. `@Nat32` wraps the same way, and a literal that does not fit its
type (`300` as a `@Nat8`) is a compile error (`INT_LITERAL_RANGE`).

```thrax
$ big   : @Int64 = 300
$ small : @Int32 = @cast big              # reinterpret across integer widths
$ back  : @Int    = @cast small            # and back to the platform word
```

## 4.12 Typed holes `???`
`???` stands for an expression not written yet. It takes whatever type its
context needs, and it is always a compile error (`TYPED_HOLE`) that names that
type and lists the names in scope that fit it, each offered as a fix. A hole is
how to ask the compiler what goes in a spot.

```thrax
$ f : @Int -> @Int = \n = n + ???
# error[TYPED_HOLE]: hole `???` has type `@Int`
# note: in scope with this type: `n`
```

---

# 5. Pattern matching

## 5.1 The match form `is`
`is scrut | pat => e | pat => e ...`. Arms are tried top to bottom; the first
matching pattern wins and binds its variables. The leading `is` distinguishes it
from `if`. There is no default branch: a catch-all is an ordinary arm whose
pattern matches anything, `| _ => d` (or `| x => d` to use the value).

Simple:
```thrax
$ classify : @Int -> Str = \n =
	is n | 0 => "zero" | 1 => "one" | _ => "many"
```

Involved (nested union patterns, exhaustive so no catch-all, see 5.9):
```thrax
$ Wrap : @union = Empty: {}, W: { Opt }
$ Opt  : @union = None: {}, Some: { @Int }
$ unwrap : Wrap -> @Int = \w =
	is w | Wrap.Empty => 0
	     | Wrap.W.{ Opt.None } => 1
	     | Wrap.W.{ Opt.Some.{ n } } => n
```

## 5.2 Literal patterns
Match integers (any width), Real, and Str by equality (refutable). A numeric
literal may be negative (`-1`, `-0.5`), also as a range bound (`-10 ... -1`).

```thrax
$ describe : @Int -> Str = \n = is n | 0 => "z" | 1 => "o" | -1 => "m1" | _ => "m"
$ yn : Str -> @Int = \s = is s | "yes" => 1 | "no" => 0 | _ => 99
```

## 5.3 Wildcard and variable patterns
`_` matches anything binding nothing; a lowercase name binds the value.

```thrax
$ tag : @Int -> @Int = \n = is n | 0 => 100 | m => m + 1
```

## 5.4 Struct patterns
`Type.{ ... }` matches a struct. Positional (bare fields, in order, all listed)
or named (dotted, any order, others ignored; a lone `.name` puns).

```thrax
$ Point  : @struct = x: @Int, y: @Int,
$ Person : @struct = name: Str, age: @Int,
$ sum_xy : Point -> @Int = \p = is p | Point.{ x, y } => x + y
$ who    : Person -> Str = \p = is p | Person.{ .name } => name
```

## 5.5 Variant patterns
`Type.Tag` (unit) or `Type.Tag.{ payload }`. May be written bare (`.Tag`), with
the union inferred from the arms. Payloads nest.

```thrax
$ Maybe : @union t = Just: t, None: {}
$ get : @Int -> Maybe @Int -> @Int = \d m = is m | Maybe.Just.{ x } => x | _ => d
$ isJust : Maybe t -> @Bool = \m = is m | .Just.{ _ } => @true | _ => @false
```

## 5.6 Range patterns
`lo ... hi` matches the inclusive interval (bounds are literals of the
scrutinee's type). Open `lo ...` matches `lo <= x`. Refutable, binds nothing.

Simple:
```thrax
$ grade : @Int -> Str = \n = is n | 90 ... 100 => "A" | 60 ... 89 => "C" | _ => "F"
```

Involved (open range, Real):
```thrax
$ sign : @Int -> Str = \n = is n | 0 ... => "nonneg" | _ => "neg"
$ band : Real -> @Int = \x = is x | 0.0 ... 0.5 => 1 | 0.5 ... 1.0 => 2 | _ => 0
```

## 5.7 Sequence and array patterns
Sequence: `[]`, `h :: t`, fixed-arity `[a, b]`, open `[a, ..rest]`. Every one of
them steps the scrutinee's `@compiler_interface_sequence_view` hook, so `@Vec`
(the default), `@Array`, `CORE`'s cons `List`, and any type of your own match the
same way: define the hook and the brackets work.

Simple:
```thrax
$ sum : @Vec @Int -> @Int = \xs = is xs | [] => 0 | h :: t => h + sum t | _ => 0
```

Involved (leading elements plus a rest tail, on a vector and on an array):
```thrax
$ second : @Vec @Int -> @Int = \xs = is xs | [_, x, ..rest] => x | _ => -1
$ head_of : @Array -> @Int = \a = is a | [h, ..rest] => h | _ => 0
```

## 5.8 Or-patterns and guards
`| p1 | p2 => e` shares one body (alternatives bind no variables). `| pat if g
=> e` adds a boolean guard; a failed pattern or guard falls through to the next
arm, even one with the same constructor.

Or-pattern:
```thrax
$ small : @Int -> @Int = \n = is n | 0 | 1 | 2 => 1 | _ => 0
```

Guards falling through the same constructor:
```thrax
$ Box : @union = Some: @Int, Nil: {},
$ grade : Box -> @Int = \x =
	is x | Box.Some.{ v } if v > 100 => 3
	     | Box.Some.{ v } if v > 0   => 2
	     | Box.Some.{ _ }             => 1
	     | Box.Nil.{}                 => 0
```

## 5.9 Exhaustiveness
Every `is` match, and every set of handler value arms, is checked after type
inference. A match that misses a value is a compile error
(`NON_EXHAUSTIVE_MATCH`) that names a missing shape, such as `Light.Green` or
`Opt.Some.{ _ }`. An arm the earlier arms already cover is a warning
(`UNREACHABLE_ARM`).

- A match needs no catch-all when its arms cover every constructor of a union.
  Booleans, tuples, and structs are covered the same way, nested to any depth.
- A guarded arm covers nothing for exhaustiveness, since its guard can fail.
- Integer literals and ranges are checked as intervals of their type's values,
  so `| 0 ... 127 => a | 128 ... => b` covers a `@Nat8`, and an arm whose range
  earlier arms already cover is unreachable. `@Int` and `@Nat` are taken at 64
  bits. A missing interval is named, such as `100` or `10 ...`.
- `@Float64`, `@Str`, and the other infinite domains need a `_` or binder arm.
  Real ranges and string-prefix patterns never cover a whole domain.
- A union with its own `@ISeqView` cannot be matched with both its variants and
  sequence patterns in one position (only `List` can); that is reported.
- A sequence pattern (`[]`, `h :: t`, `[a, ..r]`) is checked through the
  sequence view, so `[]` and `_ :: _` together cover any sequence.

```thrax
$ Light : @union = Red: {}, Yellow: {}, Green: {}
$ go : Light -> @Int = \l =
	is l | Light.Red => 0 | Light.Yellow => 1 | Light.Green => 2   # no catch-all needed
```

## 5.10 Irrefutable patterns in `let` and lambda
A `let` binder or a lambda parameter has no next arm to fall through to, so its
pattern must match every value of its type. `_`, variables, and struct, tuple,
and record patterns built from them always do. A pattern that can fail, such as
a literal or a variant of a union with several, is a compile error
(`NON_EXHAUSTIVE_MATCH`) naming a value it misses; bind a variable and match it
with `is` instead.

```thrax
$ Point  : @struct = x: @Int, y: @Int,
$ Person : @struct = name: Str, age: @Int,
$ Line   : @struct = from: Point, to: Point,
$ person : Person = Person.{ .name = "A", .age = 30 }
$ age : @Int = let Person.{ .age = a } = person in a
$ start : Line -> @Int = \Line.{ .from = Point.{ x, _ }, .to = _ } = x
```

---

# 6. Algebraic data types

## 6.1 Structs `@struct`
A named record type: `$ Name : @struct params = field: Type, ...`. Built with a
literal (`Type.{ .f = e }`, bare `.{ ... }`, or positional `Type.{ a, b }`);
read with `.field`. Nominal (unifies by name).

Simple:
```thrax
$ Person : @struct = name: Str, age: @Int,
$ p : Person = Person.{ .name = "Will", .age = 21 }
$ who : Str = p.name
```

Involved (generic, applied by juxtaposition, nested field types):
```thrax
$ Box : @struct t = val: t,
$ Wrap : @struct t = inner: Box t,
$ w : Wrap @Int = .{ .inner = Box.{ .val = 7 } }
$ deep : @Int = w.inner.val
```

## 6.2 Record update
`Type.{ .field = e | base }` copies unlisted fields from `base`. The `| base`
comes last; a listed field may read from `base`.

```thrax
$ Person : @struct = name: Str, age: @Int,
$ base : Person = Person.{ .name = "A", .age = 30 }
$ older : Person = Person.{ .age = base.age + 1 | base }
$ clone : Person = .{ | base }
```

## 6.3 Unions `@union`
A tagged sum: `$ Name : @union params = Tag: Payload, ...`. A `{}` payload is a
unit variant. Recursive and generic. Non-strict data constructors (lazy
payloads).

Simple:
```thrax
$ Maybe : @union t = Just: t, None: {}
$ some : Maybe @Int = Maybe.Just.{ 5 }
$ none : Maybe @Int = .None
```

Involved (recursive, multi-field payload, positional or named):
```thrax
$ Tree : @union a = Leaf: a, Node: { Tree a, Tree a }
$ t : Tree @Int = Tree.Node.{ Tree.Leaf.{ 1 }, Tree.Leaf.{ 2 } }
```

## 6.4 Type splice `with`
A `@struct`/`@union` may start with `with Other` to copy that type's fields or
variants ahead of its own. A copy-paste convenience with no subtyping
relationship. Transitive; a duplicate member is an error.

```thrax
$ Point  : @struct = x: @Int, y: @Int
$ Point3 : @struct = with Point, z: @Int          # x, y, then z
$ Base   : @union  = Red: {}, Green: {}
$ Color  : @union  = with Base, Blue: {}         # Red, Green, then Blue
```

## 6.5 Recursive types are lazy
A slot whose type leads back to the type that declares it holds its value
lazily: building one cell does not build the rest, and a cell is computed when
something looks at it. The forced value replaces the thunk, so a cell costs its
work once however many times it is read. This makes an unbounded structure an
ordinary declaration, with no separate kind for it.

Laziness belongs to the slot, so it does not depend on how the value is written.
It stops at an arrow: a slot typed `{} -> T` is already a suspension and stays a
plain function.

Simple (the prelude `Stream`, a recursive `@struct`):
```thrax
$ Stream : @struct t = head : t, tail : Stream t
$ first : Stream @Int -> @Int = \s = s.head
$ next  : Stream @Int -> Stream @Int = \s = s.tail
```

Involved (map over an unbounded stream; `tail` is only built when read):
```thrax
$ smap : (a -> b) -> Stream a -> Stream b = \f s =
	{ .head = f s.head, .tail = smap f s.tail }
$ tenth : @Int = (smap (\x = x + x) (count_from 1)).tail.tail.head
```

A recursive `@union` works the same way, which is how a `List` is consumed one
cons at a time:
```thrax
$ from : @Int -> List @Int = \n = List.Cons.{ n, from (n + 1) }
$ head : List @Int -> @Int = \l = is l | List.Cons.{h, t} => h | _ => 0
```

Evaluation is otherwise call by value, so a slot filled through a function
argument is evaluated at the call, as any argument is. `List.Cons.{ n, from (n
+ 1) }` suspends the tail; `cons n (from (n + 1))` does not, because `from (n +
1)` is an argument to `cons`.

---

# 7. Functions

## 7.1 Currying
Every function of several parameters is curried; partial application yields a
function.

```thrax
$ add : @Int -> @Int -> @Int = \a b = a + b
$ add5 : @Int -> @Int = add 5           # partial application
```

## 7.2 Higher-order functions
Functions are first-class values.

```thrax
$ apply_twice : (t -> t) -> t -> t = \f x = f (f x)
$ compose : (b -> c) -> (a -> b) -> a -> c = \f g x = f (g x)
```

## 7.3 One name, one definition
A name has one definition and one type. Two definitions of a name in one module is
an error; a local definition shadows an imported one of the same name; and a name
two imports bring in has no bare meaning, so a use of it says so and names the
owners (`A.name` and `B.name` still reach either). An operation that covers several
types is an interface (§7.4), not several definitions.

## 7.4 Interfaces and the context parameter `@ctx`
An interface is an ordinary `@struct` whose fields are its operations; implementing
it is defining a value of that type. A function that needs one declares it as a
`@ctx` FIRST parameter: ordinary in every respect, except that call sites do not
write it. The compiler finds it BY TYPE, taking the nearest binder in scope and
otherwise the one instance in scope (two would be an error, since global scope is
flat). It elaborates to a leading argument, so the backends need no support.

Simple:
```thrax
$ Ordering : @union = LT: {}, EQ: {}, GT: {}
$ IOrder : @struct t = compare: t -> t -> Ordering,
$ impl_IOrder_for_int : IOrder @Int = .{
	.compare = \a b = if a < b => Ordering.LT else if a > b => Ordering.GT else Ordering.EQ,
}
$ max_of : @ctx IOrder t -> t -> t -> t = \d x y =
	is d.compare x y | Ordering.GT => x | _ => y
$ biggest : @Int = max_of 3 7
```

A definition has at most ONE context parameter and it is first; its type's head
must be a declared struct or union. Several requirements travel as one tuple, which
the call site builds:

```thrax
$ abs : @ctx {IOrd t, ISub t, IZero t} -> t -> t = \ds x =
	if ds.0.lt x ds.2.zero => ds.1.sub ds.2.zero x else x
```

Chaining and the explicit form (`f (@ctx e) x`, first argument, where the `@ctx`
keyword is needed because the parameter is otherwise not addressable):

```thrax
$ max3 : @ctx IOrder t -> t -> t -> t -> t = \d x y z =
	max_of (max_of x y) z          # d satisfies max_of's requirement, so it passes down
$ as_min : @Int =
	let flip : IOrder @Int = .{ .compare = \a b = impl_IOrder_for_int.compare b a } in
	max_of (@ctx flip) 3 7
```

An instance may itself take a context, which resolution fills in recursively, so
`IEq (@Vec t)` is built from `IEq t`. `CORE`'s operators are all ordinary functions
over such interfaces: `+ - * / %` over `IAdd`/`ISub`/`IMul`/`IDiv`/`IMod`, `^` over
`IPow`, `== != < > <= >=` over `IEq`/`IOrd`, `++` over `ICat`, `::` over `ICons`, and
`to_string` over `Ito_string`. A type joins any of them by defining an instance:

```thrax
$ Money : @struct = cents: @Int
$ impl_IAdd_for_Money : IAdd Money = .{ .add = \a b = Money.{ .cents = a.cents + b.cents } }
$ total : Money = Money.{ .cents = 1 } + Money.{ .cents = 2 }
```

See `documentation/interfaces.md` for the full rules, the diagnostics, and the
blessed `@I*` interfaces the compiler's own desugars resolve.

## 7.5 Tail-call optimization
Tail-recursive calls (self, mutual, or through a recursive local `let`) run in
constant stack.

```thrax
$ sum_to : @Int -> @Int -> @Int = \n acc =
	if n == 0 => acc else sum_to (n - 1) (acc + n)      # constant stack at any depth
```

---

# 8. Algebraic effects and handlers

## 8.1 Declaring an effect `@effect`
`$ Name : @effect = op : Arg -> Resume, ...`. Each operation takes one argument
and names its resume type. An operation whose handler never resumes (an
exception) names a type variable as its result.

```thrax
$ State : @effect = get : {} -> @Int, put : @Int -> {},
$ Exn   : @effect = throw : Str -> a,
$ Yield : @effect = yield : @Int -> {},
```

An effect may take type parameters, written after `@effect` like a struct's.
In a row the effect is applied to its arguments, `<State @Str>`. One handler
serves every operation the body performs on it, so it fixes the parameters for
all of them: under one `State` handler every `get` and `put` is at one type. A
parameter may be a row variable, for an operation that takes an effectful
function, and a bare mention of the effect inside its own declaration stands for
its parameters. Arguments left out of a row (`<State>`) are inferred.

```thrax
$ State : @effect s = get : {} -> s, put : s -> {},
$ Async : @effect m e = spawn : ({} -> <Async m e | e> {}) -> {}, send : m -> {},
$ count : {} -> <State @Int> @Int = \u = let n = get {} in put (n + 1); n
```

An operation's type may also use variables the effect does not declare, like
`throw` above. Each use chooses those types, so a handler clause sees them as
opaque: it can pass such a value along (resume with the argument it was given,
store it), but it cannot make one up or inspect it. A clause that needs a
concrete type takes it from an effect parameter instead.

```thrax
$ Exn : @effect = throw : @Str -> a,
$ safe : @Int = do 1 + Exn.throw "no" ctl k | Exn.throw m => 0     # aborts: fine
# `| Exn.throw m => k 0` is an error: `k` wants `a` of `Exn.throw`, not @Int
```

## 8.2 Performing an operation
Just call it; there is no `perform` keyword. A use injects the effect into the
ambient row.

```thrax
$ State : @effect = get : {} -> @Int, put : @Int -> {},
$ tick : {} -> <State> @Int = \u = let x = get {} in let _ = put (x + 1) in x
```

## 8.3 Handling `do ... ctl`
`do body ctl k | op a => e ... | pat => e ...`. `k` is the captured
continuation, shared by every clause. Each `| op a => e` handles an operation. A
value arm `| pat => e` runs on normal completion, matching the body's final value
like an `is` arm; with none, that value is the handler's result. An operation
clause always names an operation and its argument (`| op a`, `| Eff.op a`),
which no pattern can look like, so the two kinds of arm need no marker. A shared
operation name is qualified in a clause head.

Exception (ignores `k`, so it resumes zero times):
```thrax
$ safeDiv : @Int -> @Int -> @Int = \a b =
	do if b == 0 => Exn.throw "div0" else a / b
	ctl k | Exn.throw msg => -1
```

Generator (resume once per yield, summing results):
```thrax
$ sumGen : ({} -> <Yield> {}) -> @Int = \gen =
	do gen {}
	ctl k | Yield.yield v => v + k {}
	      | _ => 0
```

## 8.4 Resuming
Resuming is applying `k`. `k v` splices the suspended computation back and
delivers `v`. `k` may be resumed **any number of times**, and may be stored
before use, which is what coroutines need. Handlers are deep (a resumed
computation is still governed by the same handler).

```thrax
$ Task : @union = Fin: {}, Susp: { @Int, {} -> Task }
$ spawn : ({} -> <Yield> {}) -> Task = \t =
	do t {}
	ctl k | Yield.yield v => Task.Susp.{ v, k }     # store k, resume later
	      | _ => Task.Fin.{}
```

Resuming more than once runs the rest of the computation once per resume, each on
its own copy, which is what nondeterminism and backtracking are:

```thrax
$ sum_all : ({} -> <Amb> @Int) -> @Int = \body =
	do body {}
	ctl k | Amb.flip u => k @true + k @false        # both branches
	      | x => x
```

A resume copies the captured computation only when something else could still
resume it: when nothing else holds `k`, the engine hands the computation over
instead. So a continuation resumed once costs nothing, whether it is applied in
the clause, captured by a lambda, or stored and resumed later, and n resumes of
one continuation cost n-1 copies, the last taking the original. A clause that
never mentions `k`, as in an exception, captures nothing at all.

## 8.5 Effect rows in types
A function's type carries the effects it may perform as a row on the arrow.
Absent row means pure. A row variable makes it effect-polymorphic. A handler
discharges the effects it handles. A performed-but-unhandled effect is a compile
error. Subsumption: a pure function is callable in any effectful context.

```thrax
$ map : (a -> <e> b) -> @Vec a -> <e> @Vec b = \f xs =
	is xs | [] => [] | h :: t => f h :: map f t
```

## 8.6 Row aliases
`$ Name : @alias = <...>` names a row of effects. Inside another row the alias
stands for its labels, so with `App` below, `<App>` is `<Gpu, Log>` and
`<App, Net>` is `<Gpu, Log, Net>`. A row alias may name other row aliases. It
is transparent, like a type alias: it checks exactly what its labels check, and
a label that both an alias and the rest of the row name is kept once.

```thrax
$ Log : @effect = log : @Int -> {},
$ Gpu : @effect = draw : @Int -> {},
$ Net : @effect = fetch : @Int -> @Int,
$ App : @alias = <Gpu, Log>
$ Online : @alias = <App, Net>
$ frame : @Int -> <App> @Int = \x = draw x; log x; x
$ sync : @Int -> <Online> @Int = \x = frame (fetch x)
```

A row variable in the body makes the alias parameterized, and it is applied to
row variables inside a row. Left unapplied, its tail is a fresh variable, as for
an under-applied type alias.

```thrax
$ WithLog : @alias e = <Log | e>
$ traced : (@Int -> <WithLog e> @Int) -> @Int -> <WithLog e> @Int = \f x = log x; f x
```

A printed row that holds every label of a closed, parameterless alias of two or
more labels shows the alias name in their place (`<App, Net>`). An unhandled
label that the performed row holds through such an alias names it:
``effect `Log` (part of `App`) is performed but not handled``. A row alias in
type position, or a type alias in a row, is an error.

## 8.7 `defer`
`defer <cleanup> in <body>` runs the cleanup when the body's scope exits, on
normal completion, on an abort by an outer non-resuming handler, or when a stored
continuation holding it completes. Nested defers run innermost-first. The
resource-safe FFI idiom.

```thrax
$ open  : @Str -> @Int = \path = 0        # stand-ins for real handles
$ close : @Int -> @Int = \h = h
$ read  : @Int -> @Int = \h = h
$ useFile : @Str -> @Int = \path =
	let f = open path in
	defer close f in
		read f
```

Several cleanups may share one `defer`, separated by commas (a trailing comma
before `in` is allowed). They nest exactly as separate `defer`s do, so they still
run innermost-first:
```thrax
$ useTwo : @Str -> @Str -> @Int = \a b =
	let f = open a, g = open b in
	defer close f,
	      close g,
	in
	read f + read g
```

---

# 9. Sized tensors and linear algebra

## 9.1 Tensor literals and indexing
`[n]T` is built from a `[..]` literal whose length fixes `n` (a mismatch is a
type error). Read with modular indexing `t.[i]` (the index reduces mod the size,
so it is total, `t.[n]` wraps to `t.[0]`). Indexing an empty `[0]T` is the sole
fault.

```thrax
$ with LA
$ v : [3]@Int = [10, 20, 30]
$ a : @Int = v.[1]        # 20
$ b : @Int = v.[3]        # wraps to v.[0] = 10
```

## 9.2 Multi-dimensional and multi-axis indexing
`[m, n]T` is nested `[m][n]T`. `t.[i, j]` reads one element (`t.[i].[j]`).

```thrax
$ with LA
$ g : [2][2]@Int = [ [1,2], [3,4] ]
$ z : @Int = g.[1, 0]     # 3
```

## 9.3 Size arithmetic
A size may be `+`/`*` over literals and size variables, evaluated mod 2^64 and
compared by a canonical form (`[n+m]` equals `[m+n]`; `[n+n]` is `[2*n]`). This
drives size-changing operations, whose result size is computed at compile time.

```thrax
$ with LA
# LA provides `concat`, its result size computed at compile time:
#   concat : [n]a -> [m]a -> [n+m]a
$ joined : [5]@Int = concat [1, 2] [3, 4, 5]      # [2+3] = [5]
```

## 9.4 Slicing and variance
Multi-axis slicing `t.[s0, s1, ...]`: `i` indexes an axis, `p ... q` narrows it
to an inclusive range, `..` keeps it whole. Every result is an O(1) strided view.
Each axis may carry a variance tag `@contra` (upper index / column) or `@co`
(lower index / row); `matmul` contracts a `@co` axis against a `@contra` axis.

```thrax
$ with LA
$ row : [m][n]a -> @Int -> [n]a = \m i = m.[i]
$ col : [m][n]a -> @Int -> [m]a = \m j = m.[.., j]
$ sub : [8]@Int -> [4]@Int = \m = m.[2 ... 5]      # inclusive 2..5, four elements
```

## 9.5 Tensor primitives and the `LA` library
The compiler provides only `@`-primitives over the buffer; every named operation
lives in `LA`. `t.[i]` desugars to the `@IIndex` interface and a lone
`t.[lo ... hi]` to `@ISlice`, so a container of your own joins the `.[..]` surface
by defining an instance. A multi-axis slice stays tensor-only: its result shape is
computed from the slots.

```thrax
$ transpose : [m][n]a -> [n][m]a = \t =
	@tensor_create (@tensor_index t 0)
		(\j = @tensor_create t (\i = @tensor_index (@tensor_index t i) j))
```

## 9.6 Expression-form ranges
`[lo ... hi]` builds whatever the resolved `@IRange` instance returns, and the open
`[lo ...]` whatever `@IRangeFrom` returns; `CORE` has instances for `@Vec`/`List`
and `Stream`/`List`. The expected type picks one, so a range of your own type needs
only its own instance. When nothing constrains it, the **first instance declared**
wins, which is how `CORE`'s order (not the compiler) sets what a bare range builds.
A sized tensor is the exception: literal bounds fix `n`, and the compiler builds it
directly.

```thrax
$ tv : [4]@Int     = [1 ... 4]     # tensor, n = 4
$ ns : @Vec @Int   = [1 ... 5]     # vector (the bare closed range)
$ s  : Stream @Int = [1 ...]       # unbounded (the bare open range)
```

A `@Vec` range materializes every element, so a large bound costs that much work
up front. The `List` instances are lazy in the tail, which is what makes a range
you only walk part of cheap:

```thrax
$ first5 : <@IO> {} =
	for ([1 ... 100000000000000] : List @Int)
		(\i = if i > 5 => break {} else IO.print "?(i), ")
```

---

# 10. Foreign function interface

## 10.1 `@extern` binding
A foreign binding is a global whose body is `@extern "C" "symbol" "lib"`. A
signature is required and drives C-ABI marshalling. An extern takes exactly one
argument (one arrow).

```thrax
$ puts : Str -> @Int = @extern "C" "puts" "libc"
$ used : @Int = puts "hi"
```

## 10.2 Multi-parameter C functions
A C function is not curried, so several parameters group into a record; its
fields, in declaration order, are the positional C arguments (marshalled by name,
so a reordered call site is fine). A nullary C function takes `{}`.

```thrax
$ pow : {base: Real, exp: Real} -> Real = @extern "C" "pow" "libm"
$ a : Real = pow {2.0, 10.0}                 # positional
$ b : Real = pow {.exp = 10.0, .base = 2.0}  # named, reordered
$ getchar : {} -> @Int = @extern "C" "getchar" "libc"
```

## 10.3 C structs and unions by value
`Name : @struct @extern "C" = ...` is a C-layout foreign type (fields must be
C-representable, not generic). Built and read with the ordinary struct literal
and `.field`, and it crosses the boundary by value. A C union is
`Name : @union @extern "C" = ...` (members share offset 0).

```thrax
$ Color : @struct @extern "C" = r: @Nat8, g: @Nat8, b: @Nat8, a: @Nat8,
$ clear : Color -> {} = @extern "C" "ClearBackground" "libraylib.so"
$ go : {} = clear Color.{ .r = 0, .g = 0, .b = 0, .a = 255 }
```

## 10.4 Callbacks and array parameters
A function-typed parameter is a C function pointer: pass a Thrax closure. A
`@Vec T` parameter (T a C-repr struct) is passed as a packed `T*` with a separate
count.

```thrax
$ Vector2 : @struct @extern "C" = x: @Float32, y: @Float32,
$ sort : {@Ptr, @Int, (@Int -> @Int -> @Int)} -> {} = @extern "C" "qsort_r" "libc"
$ draw : {@Vec Vector2, @Int} -> {} = @extern "C" "DrawLineStrip" "lib"
```

## 10.5 The `C` namespace and engine intrinsics
`library/C.thx` binds libc/libm as the auto-injected `C` namespace (used
qualified, `C.sqrt`, no import). It also holds a few conversions the language
cannot express with `@cast` (which is integer widths only): `C.i2f`/`C.f2i`
(@Int and @Float32), `C.i2d`/`C.d2i` (@Int and Real), `C.i2p` (@Int to @Ptr), and
`C.null` (the null pointer). These are provided by the engines' runtimes (an
empty `lib`); see `documentation/native-backend.md`.

```thrax
$ x : @Float32 = C.i2f 41
$ nul : @Ptr = C.null
```

Limitation: variadic C functions (`printf(fmt, ...)`) have no single binding.
Declare a fixed-arity binding per argument shape you use; there is no `...`
marker.

## 10.6 Builtins: `@extern "thrax"`
The primitives the compiler and runtime implement (`@iadd`, `@vec_push`,
`@chan_send`, `@lex`, ...) are declared in `library/CORE.thx`, each with its
signature and a comment:

```thrax
$ @iadd : t -> t -> t = @extern "thrax"
$ @vec_push : @Vec a -> a -> @Vec a = @extern "thrax"
```

The definition's name is the builtin, so there is no symbol or library string,
and a reference compiles to a direct call with no wrapper. Unlike a C extern, a
builtin is curried. Only CORE may declare one, and no module may redefine one.
The operators the parser rewrites (`;`, `|>`, `<|`) are declared the same way, for
their types: `$ (|>) : a -> (a -> <e> b) -> <e> b = @extern "thrax"`. A few primitives stay inside the
compiler because their types have no surface syntax: `@e` and `@cast` are
checked specially, and the `@tensor_*` primitives are polymorphic over axis
variance.

---

# 11. Compile-time evaluation

## 11.1 `@e`, compile-time code insertion
`@e X` runs `X` at build time and inserts the code it returns in its place. `X`
must be an `@Code`: `@parse_str` builds an expression and `@parse_items` builds
top-level items from a string. The inserted source is then checked like
hand-written code. `@e` works in expression, type, and item position, and
composes with the pipes like any one-argument function.

```thrax
$ lit : @Int -> @Code = \n = @parse_str <| to_string n

$ fib10 : @Int = @e <| lit <| fib 10                    # inserts `55`
$ n     : @e <| @parse_str "@Int64" = 1024              # inserts the type
$ @e <| @parse_items "$ injected : @Int = 99"           # inserts an item
```

`X` may perform the compile-time operations (`@emit`, `@abort`, `@fresh`,
`@link`, `@link_path`). They carry the `<@Meta>` effect, which `@e`
discharges. It discharges nothing else, so `@e` is **hermetic**: a generator
cannot perform `@IO`. Compile-time IO lives in `@build` alone (section 11.3).

Empty code inserts nothing. A site that runs only for its effect returns it, so
the directive disappears once it has run.

## 11.2 Compile-time assertions and build directives
There is no `@assert` builtin. `CORE.assert : @Bool -> @Str -> <@Meta> @Code` is
one line of ordinary Thrax over `@abort` that returns empty code, so a module
states its own invariants and checking it is testing it.

```thrax
$ @e <| assert (fib 10 == 55) "fib 10 should be 55"
$ @e <| assert (fact 5 == 120) "fact 5 should be 120"
```

A failing assertion, like any `@abort`, is an `error[META_ABORT]` carrying the
message, pointed at the `@e` site. A fault the code runs into on its own (a
division by zero, a failed match) stays a `RUNTIME_FAULT`.

`@link` / `@link_path` steer the compilation itself: they add a library to the
link line and the dlopen set, or a directory to the search path. Both return
empty code.

```thrax
$ @e <| @link "m"             # link/preload libm
$ @e <| @link_path "/opt/lib"
```

## 11.3 `@build`, the compile-time entry
`@build` is to compiling what `@main` is to running: the one name the *build*
starts from. The compiler runs it during compilation and adds the items of the
`@Code` it returns to the module that declared it. Like the entry, it has exactly
one accepted signature.

```thrax
$ @build : {} -> <@Meta, @IO> @Code = \_ =
	@parse_items "$ answer : @Int = 42"

$ @main : @Vec @Str -> <@IO> @Int = \args = answer      # 42
```

The `<@IO>` in that row is its whole privilege: `@build` is the one context where
a generator may read the world. The IO itself is not special, and there is no
compile-time IO primitive: it is `library/IO.thx` over the `C` namespace, the
same code a program runs, performed by the compile-time interpreter. So a
compile-time path is relative to the **working directory**, exactly like a path a
running program opens. `IO.read_dir` (a directory's entry names, sorted) is the
listing a `@build` usually wants.

Rules: every module may have one (not just the root, unlike `@main`), it runs
once, and it runs before any `$ @e` of that round, so code it generates is
available to the rest of the compile-time code. Anything it injects is itself
expanded, so a generated `$ @e` still runs and a generated `$ with M` loads M.
Note what "every module" implies: importing a module lets its `@build` perform IO
during *your* build, the same trust an imported `build.rs` carries.

`tests/MAIN.thx` is the worked example: it reads `examples/` at compile time and
generates its own imports and `@main`, so adding an example to the corpus is
adding a file. See `examples/META_BUILD.thx` and
documentation/metaprogramming.md.

To see what a metaprogram produced, `thrax expand <file> [MODULE]` prints the
source as the checker finally saw it: every `@e` spliced and every
`@build`'s items appended. That is the view to debug a generator from, since
nothing else shows the generated text.

---

# 12. Intrinsic types and primitives

## 12.1 `@Bool` and its literals
The built-in boolean; values are `@true` and `@false` (matched with the
`@`-spelling, since bare `true`/`false` would bind a variable). Comparisons yield
`@Bool`; `!` negates it.

```thrax
$ b : @Bool = 3 < 4
$ chk : @Int = is b | @true => 0 | _ => 1
```

## 12.2 `@Array` (byte block)
Allocated with `@Array.{ n }` (n zeroed bytes). Primitives: `@array_len`,
`@array_get`, `@array_set`, `@array_push`, `@array_slice`, `@array_alloc`. `Str`
is a byte array, so these apply to strings too. An `@Array` also joins the `[..]`
literal, pattern, and slice surfaces through its `CORE` instances of the blessed
interfaces, like any other sequence.

Simple:
```thrax
$ buf : @Array = @Array.{ 64 }
$ n : @Int = @array_len "hello"          # 5
```

Involved (build a string byte by byte, as CORE's integer stringifier does):
```thrax
$ digit : @Int -> Str = \d = @array_push "" (48 + d)   # push a byte onto a Str
```

## 12.3 `@Vec` (growable vector)
A growable typed vector, wrapped by the `VEC` library over `@vec_new`,
`@vec_push`, `@vec_get`, `@vec_set`, `@vec_len`, `@vec_fill`.

```thrax
$ with VEC
$ v = push (push (new {}) 1) 2          # VEC over @vec_* primitives
```

## 12.4 `to_string` (CORE) and interpolation
`CORE` (implicitly imported) provides `to_string` for `Str`, `@Bool`, and `@Int`,
and the `Stream`/`range`/`count_from` prelude. String interpolation desugars
through `to_string`; extend it (7.3) for your own types.

```thrax
$ s : Str = to_string 42 ++ " " ++ to_string @true      # "42 true"
```

## 12.5 `TARGET` reflection
The `TARGET` module (qualified, no import) reflects the compilation target: word
size, @Int bounds, and os/arch names, fixed at compile time.

```thrax
$ wide : @Bool = TARGET.int_bits == 64
$ name : Str = TARGET.arch ++ "-" ++ TARGET.os     # equals TARGET.name
```

---

# 13. The `@`-sigil catalogue

A quick index of the `@`-forms and where each is documented above.

| Form | Kind | Section |
| --- | --- | --- |
| `@mod` | module header | 2.1 |
| `@main` | program entry | 2.5 |
| `@build` | compile-time entry | 11.3 |
| `@struct` `@union` `@effect` `@alias` | type declarations | 6, 8.1, 3.10 |
| `@extern` | foreign binding, builtin declaration | 10 |
| `@ctx` | context (interface) parameter | 7.4 |
| `@private` | visibility | 2.4 |
| `@e` `@abort` | compile-time code insertion | 11 |
| `@link` `@link_path` | build directives | 11.2 |
| `@cast` | integer-width reinterpret | 4.11 |
| `@true` `@false` `@Bool` | boolean | 12.1 |
| `@Int8..64` `@Nat8..64` `@Float32/64` | sized numerics | 3.2 |
| `@I*` | blessed literal / indexing / range interfaces | 1.4, 9.5 |
| `@Ptr` `@Array` `@Vec` | built-in containers/pointer | 3.3, 12 |
| `@co` `@contra` | tensor axis variance | 9.4 |
| `@array_len/get/set/push/slice/alloc` | array primitives | 12.2 |
| `@vec_new/push/get/set/len/fill` | vector primitives | 12.3 |
| `@tensor_index/length/create/concat/slice/...` | tensor primitives | 9.5 |

---

# 14. Compiler CLI

`thrax [--target=ARCH-OS] <command> [file.thx] [args...]`. With no file the root
is `MAIN.thx` in the current directory (or the sole `.thx` file there).

| Command | Effect |
| --- | --- |
| `run` | run a program (needs `$ @main`) on the interpreter; extra args pass to it |
| `build` | compile a program to a native executable beside the source |
| `check` | run `@build`, expand metaprograms, run `$ @e` checks, print inferred types |
| `expand` | print the source after expansion, to inspect what a metaprogram made |
| `emit-c` | emit standalone C to stdout |
| `parse` | print the parsed syntax tree |
| `lex` | print the token stream |
| `fmt` | format files in place; `fmt --check` only lists unformatted ones |
| `fix` | apply every fix the compiler is sure of, rechecking until none is left |
| `explain` | explain a diagnostic code (`thrax explain TYPE_MISMATCH`) |
| `config` | print the `thrax.toml` options in force and document them all |
| `lsp` | run the language server on stdin/stdout (see `documentation/lsp.md`) |

Flags: `--target=ARCH-OS` cross-compiles (e.g. `x86_64-linux`, `wasm32-wasi`);
`-h` / `--help` prints usage.

```sh
thrax run                       # run ./MAIN.thx
thrax run app.thx a b           # run app.thx with args `a b`
thrax check examples/FIB.thx    # a library module: type-check and run its checks
thrax expand app.thx APP        # the source as the checker saw it, one module
thrax build app.thx
thrax --target=wasm32-wasi build app.thx
```

---

# 15. Standard library modules

Imported with `$ with MOD` (except `CORE`, `C`, and `TARGET`, which need no
import). A brief map:

| Module | Contents |
| --- | --- |
| `CORE` | `to_string`, `compose`, `Stream`, `range`, `count_from` (implicitly imported) |
| `C` | libc/libm bindings and engine conversion intrinsics (qualified) |
| `MATH` | @Int/Real helpers, libm wrappers, int/real conversions |
| `STR` | string operations (`from_int`, `substr`, `contains`, `to_lower`, ...) |
| `LIST` | list operations (`sum`, `length`, `map`, ...) |
| `VEC` | growable vectors over the `@Vec` primitives |
| `MAP` `SET` | keyed containers |
| `OPT` `RESULT` | optional and result types |
| `PATH` | path manipulation |
| `RANDOM` | pseudo-random numbers |
| `IO` | console, file, and directory IO over the `C` namespace |
| `LA` | shape-checked linear algebra over sized tensors |
| `CPX` | complex math over `CORE`'s `Cpx`: parts, polar form, roots, transcendentals |
| `TARGET` | compilation-target reflection (qualified) |
