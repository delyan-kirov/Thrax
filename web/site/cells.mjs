// The notebook's cells: the tour shown on the playground page. index.html
// imports this and renders one card per entry; web/smoke.mjs imports the same
// list and runs every `src` through the wasm compiler, so what CI verifies is
// exactly what ships. Each `src` is a complete program that prints a result.
export const CELLS = [
  {
    id: "hello",
    title: "Hello, Thrax",
    blurb: "A program is a list of typed definitions. `main : Int` is the entry point and its Int is the exit code. `HOST.print` prints a line; `;` runs one expression for its effect, then the next.",
    src: `@mod MAIN

$ with HOST

$ main : Int =
	HOST.print "hello, Thrax";
	HOST.print "every definition has a type, and main returns the exit code";
	0
`,
  },
  {
    id: "recursion",
    title: "Functions and recursion",
    blurb: "Functions are values: `\\n = ...` is a lambda, written under a separate type signature. `if` is an expression, and recursion is the loop. (`?<` is less-than; bare `<` is reserved for effect rows.)",
    src: `@mod MAIN

$ with STR
$ with HOST

$ fib : Int -> Int = \\n =
	if n ?< 2 then n
	else fib (n - 1) + fib (n - 2)

$ main : Int =
	HOST.print ("fib 10 = " ++ STR.from_int (fib 10));
	HOST.print ("fib 20 = " ++ STR.from_int (fib 20));
	0
`,
  },
  {
    id: "unions",
    title: "Sum types and pattern matching",
    blurb: "A `@union` is a sum type; each variant can carry a payload. `when` tries arms top-to-bottom and binds the payload with `Tag.{..}`. `else` is an optional catch-all. You can drop it when the arms already cover every variant, as here.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ Shape : @union =
	Circle: Int,
	Rect: {Int, Int},

$ area : Shape -> Int = \\s =
	when s
		is Shape.Circle.{r} then 3 * r * r
		is Shape.Rect.{w, h} then w * h

$ main : Int =
	HOST.print ("circle r=2: " ++ STR.from_int (area Shape.Circle.{2}));
	HOST.print ("rect 3x4:   " ++ STR.from_int (area Shape.Rect.{3, 4}));
	0
`,
  },
  {
    id: "guards",
    title: "Guards and the catch-all else",
    blurb: "An arm may carry a guard: `is <pat> if <cond> then ...`. When the guard fails the match falls through to the next arm, and a final `else` catches everything the arms miss, here the zero case.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ sign : Int -> Str = \\n =
	when n
		is m if m ?> 0 then "positive"
		is m if m ?< 0 then "negative"
	else "zero"

$ main : Int =
	HOST.print ("sign  7 = " ++ sign 7);
	HOST.print ("sign -3 = " ++ sign (0 - 3));
	HOST.print ("sign  0 = " ++ sign 0);
	0
`,
  },
  {
    id: "tuples",
    title: "Tuples",
    blurb: "`{A, B}` is an anonymous product of any arity. Read elements positionally with `.0`/`.1`, and destructure with `{a, b}` patterns in `let`, lambdas and `when` arms. A `let` can bind several names, comma-separated.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ swap : {\`A, \`B} -> {\`B, \`A} = \\t = {t.1, t.0}

$ main : Int =
	let
		p = {42, "answer"},
		q = swap p,
	in
		HOST.print <| "p.0        = " ++ STR.from_int p.0;
		HOST.print <| "swapped .0 = " ++ q.0;
		0
`,
  },
  {
    id: "lists",
    title: "Lists",
    blurb: "`[a, b, c]` builds a list, `h :: t` conses, `[]` is empty. Patterns mirror the sugar: `is []` and `is h :: t` walk a list one cell at a time.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ sum : List Int -> Int = \\xs =
	when xs
		is [] then 0
		is h :: t then h + sum t
	else 0

$ main : Int =
	let xs = [1, 2, 3, 4, 5] in
	HOST.print ("sum [1..5] = " ++ STR.from_int (sum xs));
	0
`,
  },
  {
    id: "pipes",
    title: "Pipes and sequencing",
    blurb: "`x |> f` and `f <| x` are just application at the lowest precedence, so pipelines read in order. `;` runs the left side for effect, then yields the right.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ inc : Int -> Int = \\x = x + 1
$ dbl : Int -> Int = \\x = x + x

$ main : Int =
	let r = 5 |> inc |> dbl in
	HOST.print ("5 |> inc |> dbl = " ++ STR.from_int r);
	0
`,
  },
  {
    id: "effects-gen",
    title: "Algebraic effects · generators",
    blurb: "Thrax's headline feature. An operation like `yield` is performed by calling it; a handler `do <body> ctl k is Op a = e` intercepts it, where `k` is the resumable continuation. Resuming `k` and adding the results turns a generator into a sum. No iterator protocol, just a handler.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ Yield : @effect = yield : Int -> {},

$ sumGen : ({} -> <Yield> {}) -> Int = \\gen =
	do gen {}
	ctl k is Yield.yield v = v + k {}
	      else _ = 0

$ gen3 : {} -> <Yield> {} = \\u =
	Yield.yield 10 ; Yield.yield 20 ; Yield.yield 12 ; {}

$ main : Int =
	HOST.print ("sum of yields = " ++ STR.from_int (sumGen gen3));
	0
`,
  },
  {
    id: "effects-exn",
    title: "Algebraic effects · exceptions",
    blurb: "The same machine gives you exceptions: a handler that simply ignores `k` never resumes. `throw`'s result type is polymorphic (`\`A`) because it never returns to the call site. `Exn` is handled inside `safeDiv`, so `safeDiv` is pure. Its type carries no effect.",
    src: `@mod MAIN

$ with STR
$ with HOST

$ Exn : @effect = throw : Str -> \`A,

$ safeDiv : Int -> Int -> Int = \\a b =
	do if b ?= 0 then Exn.throw "divide by zero" else a / b
	ctl k is Exn.throw msg = 0 - 1

$ main : Int =
	HOST.print ("84 / 2 = " ++ STR.from_int (safeDiv 84 2));
	HOST.print ("10 / 0 = " ++ STR.from_int (safeDiv 10 0));
	0
`,
  },
  {
    id: "effects-state",
    title: "Algebraic effects · state",
    blurb: "Mutable-looking state with no mutation: each handler clause returns a state-transforming function, and the handler threads the state through the resumptions. `counter` reads with `get`, writes with `put`, and never names a mutable cell.",
    src: `@mod MAIN

$ with STR
$ with HOST

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
	HOST.print ("counter from 10 = " ++ STR.from_int (runState counter 10));
	0
`,
  },
  {
    id: "host",
    title: "Talking to the host · WASM externs",
    blurb: "The compiler was ported to web assembly and since the language has an ffi functionality with `@extern \"WASM\" \"name\"` we can call JavaScript functions. See what happens if you make `animate` true.",
    src: `@mod MAIN

$ with STR
$ with HOST

# Flip to true and re-run to start seeing the colors.
$ animate : Bool = false

# An effect standing for "recolor the scratchpad".
$ ChangeColorEffect : @effect = recolor : Str -> {},

# Host imports
$ paint  : Str -> {}  = @extern "WASM" "change_color" ""
$ random : Int -> Int = @extern "WASM" "random" ""
$ delay  : Int -> {}  = @extern "WASM" "delay" ""

$ color : {} -> Str = \\u =
	"rgb(" ++ STR.from_int (random 256)
	      ++ ", " ++ STR.from_int (random 256)
	      ++ ", " ++ STR.from_int (random 256) ++ ")"

$ withColor : ({} -> <ChangeColorEffect> {}) -> {} = \\body =
	do body {}
	ctl k is ChangeColorEffect.recolor c = (if animate then paint c else {}) ; k {}

$ spin : Int -> <ChangeColorEffect> {} = \\n =
	if n ?= 0 then {}
	else
		let c = color {} in
		HOST.print c ;
		recolor c ;
		spin (n - 1)

$ main : Int =
	delay 150 ;
	let _ = withColor (\\u = spin 12) in
	0
`,
  },
];
