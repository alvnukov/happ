use super::intfmt::GoInteger;
use crate::gotemplates::typedvalue::{decode_go_bytes_value, decode_go_string_bytes_value};
use serde_json::{Number, Value};

pub(super) fn value_to_bool(v: &Option<Value>) -> Option<bool> {
    v.as_ref().and_then(Value::as_bool)
}

pub(super) fn value_to_integer_go(v: &Option<Value>) -> Option<GoInteger> {
    let Some(Value::Number(n)) = v.as_ref() else {
        return None;
    };
    if let Some(i) = n.as_i64() {
        return Some(GoInteger {
            raw: i as u64,
            signed: true,
        });
    }
    n.as_u64().map(|u| GoInteger {
        raw: u,
        signed: false,
    })
}

pub(super) fn value_to_f64(v: &Option<Value>) -> Option<f64> {
    match v.as_ref() {
        Some(Value::Number(n)) if n.is_f64() => n.as_f64(),
        _ => None,
    }
}

pub(super) fn value_to_rune_go(v: &Option<Value>) -> Option<char> {
    let Some(Value::Number(n)) = v.as_ref() else {
        return None;
    };
    value_number_to_rune_go(n)
}

pub(super) fn value_number_to_rune_go(n: &Number) -> Option<char> {
    let raw = if let Some(i) = n.as_i64() {
        i as i128
    } else if let Some(u) = n.as_u64() {
        u as i128
    } else {
        return None;
    };
    let code = if (0..=0x10FFFF).contains(&raw) {
        raw as u32
    } else {
        0xFFFD
    };
    Some(char::from_u32(code).unwrap_or('\u{FFFD}'))
}

pub(super) fn value_to_int_for_width_prec(v: &Option<Value>) -> Option<i64> {
    match v.as_ref() {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        _ => None,
    }
}

pub(super) fn value_to_u64_for_unicode(v: &Option<Value>) -> Option<u64> {
    match v.as_ref() {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Some(i as u64)
            } else {
                n.as_u64()
            }
        }
        _ => None,
    }
}

pub(super) fn value_to_byte_slice(v: &Option<Value>) -> Option<Vec<u8>> {
    let Some(value) = v.as_ref() else {
        return None;
    };
    decode_go_bytes_value(value)
}

pub(super) fn value_to_string_bytes(v: &Option<Value>) -> Option<Vec<u8>> {
    let Some(value) = v.as_ref() else {
        return None;
    };
    decode_go_string_bytes_value(value)
}

pub(super) fn value_as_byte_slice(v: &Value) -> Option<Vec<u8>> {
    decode_go_bytes_value(v)
}

pub(super) fn value_as_string_bytes(v: &Value) -> Option<Vec<u8>> {
    decode_go_string_bytes_value(v)
}
