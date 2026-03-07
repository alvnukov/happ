use crate::go_compat::commandkind::{
    command_field_like_path as go_command_field_like_path,
    is_map_like_for_field_call as go_is_map_like_for_field_call,
    is_non_executable_pipeline_head as go_is_non_executable_pipeline_head,
    non_function_command_target as go_non_function_command_target,
    FieldLikeCommandPath as GoFieldLikeCommandPath,
};
use serde_json::Value;

pub(super) type FieldLikeCommandPath = GoFieldLikeCommandPath;

pub(super) fn is_non_executable_pipeline_head(token: &str) -> bool {
    go_is_non_executable_pipeline_head(token)
}

pub(super) fn non_function_command_target(token: &str) -> Option<String> {
    go_non_function_command_target(token)
}

pub(super) fn command_field_like_path(token: &str) -> Option<FieldLikeCommandPath> {
    go_command_field_like_path(token)
}

pub(super) fn is_map_like_for_field_call(v: &Value) -> bool {
    go_is_map_like_for_field_call(v)
}

#[cfg(test)]
mod tests {
    use super::{
        command_field_like_path, is_map_like_for_field_call, is_non_executable_pipeline_head,
        non_function_command_target,
    };
    use serde_json::json;

    #[test]
    fn detects_non_executable_pipeline_heads() {
        assert!(is_non_executable_pipeline_head("nil"));
        assert!(is_non_executable_pipeline_head("\"x\""));
        assert!(is_non_executable_pipeline_head("12"));
        assert!(!is_non_executable_pipeline_head("printf"));
        assert!(!is_non_executable_pipeline_head("(nil)"));
    }

    #[test]
    fn extracts_non_function_targets() {
        assert_eq!(
            non_function_command_target("(\"x\")").as_deref(),
            Some("\"x\"")
        );
        assert_eq!(non_function_command_target("$v").as_deref(), Some("$v"));
        assert!(non_function_command_target("$v.x").is_none());
    }

    #[test]
    fn parses_field_like_paths() {
        let p = command_field_like_path(".a.b").expect("path");
        assert_eq!(p.receiver_expr, ".a");
        assert_eq!(p.field_name, "b");

        let p = command_field_like_path("$.a").expect("path");
        assert_eq!(p.receiver_expr, "$");
        assert_eq!(p.field_name, "a");

        let p = command_field_like_path("$v.a").expect("path");
        assert_eq!(p.receiver_expr, "$v");
        assert_eq!(p.field_name, "a");
    }

    #[test]
    fn rejects_invalid_field_like_paths() {
        for token in [".a..b", ".a.", "$.a..b", "$v.a..b", "$v.", ".a./b", ".a-b"] {
            assert!(command_field_like_path(token).is_none(), "token={token}");
        }
    }

    #[test]
    fn map_like_for_field_call_excludes_string_and_slice_like_values() {
        assert!(is_map_like_for_field_call(&json!({"k":"v"})));
        assert!(!is_map_like_for_field_call(&json!([1, 2])));
        assert!(!is_map_like_for_field_call(&json!("abc")));
    }
}
