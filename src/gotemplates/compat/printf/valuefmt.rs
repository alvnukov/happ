use serde_json::{Number, Value};

use crate::gotemplates::typedvalue::{
    decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value, go_bytes_is_nil,
};

pub(super) fn format_value_for_printf(v: &Option<Value>, verb: char, sharp: bool) -> String {
    match (verb, v) {
        (_, None) | (_, Some(Value::Null)) => "<nil>".to_string(),
        ('s', Some(Value::String(s))) => s.clone(),
        ('s', Some(value)) => {
            if let Some(bytes) = value_as_string_bytes(value) {
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                format_value_like_go(value)
            }
        }
        ('v', Some(value)) => {
            if sharp {
                format_value_go_syntax(value)
            } else {
                format_value_like_go(value)
            }
        }
        (_, Some(Value::String(s))) => s.clone(),
        (_, Some(value)) => format_value_like_go(value),
    }
}

pub(super) fn format_type_for_printf(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => printf_type_name(other),
    }
}

pub(super) fn printf_type_name(v: &Value) -> String {
    if value_as_byte_slice(v).is_some() {
        return "[]uint8".to_string();
    }
    if value_as_string_bytes(v).is_some() {
        return "string".to_string();
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format!("map[string]{}", typed_map.elem_type);
    }
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => format!("[]{}", infer_slice_type_for_sharp_v(items)),
        Value::Object(map) => {
            let ty = infer_map_value_type_for_sharp_v(map.values());
            format!("map[string]{ty}")
        }
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

pub(super) fn format_value_like_go(v: &Value) -> String {
    if let Some(bytes) = value_as_byte_slice(v) {
        let mut out = String::from("[");
        for (idx, b) in bytes.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            out.push_str(&b.to_string());
        }
        out.push(']');
        return out;
    }
    if let Some(bytes) = value_as_string_bytes(v) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format_map_entries_like_go(typed_map.entries);
    }
    match v {
        Value::Null => "<no value>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number_like_go(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = String::from("[");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format_value_like_go(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => format_map_entries_like_go(Some(map)),
    }
}

fn format_value_go_syntax(v: &Value) -> String {
    if let Some(bytes) = value_as_byte_slice(v) {
        if go_bytes_is_nil(v) {
            return "[]byte(nil)".to_string();
        }
        let mut out = String::from("[]byte{");
        for (idx, b) in bytes.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("0x{:x}", b));
        }
        out.push('}');
        return out;
    }
    if let Some(bytes) = value_as_string_bytes(v) {
        if let Ok(s) = std::str::from_utf8(&bytes) {
            return format!("{s:?}");
        }
        return quote_bytes_go_string_literal(&bytes);
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        let val_ty = typed_map.elem_type;
        let Some(entries) = typed_map.entries else {
            return format!("map[string]{val_ty}(nil)");
        };
        return format_map_entries_go_syntax(entries, val_ty);
    }
    match v {
        Value::Null => "interface {}(nil)".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number_sharp_v_go(n),
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => {
            let slice_ty = infer_slice_type_for_sharp_v(items);
            let mut out = format!("[]{slice_ty}{{");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_go_syntax_item(item, slice_ty));
            }
            out.push('}');
            out
        }
        Value::Object(map) => {
            format_map_entries_go_syntax(map, infer_map_value_type_for_sharp_v(map.values()))
        }
    }
}

fn format_map_entries_like_go(entries: Option<&serde_json::Map<String, Value>>) -> String {
    let mut out = String::from("map[");
    if let Some(map) = entries {
        let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        for (idx, k) in keys.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            out.push_str(k);
            out.push(':');
            if let Some(v) = map.get(*k) {
                out.push_str(&format_value_like_go(v));
            }
        }
    }
    out.push(']');
    out
}

fn format_map_entries_go_syntax(map: &serde_json::Map<String, Value>, val_ty: &str) -> String {
    let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    let mut out = format!("map[string]{val_ty}{{");
    for (idx, k) in keys.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("{k:?}:"));
        if let Some(v) = map.get(*k) {
            out.push_str(&format_value_go_syntax_item(v, val_ty));
        } else {
            out.push_str("interface {}(nil)");
        }
    }
    out.push('}');
    out
}

fn infer_slice_type_for_sharp_v(items: &[Value]) -> &'static str {
    if items.is_empty() {
        return "interface {}";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::String(_)) || value_as_string_bytes(item).is_some())
    {
        return "string";
    }
    if items.iter().all(|item| matches!(item, Value::Bool(_))) {
        return "bool";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_i64().is_some()))
    {
        return "int";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_u64().is_some()))
    {
        return "uint";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_f64().is_some()))
    {
        return "float64";
    }
    "interface {}"
}

fn infer_map_value_type_for_sharp_v<'a>(values: impl Iterator<Item = &'a Value>) -> &'static str {
    let vals: Vec<&Value> = values.collect();
    if vals.is_empty() {
        return "interface {}";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::String(_)) || value_as_string_bytes(v).is_some())
    {
        return "string";
    }
    if vals.iter().all(|v| matches!(v, Value::Bool(_))) {
        return "bool";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_i64().is_some()))
    {
        return "int";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_u64().is_some()))
    {
        return "uint";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_f64().is_some()))
    {
        return "float64";
    }
    "interface {}"
}

fn format_value_go_syntax_item(v: &Value, ty: &str) -> String {
    if ty == "interface {}" {
        return format_value_go_syntax(v);
    }
    if ty == "string" {
        if let Value::String(s) = v {
            return format!("{s:?}");
        }
        if let Some(bytes) = value_as_string_bytes(v) {
            if let Ok(s) = std::str::from_utf8(&bytes) {
                return format!("{s:?}");
            }
            return quote_bytes_go_string_literal(&bytes);
        }
    }
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if ty == "float64" {
                format_number_sharp_v_go(n)
            } else if ty == "uint" {
                if let Some(u) = n.as_u64() {
                    format!("0x{u:x}")
                } else {
                    n.to_string()
                }
            } else {
                n.to_string()
            }
        }
        Value::Null => "nil".to_string(),
        _ => format_value_go_syntax(v),
    }
}

fn quote_bytes_go_string_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    out.push('"');
    for &b in bytes {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x0C => out.push_str("\\f"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x0B => out.push_str("\\v"),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

fn format_number_sharp_v_go(n: &Number) -> String {
    if n.as_i64().is_some() {
        return n.to_string();
    }
    if let Some(u) = n.as_u64() {
        return format!("0x{u:x}");
    }
    if let Some(f) = n.as_f64() {
        return super::floatfmt::format_float_general_go_default(f, false);
    }
    n.to_string()
}

fn format_number_like_go(n: &Number) -> String {
    if n.as_i64().is_some() || n.as_u64().is_some() {
        return n.to_string();
    }
    if let Some(f) = n.as_f64() {
        return super::floatfmt::format_float_general_go_default(f, false);
    }
    n.to_string()
}

fn value_as_byte_slice(v: &Value) -> Option<Vec<u8>> {
    decode_go_bytes_value(v)
}

fn value_as_string_bytes(v: &Value) -> Option<Vec<u8>> {
    decode_go_string_bytes_value(v)
}
