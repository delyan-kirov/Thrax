# Negative suite

Programs the compiler must reject, each pinned to the diagnostics it has to
produce. `cargo test` runs them (`crates/thrax/tests/diagnostics.rs`) by calling
`thrax check` on every case and matching its stderr against the case's `#~`
annotations.

A case is a `NAME.thx` file, or a `NAME/` directory whose root is `MAIN.thx`
when the diagnostic needs several modules.

```text
$ p : P = P.{ 1 }  #~ error[TYPE_MISMATCH]: literal is missing field `y`
$ s : @Str = "abc
#~^ error[QUOTM_UNCLOSED]
#~ error: must have the signature
```

- A trailing `#~` pins its own line. `#~^` pins the line above, `#~^^` two
  above, and so on.
- A `#~` alone on its line, with no carets, is a diagnostic with no location in
  the case's files.
- `error[CODE]` or `warning[CODE]` give the severity and code. A bare `error`
  matches the driver's uncoded `thrax: ...` errors.
- Text after the `:` must occur in the message. It is a substring, so the column
  and the `note:`/`help:` lines are free to change.

The match is exact in both directions: every annotation needs a diagnostic and
every diagnostic needs an annotation. A case fails if it compiles when an error
is expected, if it has no annotations, or if the compiler crashes.

To add a case, write the program, run `thrax check` on it, and annotate the
lines it reports.
