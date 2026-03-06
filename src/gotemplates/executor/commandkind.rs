use super::path::{is_identifier_continue_char, split_variable_reference};
use super::tokenize::strip_outer_parens;
use crate::gotemplates::compat;

pub(super) fn is_non_executable_pipeline_head(token: &str) -> bool {
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

pub(super) fn non_function_command_target(token: &str) -> Option<String> {
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
pub(super) struct FieldLikeCommandPath {
    pub(super) receiver_expr: String,
    pub(super) field_name: String,
}

pub(super) fn command_field_like_path(token: &str) -> Option<FieldLikeCommandPath> {
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

    let mut segments = Vec::new();
    for segment in path.split('.') {
        if segment.is_empty() || !segment.chars().all(is_field_path_segment_char) {
            return None;
        }
        segments.push(segment.to_string());
    }
    if segments.is_empty() {
        return None;
    }

    let field_name = segments.last()?.clone();
    let receiver_expr = build_field_receiver_expr(base, &segments)?;
    Some(FieldLikeCommandPath {
        receiver_expr,
        field_name,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldPathBase {
    Dot,
    Root,
    Var(String),
}

fn build_field_receiver_expr(base: FieldPathBase, segments: &[String]) -> Option<String> {
    if segments.is_empty() {
        return None;
    }
    if segments.len() == 1 {
        return Some(match base {
            FieldPathBase::Dot => ".".to_string(),
            FieldPathBase::Root => "$".to_string(),
            FieldPathBase::Var(name) => name,
        });
    }
    let prefix = match base {
        FieldPathBase::Dot => ".".to_string(),
        FieldPathBase::Root => "$.".to_string(),
        FieldPathBase::Var(name) => format!("{name}."),
    };
    let tail = segments[..segments.len() - 1].join(".");
    Some(format!("{prefix}{tail}"))
}

fn is_field_path_segment_char(ch: char) -> bool {
    is_identifier_continue_char(ch) || ch == '-'
}

fn is_quoted_string(expr: &str) -> bool {
    expr.len() >= 2
        && ((expr.starts_with('"') && expr.ends_with('"'))
            || (expr.starts_with('`') && expr.ends_with('`')))
}

#[cfg(test)]
mod tests {
    use super::{
        command_field_like_path, is_non_executable_pipeline_head, non_function_command_target,
    };

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
        assert_eq!(
            non_function_command_target("$v").as_deref(),
            Some("$v")
        );
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
}
