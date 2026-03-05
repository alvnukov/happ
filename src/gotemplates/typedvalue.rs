use serde_json::{Map, Number, Value};

pub const GO_TYPE_KEY: &str = "__happ_go_type";
pub const GO_VALUE_KEY: &str = "__happ_go_value";
pub const GO_TYPE_BYTES: &str = "[]byte";

pub fn encode_go_bytes_value(bytes: &[u8]) -> Value {
    let mut map = Map::new();
    map.insert(GO_TYPE_KEY.to_string(), Value::String(GO_TYPE_BYTES.to_string()));
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
    let Value::Object(map) = value else {
        return None;
    };
    let kind = map.get(GO_TYPE_KEY)?.as_str()?;
    if kind != GO_TYPE_BYTES {
        return None;
    }
    let arr = map.get(GO_VALUE_KEY)?.as_array()?;
    number_array_to_bytes(arr)
}

fn number_array_to_bytes(items: &[Value]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let n = match item {
            Value::Number(n) => n,
            _ => return None,
        };
        let byte = if let Some(i) = n.as_i64() {
            u8::try_from(i).ok()?
        } else if let Some(u) = n.as_u64() {
            u8::try_from(u).ok()?
        } else {
            return None;
        };
        out.push(byte);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_bytes_roundtrip() {
        let v = encode_go_bytes_value(&[1, 2, 255]);
        assert_eq!(decode_go_bytes_value(&v), Some(vec![1, 2, 255]));
    }
}
