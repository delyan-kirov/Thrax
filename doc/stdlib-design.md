# Standard library design: where it goes from here

The stdlib grew organically and it showed. This document is the deliberate
version: what the two reference libraries (Koka's std and Jai's distribution
modules) actually contain, what Thrax has as of the `standard-library`
branch, and the roadmap for the rest -- including the parts gated on
language features (tuples, generic mutable storage) and why.

## 1. What Koka ships (surveyed 2026-07, koka-lang/koka dev branch)

### std/core (auto-imported)

| module | surface |
|--------|---------|
| `types`, `bool`, `order` | primitive types; `order` is a proper three-way comparison type (`Lt`/`Eq`/`Gt`) with `int`/`(==)`/`(<)`... |
| `int` | word ops, `show`, parsing |
| `char` | `is-lower/upper/digit/hex-digit/alpha/alpha-num/ascii/control/white`, `cmp`, char arith |
| `string` | `cmp`, `(<)`.., `repeat`, `is-empty`, `pad-left/right`, `trim`, `split`, `splitn`, `join`, `lines`/`unlines`, `map` over chars |
| `sslice` | string SLICES: first/last/advance/extend -- substring views without copying |
| `list` | the big one; see function list below |
| `maybe` | `maybe` (fold), `default`, `unjust`, `expect`, `map`, `(\|\|)`, `flatten`, `bool` |
| `either` | left/right projections, `map`... |
| `result` | `maybe`, `default`, `either`, `map`, `flatten`, `(==)`, `show` |
| `exn` | the `exn` EFFECT: `throw`, `try : (() -> <exn\|e> a) -> e error<a>`, `catch`, `on-exit`; `alias error<a> = result<a,exception>` |
| `vector` | fixed-size arrays: `at`, `map`, `foreach`, list conversion |
| `console` | `print(ln)` overloaded on show-able types |
| `show`, `debug`, `delayed`, `lazy`, `undiv`, `unsafe` | formatting + escape hatches |

`std/core/list` functions (the checklist): `is-empty length range/list
stride/list (==) cmp show unzip(2/3/4) take drop map map-indexed map-peek
reverse reverse-append (++) foldr foldl foldl1 foldr1 replicate split span
take-while drop-while filter remove partition filter-map find find-maybe
lookup index-of foreach foreach-while foreach-indexed intersperse join
join-end concat flatmap concat-maybe last init [] all any sum minimum
maximum lines unlines zip zip-with zipwith-indexed unzip sort(?)`.

### The rest of std

- `std/num`: `float64` (the libm surface: sqrt, ln, exp, pow, sin..,
  floor/ceiling/round, isNaN/isFinite...), `int32`, `int64`, `ddouble`
  (128-bit), `decimal`, `random`.
- `std/os`: `path` (dirname/basename/extname/combine), `file` (read/write
  text), `dir` (list/create/copy), `env` (arguments, env vars), `flags`
  (getopt-style CLI parsing), `process` (run), `readline`.
- `std/text`: `parse` (parser COMBINATORS over sslices), `regex`,
  `unicode`.
- `std/time`: instants, durations, calendars, formatting (large).
- `std/data`: `dict`/`map`/`set`/`imap`/`iset` -- mostly "Todo" STUBS on
  dev. Koka v1 had the real prior art: immutable `dict<a>` (string keys)
  plus MUTABLE `mdict<h,a>` in the `st` effect, with `freeze` converting
  mdict -> dict. That pair (immutable value + mutable st-scoped twin +
  freeze) is the Koka answer to "Haskell map vs Rust map".

What makes Koka's API "nice" is less the function count than four
conventions, all adoptable without type classes:

1. Lookup returns `maybe` -- never a sentinel, never a trap.
2. Dot syntax: `m.get(k)`, `xs.map(f)` -- any function is a "method".
   (Thrax: qualified names + `|>` cover most of this today.)
3. Implicit parameters (`?(==)`, `?cmp`, `?hash`): dictionary passing where
   the COMPILER finds the dictionary by name at the call site. Thrax passes
   dictionaries explicitly (`sort_by cmp_int`); same semantics, one word
   noisier, no new machinery.
4. Effects in rows: fallibility (`exn`), divergence (`div`), state (`st`)
   are all visible on the arrow -- the caller composes, the boundary
   handles.

