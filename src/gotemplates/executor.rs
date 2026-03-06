use super::{
    compat, parse_template_tokens_strict_with_options,
    typedvalue::{
        decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
        decode_go_typed_slice_value, encode_go_bytes_value, encode_go_nil_bytes_value,
        encode_go_string_bytes_value, encode_go_typed_slice_value, go_bytes_get, go_bytes_is_nil,
        go_bytes_len, go_string_bytes_get, go_string_bytes_len, go_zero_value_for_type,
    },
    GoTemplateScanError, GoTemplateToken, ParseCompatOptions, HELM_INCLUDE_RECURSION_MAX_REFS,
};
use serde_json::{Number, Value};
use std::borrow::Cow;
use std::collections::BTreeMap;
mod compare;
mod commandkind;
mod path;
mod textfmt;
mod tokenize;
mod trim;
use compare::{builtin_cmp, builtin_eq, builtin_ne};
use commandkind::{
    command_field_like_path, is_non_executable_pipeline_head, non_function_command_target,
};
use path::{
    is_identifier_continue_char, is_identifier_start_char, resolve_simple_path,
    split_variable_reference,
};
use textfmt::{builtin_html, builtin_js, builtin_print, builtin_urlquery, format_value_for_print};
use tokenize::{split_command_tokens, split_pipeline_commands, strip_outer_parens};
use trim::apply_lexical_trims;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingValueMode {
    GoDefault,
    GoZero,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeRenderOptions {
    pub missing_value_mode: MissingValueMode,
}

impl Default for NativeRenderOptions {
    fn default() -> Self {
        Self {
            missing_value_mode: MissingValueMode::GoDefault,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRenderError {
    Parse(GoTemplateScanError),
    UnsupportedAction { action: String, reason: String },
    MissingValue { action: String, path: String },
    TemplateNotFound { name: String },
    TemplateRecursionLimit { name: String, depth: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeFunctionResolverError {
    UnknownFunction,
    Failed { reason: String },
}

pub trait NativeFunctionResolver {
    fn call(
        &self,
        name: &str,
        args: &[Option<Value>],
    ) -> Result<Option<Value>, NativeFunctionResolverError>;
}

impl<F> NativeFunctionResolver for F
where
    F: Fn(&str, &[Option<Value>]) -> Result<Option<Value>, NativeFunctionResolverError>,
{
    fn call(
        &self,
        name: &str,
        args: &[Option<Value>],
    ) -> Result<Option<Value>, NativeFunctionResolverError> {
        self(name, args)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Terminator {
    Eof,
    End,
    Else(ElseClause),
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ElseClause {
    Plain,
    If(String),
    With(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionKind {
    Noop,
    Output(String),
    If(String),
    With(String),
    Range(String),
    Else(ElseClause),
    End,
    Define { name: String },
    Block { name: String, arg: String },
    Template { name: String, arg: Option<String> },
    Break,
    Continue,
}

pub fn render_template_native(src: &str, root: &Value) -> Result<String, NativeRenderError> {
    render_template_native_with_options(src, root, NativeRenderOptions::default())
}

pub fn render_template_native_with_options(
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
) -> Result<String, NativeRenderError> {
    render_template_native_with_resolver(src, root, options, None)
}

pub fn render_template_native_with_resolver(
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<String, NativeRenderError> {
    let mut tokens = parse_template_tokens_strict_with_options(
        src,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: true,
            visible_variables: &[],
        },
    )
    .map_err(NativeRenderError::Parse)?;
    apply_lexical_trims(&mut tokens);
    let (main_tokens, templates) = split_template_set(&tokens)?;
    let dot = root.clone();
    let mut state = EvalState::new(options.missing_value_mode);
    let eval = eval_block(
        &main_tokens,
        0,
        &templates,
        root,
        &dot,
        false,
        options,
        resolver,
        0,
        &mut state,
    )?;
    match eval.term {
        Terminator::Eof => Ok(eval.out),
        Terminator::End | Terminator::Else(_) | Terminator::Break | Terminator::Continue => {
            Err(NativeRenderError::Parse(GoTemplateScanError {
                code: "unexpected_token",
                message: "unexpected control terminator at top level",
                offset: src.len(),
            }))
        }
    }
}

#[derive(Debug, Clone)]
struct BlockEval {
    out: String,
    next_idx: usize,
    term: Terminator,
}

#[derive(Debug, Clone)]
struct EvalState {
    scopes: Vec<BTreeMap<String, Option<Value>>>,
    missing_value_mode: MissingValueMode,
}

impl EvalState {
    fn new(missing_value_mode: MissingValueMode) -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            missing_value_mode,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn declare_var(&mut self, name: &str, value: Option<Value>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), value);
        }
    }

    fn assign_var(&mut self, name: &str, value: Option<Value>) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return true;
            }
        }
        false
    }

    fn lookup_var(&self, name: &str) -> Option<Option<Value>> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        None
    }
}

fn eval_block(
    tokens: &[GoTemplateToken],
    mut idx: usize,
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    root: &Value,
    dot: &Value,
    stop_on_else_end: bool,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    state: &mut EvalState,
) -> Result<BlockEval, NativeRenderError> {
    let mut out = String::new();
    while idx < tokens.len() {
        match &tokens[idx] {
            GoTemplateToken::Literal(lit) => {
                out.push_str(lit);
                idx += 1;
            }
            GoTemplateToken::Action(action) => {
                let kind = parse_action_kind(action)?;
                if stop_on_else_end {
                    match kind {
                        ActionKind::End => {
                            return Ok(BlockEval {
                                out,
                                next_idx: idx + 1,
                                term: Terminator::End,
                            });
                        }
                        ActionKind::Else(clause) => {
                            return Ok(BlockEval {
                                out,
                                next_idx: idx + 1,
                                term: Terminator::Else(clause),
                            });
                        }
                        _ => {}
                    }
                }

                match kind {
                    ActionKind::Noop => idx += 1,
                    ActionKind::Output(expr) => {
                        out.push_str(&render_output_expr(
                            action, &expr, root, dot, options, state, resolver,
                        )?);
                        idx += 1;
                    }
                    ActionKind::If(expr) => {
                        let eval = eval_if(
                            tokens,
                            idx + 1,
                            templates,
                            &expr,
                            root,
                            dot,
                            options,
                            resolver,
                            call_depth,
                            state,
                        )?;
                        out.push_str(&eval.out);
                        if matches!(&eval.term, Terminator::Break | Terminator::Continue) {
                            return Ok(BlockEval {
                                out,
                                next_idx: eval.next_idx,
                                term: eval.term,
                            });
                        }
                        idx = eval.next_idx;
                    }
                    ActionKind::With(expr) => {
                        let eval = eval_with(
                            tokens,
                            idx + 1,
                            templates,
                            &expr,
                            root,
                            dot,
                            options,
                            resolver,
                            call_depth,
                            state,
                        )?;
                        out.push_str(&eval.out);
                        if matches!(&eval.term, Terminator::Break | Terminator::Continue) {
                            return Ok(BlockEval {
                                out,
                                next_idx: eval.next_idx,
                                term: eval.term,
                            });
                        }
                        idx = eval.next_idx;
                    }
                    ActionKind::Range(expr) => {
                        let eval = eval_range(
                            tokens,
                            idx + 1,
                            templates,
                            &expr,
                            root,
                            dot,
                            options,
                            resolver,
                            call_depth,
                            state,
                        )?;
                        out.push_str(&eval.out);
                        if matches!(&eval.term, Terminator::Break | Terminator::Continue) {
                            return Ok(BlockEval {
                                out,
                                next_idx: eval.next_idx,
                                term: eval.term,
                            });
                        }
                        idx = eval.next_idx;
                    }
                    ActionKind::Define { .. } => {
                        idx = find_matching_end(tokens, idx + 1)?;
                    }
                    ActionKind::Block { name, arg } => {
                        let end_idx = find_matching_end(tokens, idx + 1)?;
                        let fallback = &tokens[idx + 1..end_idx.saturating_sub(1)];
                        out.push_str(&eval_block_invocation(
                            &name, &arg, fallback, templates, root, dot, options, resolver,
                            call_depth, state, action,
                        )?);
                        idx = end_idx;
                    }
                    ActionKind::Template { name, arg } => {
                        out.push_str(&eval_template_invocation(
                            &name,
                            arg.as_deref(),
                            templates,
                            root,
                            dot,
                            options,
                            resolver,
                            call_depth,
                            action,
                            state,
                        )?);
                        idx += 1;
                    }
                    ActionKind::Break | ActionKind::Continue => {
                        return Ok(BlockEval {
                            out,
                            next_idx: idx + 1,
                            term: if matches!(kind, ActionKind::Break) {
                                Terminator::Break
                            } else {
                                Terminator::Continue
                            },
                        });
                    }
                    ActionKind::Else(_) | ActionKind::End => {
                        return Err(NativeRenderError::Parse(GoTemplateScanError {
                            code: "unexpected_token",
                            message: "unexpected control action",
                            offset: 0,
                        }));
                    }
                }
            }
        }
    }

    Ok(BlockEval {
        out,
        next_idx: idx,
        term: Terminator::Eof,
    })
}

fn eval_if(
    tokens: &[GoTemplateToken],
    start_idx: usize,
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    expr: &str,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    state: &mut EvalState,
) -> Result<BlockEval, NativeRenderError> {
    state.push_scope();
    let result = (|| -> Result<BlockEval, NativeRenderError> {
        let cond = eval_expr_truthy(expr, root, dot, state, resolver)?;
        if cond {
            let then_eval = eval_block(
                tokens, start_idx, templates, root, dot, true, options, resolver, call_depth, state,
            )?;
            let next_idx = match then_eval.term {
                Terminator::End => then_eval.next_idx,
                Terminator::Else(_) => find_matching_end(tokens, then_eval.next_idx)?,
                Terminator::Break | Terminator::Continue => then_eval.next_idx,
                Terminator::Eof => {
                    return Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_eof",
                        message: "unexpected EOF",
                        offset: 0,
                    }));
                }
            };
            return Ok(BlockEval {
                out: then_eval.out,
                next_idx,
                term: match then_eval.term {
                    Terminator::Break => Terminator::Break,
                    Terminator::Continue => Terminator::Continue,
                    _ => Terminator::Eof,
                },
            });
        }

        let split = find_else_or_end(tokens, start_idx)?;
        match split.term {
            Terminator::End => Ok(BlockEval {
                out: String::new(),
                next_idx: split.next_idx,
                term: Terminator::Eof,
            }),
            Terminator::Else(ElseClause::Plain) => {
                let else_eval = eval_block(
                    tokens,
                    split.next_idx,
                    templates,
                    root,
                    dot,
                    true,
                    options,
                    resolver,
                    call_depth,
                    state,
                )?;
                match else_eval.term {
                    Terminator::End => Ok(BlockEval {
                        out: else_eval.out,
                        next_idx: else_eval.next_idx,
                        term: Terminator::Eof,
                    }),
                    Terminator::Break | Terminator::Continue => Ok(else_eval),
                    _ => Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_eof",
                        message: "unexpected EOF",
                        offset: 0,
                    })),
                }
            }
            Terminator::Else(ElseClause::If(next_expr)) => eval_if(
                tokens,
                split.next_idx,
                templates,
                &next_expr,
                root,
                dot,
                options,
                resolver,
                call_depth,
                state,
            ),
            Terminator::Else(ElseClause::With(_)) => {
                Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_token",
                    message: "unexpected else-with in if",
                    offset: 0,
                }))
            }
            Terminator::Break | Terminator::Continue => {
                Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_token",
                    message: "unexpected break/continue outside range",
                    offset: 0,
                }))
            }
            Terminator::Eof => Err(NativeRenderError::Parse(GoTemplateScanError {
                code: "unexpected_eof",
                message: "unexpected EOF",
                offset: 0,
            })),
        }
    })();
    state.pop_scope();
    result
}

