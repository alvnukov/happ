use super::{MissingValueMode, NativeRenderError};
use crate::gotemplates::typedvalue::{
    decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_len, go_string_bytes_len,
    go_type_is_interface, go_zero_value_for_type,
};
use serde_json::Value;

pub(super) fn resolve_simple_path(
    root: &Value,
    dot: &Value,
    expr: &str,
    missing_value_mode: MissingValueMode,
    lookup_var: impl Fn(&str) -> Option<Option<Value>>,
) -> Result<Option<Value>, NativeRenderError> {
    if expr == "." {
        return Ok(Some(dot.clone()));
    }
    if expr == "$" {
        return Ok(Some(root.clone()));
    }
    let (base, mut path) = if let Some(rest) = expr.strip_prefix("$.") {
        (Some(root.clone()), rest)
    } else if let Some(rest) = expr.strip_prefix('.') {
        (Some(dot.clone()), rest)
    } else if let Some((name, rest)) = split_variable_reference(expr) {
        let value = if name == "$" {
            Some(root.clone())
        } else {
            lookup_var(name).unwrap_or(None)
        };
        (value, rest)
    } else {
        return Ok(None);
    };
    if path.is_empty() {
        return Ok(base);
    }

    let mut cur = match base {
        Some(v) => v,
        None => return Ok(None),
    };
    let mut came_from_zero_missing = false;
    while !path.is_empty() {
        let (seg, rest) = match split_first_segment(path) {
            Some(v) => v,
            None => return Ok(None),
        };
        cur = if let Some(typed_map) = decode_go_typed_map_value(&cur) {
            if let Some(next) = typed_map
                .entries
                .and_then(|entries| entries.get(seg))
                .cloned()
            {
                came_from_zero_missing = false;
                next
            } else if missing_value_mode == MissingValueMode::GoZero {
                let zero = go_zero_value_for_type(typed_map.elem_type);
                came_from_zero_missing =
                    matches!(zero, Value::Null) && go_type_is_interface(typed_map.elem_type);
                zero
            } else {
                return Ok(None);
            }
        } else {
            if decode_go_typed_slice_value(&cur).is_some()
                || go_bytes_len(&cur).is_some()
                || go_string_bytes_len(&cur).is_some()
            {
                return Err(NativeRenderError::UnsupportedAction {
                    action: format!("{{{{{expr}}}}}"),
                    reason: format!(
                        "can't evaluate field {seg} in type {}",
                        value_type_name_for_path(&cur)
                    ),
                });
            }
            match &cur {
                Value::Object(map) => {
                    if let Some(next) = map.get(seg) {
                        came_from_zero_missing = false;
                        next.clone()
                    } else if missing_value_mode == MissingValueMode::GoZero {
                        came_from_zero_missing = true;
                        Value::Null
                    } else {
                        return Ok(None);
                    }
                }
                Value::Null
                    if came_from_zero_missing && missing_value_mode == MissingValueMode::GoZero =>
                {
                    return Err(NativeRenderError::UnsupportedAction {
                        action: format!("{{{{{expr}}}}}"),
                        reason: format!("nil pointer evaluating interface {{}}.{seg}"),
                    });
                }
                Value::Null => return Ok(None),
                _ => {
                    return Err(NativeRenderError::UnsupportedAction {
                        action: format!("{{{{{expr}}}}}"),
                        reason: format!(
                            "can't evaluate field {seg} in type {}",
                            value_type_name_for_path(&cur)
                        ),
                    });
                }
            }
        };
        path = rest;
    }
    Ok(Some(cur))
}

pub(super) fn split_variable_reference(expr: &str) -> Option<(&str, &str)> {
    if expr == "$" {
        return Some(("$", ""));
    }
    if !expr.starts_with('$') || expr.starts_with("$.") {
        return None;
    }
    let mut iter = expr[1..].char_indices();
    let (_, first) = iter.next()?;
    if !is_identifier_start_char(first) {
        return None;
    }
    let mut end = 1 + first.len_utf8();
    for (offset, ch) in iter {
        if !is_identifier_continue_char(ch) {
            break;
        }
        end = 1 + offset + ch.len_utf8();
    }
    if end == 1 {
        return None;
    }
    let name = &expr[..end];
    if end == expr.len() {
        return Some((name, ""));
    }
    if !expr[end..].starts_with('.') {
        return None;
    }
    Some((name, &expr[end + 1..]))
}

pub(super) fn is_identifier_start_char(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

pub(super) fn is_identifier_continue_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let mut iter = path.splitn(2, '.');
    let seg = iter.next()?;
    if seg.is_empty() {
        return None;
    }
    if !seg.chars().all(is_path_segment_char) {
        return None;
    }
    let rest = iter.next().unwrap_or("");
    Some((seg, rest))
}

fn is_path_segment_char(ch: char) -> bool {
    is_identifier_continue_char(ch) || ch == '-'
}

fn value_type_name_for_path(v: &Value) -> String {
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
    use super::{resolve_simple_path, split_variable_reference, MissingValueMode};
    use crate::gotemplates::NativeRenderError;
    use serde_json::json;

    #[test]
    fn split_variable_reference_supports_go_style_scope_tokens() {
        assert_eq!(split_variable_reference("$"), Some(("$", "")));
        assert_eq!(split_variable_reference("$x"), Some(("$x", "")));
        assert_eq!(split_variable_reference("$x.y.z"), Some(("$x", "y.z")));
        assert_eq!(split_variable_reference("$.x"), None);
        assert_eq!(split_variable_reference("$1"), None);
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
