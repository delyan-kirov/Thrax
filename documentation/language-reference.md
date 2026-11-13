# Ares Language Reference

Every language feature, one entry each, with a simple example and a more
involved one. The examples use the language's real, current surface syntax
(branches are `if c => t else e`, matches are `scrut |> \ | pat => e ... else d`).
Where a feature is only partly built, the entry says so.

Every code block below is self-contained and type-checks on its own (verified
with `ares check`); a block that needs a library imports it (`$ @import LA`, etc.).

Conventions used throughout: a global is `$ name : Type = expr`; a lambda is
`\x => e` (curried: `\a b => e`); a branch is `if c => t else e`; a match is
`scrut |> \ | pat => e ...`. The comparison operators are `==` (equal),
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

```ares
$ x : @Int = 1   # trailing comment
# whole-line comment
```

## 1.2 Integer literals
Decimal, hexadecimal (`0x`), and binary (`0b`).

```ares
$ a   : @Int = 255
$ b   : @Int = 0xFF
$ c   : @Int = 0b1010
$ mix : @Int = 0x10 + 0b10 + 8   # bases mix freely in arithmetic
```

## 1.3 Real literals
Decimal point or scientific notation.

```ares
$ pi   : Real = 3.14159
$ tiny : Real = 12e-3        # 0.012
$ half : Real = 1.0 / 2.0     # an integer literal never becomes a real
```

## 1.4 Imaginary literals
An `i` suffix on any numeric literal form makes it imaginary. The suffixed
literal is not a base type: it desugars to the `@IImagLit` interface, whose `CORE`
instance builds the `Cpx` struct (see 15). The `i` counts as a suffix only when no
identifier character follows, so `3if` is still `3` then `if`.

```ares
$ unit : Cpx = 1.0i
$ whole: Cpx = 3i              # the magnitude is a Real either way
$ tiny : Cpx = 2.5e-2i         # scientific notation suffixes too
$ z    : Cpx = 3.4 + 1.2i      # the canonical spelling: real plus imaginary
```

An operator takes ONE type for both operands, so a real joins a complex only by
being made one: `CPX.of_real 3.4 + 1.2i`. Nothing is promoted silently, in either
direction. There is no imaginary PATTERN; match the fields (`z |> \ | Cpx.{re, im}`)
or compare with `==` in a guard.

## 1.5 String literals and escapes
A string is a block of bytes; source text must be well-formed UTF-8. Escapes:
`\n \t \r \0 \\ \" \' \a \b \f \v`, `\xHH` (one raw byte), `\u{...}` (a Unicode
scalar, UTF-8 encoded).

```ares
$ hi   : Str = "hello\n"
$ raw  : Str = "\x41\x42"            # "AB" via hex bytes
$ emoji: Str = "\u{1F600} café"      # code point + raw UTF-8
```

## 1.6 String interpolation
`"... ?(expr) ..."` splices an expression into a string. `\?` is a literal `?`, so
`\?(` is not a hole; braces are ordinary characters, so JSON, SQL or JS embed as
written.

What the literal builds is decided by its expected type, as for `[..]`, ranges
and numeric literals. A `@Str`, or no expectation at all, gets the default:
each hole is stringified through `to_string` over its `IToString` instance and
the pieces are concatenated, `chunk ++ to_string expr ++ chunk`.

Simple:
```ares
$ greet : @Str -> @Str = \who => "hello ?(who)!"
```

Involved (a user type interpolates by defining an `IToString` instance):
```ares
$ Point : @struct = x: @Int, y: @Int
$ impl_IToString_for_Point : IToString Point = .{ .show = \p => "(?(p.x), ?(p.y))" }
$ msg : @Str = "at ?(Point.{ .x = 3, .y = 4 }), n=?(40 + 2)"   # "at (3, 4), n=42"
```

**Building another type.** A type with an `@IInterp` instance builds the string
through a builder instead, so its holes stay structured: bound parameters,
escaping, quoting. Three blessed CORE interfaces make up the protocol:

```ares
$ @IInterp     : @struct b t = interp: (b -> b) -> t,   # start a builder, finish it
$ @IInterpText : @struct b   = text: b -> @Str -> b,    # append a literal piece
$ @IInterpHole : @struct b h = hole: b -> h -> b,       # append one hole
```

`"a ?(x) b"` expected as `t` is `@IInterp`'s `interp` applied to a chain that
appends `"a "`, then `x` through the `@IInterpHole b h` instance for its type `h`,
then `" b"`. Each hole resolves its own instance, keyed on the builder and the
hole's type, so `@Int` and `@Str` holes mix with no overloading. The holes are
evaluated first, left to right, and empty pieces are skipped. The expected type
must be known at the literal (a signature, a parameter, an annotation); a string
whose type is still open is a `@Str`.

```ares
$ Sql : @struct = text: @Str, params: @Vec Param
# ... the @IInterp, @IInterpText and @IInterpHole instances for Sql ...
$ by_name : @Str -> Sql = \name =>
    "SELECT id FROM users WHERE name = ?(name)"   # text `... = $1`, params [name]
```

`examples/INTERPOLATION.ares` builds a prepared statement and escaped HTML this
way. Expected as `@Code`, a string is a quotation: source for a compile-time
splice (section 11.2).

---

# 2. Program structure

## 2.1 Module header `@mod`
Every file begins with `$ @mod NAME`. A module name is uppercase with digits, `_`,
and the inert lowercase `x` allowed (so `MAIN`, `LIST_UTIL`, `MODxA`). A module
may span several files (repeat the header).

```ares
$ @mod MAIN
```

## 2.2 Global definitions `$`
`$ name : Type = expr`. The annotation is always required, with no exceptions:
a global's type is its contract with every importer. Leaving it out is a
`TYPE_ANNOTATION_REQUIRED` error whose fix inserts the inferred type. `$a` and
`$ a` are the same.

Simple:
```ares
$ answer : @Int = 42
$ id : t -> t = \x => x
```

Involved (definitions may appear in any order and reference each other):
```ares
$ even : @Int -> @Bool = \n => if n == 0 => @true else odd (n - 1)
$ odd  : @Int -> @Bool = \n => if n == 0 => @false else even (n - 1)
```

## 2.3 Imports `$ @import`
`$ @import MOD` brings a module's public names into scope, bare or qualified.
A qualified `MOD.symbol` needs the import too. An import brings in the whole
module, not one symbol.

```ares
$ @import MATH                 # MATH.sqrt, or bare sqrt
$ @import STR
```

When two imports export the same name, the bare name is ambiguous and the
qualified form picks one (`VEC.len`, `LIST.len`).

`$ @import ALIAS = MOD` imports `MOD` qualified only, under the name `ALIAS`. None
of its names come into scope bare, so several modules with overlapping names
import side by side without ambiguity. Its values, types and constructors are
all reached through the alias (`V.len`, `O.Option @Int`, `O.Option.Some.{ 1 }`).
Writing the module's own name as the alias makes a plain qualified-only import.

```ares
$ @import V = VEC              # V.len, never bare len or VEC.len
$ @import LIST = LIST          # LIST.len, never bare len
```

An alias is one capitalized name. It may not repeat, nor reuse the name of
another imported module, of this module, or of the built-in `C` namespace.

Each `$ @import` names one module; there is no import list.

## 2.4 Visibility `$ @private`
Symbols are public by default. `$ @private` hides every symbol below it, to the
end of the file, from importers. There is no `@public` counterpart, so a
module's public interface sits above the marker.

```ares
$ api : @Int -> @Int = \x => helper x + 1
$ @private
$ helper : @Int -> @Int = \x => x * x    # module-private
```

## 2.5 Entry point
The program entry is `@main`, the one name the compiler knows, and it has exactly
one accepted signature: it takes the argument vector and returns the `@Int` exit
code, performing `@IO`. Together with `@build` (section 11.4), the compile-time
entry, those are the only `@`-names a program may define, besides the blessed
`@I*` interface types, which only `CORE` declares.

```ares
$ @main : @Vec @Str -> <@IO> @Int = \args => 0      # args[0] is the program path
```

A module without `@main` is a library: `ares check` type-checks it and runs its
compile-time assertions, and `ares run`/`build` reject it. Nothing else is
special, so a test harness is ordinary code: a check under `$ &` (section 11)
or a global some tool evaluates by name.

---

# 3. Types

## 3.1 Base numeric and string types
The word-size numbers are `@`-spelled like every other primitive: `@Int`
(signed word), `@Nat` (unsigned word). `Real` and `Str` keep friendly names.
They are distinct nullary constructors.

