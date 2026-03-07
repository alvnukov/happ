use crate::gotemplates::compat;
use crate::gotemplates::go_compat::path::split_variable_reference;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndefinedVariableError {
    pub name: String,
}

pub fn looks_like_numeric_literal(expr: &str) -> bool {
    compat::looks_like_numeric_literal(expr)
}

pub fn looks_like_char_literal(expr: &str) -> bool {
    compat::looks_like_char_literal(expr)
}

pub fn ensure_variable_is_defined(
    expr: &str,
    lookup_var: impl Fn(&str) -> bool,
) -> Result<(), UndefinedVariableError> {
    if let Some((name, _)) = split_variable_reference(expr) {
        if name != "$" && !lookup_var(name) {
            return Err(UndefinedVariableError {
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

pub fn undefined_variable_message(name: &str) -> String {
    format!("undefined variable \"{name}\"")
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_variable_is_defined, looks_like_char_literal, looks_like_numeric_literal,
        undefined_variable_message, UndefinedVariableError,
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
        let err = ensure_variable_is_defined("$x", |_| false).expect_err("must fail");
        assert_eq!(
            err,
            UndefinedVariableError {
                name: "$x".to_string(),
            }
        );
        assert_eq!(undefined_variable_message(&err.name), "undefined variable \"$x\"");
    }
}
