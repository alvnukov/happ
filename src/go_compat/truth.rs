use crate::gotemplates::typedvalue::{
    decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_len, go_string_bytes_len,
};
use serde_json::Value;

pub fn is_truthy(v: Option<&Value>) -> bool {
    let Some(value) = v else {
        return false;
    };
    if let Some(len) = go_bytes_len(value).or_else(|| go_string_bytes_len(value)) {
        return len > 0;
    }
    if let Some(typed_map) = decode_go_typed_map_value(value) {
        return typed_map.entries.is_some_and(|entries| !entries.is_empty());
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(value) {
        return typed_slice.items.is_some_and(|items| !items.is_empty());
    }
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            n.as_i64().is_some_and(|i| i != 0)
                || n.as_u64().is_some_and(|u| u != 0)
                || n.as_f64().is_some_and(|f| f != 0.0)
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn builtin_and(args: &[Option<Value>]) -> Option<Value> {
    if args.is_empty() {
        return None;
    }
    for arg in args {
        if !is_truthy(arg.as_ref()) {
            return arg.clone();
        }
    }
    args.last().cloned().unwrap_or(None)
}

pub fn builtin_or(args: &[Option<Value>]) -> Option<Value> {
    if args.is_empty() {
        return None;
    }
    for arg in args {
        if is_truthy(arg.as_ref()) {
            return arg.clone();
        }
    }
    args.last().cloned().unwrap_or(None)
}

#[cfg(test)]
mod tests {
    use super::{builtin_and, builtin_or, is_truthy};
    use serde_json::{json, Value};

    #[test]
    fn truthiness_for_plain_json_values() {
        assert!(!is_truthy(None));
        assert!(!is_truthy(Some(&Value::Null)));
        assert!(!is_truthy(Some(&json!(0))));
        assert!(!is_truthy(Some(&json!(""))));
        assert!(!is_truthy(Some(&json!([]))));
        assert!(!is_truthy(Some(&json!({}))));
        assert!(is_truthy(Some(&json!(1))));
        assert!(is_truthy(Some(&json!("x"))));
        assert!(is_truthy(Some(&json!([1]))));
        assert!(is_truthy(Some(&json!({"x":1}))));
    }

    #[test]
    fn and_or_return_short_circuit_argument() {
        let args = vec![Some(json!(1)), Some(json!(0)), Some(json!(2))];
        assert_eq!(builtin_and(&args), Some(json!(0)));
        assert_eq!(builtin_or(&args), Some(json!(1)));

        let all_truthy = vec![Some(json!("x")), Some(json!(1))];
        assert_eq!(builtin_and(&all_truthy), Some(json!(1)));
        assert_eq!(builtin_or(&all_truthy), Some(json!("x")));
    }

    #[test]
    fn truthiness_for_typed_nil_and_non_empty_collections() {
        let nil_map = crate::gotemplates::encode_go_typed_map_value("string", None);
        let mut entries = serde_json::Map::new();
        entries.insert(String::from("k"), json!("v"));
        let map = crate::gotemplates::encode_go_typed_map_value("string", Some(entries));
        let nil_slice = crate::gotemplates::encode_go_typed_slice_value("int", None);
        let slice =
            crate::gotemplates::encode_go_typed_slice_value("int", Some(vec![json!(1)]));

        assert!(!is_truthy(Some(&nil_map)));
        assert!(is_truthy(Some(&map)));
        assert!(!is_truthy(Some(&nil_slice)));
        assert!(is_truthy(Some(&slice)));
    }
}
