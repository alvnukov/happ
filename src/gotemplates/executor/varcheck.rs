use super::{EvalState, GoTemplateScanError, NativeRenderError};
use crate::go_compat::varcheck::{
    ensure_variable_is_defined as go_ensure_variable_is_defined,
    looks_like_char_literal as go_looks_like_char_literal,
    looks_like_numeric_literal as go_looks_like_numeric_literal,
    undefined_variable_message as go_undefined_variable_message,
};

pub(super) fn looks_like_numeric_literal(expr: &str) -> bool {
    go_looks_like_numeric_literal(expr)
}

pub(super) fn looks_like_char_literal(expr: &str) -> bool {
    go_looks_like_char_literal(expr)
}

pub(super) fn ensure_variable_is_defined(
    expr: &str,
    state: &EvalState,
) -> Result<(), NativeRenderError> {
    go_ensure_variable_is_defined(expr, |name| state.lookup_var(name).is_some())
        .map_err(|err| undefined_variable_error(&err.name))
}

pub(super) fn undefined_variable_error(name: &str) -> NativeRenderError {
    NativeRenderError::Parse(GoTemplateScanError {
        code: "undefined_variable",
        message: go_undefined_variable_message(name),
        offset: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_variable_is_defined, looks_like_char_literal, looks_like_numeric_literal,
        undefined_variable_error,
    };
    use super::super::{
        EvalState, FunctionDispatchMode, LogicBackend, MissingValueMode, NativeRenderError,
    };

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
        let state = EvalState::new(
            MissingValueMode::GoDefault,
            FunctionDispatchMode::Extended,
            LogicBackend::GoCompat,
        );
        let err = ensure_variable_is_defined("$x", &state).expect_err("must fail");
        assert_eq!(err, undefined_variable_error("$x"));
        assert!(matches!(err, NativeRenderError::Parse(_)));
    }
}