fn eval_with(
    tokens: &[GoTemplateToken],
    start_idx: usize,
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    expr: &str,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    state: &mut EvalState,
) -> Result<BlockEval, NativeRenderError> {
    state.push_scope();
    let result = (|| -> Result<BlockEval, NativeRenderError> {
        let value = eval_expr_value(expr, root, dot, state, resolver)?;
        let truthy = is_truthy(&value);

        if truthy {
            let then_eval = eval_block(
                tokens,
                start_idx,
                templates,
                root,
                value.as_ref().unwrap_or(dot),
                true,
                options,
                resolver,
                call_depth,
                state,
            )?;
            let next_idx = match then_eval.term {
                Terminator::End => then_eval.next_idx,
                Terminator::Else(_) => find_matching_end(tokens, then_eval.next_idx)?,
                Terminator::Break | Terminator::Continue => then_eval.next_idx,
                Terminator::Eof => {
                    return Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_eof",
                        message: "unexpected EOF",
                        offset: 0,
                    }));
                }
            };
            return Ok(BlockEval {
                out: then_eval.out,
                next_idx,
                term: match then_eval.term {
                    Terminator::Break => Terminator::Break,
                    Terminator::Continue => Terminator::Continue,
                    _ => Terminator::Eof,
                },
            });
        }

        let split = find_else_or_end(tokens, start_idx)?;
        match split.term {
            Terminator::End => Ok(BlockEval {
                out: String::new(),
                next_idx: split.next_idx,
                term: Terminator::Eof,
            }),
            Terminator::Else(ElseClause::Plain) => {
                let else_eval = eval_block(
                    tokens,
                    split.next_idx,
                    templates,
                    root,
                    dot,
                    true,
                    options,
                    resolver,
                    call_depth,
                    state,
                )?;
                match else_eval.term {
                    Terminator::End => Ok(BlockEval {
                        out: else_eval.out,
                        next_idx: else_eval.next_idx,
                        term: Terminator::Eof,
                    }),
                    Terminator::Break | Terminator::Continue => Ok(else_eval),
                    _ => Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_eof",
                        message: "unexpected EOF",
                        offset: 0,
                    })),
                }
            }
            Terminator::Else(ElseClause::With(next_expr)) => eval_with(
                tokens,
                split.next_idx,
                templates,
                &next_expr,
                root,
                dot,
                options,
                resolver,
                call_depth,
                state,
            ),
            Terminator::Else(ElseClause::If(_)) => {
                Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_token",
                    message: "unexpected else-if in with",
                    offset: 0,
                }))
            }
            Terminator::Break | Terminator::Continue => {
                Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_token",
                    message: "unexpected break/continue outside range",
                    offset: 0,
                }))
            }
            Terminator::Eof => Err(NativeRenderError::Parse(GoTemplateScanError {
                code: "unexpected_eof",
                message: "unexpected EOF",
                offset: 0,
            })),
        }
    })();
    state.pop_scope();
    result
}

