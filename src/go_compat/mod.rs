// Unified Go-compat layer for all behavior parity domains used by happ.
// Keep all parity logic under this root to avoid behavior fragmentation.
//
// Go source markers (primary transfer map):
// - actionparse      -> go/src/text/template/parse/parse.go
// - analyzer/*       -> go/src/text/template/parse/* (analysis over parsed trees)
// - call             -> go/src/text/template/exec.go (call target display)
// - collections      -> go/src/text/template/funcs.go
// - commandkind      -> go/src/text/template/exec.go
// - compare          -> go/src/text/template/funcs.go
// - compat/literals  -> go/src/text/template/parse/lex.go + parse helpers
// - compat/printf/*  -> go/src/fmt/print.go + format.go
// - evaldiag         -> go/src/text/template/exec.go diagnostics
// - expr             -> go/src/text/template/parse/parse.go
// - externalfn       -> go/src/text/template/exec.go
// - ident            -> go/src/text/template/parse/lex.go
// - parse/*          -> go/src/text/template/parse/*
// - path             -> go/src/text/template/exec.go
// - pipeline_decl    -> go/src/text/template/parse/parse.go
// - rangeeval        -> go/src/text/template/exec.go
// - template/*       -> go/src/text/template/template.go + option paths
// - textfmt          -> go/src/text/template/funcs.go
// - tokenize/trim    -> go/src/text/template/parse/lex.go + parse.go
// - truth            -> go/src/text/template/exec.go + funcs.go
// - typeutil/valuefmt-> go/src/text/template/exec.go + funcs.go
// - varcheck         -> go/src/text/template/parse/parse.go
//
// Additional mirror namespace:
// - go_std/* exposes Go-like package hierarchy for orientation.
pub mod actionparse;
pub mod analyzer;
pub mod backend;
pub mod call;
pub mod compat;
pub mod collections;
pub mod compare;
pub mod commandkind;
pub mod evaldiag;
pub mod expr;
pub mod externalfn;
pub mod go_std;
pub mod ident;
pub mod parse;
pub mod path;
pub mod pipeline_decl;
pub mod rangeeval;
pub mod template;
pub mod textfmt;
pub mod tokenize;
pub mod trim;
pub mod truth;
pub mod typeutil;
pub mod varcheck;
pub mod valuefmt;
