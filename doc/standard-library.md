# The standard library

The standard library is a set of ordinary Thrax modules under `library/`,
nothing fancy: thin, readable code over the auto-injected `C` libc namespace
and the byte-vector primitives. It is deliberately separate from the prelude
(see doc/architecture.md): the prelude is the language's blessed core (`List`,
the numeric aliases, `assert`); the standard library is a toolbox a program
opts into with `$ with MOD`.

## How it reaches a program

The build bakes `library/*.thx` into the compiler binary
(`BLD::gen_stdlib_header` -> `artifacts/STDLIBxAMALG.hpp`); DR injects every
unit into every compile, right after the prelude and `C` (app/DR.cpp
`parse_units`). So:

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

Convention: predicates (and predicate parameters) use Int truth -- 1 true, 0
false -- matching `if`. Where an operation needs equality or hashing on a
type parameter, it takes the function(s) explicitly (dictionary passing);
there are no type classes.

| module | contents |
|--------|----------|
| `OPT`  | `Option` (`Some`/`None`), `is_some`, `is_none`, `unwrap_or`, `opt_map`, `opt_then` |
| `PAIR` | `Pair` (`.fst`/`.snd`), `pair`, `swap` |
| `LIST` | `length`, `is_empty`, `map`, `filter`, `foldl`, `foldr`, `reverse`, `append`, `concat`, `take`, `drop`, `nth`, `set_nth`, `any`, `all`, `find`, `contains`, `range`, `zip`, `sum` |
| `STR`  | `len`, `at`, `substr`, `eq`, `starts_with`, `ends_with`, `find`, `find_from`, `contains`, `split`, `lines`, `join`, `trim`(`_left`/`_right`), `repeat`, `reverse`, `map_bytes`, `to_upper`, `to_lower`, `is_space`/`is_digit`/`is_alpha`, `from_int`, `to_int` |
| `MATH` | `min`, `max`, `abs`, `sign`, `clamp`, `even`, `odd`, `gcd`, `pow` |
| `IO`   | `print`, `println`, `eprint`, `eprintln`, `print_int`, `println_int`, `read_line`, `read_file`, `write_file`, `append_file`, `remove_file`, `env` |
| `MAP`  | `Map \`K \`V`; `new`, `new_sized`, `new_str`, `new_int`, `insert`, `get`, `has`, `remove`, `size`, `to_list`, `keys`, `values`, `hash_str`, `hash_int` |

`STR` and `LIST` share some natural names (`reverse`, `find`, `contains`,
`repeat`); importing both is fine -- overloading resolves by type, and the
qualified `STR.reverse` / `LIST.find` forms always work.

### The hash map

`Map \`K \`V` is polymorphic in both key and value with no type classes: the
map STORES its key operations (`hashf`, `eqf`) as fields -- `new` takes them,
`new_str`/`new_int` bake in the common cases, and every operation reads them
back off the struct. Buckets are association lists behind a fixed bucket
count (16 by default, `new_sized` to choose); an over-full map degrades to
linear but stays correct. Maps are values: `insert`/`remove` return an
updated copy and never disturb the original. Rehashing/growth is future work.

## The libc seam (and its sharp edge)

IO goes through the injected `C` namespace, which DR extended with `fopen`,
`fclose`, `fgetc`, `fputs`, `fflush`, `fseek`, `ftell`, `write`, `remove` and
`getenv` (host libc soname via `TG::Target`, as before). Two deliberate
compromises, both documented at the declarations in app/DR.cpp:

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

`examples/STDLIB_{CORE,LIST,STR,MAP,IO}.thx` exercise the library and run in
BOTH engines via the combined runner (tests/MAIN.thx): `./build test`
(interpreter) and `./build native-test` (C backend). The IO test round-trips
a file under /tmp and cleans up after itself.

## A note on inference

Resolving `p.snd ?= "one"` (a polymorphic struct projection feeding an
overloaded operator) required letting ready field accesses participate in
TC's operator-resolution fixpoint -- previously the operator Int-defaulted
before the projection's type was grounded. See `Checker::resolve_sites` /
`settle_ready_field_sites` in compiler/TC.cpp.

## Future work

- A guard against user modules merging with stdlib module names.
- `Result`/error type once effects are the blessed error channel.
- Map growth (rehash on load factor), sorting for LIST, `Real` formatting
  and parsing in STR (`gcvt`-style via libm/libc), buffered readers.
- Proper C `int` FFI marshalling (kills the `is_byte` idiom).