fn eval_range(
    tokens: &[GoTemplateToken],
    start_idx: usize,
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    expr: &str,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    state: &mut EvalState,
) -> Result<BlockEval, NativeRenderError> {
    state.push_scope();
    let result = (|| -> Result<BlockEval, NativeRenderError> {
        let (decl, source_expr) = extract_pipeline_declaration(expr);
        if decl.as_ref().is_some_and(|d| d.names.len() > 2) {
            return Err(NativeRenderError::UnsupportedAction {
                action: format!("{{{{range {expr}}}}}"),
                reason: "range declaration supports at most two variables".to_string(),
            });
        }

        let source = eval_expr_value(&source_expr, root, dot, state, resolver)?;
        if let Some(d) = &decl {
            let default_value = source.clone();
            for name in &d.names {
                match d.mode {
                    PipelineDeclMode::Declare => state.declare_var(name, default_value.clone()),
                    PipelineDeclMode::Assign => {
                        if !state.assign_var(name, default_value.clone()) {
                            return Err(undefined_variable_error(name));
                        }
                    }
                }
            }
        }
        let items = range_items(expr, source)?;
        let range_end_idx = find_matching_end(tokens, start_idx)?;
        if items.is_empty() {
            let split = find_else_or_end(tokens, start_idx)?;
            return match split.term {
                Terminator::End => Ok(BlockEval {
                    out: String::new(),
                    next_idx: split.next_idx,
                    term: Terminator::Eof,
                }),
                Terminator::Else(ElseClause::Plain) => {
                    let else_eval = eval_block(
                        tokens,
                        split.next_idx,
                        templates,
                        root,
                        dot,
                        true,
                        options,
                        resolver,
                        call_depth,
                        state,
                    )?;
                    match else_eval.term {
                        Terminator::End => Ok(BlockEval {
                            out: else_eval.out,
                            next_idx: else_eval.next_idx,
                            term: Terminator::Eof,
                        }),
                        Terminator::Break | Terminator::Continue => {
                            Err(NativeRenderError::Parse(GoTemplateScanError {
                                code: "unexpected_token",
                                message: "break/continue outside range",
                                offset: 0,
                            }))
                        }
                        _ => Err(NativeRenderError::Parse(GoTemplateScanError {
                            code: "unexpected_eof",
                            message: "unexpected EOF",
                            offset: 0,
                        })),
                    }
                }
                Terminator::Else(_) => Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_token",
                    message: "unexpected else-chain in range",
                    offset: 0,
                })),
                Terminator::Break | Terminator::Continue => {
                    Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_token",
                        message: "unexpected break/continue outside range",
                        offset: 0,
                    }))
                }
                Terminator::Eof => Err(NativeRenderError::Parse(GoTemplateScanError {
                    code: "unexpected_eof",
                    message: "unexpected EOF",
                    offset: 0,
                })),
            };
        }

        let mut out = String::new();
        for (key, item) in items {
            state.push_scope();
            if let Some(d) = &decl {
                apply_range_iteration_bindings(expr, d, key, &item, state)?;
            }
            let eval = eval_block(
                tokens, start_idx, templates, root, &item, true, options, resolver, call_depth,
                state,
            )?;
            state.pop_scope();
            out.push_str(&eval.out);
            match eval.term {
                Terminator::End | Terminator::Else(_) | Terminator::Continue => {}
                Terminator::Break => {
                    break;
                }
                Terminator::Eof => {
                    return Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: "unexpected_eof",
                        message: "unexpected EOF",
                        offset: 0,
                    }));
                }
            }
        }

        Ok(BlockEval {
            out,
            next_idx: range_end_idx,
            term: Terminator::Eof,
        })
    })();
    state.pop_scope();
    result
}

fn find_else_or_end(
    tokens: &[GoTemplateToken],
    start_idx: usize,
) -> Result<BlockEval, NativeRenderError> {
    let mut depth = 0usize;
    let mut idx = start_idx;
    while idx < tokens.len() {
        if let GoTemplateToken::Action(action) = &tokens[idx] {
            match parse_action_kind(action)? {
                ActionKind::If(_)
                | ActionKind::With(_)
                | ActionKind::Range(_)
                | ActionKind::Define { .. }
                | ActionKind::Block { .. } => {
                    depth += 1;
                }
                ActionKind::End => {
                    if depth == 0 {
                        return Ok(BlockEval {
                            out: String::new(),
                            next_idx: idx + 1,
                            term: Terminator::End,
                        });
                    }
                    depth = depth.saturating_sub(1);
                }
                ActionKind::Else(clause) => {
                    if depth == 0 {
                        return Ok(BlockEval {
                            out: String::new(),
                            next_idx: idx + 1,
                            term: Terminator::Else(clause),
                        });
                    }
                }
                _ => {}
            }
        }
        idx += 1;
    }
    Err(NativeRenderError::Parse(GoTemplateScanError {
        code: "unexpected_eof",
        message: "unexpected EOF",
        offset: 0,
    }))
}

