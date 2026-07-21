# The standard library

The standard library is a set of ordinary Thrax modules under `library/`,
nothing fancy: thin, readable code over the auto-injected `C` libc namespace
and the byte-vector primitives. It is deliberately separate from the prelude
(see doc/architecture.md): the prelude is the language's blessed core --
`Bool`, `List`, `assert` in the readable `core/PRELUDE.thx`, plus the
generated numeric aliases, while the standard library is a toolbox a
program opts into with `$ with MOD`.

## How it reaches a program

The build bakes `core/*.thx` and `library/*.thx` into the compiler binary
(`BLD::gen_stdlib_header` -> `artifacts/STDLIBxAMALG.hpp`); DR injects every
unit into every compile, right after the generated prelude fragment and `C`
(app/DR.cpp `parse_units`). So:

- the `thrax` binary is self-contained -- no install path, no environment
  variable, and the standard library is identically available to the
  interpreter, `--emit-c` and `--build`;
- diagnostics point at the real `library/NAME.thx` file;
- a program pays only for what it reaches: MR's dead-global elimination strips
  every unreached stdlib global, so `hello world` does not carry the hash map;
- editing a `library/*.thx` and rerunning `./build` regenerates the header and
  relinks (the generated header is a tracked amalgam input via
  `Module::gen_deps`).

Modules are namespaced per module (MR), so stdlib names never collide with a
program's own -- but a user module named `LIST`, `MAP`, ... would silently
MERGE with the stdlib module of that name (MR merges same-named fragments).
Avoid reusing the stdlib module names for now; a guard is future work.

## The modules

Convention: predicates (and predicate parameters) are `Bool`-typed, matching
`if` and comparisons; three-way COMPARISONS (`cmp_int`, `cmp_str`, the `sort_by`
/ MAP orderings) stay Int (negative / 0 / positive). Where an operation needs
equality or ordering on a type parameter, it takes the function(s) explicitly
(dictionary passing); there are no type classes. IO's `write_file` /
`append_file` / `remove_file` return `Bool` success (true = ok). Paired
results use the language's tuples (`{\`A, \`B}`, accessed `.0`/`.1`) --
there is no Pair type.

| module | contents |
|--------|----------|
| `OPT`  | `Option` (`Some`/`None`), `is_some`, `is_none`, `unwrap_or`, `opt_map`, `opt_then` |
| `LIST` | `length`, `is_empty`, `map`, `filter`, `foldl`, `foldr`, `reverse`, `append`, `concat`, `take`, `drop`, `nth`, `set_nth`, `any`, `all`, `find`, `contains`, `range`, `zip`, `zip_with`, `unzip`, `sum`, `product`, `minimum`, `maximum`, `last`, `init`, `replicate`, `intersperse`, `take_while`, `drop_while`, `span`, `split_at`, `partition`, `filter_map`, `flat_map`, `remove_first`, `find_index`, `lookup`, `sort_by`, `merge_by` |
| `STR`  | `len`, `at`, `substr`, `eq`, `cmp_str`, `from_byte`, `starts_with`, `ends_with`, `find`, `find_from`, `contains`, `split`, `lines`, `join`, `concat`, `trim`(`_left`/`_right`), `repeat`, `pad_left`, `pad_right`, `replace`, `count`, `reverse`, `map_bytes`, `to_upper`, `to_lower`, `is_space`/`is_digit`/`is_alpha`/`is_alnum`/`is_upper`/`is_lower`, `from_int`, `to_int` |
| `MATH` | Int: `min`, `max`, `cmp_int`, `abs`, `sign`, `clamp`, `even`, `odd`, `gcd`, `pow`; Real: `pi`, `euler`, `min`, `max`, `abs`, `clamp`, `sqrt`, `sin`, `cos`, `tan`, `atan2`, `exp`, `log`, `floor`, `ceil`, `round`, `pow`, `fmod` (libm via `C`) |
| `IO`   | `print`, `println`, `eprint`, `eprintln`, `print_int`, `println_int`, `read_line`, `read_file`, `write_file`, `append_file`, `remove_file`, `env`, `now` |
| `MAP`  | `Map \`K \`V` (immutable AVL tree); `new`, `new_str`, `new_int`, `from_list`, `cmp_pair`, `insert`, `insert_with`, `get`, `get_or`, `has`, `remove`, `update`, `size`, `is_empty`, `to_list`, `keys`, `values`, `fold`, `map_values`, `filter`, `merge`, `min_entry`, `max_entry` |
| `RESULT` | `Result` (`Ok`/`Err`), `is_ok`, `is_err`, `unwrap_or`, `map_ok`, `map_err`, `and_then`, `ok_opt`; the `Fail` effect, `try`, `try_or`, `untry`, `expect` |
| `RANDOM` | `Rng` (Lehmer / MINSTD), `new`, `next`, `next_below`, `next_range` |
| `SET`  | `Set \`T` (ordered, over MAP); `new`, `new_int`, `new_str`, `from_list`, `add`, `has`, `remove`, `size`, `is_empty`, `to_list`, `fold`, `filter`, `merge`, `inter`, `diff` |
| `PATH` | POSIX paths, pure Str: `basename`, `dirname`, `extension`, `strip_ext`, `join`, `parts`, `is_abs` |
| `VEC`  | `Vec \`T` (growable vector, O(1) access); `new`, `fill`, `len`, `is_empty`, `get`, `get_or`, `set`, `push`, `last`, `from_list`, `to_list`, `map`, `fold` |

