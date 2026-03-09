use crate::go_compat::typedvalue::{
    decode_go_typed_map_value, decode_go_typed_slice_value, go_bytes_get, go_bytes_len,
    go_string_bytes_len,
};
use crate::go_compat::valuefmt::format_value_like_go;
use serde_json::{Number, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeItemsError {
    MalformedBytes,
    CannotIterate { rendered: String },
}

pub fn range_items(source: Option<Value>) -> Result<Vec<(Option<Value>, Value)>, RangeItemsError> {
    let Some(value) = source else {
        return Ok(Vec::new());
    };
    if let Value::Number(n) = &value {
        if let Some(i) = n.as_i64() {
            if i <= 0 {
                return Ok(Vec::new());
            }
            let len = usize::try_from(i).map_err(|_| RangeItemsError::CannotIterate {
                rendered: format_value_like_go(&value),
            })?;
            let mut out = Vec::with_capacity(len);
            for idx in 0..len {
                let n = Number::from(idx as u64);
                let v = Value::Number(n.clone());
                out.push((Some(Value::Number(n)), v));
            }
            return Ok(out);
        }
        if let Some(u) = n.as_u64() {
            let len = usize::try_from(u).map_err(|_| RangeItemsError::CannotIterate {
                rendered: format_value_like_go(&value),
            })?;
            let mut out = Vec::with_capacity(len);
            for idx in 0..len {
                let n = Number::from(idx as u64);
                let v = Value::Number(n.clone());
                out.push((Some(Value::Number(n)), v));
            }
            return Ok(out);
        }
    }
    if let Some(len) = go_bytes_len(&value) {
        let mut out = Vec::with_capacity(len);
        for idx in 0..len {
            let b = go_bytes_get(&value, idx).ok_or(RangeItemsError::MalformedBytes)?;
            out.push((
                Some(Value::Number(Number::from(idx as u64))),
                Value::Number(Number::from(b)),
            ));
        }
        return Ok(out);
    }
    if go_string_bytes_len(&value).is_some() {
        return Err(RangeItemsError::CannotIterate {
            rendered: format_value_like_go(&value),
        });
    }
    if let Some(typed_map) = decode_go_typed_map_value(&value) {
        let Some(entries) = typed_map.entries else {
            return Ok(Vec::new());
        };
        let mut keys: Vec<String> = entries.keys().cloned().collect();
        keys.sort_unstable();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(v) = entries.get(&key) {
                out.push((Some(Value::String(key)), v.clone()));
            }
        }
        return Ok(out);
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(&value) {
        let Some(items) = typed_slice.items else {
            return Ok(Vec::new());
        };
        return Ok(items
            .iter()
            .cloned()
            .enumerate()
            .map(|(idx, v)| (Some(Value::Number(Number::from(idx as u64))), v))
            .collect());
    }
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => Ok(items
            .into_iter()
            .enumerate()
            .map(|(idx, v)| (Some(Value::Number(Number::from(idx as u64))), v))
            .collect()),
        Value::Object(map) => {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort_unstable();
            let mut out = Vec::with_capacity(keys.len());
            for key in keys {
                if let Some(v) = map.get(&key) {
                    out.push((Some(Value::String(key)), v.clone()));
                }
            }
            Ok(out)
        }
        other => Err(RangeItemsError::CannotIterate {
            rendered: format_value_like_go(&other),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{range_items, RangeItemsError};
    use serde_json::{json, Value};

    #[test]
    fn range_items_orders_maps_and_indexes_arrays() {
        let map = json!({"b":2,"a":1});
        let out = range_items(Some(map)).expect("range");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (Some(Value::String("a".to_string())), json!(1)));
        assert_eq!(out[1], (Some(Value::String("b".to_string())), json!(2)));

        let arr = json!(["x", "y"]);
        let out = range_items(Some(arr)).expect("range");
        assert_eq!(out[0], (Some(json!(0)), json!("x")));
        assert_eq!(out[1], (Some(json!(1)), json!("y")));
    }

    #[test]
    fn range_items_supports_integer_values_like_go() {
        let out = range_items(Some(json!(3))).expect("must range");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], (Some(json!(0)), json!(0)));
        assert_eq!(out[1], (Some(json!(1)), json!(1)));
        assert_eq!(out[2], (Some(json!(2)), json!(2)));

        let out = range_items(Some(json!(-2))).expect("must range");
        assert!(out.is_empty());
    }

    #[test]
    fn range_items_rejects_non_iterable_values() {
        let err = range_items(Some(json!(1.5))).expect_err("must fail");
        assert_eq!(
            err,
            RangeItemsError::CannotIterate {
                rendered: "1.5".to_string()
            }
        );
    }
}
