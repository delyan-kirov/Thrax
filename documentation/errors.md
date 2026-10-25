# Diagnostic codes

Every Thrax diagnostic carries a stable code, such as `error[TYPE_MISMATCH]`.
`thrax explain CODE` prints the section below for that code, and the language
server shows the code next to the message. A code never changes meaning; a new
kind of failure gets a new code.

Each section says what the code means, the usual causes, and how to fix it. A
diagnostic's own `help:` lines are more specific, and when the compiler knows
the exact change it is offered as a fix (`thrax fix`, or the editor's quick fix).

## ASCII_CTR_CHAR

The source holds an ASCII control character (other than tab, newline and
carriage return) outside a string or comment.

Usually a stray byte pasted from a terminal or another program. Delete it; if
you meant the character in a string, write it as an escape (`"\t"`, `"\n"`).

## NON_ASCII_CHAR

A non-ASCII character appears where only ASCII is allowed: in a name, an
operator or between tokens. Strings and comments may hold any UTF-8 text.

Look for look-alike characters (a non-breaking space, curly quotes, a Unicode
minus sign) and replace them with their ASCII forms.

## ILLEGAL_RESERVED_CHAR

A character reserved for future syntax was used. Reserved, not currently
emitted.

## QUOTM_UNCLOSED

A string literal (or a block comment) is not closed before the end of the line
or the file.

Add the closing `"` (or `-#` for a `#- ... -#` comment). A string cannot span
lines; join the pieces with `++`.

## INVALID_ESCAPE

A string holds a backslash escape the lexer does not know, such as `"\q"`.

The escapes are `\n`, `\t`, `\r`, `\0`, `\\`, `\"`, `\{` and `\xHH`. To write a
backslash itself, double it.

## INVALID_UTF8

The source file is not valid UTF-8. Reserved, not currently emitted.

Re-save the file as UTF-8.

## NUMBER_PARSING_FAILURE

A numeric literal is malformed: an unfinished decimal (`2.`), a digit outside
its radix (`0b102`), or a value too large to represent.

For `2.`, write the fractional part, `2.0` (offered as a fix). A number has no
fields, so `2.x` is also reported here.

## UNKNOWN_SYMBOL

A character or operator the lexer does not know, such as `'?'` or an operator
spelling with no meaning (`+-*`).

Check for a typo in the operator. A user-defined operator must first be
declared with `$ (op) : T = ...`.

## EXPECTED_OPERAND

An operator is missing an operand. Reserved, not currently emitted: the parser
reports this as `UNEXPECTED_TOKEN`.

## UNEXPECTED_TOKEN

The parser met a token it cannot use at that point. The message names what it
expected and what it found.

When it found "the next item", the current item stopped short: something is
missing at its end, most often a closing `)`, `]` or `}` (offered as a fix), an
`else`, or the expression after `=`. The parser skips to the next `$` and keeps
going, so the errors after this one are still reported.

## PARENTHESIS_UNBALANCED

Brackets do not pair up. Reserved, not currently emitted: an unclosed bracket is
reported as `UNEXPECTED_TOKEN` at the point the closer is missing.

## EXPECTED_GLOBAL

A top-level item was expected. Reserved, not currently emitted.

Every top-level item starts with `$` at the beginning of a line.

## UNSUPPORTED

The syntax is recognized but Thrax does not support it, such as renaming an
import.

The message says what to write instead.

## TYPE_MISMATCH

Two types that must be equal are not: an argument does not match the
parameter, a body does not match its signature, the arms of a match produce
different types, an effect is performed where it is not handled.

Read the `expected` and `found` types in the message; a label points at where
the expectation came from. Common causes: an integer literal where a float is
expected (`1` is never a float; write `1.0`), a missing argument, a function
applied to too many arguments (put `;` between separate steps), or a pure
signature hiding an effect its body performs (add it to the arrow, `-> <@IO> T`).

## TYPE_UNBOUND

A name is used but not defined anywhere in scope: an unbound value, an unknown
type, or a type that is defined in a module this one does not import.

If it is a typo, the did-you-mean fix corrects it. If the name lives in a
standard-library module, the fix adds the `$ with MODULE` import. Type names are
capitalized; a lowercase name in a type is a type variable.

## TYPE_ANNOTATION_REQUIRED

The type of something cannot be inferred and must be written. Reserved, not
currently emitted; such cases are reported as `TYPE_MISMATCH` with a `help:`
suggesting the annotation.

## TYPE_CYCLE

A type refers to itself in a way that has no meaning: an infinite type
(`\x = x x`), or an alias that expands to itself.

An alias only renames a type; to make a recursive type, declare a `@struct` or
`@union`, whose name stops the expansion.

## BAD_MODULE_NAME

A module name is malformed. Reserved, not currently emitted.

## FILENAME_MISMATCH

A module's `$ @mod NAME` does not match its file name. Reserved, not currently
emitted.

## AMBIGUOUS_NAME

A bare name is imported from more than one module, so it names nothing on its
own.

Qualify it with its module, `MOD.name`; the message lists the candidates.

## DUPLICATE_SYMBOL

A module defines the same name twice. A name has one type, so it has one
definition.

To give an operation several types, declare an interface and define one
instance per type.

## UNKNOWN_MODULE

A `$ with MODULE` names a module that cannot be found, or a qualified name
`M.x` uses a module that is not in scope.

Check the spelling (a did-you-mean fix is offered), that the file `MODULE.thx`
is next to the importing file, in `library/`, or in a directory given with
`--import-dir`.

## PRIVATE_SYMBOL

A name below a module's `$ @private` marker was used from another module.
Reserved, not currently emitted.

## NO_ENTRY

A program to run has no `$ @main : @Vec @Str -> <@IO> @Int`. Reserved, not
currently emitted: the driver reports this without a code.

## ENTRY_SIGNATURE

`@main` has the wrong signature. Reserved, not currently emitted: the driver
reports this without a code. The one entry signature is
`@Vec @Str -> <@IO> @Int`.

## META_ABORT

Compile-time code ran `@abort`: a `$ @e` check (usually `assert`) or a
generator failed the build on purpose.

The message is the one the check gave. Fix what it checks, or the check.

## INT_LITERAL_RANGE

An integer literal does not fit the type it is used at, such as `300` as an
`@Nat8`.

Use a wider type, or `@cast` a wider value to wrap it into the narrow one.

## NON_EXHAUSTIVE_MATCH

A match does not cover every value its scrutinee can have; the message names a
value no arm matches.

Add an arm for it, or a catch-all `| _ => ...`. A refutable pattern in a `let`
or a lambda parameter is reported here too: bind a variable and match it with
`is`, which can handle every case.

## UNREACHABLE_ARM

A match arm can never be chosen, because earlier arms already match every value
it would. This is a warning.

Delete the arm, or move it above the arm that covers it. The level is set by
`[lints] unreachable_arm` in `thrax.toml` (`allow`, `warn` or `deny`).

## RUNTIME_FAULT

Evaluation failed: a division by zero, an out-of-range index, an unhandled
effect at run time. At compile time this comes from a `$ @e` site.

The note says which compile-time site raised it, when there is one.

## EXPANSION_LIMIT

Compile-time expansion did not settle: every round ran a site that produced
another site to expand.

A generator is producing code that contains its own call. Make it produce the
final code, or stop the recursion.

## TYPED_HOLE

A `???` typed hole: a placeholder for an expression not written yet. It has
whatever type its context needs, and the error names that type, so a hole is a
way to ask the compiler "what goes here?".

The note lists the names in scope whose type fits, locals first; each is
offered as a fix that fills the hole. A program with a hole never runs.