## 2. What Jai ships (distribution modules, from public writeups)

`Basic` (print, string builder, allocators, dynamic array ops, New/free),
`String` (search/compare/manipulate), `Hash_Table` (`Table(K,V)`,
`table_add`, `table_find`, `table_set`, `table_remove`, `table_contains`,
resizing open hash; `Hash` provides the hash functions), `Math` (scalars +
vectors/matrices), `Sort` (`quick_sort`, `bubble_sort`, compare_*),
`Random`, `File`/`File_Utilities` (read entire file, write, visit files),
`Time`?, `System`, `Process`, `Command_Line`, `Thread` (threads, mutexes,
thread groups), `Pool`/`Flat_Pool` (arena allocators), `Bucket_Array`,
`Bit_Array`, `Unicode`, `Sloppy_Math`, `Compiler`/`Preload`/
`Runtime_Support` (metaprogramming + runtime), `Curl`, `Mail`, windowing/
input/GL modules.

The Jai lesson is orthogonal to Koka's: the library is UNAPOLOGETICALLY
imperative and thin over the OS -- read the whole file into one buffer,
mutate the table in place, explicit allocators everywhere. Thrax's native
seam (the `C` namespace + libffi) is exactly the right vehicle for this
layer, and "wrap C, don't re-derive" is the rule for all OS-facing modules.

## 3. Where Thrax stands (this branch)

Done, tested in both engines (see doc/standard-library.md for the per-module
function tables): OPT, RESULT (+ `Fail` effect), LIST (Koka core set
incl. stable `sort_by`), STR (search/split/pad/replace/case/parse), MATH
(Int + Real, libm via `C`), MAP (immutable AVL tree map, ordered, three-way
cmp dictionary), RANDOM (MINSTD, value-threaded), IO (console/file/env/now).

Design decisions already made:

- **The immutable map is an ordered tree, not a hash table.** "A hashmap
  like Haskell" IS a balanced tree (`Data.Map`); with no generic mutable
  arrays in the language, association-list buckets made the old "hash map"
  O(n) with extra steps. The AVL version gets O(log n), ordered iteration,
  and structural sharing (persistence is cheap: path copying). The cmp
  dictionary is ONE function instead of hash+eq, and `MATH.cmp_int` /
  `STR.cmp_str` are the standard instances.
- **Errors are an effect first, a value second.** `Fail.fail` + row
  propagation subsumes Rust's `?` (the row IS the `?`, applied
  automatically); `try`/`try_or` reify at boundaries, `untry`/`expect`
  re-raise. `Result` remains for STORING outcomes (collections of results,
  APIs that want values).

## 4. The mutable map (Rust/Jai flavor): gated, and on what

A real hash table needs O(1) random-access mutable storage for buckets.
Thrax now HAS the gating piece: generic `Vec \`T` (route 1 below) landed --
see section 6 item 1 for what shipped. The table itself is the next step.
The original analysis, for the record:

1. **Generic vectors first** (`Vec \`T`: `vec_new/len/get/set/push` with the
   same rc==1 in-place trick the byte vectors use). Then `Table \`K \`V` is
   ~150 lines of stdlib: open addressing, MINSTD-style probe, resize at 0.75
   load, Jai's API surface (`set`, `find` -> Option, `remove`, `for` via
   `fold`). This is the principled path: one language feature unlocks the
   whole imperative-collections family (Vec, Table, Bit_Array, ...).
2. **Koka v1's mdict pattern, today**: a `State`-style effect wrapping the
   AVL map (`Table.get/set/remove` as effect ops; the handler threads the
   map; `freeze` returns it). Same API feel, O(log n) not O(1), zero new
   machinery -- a fine stopgap if the need arises before Vec lands.
3. C-backed handle table (Jai literally): a runtime-owned table addressed by
   an Int handle over FFI. Rejected for polymorphic values: storing Thrax
   values in C memory fights the refcounter's ownership; only viable for
   word/Str payloads, which is too narrow to be THE mutable map.

## 5. Tuples `{Int, Real}`, and `Map {Int, Str}` (DONE)

The language feature landed (2026-07-20, branch standard-library-part-2;
examples/TUPLES.thx):