`STR` and `LIST` share some natural names (`reverse`, `find`, `contains`,
`repeat`, `concat`); importing both is fine -- overloading resolves by type,
and the qualified `STR.reverse` / `LIST.find` forms always work.

### The map

`Map \`K \`V` is an immutable ORDERED map -- an AVL tree, the same design as
Haskell's `Data.Map` -- polymorphic in key and value with no type classes:
the map STORES its key ordering (a three-way `cmpf`, strcmp convention) as a
field. `new` takes the comparison; `new_str`/`new_int` bake in the common
keys (`STR.cmp_str`, `MATH.cmp_int`). Lookup, insert and remove are
O(log n); `to_list`/`keys`/`fold` visit entries in ascending key order.
`cmp_pair` composes two element orderings into the lexicographic ordering on
tuples, for tuple-keyed maps: `new (cmp_pair cmp_int cmp_str)` orders a
`Map {Int, Str} \`V`. Maps are values: `insert`/`remove` return an updated
copy and never disturb the original -- structural sharing makes that cheap
(path copying only). A MUTABLE hash table (Rust/Jai flavor) is future work,
now unblocked by `Vec`; see doc/stdlib-design.md.

### The vector

`Vec \`T` is the generic growable vector: O(1) indexed access over elements
of any type (the byte vectors `Str`/`Array` cover the Int-byte case). `Vec`
is an OPAQUE core type -- the type checker knows only its name and arity;
the behavior lives in the built-in primitives `vec_new` / `vec_fill` /
`vec_len` / `vec_get` / `vec_set` / `vec_push` (reserved names, like the
`array_*` family), and `library/VEC.thx` is the derived surface: `get`
returns Option and `set` leaves the vector unchanged out of range (the raw
primitives FAULT), plus `from_list`/`to_list`, `map`/`fold`. At runtime a
Vec is a variant tagged `"%vec"` whose payload fields are the elements, so
neither engine grew a new value kind. A Vec is a VALUE: mutators return an
updated vector and never disturb the original; the native backend reuses a
uniquely-owned (rc==1) spine in place for `set`/`push`, the same
opportunistic trick as the byte vectors.

### Fallible code: Result and the Fail effect

