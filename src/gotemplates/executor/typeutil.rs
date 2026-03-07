use super::{
    decode_go_string_bytes_value, decode_go_typed_map_value, decode_go_typed_slice_value,
    format_value_for_print, go_bytes_len, go_string_bytes_len, NativeRenderError,
};
use serde_json::Value;
use std::borrow::Cow;

fn value_to_i64(v: &Option<Value>) -> Option<i64> {
    match v.as_ref() {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else {
                n.as_u64().map(|u| u as i64)
            }
        }
        _ => None,
    }
}

pub(super) fn parse_slice_like_index(
    action: &str,
    call_name: &str,
    idx_arg: &Option<Value>,
    cap: usize,
) -> Result<usize, NativeRenderError> {
    let raw = match idx_arg.as_ref() {
        None | Some(Value::Null) => {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("error calling {call_name}: cannot index slice/array with nil"),
            });
        }
        Some(v) => value_to_i64(idx_arg).ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling {call_name}: cannot index slice/array with type {}",
                value_type_name_for_template(v)
            ),
        })?,
    };
    let out_of_range = if call_name == "index" {
        raw < 0 || raw as usize >= cap
    } else {
        raw < 0 || raw as usize > cap
    };
    if out_of_range {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("error calling {call_name}: index out of range: {raw}"),
        });
    }
    Ok(raw as usize)
}

pub(super) fn value_from_go_string_bytes(bytes: Vec<u8>) -> Value {
    match String::from_utf8(bytes) {
        Ok(s) => Value::String(s),
        Err(err) => crate::gotemplates::encode_go_string_bytes_value(&err.into_bytes()),
    }
}

pub(super) enum MapKeyArg {
    Key(String),
    StringLikeNonUtf8,
    WrongType,
}

pub(super) fn map_key_arg(v: &Option<Value>) -> MapKeyArg {
    match v.as_ref() {
        Some(Value::String(s)) => MapKeyArg::Key(s.clone()),
        Some(other) if go_string_bytes_len(other).is_some() => {
            let Some(bytes) = decode_go_string_bytes_value(other) else {
                return MapKeyArg::StringLikeNonUtf8;
            };
            match String::from_utf8(bytes) {
                Ok(s) => MapKeyArg::Key(s),
                Err(_) => MapKeyArg::StringLikeNonUtf8,
            }
        }
        _ => MapKeyArg::WrongType,
    }
}

pub(super) fn option_string_like_bytes(v: &Option<Value>) -> Option<Cow<'_, [u8]>> {
    match v.as_ref() {
        Some(Value::String(s)) => Some(Cow::Borrowed(s.as_bytes())),
        Some(other) => decode_go_string_bytes_value(other).map(Cow::Owned),
        None => None,
    }
}

pub(super) fn is_go_bytes_slice_option(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(|value| go_bytes_len(value).is_some())
}

pub(super) fn is_map_object_option(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(|value| {
        matches!(value, Value::Object(_))
            && go_bytes_len(value).is_none()
            && go_string_bytes_len(value).is_none()
            && decode_go_typed_slice_value(value).is_none()
    })
}

pub(super) fn format_non_comparable_type_reason(v: &Option<Value>) -> String {
    format!(
        "non-comparable type {}: {}",
        format_value_for_print(v),
        option_type_name_for_template(v)
    )
}

pub(super) fn format_non_comparable_types_reason(a: &Option<Value>, b: &Option<Value>) -> String {
    format!(
        "non-comparable types {}: {}, {}: {}",
        format_value_for_print(a),
        option_type_name_for_template(a),
        option_type_name_for_template(b),
        format_value_for_print(b)
    )
}

pub(super) fn option_type_name_for_template(v: &Option<Value>) -> String {
    match v.as_ref() {
        Some(value) => value_type_name_for_template(value),
        None => "<nil>".to_string(),
    }
}

pub(super) fn value_type_name_for_template(v: &Value) -> String {
    if go_bytes_len(v).is_some() {
        return "[]uint8".to_string();
    }
    if go_string_bytes_len(v).is_some() {
        return "string".to_string();
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(v) {
        return format!("[]{}", typed_slice.elem_type);
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format!("map[string]{}", typed_map.elem_type);
    }
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(_) => "[]interface {}".to_string(),
        Value::Object(_) => "map[string]interface {}".to_string(),
        Value::Number(n) => {
            if n.as_i64().is_some() {
                "int".to_string()
            } else if n.as_u64().is_some() {
                "uint".to_string()
            } else {
                "float64".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_slice_like_index, value_type_name_for_template};
    use serde_json::json;

    #[test]
    fn parse_slice_like_index_validates_bounds_like_go() {
        let idx = parse_slice_like_index("", "index", &Some(json!(1)), 3).expect("must parse");
        assert_eq!(idx, 1);

        let err = parse_slice_like_index("", "index", &Some(json!(3)), 3).expect_err("must fail");
        assert!(matches!(
            err,
            crate::gotemplates::NativeRenderError::UnsupportedAction { .. }
        ));

        let idx = parse_slice_like_index("", "slice", &Some(json!(3)), 3).expect("must parse");
        assert_eq!(idx, 3);
    }

    #[test]
    fn value_type_name_reports_go_typed_shapes() {
        let b = crate::gotemplates::encode_go_bytes_value(&[1, 2]);
        assert_eq!(value_type_name_for_template(&b), "[]uint8");

        let t = crate::gotemplates::encode_go_typed_slice_value("int", Some(vec![json!(1)]));
        assert_eq!(value_type_name_for_template(&t), "[]int");
    }
}
