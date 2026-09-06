# Thrax Language Reference

Every language feature, one entry each, with a simple example and a more
involved one. The examples use the language's real, current surface syntax
(branches are `if c => t else e`, matches are `is scrut | pat => e ... else d`).
Where a feature is only partly built, the entry says so.

Every code block below is self-contained and type-checks on its own (verified
with `thrax check`); a block that needs a library imports it (`$ with LA`, etc.).

Conventions used throughout: a global is `$ name : Type = expr`; a lambda is
`\x = e` (curried: `\a b = e`); a branch is `if c => t else e`; a match is
`is scrut | pat => e ... else d`. The comparison operators are `?=` (equal),
`?<` (less), `?>` (greater), `<=` (at most), `>=` (at least), each of type
`a -> a -> @bool`.

---

## Table of contents

1. Lexical structure
2. Program structure (modules, globals, imports, entry point)
3. Types
4. Expressions and syntactic sugar
5. Pattern matching
6. Algebraic data types (structs, unions, codata)
7. Functions (currying, higher-order, overloading, implicits, TCO)
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
$ x : @int = 1   # trailing comment
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

## 1.4 String literals and escapes
A string is a block of bytes; source text must be well-formed UTF-8. Escapes:
`\n \t \r \0 \\ \" \' \a \b \f \v`, `\xHH` (one raw byte), `\u{...}` (a Unicode
scalar, UTF-8 encoded).

```thrax
$ hi   : Str = "hello\n"
$ raw  : Str = "\x41\x42"            # "AB" via hex bytes
$ emoji: Str = "\u{1F600} café"      # code point + raw UTF-8
```

## 1.5 String interpolation
`"... {expr} ..."` splices an expression, stringified through the overloaded
`to_string`. Surface sugar for `chunk ++ to_string expr ++ chunk`. `\{` and `\}`
are literal braces.

Simple:
```thrax
$ greet : Str -> Str = \who = "hello {who}!"
```

Involved (a user type interpolates by adding a `to_string` overload):
```thrax
$ Point : @struct = x: @int, y: @int
$ to_string : Point -> Str = \p = "({p.x}, {p.y})"
$ msg : Str = "at {Point.{ .x = 3, .y = 4 }}, n={40 + 2}"   # "at (3, 4), n=42"
```

---

# 2. Program structure

## 2.1 Module header `@mod`
Every file begins with `@mod NAME`. A module name is uppercase with digits, `_`,
and the inert lowercase `x` allowed (so `MAIN`, `LIST_UTIL`, `MODxA`). A module
may span several files (repeat the header).

```thrax
@mod MAIN
```

## 2.2 Global definitions `$`
`$ name : Type = expr`. The annotation is required unless inference gives the
body a ground, non-arrow type (a plain `@int`/`Str` constant). Functions and
anything polymorphic must be annotated. `$a` and `$ a` are the same.

Simple:
```thrax
$ answer = 42                 # inferred @int, no annotation needed
$ id : t -> t = \x = x        # annotation required (polymorphic)
```

Involved (definitions may appear in any order and reference each other):
```thrax
$ even : @int -> @bool = \n = if n ?= 0 => @true else odd (n - 1)
$ odd  : @int -> @bool = \n = if n ?= 0 => @false else even (n - 1)
```

## 2.3 Imports `$ with`
`$ with MOD` brings a module's public names into scope (bare or qualified).
Variants alias or import a single symbol. `Module.symbol` is always valid,
imported or not.

```thrax
$ with MATH                    # MATH.sqrt, or bare sqrt
$ with STR
$ with V = VEC                 # qualified only: V.push
$ with from_int = STR.from_int # one symbol, bound bare
```

## 2.4 Visibility `$ @private` / `$ @public`
Symbols are public by default. `$ @private` hides every symbol after it from
importers; `$ @public` toggles back. Resets at end of file.

```thrax
$ api : @int -> @int = \x = helper x + 1
$ @private
$ helper : @int -> @int = \x = x * x    # module-private
```

## 2.5 Entry point
The program entry is `main` of module `MAIN`, a C-style function returning an
`@int` exit code with an open effect row `<| e>` (so it may perform any effect).
A combined test file uses `test`. A bare value entry is forced and printed.

```thrax
$ main : {} -> <| e> @int = \u = 0                 # no args
$ main : [n]Str -> <| e> @int = \argv = 0          # argv[0] is the program path
$ test : @int = 0                                  # test-harness entry
$ main : @int = 6                                  # legacy value entry: prints "main = 6"
```

---

# 3. Types

