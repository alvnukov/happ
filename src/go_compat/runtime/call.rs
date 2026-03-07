use super::typeutil::value_type_name_for_template;
use super::{
    eval_command_token_value, wrong_number_of_args, EvalState, NativeFunctionResolver,
    NativeFunctionResolverError, NativeRenderError,
};
use crate::go_compat::call::call_target_display;
use crate::go_compat::externalfn::{
    external_call_failed_reason, is_call_builtin_identifier_candidate,
    is_external_function_identifier, undefined_function_reason,
};
use serde_json::Value;

pub(super) fn eval_call_builtin(
    action: &str,
    arg_tokens: &[String],
    root: &Value,
    dot: &Value,
    has_pipe_input: bool,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    if arg_tokens.is_empty() && !has_pipe_input {
        return Err(wrong_number_of_args(action, "call", "at least 1", 0));
    }

    let mut args =
        Vec::with_capacity(arg_tokens.len().saturating_sub(1) + usize::from(has_pipe_input));
    for token in arg_tokens.iter().skip(1) {
        args.push(eval_command_token_value(
            action, token, root, dot, state, resolver,
        )?);
    }

    let first_token = arg_tokens.first().map(String::as_str);
    let first_value = if let Some(first) = first_token {
        // Go parity: keywords `nil`, `true`, `false` are literals, not function names,
        // even when used as first argument of `call`.
        if is_call_builtin_identifier_candidate(first) {
            let Some(resolver) = resolver else {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: undefined_function_reason(first),
                });
            };
            if has_pipe_input {
                args.push(pipe_input);
            }
            return call_named_external_function(action, first, &args, resolver);
        }
        eval_command_token_value(action, first, root, dot, state, resolver)?
    } else if has_pipe_input {
        pipe_input.clone()
    } else {
        None
    };

    if first_token.is_some() && has_pipe_input {
        args.push(pipe_input);
    }

    let Some(value) = first_value else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling call: call of nil".to_string(),
        });
    };
    if value == Value::Null {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling call: call of nil".to_string(),
        });
    }

    if let Value::String(ref name) = value {
        if is_external_function_identifier(name) {
            if let Some(resolver) = resolver {
                return call_named_external_function(action, name, &args, resolver);
            }
        }
    }

    let target = call_target_display(first_token, &value);
    Err(NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: format!(
            "error calling call: non-function {target} of type {}",
            value_type_name_for_template(&value)
        ),
    })
}

fn call_named_external_function(
    action: &str,
    name: &str,
    args: &[Option<Value>],
    resolver: &dyn NativeFunctionResolver,
) -> Result<Option<Value>, NativeRenderError> {
    match resolver.call(name, args) {
        Ok(v) => Ok(v),
        Err(NativeFunctionResolverError::UnknownFunction) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: undefined_function_reason(name),
            })
        }
        Err(NativeFunctionResolverError::Failed { reason }) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: external_call_failed_reason(name, &reason),
            })
        }
    }
}