fn find_matching_end(
    tokens: &[GoTemplateToken],
    start_idx: usize,
) -> Result<usize, NativeRenderError> {
    let mut depth = 0usize;
    let mut idx = start_idx;
    while idx < tokens.len() {
        if let GoTemplateToken::Action(action) = &tokens[idx] {
            match parse_action_kind(action)? {
                ActionKind::If(_)
                | ActionKind::With(_)
                | ActionKind::Range(_)
                | ActionKind::Define { .. }
                | ActionKind::Block { .. } => {
                    depth += 1;
                }
                ActionKind::End => {
                    if depth == 0 {
                        return Ok(idx + 1);
                    }
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
        idx += 1;
    }
    Err(NativeRenderError::Parse(GoTemplateScanError {
        code: "unexpected_eof",
        message: "unexpected EOF",
        offset: 0,
    }))
}

fn split_template_set(
    tokens: &[GoTemplateToken],
) -> Result<(Vec<GoTemplateToken>, BTreeMap<String, Vec<GoTemplateToken>>), NativeRenderError> {
    let mut main = Vec::with_capacity(tokens.len());
    let mut defs: BTreeMap<String, Vec<GoTemplateToken>> = BTreeMap::new();
    let mut idx = 0usize;
    while idx < tokens.len() {
        match &tokens[idx] {
            GoTemplateToken::Literal(_) => {
                main.push(tokens[idx].clone());
                idx += 1;
            }
            GoTemplateToken::Action(action) => match parse_action_kind(action)? {
                ActionKind::Define { name } => {
                    let end_idx = find_matching_end(tokens, idx + 1)?;
                    let body = tokens[idx + 1..end_idx.saturating_sub(1)].to_vec();
                    defs.insert(name, body);
                    idx = end_idx;
                }
                _ => {
                    main.push(tokens[idx].clone());
                    idx += 1;
                }
            },
        }
    }
    Ok((main, defs))
}

fn eval_template_invocation(
    name: &str,
    arg_expr: Option<&str>,
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    _action: &str,
    state: &mut EvalState,
) -> Result<String, NativeRenderError> {
    if call_depth >= HELM_INCLUDE_RECURSION_MAX_REFS {
        return Err(NativeRenderError::TemplateRecursionLimit {
            name: name.to_string(),
            depth: call_depth,
        });
    }
    let body = templates
        .get(name)
        .ok_or_else(|| NativeRenderError::TemplateNotFound {
            name: name.to_string(),
        })?;
    let next_dot = if let Some(expr) = arg_expr {
        eval_expr_value(expr, root, dot, state, resolver)?.unwrap_or(Value::Null)
    } else {
        dot.clone()
    };
    let mut isolated_state = EvalState::new(options.missing_value_mode);
    let eval = eval_block(
        body,
        0,
        templates,
        &next_dot,
        &next_dot,
        false,
        options,
        resolver,
        call_depth + 1,
        &mut isolated_state,
    )?;
    match eval.term {
        Terminator::Eof => Ok(eval.out),
        Terminator::End | Terminator::Else(_) | Terminator::Break | Terminator::Continue => {
            Err(NativeRenderError::Parse(GoTemplateScanError {
                code: "unexpected_token",
                message: "template body terminated unexpectedly",
                offset: 0,
            }))
        }
    }
}

fn eval_block_invocation(
    name: &str,
    arg_expr: &str,
    fallback_body: &[GoTemplateToken],
    templates: &BTreeMap<String, Vec<GoTemplateToken>>,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
    call_depth: usize,
    state: &mut EvalState,
    _action: &str,
) -> Result<String, NativeRenderError> {
    if call_depth >= HELM_INCLUDE_RECURSION_MAX_REFS {
        return Err(NativeRenderError::TemplateRecursionLimit {
            name: name.to_string(),
            depth: call_depth,
        });
    }
    let next_dot = eval_expr_value(arg_expr, root, dot, state, resolver)?.unwrap_or(Value::Null);
    let render_body = templates
        .get(name)
        .map(Vec::as_slice)
        .unwrap_or(fallback_body);
    let mut isolated_state = EvalState::new(options.missing_value_mode);
    let eval = eval_block(
        render_body,
        0,
        templates,
        &next_dot,
        &next_dot,
        false,
        options,
        resolver,
        call_depth + 1,
        &mut isolated_state,
    )?;
    match eval.term {
        Terminator::Eof => Ok(eval.out),
        Terminator::End | Terminator::Else(_) | Terminator::Break | Terminator::Continue => {
            Err(NativeRenderError::Parse(GoTemplateScanError {
                code: "unexpected_token",
                message: "template body terminated unexpectedly",
                offset: 0,
            }))
        }
    }
}

fn parse_action_kind(action: &str) -> Result<ActionKind, NativeRenderError> {
    let Some(inner) = action_inner(action) else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "invalid action delimiters".to_string(),
        });
    };
    if inner.is_empty() || inner.starts_with("/*") {
        return Ok(ActionKind::Noop);
    }

    if inner == "end" {
        return Ok(ActionKind::End);
    }
    if inner == "else" {
        return Ok(ActionKind::Else(ElseClause::Plain));
    }
    if let Some(expr) = inner.strip_prefix("else if ") {
        return Ok(ActionKind::Else(ElseClause::If(expr.trim().to_string())));
    }
    if let Some(expr) = inner.strip_prefix("else with ") {
        return Ok(ActionKind::Else(ElseClause::With(expr.trim().to_string())));
    }
    if let Some(expr) = inner.strip_prefix("if ") {
        return Ok(ActionKind::If(expr.trim().to_string()));
    }
    if let Some(expr) = inner.strip_prefix("with ") {
        return Ok(ActionKind::With(expr.trim().to_string()));
    }
    if let Some(expr) = inner.strip_prefix("range ") {
        return Ok(ActionKind::Range(expr.trim().to_string()));
    }
    if let Some(rest) = inner.strip_prefix("define ") {
        let name = parse_quoted_name(rest).ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "define name must be a quoted string".to_string(),
        })?;
        return Ok(ActionKind::Define { name });
    }
    if let Some(rest) = inner.strip_prefix("block ") {
        let (name, arg) = parse_block_invocation_clause(rest).ok_or_else(|| {
            NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "block clause must be: block \"name\" arg".to_string(),
            }
        })?;
        return Ok(ActionKind::Block { name, arg });
    }
    if let Some(rest) = inner.strip_prefix("template ") {
        let (name, arg) = parse_template_invocation_clause(rest).ok_or_else(|| {
            NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "template clause must be: template \"name\" [arg]".to_string(),
            }
        })?;
        return Ok(ActionKind::Template { name, arg });
    }
    if inner == "break" {
        return Ok(ActionKind::Break);
    }
    if inner == "continue" {
        return Ok(ActionKind::Continue);
    }
    Ok(ActionKind::Output(inner.to_string()))
}

fn parse_quoted_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(decoded) = decode_string_literal(trimmed) {
        return Some(decoded);
    }
    None
}

fn parse_template_invocation_clause(raw: &str) -> Option<(String, Option<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (name, tail) = compat::parse_go_quoted_prefix(trimmed)?;
    let tail = tail.trim();
    let arg = if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    };
    Some((name, arg))
}

fn parse_block_invocation_clause(raw: &str) -> Option<(String, String)> {
    let (name, arg) = parse_template_invocation_clause(raw)?;
    Some((name, arg?.trim().to_string()))
}

