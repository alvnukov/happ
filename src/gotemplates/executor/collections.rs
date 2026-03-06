use super::{
    decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
    decode_go_typed_slice_value, encode_go_bytes_value, encode_go_nil_bytes_value,
    encode_go_typed_slice_value, go_bytes_get, go_bytes_is_nil, go_bytes_len,
    go_string_bytes_get, go_string_bytes_len, go_zero_value_for_type, map_key_arg,
    option_type_name_for_template, parse_slice_like_index, value_from_go_string_bytes,
    value_type_name_for_template, wrong_number_of_args, MapKeyArg, NativeRenderError,
};
use serde_json::{Number, Value};

pub(super) fn builtin_len(action: &str, args: &[Option<Value>]) -> Result<usize, NativeRenderError> {
    if args.len() != 1 {
        return Err(wrong_number_of_args(action, "len", "1", args.len()));
    }
    let value = args[0]
        .as_ref()
        .ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling len: len of nil pointer".to_string(),
        })?;
    if let Some(len) = go_bytes_len(value).or_else(|| go_string_bytes_len(value)) {
        return Ok(len);
    }
    if let Some(typed_map) = decode_go_typed_map_value(value) {
        return Ok(typed_map.entries.map_or(0, |entries| entries.len()));
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(value) {
        return Ok(typed_slice.items.map_or(0, <[Value]>::len));
    }
    match value {
        Value::Null => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling len: len of nil pointer".to_string(),
        }),
        Value::String(s) => Ok(s.len()),
        Value::Array(a) => Ok(a.len()),
        Value::Object(m) => Ok(m.len()),
        _ => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling len: len of type {}",
                value_type_name_for_template(value)
            ),
        }),
    }
}

pub(super) fn builtin_index(
    action: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "index", "at least 1", 0));
    }
    let mut cur = args[0].clone();
    if cur.is_none() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling index: index of untyped nil".to_string(),
        });
    }
    if args.len() == 1 {
        return Ok(cur);
    }
    for idx in args.iter().skip(1) {
        if let Some(ref value) = cur {
            if let Some(typed_map) = decode_go_typed_map_value(value) {
                let next = match map_key_arg(idx) {
                    MapKeyArg::Key(key) => typed_map
                        .entries
                        .and_then(|entries| entries.get(&key))
                        .cloned()
                        .unwrap_or_else(|| go_zero_value_for_type(typed_map.elem_type)),
                    MapKeyArg::StringLikeNonUtf8 => go_zero_value_for_type(typed_map.elem_type),
                    MapKeyArg::WrongType => {
                        let suffix = if matches!(idx, None | Some(Value::Null)) {
                            "value is nil; should be string".to_string()
                        } else {
                            format!(
                                "value has type {}; should be string",
                                option_type_name_for_template(idx)
                            )
                        };
                        return Err(NativeRenderError::UnsupportedAction {
                            action: action.to_string(),
                            reason: format!("error calling index: {suffix}"),
                        });
                    }
                };
                cur = Some(next);
                continue;
            }
            if let Some(typed_slice) = decode_go_typed_slice_value(value) {
                let len = typed_slice.items.map_or(0, <[Value]>::len);
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let item = typed_slice
                    .items
                    .and_then(|items| items.get(pos))
                    .ok_or_else(|| NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed typed slice value".to_string(),
                    })?;
                cur = Some(item.clone());
                continue;
            }
            if let Some(len) = go_bytes_len(value) {
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let byte = go_bytes_get(value, pos).ok_or_else(|| {
                    NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed []byte value".to_string(),
                    }
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
            if let Some(len) = go_string_bytes_len(value) {
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let byte = go_string_bytes_get(value, pos).ok_or_else(|| {
                    NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed string value".to_string(),
                    }
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
        }
        let next = match cur {
            Some(Value::Array(ref items)) => {
                let pos = parse_slice_like_index(action, "index", idx, items.len())?;
                Some(items[pos].clone())
            }
            Some(Value::Object(ref map)) => match map_key_arg(idx) {
                MapKeyArg::Key(key) => map.get(&key).cloned(),
                MapKeyArg::StringLikeNonUtf8 => None,
                MapKeyArg::WrongType => {
                    let suffix = if matches!(idx, None | Some(Value::Null)) {
                        "value is nil; should be string".to_string()
                    } else {
                        format!(
                            "value has type {}; should be string",
                            option_type_name_for_template(idx)
                        )
                    };
                    return Err(NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: format!("error calling index: {suffix}"),
                    });
                }
            },
            Some(Value::String(ref s)) => {
                let bytes = s.as_bytes();
                let pos = parse_slice_like_index(action, "index", idx, bytes.len())?;
                Some(Value::Number(Number::from(bytes[pos])))
            }
            Some(Value::Null) | None => {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "error calling index: index of untyped nil".to_string(),
                });
            }
            Some(ref value) => {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling index: can't index item of type {}",
                        value_type_name_for_template(value)
                    ),
                });
            }
        };
        cur = next;
    }
    Ok(cur)
}

