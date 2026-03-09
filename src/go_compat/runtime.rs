pub use super::backend::LogicBackend;
use super::compat;
use super::{
    parse::report::ParseCompatOptions,
    scan::{parse_template_tokens_strict_with_options, GoTemplateScanError, GoTemplateToken},
    HELM_INCLUDE_RECURSION_MAX_REFS,
};
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
// Go parity reference: stdlib text/template/exec.go.
mod actionparse;
mod call;
mod collections;
mod commandkind;
mod compare;
mod control;
mod eval;
mod exprkind;
mod externalfn;
mod go_ffi;
mod govaluefmt;
mod path;
mod pipeline_decl;
mod rangeeval;
mod textfmt;
mod tokenize;
mod trim;
mod truth;
mod typeutil;
mod varcheck;
use crate::go_compat::ident::is_identifier_name as go_is_identifier_name;
use actionparse::parse_action_kind;
use call::eval_call_builtin;
use collections::{builtin_index, builtin_len, builtin_slice};
use commandkind::{
    command_field_like_path, is_map_like_for_field_call, is_non_executable_pipeline_head,
    non_function_command_target,
};
use compare::{builtin_eq, builtin_ge, builtin_gt, builtin_le, builtin_lt, builtin_ne};
use control::{
    eval_block_invocation, eval_if, eval_range, eval_template_invocation, eval_with,
    find_matching_end,
};
use eval::{eval_command_token_value, eval_expr_truthy, eval_expr_value, render_output_expr};
use exprkind::{
    decode_string_literal, is_complex_expression, is_niladic_function_expression, is_quoted_string,
};
use externalfn::{try_eval_dynamic_external_function, try_eval_external_function};
use govaluefmt::format_value_like_go;
use path::resolve_simple_path;
use pipeline_decl::{extract_pipeline_declaration, PipelineDeclMode, PipelineDeclaration};
use rangeeval::{apply_range_iteration_bindings, range_items};
#[cfg(test)]
use textfmt::format_value_for_print;
use textfmt::{builtin_html, builtin_js, builtin_print, builtin_urlquery};
use tokenize::{split_command_tokens, split_pipeline_commands, strip_outer_parens};
use trim::apply_lexical_trims;
use truth::{builtin_and, builtin_or, is_truthy};
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
pub enum FunctionDispatchMode {
    // Go parity mode: no dynamic external function head resolution.
    GoStrict,
    // Happ extension mode: allow dynamic external function head resolution.
    Extended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeRenderOptions {
    pub missing_value_mode: MissingValueMode,
    pub function_dispatch_mode: FunctionDispatchMode,
    pub logic_backend: LogicBackend,
}

impl Default for NativeRenderOptions {
    fn default() -> Self {
        Self {
            missing_value_mode: MissingValueMode::GoDefault,
            function_dispatch_mode: FunctionDispatchMode::Extended,
            logic_backend: LogicBackend::GoFfi,
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

pub trait RuntimeBackendApi {
    fn render(
        &self,
        src: &str,
        root: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError>;
}

struct GoCompatRuntimeBackend;
struct GoFfiRuntimeBackend;
struct DualRuntimeBackend;
struct RustNativeRuntimeBackend;

impl RuntimeBackendApi for GoCompatRuntimeBackend {
    fn render(
        &self,
        src: &str,
        root: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        render_with_go_compat(src, root, options, resolver)
    }
}

impl RuntimeBackendApi for GoFfiRuntimeBackend {
    fn render(
        &self,
        src: &str,
        root: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        render_with_go_ffi(src, root, options, resolver)
    }
}

impl RuntimeBackendApi for DualRuntimeBackend {
    fn render(
        &self,
        src: &str,
        root: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        let primary = dual_primary_backend();
        let secondary = dual_secondary_backend(primary);
        let primary_options = backend_options(options, primary);
        let secondary_options = backend_options(options, secondary);

        let primary_result = render_for_backend(primary, src, root, primary_options, resolver);
        let secondary_result =
            render_for_backend(secondary, src, root, secondary_options, resolver);

        log_dual_mismatch(src, primary, secondary, &primary_result, &secondary_result);
        primary_result
    }
}

impl RuntimeBackendApi for RustNativeRuntimeBackend {
    fn render(
        &self,
        src: &str,
        root: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        // Interface is ready; RustNative currently follows GoCompat execution path.
        render_with_go_compat(src, root, options, resolver)
    }
}

fn runtime_backend(logic_backend: LogicBackend) -> &'static dyn RuntimeBackendApi {
    static GO_COMPAT_BACKEND: GoCompatRuntimeBackend = GoCompatRuntimeBackend;
    static GO_FFI_BACKEND: GoFfiRuntimeBackend = GoFfiRuntimeBackend;
    static DUAL_BACKEND: DualRuntimeBackend = DualRuntimeBackend;
    static RUST_NATIVE_BACKEND: RustNativeRuntimeBackend = RustNativeRuntimeBackend;
    match logic_backend {
        LogicBackend::GoFfi => &GO_FFI_BACKEND,
        LogicBackend::GoCompat => &GO_COMPAT_BACKEND,
        LogicBackend::Dual => &DUAL_BACKEND,
        LogicBackend::RustNative => &RUST_NATIVE_BACKEND,
    }
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
    let mut selected = options;
    if resolver.is_some()
        && matches!(
            selected.logic_backend,
            LogicBackend::GoFfi | LogicBackend::Dual
        )
    {
        // Go FFI helper does not support resolver callbacks yet.
        // Preserve API behavior by routing resolver execution through GoCompat backend.
        selected.logic_backend = LogicBackend::GoCompat;
    }
    runtime_backend(selected.logic_backend).render(src, root, selected, resolver)
}

fn render_with_go_ffi(
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<String, NativeRenderError> {
    if resolver.is_some() {
        return Err(NativeRenderError::UnsupportedAction {
            action: "{{ ... }}".to_string(),
            reason: "go_ffi-only mode: resolver callbacks are not supported".to_string(),
        });
    }
    match go_ffi::render_template_via_go_ffi(src, root, options) {
        Ok(rendered) => Ok(rendered),
        Err(err) => match err {
            go_ffi::GoFfiError::Unavailable(reason) => Err(NativeRenderError::UnsupportedAction {
                action: "{{ ... }}".to_string(),
                reason: format!("go_ffi unavailable: {reason}"),
            }),
            go_ffi::GoFfiError::Parse(reason) => {
                if reason.contains("is not a defined function")
                    || reason.starts_with("illegal number syntax:")
                    || reason.starts_with("invalid syntax")
                {
                    Err(NativeRenderError::UnsupportedAction {
                        action: "{{ ... }}".to_string(),
                        reason,
                    })
                } else {
                    Err(NativeRenderError::Parse(GoTemplateScanError {
                        code: go_ffi::parse_error_code(&reason),
                        message: reason,
                        offset: 0,
                    }))
                }
            }
            go_ffi::GoFfiError::Execute(reason) => {
                if should_retry_with_go_compat_for_execute_error(&reason) {
                    let mut compat_options = options;
                    compat_options.logic_backend = LogicBackend::GoCompat;
                    if let Ok(rendered) = render_with_go_compat(src, root, compat_options, resolver)
                    {
                        return Ok(rendered);
                    }
                }
                if let Some(path) = extract_missing_value_path_from_reason(&reason) {
                    Err(NativeRenderError::MissingValue {
                        action: "{{ ... }}".to_string(),
                        path,
                    })
                } else {
                    Err(NativeRenderError::UnsupportedAction {
                        action: "{{ ... }}".to_string(),
                        reason,
                    })
                }
            }
        },
    }
}

fn should_retry_with_go_compat_for_execute_error(reason: &str) -> bool {
    // Some Go releases used by CI/packaging runners do not support integer
    // `range` in text/template yet. GoCompat does, so we retry there.
    reason.starts_with("range can't iterate over ")
}

fn extract_missing_value_path_from_reason(reason: &str) -> Option<String> {
    // Keep MissingValue classification narrowly scoped to missingkey=error style
    // map lookups. Errors like "nil pointer evaluating ..." must stay in
    // UnsupportedAction to preserve Go parity matrix classes.
    if !reason.contains("map has no entry for key") {
        return None;
    }
    let start = reason.find('<')?;
    let end_rel = reason[start + 1..].find('>')?;
    let end = start + 1 + end_rel;
    let raw = reason[start + 1..end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn render_for_backend(
    backend: LogicBackend,
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
    resolver: Option<&dyn NativeFunctionResolver>,
) -> Result<String, NativeRenderError> {
    match backend {
        LogicBackend::GoFfi => render_with_go_ffi(src, root, options, resolver),
        LogicBackend::GoCompat | LogicBackend::RustNative | LogicBackend::Dual => {
            render_with_go_compat(src, root, options, resolver)
        }
    }
}

fn backend_options(mut options: NativeRenderOptions, backend: LogicBackend) -> NativeRenderOptions {
    options.logic_backend = backend;
    options
}

fn dual_primary_backend() -> LogicBackend {
    env::var("HAPP_TEMPLATE_DUAL_PRIMARY")
        .ok()
        .and_then(|raw| LogicBackend::parse(&raw))
        .filter(|backend| !matches!(backend, LogicBackend::Dual))
        .unwrap_or(LogicBackend::GoFfi)
}

fn dual_secondary_backend(primary: LogicBackend) -> LogicBackend {
    match primary {
        LogicBackend::GoFfi => LogicBackend::GoCompat,
        LogicBackend::GoCompat | LogicBackend::RustNative | LogicBackend::Dual => {
            LogicBackend::GoFfi
        }
    }
}

fn log_dual_mismatch(
    src: &str,
    primary: LogicBackend,
    secondary: LogicBackend,
    primary_result: &Result<String, NativeRenderError>,
    secondary_result: &Result<String, NativeRenderError>,
) {
    if results_semantically_equal(primary_result, secondary_result) {
        return;
    }
    let line = format!(
        "go-template dual mismatch: primary={primary:?} secondary={secondary:?} template={} primary_result={} secondary_result={}",
        truncate_preview(src, 220),
        result_preview(primary_result),
        result_preview(secondary_result)
    );
    emit_dual_log(&line);
}

fn results_semantically_equal(
    left: &Result<String, NativeRenderError>,
    right: &Result<String, NativeRenderError>,
) -> bool {
    match (left, right) {
        (Ok(a), Ok(b)) => a == b,
        (Err(a), Err(b)) => errors_semantically_equal(a, b),
        _ => false,
    }
}

fn errors_semantically_equal(left: &NativeRenderError, right: &NativeRenderError) -> bool {
    match (left, right) {
        (NativeRenderError::Parse(a), NativeRenderError::Parse(b)) => {
            a.code == b.code && a.message == b.message
        }
        (
            NativeRenderError::UnsupportedAction { reason: a, .. },
            NativeRenderError::UnsupportedAction { reason: b, .. },
        ) => a == b,
        (
            NativeRenderError::MissingValue { path: a, .. },
            NativeRenderError::MissingValue { path: b, .. },
        ) => a == b,
        (
            NativeRenderError::TemplateNotFound { name: a },
            NativeRenderError::TemplateNotFound { name: b },
        ) => a == b,
        (
            NativeRenderError::TemplateRecursionLimit {
                name: an,
                depth: ad,
            },
            NativeRenderError::TemplateRecursionLimit {
                name: bn,
                depth: bd,
            },
        ) => an == bn && ad == bd,
        _ => false,
    }
}

fn emit_dual_log(line: &str) {
    if let Ok(path) = env::var("HAPP_TEMPLATE_DUAL_LOG") {
        let path = path.trim();
        if !path.is_empty() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{line}");
                return;
            }
        }
    }
    eprintln!("{line}");
}

fn result_preview(result: &Result<String, NativeRenderError>) -> String {
    match result {
        Ok(value) => format!("ok({})", truncate_preview(value, 180)),
        Err(NativeRenderError::Parse(err)) => format!(
            "parse(code={}, offset={}, msg={})",
            err.code,
            err.offset,
            truncate_preview(&err.message, 160)
        ),
        Err(NativeRenderError::UnsupportedAction { reason, .. }) => {
            format!("unsupported({})", truncate_preview(reason, 160))
        }
        Err(NativeRenderError::MissingValue { path, .. }) => {
            format!("missing(path={})", truncate_preview(path, 160))
        }
        Err(NativeRenderError::TemplateNotFound { name }) => {
            format!("template_not_found({})", truncate_preview(name, 160))
        }
        Err(NativeRenderError::TemplateRecursionLimit { name, depth }) => {
            format!("template_recursion_limit(name={name},depth={depth})")
        }
    }
}

fn truncate_preview(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, ch) in input.chars().enumerate() {
        if count >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn render_with_go_compat(
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
    let mut state = EvalState::new(
        options.missing_value_mode,
        options.function_dispatch_mode,
        options.logic_backend,
    );
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
                message: "unexpected control terminator at top level".to_string(),
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
    function_dispatch_mode: FunctionDispatchMode,
    logic_backend: LogicBackend,
}

impl EvalState {
    fn new(
        missing_value_mode: MissingValueMode,
        function_dispatch_mode: FunctionDispatchMode,
        logic_backend: LogicBackend,
    ) -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            missing_value_mode,
            function_dispatch_mode,
            logic_backend,
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
                            message: "unexpected control action".to_string(),
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

pub(super) fn is_identifier_name(name: &str) -> bool {
    go_is_identifier_name(name)
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
        "lt" => Some(Value::Bool(builtin_lt(action, args)?)),
        "le" => Some(Value::Bool(builtin_le(action, args)?)),
        "gt" => Some(Value::Bool(builtin_gt(action, args)?)),
        "ge" => Some(Value::Bool(builtin_ge(action, args)?)),
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
