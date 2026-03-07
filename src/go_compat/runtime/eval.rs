use super::*;
use crate::go_compat::evaldiag::{
    cannot_give_argument_to_non_function_reason, empty_command_in_pipeline_reason,
    empty_pipeline_reason, field_not_method_has_arguments_reason, illegal_number_syntax_reason,
    invalid_syntax_reason, is_nil_command, multi_variable_decl_in_non_range_reason,
    nil_is_not_a_command_reason, non_executable_pipeline_stage_reason,
};
use crate::go_compat::externalfn::undefined_function_reason;

// Go parity reference: stdlib text/template/exec.go expression and pipeline evaluation.
pub(super) fn eval_expr_truthy(
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<bool, NativeRenderError> {
    let val = eval_expr_value(expr, root, dot, state, resolver)?;
    Ok(is_truthy(&val))
}

pub(super) fn eval_expr_value(
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    eval_expr_value_result("", expr, root, dot, state, resolver)
}

fn eval_expr_value_result(
    action: &str,
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    // Go parity (text/template exec): bare `nil` is parsed via pipeline path and
    // later surfaced as "nil is not a command" in output contexts.
    if is_nil_command(expr) {
        return eval_pipeline_expr(action, expr, root, dot, state, resolver);
    }
    if is_complex_expression(expr) || is_niladic_function_expression(expr) {
        return eval_pipeline_expr(action, expr, root, dot, state, resolver);
    }
    ensure_variable_is_defined(expr, state)?;
    eval_simple_expr_value(expr, root, dot, state)
}

fn eval_simple_expr_value(
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &EvalState,
) -> Result<Option<Value>, NativeRenderError> {
    if expr == "nil" {
        return Ok(Some(Value::Null));
    }
    if is_quoted_string(expr) {
        return Ok(decode_string_literal(expr).map(Value::String));
    }
    if let Some(v) = parse_char_constant(expr) {
        return Ok(Some(Value::Number(Number::from(v))));
    }
    if expr == "true" {
        return Ok(Some(Value::Bool(true)));
    }
    if expr == "false" {
        return Ok(Some(Value::Bool(false)));
    }
    if let Some(n) = parse_number_value(expr) {
        return Ok(Some(n));
    }
    resolve_simple_path(root, dot, expr, state.missing_value_mode, |name| {
        state.lookup_var(name)
    })
}

pub(super) fn render_output_expr(
    action: &str,
    expr: &str,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<String, NativeRenderError> {
    let has_decl = extract_pipeline_declaration(expr).0.is_some();
    // Go parity (text/template exec): action `{{ nil }}` is rejected as command.
    if !has_decl && is_nil_command(expr) {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: nil_is_not_a_command_reason(),
        });
    }
    let value = eval_expr_value_result(action, expr, root, dot, state, resolver)?;
    if has_decl {
        return Ok(String::new());
    }
    match value {
        Some(v) => Ok(format_value_like_go(&v)),
        None => match options.missing_value_mode {
            MissingValueMode::GoDefault | MissingValueMode::GoZero => Ok("<no value>".to_string()),
            MissingValueMode::Error => Err(NativeRenderError::MissingValue {
                action: action.to_string(),
                path: expr.to_string(),
            }),
        },
    }
}

fn eval_pipeline_expr(
    action: &str,
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    let (decl, runtime_expr) = extract_pipeline_declaration(expr);
    if decl.as_ref().is_some_and(|d| d.names.len() > 1) {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: multi_variable_decl_in_non_range_reason(),
        });
    }
    let commands = split_pipeline_commands(&runtime_expr);
    if commands.is_empty() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: empty_pipeline_reason(),
        });
    }
    let mut pipe: Option<Value> = None;
    for (idx, command) in commands.iter().enumerate() {
        pipe = eval_pipeline_command(action, command, root, dot, idx + 1, pipe, state, resolver)?;
    }
    if let Some(d) = decl {
        let Some(name) = d.names.first() else {
            return Ok(pipe);
        };
        match d.mode {
            PipelineDeclMode::Declare => state.declare_var(name, pipe.clone()),
            PipelineDeclMode::Assign => {
                if !state.assign_var(name, pipe.clone()) {
                    return Err(undefined_variable_error(name));
                }
            }
        }
    }
    Ok(pipe)
}

