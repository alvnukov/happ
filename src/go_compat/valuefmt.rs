use crate::go_compat::typedvalue::{
    decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
    decode_go_typed_slice_value,
};
use serde_json::Value;

// Go parity reference: stdlib text/template/exec.go printableValue + fmt default rendering.
pub fn format_value_like_go(v: &Value) -> String {
    if let Some(bytes) = decode_go_bytes_value(v) {
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
    if let Some(bytes) = decode_go_string_bytes_value(v) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(v) {
        let mut out = String::from("[");
        if let Some(items) = typed_slice.items {
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format_value_like_go(item));
            }
        }
        out.push(']');
        return out;
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format_map_entries_like_go(typed_map.entries);
    }
    match v {
        Value::Null => "<no value>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
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

#[cfg(test)]
mod tests {
    use super::format_value_like_go;
    use serde_json::{json, Value};

    #[test]
    fn formats_plain_json_values() {
        assert_eq!(format_value_like_go(&Value::Null), "<no value>");
        assert_eq!(format_value_like_go(&json!(true)), "true");
        assert_eq!(format_value_like_go(&json!(12)), "12");
        assert_eq!(format_value_like_go(&json!("x")), "x");
        assert_eq!(format_value_like_go(&json!([1, 2])), "[1 2]");
    }

    #[test]
    fn formats_object_keys_in_sorted_order() {
        let v = json!({"b":2,"a":1});
        assert_eq!(format_value_like_go(&v), "map[a:1 b:2]");
    }

    #[test]
    fn formats_go_typed_bytes_and_string_bytes() {
        let bytes = crate::go_compat::typedvalue::encode_go_bytes_value(&[1, 2, 3]);
        assert_eq!(format_value_like_go(&bytes), "[1 2 3]");

        let sbytes = crate::go_compat::typedvalue::encode_go_string_bytes_value(b"abc");
        assert_eq!(format_value_like_go(&sbytes), "abc");
    }
}
