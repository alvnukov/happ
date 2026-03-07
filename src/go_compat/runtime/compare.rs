use super::{wrong_number_of_args, NativeRenderError};
use crate::go_compat::compare::{
    eq_values as go_eq_values, le_values as go_le_values, lt_values as go_lt_values, CompareError,
};
use serde_json::Value;

const ERR_BAD_COMPARISON_TYPE: &str = "invalid type for comparison";
const ERR_BAD_COMPARISON: &str = "incompatible types for comparison";
const ERR_NO_COMPARISON: &str = "missing argument for comparison";

pub(super) fn builtin_eq(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "eq", "at least 1", 0));
    }
    if args.len() == 1 {
        return Err(cmp_call_error(action, "eq", CompareError::MissingArgument));
    }

    let head = &args[0];
    for other in args.iter().skip(1) {
        if go_eq_values(head, other).map_err(|err| cmp_call_error(action, "eq", err))? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn builtin_ne(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "ne", "2", args.len()));
    }
    let equal =
        go_eq_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "ne", err))?;
    Ok(!equal)
}

pub(super) fn builtin_lt(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "lt", "2", args.len()));
    }
    go_lt_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "lt", err))
}

pub(super) fn builtin_le(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "le", "2", args.len()));
    }
    go_le_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "le", err))
}

pub(super) fn builtin_gt(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "gt", "2", args.len()));
    }
    let less_or_equal =
        go_le_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "gt", err))?;
    Ok(!less_or_equal)
}

pub(super) fn builtin_ge(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "ge", "2", args.len()));
    }
    let less_than =
        go_lt_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "ge", err))?;
    Ok(!less_than)
}

fn cmp_call_error(action: &str, fn_name: &str, err: CompareError) -> NativeRenderError {
    let reason = match err {
        CompareError::MissingArgument => ERR_NO_COMPARISON.to_string(),
        CompareError::InvalidType => ERR_BAD_COMPARISON_TYPE.to_string(),
        CompareError::IncompatibleTypes => ERR_BAD_COMPARISON.to_string(),
        CompareError::Detail(msg) => msg,
    };
    NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: format!("error calling {fn_name}: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_eq, builtin_ge, builtin_gt, builtin_le, builtin_lt, builtin_ne};
    use crate::gotemplates::{
        encode_go_bytes_value, encode_go_nil_bytes_value, encode_go_typed_map_value,
        encode_go_typed_slice_value, NativeRenderError,
    };
    use serde_json::{json, Map, Value};

    fn reason(err: NativeRenderError) -> String {
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => reason,
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn eq_matches_go_for_nil_maps_and_slices() {
        let nil_bytes = Some(encode_go_nil_bytes_value());
        let non_nil_bytes = Some(encode_go_bytes_value(b"x"));

        assert_eq!(
            builtin_eq("", &[nil_bytes.clone(), nil_bytes.clone()]).expect("eq must succeed"),
            true
        );
        assert_eq!(
            builtin_eq("", &[nil_bytes.clone(), non_nil_bytes.clone()]).expect("eq must succeed"),
            false
        );

        let nil_map = Some(encode_go_typed_map_value("int", None));
        let mut entries = Map::new();
        entries.insert("a".to_string(), json!(1));
        let map = Some(encode_go_typed_map_value("int", Some(entries)));

        assert_eq!(
            builtin_eq("", &[nil_map.clone(), nil_map.clone()]).expect("eq must succeed"),
            true
        );
        assert_eq!(
            builtin_eq("", &[nil_map, map]).expect("eq must succeed"),
            false
        );
    }

    #[test]
    fn eq_with_nil_and_typed_nil_value_is_true_like_go() {
        let nil_map = Some(encode_go_typed_map_value("string", None));
        assert_eq!(
            builtin_eq("", &[None, nil_map]).expect("eq must succeed"),
            true
        );
    }

    #[test]
    fn eq_reports_non_comparable_type_for_non_nil_slice() {
        let bytes = Some(encode_go_bytes_value(b"ab"));
        let err = builtin_eq("", &[bytes.clone(), bytes]).expect_err("must fail");
        let reason = reason(err);
        assert!(reason.contains("error calling eq: non-comparable type"));
        assert!(reason.contains("[]uint8"));
    }

    #[test]
    fn ne_reports_ne_prefix_not_eq_prefix() {
        let bytes = Some(encode_go_bytes_value(b"ab"));
        let err = builtin_ne("", &[bytes.clone(), bytes]).expect_err("must fail");
        let reason = reason(err);
        assert!(reason.contains("error calling ne: non-comparable type"));
        assert!(!reason.contains("error calling eq:"));
    }

    #[test]
    fn lt_and_related_ops_follow_go_signed_unsigned_rules() {
        assert_eq!(
            builtin_lt("", &[Some(json!(-1)), Some(json!(1u64))]).expect("lt must succeed"),
            true
        );
        assert_eq!(
            builtin_lt("", &[Some(json!(1u64)), Some(json!(-1))]).expect("lt must succeed"),
            false
        );
        assert_eq!(
            builtin_le("", &[Some(json!(1)), Some(json!(1))]).expect("le must succeed"),
            true
        );
        assert_eq!(
            builtin_gt("", &[Some(json!(2)), Some(json!(1))]).expect("gt must succeed"),
            true
        );
        assert_eq!(
            builtin_ge("", &[Some(json!(2)), Some(json!(2))]).expect("ge must succeed"),
            true
        );
    }

    #[test]
    fn lt_reports_invalid_and_incompatible_errors_like_go() {
        let err = builtin_lt("", &[Some(Value::Bool(true)), Some(Value::Bool(false))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling lt: invalid type for comparison"));

        let err =
            builtin_lt("", &[Some(Value::Bool(true)), Some(json!(1))]).expect_err("must fail");
        assert!(reason(err).contains("error calling lt: incompatible types for comparison"));
    }

    #[test]
    fn eq_reports_missing_argument_like_go() {
        let err = builtin_eq("", &[Some(json!(1))]).expect_err("must fail");
        assert!(reason(err).contains("error calling eq: missing argument for comparison"));
    }

    #[test]
    fn eq_handles_typed_nil_slice() {
        let nil_slice = Some(encode_go_typed_slice_value("int", None));
        assert_eq!(
            builtin_eq("", &[nil_slice.clone(), nil_slice]).expect("eq must succeed"),
            true
        );
    }
}
