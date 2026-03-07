use crate::go_compat::compat;
use crate::go_compat::ident::is_identifier_continue_char;
use crate::go_compat::path::split_variable_reference;
use crate::go_compat::tokenize::strip_outer_parens;
use crate::gotemplates::typedvalue::{
    decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_len, go_string_bytes_len,
};
use serde_json::Value;

pub fn is_non_executable_pipeline_head(token: &str) -> bool {
    let trimmed = token.trim();
    if trimmed.is_empty() || strip_outer_parens(trimmed).is_some() {
        return false;
    }
    if matches!(trimmed, "." | "nil" | "true" | "false") {
        return true;
    }
    if is_quoted_string(trimmed) || compat::looks_like_numeric_literal(trimmed) {
        return true;
    }
    compat::looks_like_char_literal(trimmed)
}

pub fn non_function_command_target(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(inner) = strip_outer_parens(trimmed) {
        return Some(inner.trim().to_string());
    }
    if matches!(trimmed, "." | "nil" | "true" | "false") {
        return Some(trimmed.to_string());
    }
    if is_quoted_string(trimmed)
        || compat::looks_like_numeric_literal(trimmed)
        || compat::looks_like_char_literal(trimmed)
    {
        return Some(trimmed.to_string());
    }
    if trimmed.starts_with('$') {
        if let Some((_, rest)) = split_variable_reference(trimmed) {
            if rest.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLikeCommandPath {
    pub receiver_expr: String,
    pub field_name: String,
}

pub fn command_field_like_path(token: &str) -> Option<FieldLikeCommandPath> {
    let trimmed = token.trim();
    if trimmed.is_empty() || strip_outer_parens(trimmed).is_some() {
        return None;
    }

    let (base, path) = if let Some(rest) = trimmed.strip_prefix("$.") {
        (FieldPathBase::Root, rest)
    } else if let Some(rest) = trimmed.strip_prefix('.') {
        (FieldPathBase::Dot, rest)
    } else if let Some((name, rest)) = split_variable_reference(trimmed) {
        (FieldPathBase::Var(name.to_string()), rest)
    } else {
        return None;
    };

    if path.is_empty() {
        return None;
    }

    let mut last_dot = None;
    let mut prev_was_dot = false;
    for (idx, ch) in path.char_indices() {
        if ch == '.' {
            if idx == 0 || idx + 1 == path.len() || prev_was_dot {
                return None;
            }
            last_dot = Some(idx);
            prev_was_dot = true;
            continue;
        }
        prev_was_dot = false;
        if !is_field_path_segment_char(ch) {
            return None;
        }
    }

    let (receiver_tail, field_name) = if let Some(dot_idx) = last_dot {
        (&path[..dot_idx], &path[dot_idx + 1..])
    } else {
        ("", path)
    };
    if field_name.is_empty() {
        return None;
    }

    let receiver_expr = build_field_receiver_expr(base, receiver_tail);
    Some(FieldLikeCommandPath {
        receiver_expr,
        field_name: field_name.to_string(),
    })
}

pub fn is_map_like_for_field_call(v: &Value) -> bool {
    if go_bytes_len(v).is_some()
        || go_string_bytes_len(v).is_some()
        || decode_go_typed_slice_value(v).is_some()
    {
        return false;
    }
    decode_go_typed_map_value(v).is_some() || matches!(v, Value::Object(_))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldPathBase {
    Dot,
    Root,
    Var(String),
}

fn build_field_receiver_expr(base: FieldPathBase, receiver_tail: &str) -> String {
    if receiver_tail.is_empty() {
        return match base {
            FieldPathBase::Dot => ".".to_string(),
            FieldPathBase::Root => "$".to_string(),
            FieldPathBase::Var(name) => name,
        };
    }
    match base {
        FieldPathBase::Dot => format!(".{receiver_tail}"),
        FieldPathBase::Root => format!("$.{receiver_tail}"),
        FieldPathBase::Var(name) => format!("{name}.{receiver_tail}"),
    }
}

fn is_field_path_segment_char(ch: char) -> bool {
    is_identifier_continue_char(ch)
}

fn is_quoted_string(expr: &str) -> bool {
    expr.len() >= 2
        && ((expr.starts_with('"') && expr.ends_with('"'))
            || (expr.starts_with('`') && expr.ends_with('`')))
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
