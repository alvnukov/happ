// Backward-compatible facade over canonical go_compat functions module.
pub use crate::go_compat::functions::{
    collect_function_calls_in_action, collect_function_calls_in_template, escape_template_action,
    normalize_values_global_context,
};
