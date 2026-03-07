use crate::go_compat::valuefmt::format_value_like_go as go_format_value_like_go;
use serde_json::Value;

pub(super) fn format_value_like_go(v: &Value) -> String {
    go_format_value_like_go(v)
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
        let bytes = crate::gotemplates::encode_go_bytes_value(&[1, 2, 3]);
        assert_eq!(format_value_like_go(&bytes), "[1 2 3]");

        let sbytes = crate::gotemplates::encode_go_string_bytes_value(b"abc");
        assert_eq!(format_value_like_go(&sbytes), "abc");
    }
}