## 3.1 Base numeric and string types
The word-size numbers are `@`-spelled like every other primitive: `@int`
(signed word), `@nat` (unsigned word). `Real` and `Str` keep friendly names.
They are distinct nullary constructors.

```thrax
$ n : @int = -5
$ u : @nat = 5
$ r : Real = 2.5
$ s : Str  = "hi"
```

## 3.2 Sized numerics
Fixed widths are `@`-spelled: `@int8/@int16/@int32/@int64`,
`@nat8/@nat16/@nat32/@nat64`, `@float32/@float64`. They are distinct from `@int`
etc. (`@int` and `@int32` do not unify). Arithmetic is overloaded per width; a
literal takes the width expected of it.

```thrax
$ w : @int32 = 1000
$ z : @int32 = w * 2          # @int32 + @int32 : @int32
```

## 3.3 Other `@`-typed built-ins
`@ptr` (raw pointer), `@bool` (values `@true`/`@false`), `@array` (byte block),
`@list` and `@vec` (prelude containers).

```thrax
$ flag : @bool  = @true
$ bytes: @array = @array.{ 8 }      # 8 zeroed bytes
$ xs   : @list @int = [1, 2, 3]
```

## 3.4 Unit `{}`
The empty record type is unit; its sole value is also `{}`. Disambiguated by
position (type vs term).

```thrax
$ nothing : {} = {}
$ ignore : @int -> {} = \x = {}
```

## 3.5 Function types and effect rows
`A -> B` is a function. A row between the arrow and result lists the effects a
call may perform: `A -> <E> B`. A bare arrow is pure (empty row). `<| e>` is an
open row (a row variable), which is what makes a function effect-polymorphic.

```thrax
$ State : @effect = get : {} -> @int, put : @int -> {},
$ pure : @int -> @int = \x = x                       # pure (empty row)
$ act  : {} -> <State> @int = \u = get {}           # may perform State
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
$ p : {@int, Str} = {42, "answer"}
$ n : @int = p.0
```

Involved (generic, nested, first-class):
```thrax
$ swap : {a, b} -> {b, a} = \t = {t.1, t.0}
$ nested : {@int, {@int, @int}} = {1, {2, 3}}
$ deep : @int = nested.1.0 + nested.1.1        # 5
```

## 3.8 Records and row polymorphism
A record type may leave its tail open with a row variable:
`{ x: @int, y: @int | r }` accepts any record (nominal struct or anonymous) with at
least those fields. Records are structural (unify by field name).

Simple:
```thrax
$ area : { x: @int, y: @int | r } -> @int = \p = p.x * p.y
```

Involved (update preserves the open tail; `with` stacks a field):
```thrax
$ shift : { x: @int | r } -> { x: @int | r } = \p = { .x = p.x + 10 | p }
$ tag   : { x: @int | r } -> { x: @int, t: @int | r } = \p = { .t = 99, with p }
```

## 3.9 Sized tensors
`[n]T` is a vector of `n` elements; `n` is a type-level natural (its own kind).
`[m, n]T` is sugar for nested `[m][n]T`. Sizes may be arithmetic
(`[n+m]a`), decided by a canonical form. See section 9.

```thrax
$ v : [3]@int = [10, 20, 30]
$ g : [2][2]@int = [ [1,2], [3,4] ]
```

## 3.10 Type aliases `@alias`
`$ Name : @alias params = Type` names a type. It may take parameters and
partially instantiate another generic type. An alias is transparent (unifies
with its expansion).

Simple:
```thrax
$ Count : @alias = @int
```

Involved (partial instantiation):
```thrax
$ Pair : @struct a b = fst: a, snd: b,
$ IntPair : @alias b = Pair @int b               # fix the first arg, keep the second open
$ ip : IntPair Str = .{ .fst = 3, .snd = "z" }  # same as Pair @int Str
```

---

# 4. Expressions and syntactic sugar

## 4.1 Lambdas
`\pat+ = e`. Multiple parameters are curried sugar. A parameter may be an
irrefutable pattern.

```thrax
$ Person : @struct = name: Str, age: @int,
$ inc : @int -> @int = \x = x + 1
$ add : @int -> @int -> @int = \a b = a + b            # curried
$ name : Person -> Str = \Person.{ name, _ } = name # destructuring parameter
```

## 4.2 Application
Juxtaposition, left-associative, binds tighter than any operator.

