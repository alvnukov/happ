use super::{
    compat, parse_template_tokens_strict_with_options,
    typedvalue::{
        decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
        encode_go_bytes_value, encode_go_string_bytes_value, go_bytes_get, go_bytes_len,
        go_string_bytes_get, go_string_bytes_len, go_zero_value_for_type,
    },
    utf8scan::push_utf8_char_from_bytes,
    GoTemplateScanError, GoTemplateToken, ParseCompatOptions, HELM_INCLUDE_RECURSION_MAX_REFS,
};
use serde_json::{Number, Value};
use std::borrow::Cow;
use std::collections::BTreeMap;
mod compare;
mod path;
use compare::{builtin_cmp, builtin_eq, builtin_ne};
use path::{
    is_identifier_continue_char, is_identifier_start_char, resolve_simple_path,
    split_variable_reference,
};

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
        pipe = eval_pipeline_command(action, command, root, dot, idx > 0, pipe, state, resolver)?;
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
    has_pipe_input: bool,
    pipe_input: Option<Value>,
    state: &mut EvalState,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<Option<Value>, NativeRenderError> {
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

    if has_pipe_input {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "non executable command in pipeline stage".to_string(),
        });
    }

    if tokens.len() == 1 {
        return eval_command_token_value(action, &tokens[0], root, dot, state, resolver);
    }

    Err(NativeRenderError::UnsupportedAction {
        action: action.to_string(),
        reason: format!("unknown function: {head}"),
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
    let Some(resolver) = resolver else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "call requires external function resolver".to_string(),
        });
    };
    let Some(first) = arg_tokens.first() else {
        return Err(NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: "error calling call: function argument is missing".to_string(),
        });
    };

    let fn_name = if is_identifier_name(first) {
        first.clone()
    } else {
        let evaluated = eval_command_token_value(action, first, root, dot, state, Some(resolver))?;
        match evaluated {
            Some(Value::String(s)) => s,
            _ => {
                return Err(NativeRenderError::UnsupportedAction {
                    action: action.to_string(),
                    reason: "error calling call: first argument must resolve to function name"
                        .to_string(),
                });
            }
        }
    };

    let mut args =
        Vec::with_capacity(arg_tokens.len().saturating_sub(1) + usize::from(has_pipe_input));
    for token in arg_tokens.iter().skip(1) {
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
        Ok(v) => Ok(v),
        Err(NativeFunctionResolverError::UnknownFunction) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("unknown function: {fn_name}"),
            })
        }
        Err(NativeFunctionResolverError::Failed { reason }) => {
            Err(NativeRenderError::UnsupportedAction {
                action: action.to_string(),
                reason: format!("error calling {fn_name}: {reason}"),
            })
        }
    }
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

fn builtin_print(args: &[Option<Value>], with_newline: bool) -> String {
    let mut out = String::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let piece = format_value_for_print(arg);
        let cur_is_string = arg
            .as_ref()
            .is_some_and(|v| matches!(v, Value::String(_)) || go_string_bytes_len(v).is_some());
        if idx > 0 && !prev_is_string && !cur_is_string {
            out.push(' ');
        }
        out.push_str(&piece);
        prev_is_string = cur_is_string;
    }
    if with_newline {
        out.push('\n');
    }
    out
}

fn builtin_urlquery(args: &[Option<Value>]) -> String {
    query_escape_bytes(&join_text_template_args_bytes(args))
}

fn builtin_html(args: &[Option<Value>]) -> String {
    html_escape(&join_text_template_args(args))
}

fn builtin_js(args: &[Option<Value>]) -> String {
    js_escape(&join_text_template_args(args))
}

fn join_text_template_args(args: &[Option<Value>]) -> String {
    let mut joined = String::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let piece = match arg {
            None => "<no value>".to_string(),
            Some(v) => format_value_like_go(v),
        };
        let cur_is_string = arg
            .as_ref()
            .is_some_and(|v| matches!(v, Value::String(_)) || go_string_bytes_len(v).is_some());
        if idx > 0 && !prev_is_string && !cur_is_string {
            joined.push(' ');
        }
        joined.push_str(&piece);
        prev_is_string = cur_is_string;
    }
    joined
}

fn join_text_template_args_bytes(args: &[Option<Value>]) -> Vec<u8> {
    let mut joined = Vec::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let (piece, cur_is_string) = match arg {
            None => (b"<no value>".as_slice().to_vec(), false),
            Some(Value::String(s)) => (s.as_bytes().to_vec(), true),
            Some(v) => {
                if let Some(bytes) = decode_go_string_bytes_value(v) {
                    (bytes, true)
                } else {
                    (format_value_like_go(v).into_bytes(), false)
                }
            }
        };
        if idx > 0 && !prev_is_string && !cur_is_string {
            joined.push(b' ');
        }
        joined.extend_from_slice(&piece);
        prev_is_string = cur_is_string;
    }
    joined
}

