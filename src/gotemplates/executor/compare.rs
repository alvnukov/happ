use super::{wrong_number_of_args, NativeRenderError};
use super::typeutil::{
    format_non_comparable_type_reason, format_non_comparable_types_reason,
    is_go_bytes_slice_option, is_map_object_option, non_comparable_kind_option,
    option_string_like_bytes,
};
use serde_json::Value;

pub(super) fn builtin_eq(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "eq", "at least 1", 0));
    }
    if args.len() == 1 {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling eq: missing argument for comparison".to_string(),
        });
    }
    let head = &args[0];
    for other in args.iter().skip(1) {
        if compare_eq(action, head, other)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn builtin_cmp(
    action: &str,
    fn_name: &str,
    args: &[Option<Value>],
    pred: impl Fn(std::cmp::Ordering) -> bool,
) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, fn_name, "2", args.len()));
    }
    let ord = match compare_ordering(action, &args[0], &args[1]) {
        Ok(ord) => ord,
        Err(NativeRenderError::UnsupportedAction { reason, .. }) => {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("error calling {fn_name}: {reason}"),
            });
        }
        Err(other) => return Err(other),
    };
    Ok(pred(ord))
}

pub(super) fn builtin_ne(action: &str, args: &[Option<Value>]) -> Result<bool, NativeRenderError> {
    if args.len() != 2 {
        return Err(wrong_number_of_args(action, "ne", "2", args.len()));
    }
    Ok(!compare_eq(action, &args[0], &args[1])?)
}

fn compare_eq(
    action: &str,
    a: &Option<Value>,
    b: &Option<Value>,
) -> Result<bool, NativeRenderError> {
    if let (Some(sa), Some(sb)) = (option_string_like_bytes(a), option_string_like_bytes(b)) {
        return Ok(sa.as_ref() == sb.as_ref());
    }
    match (a, b) {
        (None, None) => Ok(true),
        (None, Some(Value::Null)) | (Some(Value::Null), None) => Ok(true),
        (Some(Value::Null), Some(Value::Null)) => Ok(true),
        (Some(Value::Bool(av)), Some(Value::Bool(bv))) => Ok(av == bv),
        (Some(Value::String(av)), Some(Value::String(bv))) => Ok(av == bv),
        (Some(Value::Number(_)), Some(Value::Number(_))) => compare_number_eq(action, a, b),
        (None, _) | (_, None) | (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(false),
        _ => match (non_comparable_kind_option(a), non_comparable_kind_option(b)) {
            (Some(ka), Some(kb)) if ka == kb => Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format_non_comparable_type_reason(b),
            }),
            (Some(_), Some(_)) => Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format_non_comparable_types_reason(a, b),
            }),
            _ => Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "error calling eq: incompatible types for comparison".to_string(),
            }),
        },
    }
}

fn compare_ordering(
    action: &str,
    a: &Option<Value>,
    b: &Option<Value>,
) -> Result<std::cmp::Ordering, NativeRenderError> {
    let av = a.as_ref();
    let bv = b.as_ref();
    if let (Some(na), Some(nb)) = (number_kind(a), number_kind(b)) {
        return compare_number_ordering(action, na, nb);
    }
    if let (Some(sa), Some(sb)) = (option_string_like_bytes(a), option_string_like_bytes(b)) {
        return Ok(sa.as_ref().cmp(sb.as_ref()));
    }

    let same_kind = matches!(
        (av, bv),
        (Some(Value::Bool(_)), Some(Value::Bool(_)))
            | (Some(Value::Array(_)), Some(Value::Array(_)))
            | (Some(Value::Null), Some(Value::Null))
            | (None, None)
            | (None, Some(Value::Null))
            | (Some(Value::Null), None)
    ) || (is_go_bytes_slice_option(a) && is_go_bytes_slice_option(b))
        || (is_map_object_option(a) && is_map_object_option(b));
    if same_kind {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "invalid type for comparison".to_string(),
        });
    }

    Err(NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: "incompatible types for comparison".to_string(),
    })
}

#[derive(Debug, Clone, Copy)]
enum NumberKind {
    Int(i64),
    Uint(u64),
    Float(f64),
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

fn compare_number_eq(
    action: &str,
    a: &Option<Value>,
    b: &Option<Value>,
) -> Result<bool, NativeRenderError> {
    let Some(na) = number_kind(a) else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling eq: incompatible types for comparison".to_string(),
        });
    };
    let Some(nb) = number_kind(b) else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling eq: incompatible types for comparison".to_string(),
        });
    };
    match (na, nb) {
        (NumberKind::Int(x), NumberKind::Int(y)) => Ok(x == y),
        (NumberKind::Uint(x), NumberKind::Uint(y)) => Ok(x == y),
        (NumberKind::Float(x), NumberKind::Float(y)) => Ok(x == y),
        (NumberKind::Int(x), NumberKind::Uint(y)) => Ok(x >= 0 && (x as u64) == y),
        (NumberKind::Uint(x), NumberKind::Int(y)) => Ok(y >= 0 && x == (y as u64)),
        _ => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling eq: incompatible types for comparison".to_string(),
        }),
    }
}

fn compare_number_ordering(
    action: &str,
    a: NumberKind,
    b: NumberKind,
) -> Result<std::cmp::Ordering, NativeRenderError> {
    use std::cmp::Ordering;
    let ord = match (a, b) {
        (NumberKind::Int(x), NumberKind::Int(y)) => x.cmp(&y),
        (NumberKind::Uint(x), NumberKind::Uint(y)) => x.cmp(&y),
        (NumberKind::Float(x), NumberKind::Float(y)) => {
            x.partial_cmp(&y)
                .ok_or_else(|| NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "comparison failed".to_string(),
                })?
        }
        (NumberKind::Int(x), NumberKind::Uint(y)) => {
            if x < 0 {
                Ordering::Less
            } else {
                (x as u64).cmp(&y)
            }
        }
        (NumberKind::Uint(x), NumberKind::Int(y)) => {
            if y < 0 {
                Ordering::Greater
            } else {
                x.cmp(&(y as u64))
            }
        }
        _ => {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "incompatible types for comparison".to_string(),
            });
        }
    };
    Ok(ord)
}