```thrax
$ inc : @int -> @int = \x = x + 1
$ add : @int -> @int -> @int = \a b = a + b
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
`?=` `?<` `?>` `<=` `>=`, all `a -> a -> @bool`. Use `!(a ?= b)` for inequality.

```thrax
$ lt : @bool = 3 ?< 8
$ le : @bool = 8 <= 8
$ ne : @bool = !(3 ?= 4)
```

## 4.5 Concatenation `++` and cons `::`
`++` joins strings/arrays/vectors of the same type; `::` conses onto a `@list`.

```thrax
$ s  : Str = "Hello" ++ " " ++ "world"
$ xs : @list @int = 1 :: 2 :: 3 :: []
```

## 4.6 Short-circuit `&&` / `||`
Desugar to a lazy `if`, so the right operand runs only when needed.

```thrax
$ safe : @int -> @bool = \n = n ?> 0 && 100 / n ?> 5
```

## 4.7 Conditional `if ... => ... else`
`if c => t else e`. The condition is a `@bool`. Chains with `else if`. It is lazy
(only the taken branch runs) and is not a pattern match.

Simple:
```thrax
$ abs : @int -> @int = \n = if n ?> 0 => n else 0 - n
```

Involved (else-if chain):
```thrax
$ sign : @int -> @int = \n =
	if n ?= 0 => 0
	else if n ?> 0 => 1
	else 0 - 1
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
$ len : @list @int -> @int =
	let go : @list @int -> @int -> @int = \l n =
		is l | _ :: t => go t (n + 1) else n
	 in \l = go l 0
```

## 4.9 Sequencing `;` and pipes `|>` `<|`
Parser sugar, lowest precedence. `a ; b` = `let _ = a in b`. `x |> f` = `f x`
(left-assoc). `f <| x` = `f x` (right-assoc).

```thrax
$ inc : @int -> @int = \x = x + 1
$ double : @int -> @int = \x = x * 2
$ step1 : {} -> @int = \u = 0
$ step2 : {} -> @int = \u = 0
$ eff = step1 {} ; step2 {} ; 0            # run for effect, return 0
$ y   = 5 |> inc |> double                 # double (inc 5)
$ z   = double <| inc <| 5                 # same
```

## 4.10 `@cast`
Reinterprets an integer at a different integer width. The target width comes
from context (annotate if unknown). Integer widths only, not int/real/ptr (those
use `C.i2f` / `C.i2p`).

```thrax
$ big   : @int64 = 300
$ small : @int32 = @cast big              # reinterpret across integer widths
$ back  : @int    = @cast small            # and back to the platform word
```

---

# 5. Pattern matching

## 5.1 The match form `is`
`is scrut | pat => e | pat => e ... else d`. Arms are tried top to bottom; the
first matching pattern wins and binds its variables. The leading `is`
distinguishes it from `if`.

Simple:
```thrax
$ classify : @int -> Str = \n =
	is n | 0 => "zero" | 1 => "one" else "many"
```

Involved (nested union patterns, exhaustive so no `else`, see 5.9):
```thrax
$ Wrap : @union = Empty: {}, W: { Opt }
$ Opt  : @union = None: {}, Some: { @int }
$ unwrap : Wrap -> @int = \w =
	is w | Wrap.Empty => 0
	     | Wrap.W.{ Opt.None } => 1
	     | Wrap.W.{ Opt.Some.{ n } } => n
