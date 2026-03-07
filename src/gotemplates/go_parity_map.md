# Go Parity Map

This file tracks which Go stdlib sources are the reference for the Rust
`gotemplates` implementation.

## Parser and Scanner

- Rust: `src/gotemplates/parser.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: pipeline parsing, command parsing, declaration handling, undefined
    function checks, undefined variable checks.
- Rust: `src/gotemplates/parser/lex.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: action lexing, token classification, number and identifier scanning.
- Rust: `src/gotemplates/scanner.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: action token scanning, delimiter handling, string/comment boundaries.

## Builtins and Rendering

- Rust: `src/gotemplates/functions.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: builtin function surface and semantic expectations.
- Rust: `src/gotemplates/executor.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: runtime pipeline evaluation, control flow, range/if/with behavior.
- Rust: `src/gotemplates/executor/control.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: control-flow block execution (`if`/`with`/`range`), `else`/`end`
    boundary matching, template/block invocation.
- Rust: `src/gotemplates/executor/eval.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: expression evaluation, pipeline execution, command dispatch and
    non-executable command diagnostics.

## printf Compatibility

- Rust: `src/gotemplates/compat/printf.rs` and `src/gotemplates/compat/printf/*`
  - Go reference: `src/fmt/print.go`, `src/fmt/format.go`
  - Scope: `%` verb parsing, argument indexing (`%[n]`), `*` width/precision
    semantics, mismatch diagnostics and output layout.

## Deviation Rules

- Any deliberate behavior difference must be marked in code with `Deviation:`.
- Tests are not updated to fit Rust behavior; Rust behavior is fixed to match Go.