fn eval_expr_truthy(
    expr: &str,
    root: &Value,
    dot: &Value,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<bool, NativeRenderError> {
    let val = eval_expr_value(expr, root, dot, state, resolver)?;
    Ok(is_truthy(&val))
}

fn is_truthy(v: &Option<Value>) -> bool {
    let Some(value) = v.as_ref() else {
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

fn eval_expr_value(
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

fn render_output_expr(
    action: &str,
    expr: &str,
    root: &Value,
    dot: &Value,
    options: NativeRenderOptions,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<String, NativeRenderError> {
    let has_decl = extract_pipeline_declaration(expr).0.is_some();
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
            reason: "multi-variable declarations are only supported in range pipelines".to_string(),
        });
    }
    let commands = split_pipeline_commands(&runtime_expr);
    if commands.is_empty() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "empty pipeline".to_string(),
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
            reason: "empty command in pipeline".to_string(),
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

    if has_pipe_input && is_non_executable_pipeline_head(head) {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("non executable command in pipeline stage {pipeline_stage}"),
        });
    }

    if has_pipe_input || tokens.len() > 1 {
        if let Some(target) = non_function_command_target(head) {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("can't give argument to non-function {target}"),
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
                    reason: format!(
                        "{} is not a method but has arguments",
                        field_path.field_name
                    ),
                });
            }
            let _ = eval_command_token_value(action, head, root, dot, state, resolver)?;
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "{} is not a method but has arguments",
                    field_path.field_name
                ),
            });
        }
    }

    if has_pipe_input {
        if is_identifier_name(head) {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("\"{head}\" is not a defined function"),
            });
        }
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("non executable command in pipeline stage {pipeline_stage}"),
        });
    }

    if tokens.len() == 1 {
        if tokens[0].trim() == "nil" {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "nil is not a command".to_string(),
            });
        }
        return eval_command_token_value(action, &tokens[0], root, dot, state, resolver);
    }

    Err(NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: format!("\"{head}\" is not a defined function"),
    })
}

fn is_map_like_for_field_call(v: &Value) -> bool {
    if go_bytes_len(v).is_some()
        || go_string_bytes_len(v).is_some()
        || decode_go_typed_slice_value(v).is_some()
    {
        return false;
    }
    decode_go_typed_map_value(v).is_some() || matches!(v, Value::Object(_))
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

fn eval_call_builtin(
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

    let mut args = Vec::with_capacity(arg_tokens.len().saturating_sub(1) + usize::from(has_pipe_input));
    for token in arg_tokens.iter().skip(1) {
        args.push(eval_command_token_value(
            action,
            token,
            root,
            dot,
            state,
            resolver,
        )?);
    }

    let first_token = arg_tokens.first().map(String::as_str);
    let first_value = if let Some(first) = first_token {
        if is_identifier_name(first) {
            let Some(resolver) = resolver else {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!("\"{first}\" is not a defined function"),
                });
            };
            if has_pipe_input {
                args.push(pipe_input);
            }
            return call_named_external_function(action, first, &args, resolver);
        }
        eval_command_token_value(action, first, root, dot, state, resolver)?
    } else if has_pipe_input {
        pipe_input
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
        if is_identifier_name(name) {
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
                reason: format!("\"{name}\" is not a defined function"),
            })
        }
        Err(NativeFunctionResolverError::Failed { reason }) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("error calling {name}: {reason}"),
            })
        }
    }
}

fn call_target_display(token: Option<&str>, value: &Value) -> String {
    if let Some(raw) = token {
        let trimmed = raw.trim();
        if let Some(inner) = strip_outer_parens(trimmed) {
            return inner.trim().to_string();
        }
        return trimmed.to_string();
    }
    format_value_for_print(&Some(value.clone()))
}

fn try_eval_external_function(
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
    if !is_identifier_name(name) {
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
                reason: format!("error calling {name}: {reason}"),
            })
        }
    }
}

fn try_eval_dynamic_external_function(
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
    if tokens.is_empty() || is_identifier_name(&tokens[0]) {
        return Ok(None);
    }

    let Some(Value::String(fn_name)) =
        eval_command_token_value(action, &tokens[0], root, dot, state, Some(resolver))?
    else {
        return Ok(None);
    };
    if !is_identifier_name(&fn_name) {
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
                reason: format!("error calling {fn_name}: {reason}"),
            })
        }
    }
}

fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_identifier_start_char(first) {
        return false;
    }
    chars.all(is_identifier_continue_char)
}

fn eval_command_token_value(
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
            reason: format!("invalid syntax: {token}"),
        });
    }
    if looks_like_numeric_literal(token) && parse_number_value(token).is_none() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("illegal number syntax: {token}"),
        });
    }
    ensure_variable_is_defined(token, state)?;
    eval_simple_expr_value(token, root, dot, state)
}

fn looks_like_numeric_literal(expr: &str) -> bool {
    compat::looks_like_numeric_literal(expr)
}

fn looks_like_char_literal(expr: &str) -> bool {
    compat::looks_like_char_literal(expr)
}

fn ensure_variable_is_defined(expr: &str, state: &EvalState) -> Result<(), NativeRenderError> {
    if let Some((name, _)) = split_variable_reference(expr) {
        if name != "$" && state.lookup_var(name).is_none() {
            return Err(undefined_variable_error(name));
        }
    }
    Ok(())
}