```

## 5.2 Literal patterns
Match @int, Real, and Str by equality (refutable).

```thrax
$ describe : @int -> Str = \n = is n | 0 => "z" | 1 => "o" else "m"
$ yn : Str -> @int = \s = is s | "yes" => 1 | "no" => 0 else 99
```

## 5.3 Wildcard and variable patterns
`_` matches anything binding nothing; a lowercase name binds the value.

```thrax
$ tag : @int -> @int = \n = is n | 0 => 100 | m => m + 1 else 0
```

## 5.4 Struct patterns
`Type.{ ... }` matches a struct. Positional (bare fields, in order, all listed)
or named (dotted, any order, others ignored; a lone `.name` puns).

```thrax
$ Point  : @struct = x: @int, y: @int,
$ Person : @struct = name: Str, age: @int,
$ sum_xy : Point -> @int = \p = is p | Point.{ x, y } => x + y else 0
$ who    : Person -> Str = \p = is p | Person.{ .name } => name else "?"
```

## 5.5 Variant patterns
`Type.Tag` (unit) or `Type.Tag.{ payload }`. May be written bare (`.Tag`), with
the union inferred from the arms. Payloads nest.

```thrax
$ Maybe : @union t = Just: t, None: {}
$ get : @int -> Maybe @int -> @int = \d m = is m | Maybe.Just.{ x } => x else d
$ isJust : Maybe t -> @bool = \m = is m | .Just.{ _ } => @true else @false
```

## 5.6 Range patterns
`lo ... hi` matches the inclusive interval (bounds are literals of the
scrutinee's type). Open `lo ...` matches `lo <= x`. Refutable, binds nothing.

Simple:
```thrax
$ grade : @int -> Str = \n = is n | 90 ... 100 => "A" | 60 ... 89 => "C" else "F"
```

Involved (open range, Real):
```thrax
$ sign : @int -> Str = \n = is n | 0 ... => "nonneg" else "neg"
$ band : Real -> @int = \x = is x | 0.0 ... 0.5 => 1 | 0.5 ... 1.0 => 2 else 0
```

## 5.7 List and array patterns
List: `[]`, `h :: t`, fixed-arity `[a, b]`, open `[a, ..rest]`. The same `[..]`
brackets destructure a `@array` (type-directed); `::` stays list-only.

Simple:
```thrax
$ sum : @list @int -> @int = \xs = is xs | [] => 0 | h :: t => h + sum t else 0
```

Involved (leading cells plus a rest tail, on a list and on an array):
```thrax
$ second : @list @int -> @int = \xs = is xs | [_, x, ..rest] => x else 0 - 1
$ head_of : @array -> @int = \a = is a | [h, ..rest] => h else 0
```

## 5.8 Or-patterns and guards
`| p1 | p2 => e` shares one body (alternatives bind no variables). `| pat if g
=> e` adds a boolean guard; a failed pattern or guard falls through to the next
arm, even one with the same constructor.

Or-pattern:
```thrax
$ small : @int -> @int = \n = is n | 0 | 1 | 2 => 1 else 0
```

Guards falling through the same constructor:
```thrax
$ Box : @union = Some: @int, Nil: {},
$ grade : Box -> @int = \x =
	is x | Box.Some.{ v } if v ?> 100 => 3
	     | Box.Some.{ v } if v ?> 0   => 2
	     | Box.Some.{ _ }             => 1
	     | Box.Nil.{}                 => 0
	     else 0 - 1
```

## 5.9 Exhaustiveness
`else` is optional when the arms cover every constructor of a union; the checker
runs a usefulness algorithm reasoning recursively through nested payloads. A
non-exhaustive match with no `else` is a compile error naming the missing shape.

```thrax
$ Light : @union = Red: {}, Yellow: {}, Green: {}
$ go : Light -> @int = \l =
	is l | Light.Red => 0 | Light.Yellow => 1 | Light.Green => 2   # no else needed
```

## 5.10 Irrefutable patterns in `let` and lambda
Only `_`, variables, and struct/tuple patterns built from them (no literals) may
appear in a `let` binder or a lambda parameter; they desugar to field accesses.

```thrax
$ Point  : @struct = x: @int, y: @int,
$ Person : @struct = name: Str, age: @int,
$ Line   : @struct = from: Point, to: Point,
$ person : Person = Person.{ .name = "A", .age = 30 }
$ age : @int = let Person.{ .age = a } = person in a
$ start : Line -> @int = \Line.{ .from = Point.{ x, _ }, .to = _ } = x
```

---

# 6. Algebraic data types

## 6.1 Structs `@struct`
A named record type: `$ Name : @struct params = field: Type, ...`. Built with a
literal (`Type.{ .f = e }`, bare `.{ ... }`, or positional `Type.{ a, b }`);
read with `.field`. Nominal (unifies by name).

Simple:
```thrax
$ Person : @struct = name: Str, age: @int,
$ p : Person = Person.{ .name = "Will", .age = 21 }
$ who : Str = p.name
```

Involved (generic, applied by juxtaposition, nested field types):
```thrax
$ Box : @struct t = val: t,
$ Wrap : @struct t = inner: Box t,
$ w : Wrap @int = .{ .inner = Box.{ .val = 7 } }
$ deep : @int = w.inner.val
```

## 6.2 Record update
`Type.{ .field = e | base }` copies unlisted fields from `base`. The `| base`
comes last; a listed field may read from `base`.

```thrax
$ Person : @struct = name: Str, age: @int,
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
$ some : Maybe @int = Maybe.Just.{ 5 }
$ none : Maybe @int = .None
```

Involved (recursive, multi-field payload, positional or named):
```thrax
$ Tree : @union a = Leaf: a, Node: { Tree a, Tree a }
$ t : Tree @int = Tree.Node.{ Tree.Leaf.{ 1 }, Tree.Leaf.{ 2 } }
```

## 6.4 Type splice `with`
A `@struct`/`@union` may start with `with Other` to copy that type's fields or
variants ahead of its own. A copy-paste convenience with no subtyping
relationship. Transitive; a duplicate member is an error.

```thrax
$ Point  : @struct = x: @int, y: @int
$ Point3 : @struct = with Point, z: @int          # x, y, then z
$ Base   : @union  = Red: {}, Green: {}
$ Color  : @union  = with Base, Blue: {}         # Red, Green, then Blue
```

## 6.5 Codata `@codata`
Coinductive types defined by their observations (dual to a struct's fields).
Built with `{ .obs = e, ... }`, consumed with `s.obs`. Observations are
non-memoized thunks, so an infinite structure is fine.

Simple (the prelude `Stream`, observed):
```thrax
$ first : Stream @int -> @int = \s = s.head
$ next  : Stream @int -> Stream @int = \s = s.tail
```

Involved (map over an infinite stream, still lazy):
```thrax
$ smap : (a -> b) -> Stream a -> Stream b = \f s =
	{ .head = f s.head, .tail = smap f s.tail }
