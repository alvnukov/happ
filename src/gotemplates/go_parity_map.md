# Go Parity Map

This file tracks which Go stdlib sources are the reference for the Rust
`gotemplates` implementation.

All Go-related compatibility code is centralized under `src/go_compat/*`.
Mirror namespace for upstream transfer lives at `src/go_compat/go_std/*`.

Backend switch interface:
- `NativeRenderOptions.logic_backend` controls which logic backend is used.
- `LogicBackend::GoFfi` is available as an explicit FFI backend (Go helper).
- `LogicBackend::Dual` runs go_ffi + go_compat in parallel and logs mismatches.
- `HAPP_TEMPLATE_BACKEND` can override backend selection (`go_ffi`, `go_compat`, `dual`, `rust_native`).
- `HAPP_TEMPLATE_DUAL_PRIMARY` controls primary result source in dual mode (`go_ffi` by default).
- `HAPP_TEMPLATE_DUAL_LOG` optionally redirects dual mismatch logs to a file (append mode).

The current priority is the builtins and execution branches observed in the
real chart corpus (`helm-apps` + integration examples). Less-used branches
(`call`, html/js/urlquery edge cases) are kept but are not the primary parity
target until the used surface is fully stabilized.

## Parser and Scanner

- Rust: `src/go_compat/parse/action.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: pipeline parsing, command parsing, declaration handling, undefined
    function checks, undefined variable checks.
- Rust: `src/go_compat/parse/action/lex.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: action lexing, token classification, number and identifier scanning.
- Rust: `src/gotemplates/parser.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: backward-compatible facade over canonical go_compat parse action APIs.
- Rust: `src/go_compat/parse/report.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: shared parse compatibility report model (`ParseCompatOptions`, control actions,
    declared/assigned/referenced variable spans) reused by parser/scanner and go_compat parse APIs.
- Rust: `src/gotemplates/scanner.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: backward-compatible facade over canonical go_compat scan APIs.
- Rust: `src/go_compat/scan.rs`
  - Go reference: `src/text/template/parse/lex.go` (token/error surface shape)
  - Scope: canonical token/span/error types plus action scanning/token parsing APIs.
- Rust: `src/go_compat/tokenize.rs`
  - Go reference: `src/text/template/parse/parse.go`, `src/text/template/parse/lex.go`
  - Scope: reusable command/pipeline token boundaries and outer-parentheses
    handling shared by executor and function-call analysis.
- Rust: `src/go_compat/utf8scan.rs`
  - Go reference: utility aligned with UTF-8 traversal used by parse/lex style scanners
  - Scope: shared UTF-8 byte scanning helper used by tokenizer paths.
- Rust: `src/go_compat/ident.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: shared Go identifier start/continue/name checks used across parser
    and executor-side analyzers.
- Rust: `src/go_compat/expr.rs`
  - Go reference: `src/text/template/parse/parse.go` (expression-shape checks)
  - Scope: quoted-string, complex-expression and niladic-identifier classifiers.
- Rust: `src/go_compat/pipeline_decl.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: extraction of pipeline declaration prefixes (`$x :=`, `$i, $v =`).
- Rust: `src/go_compat/actionparse.rs`
  - Go reference: `src/text/template/parse/parse.go`, `src/text/template/exec.go`
  - Scope: action-head classification (`if/with/range/else/template/block/define`)
    with trim-marker aware delimiter handling and structured parse errors.
- Rust: `src/go_compat/path.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: variable reference splitting (`$`, `$x`, `$x.y`), path segment/token checks,
    runtime simple-path traversal and Go-style path type naming/error reasons.
- Rust: `src/go_compat/commandkind.rs`
  - Go reference: `src/text/template/exec.go` (`notAFunction` and command-kind checks)
  - Scope: non-executable head detection, non-function targets, field-like command paths.
- Rust: `src/go_compat/call.rs`
  - Go reference: `src/text/template/exec.go` (`evalCall` target rendering path)
  - Scope: call-target display normalization (outer-paren stripping and fallback
    value rendering) reused by runtime call diagnostics.
- Rust: `src/go_compat/typeutil.rs`
  - Go reference: `src/text/template/exec.go`, `src/text/template/funcs.go`
  - Scope: slice/index argument normalization, map-key coercion, string-like byte helpers
    and shared Go type-name classification used by compare/collections paths.
- Rust: `src/go_compat/typedvalue.rs`
  - Go reference: internal typed-shape bridge used to represent Go-typed values in JSON
  - Scope: canonical encoding/decoding of `[]byte`, typed map/slice and zero-value helpers
    used by executor and go_compat builtins.
