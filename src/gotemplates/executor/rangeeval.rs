use super::{
    decode_go_typed_map_value, decode_go_typed_slice_value, format_value_for_print, go_bytes_get,
    go_bytes_len, go_string_bytes_len, undefined_variable_error, EvalState, NativeRenderError,
    PipelineDeclMode, PipelineDeclaration,
};
use serde_json::{Number, Value};

pub(super) fn range_items(
    expr: &str,
    source: Option<Value>,
) -> Result<Vec<(Option<Value>, Value)>, NativeRenderError> {
    let Some(value) = source else {
        return Ok(Vec::new());
    };
    if let Some(len) = go_bytes_len(&value) {
        let mut out = Vec::with_capacity(len);
        for idx in 0..len {
            let b =
                go_bytes_get(&value, idx).ok_or_else(|| NativeRenderError::UnsupportedAction {
                    action: format!("{{{{range {expr}}}}}"),
                    reason: "malformed []byte value".to_string(),
                })?;
            out.push((
                Some(Value::Number(Number::from(idx as u64))),
                Value::Number(Number::from(b)),
            ));
        }
        return Ok(out);
    }
    if go_string_bytes_len(&value).is_some() {
        return Err(NativeRenderError::UnsupportedAction {
            action: format!("{{{{range {expr}}}}}"),
            reason: format!(
                "range can't iterate over {}",
                format_value_for_print(&Some(value))
            ),
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
        other => Err(NativeRenderError::UnsupportedAction {
            action: format!("{{{{range {expr}}}}}"),
            reason: format!(
                "range can't iterate over {}",
                format_value_for_print(&Some(other))
            ),
        }),
    }
}

pub(super) fn apply_range_iteration_bindings(
    expr: &str,
    decl: &PipelineDeclaration,
    key: Option<Value>,
    item: &Value,
    state: &mut EvalState,
) -> Result<(), NativeRenderError> {
    let mut bind = |name: &str, value: Option<Value>| -> Result<(), NativeRenderError> {
        match decl.mode {
            PipelineDeclMode::Declare => {
                state.declare_var(name, value);
                Ok(())
            }
            PipelineDeclMode::Assign => {
                if state.assign_var(name, value) {
                    Ok(())
                } else {
                    Err(undefined_variable_error(name))
                }
            }
        }
    };

    match decl.names.len() {
        0 => Ok(()),
        1 => bind(&decl.names[0], Some(item.clone())),
        2 => {
            bind(&decl.names[0], key)?;
            bind(&decl.names[1], Some(item.clone()))
        }
        _ => Err(NativeRenderError::UnsupportedAction {
            action: format!("{{{{range {expr}}}}}"),
            reason: "range declaration supports at most two variables".to_string(),
        }),
    }
}
