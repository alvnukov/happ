use super::typeutil::{
    format_non_comparable_type_reason, format_non_comparable_types_reason,
    is_go_bytes_slice_option, is_map_object_option, option_string_like_bytes,
};
use super::{decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_is_nil};
use super::{wrong_number_of_args, NativeRenderError};
use serde_json::Value;

const ERR_BAD_COMPARISON_TYPE: &str = "invalid type for comparison";
const ERR_BAD_COMPARISON: &str = "incompatible types for comparison";
const ERR_NO_COMPARISON: &str = "missing argument for comparison";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BasicKind {
    Bool,
    Int,
    Float,
    String,
    Uint,
}

#[derive(Debug, Clone, Copy)]
enum NumberKind {
    Int(i64),
    Uint(u64),
    Float(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompareCategory {
    Invalid,
    Slice,
    Map,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompareError {
    MissingArgument,
    InvalidType,
    IncompatibleTypes,
    Detail(String),
}

pub(super) fn builtin_eq(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "eq", "at least 1", 0));
    }
    if args.len() == 1 {
        return Err(cmp_call_error(action, "eq", CompareError::MissingArgument));
    }

    let head = &args[0];
    for other in args.iter().skip(1) {
        if eq_values(head, other).map_err(|err| cmp_call_error(action, "eq", err))? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn builtin_ne(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "ne", "2", args.len()));
    }
    let equal = eq_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "ne", err))?;
    Ok(!equal)
}

pub(super) fn builtin_lt(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "lt", "2", args.len()));
    }
    lt_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "lt", err))
}

pub(super) fn builtin_le(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "le", "2", args.len()));
    }
    le_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "le", err))
}

pub(super) fn builtin_gt(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "gt", "2", args.len()));
    }
    let less_or_equal =
        le_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "gt", err))?;
    Ok(!less_or_equal)
}

pub(super) fn builtin_ge(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "ge", "2", args.len()));
    }
    let less_than =
        lt_values(&args[0], &args[1]).map_err(|err| cmp_call_error(action, "ge", err))?;
    Ok(!less_than)
}

fn eq_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let k1 = basic_kind(a);
    let k2 = basic_kind(b);
    if k1 != k2 {
        return match (k1, k2) {
            (Some(BasicKind::Int), Some(BasicKind::Uint)) => match (number_kind(a), number_kind(b))
            {
                (Some(NumberKind::Int(x)), Some(NumberKind::Uint(y))) => Ok(x >= 0 && (x as u64) == y),
                _ => Err(CompareError::IncompatibleTypes),
            },
            (Some(BasicKind::Uint), Some(BasicKind::Int)) => match (number_kind(a), number_kind(b))
            {
                (Some(NumberKind::Uint(x)), Some(NumberKind::Int(y))) => Ok(y >= 0 && x == (y as u64)),
                _ => Err(CompareError::IncompatibleTypes),
            },
            _ => {
                if is_valid_value(a) && is_valid_value(b) {
                    Err(CompareError::IncompatibleTypes)
                } else {
                    Ok(false)
                }
            }
        };
    }

    match k1 {
        Some(BasicKind::Bool) => match (a, b) {
            (Some(Value::Bool(av)), Some(Value::Bool(bv))) => Ok(av == bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        Some(BasicKind::Int) => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Int(av)), Some(NumberKind::Int(bv))) => Ok(av == bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        Some(BasicKind::Uint) => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Uint(av)), Some(NumberKind::Uint(bv))) => Ok(av == bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        Some(BasicKind::Float) => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Float(av)), Some(NumberKind::Float(bv))) => Ok(av == bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        Some(BasicKind::String) => match (option_string_like_bytes(a), option_string_like_bytes(b)) {
            (Some(av), Some(bv)) => Ok(av.as_ref() == bv.as_ref()),
            _ => Err(CompareError::IncompatibleTypes),
        },
        None => eq_non_basic_values(a, b),
    }
}

fn eq_non_basic_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    if !can_compare(a, b) {
        return Err(CompareError::Detail(format_non_comparable_types_reason(a, b)));
    }
    if is_nil(a) || is_nil(b) {
        return Ok(is_nil(a) == is_nil(b));
    }
    if !is_value_comparable(b) {
        return Err(CompareError::Detail(format_non_comparable_type_reason(b)));
    }
    Ok(a == b)
}