fn query_escape_bytes(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() + input.len() / 3);
    for b in input {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex_upper((*b >> 4) & 0x0F));
                out.push(hex_upper(*b & 0x0F));
            }
        }
    }
    out
}

fn html_escape(input: &str) -> String {
    if !input
        .chars()
        .any(|ch| matches!(ch, '\'' | '"' | '&' | '<' | '>' | '\0'))
    {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len() + input.len() / 4);
    for ch in input.chars() {
        match ch {
            '\0' => out.push('\u{FFFD}'),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn js_escape(input: &str) -> String {
    if !input.chars().any(js_is_special) {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len() + input.len() / 4);
    for ch in input.chars() {
        if !js_is_special(ch) {
            out.push(ch);
            continue;
        }
        if ch.is_ascii() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '"' => out.push_str("\\\""),
                '<' => out.push_str("\\u003C"),
                '>' => out.push_str("\\u003E"),
                '&' => out.push_str("\\u0026"),
                '=' => out.push_str("\\u003D"),
                _ => {
                    let v = ch as u32;
                    out.push_str("\\u00");
                    out.push(hex_upper(((v >> 4) & 0x0F) as u8));
                    out.push(hex_upper((v & 0x0F) as u8));
                }
            }
            continue;
        }

        if ch.is_control() {
            let v = ch as u32;
            let code = format!("{v:04X}");
            out.push_str("\\u");
            out.push_str(&code);
        } else {
            out.push(ch);
        }
    }
    out
}

fn js_is_special(ch: char) -> bool {
    matches!(ch, '\\' | '\'' | '"' | '<' | '>' | '&' | '=') || ch < ' ' || !ch.is_ascii()
}

fn hex_upper(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => '0',
    }
}

