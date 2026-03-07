// Unified Go-compat layer for all behavior parity domains used by happ.
// Keep all parity logic under this root to avoid behavior fragmentation.
pub mod actionparse;
pub mod analyzer;
pub mod call;
pub mod collections;
pub mod compare;
pub mod commandkind;
pub mod expr;
pub mod externalfn;
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