```ares
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

```ares
$ w : @Int32 = 1000
$ z : @Int32 = w * 2          # @Int32 + @Int32 : @Int32
```

## 3.3 Other `@`-typed built-ins
`@Ptr` (raw pointer), `@Bool` (values `@true`/`@false`), `@Array` (byte block),
`@Vec` (the growable vector, and the default sequence).

```ares
$ flag : @Bool  = @true
$ bytes: @Array = @Array.{ 8 }      # 8 zeroed bytes
$ xs   : @Vec @Int = [1, 2, 3]
```

## 3.4 Unit `{}`
The empty record type is unit; its sole value is also `{}`. Disambiguated by
position (type vs term).

```ares
$ nothing : {} = {}
$ ignore : @Int -> {} = \x => {}
```

## 3.5 Function types and effect rows
`A -> B` is a function. A row between the arrow and result lists the effects a
call may perform: `A -> <E> B`. A bare arrow is pure (empty row). `<| e>` is an
open row (a row variable), which is what makes a function effect-polymorphic.

```ares
$ State : @effect = get : {} -> @Int, put : @Int -> {},
$ pure : @Int -> @Int = \x => x                       # pure (empty row)
$ act  : {} -> <State> @Int = \u => get {}           # may perform State
$ poly : (a -> <e> b) -> a -> <e> b = \f x => f x   # effect-polymorphic
```

## 3.6 Type variables
A lowercase name free in a signature is a type variable, universally quantified
over the whole signature. The same lowercase name repeated is the same variable;
there is no separate backtick spelling.

```ares
$ fst : {a, b} -> a = \t => t.0
$ const : t -> u -> t = \x y => x
```

## 3.7 Tuples
`{A, B, ...}` is a structural product of any arity. `{}` is unit, `{A}` a
one-tuple. Elements read positionally with `.0`, `.1`.

Simple:
```ares
$ p : {@Int, Str} = {42, "answer"}
$ n : @Int = p.0
```

Involved (generic, nested, first-class):
```ares
$ swap : {a, b} -> {b, a} = \t => {t.1, t.0}
$ nested : {@Int, {@Int, @Int}} = {1, {2, 3}}
$ deep : @Int = nested.1.0 + nested.1.1        # 5
```

## 3.8 Records and row polymorphism
A record type may leave its tail open with a row variable:
`{ x: @Int, y: @Int | r }` accepts any record (nominal struct or anonymous) with at
least those fields. Records are structural (unify by field name).

Simple:
```ares
$ area : { x: @Int, y: @Int | r } -> @Int = \p => p.x * p.y
```

Involved (update preserves the open tail; `..p` extends `p` with a field):
```ares
$ shift : { x: @Int | r } -> { x: @Int | r } = \p => { .x = p.x + 10 | p }
$ tag   : { x: @Int | r } -> { x: @Int, t: @Int | r } = \p => { .t = 99, ..p }
```

## 3.9 Sized tensors
`@Tensor n T` is a vector of `n` elements; `n` is a type-level natural (its own
kind). A matrix is a tensor of tensors, `@Tensor m (@Tensor n T)`, which `LA`
names `Matrix m n T`. `@Tensor` heads its application (as an argument it is
parenthesized). A size is a literal, a size variable, or parenthesized arithmetic
(`@Tensor (n + m) a`), decided by a canonical form. See section 9.

```ares
$ v : @Tensor 3 @Int = [10, 20, 30]
$ g : @Tensor 2 (@Tensor 2 @Int) = [ [1,2], [3,4] ]
```

## 3.10 Type aliases `@alias`
`$ Name : @alias params = Type` names a type. It may take parameters and
partially instantiate another generic type. An alias is transparent (unifies
with its expansion).

Simple:
```ares
$ Count : @alias = @Int
```

Involved (partial instantiation):
```ares
$ Pair : @struct a b = fst: a, snd: b,
$ IntPair : @alias b = Pair @Int b               # fix the first arg, keep the second open
$ ip : IntPair Str = .{ .fst = 3, .snd = "z" }  # same as Pair @Int Str
```

An alias names a type. A row has no alias: an effect row or a record row is
written out where it is used.

---

# 4. Expressions and syntactic sugar

## 4.1 Lambdas
`\pat ... => e`. Parameters are juxtaposed and curried sugar. A parameter may
be an irrefutable pattern, and may carry a `: Type` annotation, always inside
parens, `(x: T)`, even when it is the only parameter, so the type ends at `)`.
(A lowercase name in a type is a type variable, so a bare annotation would
swallow the next parameter as its argument.)

```ares
$ Person : @struct = name: Str, age: @Int,
$ inc : @Int -> @Int = \x => x + 1
$ add : @Int -> @Int -> @Int = \a b => a + b                  # curried
$ name : Person -> Str = \Person.{ name, _ } => name          # destructuring parameter
$ dec : @Int -> @Int = \(x: @Int) => x - 1                     # annotated parameter
$ sub : @Int -> @Int -> @Int = \(a: @Int) (b: @Int) => a - b  # one annotation per parameter
```

`\x = e` and `\x: T => e` are errors with fix-its. The three tokens have one
job each: `=` names something (`$ x = e`, `let x = e`), `=>` abstracts over a
pattern (lambdas and every kind of arm), `->` is the function type.

## 4.2 Application
Juxtaposition, left-associative, binds tighter than any operator.

```ares
$ inc : @Int -> @Int = \x => x + 1
$ add : @Int -> @Int -> @Int = \a b => a + b
$ r : @Int = add (inc 2) 3   # (add (inc 2)) 3
```

## 4.3 Arithmetic and unary operators
`+ - * / %`, unary `-` (negation) and `!` (boolean not). Standard precedence,
`( )` groups.

```ares
$ a : @Int  = 1 + 2 * 3 - 4      # 3
$ b : @Int  = (1 + 2) * (3 - 4)  # -3
$ c : @Int  = -42
$ d : @Bool = !@true             # @false
```

The bitwise operators work on every integer type: `&&&` (and), `|||` (or),
`^^^` (xor), `<<<` and `>>>` (shifts by an `@Int` count) and unary `~~~`
(complement). `&&&` and the shifts bind like `*`, `|||` and `^^^` like `+`, so
all bind tighter than a comparison. `>>>` is arithmetic on the signed types and
logical on the nats. A count of 64 or more, or a negative one, shifts every bit
out. A narrow type wraps what leaves its range.

```ares
$ e : @Int  = 12 &&& 10 ||| 1    # 9
$ f : @Int  = (0 - 16) >>> 2     # -4
$ g : @Nat8 = ~~~200             # 55
```

## 4.4 Comparisons
`==` `!=` `<` `>` `<=` `>=`, all `a -> a -> @Bool`.

```ares
$ lt : @Bool = 3 < 8
$ le : @Bool = 8 <= 8
$ ne : @Bool = 3 != 4
```

Like arithmetic, these are `CORE` functions over interfaces: `==` and `!=` over
`IEq`, the four orderings over `IOrd`, whose instances bottom out at the
comparison intrinsics (`@ieq`, `@ilt`, `@ult`, `@feq`, `@flt`, `@seq`, `@slt`).
A type of your own joins them with an instance:

```ares
$ Money : @struct = cents: @Int

$ impl_IOrd_for_Money : IOrd Money = .{ .lt = \a b => a.cents < b.cents }
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

```ares
$ s  : Str = "Hello" ++ " " ++ "world"
$ xs : @Vec @Int = 1 :: 2 :: 3 :: []
```

`::` builds a fresh vector, so it is O(n) and a `::`-recursion is quadratic;
`VEC.push` is the idiomatic way to build one up.

## 4.6 Short-circuit `&&` / `||`
Desugar to a lazy `if`, so the right operand runs only when needed.

```ares
$ safe : @Int -> @Bool = \n => n > 0 && 100 / n > 5
```

## 4.7 Conditional `if ... => ... else`
`if c => t else e`. The condition is a `@Bool`. One `if` has one arm; several
chain with `else if`. It is lazy (only the taken branch runs) and is not a
pattern match.

Simple:
```ares
$ abs : @Int -> @Int = \n => if n > 0 => n else -n
```

Involved (else-if chain):
```ares
$ sign : @Int -> @Int = \n =>
	if n == 0 => 0
	else if n > 0 => 1
	else -1
```

## 4.8 Local bindings `let ... in`
`let pat = e in body`. One `let` has one binding; several nest, each scoping
the rest (`let a = 1 in let b = a + 1 in b`). A binding is recursive exactly when its name occurs in its own body
(no `let rec`). A binder may carry a type annotation.