fn undefined_variable_error(_name: &str) -> NativeRenderError {
    NativeRenderError::Parse(GoTemplateScanError {
        code: "undefined_variable",
        message: "undefined variable",
        offset: 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipelineDeclMode {
    Declare,
    Assign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PipelineDeclaration {
    names: Vec<String>,
    mode: PipelineDeclMode,
}

fn extract_pipeline_declaration(expr: &str) -> (Option<PipelineDeclaration>, String) {
    let commands = split_pipeline_commands(expr);
    if commands.is_empty() {
        return (None, expr.trim().to_string());
    }
    let first_tokens = split_command_tokens(&commands[0]);
    let Some((decl, rest_tokens_start)) = parse_pipeline_decl_tokens(&first_tokens) else {
        return (None, expr.trim().to_string());
    };

    let mut rebuilt = Vec::new();
    if rest_tokens_start < first_tokens.len() {
        rebuilt.push(first_tokens[rest_tokens_start..].join(" "));
    }
    for cmd in commands.iter().skip(1) {
        rebuilt.push(cmd.clone());
    }
    (Some(decl), rebuilt.join(" | "))
}

fn parse_pipeline_decl_tokens(tokens: &[String]) -> Option<(PipelineDeclaration, usize)> {
    if tokens.len() >= 3 && is_variable_token(&tokens[0]) && is_decl_op_token(&tokens[1]) {
        return Some((
            PipelineDeclaration {
                names: vec![tokens[0].clone()],
                mode: decl_mode_from_token(&tokens[1])?,
            },
            2,
        ));
    }
    if tokens.len() >= 5
        && is_variable_token(&tokens[0])
        && tokens[1] == ","
        && is_variable_token(&tokens[2])
        && is_decl_op_token(&tokens[3])
    {
        return Some((
            PipelineDeclaration {
                names: vec![tokens[0].clone(), tokens[2].clone()],
                mode: decl_mode_from_token(&tokens[3])?,
            },
            4,
        ));
    }
    None
}

fn is_variable_token(token: &str) -> bool {
    if !token.starts_with('$') || token == "$" {
        return false;
    }
    token[1..].chars().all(is_identifier_continue_char)
}

fn is_decl_op_token(token: &str) -> bool {
    matches!(token, ":=" | "=")
}

fn decl_mode_from_token(token: &str) -> Option<PipelineDeclMode> {
    match token {
        ":=" => Some(PipelineDeclMode::Declare),
        "=" => Some(PipelineDeclMode::Assign),
        _ => None,
    }
}

fn is_builtin_function_name(name: &str) -> bool {
    matches!(
        name,
        "and"
            | "call"
            | "or"
            | "not"
            | "len"
            | "index"
            | "slice"
            | "html"
            | "js"
            | "print"
            | "printf"
            | "println"
            | "urlquery"
            | "eq"
            | "ne"
            | "lt"
            | "le"
            | "gt"
            | "ge"
    )
}

fn eval_builtin_function(
    action: &str,
    name: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeRenderError> {
    let value = match name {
        "and" => builtin_and(args),
        "or" => builtin_or(args),
        "not" => {
            if args.len() != 1 {
                return Err(wrong_number_of_args(action, "not", "1", args.len()));
            }
            Some(Value::Bool(!is_truthy(&args[0])))
        }
        "len" => Some(Value::Number(Number::from(
            builtin_len(action, args)? as u64
        ))),
        "index" => builtin_index(action, args)?,
        "slice" => builtin_slice(action, args)?,
        "html" => Some(Value::String(builtin_html(args))),
        "js" => Some(Value::String(builtin_js(args))),
        "print" => Some(Value::String(builtin_print(args, false))),
        "println" => Some(Value::String(builtin_print(args, true))),
        "printf" => Some(Value::String(builtin_printf(action, args)?)),
        "urlquery" => Some(Value::String(builtin_urlquery(args))),
        "eq" => Some(Value::Bool(builtin_eq(action, args)?)),
        "ne" => Some(Value::Bool(builtin_ne(action, args)?)),
        "lt" => Some(Value::Bool(builtin_cmp(action, "lt", args, |o| o.is_lt())?)),
        "le" => Some(Value::Bool(builtin_cmp(action, "le", args, |o| o.is_le())?)),
        "gt" => Some(Value::Bool(builtin_cmp(action, "gt", args, |o| o.is_gt())?)),
        "ge" => Some(Value::Bool(builtin_cmp(action, "ge", args, |o| o.is_ge())?)),
        _ => {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("function {name} is not supported by native executor"),
            });
        }
    };
    Ok(value)
}

fn builtin_and(args: &[Option<Value>]) -> Option<Value> {
    if args.is_empty() {
        return None;
    }
    for arg in args {
        if !is_truthy(arg) {
            return arg.clone();
        }
    }
    args.last().cloned().unwrap_or(None)
}

fn builtin_or(args: &[Option<Value>]) -> Option<Value> {
    if args.is_empty() {
        return None;
    }
    for arg in args {
        if is_truthy(arg) {
            return arg.clone();
        }
    }
    args.last().cloned().unwrap_or(None)
}

fn builtin_len(action: &str, args: &[Option<Value>]) -> Result<usize, NativeRenderError> {
    if args.len() != 1 {
        return Err(wrong_number_of_args(action, "len", "1", args.len()));
    }
    let value = args[0]
        .as_ref()
        .ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling len: len of nil pointer".to_string(),
        })?;
    if let Some(len) = go_bytes_len(value).or_else(|| go_string_bytes_len(value)) {
        return Ok(len);
    }
    if let Some(typed_map) = decode_go_typed_map_value(value) {
        return Ok(typed_map.entries.map_or(0, |entries| entries.len()));
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(value) {
        return Ok(typed_slice.items.map_or(0, <[Value]>::len));
    }
    match value {
        Value::Null => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling len: len of nil pointer".to_string(),
        }),
        Value::String(s) => Ok(s.len()),
        Value::Array(a) => Ok(a.len()),
        Value::Object(m) => Ok(m.len()),
        _ => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling len: len of type {}",
                value_type_name_for_template(value)
            ),
        }),
    }
}

fn builtin_index(action: &str, args: &[Option<Value>]) -> Result<Option<Value>, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "index", "at least 1", 0));
    }
    let mut cur = args[0].clone();
    if cur.is_none() {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling index: index of untyped nil".to_string(),
        });
    }
    if args.len() == 1 {
        return Ok(cur);
    }
    for idx in args.iter().skip(1) {
        if let Some(ref value) = cur {
            if let Some(typed_map) = decode_go_typed_map_value(value) {
                let next = match map_key_arg(idx) {
                    MapKeyArg::Key(key) => typed_map
                        .entries
                        .and_then(|entries| entries.get(&key))
                        .cloned()
                        .unwrap_or_else(|| go_zero_value_for_type(typed_map.elem_type)),
                    MapKeyArg::StringLikeNonUtf8 => go_zero_value_for_type(typed_map.elem_type),
                    MapKeyArg::WrongType => {
                        let suffix = if matches!(idx, None | Some(Value::Null)) {
                            "value is nil; should be string".to_string()
                        } else {
                            format!(
                                "value has type {}; should be string",
                                option_type_name_for_template(idx)
                            )
                        };
                        return Err(NativeRenderError::UnsupportedAction {
                            action: action.to_string(),
                            reason: format!("error calling index: {suffix}"),
                        });
                    }
                };
                cur = Some(next);
                continue;
            }
            if let Some(typed_slice) = decode_go_typed_slice_value(value) {
                let len = typed_slice.items.map_or(0, <[Value]>::len);
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let item = typed_slice
                    .items
                    .and_then(|items| items.get(pos))
                    .ok_or_else(|| NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed typed slice value".to_string(),
                    })?;
                cur = Some(item.clone());
                continue;
            }
            if let Some(len) = go_bytes_len(value) {
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let byte = go_bytes_get(value, pos).ok_or_else(|| {
                    NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed []byte value".to_string(),
                    }
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
            if let Some(len) = go_string_bytes_len(value) {
                let pos = parse_slice_like_index(action, "index", idx, len)?;
                let byte = go_string_bytes_get(value, pos).ok_or_else(|| {
                    NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: "error calling index: malformed string value".to_string(),
                    }
                })?;
                cur = Some(Value::Number(Number::from(byte)));
                continue;
            }
        }
        let next = match cur {
            Some(Value::Array(ref items)) => {
                let pos = parse_slice_like_index(action, "index", idx, items.len())?;
                Some(items[pos].clone())
            }
            Some(Value::Object(ref map)) => match map_key_arg(idx) {
                MapKeyArg::Key(key) => map.get(&key).cloned(),
                MapKeyArg::StringLikeNonUtf8 => None,
                MapKeyArg::WrongType => {
                    let suffix = if matches!(idx, None | Some(Value::Null)) {
                        "value is nil; should be string".to_string()
                    } else {
                        format!(
                            "value has type {}; should be string",
                            option_type_name_for_template(idx)
                        )
                    };
                    return Err(NativeRenderError::UnsupportedAction {
                        action: action.to_string(),
                        reason: format!("error calling index: {suffix}"),
                    });
                }
            },
            Some(Value::String(ref s)) => {
                let bytes = s.as_bytes();
                let pos = parse_slice_like_index(action, "index", idx, bytes.len())?;
                Some(Value::Number(Number::from(bytes[pos])))
            }
            Some(Value::Null) | None => {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "error calling index: index of untyped nil".to_string(),
                });
            }
            Some(ref value) => {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling index: can't index item of type {}",
                        value_type_name_for_template(value)
                    ),
                });
            }
        };
        cur = next;
    }
    Ok(cur)
}

