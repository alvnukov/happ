use super::path::split_variable_reference;
use super::{EvalState, GoTemplateScanError, NativeRenderError};
use crate::gotemplates::compat;

pub(super) fn looks_like_numeric_literal(expr: &str) -> bool {
    compat::looks_like_numeric_literal(expr)
}

pub(super) fn looks_like_char_literal(expr: &str) -> bool {
    compat::looks_like_char_literal(expr)
}

pub(super) fn ensure_variable_is_defined(
    expr: &str,
    state: &EvalState,
) -> Result<(), NativeRenderError> {
    if let Some((name, _)) = split_variable_reference(expr) {
        if name != "$" && state.lookup_var(name).is_none() {
            return Err(undefined_variable_error(name));
        }
    }
    Ok(())
}

pub(super) fn undefined_variable_error(name: &str) -> NativeRenderError {
    NativeRenderError::Parse(GoTemplateScanError {
        code: "undefined_variable",
        message: format!("undefined variable \"{name}\""),
        offset: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_variable_is_defined, looks_like_char_literal, looks_like_numeric_literal,
        undefined_variable_error,
    };
    use super::super::{EvalState, FunctionDispatchMode, MissingValueMode, NativeRenderError};

    #[test]
    fn literal_shape_helpers_match_expected_inputs() {
        assert!(looks_like_numeric_literal("12"));
        assert!(looks_like_numeric_literal("-1.5"));
        assert!(!looks_like_numeric_literal("x12"));

        assert!(looks_like_char_literal("'x'"));
        assert!(!looks_like_char_literal("x"));
    }

    #[test]
    fn variable_guard_reports_undefined_variable_name() {
        let state = EvalState::new(MissingValueMode::GoDefault, FunctionDispatchMode::Extended);
        let err = ensure_variable_is_defined("$x", &state).expect_err("must fail");
        assert_eq!(err, undefined_variable_error("$x"));
        assert!(matches!(err, NativeRenderError::Parse(_)));
    }
}
