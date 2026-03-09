use crate::go_compat::typedvalue::{
    decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_is_nil,
};
use crate::go_compat::typeutil::{
    is_go_bytes_slice, is_map_object, option_string_like_bytes, option_type_name_for_template,
};
use crate::go_compat::valuefmt::format_value_like_go;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareError {
    MissingArgument,
    InvalidType,
    IncompatibleTypes,
    Detail(String),
}

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

pub fn eq_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let k1 = basic_kind(a);
    let k2 = basic_kind(b);
    if k1 != k2 {
        return match (k1, k2) {
            (Some(BasicKind::Int), Some(BasicKind::Uint)) => match (number_kind(a), number_kind(b))
            {
                (Some(NumberKind::Int(x)), Some(NumberKind::Uint(y))) => {
                    Ok(x >= 0 && (x as u64) == y)
                }
                _ => Err(CompareError::IncompatibleTypes),
            },
            (Some(BasicKind::Uint), Some(BasicKind::Int)) => match (number_kind(a), number_kind(b))
            {
                (Some(NumberKind::Uint(x)), Some(NumberKind::Int(y))) => {
                    Ok(y >= 0 && x == (y as u64))
                }
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
        Some(BasicKind::String) => match (
            option_string_like_bytes(a.as_ref()),
            option_string_like_bytes(b.as_ref()),
        ) {
            (Some(av), Some(bv)) => Ok(av.as_ref() == bv.as_ref()),
            _ => Err(CompareError::IncompatibleTypes),
        },
        None => eq_non_basic_values(a, b),
    }
}

pub fn lt_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let k1 = basic_kind(a).ok_or(CompareError::InvalidType)?;
    let k2 = basic_kind(b).ok_or(CompareError::InvalidType)?;
    if k1 != k2 {
        return match (number_kind(a), number_kind(b)) {
            (Some(NumberKind::Int(x)), Some(NumberKind::Uint(y))) => Ok(x < 0 || (x as u64) < y),
            (Some(NumberKind::Uint(x)), Some(NumberKind::Int(y))) => Ok(y >= 0 && x < (y as u64)),
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
        BasicKind::String => match (
            option_string_like_bytes(a.as_ref()),
            option_string_like_bytes(b.as_ref()),
        ) {
            (Some(av), Some(bv)) => Ok(av.as_ref() < bv.as_ref()),
            _ => Err(CompareError::IncompatibleTypes),
        },
    }
}

pub fn le_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    let less_than = lt_values(a, b)?;
    if less_than {
        return Ok(true);
    }
    eq_values(a, b)
}

fn basic_kind(v: &Option<Value>) -> Option<BasicKind> {
    if option_string_like_bytes(v.as_ref()).is_some() {
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

fn eq_non_basic_values(a: &Option<Value>, b: &Option<Value>) -> Result<bool, CompareError> {
    if !can_compare(a, b) {
        return Err(CompareError::Detail(format_non_comparable_types_reason(
            a, b,
        )));
    }
    if is_nil(a) || is_nil(b) {
        return Ok(is_nil(a) == is_nil(b));
    }
    if !is_value_comparable(b) {
        return Err(CompareError::Detail(format_non_comparable_type_reason(b)));
    }
    Ok(a == b)
}

fn compare_category(v: &Option<Value>) -> CompareCategory {
    match v.as_ref() {
        None | Some(Value::Null) => CompareCategory::Invalid,
        Some(Value::Array(_)) => CompareCategory::Slice,
        Some(inner) if is_go_bytes_slice(inner) => CompareCategory::Slice,
        Some(inner) if decode_go_typed_slice_value(inner).is_some() => CompareCategory::Slice,
        Some(inner) if is_map_object(inner) || decode_go_typed_map_value(inner).is_some() => {
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

fn format_value_for_print(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => format_value_like_go(other),
    }
}

fn format_non_comparable_type_reason(v: &Option<Value>) -> String {
    format!(
        "non-comparable type {}: {}",
        format_value_for_print(v),
        option_type_name_for_template(v.as_ref())
    )
}

fn format_non_comparable_types_reason(a: &Option<Value>, b: &Option<Value>) -> String {
    format!(
        "non-comparable types {}: {}, {}: {}",
        format_value_for_print(a),
        option_type_name_for_template(a.as_ref()),
        option_type_name_for_template(b.as_ref()),
        format_value_for_print(b)
    )
}

#[cfg(test)]
mod tests {
    use super::{eq_values, le_values, lt_values, CompareError};
    use crate::go_compat::typedvalue::{
        encode_go_bytes_value, encode_go_nil_bytes_value, encode_go_typed_map_value,
        encode_go_typed_slice_value,
    };
    use serde_json::{json, Map, Value};

    #[test]
    fn eq_matches_go_for_nil_maps_and_slices() {
        let nil_bytes = Some(encode_go_nil_bytes_value());
        let non_nil_bytes = Some(encode_go_bytes_value(b"x"));

        assert_eq!(eq_values(&nil_bytes, &nil_bytes).expect("eq"), true);
        assert_eq!(eq_values(&nil_bytes, &non_nil_bytes).expect("eq"), false);

        let nil_map = Some(encode_go_typed_map_value("int", None));
        let mut entries = Map::new();
        entries.insert("a".to_string(), json!(1));
        let map = Some(encode_go_typed_map_value("int", Some(entries)));

        assert_eq!(eq_values(&nil_map, &nil_map).expect("eq"), true);
        assert_eq!(eq_values(&nil_map, &map).expect("eq"), false);
    }

    #[test]
    fn eq_with_nil_and_typed_nil_value_is_true_like_go() {
        let nil_map = Some(encode_go_typed_map_value("string", None));
        assert_eq!(eq_values(&None, &nil_map).expect("eq"), true);
    }

    #[test]
    fn eq_reports_non_comparable_type_for_non_nil_slice() {
        let bytes = Some(encode_go_bytes_value(b"ab"));
        let err = eq_values(&bytes, &bytes).expect_err("must fail");
        match err {
            CompareError::Detail(reason) => {
                assert!(reason.contains("non-comparable type"));
                assert!(reason.contains("[]uint8"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn lt_and_related_ops_follow_go_signed_unsigned_rules() {
        assert_eq!(
            lt_values(&Some(json!(-1)), &Some(json!(1u64))).expect("lt"),
            true
        );
        assert_eq!(
            lt_values(&Some(json!(1u64)), &Some(json!(-1))).expect("lt"),
            false
        );
        assert_eq!(
            le_values(&Some(json!(1)), &Some(json!(1))).expect("le"),
            true
        );
    }

    #[test]
    fn lt_reports_invalid_and_incompatible_errors_like_go() {
        let err =
            lt_values(&Some(Value::Bool(true)), &Some(Value::Bool(false))).expect_err("must fail");
        assert_eq!(err, CompareError::InvalidType);

        let err = lt_values(&Some(Value::Bool(true)), &Some(json!(1))).expect_err("must fail");
        assert_eq!(err, CompareError::IncompatibleTypes);
    }

    #[test]
    fn eq_handles_typed_nil_slice() {
        let nil_slice = Some(encode_go_typed_slice_value("int", None));
        assert_eq!(eq_values(&nil_slice, &nil_slice).expect("eq"), true);
    }
}