- Type former `{A, B, ...}` of ANY arity: `{}` is the empty tuple (it IS
  the unit type, kept word-represented), `{A}` a real one-element tuple,
  no upper bound (TC registers each arity's synthetic struct def on first
  use, `ensure_tuple`). Literals `{1, 2.5}`, positional access `.0`/`.1`
  (chains split: `t.0.1` arrives from the lexer as one Real token and EX
  splits it, Rust-style), and `{p, q}` patterns in `when` arms, `let`
  binders and lambda parameters (`{}` is not a pattern; unit matches `_`).
- Implementation shape: a tuple is an ANONYMOUS STRUCT. EX emits the
  reserved per-arity names `%tupleN` (OP::tuple_name) with positional
  fields "0".."n-1"; TC registers the synthetic struct def lazily wherever
  a tuple name enters the type world (signature, literal, pattern), so
  literals, field sites, patterns, PatLower and unification all ride the
  existing nominal-struct machinery -- ZERO engine/runtime changes, and MR
  passes the `%`-names through untouched (they cannot clash: user type
  names must start uppercase). Diagnostics render `{@int64, @str}`, never
  `%tupleN`.
- One grammar subtlety: after `Tag:` in a union declaration, `{` always
  opens the PAYLOAD's field braces, never a bare tuple type (a single
  tuple-typed field is `Tag: {{A, B}}` or `Tag: {t: {A, B}}`); doc/thrax.y
  resolves this structurally (payload_decl / type_nb).
- `Map {Int, Str}` works today as a type argument; keying a map by a tuple
  just needs a lexicographic comparison (dictionary passing, see
  STDLIB tests / section 4 of doc/standard-library.md).

The stdlib half is DONE too: PAIR is REMOVED (not sugar -- deleted).
LIST's `zip`/`unzip`/`span`/`split_at`/`partition`/`lookup`, MAP's
`from_list`/`to_list`/`min_entry`/`max_entry` and RANDOM's `next` family
all speak `{\`A, \`B}` with `.0`/`.1` access. `MAP.cmp_pair` (lexicographic
composition of element orderings) ships the tuple-keyed-map convenience:
`new (cmp_pair cmp_int cmp_str)` orders a `Map {Int, Str} \`V`.

## 6. Target module tree and phases

Phase 1 (DONE): the core above. Follow-up landed with it: `SET` (the AVL
tree keyed by the elements), `PATH` (POSIX paths, pure Str), and the TC
overload fix that removed the annotated-let workaround (user sites now
join the resolution fixpoint conservatively; see doc/standard-library.md,
"A note on inference").

### Agreed sequencing for the rest (decided 2026-07-20)

1. **Generic `Vec \`T`**: DONE. `Vec` is an opaque core type (TC knows the
   name and arity 1; behavior lives in the reserved-name primitives
   `vec_new/fill/len/get/set/push`, seeded like the `array_*` family). The
   representation is the existing variant value tagged `"%vec"` whose
   payload fields are the elements, NO new value kind in either engine;
   the interpreter always copies (value semantics), the native runtime
   mutates a uniquely-owned (rc==1) spine in place for `set`/`push` like
   the byte vectors (no capacity field yet, so a non-unique push is O(n)
   pointer memcpy -- add capacity when it shows up). `library/VEC.thx` is
   the derived surface: checked `get` -> Option / `set` (out-of-range =
   unchanged), `from_list`/`to_list`, `map`/`fold`. Next: `TABLE` is ~150
   lines of stdlib over it: open addressing, resize at 0.75 load, Jai's
   surface (`set`, `find` -> Option, `remove`, iteration via `fold`).
2. **Tuples `{A, B}`** (section 5): DONE, out of order -- landed before
   Vec because the surface turned out small (anonymous structs, zero
   engine work). PAIR is gone; `zip`/`span`/`partition`/`Map.to_list`
   speak tuples; `MAP.cmp_pair` composes element orderings for tuple keys.
3. **Alternative pure-stdlib track** (when compiler work is unwelcome):
   the remaining OS layer below.

Phase 2 -- OS layer, pure C wrapping, no language work:
- `PATH` (DONE), `SET` (DONE).
- `DIR`: list/create/remove (opendir/readdir/mkdir/rmdir externs; readdir
  returns a struct -- needs either a small runtime shim or the dirent
  layout per platform, prefer a shim in THxRT).
