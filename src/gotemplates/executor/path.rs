use super::{MissingValueMode, NativeRenderError};
use crate::gotemplates::go_compat::path::{
    resolve_simple_path as go_resolve_simple_path,
    split_variable_reference as go_split_variable_reference, PathMissingValueMode,
    ResolveSimplePathError,
};
use serde_json::Value;

pub(super) fn resolve_simple_path(
    root: &Value,
    dot: &Value,
    expr: &str,
    missing_value_mode: MissingValueMode,
    lookup_var: impl Fn(&str) -> Option<Option<Value>>,
) -> Result<Option<Value>, NativeRenderError> {
    let mode = match missing_value_mode {
        MissingValueMode::GoZero => PathMissingValueMode::GoZero,
        MissingValueMode::GoDefault | MissingValueMode::Error => PathMissingValueMode::GoDefault,
    };
    go_resolve_simple_path(root, dot, expr, mode, lookup_var).map_err(|err| match err {
        ResolveSimplePathError::CannotEvaluateField { segment, type_name } => {
            NativeRenderError::UnsupportedAction {
                action: format!("{{{{{expr}}}}}"),
                reason: format!("can't evaluate field {segment} in type {type_name}"),
            }
        }
        ResolveSimplePathError::NilPointerEvaluatingInterface { segment } => {
            NativeRenderError::UnsupportedAction {
                action: format!("{{{{{expr}}}}}"),
                reason: format!("nil pointer evaluating interface {{}}.{segment}"),
            }
        }
    })
}

pub(super) fn split_variable_reference(expr: &str) -> Option<(&str, &str)> {
    go_split_variable_reference(expr)
}

#[cfg(test)]
mod tests {
    use super::{resolve_simple_path, split_variable_reference, MissingValueMode};
    use crate::gotemplates::NativeRenderError;
    use serde_json::json;

    #[test]
    fn split_variable_reference_supports_go_style_scope_tokens() {
        assert_eq!(split_variable_reference("$"), Some(("$", "")));
        assert_eq!(split_variable_reference("$x"), Some(("$x", "")));
        assert_eq!(split_variable_reference("$x.y.z"), Some(("$x", "y.z")));
        assert_eq!(split_variable_reference("$1"), Some(("$1", "")));
        assert_eq!(split_variable_reference("$.x"), None);
    }

    #[test]
    fn resolve_simple_path_handles_dot_root_and_vars() {
        let root = json!({"v":{"k":"x"}});
        let dot = json!({"a":1});
        let val =
            resolve_simple_path(&root, &dot, ".", MissingValueMode::GoDefault, |_| None).expect("ok");
        assert_eq!(val, Some(dot.clone()));

        let val =
            resolve_simple_path(&root, &dot, "$.v.k", MissingValueMode::GoDefault, |_| None)
                .expect("ok");
        assert_eq!(val, Some(json!("x")));

        let val = resolve_simple_path(&root, &dot, "$x.k", MissingValueMode::GoDefault, |name| {
            if name == "$x" {
                Some(Some(json!({"k":"v"})))
            } else {
                None
            }
        })
        .expect("ok");
        assert_eq!(val, Some(json!("v")));
    }

    #[test]
    fn resolve_simple_path_reports_slice_field_errors_like_go() {
        let root = json!({"arr":[1,2]});
        let err = resolve_simple_path(&root, &root, ".arr.x", MissingValueMode::GoDefault, |_| None)
            .expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("can't evaluate field x in type []interface {}"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn resolve_simple_path_rejects_non_identifier_segments() {
        let root = json!({"a-b": 1});
        let out =
            resolve_simple_path(&root, &root, ".a-b", MissingValueMode::GoDefault, |_| None)
                .expect("must evaluate");
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_simple_path_gozero_keeps_nil_pointer_interface_error() {
        let root = json!({"m":{}});
        let err =
            resolve_simple_path(&root, &root, ".m.missing.y", MissingValueMode::GoZero, |_| None)
                .expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("nil pointer evaluating interface {}.y"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
