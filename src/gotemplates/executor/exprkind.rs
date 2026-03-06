use super::is_identifier_name;
use crate::gotemplates::compat;

pub(super) fn decode_string_literal(inner: &str) -> Option<String> {
    compat::decode_go_string_literal(inner)
}

pub(super) fn is_quoted_string(inner: &str) -> bool {
    inner.len() >= 2
        && ((inner.starts_with('"') && inner.ends_with('"'))
            || (inner.starts_with('`') && inner.ends_with('`')))
}

pub(super) fn is_complex_expression(expr: &str) -> bool {
    if expr.is_empty() {
        return false;
    }
    if is_quoted_string(expr) {
        return false;
    }
    if expr.contains('|')
        || expr.contains('(')
        || expr.contains(')')
        || expr.contains(":=")
        || expr.contains(',')
    {
        return true;
    }
    if expr.contains('=') && !expr.starts_with('=') {
        return true;
    }
    if expr.contains(char::is_whitespace) {
        return true;
    }
    false
}

pub(super) fn is_niladic_function_expression(expr: &str) -> bool {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return false;
    }
    if matches!(trimmed, "true" | "false" | "nil") {
        return false;
    }
    is_identifier_name(trimmed)
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
