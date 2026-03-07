use crate::gotemplates::go_compat::ident::is_identifier_name;

pub fn is_external_function_identifier(name: &str) -> bool {
    is_identifier_name(name)
}

pub fn is_call_builtin_identifier_candidate(name: &str) -> bool {
    is_identifier_name(name) && !matches!(name, "nil" | "true" | "false")
}

pub fn undefined_function_reason(name: &str) -> String {
    format!("\"{name}\" is not a defined function")
}

pub fn external_call_failed_reason(name: &str, reason: &str) -> String {
    format!("error calling {name}: {reason}")
}

#[cfg(test)]
mod tests {
    use super::{
        external_call_failed_reason, is_call_builtin_identifier_candidate,
        is_external_function_identifier, undefined_function_reason,
    };

    #[test]
    fn identifier_checks_match_call_and_external_needs() {
        assert!(is_external_function_identifier("tpl"));
        assert!(!is_external_function_identifier(".Values"));

        assert!(is_call_builtin_identifier_candidate("tpl"));
        assert!(!is_call_builtin_identifier_candidate("nil"));
        assert!(!is_call_builtin_identifier_candidate("true"));
        assert!(!is_call_builtin_identifier_candidate("false"));
    }

    #[test]
    fn reason_builders_match_runtime_strings() {
        assert_eq!(undefined_function_reason("tpl"), "\"tpl\" is not a defined function");
        assert_eq!(
            external_call_failed_reason("tpl", "boom"),
            "error calling tpl: boom"
        );
    }
}
