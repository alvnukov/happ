use super::{MissingValueMode, NativeRenderError};
use crate::gotemplates::typedvalue::{
    decode_go_typed_map_value, go_type_is_interface, go_zero_value_for_type,
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
                _ => return Ok(None),
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
    let mut end = 1usize;
    for (offset, ch) in expr[1..].char_indices() {
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