- `PROCESS`: `run : Str -> Int` (system), later popen for captured output.
- `ARGS`/`FLAGS`: argv access (needs DR to expose argv; today only `env`),
  then Koka-flags-style parsing in pure Thrax.
- `TIME`: now (done), `clock`, formatting via localtime/strftime (shim for
  the struct tm layout).

Phase 3 -- language-gated (ordering above):
- Generic `Vec \`T`: DONE -> `TABLE` (mutable hash map, section 4),
  `BIT_ARRAY`.
- Tuples (section 5): DONE (PAIR removed); `MAP.cmp_pair` shipped.
- C `int` return marshalling (kills `IO.is_byte`), argv, buffered readers.

Phase 4 -- text:
- `PARSE`: parser combinators over `Str` + offset (no slices needed to
  start); this is also the dogfood test for the effect system (a `Parse`
  effect with backtracking handler).
- `REGEX` (wrap a C engine or defer), `UNICODE` (decode/encode UTF-8 over
  byte vectors).

Non-goals for now: Koka's time-scale/astronomy tower, DOM bindings, Jai's
windowing/GL layer, threads (no runtime story yet), decimal/ddouble.

## 7. core.thx: the language-defined layer (DONE)

Historical note: `if` took Int truth because the language predates its type
system; predicates returned 1/0 to match. Both halves of the plan landed
(2026-07-20, branch standard-library-part-2):

- **One real file**: `core/PRELUDE.thx`, baked into the stdlib amalgam and
  auto-injected with BARE names (the prelude exemption). It holds what the
  language DEFINES in itself: `Bool` (with `true`/`false`), `List`, and
  `assert`. The file is documentation: the `@` sigil marks what is truly
  built-in; everything else is readable Thrax you can open. Diagnostics
  point into it like any stdlib file.
- **The target-dependent lines stay generated**: DR still emits the other
  half of the PRELUDE module (`PRELUDE_implTarget.thx`) -- `Int`/`Nat`
  alias the TARGET's word type, and the fixed aliases mirror
  OP::base_aliases. A future `@word`/`@uword` canonical spelling would let
  the whole prelude go static.
- **Bool is blessed and erased**: `Bool : @union = True: {}, False: {},`
  lives in core; the compiler makes comparisons/`?=`/`!` return it and
  `if`/guards/`@assert` consume it (OP::TY_BOOL; overload RESULT types
  flipped in TCxDATA while the mono impl keys stay operand-derived, so the
  engines' 1/0 impls serve unchanged). It is ERASED after checking:
  CR rewrites `Bool.True`/`Bool.False` literals to Int 1/0, and PatLower
  rewrites Bool variant patterns to integer-literal patterns -- so at
  runtime and across the C FFI a Bool is a plain word, but user code can
  still construct and match it as an ordinary union (examples/AGTxSUM.thx
  does both). Stdlib predicates and predicate parameters are all
  `Bool`-typed now; IO's write/append/remove return `Bool` success
  (true = ok) instead of C-style 0/1 status codes.

## 8. Extensible `[..]` (planned)

`[a, b, c]` literals and `[..]` patterns are type-directed today between
exactly two hardwired types: the prelude `List` and the byte-vector
`Array`. To let user types opt in, desugar literals to an overloaded
builder protocol instead of special-casing:

    [a, b, c]  ==>  seq_done (seq_push (seq_push (seq_push seq_empty a) b) c)

Three overloadable functions -- `seq_empty`, `seq_push`, `seq_done` --
resolved by the expected type like any overload (the conservative
user-site fixpoint already handles expected-type-directed picks). `done`
exists so List can build reversed and flip once (O(n)), while Array's
`done` is the identity. List's and Array's instances then live in core.thx
-- the "implement the `[]` syntax for List and Array in the core" idea --
and `Vec \`T` picks up literals for free the day it lands, as does any
user collection.

Two footnotes:
- PATTERNS stay hardwired to List/Array for now. `is [a, b, ..rest]` for a
  custom type needs an uncons VIEW (`seq_uncons : T -> Option {head,
  rest}`), i.e. view patterns -- a separate, later design.
- The desugaring turns one literal node into n applications; if literal
  performance ever matters the compiler can keep the current fused path as
  an optimization for the known core instances.

Sequencing note: the protocol pays off most alongside Vec (section 6), so
the natural order is Bool/core.thx, then Vec + `[]` protocol together,
then tuples.
