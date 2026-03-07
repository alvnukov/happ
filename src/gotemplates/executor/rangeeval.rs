use super::{
    undefined_variable_error, EvalState, NativeRenderError, PipelineDeclMode, PipelineDeclaration,
};
use crate::go_compat::rangeeval::{
    range_items as go_range_items, RangeItemsError,
};
use serde_json::Value;

pub(super) fn range_items(
    expr: &str,
    source: Option<Value>,
) -> Result<Vec<(Option<Value>, Value)>, NativeRenderError> {
    go_range_items(source).map_err(|err| match err {
        RangeItemsError::MalformedBytes => NativeRenderError::UnsupportedAction {
            action: format!("{{{{range {expr}}}}}"),
            reason: "malformed []byte value".to_string(),
        },
        RangeItemsError::CannotIterate { rendered } => NativeRenderError::UnsupportedAction {
            action: format!("{{{{range {expr}}}}}"),
            reason: format!("range can't iterate over {rendered}"),
        },
    })
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
