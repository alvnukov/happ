use serde_json::{Map, Number, Value};

pub const GO_TYPE_KEY: &str = "__happ_go_type";
pub const GO_VALUE_KEY: &str = "__happ_go_value";
pub const GO_TYPE_BYTES: &str = "[]byte";
pub const GO_TYPE_STRING_BYTES: &str = "string-bytes";
pub const GO_TYPE_MAP_PREFIX: &str = "map[string]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoTypedMapRef<'a> {
    pub elem_type: &'a str,
    pub entries: Option<&'a Map<String, Value>>,
}

pub fn encode_go_bytes_value(bytes: &[u8]) -> Value {
    let mut map = Map::new();
    map.insert(
        GO_TYPE_KEY.to_string(),
        Value::String(GO_TYPE_BYTES.to_string()),
    );
    map.insert(
        GO_VALUE_KEY.to_string(),
        Value::Array(
            bytes
                .iter()
                .map(|b| Value::Number(Number::from(*b)))
                .collect(),
        ),
    );
    Value::Object(map)
}

pub fn decode_go_bytes_value(value: &Value) -> Option<Vec<u8>> {
    number_array_to_bytes(typed_number_array(value, GO_TYPE_BYTES)?)
}

pub fn go_bytes_len(value: &Value) -> Option<usize> {
    let items = typed_number_array(value, GO_TYPE_BYTES)?;
    for item in items {
        number_value_to_byte(item)?;
    }
    Some(items.len())
}

pub fn go_bytes_get(value: &Value, index: usize) -> Option<u8> {
    let item = typed_number_array(value, GO_TYPE_BYTES)?.get(index)?;
    number_value_to_byte(item)
}

pub fn encode_go_string_bytes_value(bytes: &[u8]) -> Value {
    let mut map = Map::new();
    map.insert(
        GO_TYPE_KEY.to_string(),
        Value::String(GO_TYPE_STRING_BYTES.to_string()),
    );
    map.insert(
        GO_VALUE_KEY.to_string(),
        Value::Array(
            bytes
                .iter()
                .map(|b| Value::Number(Number::from(*b)))
                .collect(),
        ),
    );
    Value::Object(map)
}

pub fn decode_go_string_bytes_value(value: &Value) -> Option<Vec<u8>> {
    number_array_to_bytes(typed_number_array(value, GO_TYPE_STRING_BYTES)?)
}

pub fn go_string_bytes_len(value: &Value) -> Option<usize> {
    let items = typed_number_array(value, GO_TYPE_STRING_BYTES)?;
    for item in items {
        number_value_to_byte(item)?;
    }
    Some(items.len())
}

pub fn go_string_bytes_get(value: &Value, index: usize) -> Option<u8> {
    let item = typed_number_array(value, GO_TYPE_STRING_BYTES)?.get(index)?;
    number_value_to_byte(item)
}

pub fn encode_go_typed_map_value(elem_type: &str, entries: Option<Map<String, Value>>) -> Value {
    let mut map = Map::new();
    map.insert(
        GO_TYPE_KEY.to_string(),
        Value::String(format!("{GO_TYPE_MAP_PREFIX}{elem_type}")),
    );
    map.insert(
        GO_VALUE_KEY.to_string(),
        match entries {
            Some(entries) => Value::Object(entries),
            None => Value::Null,
        },
    );
    Value::Object(map)
}

pub fn decode_go_typed_map_value(value: &Value) -> Option<GoTypedMapRef<'_>> {
    let Value::Object(map) = value else {
        return None;
    };
    let kind = map.get(GO_TYPE_KEY)?.as_str()?;
    let elem_type = kind.strip_prefix(GO_TYPE_MAP_PREFIX)?;
    if elem_type.is_empty() {
        return None;
    }
    let payload = map.get(GO_VALUE_KEY)?;
    let entries = match payload {
        Value::Object(entries) => Some(entries),
        Value::Null => None,
        _ => return None,
    };
    Some(GoTypedMapRef { elem_type, entries })
}

pub fn go_type_is_interface(type_name: &str) -> bool {
    matches!(type_name.trim(), "interface {}" | "interface{}" | "any")
}

pub fn go_zero_value_for_type(type_name: &str) -> Value {
    let kind = type_name.trim();
    if kind.is_empty() {
        return Value::Null;
    }
    if let Some(elem_type) = kind.strip_prefix(GO_TYPE_MAP_PREFIX) {
        return encode_go_typed_map_value(elem_type, None);
    }
    if kind == "[]byte" || kind == "[]uint8" {
        return encode_go_bytes_value(&[]);
    }
    if kind.starts_with("[]") {
        return Value::Array(Vec::new());
    }
    match kind {
        "bool" => Value::Bool(false),
        "string" => Value::String(String::new()),
        "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16" | "uint32"
        | "uint64" | "uintptr" | "byte" | "rune" => Value::Number(Number::from(0)),
        "float32" | "float64" => {
            Value::Number(Number::from_f64(0.0).unwrap_or_else(|| Number::from(0)))
        }
        "interface {}" | "interface{}" | "any" => Value::Null,
        _ => Value::Null,
    }
}