$ tenth : @int = (smap (\x = x + x) (count_from 1)).tail.tail.head
```

---

# 7. Functions

## 7.1 Currying
Every function of several parameters is curried; partial application yields a
function.

```thrax
$ add : @int -> @int -> @int = \a b = a + b
$ add5 : @int -> @int = add 5           # partial application
```

## 7.2 Higher-order functions
Functions are first-class values.

```thrax
$ apply_twice : (t -> t) -> t -> t = \f x = f (f x)
$ compose : (b -> c) -> (a -> b) -> a -> c = \f g x = f (g x)
```

## 7.3 Function overloading
Several definitions may share a name; a use resolves by argument type. A local
definition extends an imported one into a merged overload set. This is how
`to_string` (in `CORE`) is extended for new types.

Simple:
```thrax
$ show : @int -> Str = \n = to_string n
$ show : @bool -> Str = \b = if b => "yes" else "no"
```

Involved (extend the imported `to_string` for a user type; interpolation then
picks it up):
```thrax
$ Point : @struct = x: @int, y: @int
$ to_string : Point -> Str = \p = "({p.x}, {p.y})"
$ line : Str = "p = {Point.{ .x = 1, .y = 2 }}"
```

Note: OPERATOR overloading (`$ @operator.{ + } : ...`) parses but is currently a
no-op (the definition is dropped after parsing, so a `+` on a user type does not
resolve). Function overloading on ordinary names does work.

## 7.4 Implicit parameters `@ctx`
After a signature, `@ctx name : Type` declares an implicit parameter resolved by
name from the surrounding scope (a local binder wins, else a global picked by
type), or passed explicitly with `@ctx e`. Elaborates to dictionary passing.

Simple:
```thrax
$ Ordering : @union = LT: {}, EQ: {}, GT: {}
$ max_of : a -> a -> a  @ctx compare : a -> a -> Ordering = \x y =
	is compare x y | Ordering.GT => x else y
```

Involved (chaining passes the implicit down; explicit override at the call):
```thrax
$ Ordering : @union = LT: {}, EQ: {}, GT: {}
$ compare : @int -> @int -> Ordering = \a b =
	if a ?< b => Ordering.LT else if a ?> b => Ordering.GT else Ordering.EQ
$ flip : @int -> @int -> Ordering = \a b = compare b a
$ max_of : a -> a -> a  @ctx compare : a -> a -> Ordering = \x y =
	is compare x y | Ordering.GT => x else y
$ max3 : a -> a -> a -> a  @ctx compare : a -> a -> Ordering = \x y z =
	max_of (max_of x y) z
$ as_min : @int = max_of 3 7 @ctx flip       # flip reverses the order
```

## 7.5 Tail-call optimization
Tail-recursive calls (self, mutual, or through a recursive local `let`) run in
constant stack.

```thrax
$ sum_to : @int -> @int -> @int = \n acc =
	if n ?= 0 => acc else sum_to (n - 1) (acc + n)      # constant stack at any depth
