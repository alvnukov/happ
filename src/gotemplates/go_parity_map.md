# Go Parity Map

This file tracks which Go stdlib sources are the reference for the Rust
`gotemplates` implementation.

The current priority is the builtins and execution branches observed in the
real chart corpus (`helm-apps` + integration examples). Less-used branches
(`call`, html/js/urlquery edge cases) are kept but are not the primary parity
target until the used surface is fully stabilized.

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
    non-executable command diagnostics, field-with-arguments errors.
- Rust: `src/gotemplates/executor/externalfn.rs`
  - Go reference: `src/text/template/exec.go` (`evalFunction` identifier dispatch)
  - Scope: external function dispatch boundary; `FunctionDispatchMode::GoStrict`
    keeps Go-compatible identifier-only head resolution, while
    `FunctionDispatchMode::Extended` enables happ dynamic-head extension.
- Rust: `src/gotemplates/executor/path.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: used field-path resolution for `.`, `$`, `$var` chains, map/slice
    field diagnostics and missing-value modes.
- Rust: `src/gotemplates/executor/compare.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: builtin comparison semantics (`eq/ne/lt/le/gt/ge`), nil handling,
    non-comparable diagnostics and signed/unsigned integer cross-comparison.
- Rust: `src/gotemplates/executor/collections.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: used collection builtins (`len/index/slice`) with Go-compatible
    bounds, missing-key behavior and index diagnostics.
- Rust: `src/gotemplates/executor/truth.rs`
  - Go reference: `src/text/template/exec.go`, `src/text/template/funcs.go`
  - Scope: truthiness semantics and `and/or/not` short-circuit result behavior.
- Rust: `src/gotemplates/executor/textfmt.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: used text builtins (`print/println`) and shared argument rendering
    path for `html/js/urlquery`.

## printf Compatibility

- Rust: `src/gotemplates/compat/printf.rs` and `src/gotemplates/compat/printf/*`
  - Go reference: `src/fmt/print.go`, `src/fmt/format.go`
  - Scope: `%` verb parsing, argument indexing (`%[n]`), `*` width/precision
    semantics, mismatch diagnostics and output layout.

## Deviation Rules

- Any deliberate behavior difference must be marked in code with `Deviation:`.
- Tests are not updated to fit Rust behavior; Rust behavior is fixed to match Go.
