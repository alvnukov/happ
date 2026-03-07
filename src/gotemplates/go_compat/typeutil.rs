use crate::gotemplates::go_compat::path::value_type_name_for_path;
use crate::gotemplates::typedvalue::{
    decode_go_string_bytes_value, decode_go_typed_slice_value, go_bytes_len, go_string_bytes_len,
};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceLikeIndexMode {
    Index,
    Slice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseSliceLikeIndexError {
    Nil,
    WrongType { type_name: String },
    OutOfRange { raw: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapKeyArg {
    Key(String),
    StringLikeNonUtf8,
    WrongType,
}

pub fn parse_slice_like_index(
    idx_arg: Option<&Value>,
    cap: usize,
    mode: SliceLikeIndexMode,
) -> Result<usize, ParseSliceLikeIndexError> {
    let raw = match idx_arg {
        None | Some(Value::Null) => return Err(ParseSliceLikeIndexError::Nil),
        Some(v) => value_to_i64(v).ok_or_else(|| ParseSliceLikeIndexError::WrongType {
            type_name: value_type_name_for_template(v),
        })?,
    };
    let out_of_range = match mode {
        SliceLikeIndexMode::Index => raw < 0 || raw as usize >= cap,
        SliceLikeIndexMode::Slice => raw < 0 || raw as usize > cap,
    };
    if out_of_range {
        return Err(ParseSliceLikeIndexError::OutOfRange { raw });
    }
    Ok(raw as usize)
}

pub fn value_from_go_string_bytes(bytes: Vec<u8>) -> Value {
    match String::from_utf8(bytes) {
        Ok(s) => Value::String(s),
        Err(err) => crate::gotemplates::encode_go_string_bytes_value(&err.into_bytes()),
    }
}

pub fn map_key_arg(v: Option<&Value>) -> MapKeyArg {
    match v {
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

pub fn option_string_like_bytes<'a>(v: Option<&'a Value>) -> Option<Cow<'a, [u8]>> {
    match v {
        Some(Value::String(s)) => Some(Cow::Borrowed(s.as_bytes())),
        Some(other) => decode_go_string_bytes_value(other).map(Cow::Owned),
        None => None,
    }
}

pub fn is_go_bytes_slice(v: &Value) -> bool {
    go_bytes_len(v).is_some()
}

pub fn is_map_object(v: &Value) -> bool {
    matches!(v, Value::Object(_))
        && go_bytes_len(v).is_none()
        && go_string_bytes_len(v).is_none()
        && decode_go_typed_slice_value(v).is_none()
}

pub fn option_type_name_for_template(v: Option<&Value>) -> String {
    match v {
        Some(value) => value_type_name_for_template(value),
        None => "<nil>".to_string(),
    }
}

pub fn value_type_name_for_template(v: &Value) -> String {
    value_type_name_for_path(v)
}

fn value_to_i64(v: &Value) -> Option<i64> {
    let Value::Number(n) = v else {
        return None;
    };
    if let Some(i) = n.as_i64() {
        Some(i)
    } else {
        n.as_u64().map(|u| u as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_key_arg, option_string_like_bytes, parse_slice_like_index, value_type_name_for_template,
        MapKeyArg, ParseSliceLikeIndexError, SliceLikeIndexMode,
    };
    use serde_json::json;

    #[test]
    fn parse_slice_like_index_validates_bounds_like_go() {
        let idx = parse_slice_like_index(Some(&json!(1)), 3, SliceLikeIndexMode::Index)
            .expect("must parse");
        assert_eq!(idx, 1);

        let err = parse_slice_like_index(Some(&json!(3)), 3, SliceLikeIndexMode::Index)
            .expect_err("must fail");
        assert_eq!(err, ParseSliceLikeIndexError::OutOfRange { raw: 3 });

        let idx = parse_slice_like_index(Some(&json!(3)), 3, SliceLikeIndexMode::Slice)
            .expect("must parse");
        assert_eq!(idx, 3);
    }

    #[test]
    fn map_key_arg_supports_utf8_and_rejects_wrong_types() {
        assert_eq!(map_key_arg(Some(&json!("k"))), MapKeyArg::Key("k".to_string()));
        assert_eq!(map_key_arg(Some(&json!(1))), MapKeyArg::WrongType);
    }

    #[test]
    fn option_string_like_bytes_handles_plain_strings() {
        let bytes = option_string_like_bytes(Some(&json!("ab"))).expect("bytes");
        assert_eq!(bytes.as_ref(), b"ab");
    }

    #[test]
    fn value_type_name_reports_go_typed_shapes() {
        let b = crate::gotemplates::encode_go_bytes_value(&[1, 2]);
        assert_eq!(value_type_name_for_template(&b), "[]uint8");

        let t = crate::gotemplates::encode_go_typed_slice_value("int", Some(vec![json!(1)]));
        assert_eq!(value_type_name_for_template(&t), "[]int");
    }
}
