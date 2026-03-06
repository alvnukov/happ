use super::{
    compat, parse_template_tokens_strict_with_options,
    typedvalue::{
        decode_go_bytes_value, decode_go_string_bytes_value, decode_go_typed_map_value,
        decode_go_typed_slice_value, encode_go_bytes_value, encode_go_nil_bytes_value,
        encode_go_typed_slice_value, go_bytes_get, go_bytes_is_nil, go_bytes_len,
        go_string_bytes_get, go_string_bytes_len, go_zero_value_for_type,
    },
    GoTemplateScanError, GoTemplateToken, ParseCompatOptions, HELM_INCLUDE_RECURSION_MAX_REFS,
};
use serde_json::{Number, Value};
use std::collections::BTreeMap;
mod compare;
mod call;
mod commandkind;
mod collections;
mod exprkind;
mod externalfn;
mod govaluefmt;
mod path;
mod pipeline_decl;
mod actionparse;
mod rangeeval;
mod textfmt;
mod tokenize;
mod truth;
mod typeutil;
mod trim;
mod varcheck;
use actionparse::parse_action_kind;
use call::eval_call_builtin;
use collections::{builtin_index, builtin_len, builtin_slice};
use compare::{builtin_cmp, builtin_eq, builtin_ne};
use commandkind::{
    command_field_like_path, is_non_executable_pipeline_head, non_function_command_target,
};
use exprkind::{
    decode_string_literal, is_complex_expression, is_niladic_function_expression, is_quoted_string,
};
use externalfn::{try_eval_dynamic_external_function, try_eval_external_function};
use govaluefmt::format_value_like_go;
use path::{
    is_identifier_continue_char, is_identifier_start_char, resolve_simple_path,
};
use pipeline_decl::{extract_pipeline_declaration, PipelineDeclMode, PipelineDeclaration};
use rangeeval::{apply_range_iteration_bindings, range_items};
use textfmt::{builtin_html, builtin_js, builtin_print, builtin_urlquery, format_value_for_print};
use tokenize::{split_command_tokens, split_pipeline_commands, strip_outer_parens};
use truth::{builtin_and, builtin_or, is_truthy};
use trim::apply_lexical_trims;
use varcheck::{
    ensure_variable_is_defined, looks_like_char_literal, looks_like_numeric_literal,
    undefined_variable_error,
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

pub(super) fn is_identifier_name(name: &str) -> bool {
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

fn parse_number_value(expr: &str) -> Option<Value> {
    compat::parse_number_value(expr)
}

fn parse_char_constant(expr: &str) -> Option<i64> {
    compat::parse_char_constant(expr)
}

#[cfg(test)]
mod tests;