```

---

# 8. Algebraic effects and handlers

## 8.1 Declaring an effect `@effect`
`$ Name : @effect = op : Arg -> Resume, ...`. Each operation takes one argument
and names its resume type. An operation whose handler never resumes (an
exception) names a type variable as its result.

```thrax
$ State : @effect = get : {} -> @int, put : @int -> {},
$ Exn   : @effect = throw : Str -> a,
$ Yield : @effect = yield : @int -> {},
```

## 8.2 Performing an operation
Just call it; there is no `perform` keyword. A use injects the effect into the
ambient row.

```thrax
$ State : @effect = get : {} -> @int, put : @int -> {},
$ tick : {} -> <State> @int = \u = let x = get {} in let _ = put (x + 1) in x
```

## 8.3 Handling `do ... ctl`
`do body ctl k | op a => e ... else x => e`. `k` is the captured continuation,
shared by every clause. Each `| op a => e` handles an operation; `else x => e`
runs on normal completion (defaults to identity). A shared operation name is
qualified in a clause head.

Exception (ignores `k`, so it resumes zero times):
```thrax
$ safeDiv : @int -> @int -> @int = \a b =
	do if b ?= 0 => Exn.throw "div0" else a / b
	ctl k | Exn.throw msg => 0 - 1
```

Generator (resume once per yield, summing results):
```thrax
$ sumGen : ({} -> <Yield> {}) -> @int = \gen =
	do gen {}
	ctl k | Yield.yield v => v + k {}
	      else _ => 0
```

## 8.4 Resuming
Resuming is applying `k`. `k v` splices the suspended computation back and
delivers `v`. `k` is affine (at most once; twice is a runtime error) and may be
stored before use, which is what coroutines need. Handlers are deep (a resumed
computation is still governed by the same handler).

```thrax
$ Task : @union = Fin: {}, Susp: { @int, {} -> Task }
$ spawn : ({} -> <Yield> {}) -> Task = \t =
	do t {}
	ctl k | Yield.yield v => Task.Susp.{ v, k }     # store k, resume later
	      else _ => Task.Fin.{}
```

## 8.5 Effect rows in types
A function's type carries the effects it may perform as a row on the arrow.
Absent row means pure. A row variable makes it effect-polymorphic. A handler
discharges the effects it handles. A performed-but-unhandled effect is a compile
error. Subsumption: a pure function is callable in any effectful context.

```thrax
$ map : (a -> <e> b) -> @list a -> <e> @list b = \f xs =
	is xs | [] => [] | h :: t => f h :: map f t else []
```

## 8.6 `defer`
`defer <cleanup> do <body>` runs the cleanup when the body's scope exits, on
normal completion, on an abort by an outer non-resuming handler, or when a stored
continuation holding it completes. Nested defers run innermost-first. The
resource-safe FFI idiom.

```thrax
$ open  : Str -> @int = \path = 0         # stand-ins for real handles
$ close : @int -> @int = \h = h
$ read  : @int -> @int = \h = h
$ useFile : Str -> @int = \path =
	let f = open path in
	defer close f do
		read f
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
$ v : [3]@int = [10, 20, 30]
$ a : @int = v.[1]        # 20
$ b : @int = v.[3]        # wraps to v.[0] = 10
```

## 9.2 Multi-dimensional and multi-axis indexing
`[m, n]T` is nested `[m][n]T`. `t.[i, j]` reads one element (`t.[i].[j]`).

```thrax
$ with LA
$ g : [2][2]@int = [ [1,2], [3,4] ]
$ z : @int = g.[1, 0]     # 3
```

## 9.3 Size arithmetic
A size may be `+`/`*` over literals and size variables, evaluated mod 2^64 and
compared by a canonical form (`[n+m]` equals `[m+n]`; `[n+n]` is `[2*n]`). This
drives size-changing operations, whose result size is computed at compile time.

```thrax
$ with LA
# LA provides `concat`, its result size computed at compile time:
#   concat : [n]a -> [m]a -> [n+m]a
$ joined : [5]@int = concat [1, 2] [3, 4, 5]      # [2+3] = [5]
```

## 9.4 Slicing and variance
Multi-axis slicing `t.[s0, s1, ...]`: `i` indexes an axis, `p ... q` narrows it
to an inclusive range, `..` keeps it whole. Every result is an O(1) strided view.
Each axis may carry a variance tag `@contra` (upper index / column) or `@co`
(lower index / row); `matmul` contracts a `@co` axis against a `@contra` axis.

```thrax
$ with LA
$ row : [m][n]a -> @int -> [n]a = \m i = m.[i]
$ col : [m][n]a -> @int -> [m]a = \m j = m.[.., j]
$ sub : [8]@int -> [4]@int = \m = m.[2 ... 5]      # inclusive 2..5, four elements
```

## 9.5 Tensor primitives and the `LA` library
The compiler provides only `@`-primitives over the buffer; every named operation
lives in `LA`. `t.[i]` desugars to an overloadable `index` function (so a custom
container joins the `.[..]` surface with its own overload).

```thrax
$ transpose : [m][n]a -> [n][m]a = \t =
	@tensor_create (@tensor_index t 0)
		(\j = @tensor_create t (\i = @tensor_index (@tensor_index t i) j))
