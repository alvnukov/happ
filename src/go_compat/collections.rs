use crate::go_compat::typedvalue::{
    decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
    decode_go_typed_slice_value, encode_go_bytes_value, encode_go_nil_bytes_value,
    encode_go_typed_slice_value, go_bytes_get, go_bytes_is_nil, go_bytes_len, go_string_bytes_get,
    go_string_bytes_len, go_zero_value_for_type,
};
use crate::go_compat::typeutil::{
    map_key_arg, option_type_name_for_template, parse_slice_like_index, value_from_go_string_bytes,
    value_type_name_for_template, MapKeyArg, ParseSliceLikeIndexError, SliceLikeIndexMode,
};
use serde_json::{Number, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionsError {
    pub reason: String,
}

pub fn builtin_len(args: &[Option<Value>]) -> Result<usize, CollectionsError> {
    if args.len() != 1 {
        return Err(wrong_number_of_args("len", "1", args.len()));
    }
    let value = args[0].as_ref().ok_or_else(|| CollectionsError {
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
        Value::Null => Err(CollectionsError {
            reason: "error calling len: len of nil pointer".to_string(),
        }),
        Value::String(s) => Ok(s.len()),
        Value::Array(a) => Ok(a.len()),
        Value::Object(m) => Ok(m.len()),
        _ => Err(CollectionsError {
            reason: format!(
                "error calling len: len of type {}",
                value_type_name_for_template(value)
            ),
        }),
    }
}

pub fn builtin_index(args: &[Option<Value>]) -> Result<Option<Value>, CollectionsError> {
    if args.is_empty() {
        return Err(wrong_number_of_args("index", "at least 1", 0));
    }
    let mut cur = args[0].clone();
    if cur.is_none() {
        return Err(CollectionsError {
            reason: "error calling index: index of untyped nil".to_string(),
        });
    }
    if args.len() == 1 {
        return Ok(cur);
    }
    for (hop_idx, idx) in args.iter().skip(1).enumerate() {
        if matches!(cur, Some(Value::Null) | None) {
            return Err(CollectionsError {
                reason: if hop_idx == 0 {
                    "error calling index: index of untyped nil".to_string()
                } else {
                    "error calling index: index of nil pointer".to_string()
                },
            });
        }
        if let Some(ref value) = cur {
            if let Some(typed_map) = decode_go_typed_map_value(value) {
                let next = match map_key_arg(idx.as_ref()) {
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
                                option_type_name_for_template(idx.as_ref())
                            )
                        };
                        return Err(CollectionsError {
                            reason: format!("error calling index: {suffix}"),
                        });
                    }
                };
                cur = Some(next);
                continue;
            }
            if let Some(typed_slice) = decode_go_typed_slice_value(value) {
                let len = typed_slice.items.map_or(0, <[Value]>::len);
                let pos = parse_index_pos(idx, len)?;
                if pos == len {
                    return Err(index_reflect_out_of_range(IndexTargetKind::Slice));
                }
                let item = typed_slice
                    .items
                    .and_then(|items| items.get(pos))
                    .ok_or_else(|| CollectionsError {
                        reason: "error calling index: malformed typed slice value".to_string(),
                    })?;
                cur = Some(item.clone());
                continue;
            }
            if let Some(len) = go_bytes_len(value) {
                let pos = parse_index_pos(idx, len)?;
                if pos == len {
                    return Err(index_reflect_out_of_range(IndexTargetKind::Slice));
                }
                let byte = go_bytes_get(value, pos).ok_or_else(|| CollectionsError {
                    reason: "error calling index: malformed []byte value".to_string(),
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
            if let Some(len) = go_string_bytes_len(value) {
                let pos = parse_index_pos(idx, len)?;
                if pos == len {
                    return Err(index_reflect_out_of_range(IndexTargetKind::String));
                }
                let byte = go_string_bytes_get(value, pos).ok_or_else(|| CollectionsError {
                    reason: "error calling index: malformed string value".to_string(),
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
        }
        let next = match cur {
            Some(Value::Array(ref items)) => {
                let pos = parse_index_pos(idx, items.len())?;
                if pos == items.len() {
                    return Err(index_reflect_out_of_range(IndexTargetKind::Slice));
                }
                Some(items[pos].clone())
            }
            Some(Value::Object(ref map)) => match map_key_arg(idx.as_ref()) {
                MapKeyArg::Key(key) => map.get(&key).cloned(),
                MapKeyArg::StringLikeNonUtf8 => None,
                MapKeyArg::WrongType => {
                    let suffix = if matches!(idx, None | Some(Value::Null)) {
                        "value is nil; should be string".to_string()
                    } else {
                        format!(
                            "value has type {}; should be string",
                            option_type_name_for_template(idx.as_ref())
                        )
                    };
                    return Err(CollectionsError {
                        reason: format!("error calling index: {suffix}"),
                    });
                }
            },
            Some(Value::String(ref s)) => {
                let bytes = s.as_bytes();
                let pos = parse_index_pos(idx, bytes.len())?;
                if pos == bytes.len() {
                    return Err(index_reflect_out_of_range(IndexTargetKind::String));
                }
                Some(Value::Number(Number::from(bytes[pos])))
            }
            Some(ref value) => {
                return Err(CollectionsError {
                    reason: format!(
                        "error calling index: can't index item of type {}",
                        value_type_name_for_template(value)
                    ),
                });
            }
            None => {
                return Err(CollectionsError {
                    reason: "error calling index: nil cursor".to_string(),
                });
            }
        };
        cur = next;
    }
    Ok(cur)
}

pub fn builtin_slice(args: &[Option<Value>]) -> Result<Option<Value>, CollectionsError> {
    if args.is_empty() {
        return Err(wrong_number_of_args("slice", "at least 1", 0));
    }
    if args.len() > 4 {
        return Err(CollectionsError {
            reason: format!(
                "error calling slice: too many slice indexes: {}",
                args.len() - 1
            ),
        });
    }
    let item = args[0].as_ref().ok_or_else(|| CollectionsError {
        reason: "error calling slice: slice of untyped nil".to_string(),
    })?;

    if let Some(bytes) = decode_go_bytes_value(item) {
        let was_nil_bytes = go_bytes_is_nil(item);
        let cap = bytes.len();
        let len = bytes.len();
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_pos(index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(CollectionsError {
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
            return Err(CollectionsError {
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
            idx[i] = parse_slice_pos(index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(CollectionsError {
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() == 4 {
            return Err(CollectionsError {
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
            idx[i] = parse_slice_pos(index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(CollectionsError {
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() > 3 && idx[1] > idx[2] {
            return Err(CollectionsError {
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
            return Err(CollectionsError {
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
                idx[i] = parse_slice_pos(index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(CollectionsError {
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
                return Err(CollectionsError {
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
                return Err(CollectionsError {
                    reason: "error calling slice: cannot 3-index slice a string".to_string(),
                });
            }
            let cap = s.len();
            let len = s.len();
            let mut idx = [0usize, len];
            for (i, index_arg) in args.iter().skip(1).enumerate() {
                idx[i] = parse_slice_pos(index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(CollectionsError {
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[0], idx[1]
                    ),
                });
            }
            let bytes = s.as_bytes()[idx[0]..idx[1]].to_vec();
            Ok(Some(value_from_go_string_bytes(bytes)))
        }
        _ => Err(CollectionsError {
            reason: format!(
                "error calling slice: can't slice item of type {}",
                value_type_name_for_template(item)
            ),
        }),
    }
}

#[derive(Debug, Clone, Copy)]
enum IndexTargetKind {
    Slice,
    String,
}

fn wrong_number_of_args(fn_name: &str, want: &str, got: usize) -> CollectionsError {
    CollectionsError {
        reason: format!("wrong number of args for {fn_name}: want {want} got {got}"),
    }
}

fn parse_slice_pos(idx_arg: &Option<Value>, cap: usize) -> Result<usize, CollectionsError> {
    parse_slice_like_index(idx_arg.as_ref(), cap, SliceLikeIndexMode::Slice)
        .map_err(|err| map_parse_slice_like_index_error("slice", err))
}

fn parse_index_pos(idx_arg: &Option<Value>, len: usize) -> Result<usize, CollectionsError> {
    // Go text/template indexArg permits `x == cap` and then the following reflect
    // index operation raises a more specific "reflect: ... index out of range" panic.
    // We preserve that behavior by parsing with a +1 bound and handling `x == len`
    // at the call site.
    let parse_cap = len.saturating_add(1);
    parse_slice_like_index(idx_arg.as_ref(), parse_cap, SliceLikeIndexMode::Index)
        .map_err(|err| map_parse_slice_like_index_error("index", err))
}

fn map_parse_slice_like_index_error(
    call_name: &str,
    err: ParseSliceLikeIndexError,
) -> CollectionsError {
    let reason = match err {
        ParseSliceLikeIndexError::Nil => {
            format!("error calling {call_name}: cannot index slice/array with nil")
        }
        ParseSliceLikeIndexError::WrongType { type_name } => {
            format!("error calling {call_name}: cannot index slice/array with type {type_name}")
        }
        ParseSliceLikeIndexError::OutOfRange { raw } => {
            format!("error calling {call_name}: index out of range: {raw}")
        }
    };
    CollectionsError { reason }
}

fn index_reflect_out_of_range(kind: IndexTargetKind) -> CollectionsError {
    let detail = match kind {
        IndexTargetKind::Slice => "reflect: slice index out of range",
        IndexTargetKind::String => "reflect: string index out of range",
    };
    CollectionsError {
        reason: format!("error calling index: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_index, builtin_len, builtin_slice, CollectionsError};
    use serde_json::{json, Map, Number, Value};

    fn reason(err: CollectionsError) -> String {
        err.reason
    }

    #[test]
    fn index_boundary_equals_len_matches_go_reflect_errors() {
        let err = builtin_index(&[Some(json!([1, 2])), Some(json!(2))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: reflect: slice index out of range"));

        let err = builtin_index(&[Some(json!("ab")), Some(json!(2))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: reflect: string index out of range"));
    }

    #[test]
    fn index_above_len_keeps_index_out_of_range_message() {
        let err = builtin_index(&[Some(json!([1, 2])), Some(json!(3))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: index out of range: 3"));
    }

    #[test]
    fn len_matches_go_for_supported_and_unsupported_values() {
        assert_eq!(builtin_len(&[Some(json!([1, 2, 3]))]).expect("len"), 3);
        assert_eq!(builtin_len(&[Some(json!("abc"))]).expect("len"), 3);

        let err = builtin_len(&[None]).expect_err("must fail");
        assert!(reason(err).contains("error calling len: len of nil pointer"));

        let err = builtin_len(&[Some(json!(3))]).expect_err("must fail");
        assert!(reason(err).contains("error calling len: len of type int"));
    }

    #[test]
    fn index_map_missing_key_returns_zero_value_for_typed_maps() {
        let mut entries = Map::new();
        entries.insert("a".to_string(), Value::Number(Number::from(7)));
        let typed = crate::go_compat::typedvalue::encode_go_typed_map_value("int", Some(entries));
        let out = builtin_index(&[Some(typed), Some(json!("missing"))]).expect("index");
        assert_eq!(out, Some(Value::Number(Number::from(0))));
    }

    #[test]
    fn slice_respects_string_and_index_rules() {
        let out =
            builtin_slice(&[Some(json!("abcd")), Some(json!(1)), Some(json!(3))]).expect("slice");
        assert_eq!(out, Some(json!("bc")));

        let err = builtin_slice(&[
            Some(json!("abcd")),
            Some(json!(1)),
            Some(json!(2)),
            Some(json!(2)),
        ])
        .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: cannot 3-index slice a string"));
    }

    #[test]
    fn slice_validates_bounds_like_go() {
        let err = builtin_slice(&[Some(json!([1, 2, 3])), Some(json!(2)), Some(json!(1))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: invalid slice index: 2 > 1"));

        let err = builtin_slice(&[Some(json!([1, 2, 3])), Some(json!(4)), Some(json!(5))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling slice: index out of range: 4"));
    }

    #[test]
    fn index_chain_after_missing_map_reports_nil_pointer_like_go() {
        let root = Some(json!({"a":{"x":1}}));
        let err = builtin_index(&[root, Some(json!("missing")), Some(json!("x"))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of nil pointer"));
    }

    #[test]
    fn index_root_untyped_nil_still_reports_untyped_nil() {
        let err = builtin_index(&[Some(Value::Null), Some(json!(1))]).expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of untyped nil"));
    }

    #[test]
    fn index_chain_after_typed_interface_zero_reports_nil_pointer() {
        let mut entries = Map::new();
        entries.insert("a".to_string(), json!({"x":1}));
        let typed =
            crate::go_compat::typedvalue::encode_go_typed_map_value("interface {}", Some(entries));
        let err = builtin_index(&[Some(typed), Some(json!("missing")), Some(json!("x"))])
            .expect_err("must fail");
        assert!(reason(err).contains("error calling index: index of nil pointer"));
    }
}