fn builtin_slice(action: &str, args: &[Option<Value>]) -> Result<Option<Value>, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "slice", "at least 1", 0));
    }
    if args.len() > 4 {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling slice: too many slice indexes: {}",
                args.len() - 1
            ),
        });
    }
    let item = args[0]
        .as_ref()
        .ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling slice: slice of untyped nil".to_string(),
        })?;

    if let Some(bytes) = decode_go_bytes_value(item) {
        let was_nil_bytes = go_bytes_is_nil(item);
        let cap = bytes.len();
        let len = bytes.len();
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() < 4 {
            if was_nil_bytes && idx[0] == 0 && idx[1] == 0 {
                return Ok(Some(encode_go_nil_bytes_value()));
            }
            return Ok(Some(encode_go_bytes_value(&bytes[idx[0]..idx[1]])));
        }
        if idx[1] > idx[2] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[1], idx[2]
                ),
            });
        }
        if was_nil_bytes && idx[0] == 0 && idx[1] == 0 {
            return Ok(Some(encode_go_nil_bytes_value()));
        }
        return Ok(Some(encode_go_bytes_value(&bytes[idx[0]..idx[1]])));
    }
    if let Some(bytes) = decode_go_string_bytes_value(item) {
        let cap = bytes.len();
        let len = bytes.len();
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() == 4 {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "error calling slice: cannot 3-index slice a string".to_string(),
            });
        }
        let sliced = bytes[idx[0]..idx[1]].to_vec();
        return Ok(Some(value_from_go_string_bytes(sliced)));
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(item) {
        let cap = typed_slice.items.map_or(0, <[Value]>::len);
        let len = cap;
        let mut idx = [0usize, len, cap];
        for (i, index_arg) in args.iter().skip(1).enumerate() {
            idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
        }
        if idx[0] > idx[1] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[0], idx[1]
                ),
            });
        }
        if args.len() > 3 && idx[1] > idx[2] {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!(
                    "error calling slice: invalid slice index: {} > {}",
                    idx[1], idx[2]
                ),
            });
        }
        if typed_slice.items.is_none() {
            return Ok(Some(encode_go_typed_slice_value(
                typed_slice.elem_type,
                None,
            )));
        }
        let Some(items) = typed_slice.items else {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: "error calling slice: malformed typed slice value".to_string(),
            });
        };
        return Ok(Some(encode_go_typed_slice_value(
            typed_slice.elem_type,
            Some(items[idx[0]..idx[1]].to_vec()),
        )));
    }

    match item {
        Value::Array(items) => {
            let cap = items.len();
            let len = items.len();
            let mut idx = [0usize, len, cap];
            for (i, index_arg) in args.iter().skip(1).enumerate() {
                idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[0], idx[1]
                    ),
                });
            }
            if args.len() <= 3 {
                return Ok(Some(Value::Array(items[idx[0]..idx[1]].to_vec())));
            }
            if idx[1] > idx[2] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[1], idx[2]
                    ),
                });
            }
            Ok(Some(Value::Array(items[idx[0]..idx[1]].to_vec())))
        }
        Value::String(s) => {
            if args.len() == 4 {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "error calling slice: cannot 3-index slice a string".to_string(),
                });
            }
            let cap = s.len();
            let len = s.len();
            let mut idx = [0usize, len];
            for (i, index_arg) in args.iter().skip(1).enumerate() {
                idx[i] = parse_slice_like_index(action, "slice", index_arg, cap)?;
            }
            if idx[0] > idx[1] {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: format!(
                        "error calling slice: invalid slice index: {} > {}",
                        idx[0], idx[1]
                    ),
                });
            }
            let bytes = s.as_bytes()[idx[0]..idx[1]].to_vec();
            Ok(Some(value_from_go_string_bytes(bytes)))
        }
        _ => Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling slice: can't slice item of type {}",
                value_type_name_for_template(item)
            ),
        }),
    }
}

fn builtin_printf(action: &str, args: &[Option<Value>]) -> Result<String, NativeRenderError> {
    if args.is_empty() {
        return Err(wrong_number_of_args(action, "printf", "at least 1", 0));
    }
    let Some(fmt) = args
        .first()
        .and_then(|v| v.as_ref())
        .and_then(Value::as_str)
    else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "printf format must be a string".to_string(),
        });
    };
    compat::go_printf(fmt, &args[1..]).map_err(|reason| NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason,
    })
}

fn wrong_number_of_args(action: &str, fn_name: &str, want: &str, got: usize) -> NativeRenderError {
    NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: format!("wrong number of args for {fn_name}: want {want} got {got}"),
    }
}

fn value_to_i64(v: &Option<Value>) -> Option<i64> {
    match v.as_ref() {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else {
                n.as_u64().map(|u| u as i64)
            }
        }
        _ => None,
    }
}

fn parse_slice_like_index(
    action: &str,
    call_name: &str,
    idx_arg: &Option<Value>,
    cap: usize,
) -> Result<usize, NativeRenderError> {
    let raw = match idx_arg.as_ref() {
        None | Some(Value::Null) => {
            return Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("error calling {call_name}: cannot index slice/array with nil"),
            });
        }
        Some(v) => value_to_i64(idx_arg).ok_or_else(|| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!(
                "error calling {call_name}: cannot index slice/array with type {}",
                value_type_name_for_template(v)
            ),
        })?,
    };
    let out_of_range = if call_name == "index" {
        raw < 0 || raw as usize >= cap
    } else {
        raw < 0 || raw as usize > cap
    };
    if out_of_range {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: format!("error calling {call_name}: index out of range: {raw}"),
        });
    }
    Ok(raw as usize)
}

fn value_from_go_string_bytes(bytes: Vec<u8>) -> Value {
    match String::from_utf8(bytes) {
        Ok(s) => Value::String(s),
        Err(err) => encode_go_string_bytes_value(&err.into_bytes()),
    }
}