fn lt_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let k1 = basic_kind(a).ok_or(CompareError::InvalidType)?;
    let k2 = basic_kind(b).ok_or(CompareError::InvalidType)?;
    if k1 != k2 {
        return match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Int(x)), Some(NumberKind::Uint(y))) => {
                Ok(x < 0 || (x as u64) < y)
            }
            (Some(NumberKind::Uint(x)), Some(NumberKind::Int(y))) => {
                Ok(y >= 0 && x < (y as u64))
            }
            _ => Err(CompareError::IncompatibleTypes),
        };
    }

    match k1 {
        BasicKind::Bool => Err(CompareError::InvalidType),
        BasicKind::Int => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Int(av)), Some(NumberKind::Int(bv))) => Ok(av < bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        BasicKind::Uint => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Uint(av)), Some(NumberKind::Uint(bv))) => Ok(av < bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        BasicKind::Float => match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Float(av)), Some(NumberKind::Float(bv))) => Ok(av < bv),
            _ => Err(CompareError::IncompatibleTypes),
        },
        BasicKind::String => match (option_string_like_bytes(a), option_string_like_bytes(b)) {
            (Some(av), Some(bv)) => Ok(av.as_ref() < bv.as_ref()),
            _ => Err(CompareError::IncompatibleTypes),
        },
    }
}

fn le_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let less_than = lt_values(a, b)?;
    if less_than {
        return Ok(true);
    }
    eq_values(a, b)
}

fn basic_kind(v: &Option<Value>) -> Option<BasicKind> {
    if option_string_like_bytes(v).is_some() {
        return Some(BasicKind::String);
    }
    match number_kind(v) {
        Some(NumberKind::Int(_)) => return Some(BasicKind::Int),
        Some(NumberKind::Uint(_)) => return Some(BasicKind::Uint),
        Some(NumberKind::Float(_)) => return Some(BasicKind::Float),
        None => {}
    }
    match v.as_ref() {
        Some(Value::Bool(_)) => Some(BasicKind::Bool),
        _ => None,
    }
}

fn number_kind(v: &Option<Value>) -> Option<NumberKind> {
    let Some(Value::Number(n)) = v.as_ref() else {
        return None;
    };
    if let Some(i) = n.as_i64() {
        return Some(NumberKind::Int(i));
    }
    if let Some(u) = n.as_u64() {
        return Some(NumberKind::Uint(u));
    }
    n.as_f64().map(NumberKind::Float)
}

fn compare_category(v: &Option<Value>) -> CompareCategory {
    match v.as_ref() {
        None | Some(Value::Null) => CompareCategory::Invalid,
        Some(Value::Array(_)) => CompareCategory::Slice,
        Some(_) if is_go_bytes_slice_option(v) => CompareCategory::Slice,
        Some(inner) if decode_go_typed_slice_value(inner).is_some() => CompareCategory::Slice,
        Some(inner) if is_map_object_option(v) || decode_go_typed_map_value(inner).is_some() => {
            CompareCategory::Map
        }
        Some(_) => CompareCategory::Other,
    }
}

fn can_compare(a: &Option<Value>, b: &Option<Value>) -> bool {
    let ka = compare_category(a);
    let kb = compare_category(b);
    if ka == kb {
        return true;
    }
    matches!(
        (ka, kb),
        (CompareCategory::Invalid, CompareCategory::Map)
            | (CompareCategory::Map, CompareCategory::Invalid)
            | (CompareCategory::Invalid, CompareCategory::Slice)
            | (CompareCategory::Slice, CompareCategory::Invalid)
    )
}

fn is_nil(v: &Option<Value>) -> bool {
    match v.as_ref() {
        None | Some(Value::Null) => true,
        Some(value) if go_bytes_is_nil(value) => true,
        Some(value) => {
            if let Some(slice) = decode_go_typed_slice_value(value) {
                return slice.items.is_none();
            }
            if let Some(map) = decode_go_typed_map_value(value) {
                return map.entries.is_none();
            }
            false
        }
    }
}

fn is_valid_value(v: &Option<Value>) -> bool {
    !matches!(v, None | Some(Value::Null))
}

fn is_value_comparable(v: &Option<Value>) -> bool {
    !matches!(
        compare_category(v),
        CompareCategory::Slice | CompareCategory::Map
    )
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
    use super::{
        builtin_eq, builtin_ge, builtin_gt, builtin_le, builtin_lt, builtin_ne,
    };
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

        let err = builtin_lt("", &[Some(Value::Bool(true)), Some(json!(1))]).expect_err("must fail");
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