- Rust: `src/go_compat/compare.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: core comparison semantics (`eq/lt/le`), nil/map/slice comparability
    classes and detail reasons for non-comparable values.
- Rust: `src/go_compat/varcheck.rs`
  - Go reference: `src/text/template/parse/parse.go`
  - Scope: variable-visibility guard (`$var`), numeric/char literal shape checks
    and canonical undefined-variable diagnostic message builder.
- Rust: `src/go_compat/evaldiag.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: canonical runtime diagnostic strings and nil-command classification
    reused across evaluator branches to avoid duplicated message logic.

## Builtins and Rendering

- Rust: `src/go_compat/functions.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: canonical action-level function-call analysis helpers reused by planner/import flow.
- Rust: `src/gotemplates/functions.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: backward-compatible facade over canonical go_compat functions APIs.
- Rust: `src/gotemplates/executor.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: backward-compatible facade over canonical go_compat runtime APIs.
- Rust: `src/go_compat/runtime.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: canonical runtime API surface and execution engine.
- Rust: `src/go_compat/runtime/go_ffi.rs`
  - Go reference: `src/text/template/template.go`, `src/text/template/exec.go`
  - Scope: optional Go FFI execution backend (`go build` helper + JSON bridge).
- Rust: `src/go_compat/runtime/control.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: runtime submodule source for
    control-flow block execution (`if`/`with`/`range`), `else`/`end` boundary
    matching, template/block invocation.
- Rust: `src/go_compat/runtime/eval.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: expression evaluation, pipeline execution, command dispatch and
    non-executable command diagnostics, field-with-arguments errors.
- Rust: `src/go_compat/runtime/externalfn.rs`
  - Go reference: `src/text/template/exec.go` (`evalFunction` identifier dispatch)
  - Scope: external function dispatch boundary; `FunctionDispatchMode::GoStrict`
    keeps Go-compatible identifier-only head resolution, while
    `FunctionDispatchMode::Extended` enables happ dynamic-head extension.
- Rust: `src/go_compat/externalfn.rs`
  - Go reference: `src/text/template/exec.go` (`evalFunction` / unknown-function diagnostics)
  - Scope: shared identifier candidacy checks for external calls and canonical
    unknown/failed function reason builders reused by runtime adapters.
- Rust: `src/go_compat/runtime/path.rs`
  - Go reference: `src/text/template/exec.go`
  - Scope: used field-path resolution for `.`, `$`, `$var` chains, map/slice
    field diagnostics and missing-value modes.
- Rust: `src/go_compat/runtime/compare.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: builtin comparison semantics (`eq/ne/lt/le/gt/ge`), nil handling,
    non-comparable diagnostics and signed/unsigned integer cross-comparison.
- Rust: `src/go_compat/runtime/collections.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: adapter layer mapping runtime collection builtin calls into go_compat APIs.
- Rust: `src/go_compat/collections.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: core collection builtins (`len/index/slice`) with Go-compatible bounds,
    map-key coercion, typed nil/zero behavior and reflect-style out-of-range diagnostics.
- Rust: `src/go_compat/runtime/truth.rs`
  - Go reference: `src/text/template/exec.go`, `src/text/template/funcs.go`
  - Scope: truthiness semantics and `and/or/not` short-circuit result behavior.
- Rust: `src/go_compat/runtime/textfmt.rs`
  - Go reference: `src/text/template/funcs.go`
  - Scope: adapter layer mapping runtime builtin calls into go_compat text-format APIs.
- Rust: `src/go_compat/textfmt.rs`
  - Go reference: `src/text/template/funcs.go` (`print/println/html/js/urlquery`, `JSEscape` / `jsIsSpecial`)
  - Scope: text builtin rendering (`print`, `html`, `js`, `urlquery`) + Go-specific
    Unicode escape classification shared by `js` paths.
- Rust: `src/go_compat/trim.rs`
  - Go reference: `src/text/template/parse/lex.go`
  - Scope: trim-marker and ASCII whitespace helpers for `{{-` / `-}}` handling.
- Rust: `src/go_compat/valuefmt.rs`
  - Go reference: `src/text/template/exec.go` (`printableValue`) + Go fmt defaults
  - Scope: Go-like formatting for rendered values, including typed map/slice/bytes.
- Rust: `src/go_compat/truth.rs`
  - Go reference: `src/text/template/exec.go`, `src/text/template/funcs.go`
  - Scope: core truthiness + `and`/`or` short-circuit value selection semantics.

## printf Compatibility

- Rust: `src/go_compat/compat.rs`, `src/go_compat/compat/printf.rs` and `src/go_compat/compat/printf/*`
  - Go reference: `src/fmt/print.go`, `src/fmt/format.go`
  - Scope: `%` verb parsing, argument indexing (`%[n]`), `*` width/precision
    semantics, mismatch diagnostics and output layout.

## Deviation Rules

- Any deliberate behavior difference must be marked in code with `Deviation:`.
- Tests are not updated to fit Rust behavior; Rust behavior is fixed to match Go.