Simple:
```ares
$ x : @Int = let a = 6 in a * 7
```

Involved (chain, destructuring, recursion, annotation):
```ares
$ r : @Int =
	let {a, b} = {3, 4} in
	let s = a + b in
	s * 2
$ len : @Vec @Int -> @Int =
	let go : @Vec @Int -> @Int -> @Int = \l n =>
		l |> \ | _ :: t => go t (n + 1) | _ => n
	 in \l => go l 0
```

## 4.9 Sequencing `;`, pipes `|>` `<|`, composition `<|>`
Parser sugar, lowest precedence. `a ; b` = `let _ = a in b`. `x |> f` = `f x`
(left-assoc). `f <| x` = `f x` (right-assoc).

`f <|> g` composes, and is the lambda it means: `\x => f (g x)`. It is
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

```ares
$ inc : @Int -> @Int = \x => x + 1
$ double : @Int -> @Int = \x => x * 2
$ step1 : {} -> @Int = \u => 0
$ step2 : {} -> @Int = \u => 0
$ eff : @Int = step1 {} ; step2 {} ; 0     # run for effect, return 0
$ y   : @Int = 5 |> inc |> double          # double (inc 5)
$ z   : @Int = double <| inc <| 5          # same
$ f   : @Int -> @Int = double <|> inc      # \x = double (inc x)
$ w   : @Int = (double <|> inc) 5          # 12
$ shout : @Str -> <@IO> {} = \s => IO.println s
$ show  : @Int -> <@IO> {} = shout <|> to_string   # the row is the union
$ h2    : @Int -> @Int = (<|>) double inc          # = compose double inc
```

## 4.10 Operators as functions `(op)`
An operator in parentheses is a plain function value, the same binding a
`$ (op) : ... = ...` definition binds. It curries and passes as an argument; since
it takes a context parameter, the type it is used at decides which instance is
injected, so a bare `(+)` with nothing to pin its type is an error that says so.

```ares
$ two   : @Int = (+) 1 1                   # 1 + 1
$ add1  : @Int -> @Int = (+) 1             # curried
$ plus  : @Int -> @Int -> @Int = (+)       # the instance comes from the type
$ total : @Int = VEC.foldl (+) 0 [1, 2, 3] # as an argument
```

The parens must hold exactly one operator, so `(a + b)` and `(-1)` still group.
`|` and `<>` are grammatical delimiters, not operators, and have no function
form. An operator the parser rewrites rather than calls (`&&`, `||`, `;`, `|>`,
`<|`) has no binding of its own, so `(op)` is the function `\l r => l op r`;
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

```ares
$ big   : @Int64 = 300
$ small : @Int32 = @cast big              # reinterpret across integer widths
$ back  : @Int    = @cast small            # and back to the platform word
```

## 4.12 Typed holes `???`
`???` stands for an expression not written yet. It takes whatever type its
context needs, and it is always a compile error (`TYPED_HOLE`) that names that
type and lists the names in scope that fit it, each offered as a fix. A hole is
how to ask the compiler what goes in a spot.

```ares
$ f : @Int -> @Int = \n => n + ???
# error[TYPED_HOLE]: hole `???` has type `@Int`
# note: in scope with this type: `n`
```

---

# 5. Pattern matching

## 5.1 The multi-arm lambda and matches
`\ | pat => e | pat => e ...` is a lambda matched against the arms top to
bottom; the first matching pattern wins and binds its variables. A match is the
multi-arm lambda applied to the scrutinee, usually with `|>`:
`scrut |> \ | pat => e ...`. There is no default branch: a catch-all is an
ordinary arm whose pattern matches anything, `| _ => d` (or `| x => d` to use
the value). Arms belong to the innermost multi-arm lambda or handler, so a
nested one is parenthesized.

An arm lists one pattern per argument, separated by commas: `\ | 0, _ => ...`
takes two curried arguments and matches both at once. Every alternative of
every arm has the same number of patterns. The comma separates arguments and
never builds a tuple; a tuple argument is matched with `{ a, b }`, which is one
argument. A handler's value arms match its one value, so they take one pattern.

Simple:
```ares
$ classify : @Int -> @Str = \ | 0 => "zero" | 1 => "one" | _ => "many"
$ yn : @Str -> @Int = \s => s |> \ | "yes" => 1 | "no" => 0 | _ => 99
$ take : @Int -> List t -> List t = \
    | 0, _       => []
    | _, []      => []
    | n, x :: xs => x :: take (n - 1) xs
$ total : { @Int, @Int } -> @Int = \ | { 0, acc } => acc | { n, acc } => total { n - 1, acc + n }
```

Involved (nested union patterns, exhaustive so no catch-all, see 5.9):
```ares
$ Wrap : @union = Empty: {}, W: { Opt }
$ Opt  : @union = None: {}, Some: { @Int }
$ unwrap : Wrap -> @Int = \
    | Wrap.Empty                => 0
    | Wrap.W.{ Opt.None }       => 1
    | Wrap.W.{ Opt.Some.{ n } } => n
```

An empty match, on a value of an uninhabited type, is `x |> @absurd`.

## 5.2 Literal patterns
Match integers (any width), Real, and Str by equality (refutable). A numeric
literal may be negative (`-1`, `-0.5`), also as a range bound (`-10 ... -1`).

```ares
$ describe : @Int -> Str = \n => n |> \ | 0 => "z" | 1 => "o" | -1 => "m1" | _ => "m"
$ yn : Str -> @Int = \s => s |> \ | "yes" => 1 | "no" => 0 | _ => 99
```

A string literal can be a piece of a `++` pattern (5.7): `"GET " ++ path`
strips a prefix, `name ++ ".ares"` a suffix, and `"<" ++ body ++ ">"` both.

## 5.3 Wildcard and variable patterns
`_` matches anything binding nothing; a lowercase name binds the value.

```ares
$ tag : @Int -> @Int = \n => n |> \ | 0 => 100 | m => m + 1
```

## 5.4 Struct patterns
`Type.{ ... }` matches a struct. Positional (bare fields, in order, all listed)
or named (dotted, any order, others ignored; a lone `.name` puns).

```ares
$ Point  : @struct = x: @Int, y: @Int,
$ Person : @struct = name: Str, age: @Int,
$ sum_xy : Point -> @Int = \p => p |> \ | Point.{ x, y } => x + y
$ who    : Person -> Str = \p => p |> \ | Person.{ .name } => name
```

## 5.5 Variant patterns
`Type.Tag` (unit) or `Type.Tag.{ payload }`. May be written bare (`.Tag`), with
the union inferred from the arms. Payloads nest.

```ares
$ Maybe : @union t = Just: t, None: {}
$ get : @Int -> Maybe @Int -> @Int = \d m => m |> \ | Maybe.Just.{ x } => x | _ => d
$ isJust : Maybe t -> @Bool = \m => m |> \ | .Just.{ _ } => @true | _ => @false
```

