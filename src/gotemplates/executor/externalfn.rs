use super::{
    eval_command_token_value, EvalState, NativeFunctionResolver, NativeFunctionResolverError,
    NativeRenderError,
};
use crate::go_compat::externalfn::{
    external_call_failed_reason, is_external_function_identifier,
};
use serde_json::Value;

pub(super) fn try_eval_external_function(
    action: &str,
    name: &str,
    arg_tokens: &[String],
    root: &Value,
    dot: &Value,
    has_pipe_input: bool,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Option<Value>>, NativeRenderError> {
    let Some(resolver) = resolver else {
        return Ok(None);
    };
    if !is_external_function_identifier(name) {
        return Ok(None);
    }

    let mut args = Vec::with_capacity(arg_tokens.len() + usize::from(has_pipe_input));
    for token in arg_tokens {
        args.push(eval_command_token_value(
            action,
            token,
            root,
            dot,
            state,
            Some(resolver),
        )?);
    }
    if has_pipe_input {
        args.push(pipe_input);
    }

    match resolver.call(name, &args) {
        Ok(v) => Ok(Some(v)),
        Err(NativeFunctionResolverError::UnknownFunction) => Ok(None),
        Err(NativeFunctionResolverError::Failed { reason }) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: external_call_failed_reason(name, &reason),
            })
        }
    }
}

pub(super) fn try_eval_dynamic_external_function(
    action: &str,
    tokens: &[String],
    root: &Value,
    dot: &Value,
    has_pipe_input: bool,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Option<Value>>, NativeRenderError> {
    let Some(resolver) = resolver else {
        return Ok(None);
    };
    if tokens.is_empty() || is_external_function_identifier(&tokens[0]) {
        return Ok(None);
    }

    let Some(Value::String(fn_name)) =
        eval_command_token_value(action, &tokens[0], root, dot, state, Some(resolver))?
    else {
        return Ok(None);
    };
    if !is_external_function_identifier(&fn_name) {
        return Ok(None);
    }

    let mut args = Vec::with_capacity(tokens.len().saturating_sub(1) + usize::from(has_pipe_input));
    for token in tokens.iter().skip(1) {
        args.push(eval_command_token_value(
            action,
            token,
            root,
            dot,
            state,
            Some(resolver),
        )?);
    }
    if has_pipe_input {
        args.push(pipe_input);
    }

    match resolver.call(&fn_name, &args) {
        Ok(v) => Ok(Some(v)),
        Err(NativeFunctionResolverError::UnknownFunction) => Ok(None),
        Err(NativeFunctionResolverError::Failed { reason }) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: external_call_failed_reason(&fn_name, &reason),
            })
        }
    }
}