fn typed_number_array<'a>(value: &'a Value, expected_type: &str) -> Option<&'a [Value]> {
    let Value::Object(map) = value else {
        return None;
    };
    let kind = map.get(GO_TYPE_KEY)?.as_str()?;
    if kind != expected_type {
        return None;
    }
    map.get(GO_VALUE_KEY)?.as_array().map(Vec::as_slice)
}

fn number_array_to_bytes(items: &[Value]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(number_value_to_byte(item)?);
    }
    Some(out)
}

fn number_value_to_byte(item: &Value) -> Option<u8> {
    let n = match item {
        Value::Number(n) => n,
        _ => return None,
    };
    if let Some(i) = n.as_i64() {
        u8::try_from(i).ok()
    } else if let Some(u) = n.as_u64() {
        u8::try_from(u).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_bytes_roundtrip() {
        let v = encode_go_bytes_value(&[1, 2, 255]);
        assert_eq!(decode_go_bytes_value(&v), Some(vec![1, 2, 255]));
    }

    #[test]
    fn go_bytes_len_and_get_work_without_full_decode() {
        let v = encode_go_bytes_value(&[10, 20, 30]);
        assert_eq!(go_bytes_len(&v), Some(3));
        assert_eq!(go_bytes_get(&v, 0), Some(10));
        assert_eq!(go_bytes_get(&v, 2), Some(30));
        assert_eq!(go_bytes_get(&v, 3), None);
    }

    #[test]
    fn go_bytes_helpers_reject_invalid_payload() {
        let mut map = Map::new();
        map.insert(
            GO_TYPE_KEY.to_string(),
            Value::String(GO_TYPE_BYTES.to_string()),
        );
        map.insert(
            GO_VALUE_KEY.to_string(),
            Value::Array(vec![Value::String("x".to_string())]),
        );
        let v = Value::Object(map);
        assert_eq!(go_bytes_len(&v), None);
        assert_eq!(go_bytes_get(&v, 0), None);
        assert_eq!(decode_go_bytes_value(&v), None);
    }

    #[test]
    fn go_string_bytes_roundtrip() {
        let v = encode_go_string_bytes_value(&[0x61, 0x97]);
        assert_eq!(decode_go_string_bytes_value(&v), Some(vec![0x61, 0x97]));
        assert_eq!(go_string_bytes_len(&v), Some(2));
        assert_eq!(go_string_bytes_get(&v, 0), Some(0x61));
        assert_eq!(go_string_bytes_get(&v, 1), Some(0x97));
        assert_eq!(go_string_bytes_get(&v, 2), None);
    }

    #[test]
    fn go_typed_map_roundtrip_supports_nil_and_non_nil() {
        let mut entries = Map::new();
        entries.insert("a".to_string(), Value::Number(Number::from(1)));
        let non_nil = encode_go_typed_map_value("int", Some(entries));
        let decoded = decode_go_typed_map_value(&non_nil).expect("typed map must decode");
        assert_eq!(decoded.elem_type, "int");
        assert_eq!(
            decoded
                .entries
                .and_then(|m| m.get("a"))
                .and_then(Value::as_i64),
            Some(1)
        );

        let nil_map = encode_go_typed_map_value("int", None);
        let decoded = decode_go_typed_map_value(&nil_map).expect("typed map must decode");
        assert_eq!(decoded.elem_type, "int");
        assert!(decoded.entries.is_none());
    }

    #[test]
    fn go_zero_value_for_type_supports_map_and_primitives() {
        assert_eq!(
            go_zero_value_for_type("int"),
            Value::Number(Number::from(0))
        );
        assert_eq!(go_zero_value_for_type("bool"), Value::Bool(false));
        assert_eq!(
            go_zero_value_for_type("string"),
            Value::String(String::new())
        );
        assert_eq!(
            go_zero_value_for_type("float64").as_f64(),
            Some(0.0),
            "float zero must preserve float kind"
        );
        assert!(matches!(
            go_zero_value_for_type("interface {}"),
            Value::Null
        ));
        assert!(matches!(go_zero_value_for_type("interface{}"), Value::Null));
        let typed_map = go_zero_value_for_type("map[string]int");
        let decoded = decode_go_typed_map_value(&typed_map).expect("typed map must decode");
        assert_eq!(decoded.elem_type, "int");
        assert!(decoded.entries.is_none());
    }
}
