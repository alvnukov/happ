pub mod compat;
// Parity source map: src/gotemplates/go_parity_map.md
mod executor;
mod functions;
// Backward-compatible alias; canonical module is `crate::go_compat`.
pub mod go_compat;
mod parser;
mod planner;
mod scanner;
mod typedvalue;
mod utf8scan;

pub use executor::{
    FunctionDispatchMode,
    render_template_native, render_template_native_with_options,
    render_template_native_with_resolver, MissingValueMode, NativeFunctionResolver,
    NativeFunctionResolverError, NativeRenderError, NativeRenderOptions,
};
pub use functions::{
    collect_function_calls_in_action, collect_function_calls_in_template, escape_template_action,
    normalize_values_global_context,
};
pub use parser::ParseCompatOptions;
pub use planner::{plan_template_execution, CompatibilityReason, CompatibilityTier, ExecutionPlan};
pub use scanner::{
    collect_action_spans, contains_template_markup, parse_template_tokens,
    parse_template_tokens_strict, parse_template_tokens_strict_with_options,
    parse_template_tokens_strict_with_options_and_delims, scan_template_actions,
};
pub use typedvalue::{
    decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
    decode_go_typed_slice_value, encode_go_bytes_value, encode_go_nil_bytes_value,
    encode_go_string_bytes_value, encode_go_typed_map_value, encode_go_typed_slice_value,
    go_bytes_is_nil, go_type_is_interface, go_zero_value_for_type, GoTypedMapRef, GoTypedSliceRef,
    GO_TYPE_BYTES, GO_TYPE_KEY, GO_TYPE_MAP_PREFIX, GO_TYPE_SLICE_PREFIX, GO_TYPE_STRING_BYTES,
    GO_VALUE_KEY,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoTemplateToken {
    Literal(String),
    Action(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoTemplateActionSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoTemplateScanError {
    pub code: &'static str,
    pub message: String,
    pub offset: usize,
}

// Based on Helm engine recursionMaxNums in pkg/engine/engine.go.
pub const HELM_INCLUDE_RECURSION_MAX_REFS: usize = 1000;