## 5.6 Range patterns
`lo ... hi` matches the inclusive interval (bounds are literals of the
scrutinee's type). Open `lo ...` matches `lo <= x`. Refutable, binds nothing.

Simple:
```ares
$ grade : @Int -> Str = \n => n |> \ | 90 ... 100 => "A" | 60 ... 89 => "C" | _ => "F"
```

Involved (open range, Real):
```ares
$ sign : @Int -> Str = \n => n |> \ | 0 ... => "nonneg" | _ => "neg"
$ band : Real -> @Int = \x => x |> \ | 0.0 ... 0.5 => 1 | 0.5 ... 1.0 => 2 | _ => 0
```

## 5.7 Sequence and array patterns
A closed `[a, b]` matches a sequence of exactly those elements, and `[]` an
empty one. `++` splits a sequence into fixed pieces around at most one part of
unknown length: `pre ++ mid ++ suf`. A fixed piece is a closed `[..]` list or,
on a `@Str`, a string literal; adjacent pieces merge (`[1] ++ [2]` is `[1, 2]`).
`h :: t` is shorthand for `[h] ++ t`.

Pieces before the middle step the scrutinee's `@ISeqView` from the front;
pieces after it step its `@ISeqViewBack` from the back. `@Vec` (the default),
`@Array`, and `@Str` (as its bytes) have both, `CORE`'s cons `List` only the
front one, and any type of your own joins with an instance. Two parts of unknown
length (`"a" ++ x ++ "b" ++ y`) are an error: the sequence could split between
them in more than one way.

Simple:
```ares
$ sum : @Vec @Int -> @Int = \xs => xs |> \ | [] => 0 | h :: t => h + sum t
```

Involved (a prefix, a suffix, and both ends):
```ares
$ second : @Vec @Int -> @Int = \xs => xs |> \ | [_, x] ++ rest => x | _ => -1
$ ext    : @Str -> @Str = \s => s |> \ | name ++ ".ares" => name | _ => s
$ ends   : @Array -> @Int = \a => a |> \ | [x] ++ mid ++ [z] => x + z | _ => 0
```

A sized tensor `@Tensor n T` (3.9) knows its length, so its patterns are checked
at compile time instead of tested: `[x, y, z]` covers a `@Tensor 3 T` with no `_`
arm, a wrong length is a type error, and the middle of a split gets an exact size
(`[a] ++ r` on a `@Tensor 3 T` binds `r : @Tensor 2 T`). The size must be concrete there.
A rank-2 tensor splits along its first axis, one row per element.

```ares
$ sum3 : @Tensor 3 @Int -> @Int = \t => t |> \ | [x, y, z] => x + y + z
```

## 5.8 Or-patterns and guards
`| p1 | p2 => e` shares one body (alternatives bind no variables). `| pat if g
=> e` adds a boolean guard; a failed pattern or guard falls through to the next
arm, even one with the same constructor.

Or-pattern:
```ares
$ small : @Int -> @Int = \n => n |> \ | 0 | 1 | 2 => 1 | _ => 0
```

Guards falling through the same constructor:
```ares
$ Box : @union = Some: @Int, Nil: {},
$ grade : Box -> @Int = \x =>
	x |> \ | Box.Some.{ v } if v > 100 => 3
	     | Box.Some.{ v } if v > 0   => 2
	     | Box.Some.{ _ }             => 1
	     | Box.Nil.{}                 => 0
```

## 5.9 Exhaustiveness
Every match, and every set of handler value arms, is checked after type
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
  Real ranges and string `++` patterns never cover a whole domain.
- A union with its own `@ISeqView` cannot be matched with both its variants and
  sequence patterns in one position (only `List` can); that is reported.
- A sequence pattern (`[]`, `h :: t`, `[a] ++ r`) is checked through the
  sequence view, so `[]` and `_ :: _` together cover any sequence. A piece
  matched from the back (`r ++ [z]`) is not modeled, so it needs a `_` arm.

```ares
$ Light : @union = Red: {}, Yellow: {}, Green: {}
$ go : Light -> @Int = \l =>
	l |> \ | Light.Red => 0 | Light.Yellow => 1 | Light.Green => 2   # no catch-all needed
```

## 5.10 Irrefutable patterns in `let` and lambda
A `let` binder or a lambda parameter has no next arm to fall through to, so its
pattern must match every value of its type. `_`, variables, and struct, tuple,
and record patterns built from them always do. A pattern that can fail, such as
a literal or a variant of a union with several, is a compile error
(`NON_EXHAUSTIVE_MATCH`) naming a value it misses; match it instead,
`e |> \ | pat => ... | _ => ...`. For a lambda's one unannotated parameter
the error carries that rewrite as a fix: `\P => B` becomes
`\ | P => B | _ => ???`. There is no `let ... else`: `let` is a definition, not
sugar for applying a lambda.

```ares
$ Point  : @struct = x: @Int, y: @Int,
$ Person : @struct = name: Str, age: @Int,
$ Line   : @struct = from: Point, to: Point,
$ person : Person = Person.{ .name = "A", .age = 30 }
$ age : @Int = let Person.{ .age = a } = person in a
$ start : Line -> @Int = \Line.{ .from = Point.{ x, _ }, .to = _ } => x
```

---

# 6. Algebraic data types

## 6.1 Structs `@struct`
A named record type: `$ Name : @struct params = field: Type, ...`. Built with a
literal (`Type.{ .f = e }`, bare `.{ ... }`, or positional `Type.{ a, b }`);
read with `.field`. Nominal (unifies by name).

Simple:
```ares
$ Person : @struct = name: Str, age: @Int,
$ p : Person = Person.{ .name = "Will", .age = 21 }
$ who : Str = p.name
```

Involved (generic, applied by juxtaposition, nested field types):
```ares
$ Box : @struct t = val: t,
$ Wrap : @struct t = inner: Box t,
$ w : Wrap @Int = .{ .inner = Box.{ .val = 7 } }
$ deep : @Int = w.inner.val
```

## 6.2 Record update
`Type.{ .field = e | base }` copies unlisted fields from `base`. The `| base`
comes last; a listed field may read from `base`.

```ares
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
```ares
$ Maybe : @union t = Just: t, None: {}
$ some : Maybe @Int = Maybe.Just.{ 5 }
$ none : Maybe @Int = .None
```

Involved (recursive, multi-field payload, positional or named):
```ares
$ Tree : @union a = Leaf: a, Node: { Tree a, Tree a }
$ t : Tree @Int = Tree.Node.{ Tree.Leaf.{ 1 }, Tree.Leaf.{ 2 } }
```

## 6.4 Generated members `&`
An entry `&X` of a `@struct` or `@union` runs `X` at build time and splices the
fields or variants its `@Code` spells in its place.
DERIVE's `fields_of` and `variants_of` copy another declaration's members, a
copy with no subtyping relationship. A copied type may itself be generated; a
generic one is applied to the new type's parameters (`fields_of "Pair b"`). A
duplicate member is an error.

```ares
$ @import DERIVE
$ Point  : @struct = x: @Int, y: @Int
$ Point3 : @struct = &DERIVE.fields_of "Point", z: @Int   # x, y, then z
$ Base   : @union  = Red: {}, Green: {}
$ Color  : @union  = &DERIVE.variants_of "Base", Blue: {} # Red, Green, then Blue
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
```ares
$ Stream : @struct t = head : t, tail : Stream t
$ first : Stream @Int -> @Int = \s => s.head
$ next  : Stream @Int -> Stream @Int = \s => s.tail
```

Involved (map over an unbounded stream; `tail` is only built when read):
```ares
$ smap : (a -> b) -> Stream a -> Stream b = \f s =>
	{ .head = f s.head, .tail = smap f s.tail }
$ tenth : @Int = (smap (\x => x + x) (count_from 1)).tail.tail.head
```

A recursive `@union` works the same way, which is how a `List` is consumed one
cons at a time:
```ares
$ from : @Int -> List @Int = \n => List.Cons.{ n, from (n + 1) }
$ head : List @Int -> @Int = \l => l |> \ | List.Cons.{h, t} => h | _ => 0
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

```ares
$ add : @Int -> @Int -> @Int = \a b => a + b
$ add5 : @Int -> @Int = add 5           # partial application
```

## 7.2 Higher-order functions
Functions are first-class values.

```ares
$ apply_twice : (t -> t) -> t -> t = \f x => f (f x)
$ compose : (b -> c) -> (a -> b) -> a -> c = \f g x => f (g x)
```

## 7.3 One name, one definition
A name has one definition and one type. Two definitions of a name in one module is
an error; a local definition shadows an imported one of the same name; and a name
two imports bring in has no bare meaning, so a use of it says so and names the
owners (`A.name` and `B.name` still reach either). An operation that covers several
types is an interface (§7.4), not several definitions.

A local binder (a lambda parameter, a `let`, a pattern variable, a record
parameter's field, a handler binder) cannot reuse the name of a global defined in
the same module (`SHADOWED_GLOBAL`), so inside a module a global's name always
means that global. Imported names may be shadowed locally.

## 7.4 Interfaces and `@use` parameters
An interface is an ordinary `@struct` whose fields are its operations; implementing
it is defining a value of that type. A function that needs one declares a `@use`
parameter, a record of named fields, each an instance it needs. Call sites do not
write it: the compiler finds each field BY TYPE, taking the nearest binder in scope
and otherwise the one instance in scope (two would be an error, since global scope
is flat). The fields are in scope by name in the body, so the body's lambda starts
at the first explicit parameter. It elaborates to a leading argument, so the
backends need no support.

Simple:
```ares
$ Ordering : @union = LT: {}, EQ: {}, GT: {}
$ IOrder : @struct t = compare: t -> t -> Ordering,
$ impl_IOrder_for_int : IOrder @Int = .{
	.compare = \a b => if a < b => Ordering.LT else if a > b => Ordering.GT else Ordering.EQ,
}
$ max_of : @use {d: IOrder t} -> t -> t -> t = \x y =>
	d.compare x y |> \ | Ordering.GT => x | _ => y
$ biggest : @Int = max_of 3 7
```

A signature may have any number of `@use` parameters, anywhere on its arrows, and
each names one or more fields; a field's type must be a declared struct or union
applied to its arguments, and field names are unique across the signature:

```ares
$ abs : @use {o: IOrd t, s: ISub t, z: IZero t} -> t -> t = \x =>
	if o.lt x z.zero => s.sub z.zero x else x
$ floor_at : @use {o: IOrd t} -> t -> @use {z: IZero t} -> t -> t = \x y =>
	if o.lt x z.zero => y else x
```

Calls inside the body find the fields by search, so they pass down without being
written. An explicit `@use {.f = e}` argument names the fields it supplies (the
rest are still searched) and goes where its parameter stands, before the explicit
argument that follows it:

```ares
$ Ordering : @union = LT: {}, EQ: {}, GT: {}
$ IOrder : @struct t = compare: t -> t -> Ordering,
$ impl_IOrder_for_int : IOrder @Int = .{
	.compare = \a b => if a < b => Ordering.LT else if a > b => Ordering.GT else Ordering.EQ,
}
$ max_of : @use {d: IOrder t} -> t -> t -> t = \x y =>
	d.compare x y |> \ | Ordering.GT => x | _ => y
$ max3 : @use {d: IOrder t} -> t -> t -> t -> t = \x y z =>
	max_of (max_of x y) z          # d satisfies max_of's requirement, so it passes down
$ as_min : @Int =
	let flip : IOrder @Int = .{ .compare = \a b => impl_IOrder_for_int.compare b a } in
	max_of @use {.d = flip} 3 7
```

`@use` stands only on a global definition's own signature: not inside a
parameter's type (`(@use {d: IOrd t} -> t) -> t`), an annotation, a field type, or
(for now) a local `let`'s signature. `@use {}` is an error. `@ctx` is reserved:
with dependent types (#406) it becomes the binder of parameters solved by
unification (`@ctx {t: @Type}`), and writing it today is an error that points here.

An instance may itself take a context, which resolution fills in recursively, so
`IEq (@Vec t)` is built from `IEq t`. `CORE`'s operators are all ordinary functions
over such interfaces: `+ - * / %` over `IAdd`/`ISub`/`IMul`/`IDiv`/`IMod`, `^` over
`IPow`, `== != < > <= >=` over `IEq`/`IOrd`, `++` over `ICat`, `::` over `ICons`, and
`to_string` over `IToString`. A type joins any of them by defining an instance:

```ares
$ Money : @struct = cents: @Int
$ impl_IAdd_for_Money : IAdd Money = .{ .add = \a b => Money.{ .cents = a.cents + b.cents } }
$ total : Money = Money.{ .cents = 1 } + Money.{ .cents = 2 }
```

See `documentation/interfaces.md` for the full rules, the diagnostics, and the
blessed `@I*` interfaces the compiler's own desugars resolve.

## 7.5 Tail-call optimization
Tail-recursive calls (self, mutual, or through a recursive local `let`) run in
constant stack.

```ares
$ sum_to : @Int -> @Int -> @Int = \n acc =>
	if n == 0 => acc else sum_to (n - 1) (acc + n)      # constant stack at any depth
```

---

# 8. Algebraic effects and handlers

## 8.1 Declaring an effect `@effect`
`$ Name : @effect = op : Arg -> Resume, ...`. Each operation takes one argument
and names its resume type. An operation whose handler never resumes (an
exception) names a type variable as its result.

```ares
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

```ares
$ State : @effect s = get : {} -> s, put : s -> {},
$ Async : @effect m e = spawn : ({} -> <Async m e | e> {}) -> {}, send : m -> {},
$ count : {} -> <State @Int> @Int = \u => let n = get {} in put (n + 1); n
```

An operation's type may also use variables the effect does not declare, like
`throw` above. Each use chooses those types, so a handler clause sees them as
opaque: it can pass such a value along (resume with the argument it was given,
store it), but it cannot make one up or inspect it. A clause that needs a
concrete type takes it from an effect parameter instead.

```ares
$ Exn : @effect = throw : @Str -> a,
$ safe : @Int = do 1 + Exn.throw "no" ctl k | Exn.throw m => 0     # aborts: fine
# `| Exn.throw m => k 0` is an error: `k` wants `a` of `Exn.throw`, not @Int
```

## 8.2 Performing an operation
Just call it; there is no `perform` keyword. A use injects the effect into the
ambient row.

```ares
$ State : @effect = get : {} -> @Int, put : @Int -> {},
$ tick : {} -> <State> @Int = \u => let x = get {} in let _ = put (x + 1) in x
```

## 8.3 Handling `do ... ctl`
`do body ctl k | op a => e ... | pat => e ...`. `k` is the captured
continuation, shared by every clause. Each `| op a => e` handles an operation. A
value arm `| pat => e` runs on normal completion, matching the body's final value
like a match arm; with none, that value is the handler's result. An operation
clause always names an operation and its argument (`| op a`, `| Eff.op a`),
which no pattern can look like, so the two kinds of arm need no marker. A shared
operation name is qualified in a clause head.

Exception (ignores `k`, so it resumes zero times):
```ares
$ safeDiv : @Int -> @Int -> @Int = \a b =>
	do if b == 0 => Exn.throw "div0" else a / b
	ctl k | Exn.throw msg => -1
```

Generator (resume once per yield, summing results):
```ares
$ sumGen : ({} -> <Yield> {}) -> @Int = \gen =>
	do gen {}
	ctl k | Yield.yield v => v + k {}
	      | _ => 0
```

## 8.4 Resuming
Resuming is applying `k`. `k v` splices the suspended computation back and
delivers `v`. `k` may be resumed **any number of times**, and may be stored
before use, which is what coroutines need. Handlers are deep (a resumed
computation is still governed by the same handler).

```ares
$ Task : @union = Fin: {}, Susp: { @Int, {} -> Task }
$ spawn : ({} -> <Yield> {}) -> Task = \t =>
	do t {}
	ctl k | Yield.yield v => Task.Susp.{ v, k }     # store k, resume later
	      | _ => Task.Fin.{}
```

Resuming more than once runs the rest of the computation once per resume, each on
its own copy, which is what nondeterminism and backtracking are:

```ares
$ sum_all : ({} -> <Amb> @Int) -> @Int = \body =>
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

```ares
$ map : (a -> <e> b) -> @Vec a -> <e> @Vec b = \f xs =>
	xs |> \ | [] => [] | h :: t => f h :: map f t
```

## 8.6 `@finale`
`@finale cleanup body` runs `body {}`, then `cleanup {}` when the body's scope
exits: on normal completion, on an abort by an outer non-resuming handler, or
when a stored continuation holding it completes. Nested finales run
innermost-first. It is an ordinary CORE function, not syntax, so the parser has
no rule for it and it can be partially applied and passed around. The
resource-safe FFI idiom.

```ares
$ open  : @Str -> @Int = \path => 0        # stand-ins for real handles
$ close : @Int -> {} = \h => {}
$ read  : @Int -> @Int = \h => h
$ useFile : @Str -> @Int = \path =>
	let f = open path in
	@finale (\_ => close f) <| \_ =>
	read f
```

Several cleanups nest as separate `@finale`s, and run innermost-first (here
`close g`, then `close f`):
```ares
$ useTwo : @Str -> @Str -> @Int = \a b =>
	let f = open a in
	let g = open b in
	@finale (\_ => close f) <| \_ =>
	@finale (\_ => close g) <| \_ =>
	read f + read g
```

Both arguments are thunks, because Ares is call-by-value: `@finale (close f)`
would close the file before the body ran. The cleanup returns `{}`, and its row
is the body's, so a pure function passed directly where the body performs
effects needs a lambda around it (`\_ => closeWindow {}`). A library wraps it
in a bracket function so callers never see the handle's cleanup:
```ares
$ with_file : @Str -> (@Int -> @Int) -> @Int = \path body =>
	let f = open path in
	@finale (\_ => close f) <| \_ => body f
```

---

# 9. Sized tensors and linear algebra

## 9.1 Tensor literals and indexing
`@Tensor n T` is built from a `[..]` literal whose length fixes `n` (a mismatch
is a type error). Read with modular indexing `t.[i]` (the index reduces mod the
size, so it is total, `t.[n]` wraps to `t.[0]`). Indexing an empty `@Tensor 0 T`
is the sole fault.

```ares
$ @import LA
$ v : @Tensor 3 @Int = [10, 20, 30]
$ a : @Int = v.[1]        # 20
$ b : @Int = v.[3]        # wraps to v.[0] = 10
```

## 9.2 Multi-dimensional and multi-axis indexing
A matrix is a tensor of tensors, `Matrix m n T` = `@Tensor m (@Tensor n T)`.
`t.[i, j]` reads one element (`t.[i].[j]`).

```ares
$ @import LA
$ g : Matrix 2 2 @Int = [ [1,2], [3,4] ]
$ z : @Int = g.[1, 0]     # 3
```

## 9.3 Size arithmetic
A size may be `+`/`*` over literals and size variables, evaluated mod 2^64 and
compared by a canonical form (`n + m` equals `m + n`; `n + n` is `2 * n`). This
drives size-changing operations, whose result size is computed at compile time.

```ares
$ @import LA
# LA provides `concat`, its result size computed at compile time:
#   concat : @Tensor n a -> @Tensor m a -> @Tensor (n + m) a
$ joined : @Tensor 5 @Int = concat [1, 2] [3, 4, 5]      # 2 + 3 = 5
```

## 9.4 Slicing
Multi-axis slicing `t.[s0, s1, ...]`: `i` indexes an axis, `p ... q` narrows it
to an inclusive range, `..` keeps it whole. Every result is an O(1) strided view.

```ares
$ @import LA
$ row : Matrix m n a -> @Int -> @Tensor n a = \m i => m.[i]
$ col : Matrix m n a -> @Int -> @Tensor m a = \m j => m.[.., j]
$ sub : @Tensor 8 @Int -> @Tensor 4 @Int = \m => m.[2 ... 5]      # inclusive 2..5, four elements
```

## 9.5 Tensor primitives and the `LA` library
The compiler provides only `@`-primitives over the buffer; every named operation
lives in `LA`. `t.[i]` desugars to the `@IIndex` interface and a lone
`t.[lo ... hi]` to `@ISlice`, so a container of your own joins the `.[..]` surface
by defining an instance. A multi-axis slice stays tensor-only: its result shape is
computed from the slots.

```ares
$ transpose : Matrix m n a -> Matrix n m a = \t =>
	@tensor_create (@tensor_index t 0)
		(\j => @tensor_create t (\i => @tensor_index (@tensor_index t i) j))
```

## 9.6 Expression-form ranges
`[lo ... hi]` builds whatever the resolved `@IRange` instance returns, and the open
`[lo ...]` whatever `@IRangeFrom` returns; `CORE` has instances for `@Vec`/`List`
and `Stream`/`List`. The expected type picks one, so a range of your own type needs
only its own instance. When nothing constrains it, the **first instance declared**
wins, which is how `CORE`'s order (not the compiler) sets what a bare range builds.
A sized tensor is the exception: literal bounds fix `n`, and the compiler builds it
directly.

```ares
$ tv : @Tensor 4 @Int = [1 ... 4]     # tensor, n = 4
$ ns : @Vec @Int      = [1 ... 5]     # vector (the bare closed range)
$ s  : Stream @Int    = [1 ...]       # unbounded (the bare open range)
```

A `@Vec` range materializes every element, so a large bound costs that much work
up front. The `List` instances are lazy in the tail, which is what makes a range
you only walk part of cheap:

```ares
$ first5 : <@IO> {} =
	for ([1 ... 100000000000000] : List @Int)
		(\i => if i > 5 => break {} else IO.print "?(i), ")
```

---

# 10. Foreign function interface

## 10.1 `@extern` binding
A foreign binding is a global whose body is `@extern "C" "symbol" "lib"`. A
signature is required and drives C-ABI marshalling. An extern takes exactly one
argument (one arrow).

```ares
$ puts : Str -> @Int = @extern "C" "puts" "libc"
$ used : @Int = puts "hi"
```

## 10.2 Multi-parameter C functions
A C function is not curried, so several parameters group into a record; its
fields, in declaration order, are the positional C arguments (marshalled by name,
so a reordered call site is fine). A nullary C function takes `{}`.

```ares
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

```ares
$ Color : @struct @extern "C" = r: @Nat8, g: @Nat8, b: @Nat8, a: @Nat8,
$ clear : Color -> {} = @extern "C" "ClearBackground" "libraylib.so"
$ go : {} = clear Color.{ .r = 0, .g = 0, .b = 0, .a = 255 }
```

## 10.4 Callbacks and array parameters
A function-typed parameter is a C function pointer: pass a Ares closure. A
`@Vec T` parameter (T a C-repr struct) is passed as a packed `T*` with a separate
count.

```ares
$ Vector2 : @struct @extern "C" = x: @Float32, y: @Float32,
$ sort : {@Ptr, @Int, (@Int -> @Int -> @Int)} -> {} = @extern "C" "qsort_r" "libc"
$ draw : {@Vec Vector2, @Int} -> {} = @extern "C" "DrawLineStrip" "lib"
```

## 10.5 The `C` namespace and engine intrinsics
`library/C.ares` binds libc/libm as the auto-injected `C` namespace (used
qualified, `C.sqrt`, no import). It also holds a few conversions the language
cannot express with `@cast` (which is integer widths only): `C.i2f`/`C.f2i`
(@Int and @Float32), `C.i2d`/`C.d2i` (@Int and Real), `C.i2p`/`C.p2i` (@Int to
@Ptr and back), `C.ptr_add` (an address `n` bytes on), and `C.null` (the null
pointer). These are provided by the engines' runtimes (an
empty `lib`); see `documentation/native-backend.md`.

