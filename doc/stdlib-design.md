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
function tables): OPT, PAIR, RESULT (+ `Fail` effect), LIST (Koka core set
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
Thrax today has: immutable everything, byte vectors (`Str`/`Array`) with
opportunistic in-place mutation at rc==1, and no generic `Array \`T`. Three
routes, in order of preference:

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

## 5. Tuples `{Int, Real}`, and `Map {Int, Str}`

Anonymous products already exist in one position: a union constructor's
payload is literally a brace tuple (`Cons: {\`T, List \`T}`), constructed
and matched as `.Cons.{a, b}`. The proposal is to promote that form to
first-class types:

- Type former: `{A, B, ...}` (n >= 2; `{}` stays unit, `{A}` is not a
  tuple).
- Literals `{1, 2.5}`, patterns `{a, b}` in `when`/`let`/params -- the
  syntax already parses inside constructor sugar, so the grammar cost is
  low; the work is TC (a structural product type) and both backends (layout
  = existing struct layout, anonymous).
- PAIR then becomes sugar (`Pair A B` an alias for `{A, B}`, `.fst`/`.snd`
  positional accessors `.0`/`.1` or prelude functions), and LIST's
  `zip`/`span`/`partition`/MAP's `to_list` all migrate to tuples.

`Map {Int, Str}` -- a tuple as a type ARGUMENT -- then falls out for free
under the existing curried application (`Map {Int, Str} V` is a map keyed by
pairs, needing only a `cmp` for tuples: lexicographic composition
`cmp_pair cmp_int cmp_str`, which the stdlib can ship generically). Reading
`Map {Int, Str}` as sugar for `Map Int Str` (tuple of type args) is NOT
recommended: it would make `{...}` mean two different things at the type
level, and curried `Map Int Str` is already fine.

## 6. Target module tree and phases

Phase 1 (DONE): the core above. Follow-up landed with it: `SET` (the AVL
tree keyed by the elements), `PATH` (POSIX paths, pure Str), and the TC
overload fix that removed the annotated-let workaround (user sites now
join the resolution fixpoint conservatively; see doc/standard-library.md,
"A note on inference").

### Agreed sequencing for the rest (decided 2026-07-20)

1. **NEXT: generic `Vec \`T`** -- the single highest-leverage item, because
   it is the only blocker for the mutable Rust/Jai-style `TABLE` (section
   4) and also unlocks `BIT_ARRAY`, buffered readers and efficient string
   builders. Not greenfield: `Str`/`Array` already prove the design
   (growable storage, opportunistic rc==1 in-place mutation, copy on
   shared); the work is generalizing that mechanism from bytes to boxed
   values in both engines. Then `TABLE` is ~150 lines of stdlib: open
   addressing, resize at 0.75 load, Jai's surface (`set`, `find` ->
   Option, `remove`, iteration via `fold`).
2. **Then tuples `{A, B}`** (section 5): bigger surface (parser, TC, CR,
   both backends), compounding payoff -- PAIR retires, `zip`/`span`/
   `partition`/`Map.to_list` get nicer, `Map {Int, Str}` (tuple keys)
   falls out with a generic lexicographic `cmp_pair`. After Vec, because
   Vec unblocks a module family while tuples improve ergonomics.
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
- Generic `Vec \`T` -> `TABLE` (mutable hash map, section 4), `BIT_ARRAY`.
- Tuples (section 5) -> PAIR retirement, tuple cmp/eq combinators.
- C `int` return marshalling (kills `IO.is_byte`), argv, buffered readers.

Phase 4 -- text:
- `PARSE`: parser combinators over `Str` + offset (no slices needed to
  start); this is also the dogfood test for the effect system (a `Parse`
  effect with backtracking handler).
- `REGEX` (wrap a C engine or defer), `UNICODE` (decode/encode UTF-8 over
  byte vectors).

Non-goals for now: Koka's time-scale/astronomy tower, DOM bindings, Jai's
windowing/GL layer, threads (no runtime story yet), decimal/ddouble.

## 7. core.thx: the language-defined layer (planned)

Historical note: `if` takes Int truth because the language predates its type
system; predicates return 1/0 to match. The plan is a visible CORE layer
that upgrades this without hiding anything:

- **One real file** (a `core/` module, baked and auto-injected like the
  stdlib but with BARE names -- today's prelude exemption). It holds what
  the language DEFINES in itself: `List`, the numeric aliases, `assert`,
  and eventually `Bool`, `String` as an alias of `@str`, ... Today this
  text lives as a C++ string in app/DR.cpp (`prelude_source`), invisible to
  users. As a file it is documentation: the `@` sigil marks what is truly
  built-in; everything else is readable Thrax defined somewhere you can
  open. Diagnostics already point into real files for the stdlib; core gets
  the same.
- **The target-dependent lines stay honest**: `Int`/`Nat` alias the
  TARGET's word type, which is why the prelude is generated. Either DR
  keeps generating just those two lines, or (cleaner) a `@word`/`@uword`
  canonical spelling folds in TG and core.thx becomes fully static text.
- **Bool rides this**: declare `Bool : @union = True: {}, False: {},` in
  core and BLESS it (the compiler already blesses one core type: `List`,
  which `[..]`/`::` are hardwired to). Blessing means comparisons return
  `Bool` and `if`/guards consume it. Representation can stay a machine
  word (a nullary-only union is just its tag), so the engines barely
  change; the churn is TC's overload_db result types plus flipping stdlib
  predicates, which the checker will point at exhaustively. Worth doing
  BEFORE Vec/Table so new modules stop baking in Int truth.

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