enum MapKeyArg {
    Key(String),
    StringLikeNonUtf8,
    WrongType,
}

fn map_key_arg(v: &Option<Value>) -> MapKeyArg {
    match v.as_ref() {
        Some(Value::String(s)) => MapKeyArg::Key(s.clone()),
        Some(other) if go_string_bytes_len(other).is_some() => {
            let Some(bytes) = decode_go_string_bytes_value(other) else {
                return MapKeyArg::StringLikeNonUtf8;
            };
            match String::from_utf8(bytes) {
                Ok(s) => MapKeyArg::Key(s),
                Err(_) => MapKeyArg::StringLikeNonUtf8,
            }
        }
        _ => MapKeyArg::WrongType,
    }
}

fn option_string_like_bytes(v: &Option<Value>) -> Option<Cow<'_, [u8]>> {
    match v.as_ref() {
        Some(Value::String(s)) => Some(Cow::Borrowed(s.as_bytes())),
        Some(other) => decode_go_string_bytes_value(other).map(Cow::Owned),
        None => None,
    }
}

fn is_go_bytes_slice_option(v: &Option<Value>) -> bool {
    v.as_ref()
        .is_some_and(|value| go_bytes_len(value).is_some())
}

fn is_map_object_option(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(|value| {
        matches!(value, Value::Object(_))
            && go_bytes_len(value).is_none()
            && go_string_bytes_len(value).is_none()
            && decode_go_typed_slice_value(value).is_none()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonComparableKind {
    Slice,
    Map,
}

fn non_comparable_kind_option(v: &Option<Value>) -> Option<NonComparableKind> {
    match v.as_ref() {
        Some(Value::Array(_)) => Some(NonComparableKind::Slice),
        Some(value) if go_bytes_len(value).is_some() => Some(NonComparableKind::Slice),
        Some(value) if decode_go_typed_slice_value(value).is_some() => {
            Some(NonComparableKind::Slice)
        }
        Some(value)
            if matches!(value, Value::Object(_))
                && go_bytes_len(value).is_none()
                && go_string_bytes_len(value).is_none()
                && decode_go_typed_slice_value(value).is_none() =>
        {
            Some(NonComparableKind::Map)
        }
        _ => None,
    }
}

fn format_non_comparable_type_reason(v: &Option<Value>) -> String {
    format!(
        "error calling eq: non-comparable type {}: {}",
        format_value_for_print(v),
        option_type_name_for_template(v)
    )
}

fn format_non_comparable_types_reason(a: &Option<Value>, b: &Option<Value>) -> String {
    format!(
        "error calling eq: non-comparable types {}: {}, {}: {}",
        format_value_for_print(a),
        option_type_name_for_template(a),
        option_type_name_for_template(b),
        format_value_for_print(b)
    )
}

fn option_type_name_for_template(v: &Option<Value>) -> String {
    match v.as_ref() {
        Some(value) => value_type_name_for_template(value),
        None => "<nil>".to_string(),
    }
}

fn value_type_name_for_template(v: &Value) -> String {
    if go_bytes_len(v).is_some() {
        return "[]uint8".to_string();
    }
    if go_string_bytes_len(v).is_some() {
        return "string".to_string();
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(v) {
        return format!("[]{}", typed_slice.elem_type);
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format!("map[string]{}", typed_map.elem_type);
    }
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(_) => "[]interface {}".to_string(),
        Value::Object(_) => "map[string]interface {}".to_string(),
        Value::Number(n) => {
            if n.as_i64().is_some() {
                "int".to_string()
            } else if n.as_u64().is_some() {
                "uint".to_string()
            } else {
                "float64".to_string()
            }
        }
    }
}

fn range_items(
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

fn apply_range_iteration_bindings(
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

fn action_inner(action: &str) -> Option<&str> {
    if !(action.starts_with("{{") && action.ends_with("}}")) || action.len() < 4 {
        return None;
    }
    let inner = &action[2..action.len() - 2];
    let bytes = inner.as_bytes();
    let mut start = 0usize;
    let mut end = inner.len();

    if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1].is_ascii_whitespace() {
        start = 1;
    }
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while start < end && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end > start && bytes[end - 1] == b'-' {
        end -= 1;
        while start < end && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
    }
    Some(&inner[start..end])
}

fn parse_number_value(expr: &str) -> Option<Value> {
    compat::parse_number_value(expr)
}

fn parse_char_constant(expr: &str) -> Option<i64> {
    compat::parse_char_constant(expr)
}

fn format_value_like_go(v: &Value) -> String {
    if let Some(bytes) = decode_go_bytes_value(v) {
        let mut out = String::from("[");
        for (idx, b) in bytes.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            out.push_str(&b.to_string());
        }
        out.push(']');
        return out;
    }
    if let Some(bytes) = decode_go_string_bytes_value(v) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    if let Some(typed_slice) = decode_go_typed_slice_value(v) {
        let mut out = String::from("[");
        if let Some(items) = typed_slice.items {
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format_value_like_go(item));
            }
        }
        out.push(']');
        return out;
    }
    if let Some(typed_map) = decode_go_typed_map_value(v) {
        return format_map_entries_like_go(typed_map.entries);
    }
    match v {
        Value::Null => "<no value>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = String::from("[");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format_value_like_go(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => format_map_entries_like_go(Some(map)),
    }
}

fn format_map_entries_like_go(entries: Option<&serde_json::Map<String, Value>>) -> String {
    let mut out = String::from("map[");
    if let Some(map) = entries {
        let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        for (idx, k) in keys.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            out.push_str(k);
            out.push(':');
            if let Some(v) = map.get(*k) {
                out.push_str(&format_value_like_go(v));
            }
        }
    }
    out.push(']');
    out
}

fn decode_string_literal(inner: &str) -> Option<String> {
    compat::decode_go_string_literal(inner)
}

fn is_quoted_string(inner: &str) -> bool {
    inner.len() >= 2
        && ((inner.starts_with('"') && inner.ends_with('"'))
            || (inner.starts_with('`') && inner.ends_with('`')))
}

fn is_complex_expression(expr: &str) -> bool {
    if expr.is_empty() {
        return false;
    }
    if is_quoted_string(expr) {
        return false;
    }
    if expr.contains('|')
        || expr.contains('(')
        || expr.contains(')')
        || expr.contains(":=")
        || expr.contains(',')
    {
        return true;
    }
    if expr.contains('=') && !expr.starts_with('=') {
        return true;
    }
    if expr.contains(char::is_whitespace) {
        return true;
    }
    false
}

fn is_niladic_function_expression(expr: &str) -> bool {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return false;
    }
    if matches!(trimmed, "true" | "false" | "nil") {
        return false;
    }
    is_identifier_name(trimmed)
}

#[cfg(test)]
mod tests;
