pub mod compat;
mod executor;
mod functions;
mod parser;
mod planner;
mod scanner;
mod typedvalue;

pub use executor::{
    render_template_native, render_template_native_with_options, MissingValueMode,
    render_template_native_with_resolver, NativeFunctionResolver, NativeFunctionResolverError,
    NativeRenderError, NativeRenderOptions,
};
pub use functions::{
    collect_function_calls_in_action, collect_function_calls_in_template, escape_template_action,
    normalize_values_global_context,
};
pub use parser::ParseCompatOptions;
pub use planner::{plan_template_execution, CompatibilityReason, CompatibilityTier, ExecutionPlan};
pub use scanner::{
    collect_action_spans, contains_template_markup, parse_template_tokens,
    parse_template_tokens_strict, parse_template_tokens_strict_with_options, scan_template_actions,
};
pub use typedvalue::{decode_go_bytes_value, encode_go_bytes_value, GO_TYPE_BYTES, GO_TYPE_KEY, GO_VALUE_KEY};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoTemplateScanError {
    pub code: &'static str,
    pub message: &'static str,
    pub offset: usize,
}

// Based on Helm engine recursionMaxNums in pkg/engine/engine.go.
pub const HELM_INCLUDE_RECURSION_MAX_REFS: usize = 1000;
