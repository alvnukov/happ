use super::{format_value_for_print, NativeRenderError};
use crate::go_compat::typeutil::{
    is_go_bytes_slice as go_is_go_bytes_slice, is_map_object as go_is_map_object,
    map_key_arg as go_map_key_arg, option_string_like_bytes as go_option_string_like_bytes,
    option_type_name_for_template as go_option_type_name_for_template,
    parse_slice_like_index as go_parse_slice_like_index,
    value_from_go_string_bytes as go_value_from_go_string_bytes,
    value_type_name_for_template as go_value_type_name_for_template, ParseSliceLikeIndexError,
    SliceLikeIndexMode,
};
use serde_json::Value;
use std::borrow::Cow;

pub(super) use crate::go_compat::typeutil::MapKeyArg;

pub(super) fn parse_slice_like_index(
    action: &str,
    call_name: &str,
    idx_arg: &Option<Value>,
    cap: usize,
) -> Result<usize, NativeRenderError> {
    let mode = if call_name == "index" {
        SliceLikeIndexMode::Index
    } else {
        SliceLikeIndexMode::Slice
    };
    go_parse_slice_like_index(idx_arg.as_ref(), cap, mode).map_err(|err| match err {
        ParseSliceLikeIndexError::Nil => NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("error calling {call_name}: cannot index slice/array with nil"),
        },
        ParseSliceLikeIndexError::WrongType { type_name } => NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling {call_name}: cannot index slice/array with type {type_name}"
            ),
        },
        ParseSliceLikeIndexError::OutOfRange { raw } => NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("error calling {call_name}: index out of range: {raw}"),
        },
    })
}

pub(super) fn value_from_go_string_bytes(bytes: Vec<u8>) -> Value {
    go_value_from_go_string_bytes(bytes)
}

pub(super) fn map_key_arg(v: &Option<Value>) -> MapKeyArg {
    go_map_key_arg(v.as_ref())
}

pub(super) fn option_string_like_bytes(v: &Option<Value>) -> Option<Cow<'_, [u8]>> {
    go_option_string_like_bytes(v.as_ref())
}

pub(super) fn is_go_bytes_slice_option(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(go_is_go_bytes_slice)
}

pub(super) fn is_map_object_option(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(go_is_map_object)
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
    go_option_type_name_for_template(v.as_ref())
}

pub(super) fn value_type_name_for_template(v: &Value) -> String {
    go_value_type_name_for_template(v)
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