```

## 9.6 Expression-form ranges
`[lo ... hi]` is a type-directed inclusive-range literal. Its target comes from
the expected type: a sized tensor (literal bounds fix `n`), a `List` (the
default), or, open, a `Stream`.

```thrax
$ tv : [4]@int     = [1 ... 4]     # tensor, n = 4
$ ns : @list @int  = [1 ... 5]     # list (default)
$ s  : Stream @int = [1 ...]       # infinite stream (open range)
```

---

# 10. Foreign function interface

## 10.1 `@extern` binding
A foreign binding is a global whose body is `@extern "C" "symbol" "lib"`. A
signature is required and drives C-ABI marshalling. An extern takes exactly one
argument (one arrow).

```thrax
$ puts : Str -> @int = @extern "C" "puts" "libc"
$ used : @int = puts "hi"
```

## 10.2 Multi-parameter C functions
A C function is not curried, so several parameters group into a record; its
fields, in declaration order, are the positional C arguments (marshalled by name,
so a reordered call site is fine). A nullary C function takes `{}`.

```thrax
$ pow : {base: Real, exp: Real} -> Real = @extern "C" "pow" "libm"
$ a : Real = pow {2.0, 10.0}                 # positional
$ b : Real = pow {.exp = 10.0, .base = 2.0}  # named, reordered
$ getchar : {} -> @int = @extern "C" "getchar" "libc"
```

## 10.3 C structs and unions by value
`Name : @struct @extern "C" = ...` is a C-layout foreign type (fields must be
C-representable, not generic). Built and read with the ordinary struct literal
and `.field`, and it crosses the boundary by value. A C union is
`Name : @union @extern "C" = ...` (members share offset 0).

```thrax
$ Color : @struct @extern "C" = r: @nat8, g: @nat8, b: @nat8, a: @nat8,
$ clear : Color -> {} = @extern "C" "ClearBackground" "libraylib.so"
$ go : {} = clear Color.{ .r = 0, .g = 0, .b = 0, .a = 255 }
```

## 10.4 Callbacks and array parameters
A function-typed parameter is a C function pointer: pass a Thrax closure. A
`@list T` parameter (T a C-repr struct) is passed as a packed `T*` with a separate
count.

```thrax
$ Vector2 : @struct @extern "C" = x: @float32, y: @float32,
$ sort : {@ptr, @int, (@int -> @int -> @int)} -> {} = @extern "C" "qsort_r" "libc"
$ draw : {@list Vector2, @int} -> {} = @extern "C" "DrawLineStrip" "lib"
```

## 10.5 The `C` namespace and engine intrinsics
`library/C.thx` binds libc/libm as the auto-injected `C` namespace (used
qualified, `C.sqrt`, no import). It also holds a few conversions the language
cannot express with `@cast` (which is integer widths only): `C.i2f`/`C.f2i`
(@int and @float32), `C.i2d`/`C.d2i` (@int and Real), `C.i2p` (@int to @ptr), and
`C.null` (the null pointer). These are provided by the engines' runtimes (an
empty `lib`); see `documentation/native-backend.md`.

```thrax
$ x : @float32 = C.i2f 41
$ nul : @ptr = C.null
```

Limitation: variadic C functions (`printf(fmt, ...)`) have no single binding.
Declare a fixed-arity binding per argument shape you use; there is no `...`
marker.

---

# 11. Compile-time evaluation

## 11.1 `@assert`
`$ @assert (expr)` evaluates a boolean at build time and fails the build if it is
false.

```thrax
$ @assert (fib 10 ?= 55)
$ @assert (fact 5 ?= 120)
```

## 11.2 `@run` and BUILD directives
`$ @run expr` forces an expression through the interpreter at build time (the
value is discarded). A `BUILD` directive value steers the compilation itself
(e.g. adds a library to the link line and the dlopen set).

```thrax
$ with BUILD
$ @run triple 14              # run for effect at build time
$ @run BUILD.lib "m"          # link/preload libm
```

---

# 12. Intrinsic types and primitives

## 12.1 `@bool` and its literals
The built-in boolean; values are `@true` and `@false` (matched with the
`@`-spelling, since bare `true`/`false` would bind a variable). Comparisons yield
`@bool`; `!` negates it.

```thrax
$ b : @bool = 3 ?< 4
$ chk : @int = is b | @true => 0 else 1
```

## 12.2 `@array` (byte block)
Allocated with `@array.{ n }` (n zeroed bytes). Primitives: `@array_len`,
`@array_get`, `@array_set`, `@array_push`, `@array_slice`, `@array_alloc`. `Str`
is a byte array, so these apply to strings too; `[..]` patterns destructure an
array.

Simple:
```thrax
$ buf : @array = @array.{ 64 }
$ n : @int = @array_len "hello"          # 5
```

Involved (build a string byte by byte, as CORE's integer stringifier does):
```thrax
$ digit : @int -> Str = \d = @array_push "" (48 + d)   # push a byte onto a Str
```

## 12.3 `@vec` (growable vector)
A growable typed vector, wrapped by the `VEC` library over `@vec_new`,
`@vec_push`, `@vec_get`, `@vec_set`, `@vec_len`, `@vec_fill`.

```thrax
$ with VEC
$ v = push (push (new {}) 1) 2          # VEC over @vec_* primitives
```

## 12.4 `to_string` (CORE) and interpolation
`CORE` (implicitly imported) provides `to_string` for `Str`, `@bool`, and `@int`,
and the `Stream`/`range`/`count_from` prelude. String interpolation desugars
through `to_string`; extend it (7.3) for your own types.

```thrax
$ s : Str = to_string 42 ++ " " ++ to_string @true      # "42 true"
```

## 12.5 `TARGET` reflection
The `TARGET` module (qualified, no import) reflects the compilation target: word
size, @int bounds, and os/arch names, fixed at compile time.

```thrax
$ wide : @bool = TARGET.int_bits ?= 64
$ name : Str = TARGET.arch ++ "-" ++ TARGET.os     # equals TARGET.name
```

---

# 13. The `@`-sigil catalogue

A quick index of the `@`-forms and where each is documented above.

| Form | Kind | Section |
| --- | --- | --- |
| `@mod` | module header | 2.1 |
| `@struct` `@union` `@codata` `@effect` `@alias` | type declarations | 6, 8.1, 3.10 |
| `@extern` | foreign binding | 10 |
| `@ctx` | implicit parameter | 7.4 |
| `@operator` | operator overload (parsed, not implemented) | 7.3 |
| `@private` `@public` | visibility | 2.4 |
| `@run` `@assert` | compile-time evaluation | 11 |
| `@cast` | integer-width reinterpret | 4.10 |
| `@true` `@false` `@bool` | boolean | 12.1 |
| `@int8..64` `@nat8..64` `@float32/64` | sized numerics | 3.2 |
| `@ptr` `@array` `@list` `@vec` | built-in containers/pointer | 3.3, 12 |
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
| `run` | run a program on the interpreter (extra args pass to it) |
| `build` | compile to a native executable beside the source |
| `check` | type-check only, print inferred types |
| `emit-c` | emit standalone C to stdout |
| `parse` | print the parsed syntax tree |
| `lex` | print the token stream |

Flags: `--target=ARCH-OS` cross-compiles (e.g. `x86_64-linux`, `wasm32-wasi`);
`-h` / `--help` prints usage.

```sh
thrax run                       # run ./MAIN.thx
thrax run app.thx a b           # run app.thx with args `a b`
thrax build examples/FIB.thx
thrax --target=wasm32-wasi build examples/FIB.thx
```

---

# 15. Standard library modules

Imported with `$ with MOD` (except `CORE`, `C`, and `TARGET`, which need no
import). A brief map:

| Module | Contents |
| --- | --- |
| `CORE` | `to_string`, `Stream`, `range`, `count_from` (implicitly imported) |
| `C` | libc/libm bindings and engine conversion intrinsics (qualified) |
| `MATH` | @int/Real helpers, libm wrappers, int/real conversions |
| `STR` | string operations (`from_int`, `substr`, `contains`, `to_lower`, ...) |
| `LIST` | list operations (`sum`, `length`, `map`, ...) |
| `VEC` | growable vectors over the `@vec` primitives |
| `MAP` `SET` | keyed containers |
| `OPT` `RESULT` | optional and result types |
| `PATH` | path manipulation |
| `RANDOM` | pseudo-random numbers |
| `IO` | console and file IO over the `C` namespace |
| `LA` | shape-checked linear algebra over sized tensors |
| `BUILD` | compile-time build directives (`@run BUILD.lib ...`) |
| `TARGET` | compilation-target reflection (qualified) |