```ares
$ x : @Float32 = C.i2f 41
$ nul : @Ptr = C.null
```

Limitation: variadic C functions (`printf(fmt, ...)`) have no single binding.
Declare a fixed-arity binding per argument shape you use; there is no `...`
marker.

## 10.6 Reading C memory
C memory reaches Ares two ways, both copies:

- **Bytes**: `@ptr_bytes p n` (CORE) is a fresh `@Str` of the `n` bytes at `p`,
  NUL bytes and all. (A `@Str` *result* of a C function is a `char*` and ends at
  its first NUL.) This is how `IO.read_file` reads: `C.read` into `C.malloc`
  memory, then `@ptr_bytes`.
- **Typed elements**: a binding with `@extern "load"` (no symbol, no library)
  reads element `i` of a C array of `T` at `p`. Its type must be
  `{ p: @Ptr, i: @Int } -> T`, where `T` is a sized number, `@Int`/`@Nat`, `@Ptr`,
  `@Str` (a `char*`, copied up to its NUL; NULL reads as `""`), or a
  `@struct @extern "C"` type. A C `bool` loads as `@Nat8`. `VEC.from_c at p n`
  collects the first `n` elements as a vector.

```ares
$ Point : @struct @extern "C" = x: @Int32, y: @Int32,
$ point_at : { p: @Ptr, i: @Int } -> Point = @extern "load"
$ arg_at : { p: @Ptr, i: @Int } -> @Str = @extern "load"      # a `char**`

$ points : @Ptr -> @Int -> @Vec Point = \p n => VEC.from_c point_at p n
```

