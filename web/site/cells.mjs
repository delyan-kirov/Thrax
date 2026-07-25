// The notebook's cells: the tour shown on the playground page. index.html
// imports this and renders one card per entry; web/smoke.mjs imports the same
// list and runs every `src` through the wasm compiler, so what CI verifies is
// exactly what ships. Each `src` is a complete program that prints a result.
export const CELLS = [
  {
    id: "hello",
    title: "Hello, Thrax",
    blurb: "A program is a list of typed definitions. `main : Int` is the entry point and its Int is the exit code. `C.puts` prints a line; `;` runs one expression for its effect, then the next.",
    src: `@mod MAIN

$ main : Int =
	C.puts "hello, Thrax";
	C.puts "every definition has a type, and main returns the exit code";
	0
`,
  },
  {
    id: "recursion",
    title: "Functions and recursion",
    blurb: "Functions are values: `\\n = ...` is a lambda, written under a separate type signature. `if` is an expression, and recursion is the loop. (`?<` is less-than; bare `<` is reserved for effect rows.)",
    src: `@mod MAIN

$ with STR

$ fib : Int -> Int = \\n =
	if n ?< 2 then n
	else fib (n - 1) + fib (n - 2)

$ main : Int =
	C.puts ("fib 10 = " ++ STR.from_int (fib 10));
	C.puts ("fib 20 = " ++ STR.from_int (fib 20));
	0
`,
  },
  {
    id: "unions",
    title: "Sum types and pattern matching",
    blurb: "A `@union` is a sum type; each variant can carry a payload. `when` tries arms top-to-bottom and binds the payload with `Tag.{..}`. `else` is an optional catch-all. You can drop it when the arms already cover every variant, as here.",
    src: `@mod MAIN

$ with STR

$ Shape : @union =
	Circle: Int,
	Rect: {Int, Int},

$ area : Shape -> Int = \\s =
	when s
		is Shape.Circle.{r} then 3 * r * r
		is Shape.Rect.{w, h} then w * h

$ main : Int =
	C.puts ("circle r=2: " ++ STR.from_int (area Shape.Circle.{2}));
	C.puts ("rect 3x4:   " ++ STR.from_int (area Shape.Rect.{3, 4}));
	0
`,
  },
  {
    id: "guards",
    title: "Guards and the catch-all else",
    blurb: "An arm may carry a guard: `is <pat> if <cond> then ...`. When the guard fails the match falls through to the next arm, and a final `else` catches everything the arms miss, here the zero case.",
    src: `@mod MAIN

$ with STR

$ sign : Int -> Str = \\n =
	when n
		is m if m ?> 0 then "positive"
		is m if m ?< 0 then "negative"
	else "zero"

$ main : Int =
	C.puts ("sign  7 = " ++ sign 7);
	C.puts ("sign -3 = " ++ sign (0 - 3));
	C.puts ("sign  0 = " ++ sign 0);
	0
`,
  },
  {
    id: "tuples",
    title: "Tuples",
    blurb: "`{A, B}` is an anonymous product of any arity. Read elements positionally with `.0`/`.1`, and destructure with `{a, b}` patterns in `let`, lambdas and `when` arms. A `let` can bind several names, comma-separated.",
    src: `@mod MAIN

$ with STR

$ swap : {\`A, \`B} -> {\`B, \`A} = \\t = {t.1, t.0}

$ main : Int =
	let
		p = {42, "answer"},
		q = swap p,
	in
		C.puts <| "p.0        = " ++ STR.from_int p.0;
		C.puts <| "swapped .0 = " ++ q.0;
		0
`,
  },
  {
    id: "lists",
    title: "Lists",
    blurb: "`[a, b, c]` builds a list, `h :: t` conses, `[]` is empty. Patterns mirror the sugar: `is []` and `is h :: t` walk a list one cell at a time.",
    src: `@mod MAIN

$ with STR

$ sum : List Int -> Int = \\xs =
	when xs
		is [] then 0
		is h :: t then h + sum t
	else 0

$ main : Int =
	let xs = [1, 2, 3, 4, 5] in
	C.puts ("sum [1..5] = " ++ STR.from_int (sum xs));
	0
`,
  },
  {
    id: "pipes",
    title: "Pipes and sequencing",
    blurb: "`x |> f` and `f <| x` are just application at the lowest precedence, so pipelines read in order. `;` runs the left side for effect, then yields the right.",
    src: `@mod MAIN

$ with STR

$ inc : Int -> Int = \\x = x + 1
$ dbl : Int -> Int = \\x = x + x

$ main : Int =
	let r = 5 |> inc |> dbl in
	C.puts ("5 |> inc |> dbl = " ++ STR.from_int r);
	0
`,
  },
  {
    id: "effects-gen",
    title: "Algebraic effects · generators",
    blurb: "Thrax's headline feature. An operation like `yield` is performed by calling it; a handler `do <body> ctl k is Op a = e` intercepts it, where `k` is the resumable continuation. Resuming `k` and adding the results turns a generator into a sum. No iterator protocol, just a handler.",
    src: `@mod MAIN

$ with STR

$ Yield : @effect = yield : Int -> {},

$ sumGen : ({} -> <Yield> {}) -> Int = \\gen =
	do gen {}
	ctl k is Yield.yield v = v + k {}
	      else _ = 0

$ gen3 : {} -> <Yield> {} = \\u =
	Yield.yield 10 ; Yield.yield 20 ; Yield.yield 12 ; {}

$ main : Int =
	C.puts ("sum of yields = " ++ STR.from_int (sumGen gen3));
	0
`,
  },
  {
    id: "effects-exn",
    title: "Algebraic effects · exceptions",
    blurb: "The same machine gives you exceptions: a handler that simply ignores `k` never resumes. `throw`'s result type is polymorphic (`\`a`) because it never returns to the call site. `Exn` is handled inside `safeDiv`, so `safeDiv` is pure. Its type carries no effect.",
    src: `@mod MAIN

$ with STR

$ Exn : @effect = throw : Str -> \`a,

$ safeDiv : Int -> Int -> Int = \\a b =
	do if b ?= 0 then Exn.throw "divide by zero" else a / b
	ctl k is Exn.throw msg = 0 - 1

$ main : Int =
	C.puts ("84 / 2 = " ++ STR.from_int (safeDiv 84 2));
	C.puts ("10 / 0 = " ++ STR.from_int (safeDiv 10 0));
	0
`,
  },
  {
    id: "effects-state",
    title: "Algebraic effects · state",
    blurb: "Mutable-looking state with no mutation: each handler clause returns a state-transforming function, and the handler threads the state through the resumptions. `counter` reads with `get`, writes with `put`, and never names a mutable cell.",
    src: `@mod MAIN

$ with STR

$ State : @effect = get : {} -> Int, put : Int -> {},

$ runState : ({} -> <State> Int) -> Int -> Int = \\action s0 =
	let h = do action {}
	        ctl k is get u = \\s = (k s) s
	              is put n = \\s = (k {}) n
	              else x = \\s = x
	 in h s0

$ counter : {} -> <State> Int = \\u =
	let
		x = get {},
		_ = put <| x + 1,
		y = get {}
	 in
		x + y

$ main : Int =
	C.puts ("counter from 10 = " ++ STR.from_int (runState counter 10));
	0
`,
  },
  {
    id: "browser",
    title: "Compiled in your browser",
    blurb: "This playground is the Thrax compiler itself, built to WebAssembly. Every program above is compiled, not interpreted by JS. `TARGET` is generated per compile; switch a cell's mode to see its Generated C or IR.",
    src: `@mod MAIN

$ with STR

$ main : Int =
	C.puts ("target:   " ++ TARGET.name);
	C.puts ("int bits: " ++ STR.from_int TARGET.int_bits);
	0
`,
  },
];
