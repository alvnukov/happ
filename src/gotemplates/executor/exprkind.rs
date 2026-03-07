use crate::gotemplates::go_compat::expr::{
    decode_string_literal as go_decode_string_literal,
    is_complex_expression as go_is_complex_expression,
    is_niladic_function_expression as go_is_niladic_function_expression,
    is_quoted_string as go_is_quoted_string,
};

pub(super) fn decode_string_literal(inner: &str) -> Option<String> {
    go_decode_string_literal(inner)
}

pub(super) fn is_quoted_string(inner: &str) -> bool {
    go_is_quoted_string(inner)
}

pub(super) fn is_complex_expression(expr: &str) -> bool {
    go_is_complex_expression(expr)
}

pub(super) fn is_niladic_function_expression(expr: &str) -> bool {
    go_is_niladic_function_expression(expr)
}

#[cfg(test)]
mod tests {
    use super::{is_complex_expression, is_niladic_function_expression, is_quoted_string};

    #[test]
    fn detects_complex_expressions() {
        assert!(is_complex_expression("printf \"%s\" .v"));
        assert!(is_complex_expression(".a | quote"));
        assert!(is_complex_expression("$x := .v"));
        assert!(!is_complex_expression(".Values.a"));
        assert!(!is_complex_expression("\"x y\""));
    }

    #[test]
    fn niladic_identifier_detection() {
        assert!(is_niladic_function_expression("printf"));
        assert!(!is_niladic_function_expression("true"));
        assert!(!is_niladic_function_expression(""));
        assert!(!is_niladic_function_expression(".Values.a"));
    }

    #[test]
    fn quoted_string_detection() {
        assert!(is_quoted_string("\"x\""));
        assert!(is_quoted_string("`x`"));
        assert!(!is_quoted_string("x"));
    }
}