fn format_value_for_print(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => format_value_like_go(other),
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
        Some(value)
            if matches!(value, Value::Object(_))
                && go_bytes_len(value).is_none()
                && go_string_bytes_len(value).is_none() =>
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

fn strip_outer_parens(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
        return None;
    }
    let mut depth = 0i32;
    let bytes = trimmed.as_bytes();
    for (i, ch) in bytes.iter().enumerate() {
        match *ch {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 && i + 1 < bytes.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(&trimmed[1..trimmed.len() - 1])
}

fn split_pipeline_commands(inner: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
        Comment,
    }

    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut paren_depth: i32 = 0;
    let mut state = State::Normal;

    while i < bytes.len() {
        match state {
            State::Normal => {
                if starts_with(bytes, i, b"/*") {
                    state = State::Comment;
                    i += 2;
                    continue;
                }
                match bytes[i] {
                    b'\'' => {
                        state = State::SingleQuote;
                        i += 1;
                    }
                    b'"' => {
                        state = State::DoubleQuote;
                        i += 1;
                    }
                    b'`' => {
                        state = State::RawQuote;
                        i += 1;
                    }
                    b'(' => {
                        paren_depth += 1;
                        i += 1;
                    }
                    b')' => {
                        if paren_depth > 0 {
                            paren_depth -= 1;
                        }
                        i += 1;
                    }
                    b'|' if paren_depth == 0 => {
                        let cmd = inner[start..i].trim();
                        if !cmd.is_empty() {
                            out.push(cmd.to_string());
                        }
                        start = i + 1;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            State::SingleQuote => {
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == b'\'' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::DoubleQuote => {
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == b'"' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::RawQuote => {
                if bytes[i] == b'`' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::Comment => {
                if starts_with(bytes, i, b"*/") {
                    state = State::Normal;
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
    }

    if start <= inner.len() {
        let cmd = inner[start..].trim();
        if !cmd.is_empty() {
            out.push(cmd.to_string());
        }
    }
    out
}

fn split_command_tokens(command: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
    }

    let bytes = command.as_bytes();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;
    let mut state = State::Normal;
    let mut paren_depth = 0i32;

    while i < bytes.len() {
        match state {
            State::Normal => {
                if bytes[i].is_ascii_whitespace() && paren_depth == 0 {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
                    i += 1;
                    continue;
                }
                if bytes[i] == b',' && paren_depth == 0 {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
                    out.push(",".to_string());
                    i += 1;
                    continue;
                }
                match bytes[i] {
                    b'\'' => {
                        state = State::SingleQuote;
                        buf.push('\'');
                        i += 1;
                    }
                    b'"' => {
                        state = State::DoubleQuote;
                        buf.push('"');
                        i += 1;
                    }
                    b'`' => {
                        state = State::RawQuote;
                        buf.push('`');
                        i += 1;
                    }
                    b'(' => {
                        paren_depth += 1;
                        buf.push('(');
                        i += 1;
                    }
                    b')' => {
                        if paren_depth > 0 {
                            paren_depth -= 1;
                        }
                        buf.push(')');
                        i += 1;
                    }
                    _ => {
                        i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    }
                }
            }
            State::SingleQuote => {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push('\\');
                    i += 1;
                    i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    continue;
                }
                if bytes[i] == b'\'' {
                    buf.push('\'');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
            State::DoubleQuote => {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push('\\');
                    i += 1;
                    i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    continue;
                }
                if bytes[i] == b'"' {
                    buf.push('"');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
            State::RawQuote => {
                if bytes[i] == b'`' {
                    buf.push('`');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
        }
    }

    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn starts_with(haystack: &[u8], offset: usize, needle: &[u8]) -> bool {
    haystack
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|chunk| chunk == needle)
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

fn apply_lexical_trims(tokens: &mut [GoTemplateToken]) {
    for i in 0..tokens.len() {
        let action = match &tokens[i] {
            GoTemplateToken::Action(a) => a.clone(),
            GoTemplateToken::Literal(_) => continue,
        };
        if has_left_trim_marker(&action) && i > 0 {
            if let GoTemplateToken::Literal(prev) = &mut tokens[i - 1] {
                trim_right_ascii_whitespace_in_place(prev);
            }
        }
        if has_right_trim_marker(&action) && i + 1 < tokens.len() {
            if let GoTemplateToken::Literal(next) = &mut tokens[i + 1] {
                *next = trim_left_ascii_whitespace(next).to_string();
            }
        }
    }
}

fn has_left_trim_marker(action: &str) -> bool {
    action
        .as_bytes()
        .get(2..4)
        .is_some_and(|s| s[0] == b'-' && s[1].is_ascii_whitespace())
}

fn has_right_trim_marker(action: &str) -> bool {
    if action.len() < 4 || !action.ends_with("}}") {
        return false;
    }
    let bytes = action.as_bytes();
    let dash = bytes.len().saturating_sub(3);
    let prev = bytes.len().saturating_sub(4);
    bytes.get(dash).copied() == Some(b'-')
        && bytes
            .get(prev)
            .copied()
            .is_some_and(|b| b.is_ascii_whitespace())
}

fn trim_left_ascii_whitespace(s: &str) -> &str {
    let mut idx = 0usize;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_whitespace() {
            idx = i + ch.len_utf8();
            continue;
        }
        idx = i;
        break;
    }
    if s.is_empty() {
        s
    } else if idx >= s.len() {
        ""
    } else {
        &s[idx..]
    }
}

fn trim_right_ascii_whitespace_in_place(out: &mut String) {
    while out
        .as_bytes()
        .last()
        .copied()
        .is_some_and(|b| b.is_ascii_whitespace())
    {
        out.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_renderer_renders_literals_and_simple_paths() {
        let data = json!({"a":{"b":"ok"}});
        let out = render_template_native("A{{.a.b}}C", &data).expect("must render");
        assert_eq!(out, "AokC");
    }

    #[test]
    fn native_renderer_uses_go_missing_value_default() {
        let data = json!({"a":{"b":"ok"}});
        let out = render_template_native("{{.a.c}}", &data).expect("must render");
        assert_eq!(out, "<no value>");
    }

    #[test]
    fn native_renderer_go_zero_mode_keeps_leaf_missing_as_no_value() {
        let data = json!({"m":{"a":1}});
        let out = render_template_native_with_options(
            "{{.m.missing}}",
            &data,
            NativeRenderOptions {
                missing_value_mode: MissingValueMode::GoZero,
            },
        )
        .expect("must render");
        assert_eq!(out, "<no value>");
    }

    #[test]
    fn native_renderer_go_zero_mode_errors_on_nested_missing_after_nil_interface() {
        let data = json!({"m":{"a":1}});
        let err = render_template_native_with_options(
            "{{.m.missing.y}}",
            &data,
            NativeRenderOptions {
                missing_value_mode: MissingValueMode::GoZero,
            },
        )
        .expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("nil pointer evaluating interface {}.y"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_applies_trim_markers() {
        let data = json!({"a":{"b":"ok"}});
        let out = render_template_native("x {{- .a.b -}} y", &data).expect("must render");
        assert_eq!(out, "xoky");
    }

    #[test]
    fn native_renderer_supports_if_with_else() {
        let data = json!({"flag": false});
        let out =
            render_template_native("{{if .flag}}yes{{else}}no{{end}}", &data).expect("must render");
        assert_eq!(out, "no");
    }

    #[test]
    fn native_renderer_supports_with() {
        let data = json!({"user": {"name":"alice"}});
        let out = render_template_native("{{with .user}}{{.name}}{{else}}none{{end}}", &data)
            .expect("must render");
        assert_eq!(out, "alice");
    }

    #[test]
    fn native_renderer_supports_range_with_else() {
        let data = json!({"items": ["a", "b"]});
        let out = render_template_native("{{range .items}}{{.}}{{else}}empty{{end}}", &data)
            .expect("must render");
        assert_eq!(out, "ab");

        let empty = json!({"items": []});
        let out = render_template_native("{{range .items}}{{.}}{{else}}empty{{end}}", &empty)
            .expect("must render");
        assert_eq!(out, "empty");
    }

    #[test]
    fn native_renderer_supports_template_invocation() {
        let data = json!({"v":"x"});
        let tpl = "{{define \"t\"}}<{{.v}}>{{end}}{{template \"t\" .}}";
        let out = render_template_native(tpl, &data).expect("must render");
        assert_eq!(out, "<x>");
    }

    #[test]
    fn native_renderer_supports_template_invocation_with_arg() {
        let data = json!({"v":"x","user":{"name":"alice"}});
        let tpl = "{{define \"name\"}}{{.name}}{{end}}{{template \"name\" .user}}";
        let out = render_template_native(tpl, &data).expect("must render");
        assert_eq!(out, "alice");
    }

    #[test]
    fn native_renderer_supports_pipeline_and_builtins_subset() {
        let data = json!({
            "items": ["x", "y"],
            "m": {"k":"v"},
            "n": 7,
            "s": "ok"
        });
        let out = render_template_native("{{print 1 2}}", &data).expect("must render");
        assert_eq!(out, "1 2");
        let out = render_template_native("{{printf \"%s-%d\" .s 7}}", &data).expect("must render");
        assert_eq!(out, "ok-7");
        let out = render_template_native("{{printf \"%f\" 1.2}}", &data).expect("must render");
        assert_eq!(out, "1.200000");
        let out = render_template_native("{{printf \"%.2f\" 1.2}}", &data).expect("must render");
        assert_eq!(out, "1.20");
        let out = render_template_native("{{printf \"%e\" 1.2}}", &data).expect("must render");
        assert_eq!(out, "1.200000e+00");
        let out = render_template_native("{{printf \"%E\" 1.2}}", &data).expect("must render");
        assert_eq!(out, "1.200000E+00");
        let out = render_template_native("{{printf \"%o\" 9}}", &data).expect("must render");
        assert_eq!(out, "11");
        let out = render_template_native("{{printf \"%b\" 9}}", &data).expect("must render");
        assert_eq!(out, "1001");
        let out = render_template_native("{{printf \"%g\" 3.5}}", &data).expect("must render");
        assert_eq!(out, "3.5");
        let out =
            render_template_native("{{printf \"%G\" 1234567.0}}", &data).expect("must render");
        assert_eq!(out, "1.234567E+06");
        let out = render_template_native("{{printf \"%T\" 0xef}}", &data).expect("must render");
        assert_eq!(out, "int");
        let out = render_template_native("{{printf \"%04x\" -1}}", &data).expect("must render");
        assert_eq!(out, "-001");
        let out = render_template_native("{{3 | printf \"%d\"}}", &data).expect("must render");
        assert_eq!(out, "3");
        let out = render_template_native("{{len .items}}", &data).expect("must render");
        assert_eq!(out, "2");
        let out = render_template_native("{{index .items 1}}", &data).expect("must render");
        assert_eq!(out, "y");
        let out = render_template_native("{{index .m \"k\"}}", &data).expect("must render");
        assert_eq!(out, "v");
        let out = render_template_native("{{or .missing \"x\"}}", &data).expect("must render");
        assert_eq!(out, "x");
        let out = render_template_native("{{and .missing \"x\"}}", &data).expect("must render");
        assert_eq!(out, "<no value>");
        let out = render_template_native("{{slice .items 1}}", &data).expect("must render");
        assert_eq!(out, "[y]");
        let out = render_template_native("{{slice \"abcd\" 1 3}}", &data).expect("must render");
        assert_eq!(out, "bc");
        let out = render_template_native("{{urlquery \"a b\" \"+\"}}", &data).expect("must render");
        assert_eq!(out, "a+b%2B");
        let out = render_template_native("{{urlquery (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "%97");
        let out = render_template_native("{{urlquery .missing}}", &data).expect("must render");
        assert_eq!(out, "%3Cno+value%3E");
        let out =
            render_template_native("{{html \"<x&'\\\"\\u0000>\"}}", &data).expect("must render");
        assert_eq!(out, "&lt;x&amp;&#39;&#34;\u{FFFD}&gt;");
        let out = render_template_native("{{js \"<x&'\\\"=\\n>\"}}", &data).expect("must render");
        assert_eq!(out, "\\u003Cx\\u0026\\'\\\"\\u003D\\u000A\\u003E");
    }

    #[test]
    fn native_renderer_keeps_go_type_strictness_for_numeric_ops() {
        let data = json!({"items":["x","y"], "m":{"1":"v"}});
        let out = render_template_native("{{printf \"%d\" \"7\"}}", &data).expect("must render");
        assert_eq!(out, "%!d(string=7)");
        let err = render_template_native("{{index .items \"1\"}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("cannot index slice/array with type string"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let err = render_template_native("{{index .m 1}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("value has type int; should be string"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_builtins_support_typed_go_bytes() {
        let mut data = serde_json::Map::new();
        data.insert(
            "b".to_string(),
            crate::gotemplates::encode_go_bytes_value(b"abc"),
        );
        let root = Value::Object(data);

        let out = render_template_native("{{len .b}}", &root).expect("must render");
        assert_eq!(out, "3");

        let out = render_template_native("{{index .b 1}}", &root).expect("must render");
        assert_eq!(out, "98");

        let out =
            render_template_native("{{printf \"%s\" (slice .b 1 3)}}", &root).expect("must render");
        assert_eq!(out, "bc");
    }

    #[test]
    fn native_renderer_treats_typed_go_bytes_as_slice_in_if_and_range() {
        let mut non_empty = serde_json::Map::new();
        non_empty.insert(
            "b".to_string(),
            crate::gotemplates::encode_go_bytes_value(b"ab"),
        );
        let non_empty = Value::Object(non_empty);
        let out = render_template_native("{{if .b}}yes{{else}}no{{end}}", &non_empty)
            .expect("must render");
        assert_eq!(out, "yes");
        let out = render_template_native("{{range $i, $v := .b}}{{$i}}:{{$v}};{{end}}", &non_empty)
            .expect("must render");
        assert_eq!(out, "0:97;1:98;");

        let mut empty = serde_json::Map::new();
        empty.insert(
            "b".to_string(),
            crate::gotemplates::encode_go_bytes_value(b""),
        );
        let empty = Value::Object(empty);
        let out =
            render_template_native("{{if .b}}yes{{else}}no{{end}}", &empty).expect("must render");
        assert_eq!(out, "no");
    }

    #[test]
    fn native_renderer_go_zero_mode_returns_typed_map_zero_values() {
        let mut int_entries = serde_json::Map::new();
        int_entries.insert("a".to_string(), Value::Number(Number::from(1)));

        let mut root = serde_json::Map::new();
        root.insert(
            "m".to_string(),
            crate::gotemplates::encode_go_typed_map_value("int", Some(int_entries)),
        );
        let root = Value::Object(root);

        let out = render_template_native_with_options(
            "{{.m.missing}}|{{printf \"%T\" .m.missing}}",
            &root,
            NativeRenderOptions {
                missing_value_mode: MissingValueMode::GoZero,
            },
        )
        .expect("must render");
        assert_eq!(out, "0|int");

        let out = render_template_native(
            "{{index .m \"missing\"}}|{{printf \"%T\" (index .m \"missing\")}}",
            &root,
        )
        .expect("must render");
        assert_eq!(out, "0|int");
    }

    #[test]
    fn native_renderer_handles_nested_typed_map_missing_like_go() {
        let mut inner = serde_json::Map::new();
        inner.insert("y".to_string(), Value::Number(Number::from(2)));
        let mut outer = serde_json::Map::new();
        outer.insert(
            "x".to_string(),
            crate::gotemplates::encode_go_typed_map_value("int", Some(inner)),
        );
        let mut root = serde_json::Map::new();
        root.insert(
            "m".to_string(),
            crate::gotemplates::encode_go_typed_map_value("map[string]int", Some(outer)),
        );
        root.insert(
            "nilMap".to_string(),
            crate::gotemplates::encode_go_typed_map_value("int", None),
        );
        let root = Value::Object(root);

        let out = render_template_native_with_options(
            "{{.m.missing.y}}|{{index .m \"missing\"}}|{{printf \"%T\" (index .m \"missing\")}}",
            &root,
            NativeRenderOptions {
                missing_value_mode: MissingValueMode::GoZero,
            },
        )
        .expect("must render");
        assert_eq!(out, "0|map[]|map[string]int");

        let out = render_template_native_with_options(
            "{{.m.missing.y}}",
            &root,
            NativeRenderOptions {
                missing_value_mode: MissingValueMode::GoDefault,
            },
        )
        .expect("must render");
        assert_eq!(out, "<no value>");

        let out = render_template_native("{{len .nilMap}}", &root).expect("must render");
        assert_eq!(out, "0");
        let out = render_template_native("{{range .nilMap}}x{{else}}empty{{end}}", &root)
            .expect("must render");
        assert_eq!(out, "empty");
    }

    #[test]
    fn native_renderer_builtin_errors_follow_go_text_template_style() {
        let data = json!({"items":["x"], "s":"abc"});

        let err = render_template_native("{{len 3}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling len: len of type int"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{index 1 0}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling index: can't index item of type int"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let data = json!({"m": {"k": "v"}});
        let err = render_template_native("{{index .m nil}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling index: value is nil; should be string"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let data = json!({"items":["x"], "u": u64::MAX});
        let err = render_template_native("{{index .items .u}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling index: index out of range: -1"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{slice 1 0}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling slice: can't slice item of type int"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{range true}}x{{end}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("range can't iterate over true"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{range 1.5}}x{{end}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("range can't iterate over 1.5"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{lt true false}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling lt: invalid type for comparison"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{lt true 1}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling lt: incompatible types for comparison"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_eq_reports_non_comparable_like_go_text_template() {
        let mut m = serde_json::Map::new();
        m.insert(
            "arr".to_string(),
            Value::Array(vec![
                Value::Number(Number::from(1)),
                Value::Number(Number::from(2)),
            ]),
        );
        let mut obj = serde_json::Map::new();
        obj.insert("a".to_string(), Value::Number(Number::from(1)));
        m.insert("mapv".to_string(), Value::Object(obj));
        m.insert(
            "bytes".to_string(),
            crate::gotemplates::encode_go_bytes_value(b"ab"),
        );
        let data = Value::Object(m);

        let err = render_template_native("{{eq .arr .arr}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling eq: non-comparable type"));
                assert!(reason.contains("[]interface {}"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{eq .arr .mapv}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling eq: non-comparable types"));
                assert!(reason.contains("[]interface {}"));
                assert!(reason.contains("map[string]interface {}"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{eq .bytes .arr}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling eq: non-comparable type"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_compares_string_bytes_like_go_strings() {
        let data = json!({"m":{"a":"ok"}});

        let out = render_template_native("{{eq (slice \"日本\" 1 2) (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "true");

        let out = render_template_native("{{ne (slice \"日本\" 1 2) (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "false");

        let out = render_template_native("{{lt (slice \"ab\" 0 1) (slice \"ab\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "true");

        let err =
            render_template_native("{{eq (slice \"日本\" 1 2) .m}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling eq: incompatible types for comparison"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_allows_string_bytes_as_map_index_key() {
        let data = json!({"m":{"a":"ok"}, "m2":{"�":"hit"}});

        let out =
            render_template_native("{{index .m (slice \"ab\" 0 1)}}", &data).expect("must render");
        assert_eq!(out, "ok");

        let out = render_template_native("{{index .m (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "<no value>");

        let out = render_template_native("{{index .m2 (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "<no value>");
    }

    #[test]
    fn native_renderer_matches_builtin_arity_and_index_identity() {
        let data = json!({"x": 1});
        let out = render_template_native("{{index 1}}", &data).expect("must render");
        assert_eq!(out, "1");

        assert!(render_template_native("{{and}}", &data).is_err());
        assert!(render_template_native("{{or}}", &data).is_err());
        assert!(render_template_native("{{not}}", &data).is_err());
        assert!(render_template_native("{{not 1 2}}", &data).is_err());
        assert!(render_template_native("{{eq}}", &data).is_err());
        assert!(render_template_native("{{eq 1}}", &data).is_err());
        assert!(render_template_native("{{ne 1 2 3}}", &data).is_err());
        assert!(render_template_native("{{lt 1}}", &data).is_err());
        assert!(render_template_native("{{len}}", &data).is_err());
        assert!(render_template_native("{{slice}}", &data).is_err());
        assert!(render_template_native("{{printf}}", &data).is_err());
    }

    #[test]
    fn native_renderer_supports_variable_declare_and_assign() {
        let data = json!({"v":"rootv"});
        let out =
            render_template_native("{{$x := .v}}{{$x = \"b\"}}{{$x}}", &data).expect("must render");
        assert_eq!(out, "b");
    }

    #[test]
    fn native_renderer_supports_range_variable_declarations() {
        let data = json!({"items":["a","b"]});
        let out = render_template_native("{{range $i, $v := .items}}{{$i}}={{$v}};{{end}}", &data)
            .expect("must render");
        assert_eq!(out, "0=a;1=b;");
    }

    #[test]
    fn native_renderer_rejects_range_over_integer_like_go() {
        let data = json!({});
        let err = render_template_native("{{range 3}}{{.}}{{end}}", &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("range can't iterate over 3"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = render_template_native("{{range $i, $v := 3}}{{$i}}={{$v}};{{end}}", &data)
            .expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("range can't iterate over 3"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_supports_range_break_and_continue() {
        let data = json!({"items":[0,1,2,3]});
        let out = render_template_native(
            "{{range .items}}{{if eq . 2}}{{break}}{{end}}{{.}}{{end}}",
            &data,
        )
        .expect("must render");
        assert_eq!(out, "01");
        let out = render_template_native(
            "{{range .items}}{{if eq . 2}}{{continue}}{{end}}{{.}}{{end}}",
            &data,
        )
        .expect("must render");
        assert_eq!(out, "013");
        assert!(render_template_native("{{break}}", &data).is_err());
        assert!(render_template_native("{{continue}}", &data).is_err());
    }

    #[test]
    fn native_renderer_slice_string_keeps_byte_semantics_for_printf() {
        let data = json!({});
        let out = render_template_native("{{printf \"%x\" (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "97");

        let out = render_template_native("{{printf \"%q\" (slice \"日本\" 1 2)}}", &data)
            .expect("must render");
        assert_eq!(out, "\"\\x97\"");
    }

    #[test]
    fn native_renderer_preserves_unicode_literals_in_function_args() {
        let data = json!({});
        let out =
            render_template_native("{{printf \"%s\" \"日本語\"}}", &data).expect("must render");
        assert_eq!(out, "日本語");
    }

    #[test]
    fn native_renderer_range_else_exposes_declared_variable() {
        let data = json!({"empty":[]});
        let out = render_template_native("{{range $v := .empty}}x{{else}}{{$v}}{{end}}", &data)
            .expect("must render");
        assert_eq!(out, "[]");
    }

    #[test]
    fn native_renderer_template_call_resets_root_context() {
        let data = json!({"v":"rootv","user":{"v":"userv"}});
        let out = render_template_native(
            "{{define \"t\"}}{{$.v}}{{end}}{{template \"t\" .user}}",
            &data,
        )
        .expect("must render");
        assert_eq!(out, "userv");
    }

    #[test]
    fn native_renderer_supports_block_action() {
        let data = json!({"user":{"name":"alice"}});
        let out = render_template_native("{{block \"b\" .user}}{{.name}}{{end}}", &data)
            .expect("must render");
        assert_eq!(out, "alice");
    }

    #[test]
    fn native_renderer_and_or_short_circuit_matches_go() {
        let data = json!({});
        let out = render_template_native("{{or 0 1 (index nil 0)}}", &data).expect("must render");
        assert_eq!(out, "1");

        let out = render_template_native("{{and 1 0 (index nil 0)}}", &data).expect("must render");
        assert_eq!(out, "0");

        assert!(render_template_native("{{or 0 0 (index nil 0)}}", &data).is_err());
        assert!(render_template_native("{{and 1 1 (index nil 0)}}", &data).is_err());
    }

    #[test]
    fn native_renderer_supports_external_function_resolver_with_args() {
        let data = json!({});
        let out = render_template_native_with_resolver(
            "{{ext \"a\" 2}}",
            &data,
            NativeRenderOptions::default(),
            Some(&|name: &str, args: &[Option<Value>]| {
                if name != "ext" {
                    return Err(NativeFunctionResolverError::UnknownFunction);
                }
                assert_eq!(args.len(), 2);
                Ok(Some(Value::String(format!(
                    "{}:{}",
                    format_value_for_print(&args[0]),
                    format_value_for_print(&args[1])
                ))))
            }),
        )
        .expect("must render");
        assert_eq!(out, "a:2");
    }

    #[test]
    fn native_renderer_supports_call_builtin_via_resolver() {
        let data = json!({"fn":"ext"});
        let resolver = |name: &str, args: &[Option<Value>]| {
            if name != "ext" {
                return Err(NativeFunctionResolverError::UnknownFunction);
            }
            Ok(Some(Value::String(format!(
                "called:{}",
                format_value_for_print(&args[0])
            ))))
        };
        let out = render_template_native_with_resolver(
            "{{call ext \"x\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "called:x");
        let out = render_template_native_with_resolver(
            "{{call .fn \"y\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "called:y");
        let out = render_template_native_with_resolver(
            "{{.fn \"z\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "called:z");
    }

    #[test]
    fn native_renderer_supports_unicode_identifiers_in_resolver_and_paths() {
        let data = json!({"fn":"привет","данные":{"ключ":"значение"}});
        let resolver = |name: &str, args: &[Option<Value>]| {
            if name != "привет" {
                return Err(NativeFunctionResolverError::UnknownFunction);
            }
            Ok(Some(Value::String(format!(
                "ok:{}",
                format_value_for_print(&args[0])
            ))))
        };

        let out = render_template_native_with_resolver(
            "{{привет \"мир\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "ok:мир");

        let out = render_template_native_with_resolver(
            "{{call .fn \"x\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "ok:x");

        let out = render_template_native_with_resolver(
            "{{.fn \"y\"}}",
            &data,
            NativeRenderOptions::default(),
            Some(&resolver),
        )
        .expect("must render");
        assert_eq!(out, "ok:y");

        let out =
            render_template_native("{{$имя := .данные.ключ}}{{$имя}}", &data).expect("must render");
        assert_eq!(out, "значение");
    }

    #[test]
    fn native_renderer_supports_external_niladic_function() {
        let data = json!({"ext":"value-from-data"});
        let out = render_template_native_with_resolver(
            "{{ext}}",
            &data,
            NativeRenderOptions::default(),
            Some(&|name: &str, _args: &[Option<Value>]| {
                if name == "ext" {
                    Ok(Some(Value::String("value-from-resolver".to_string())))
                } else {
                    Err(NativeFunctionResolverError::UnknownFunction)
                }
            }),
        )
        .expect("must render");
        assert_eq!(out, "value-from-resolver");
    }

    #[test]
    fn native_renderer_external_resolver_can_return_typed_go_bytes() {
        let data = json!({});
        let out = render_template_native_with_resolver(
            "{{printf \"%s\" (ext)}}",
            &data,
            NativeRenderOptions::default(),
            Some(&|name: &str, _args: &[Option<Value>]| {
                if name == "ext" {
                    Ok(Some(crate::gotemplates::encode_go_bytes_value(b"ab")))
                } else {
                    Err(NativeFunctionResolverError::UnknownFunction)
                }
            }),
        )
        .expect("must render");
        assert_eq!(out, "ab");
    }

    #[test]
    fn native_renderer_reports_external_function_error() {
        let data = json!({});
        let err = render_template_native_with_resolver(
            "{{ext 1}}",
            &data,
            NativeRenderOptions::default(),
            Some(&|name: &str, _args: &[Option<Value>]| {
                if name == "ext" {
                    Err(NativeFunctionResolverError::Failed {
                        reason: "boom".to_string(),
                    })
                } else {
                    Err(NativeFunctionResolverError::UnknownFunction)
                }
            }),
        )
        .expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains("error calling ext: boom"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn native_renderer_parses_go_char_literal_escapes() {
        let data = json!({});
        let out = render_template_native("{{print '\\n'}}", &data).expect("must render");
        assert_eq!(out, "10");
        let out = render_template_native("{{print '\\x41'}}", &data).expect("must render");
        assert_eq!(out, "65");
        let out = render_template_native("{{print '\\u263A'}}", &data).expect("must render");
        assert_eq!(out, "9786");
        let out = render_template_native("{{print '\\U0001F600'}}", &data).expect("must render");
        assert_eq!(out, "128512");
        assert!(render_template_native("{{print '\\400'}}", &data).is_err());
    }

    #[test]
    fn native_renderer_validates_go_number_underscore_syntax() {
        let data = json!({});
        let out = render_template_native("{{print 0x_10}}", &data).expect("must render");
        assert_eq!(out, "16");
        assert!(render_template_native("{{print 1__2}}", &data).is_err());
        assert!(render_template_native("{{print 12_}}", &data).is_err());
    }

    #[test]
    fn native_renderer_reports_undefined_variable_from_outer_scope_in_define() {
        let data = json!({"v":"rootv"});
        let err = render_template_native(
            "{{$x := \"outer\"}}{{define \"t\"}}{{$x}}{{end}}{{template \"t\" .}}",
            &data,
        )
        .expect_err("must fail");
        match err {
            NativeRenderError::Parse(parse) => assert_eq!(parse.code, "undefined_variable"),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