fn eval_pipeline_command(
    action: &str,
    command: &str,
    root: &Value,
    dot: &Value,
    pipeline_stage: usize,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    let has_pipe_input = pipeline_stage > 1;
    let tokens = split_command_tokens(command);
    if tokens.is_empty() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: empty_command_in_pipeline_reason(),
        });
    }

    let head = tokens[0].as_str();
    if head == "call" {
        return eval_call_builtin(
            action,
            &tokens[1..],
            root,
            dot,
            has_pipe_input,
            pipe_input,
            state,
            resolver,
        );
    }
    if head == "and" || head == "or" {
        return eval_short_circuit_builtin(
            action,
            head,
            &tokens[1..],
            root,
            dot,
            has_pipe_input,
            pipe_input,
            state,
            resolver,
        );
    }

    if is_builtin_function_name(head) {
        let mut args =
            Vec::with_capacity(tokens.len().saturating_sub(1) + usize::from(has_pipe_input));
        for token in tokens.iter().skip(1) {
            args.push(eval_command_token_value(
                action, token, root, dot, state, resolver,
            )?);
        }
        if has_pipe_input {
            args.push(pipe_input);
        }
        return eval_builtin_function(action, head, &args);
    }

    if let Some(result) = try_eval_external_function(
        action,
        head,
        &tokens[1..],
        root,
        dot,
        has_pipe_input,
        pipe_input.clone(),
        state,
        resolver,
    )? {
        return Ok(result);
    }

    let allow_dynamic_external_head =
        matches!(state.function_dispatch_mode, FunctionDispatchMode::Extended)
            && matches!(
                state.logic_backend,
                LogicBackend::GoCompat | LogicBackend::RustNative
            );
    if allow_dynamic_external_head {
        if let Some(result) = try_eval_dynamic_external_function(
            action,
            &tokens,
            root,
            dot,
            has_pipe_input,
            pipe_input.clone(),
            state,
            resolver,
        )? {
            return Ok(result);
        }
    }

    if has_pipe_input && is_non_executable_pipeline_head(head) {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: non_executable_pipeline_stage_reason(pipeline_stage),
        });
    }

    if has_pipe_input || tokens.len() > 1 {
        if let Some(target) = non_function_command_target(head) {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: cannot_give_argument_to_non_function_reason(&target),
            });
        }
        if let Some(field_path) = command_field_like_path(head) {
            let receiver = eval_command_token_value(
                action,
                &field_path.receiver_expr,
                root,
                dot,
                state,
                resolver,
            )?;
            let Some(receiver) = receiver else {
                return Ok(None);
            };
            if receiver == Value::Null {
                return Ok(None);
            }
            if is_map_like_for_field_call(&receiver) {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: field_not_method_has_arguments_reason(&field_path.field_name),
                });
            }
            let _ = eval_command_token_value(action, head, root, dot, state, resolver)?;
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: field_not_method_has_arguments_reason(&field_path.field_name),
            });
        }
    }

    if has_pipe_input {
        if is_identifier_name(head) {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: undefined_function_reason(head),
            });
        }
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: non_executable_pipeline_stage_reason(pipeline_stage),
        });
    }

    if tokens.len() == 1 {
        if is_nil_command(&tokens[0]) {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: nil_is_not_a_command_reason(),
            });
        }
        return eval_command_token_value(action, &tokens[0], root, dot, state, resolver);
    }

    Err(NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: undefined_function_reason(head),
    })
}

fn eval_short_circuit_builtin(
    action: &str,
    name: &str,
    arg_tokens: &[String],
    root: &Value,
    dot: &Value,
    has_pipe_input: bool,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    let total_args = arg_tokens.len() + usize::from(has_pipe_input);
    if total_args == 0 {
        return Err(wrong_number_of_args(action, name, "at least 1", 0));
    }
    let mut last = None;

    for token in arg_tokens {
        let val = eval_command_token_value(action, token, root, dot, state, resolver)?;
        let truth = is_truthy(&val);
        last = val.clone();
        match name {
            "and" if !truth => return Ok(val),
            "or" if truth => return Ok(val),
            _ => {}
        }
    }

    if has_pipe_input {
        let val = pipe_input;
        let truth = is_truthy(&val);
        last = val.clone();
        match name {
            "and" if !truth => return Ok(val),
            "or" if truth => return Ok(val),
            _ => {}
        }
    }

    Ok(last)
}

pub(super) fn eval_command_token_value(
    action: &str,
    token: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
    if let Some(inner) = strip_outer_parens(token) {
        return eval_pipeline_expr(action, inner, root, dot, state, resolver);
    }
    if looks_like_char_literal(token) && parse_char_constant(token).is_none() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: invalid_syntax_reason(token),
        });
    }
    if looks_like_numeric_literal(token) && parse_number_value(token).is_none() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: illegal_number_syntax_reason(token),
        });
    }
    ensure_variable_is_defined(token, state)?;
    eval_simple_expr_value(token, root, dot, state)
}
