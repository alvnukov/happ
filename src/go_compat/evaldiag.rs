pub fn is_nil_command(expr: &str) -> bool {
    expr.trim() == "nil"
}

pub fn nil_is_not_a_command_reason() -> String {
    "nil is not a command".to_string()
}

pub fn empty_pipeline_reason() -> String {
    "empty pipeline".to_string()
}

pub fn empty_command_in_pipeline_reason() -> String {
    "empty command in pipeline".to_string()
}

pub fn multi_variable_decl_in_non_range_reason() -> String {
    "multi-variable declarations are only supported in range pipelines".to_string()
}

pub fn non_executable_pipeline_stage_reason(stage: usize) -> String {
    format!("non executable command in pipeline stage {stage}")
}

pub fn cannot_give_argument_to_non_function_reason(target: &str) -> String {
    format!("can't give argument to non-function {target}")
}

pub fn field_not_method_has_arguments_reason(field_name: &str) -> String {
    format!("{field_name} is not a method but has arguments")
}

pub fn invalid_syntax_reason(token: &str) -> String {
    format!("invalid syntax: {token}")
}

pub fn illegal_number_syntax_reason(token: &str) -> String {
    format!("illegal number syntax: {token}")
}

#[cfg(test)]
mod tests {
    use super::{
        cannot_give_argument_to_non_function_reason, empty_command_in_pipeline_reason,
        empty_pipeline_reason, field_not_method_has_arguments_reason, illegal_number_syntax_reason,
        invalid_syntax_reason, is_nil_command, multi_variable_decl_in_non_range_reason,
        nil_is_not_a_command_reason, non_executable_pipeline_stage_reason,
    };

    #[test]
    fn nil_command_detection_is_trim_aware() {
        assert!(is_nil_command("nil"));
        assert!(is_nil_command(" nil "));
        assert!(!is_nil_command("nilx"));
    }

    #[test]
    fn reason_builders_match_runtime_contracts() {
        assert_eq!(nil_is_not_a_command_reason(), "nil is not a command");
        assert_eq!(empty_pipeline_reason(), "empty pipeline");
        assert_eq!(empty_command_in_pipeline_reason(), "empty command in pipeline");
        assert_eq!(
            multi_variable_decl_in_non_range_reason(),
            "multi-variable declarations are only supported in range pipelines"
        );
        assert_eq!(
            non_executable_pipeline_stage_reason(2),
            "non executable command in pipeline stage 2"
        );
        assert_eq!(
            cannot_give_argument_to_non_function_reason("nil"),
            "can't give argument to non-function nil"
        );
        assert_eq!(
            field_not_method_has_arguments_reason("Name"),
            "Name is not a method but has arguments"
        );
        assert_eq!(invalid_syntax_reason("'"), "invalid syntax: '");
        assert_eq!(
            illegal_number_syntax_reason("1_"),
            "illegal number syntax: 1_"
        );
    }
}