pub(super) fn builtin_slice(
    action: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "slice", "at least 1", 0));
    }
    if args.len() > 4 {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling slice: too many slice indexes: {}",
                args.len() - 1
            ),
        });
    }
    let item = args[0]
        .as_ref()
        .ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling slice: slice of untyped nil".to_string(),
        })?;

    if let Some(bytes) = decode_go_bytes_value(item) {
        let was_nil_bytes = go_bytes_is_nil(item);
        let cap = bytes.len();
        let len = bytes.len();
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() < 4 {
            if was_nil_bytes && idx[0] == 0 && idx[1] == 0 {
                return Ok(Some(encode_go_nil_bytes_value()));
            }
            return Ok(Some(encode_go_bytes_value(&bytes[idx[0]..idx[1]])));
        }
        if idx[1] > idx[2] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[1], idx[2]
                ),
            });
        }
        if was_nil_bytes && idx[0] == 0 && idx[1] == 0 {
            return Ok(Some(encode_go_nil_bytes_value()));
        }
        return Ok(Some(encode_go_bytes_value(&bytes[idx[0]..idx[1]])));
    }
    if let Some(bytes) = decode_go_string_bytes_value(item) {
        let cap = bytes.len();
        let len = bytes.len();
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() == 4 {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "error calling slice: cannot 3-index slice a string".to_string(),
            });
        }
        let sliced = bytes[idx[0]..idx[1]].to_vec();
        return Ok(Some(value_from_go_string_bytes(sliced)));
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(item) {
        let cap = typed_slice.items.map_or(0, <[Value]>::len);
        let len = cap;
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() > 3 && idx[1] > idx[2] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[1], idx[2]
                ),
            });
        }
        if typed_slice.items.is_none() {
            return Ok(Some(encode_go_typed_slice_value(
                typed_slice.elem_type,
                None,
            )));
        }
        let Some(items) = typed_slice.items else {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "error calling slice: malformed typed slice value".to_string(),
            });
        };
        return Ok(Some(encode_go_typed_slice_value(
            typed_slice.elem_type,
            Some(items[idx[0]..idx[1]].to_vec()),
        )));
    }

    match item {
        Value::Array(items) => {
            let cap = items.len();
            let len = items.len();
            let mut idx = [0usize, len, cap];
            for (i, index_arg) in args.iter().skip(1).enumerate() {
                idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[0], idx[1]
                    ),
                });
            }
            if args.len() <= 3 {
                return Ok(Some(Value::Array(items[idx[0]..idx[1]].to_vec())));
            }
            if idx[1] > idx[2] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[1], idx[2]
                    ),
                });
            }
            Ok(Some(Value::Array(items[idx[0]..idx[1]].to_vec())))
        }
        Value::String(s) => {
            if args.len() == 4 {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "error calling slice: cannot 3-index slice a string".to_string(),
                });
            }
            let cap = s.len();
            let len = s.len();
            let mut idx = [0usize, len];
            for (i, index_arg) in args.iter().skip(1).enumerate() {
                idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[0], idx[1]
                    ),
                });
            }
            let bytes = s.as_bytes()[idx[0]..idx[1]].to_vec();
            Ok(Some(value_from_go_string_bytes(bytes)))
        }
        _ => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling slice: can't slice item of type {}",
                value_type_name_for_template(item)
            ),
        }),
    }
}