`RESULT` carries both shapes of fallibility and the bridges between them:
`Result` for storing outcomes as values, and the `Fail` effect for
propagating them. A function that may fail says so in its row
(`Int -> <Fail> Int`) and just calls `Fail.fail "msg"`; callers compose with
no plumbing (the row does what Rust's `?` does, implicitly), and a boundary
turns the possibility back into a value with `try` (giving a
`Result \`T Str`, Koka's `error<a>` pattern) or `try_or` (a default).
`untry`/`expect` re-raise a `Result`/`Option` inside a failing computation.

## The libc seam (and its sharp edge)

IO goes through the injected `C` namespace, which DR extended with `fopen`,
`fclose`, `fgetc`, `fputs`, `fflush`, `fseek`, `ftell`, `write`, `remove`,
`getenv` and `time`, plus the libm functions (`sqrt`, `sin`, `cos`, `tan`,
`atan2`, `exp`, `log`, `floor`, `ceil`, `round`, `pow`, `fmod`). The
declarations name their libraries SYMBOLICALLY (`"libc"`, `"libm"`); where
the symbols actually live is resolved at the edge -- `TG::Target::soname`
for the interpreter's dlopen (on Linux libm is its own soname: Nix ships the
symbols only in libm.so.6 even though glibc >= 2.34 folded them into libc
for static linking; elsewhere the C library carries them), and a link-line
flag for the native backend (generated programs never dlopen). Two
deliberate compromises, both documented at the declarations in app/DR.cpp:

- **`FILE*` travels as `Int`, not `Ptr`.** The handle is only passed back to
  the same functions or tested against 0 (NULL), and `?=` is defined on Int
  but not Ptr.
- **A C `int` return does not arrive sign-extended.** The FFI widens returns
  by the DECLARED Thrax type; declaring libc's 32-bit `int` results as the
  64-bit `Int` means a negative return (EOF, error codes) arrives as its
  zero-extended bit pattern, not a negative Int. The stdlib therefore never
  tests `?< 0`: success is `?= 0` (exact under any extension), and
  end-of-stream is "not a byte", i.e. outside 0..255 (`IO.is_byte`) -- also
  exact under any extension. File reads are counted (`fseek`/`ftell`), not
  EOF-terminated. The principled fix is FFI awareness of C's `int` width
  (declare `Int32` and make sized ints usable, or a TG-provided `cint`);
  until then, new `C` declarations must follow the same rules.

Console output uses unbuffered `C.write` throughout (never mixed with C's
buffered stdout), so ordering is deterministic. `C.puts` remains for the
prelude's `assert` and quick scripts.

## Testing

`examples/STDLIB_{CORE,LIST,STR,MAP,RESULT,RANDOM,IO,SET,PATH,VEC}.thx`
exercise the
library and run in BOTH engines via the combined runner (tests/MAIN.thx):
`./build test` (interpreter) and `./build native-test` (C backend). The IO
test round-trips a file under /tmp and cleans up after itself; the MAP test
stresses the AVL balance with 200 ascending inserts (the classic worst case
for an unbalanced BST); the RANDOM test pins the exact MINSTD sequence.

## A note on inference

Resolving `p.snd ?= "one"` (a polymorphic struct projection feeding an
overloaded operator) required letting ready field accesses participate in
TC's operator-resolution fixpoint -- previously the operator Int-defaulted
before the projection's type was grounded. See `Checker::resolve_sites` /
`settle_ready_field_sites` in compiler/TC.cpp.

User overload sites joined the same fixpoint for the same reason:
`(pow 2.0 10.0) ?= 1024.0` (a USER overload feeding an overloaded operator)
used to deadlock -- built-in sites resolved (and Int-defaulted) before any
user site was judged, so `?=` forced `pow`'s result to Int and both sites
failed. Inside the fixpoint a user site is resolved CONSERVATIVELY (commit
only when exactly one candidate fits, wait on open operator operands or
multiple fits, never default); this is sound because unification only ever
shrinks a site's fit set, so an early single-fit commit is the same commit a
later pass would make. `resolve_user_sites` afterwards forces whatever is
left, with the old Int-defaulting (`\x = x + x` still picks the Int
built-in). See `Checker::resolve_one_user_site`.

## Future work

See doc/stdlib-design.md for the full inventory-driven roadmap (Koka/Jai
module surveys, the mutable hash table, tuples, SET/PATH/DIR/PROCESS/...).
Smaller items:

- A guard against user modules merging with stdlib module names.
- `Real` formatting and parsing in STR (`gcvt`-style via libm/libc),
  buffered readers.
- Proper C `int` FFI marshalling (kills the `is_byte` idiom).
- Migrate IO's error reporting from Int codes / Option to `Result` with a
  standard `Error` union, or `<Fail>` rows, once the shape settles.