Like a C dereference, a load trusts the program: the memory must hold `n`
valid elements (a load from NULL faults; anything else out of bounds is
undefined).

## 10.7 Builtins: `@extern "ares"`
The primitives the compiler and runtime implement (`@iadd`, `@vec_push`,
`@chan_send`, `@lex`, ...) are declared in `library/CORE.ares`, each with its
signature and a comment:

```ares
$ @iadd : t -> t -> t = @extern "ares"
$ @vec_push : @Vec a -> a -> @Vec a = @extern "ares"
```

The definition's name is the builtin, so there is no symbol or library string,
and a reference compiles to a direct call with no wrapper. Unlike a C extern, a
builtin is curried. Only CORE may declare one, and no module may redefine one.
The operators the parser rewrites (`;`, `|>`, `<|`) are declared the same way, for
their types: `$ (|>) : a -> (a -> <e> b) -> <e> b = @extern "ares"`. A few primitives stay inside the
compiler because their types have no surface syntax: `@cast` is checked
specially, and the `@tensor_*` primitives are typed in the compiler (#342).

---

# 11. Compile-time evaluation

## 11.1 `&`, the splice
`&X` runs `X` at build time and replaces itself with the result:

- an `@Code` is spliced in as source, usually written as a quotation
  (section 11.2);
- first-order data (a number, a `@Bool` or a `@Str`) is inserted as a literal.

The inserted source is then checked like hand-written code. Any other result,
such as a function, is an error at the `&`. In type, row, member and item
position only code makes sense, so there the operand is expected as `@Code`
and a string literal is a quotation; in expression and pattern position a
string is data.

`&` is syntax, not a function: there is no `(&)` section, no `x |> &`, and no
`$ (&)` definition. It binds like the other prefix operators, so it scopes over
an application: `&f a b` splices `f a b`, `&f a ++ b` is `(&(f a)) ++ b`, and a
pipe chain is parenthesized, `&(x |> f)`. It works in every position:

```ares
$ fib10 : @Int = &fib 10                            # expression: inserts `55`
$ n     : &"@Int64" = 1024                          # type
$ &"$ injected : @Int = 99"                         # item
$ is55  : @Int -> @Bool = \ | &fib 10 => @true | _ => @false   # pattern
```

A type splice may stand at any type position: a signature, a struct field, a
union payload, an alias body, an effect operation, an effect-row argument, a
`@use` field, or an annotation inside a body (`let x : T`, `\(x : T)`,
`(e : T)`). Effect-row entries themselves can be spliced too, as `<&X>`; a
`&` right after an operator lexes on its own, so `<&X>` is `<` then `&X`.

```ares
$ Point : @struct = x: &pick {}, y: &pick {}
```

In a pattern, `&X` is how a value the compiler can compute stands as a
constant: `&answer` matches the value of the global `answer`, while a bare
`answer` would bind. Because the value is inserted as a real literal,
exhaustiveness and `UNREACHABLE_ARM` treat it like a hand-written `42`. The
operand ends before the `=>`, `|` or `::` after it. A generator may also build
a pattern as `@Code`; that pattern may bind names, and its coverage is checked
once it is spliced.

A generator never runs code the checker could not yet resolve. While `&` sites
remain, a missing name or instance is left open, since a generator may still
supply it; a `&` whose code reaches one waits for a later round, and if no
round supplies it the error is reported as written.

`X` may perform the compile-time operations (`@emit`, `@abort`, `@fresh`,
`@link`, `@link_path`, and the reflection `@type_kind`, `@type_params`,
`@type_fields`, `@type_variants`, `@type_of`). They carry the `<@Meta>` effect,
which `&` discharges. The richer ones speak CORE types:

- `@lookup name` is a `Decl`: `Struct`, `Union` (each with its shape), `Value`
  (a global's type as source) or `Missing`.
- `@here {}` is the running `&`'s `Site` (`file`, `line`, `column`).
- `@check code` type-checks an expression fragment as if written at the `&`
  and gives its type as source; an ill-typed fragment fails the build.
- `@fail d` and `@warn d` report a `Diag` (`message`, `notes`, `helps`; `diag
  msg` has none) at the `&`: `@fail` as `META_ABORT`, ending the build, `@warn`
  as a `META_WARNING` warning.

`@Code` is source text with no syntax category of its own: the site that
splices it decides what it must parse as (an expression, a type, the entries of
a row, a pattern, fields, variants or items) and checks it there. An effect-row
splice `<&X>` stands for one or more entries.

`&` discharges `<@Meta>` and nothing else, so it is **hermetic**: a generator
cannot perform `@IO`. Compile-time IO lives in `@build` alone (section 11.4).

Empty code inserts nothing. A site that runs only for its effect returns it, so
the directive disappears once it has run.

A syntax error in a fragment points into the string literal that spells it,
escapes accounted for, even when that literal is in a helper the `&` calls. A
fragment built at compile time (`"a" ++ b`, or a quotation with holes) has no
such literal, so the error points at the `&` and quotes the fragment.

## 11.2 Quotations
A string literal expected as `@Code` is a **quotation**: its text is source. It
is the `@Code` case of the type-directed literal (section 1.6), resolved through
CORE's `@IStrLit @Code` and `@IInterp @Code @Code` instances, so no call builds
it. Its holes run in the generator, and what a hole pastes depends on its type:

| Hole type | Pasted |
|---|---|
| `@Code` | its source, as is |
| a number, `@Bool`, `@Str` | the literal spelling its value: `3`, `@true`, `"hi"` |

```ares
$ times : @Int -> @Code -> @Code = \n c => "(?(c)) * ?(n)"
$ sum   : @Code = "1 + 2 * 3"
$ x     : @Int = &times 3 sum                   # inserts `(1 + 2 * 3) * 3`
$ greet : @Str -> @Code = \s => "?(s) ++ \"!\""
$ hi    : @Str = &greet "hi"                    # inserts `"hi" ++ "!"`
```

A hole pastes code bare, so the quotation decides precedence: write `(?(c))`
where it matters. A `@Str` hole is always a string literal; to paste a computed
name or other text as code, convert it with `@code : @Str -> @Code`, as in
`"$ ?(@code name) : @Int = 1"`. `@code` is also how a generator turns text it
built (`join ", " fields`) into code. Neither checks syntax: the splice site
does. `@parse_str` remains for code that only exists at run time; it checks the
text as an expression on the spot.

A quotation is **hygienic**. A name it spells means what it means where the
quotation is written: a generator's `scale` is its own module's `scale`, even
where the code lands in a module with a `scale` of its own. A binder it spells
binds only names the same quotation spells, so a generator's `let tmp` never
captures the caller's `tmp`. Code the site hands in keeps the site's meaning: a
quotation written in the `&` operand itself, and `@code` text.

```ares
$ plus_one : @Code -> @Code = \c => "let tmp = 1 in tmp + ?(c)"
$ add_one  : @Int -> @Int = \tmp => &plus_one "tmp"      # tmp + 1, the caller's tmp
$ some_of  : @Code -> @Code = \v => "Option.Some.{?(v)}"
$ or_zero  : Option @Int -> @Int = \ | &some_of "x" => x | _ => 0
```

So a generated pattern binds a name for the arm body only when the site names
it, as `&some_of "x"` does. The splice site renames a generator's binders apart
(`tmp__6`) and qualifies the names it reaches (`GEN.scale`), which `ares expand`
shows; operators and effect operations resolve at the site.

A string literal expected as CORE's `Template` is a **macro argument**:

```ares
$ Template : @struct = pieces: @Vec @Str, holes: @Vec @Code
```

Its pieces stay text and its holes are **quoted**, not evaluated, since they
name what is in scope where the literal is written, which at a `&` is often a
run-time parameter. `@check` at that `&` sees the locals in scope there, so a
macro learns a hole's type:

```ares
$ sql : Template -> <@Meta> @Code = ...   # numbers the holes `$1`, `$2`, ...
$ by_name : @Str -> @Int -> Query = \name age =>
    &sql "SELECT id FROM users WHERE name = ?(name) AND age > ?(age)"
```

`sql` receives `pieces = ["SELECT id FROM users WHERE name = ", " AND age > ", ""]`
and the code `name` and `age`, whose types `@check` reports as `@Str` and
`@Int`. A hole is pasted back as the site's code, so it keeps the site's
meaning. A literal with no holes is a `Template` of one piece.
`examples/META_TEMPLATE.ares` is the whole macro.

## 11.3 Compile-time assertions and build directives
There is no `@assert` builtin. `CORE.assert : @Bool -> @Str -> <@Meta> @Code` is
one line of ordinary Ares over `@abort` that returns empty code, so a module
states its own invariants and checking it is testing it.

```ares
$ &assert (fib 10 == 55) "fib 10 should be 55"
$ &assert (fact 5 == 120) "fact 5 should be 120"
```

A failing assertion, like any `@abort`, is an `error[META_ABORT]` carrying the
message, pointed at the `&` site. A fault the code runs into on its own (a
division by zero, a failed match) stays a `RUNTIME_FAULT`.

`@link` / `@link_path` steer the compilation itself: they add a library to the
link line and the dlopen set, or a directory to the search path. Both return
empty code.

```ares
$ &@link "m"             # link/preload libm
$ &@link_path "/opt/lib"
```

## 11.4 `@build`, the compile-time entry
`@build` is to compiling what `@main` is to running: the one name the *build*
starts from. The compiler runs it during compilation and adds the items of the
`@Code` it returns to the module that declared it. Like the entry, it has exactly
one accepted signature.

```ares
$ @build : {} -> <@Meta, @IO> @Code = \_ =>
	"$ answer : @Int = 42"

$ @main : @Vec @Str -> <@IO> @Int = \args => answer      # 42
```

The `<@IO>` in that row is its whole privilege: `@build` is the one context where
a generator may read the world. The IO itself is not special, and there is no
compile-time IO primitive: it is `library/IO.ares` over the `C` namespace, the
same code a program runs, performed by the compile-time interpreter. So a
compile-time path is relative to the **working directory**, exactly like a path a
running program opens. `IO.read_dir` (a directory's entry names, sorted) is the
listing a `@build` usually wants.

Rules: every module may have one (not just the root, unlike `@main`), it runs
once, and it runs before any `$ &` of that round, so code it generates is
available to the rest of the compile-time code. Anything it injects is itself
expanded, so a generated `$ &` still runs and a generated `$ @import M` loads M.
Note what "every module" implies: importing a module lets its `@build` perform IO
during *your* build, the same trust an imported `build.rs` carries.

`tests/MAIN.ares` is the worked example: it reads `examples/` at compile time and
generates its own imports and `@main`, so adding an example to the corpus is
adding a file. See `examples/META_BUILD.ares` and
documentation/metaprogramming.md.

To see what a metaprogram produced, `ares expand <file> [MODULE]` prints the
source as the checker finally saw it: every `&` spliced and every
`@build`'s items appended. That is the view to debug a generator from, since
nothing else shows the generated text.

## 11.5 `BUILD.ares`, the project file
A program's own modules are found beside it (section 2). A project that keeps
them elsewhere says so in a `BUILD.ares`: the module `BUILD`, whose `@build` calls
`@import_path` once per directory to add to the module search, and `@option`
once per project option it sets.

```ares
$ @mod BUILD

$ @build : {} -> <@Meta, @IO> @Code = \_ =>
    @import_path "source";
    @option "format.width" "120";
    @option "lints.unreachable_arm" "deny";
    ""
```

`@option "section.name" "value"` takes the value as text, read in the option's
kind; an unknown option or a value it does not accept is a `BUILD_SCRIPT` error.
An option not set keeps its default:

| Option | Default | Accepts | Meaning |
|---|---|---|---|
| `format.width` | `100` | 40 to 400 | the line width `ares fmt` fits code into |
| `format.indent` | `4` | 1 to 16 | spaces per indentation level |
| `check.max_errors` | `20` | 1 to 10000 | the most errors `ares check` prints |
| `lsp.check` | `type` | `type`, `save` | check as you type, or only when a file is saved |
| `lsp.focus` | `roots` | `all`, `roots`, `one` | how loudly errors are shown (see `documentation/lsp.md`) |
| `lsp.debounce_ms` | `150` | 0 to 5000 | how long typing must pause before the file is checked |
| `lsp.grace` | `true` | `true`, `false` | hold back errors in the definition being typed |
| `lsp.grace_ms` | `1500` | 0 to 60000 | the pause after which held-back errors are shown |
| `lsp.max_diagnostics` | `50` | 1 to 10000 | the most diagnostics shown per file |
| `lints.unreachable_arm` | `warn` | `allow`, `warn`, `deny` | a match arm no value can reach |

An editor's settings override the `lsp` options for that editor. When
`BUILD.ares` fails, `ares fmt` and the language server report it and carry on
with the defaults; a compile stops.

The compiler looks for `BUILD.ares` in the root file's directory and then in each
directory above it, and runs the nearest one before it loads the program. Its
directories are searched for every root at or below it, after `--import-dir`
and before the program's neighbourhood. A path is relative to `BUILD.ares`.

Rules, each a `BUILD_SCRIPT` error:

- it declares `$ @mod BUILD` and its `@build` returns empty code;
- `@import_path` works only there, in any other module it faults;
- nothing imports `BUILD`.

`BUILD.ares` runs before its own directories are searched, so its imports see only
the standard library, its neighbours and `--import-dir`. `applications/alec` is
the worked example: `MAIN.ares`, `TEST.ares` and `BENCH.ares` sit at the top and
every module they import is in `source/`.

---

# 12. Intrinsic types and primitives

## 12.1 `@Bool` and its literals
The built-in boolean; values are `@true` and `@false` (matched with the
`@`-spelling, since bare `true`/`false` would bind a variable). Comparisons yield
`@Bool`; `!` negates it.

```ares
$ b : @Bool = 3 < 4
$ chk : @Int = b |> \ | @true => 0 | _ => 1
```

## 12.2 `@Array` (byte block)
Allocated with `@Array.{ n }` (n zeroed bytes). Primitives: `@array_len`,
`@array_get`, `@array_set`, `@array_push`, `@array_slice`, `@array_alloc`. `Str`
is a byte array, so these apply to strings too. An `@Array` also joins the `[..]`
literal, pattern, and slice surfaces through its `CORE` instances of the blessed
interfaces, like any other sequence.

Simple:
```ares
$ buf : @Array = @Array.{ 64 }
$ n : @Int = @array_len "hello"          # 5
```

Involved (build a string byte by byte, as CORE's integer stringifier does):
```ares
$ digit : @Int -> Str = \d => @array_push "" (48 + d)   # push a byte onto a Str
```

## 12.3 `@Vec` (growable vector)
A growable typed vector, wrapped by the `VEC` library over `@vec_new`,
`@vec_push`, `@vec_get`, `@vec_set`, `@vec_len`, `@vec_fill`.

```ares
$ @import VEC
$ v : @Vec @Int = push (push (new {}) 1) 2   # VEC over @vec_* primitives
```

## 12.4 `to_string` (CORE) and interpolation
`CORE` (implicitly imported) provides `to_string` for `Str`, `@Bool`, and `@Int`,
and the `Stream`/`range`/`count_from` prelude. String interpolation desugars
through `to_string`; extend it (7.3) for your own types.

```ares
$ s : Str = to_string 42 ++ " " ++ to_string @true      # "42 true"
```

## 12.5 `TARGET` reflection
The `TARGET` module (qualified, no import) reflects the compilation target: word
size, @Int bounds, and os/arch names, fixed at compile time.

```ares
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
| `@use` | instance (interface) parameter and explicit argument | 7.4 |
| `@ctx` | reserved for dependent types | 7.4 |
| `@absurd` | the empty match | 5.1 |
| `@private` | visibility | 2.4 |
| `&` `@abort` | compile-time splice | 11 |
| `@link` `@link_path` | build directives | 11.2 |
| `@import_path` | module search, in `BUILD.ares` | 11.4 |
| `@cast` | integer-width reinterpret | 4.11 |
| `@true` `@false` `@Bool` | boolean | 12.1 |
| `@Int8..64` `@Nat8..64` `@Float32/64` | sized numerics | 3.2 |
| `@I*` | blessed literal / indexing / range / interpolation interfaces | 1.4, 1.6, 9.5 |
| `@Ptr` `@Array` `@Vec` | built-in containers/pointer | 3.3, 12 |
| `@Tensor` | sized tensor type | 3.9 |
| `@array_len/get/set/push/slice/alloc` | array primitives | 12.2 |
| `@vec_new/push/get/set/len/fill` | vector primitives | 12.3 |
| `@tensor_index/length/create/concat/slice/...` | tensor primitives | 9.5 |

---

# 14. Compiler CLI

`ares [--target=ARCH-OS] <command> [file.ares] [args...]`. With no file the root
is `MAIN.ares` in the current directory (or the sole `.ares` file there).

| Command | Effect |
| --- | --- |
| `run` | run a program (needs `$ @main`) on the interpreter; extra args pass to it |
| `build` | compile a program to a native executable beside the source |
| `check` | run `@build`, expand metaprograms, run `$ &` checks, print inferred types |
| `expand` | print the source after expansion, to inspect what a metaprogram made |
| `emit-c` | emit standalone C to stdout |
| `parse` | print the parsed syntax tree |
| `lex` | print the token stream |
| `fmt` | format files in place; `fmt --check` only lists unformatted ones |
| `fix` | apply every fix the compiler is sure of, rechecking until none is left |
| `explain` | explain a diagnostic code (`ares explain TYPE_MISMATCH`) |
| `lsp` | run the language server on stdin/stdout (see `documentation/lsp.md`) |

Flags: `--target=ARCH-OS` cross-compiles (e.g. `x86_64-linux`, `wasm32-wasi`);
`-h` / `--help` prints usage.

```sh
ares run                       # run ./MAIN.ares
ares run app.ares a b           # run app.ares with args `a b`
ares check examples/FIB.ares    # a library module: type-check and run its checks
ares expand app.ares APP        # the source as the checker saw it, one module
ares build app.ares
ares --target=wasm32-wasi build app.ares
```

---

# 15. Standard library modules

Imported with `$ @import MOD` (except `CORE`, `C`, and `TARGET`, which need no
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

