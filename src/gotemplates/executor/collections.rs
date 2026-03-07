use crate::gotemplates::go_compat::collections::{
    builtin_index as go_builtin_index, builtin_len as go_builtin_len,
    builtin_slice as go_builtin_slice,
};
use super::NativeRenderError;
use serde_json::Value;

pub(super) fn builtin_len(action: &str, args: &[Option<Value>]) -> Result<usize, NativeRenderError> {
    go_builtin_len(args).map_err(|err| NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: err.reason,
    })
}

pub(super) fn builtin_index(
    action: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeRenderError> {
    go_builtin_index(args).map_err(|err| NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: err.reason,
    })
}

pub(super) fn builtin_slice(
    action: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeRenderError> {
    go_builtin_slice(args).map_err(|err| NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: err.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::{builtin_index, builtin_len, builtin_slice};
    use crate::gotemplates::NativeRenderError;
    use serde_json::{json, Map, Number, Value};

    fn reason(err: NativeRenderError) -> String {
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => reason,
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn index_boundary_equals_len_matches_go_reflect_errors() {
        let err = builtin_index("", &[Some(json!([1, 2])), Some(json!(2))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: reflect: slice index out of range"));

        let err = builtin_index("", &[Some(json!("ab")), Some(json!(2))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: reflect: string index out of range"));
    }

    #[test]
    fn index_above_len_keeps_index_out_of_range_message() {
        let err = builtin_index("", &[Some(json!([1, 2])), Some(json!(3))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: index out of range: 3"));
    }

    #[test]
    fn len_matches_go_for_supported_and_unsupported_values() {
        assert_eq!(builtin_len("", &[Some(json!([1, 2, 3]))]).expect("len"), 3);
        assert_eq!(builtin_len("", &[Some(json!("abc"))]).expect("len"), 3);

        let err = builtin_len("", &[None]).expect_err("must fail");
        assert!(reason(err).contains("error calling len: len of nil pointer"));

        let err = builtin_len("", &[Some(json!(3))]).expect_err("must fail");
        assert!(reason(err).contains("error calling len: len of type int"));
    }

    #[test]
    fn index_map_missing_key_returns_zero_value_for_typed_maps() {
        let mut entries = Map::new();
        entries.insert("a".to_string(), Value::Number(Number::from(7)));
        let typed = crate::gotemplates::encode_go_typed_map_value("int", Some(entries));
        let out = builtin_index("", &[Some(typed), Some(json!("missing"))]).expect("index");
        assert_eq!(out, Some(Value::Number(Number::from(0))));
    }

    #[test]
    fn slice_respects_string_and_index_rules() {
        let out =
            builtin_slice("", &[Some(json!("abcd")), Some(json!(1)), Some(json!(3))]).expect("slice");
        assert_eq!(out, Some(json!("bc")));

        let err = builtin_slice(
            "",
            &[Some(json!("abcd")), Some(json!(1)), Some(json!(2)), Some(json!(2))],
        )
        .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: cannot 3-index slice a string"));
    }

    #[test]
    fn slice_validates_bounds_like_go() {
        let err = builtin_slice("", &[Some(json!([1, 2, 3])), Some(json!(2)), Some(json!(1))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: invalid slice index: 2 > 1"));

        let err = builtin_slice("", &[Some(json!([1, 2, 3])), Some(json!(4)), Some(json!(5))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: index out of range: 4"));
    }

    #[test]
    fn index_chain_after_missing_map_reports_nil_pointer_like_go() {
        let root = Some(json!({"a":{"x":1}}));
        let err = builtin_index("", &[root, Some(json!("missing")), Some(json!("x"))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of nil pointer"));
    }

    #[test]
    fn index_root_untyped_nil_still_reports_untyped_nil() {
        let err = builtin_index("", &[Some(Value::Null), Some(json!(1))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of untyped nil"));
    }

    #[test]
    fn index_chain_after_typed_interface_zero_reports_nil_pointer() {
        let mut entries = Map::new();
        entries.insert("a".to_string(), json!({"x":1}));
        let typed = crate::gotemplates::encode_go_typed_map_value("interface {}", Some(entries));
        let err = builtin_index("", &[Some(typed), Some(json!("missing")), Some(json!("x"))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of nil pointer"));
    }
}
