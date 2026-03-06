use super::{GoTemplateActionSpan, GoTemplateScanError, GoTemplateToken};
use crate::gotemplates::parser::{
    parse_action_report_with_options, ActionParseReport, ControlAction, ControlKind,
    ParseCompatOptions,
};
use std::collections::BTreeMap;

const LEFT_DELIM: &str = "{{";
const RIGHT_DELIM: &str = "}}";
const LEFT_COMMENT: &str = "/*";
const RIGHT_COMMENT: &str = "*/";

pub fn contains_template_markup(s: &str) -> bool {
    contains_template_markup_with_delims(s, LEFT_DELIM, RIGHT_DELIM)
}

pub fn collect_action_spans(src: &str) -> Vec<GoTemplateActionSpan> {
    let (spans, _) = scan_template_actions_with_delims(src, LEFT_DELIM, RIGHT_DELIM);
    spans
}

pub fn scan_template_actions(src: &str) -> (Vec<GoTemplateActionSpan>, Vec<GoTemplateScanError>) {
    scan_template_actions_with_delims(src, LEFT_DELIM, RIGHT_DELIM)
}

pub fn scan_template_actions_with_delims(
    src: &str,
    left_delim: &str,
    right_delim: &str,
) -> (Vec<GoTemplateActionSpan>, Vec<GoTemplateScanError>) {
    if left_delim.is_empty() || right_delim.is_empty() {
        return (
            Vec::new(),
            vec![GoTemplateScanError {
                code: "invalid_delimiters",
                message: "template delimiters must be non-empty",
                offset: 0,
            }],
        );
    }

    let mut spans = Vec::new();
    let mut errors = Vec::new();
    let mut cursor = 0usize;

    while cursor < src.len() {
        let Some(open_rel) = src[cursor..].find(left_delim) else {
            break;
        };
        let open = cursor + open_rel;
        let action_start = open + left_delim.len();

        match scan_action_end_with_delims(src, action_start, left_delim, right_delim) {
            Ok(end) => {
                spans.push(GoTemplateActionSpan { start: open, end });
                cursor = end;
            }
            Err(err) => {
                errors.push(err);
                break;
            }
        }
    }

    (spans, errors)
}

pub fn parse_template_tokens(src: &str) -> Option<Vec<GoTemplateToken>> {
    parse_template_tokens_strict(src).ok()
}

pub fn parse_template_tokens_strict(
    src: &str,
) -> Result<Vec<GoTemplateToken>, GoTemplateScanError> {
    parse_template_tokens_strict_with_options(src, ParseCompatOptions::default())
}

pub fn parse_template_tokens_strict_with_options(
    src: &str,
    options: ParseCompatOptions<'_>,
) -> Result<Vec<GoTemplateToken>, GoTemplateScanError> {
    parse_template_tokens_strict_with_options_and_delims(src, LEFT_DELIM, RIGHT_DELIM, options)
}

pub fn parse_template_tokens_strict_with_options_and_delims(
    src: &str,
    left_delim: &str,
    right_delim: &str,
    options: ParseCompatOptions<'_>,
) -> Result<Vec<GoTemplateToken>, GoTemplateScanError> {
    if left_delim.is_empty() || right_delim.is_empty() {
        return Err(GoTemplateScanError {
            code: "invalid_delimiters",
            message: "template delimiters must be non-empty",
            offset: 0,
        });
    }
    let (spans, errors) = scan_template_actions_with_delims(src, left_delim, right_delim);
    if let Some(err) = errors.first().copied() {
        return Err(err);
    }
    let action_offset_delta = left_delim.len() as isize - LEFT_DELIM.len() as isize;
    let mut stack: Vec<SimpleFrame> = Vec::new();
    let mut range_depth = 0usize;
    let mut var_scopes: Vec<Vec<String>> = vec![vec!["$".to_string()]];
    let mut define_states: BTreeMap<String, bool> = BTreeMap::new();
    let mut out = Vec::with_capacity(spans.len().saturating_mul(2).saturating_add(1));
    let mut cursor = 0usize;

    for span in &spans {
        let literal = &src[cursor..span.start];
        mark_define_body_from_literal(&mut stack, literal);

        let action = &src[span.start..span.end];
        let normalized_action = normalize_action_delimiters(action, left_delim, right_delim)
            .ok_or(GoTemplateScanError {
                code: "invalid_delimiters",
                message: "template action uses invalid delimiters",
                offset: span.start,
            })?;
        let visible_variables = collect_visible_variables(&var_scopes);
        let mut parse_options = options;
        parse_options.visible_variables = &visible_variables;
        let mut report =
            parse_action_report_with_options(&normalized_action, span.start, parse_options)
                .map_err(|err| shift_scan_error_offset(err, action_offset_delta))?;
        shift_action_report_offsets(&mut report, action_offset_delta);
        mark_define_body_from_action(&mut stack, &report, &normalized_action);
        validate_variable_references(&var_scopes, &report, options.check_variables)?;
        apply_control_action(
            &mut stack,
            &mut range_depth,
            &mut var_scopes,
            &mut define_states,
            report.control,
            report.define_name.clone(),
            span.start,
            options.check_variables,
        )?;
        apply_declared_variables(&mut var_scopes, &report, options.check_variables);

        out.push(GoTemplateToken::Literal(literal.to_string()));
        out.push(GoTemplateToken::Action(normalized_action));
        cursor = span.end;
    }

    if !stack.is_empty() {
        return Err(GoTemplateScanError {
            code: "unexpected_eof",
            message: "unexpected EOF",
            offset: src.len(),
        });
    }

    out.push(GoTemplateToken::Literal(src[cursor..].to_string()));
    Ok(out)
}

fn apply_control_action(
    stack: &mut Vec<SimpleFrame>,
    range_depth: &mut usize,
    var_scopes: &mut Vec<Vec<String>>,
    define_states: &mut BTreeMap<String, bool>,
    action: ControlAction,
    define_name: Option<String>,
    offset: usize,
    check_variables: bool,
) -> Result<(), GoTemplateScanError> {
    match action {
        ControlAction::None => {}
        ControlAction::Open(kind) => {
            let contributes_range = matches!(kind, ControlKind::Range);
            if contributes_range {
                *range_depth = range_depth.saturating_add(1);
            }
            let mut scope_depth_before = 0usize;
            if check_variables {
                scope_depth_before = var_scopes.len();
                var_scopes.push(Vec::new());
            }
            stack.push(SimpleFrame {
                kind,
                else_state: ElseState::None,
                contributes_range,
                scope_depth_before,
                has_variable_scope: check_variables,
                define_name: if matches!(kind, ControlKind::Define) {
                    define_name
                } else {
                    None
                },
                define_body_has_content: false,
                open_offset: offset,
            });
        }
        ControlAction::Else(nested) => {
            let Some(top) = stack.last_mut() else {
                return Err(GoTemplateScanError {
                    code: "unexpected_else_action",
                    message: "unexpected {{else}}",
                    offset,
                });
            };
            if !matches!(
                top.kind,
                ControlKind::If | ControlKind::Range | ControlKind::With
            ) || matches!(top.else_state, ElseState::Terminal)
            {
                return Err(GoTemplateScanError {
                    code: "unexpected_else_action",
                    message: "unexpected {{else}}",
                    offset,
                });
            }
            if top.contributes_range {
                *range_depth = range_depth.saturating_sub(1);
                top.contributes_range = false;
            }
            if let Some(kind) = nested {
                let allowed = matches!(
                    (top.kind, kind),
                    (ControlKind::If, ControlKind::If) | (ControlKind::With, ControlKind::With)
                );
                if !allowed {
                    return Err(GoTemplateScanError {
                        code: "unexpected_token",
                        message: "unexpected token in input",
                        offset,
                    });
                }
                top.else_state = ElseState::Chain;
            } else {
                top.else_state = ElseState::Terminal;
            }
        }
        ControlAction::Break => {
            if *range_depth == 0 {
                return Err(GoTemplateScanError {
                    code: "break_outside_range",
                    message: "{{break}} outside {{range}}",
                    offset,
                });
            }
        }
        ControlAction::Continue => {
            if *range_depth == 0 {
                return Err(GoTemplateScanError {
                    code: "continue_outside_range",
                    message: "{{continue}} outside {{range}}",
                    offset,
                });
            }
        }
        ControlAction::End => {
            let Some(frame) = stack.pop() else {
                return Err(GoTemplateScanError {
                    code: "unexpected_end_action",
                    message: "unexpected {{end}}",
                    offset,
                });
            };
            if frame.contributes_range {
                *range_depth = range_depth.saturating_sub(1);
            }
            if frame.has_variable_scope {
                while var_scopes.len() > frame.scope_depth_before {
                    let _ = var_scopes.pop();
                }
            }
            if matches!(frame.kind, ControlKind::Define) {
                if let Some(name) = frame.define_name.as_deref() {
                    let prev_non_empty = define_states.get(name).copied().unwrap_or(false);
                    if prev_non_empty && frame.define_body_has_content {
                        return Err(GoTemplateScanError {
                            code: "multiple_template_definition",
                            message: "multiple definition of template",
                            offset: frame.open_offset,
                        });
                    }
                    let now_non_empty = prev_non_empty || frame.define_body_has_content;
                    define_states.insert(name.to_string(), now_non_empty);
                }
            }
        }
    }
    Ok(())
}

fn mark_define_body_from_literal(stack: &mut [SimpleFrame], literal: &str) {
    let Some(top) = stack.last_mut() else {
        return;
    };
    if !matches!(top.kind, ControlKind::Define) {
        return;
    }
    if literal.chars().any(|c| !c.is_whitespace()) {
        top.define_body_has_content = true;
    }
}

fn mark_define_body_from_action(
    stack: &mut [SimpleFrame],
    report: &ActionParseReport,
    action: &str,
) {
    let Some(top) = stack.last_mut() else {
        return;
    };
    if !matches!(top.kind, ControlKind::Define) {
        return;
    }
    if is_comment_action(action) {
        return;
    }
    if matches!(report.control, ControlAction::End | ControlAction::Else(_)) {
        return;
    }
    if matches!(report.control, ControlAction::Open(ControlKind::Define)) {
        return;
    }
    top.define_body_has_content = true;
}

fn validate_variable_references(
    var_scopes: &[Vec<String>],
    report: &ActionParseReport,
    check_variables: bool,
) -> Result<(), GoTemplateScanError> {
    if !check_variables {
        return Ok(());
    }
    for v in &report.referenced_vars {
        if variable_exists(var_scopes, v.name.as_str()) {
            continue;
        }
        if report
            .declared_vars
            .iter()
            .any(|decl| decl.name.as_str() == v.name.as_str())
        {
            continue;
        }
        return Err(GoTemplateScanError {
            code: "undefined_variable",
            message: format!("undefined variable \"{}\"", v.name),
            offset: v.offset,
        });
    }
    Ok(())
}

fn apply_declared_variables(
    var_scopes: &mut [Vec<String>],
    report: &ActionParseReport,
    check_variables: bool,
) {
    if !check_variables || report.declared_vars.is_empty() || var_scopes.is_empty() {
        return;
    }
    let target_idx = var_scopes.len().saturating_sub(1);
    for decl in &report.declared_vars {
        if !var_scopes[target_idx].iter().any(|v| v == &decl.name) {
            var_scopes[target_idx].push(decl.name.clone());
        }
    }
}

fn variable_exists(var_scopes: &[Vec<String>], name: &str) -> bool {
    for scope in var_scopes.iter().rev() {
        if scope.iter().any(|v| v == name) {
            return true;
        }
    }
    false
}

fn collect_visible_variables(var_scopes: &[Vec<String>]) -> Vec<&str> {
    let mut out = Vec::new();
    for scope in var_scopes {
        for v in scope {
            if out.iter().any(|seen| seen == &v.as_str()) {
                continue;
            }
            out.push(v.as_str());
        }
    }
    out
}

#[derive(Debug, Clone)]
struct SimpleFrame {
    kind: ControlKind,
    else_state: ElseState,
    contributes_range: bool,
    scope_depth_before: usize,
    has_variable_scope: bool,
    define_name: Option<String>,
    define_body_has_content: bool,
    open_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElseState {
    None,
    Chain,
    Terminal,
}

fn is_comment_action(action: &str) -> bool {
    if !(action.starts_with("{{") && action.ends_with("}}")) {
        return false;
    }
    let mut inner = &action[2..action.len() - 2];
    if inner.len() >= 2 && inner.as_bytes()[0] == b'-' && inner.as_bytes()[1].is_ascii_whitespace()
    {
        inner = &inner[1..];
    }
    let inner = inner.trim();
    inner.starts_with("/*")
}

fn scan_action_end_with_delims(
    src: &str,
    action_start: usize,
    left_delim: &str,
    right_delim: &str,
) -> Result<usize, GoTemplateScanError> {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
    }

    let bytes = src.as_bytes();
    let right = right_delim.as_bytes();
    let mut i = action_start;
    if has_left_trim_marker(bytes, i) {
        i += 2;
    }
    if starts_with(bytes, i, LEFT_COMMENT.as_bytes()) {
        return scan_comment_action_end_with_delims(bytes, i, right);
    }

    let mut paren_depth: i32 = 0;
    let mut state = State::Normal;

    while i < bytes.len() {
        match state {
            State::Normal => {
                if paren_depth == 0 {
                    if let Some(end) = at_right_delim_end_with_delims(bytes, i, right) {
                        return Ok(end);
                    }
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
                        if paren_depth == 0 {
                            return Err(GoTemplateScanError {
                                code: "unexpected_right_paren",
                                message: "unexpected right paren",
                                offset: i,
                            });
                        }
                        paren_depth -= 1;
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
                if bytes[i] == b'\n' {
                    return Err(GoTemplateScanError {
                        code: "unterminated_character_constant",
                        message: "unterminated character constant",
                        offset: i,
                    });
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
                if bytes[i] == b'\n' {
                    return Err(GoTemplateScanError {
                        code: "unterminated_quoted_string",
                        message: "unterminated quoted string",
                        offset: i,
                    });
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
        }
    }

    let (code, message) = match state {
        State::SingleQuote => (
            "unterminated_character_constant",
            "unterminated character constant",
        ),
        State::DoubleQuote => ("unterminated_quoted_string", "unterminated quoted string"),
        State::RawQuote => (
            "unterminated_raw_quoted_string",
            "unterminated raw quoted string",
        ),
        State::Normal if paren_depth > 0 => ("unclosed_left_paren", "unclosed left paren"),
        State::Normal => (
            "unterminated_action",
            "template action is missing closing '}}'",
        ),
    };

    Err(GoTemplateScanError {
        code,
        message,
        offset: action_start.saturating_sub(left_delim.len()),
    })
}

fn scan_comment_action_end_with_delims(
    bytes: &[u8],
    comment_start: usize,
    right_delim: &[u8],
) -> Result<usize, GoTemplateScanError> {
    let mut i = comment_start + LEFT_COMMENT.len();
    while i < bytes.len() {
        if starts_with(bytes, i, RIGHT_COMMENT.as_bytes()) {
            let after_comment = i + RIGHT_COMMENT.len();
            if let Some(end) = at_right_delim_end_with_delims(bytes, after_comment, right_delim) {
                return Ok(end);
            }
            return Err(GoTemplateScanError {
                code: "comment_ends_before_closing_delimiter",
                message: "comment ends before closing delimiter",
                offset: after_comment,
            });
        }
        i += 1;
    }
    Err(GoTemplateScanError {
        code: "unclosed_comment",
        message: "unclosed comment",
        offset: comment_start,
    })
}

fn starts_with(haystack: &[u8], offset: usize, needle: &[u8]) -> bool {
    haystack
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|chunk| chunk == needle)
}

fn starts_with_right_trim_delim_with_delims(
    bytes: &[u8],
    offset: usize,
    right_delim: &[u8],
) -> bool {
    let need = 2 + right_delim.len();
    bytes.get(offset..offset + need).is_some_and(|chunk| {
        chunk[0].is_ascii_whitespace() && chunk[1] == b'-' && &chunk[2..] == right_delim
    })
}

fn at_right_delim_end_with_delims(
    bytes: &[u8],
    offset: usize,
    right_delim: &[u8],
) -> Option<usize> {
    if starts_with_right_trim_delim_with_delims(bytes, offset, right_delim) {
        return Some(offset + 2 + right_delim.len());
    }
    if starts_with(bytes, offset, right_delim) {
        return Some(offset + right_delim.len());
    }
    None
}

fn has_left_trim_marker(bytes: &[u8], offset: usize) -> bool {
    bytes
        .get(offset..offset + 2)
        .is_some_and(|chunk| chunk[0] == b'-' && chunk[1].is_ascii_whitespace())
}

fn contains_template_markup_with_delims(s: &str, left_delim: &str, right_delim: &str) -> bool {
    !left_delim.is_empty()
        && !right_delim.is_empty()
        && s.contains(left_delim)
        && s.contains(right_delim)
}

fn normalize_action_delimiters(
    action: &str,
    left_delim: &str,
    right_delim: &str,
) -> Option<String> {
    if !action.starts_with(left_delim) || !action.ends_with(right_delim) {
        return None;
    }
    if action.len() < left_delim.len() + right_delim.len() {
        return None;
    }
    let inner = &action[left_delim.len()..action.len() - right_delim.len()];
    let mut out = String::with_capacity(LEFT_DELIM.len() + inner.len() + RIGHT_DELIM.len());
    out.push_str(LEFT_DELIM);
    out.push_str(inner);
    out.push_str(RIGHT_DELIM);
    Some(out)
}

fn shift_action_report_offsets(report: &mut ActionParseReport, delta: isize) {
    for item in &mut report.declared_vars {
        item.offset = shift_offset(item.offset, delta);
    }
    for item in &mut report.assigned_vars {
        item.offset = shift_offset(item.offset, delta);
    }
    for item in &mut report.referenced_vars {
        item.offset = shift_offset(item.offset, delta);
    }
}

fn shift_scan_error_offset(mut err: GoTemplateScanError, delta: isize) -> GoTemplateScanError {
    err.offset = shift_offset(err.offset, delta);
    err
}

fn shift_offset(offset: usize, delta: isize) -> usize {
    if delta >= 0 {
        offset.saturating_add(delta as usize)
    } else {
        offset.saturating_sub((-delta) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_template_markup_requires_both_delimiters() {
        assert!(contains_template_markup("a {{ .Values.x }} b"));
        assert!(!contains_template_markup("a {{ .Values.x b"));
        assert!(!contains_template_markup("a .Values.x }} b"));
    }

    #[test]
    fn parse_template_tokens_returns_none_for_unbalanced_action() {
        assert!(parse_template_tokens("x {{ include \"a\" . ").is_none());
    }

    #[test]
    fn parse_template_tokens_splits_literals_and_actions() {
        let tokens = parse_template_tokens("A{{ .Values.a }}B{{ include \"x\" . }}C")
            .expect("balanced tokens");
        assert_eq!(
            tokens,
            vec![
                GoTemplateToken::Literal("A".to_string()),
                GoTemplateToken::Action("{{ .Values.a }}".to_string()),
                GoTemplateToken::Literal("B".to_string()),
                GoTemplateToken::Action("{{ include \"x\" . }}".to_string()),
                GoTemplateToken::Literal("C".to_string()),
            ]
        );
    }

    #[test]
    fn parse_template_tokens_supports_comment_actions_like_go() {
        let tokens = parse_template_tokens("hello-{{/* this is a comment */}}-world")
            .expect("balanced with comment");
        assert_eq!(
            tokens,
            vec![
                GoTemplateToken::Literal("hello-".to_string()),
                GoTemplateToken::Action("{{/* this is a comment */}}".to_string()),
                GoTemplateToken::Literal("-world".to_string())
            ]
        );
    }

    #[test]
    fn parse_template_tokens_keeps_trim_markers() {
        let tokens = parse_template_tokens("hello- {{- 3 -}} -world").expect("balanced with trim");
        assert_eq!(
            tokens,
            vec![
                GoTemplateToken::Literal("hello- ".to_string()),
                GoTemplateToken::Action("{{- 3 -}}".to_string()),
                GoTemplateToken::Literal(" -world".to_string())
            ]
        );
    }

    #[test]
    fn parse_template_tokens_with_custom_delims_normalizes_to_default_actions() {
        let tokens = parse_template_tokens_strict_with_options_and_delims(
            "A<< .Values.a >>B",
            "<<",
            ">>",
            ParseCompatOptions::default(),
        )
        .expect("must parse");
        assert_eq!(
            tokens,
            vec![
                GoTemplateToken::Literal("A".to_string()),
                GoTemplateToken::Action("{{ .Values.a }}".to_string()),
                GoTemplateToken::Literal("B".to_string()),
            ]
        );
    }

    #[test]
    fn custom_delims_keep_original_error_offsets() {
        let err = parse_template_tokens_strict_with_options_and_delims(
            "AA[[[$x]]]",
            "[[[",
            "]]]",
            ParseCompatOptions {
                skip_func_check: true,
                known_functions: &[],
                check_variables: true,
                visible_variables: &[],
            },
        )
        .expect_err("must fail");
        assert_eq!(err.code, "undefined_variable");
        assert_eq!(err.offset, 5);
        assert!(err.message.contains("undefined variable \"$x\""));
    }

    #[test]
    fn scan_template_actions_follows_go_lexer_for_inner_left_delim() {
        let src = "{{ include \"a\" {{ .Values.x }} }}";
        let (spans, errors) = scan_template_actions(src);
        assert_eq!(spans.len(), 1);
        assert!(src[spans[0].start..spans[0].end].contains("include \"a\""));
        assert!(src[spans[0].start..spans[0].end].contains(".Values.x"));
        assert!(errors.is_empty());
    }

    #[test]
    fn scan_template_actions_reports_comment_without_closing_delimiter() {
        let src = "{{/* comment */ x }}";
        let (_, errors) = scan_template_actions(src);
        assert!(errors
            .iter()
            .any(|e| e.code == "comment_ends_before_closing_delimiter"));
    }

    #[test]
    fn scan_template_actions_reports_unterminated_quote() {
        let src = "{{ include \"a }}";
        let (_, errors) = scan_template_actions(src);
        assert!(errors
            .iter()
            .any(|e| e.code == "unterminated_quoted_string"));
    }

    #[test]
    fn parse_template_tokens_strict_reports_unexpected_left_delim_like_go_parser() {
        let src = "{{ include \"a\" {{ .Values.x }} }}";
        let err = parse_template_tokens_strict(src).expect_err("must fail");
        assert_eq!(err.code, "unexpected_left_delim_in_operand");
    }

    #[test]
    fn parse_template_tokens_strict_reports_unexpected_dot_like_go_parser() {
        let src = "{{ .Values.bad..path }}";
        let err = parse_template_tokens_strict(src).expect_err("must fail");
        assert_eq!(err.code, "unexpected_dot_in_operand");
    }

    #[test]
    fn parse_template_tokens_strict_rejects_break_outside_range() {
        let err = parse_template_tokens_strict("{{ break }}").expect_err("must fail");
        assert_eq!(err.code, "break_outside_range");
    }

    #[test]
    fn parse_template_tokens_strict_rejects_continue_outside_range() {
        let err = parse_template_tokens_strict("{{ continue }}").expect_err("must fail");
        assert_eq!(err.code, "continue_outside_range");
    }

    #[test]
    fn parse_template_tokens_strict_accepts_break_in_range_body() {
        parse_template_tokens_strict("{{ range .Items }}{{ break }}{{ end }}").expect("must parse");
    }

    #[test]
    fn parse_template_tokens_strict_rejects_break_in_range_else() {
        let src = "{{ range .Items }}x{{ else }}{{ break }}{{ end }}";
        let err = parse_template_tokens_strict(src).expect_err("must fail");
        assert_eq!(err.code, "break_outside_range");
    }

    #[test]
    fn parse_template_tokens_strict_supports_skip_func_check_mode() {
        let src = "{{ totallyUnknownFn . }}";
        let ok = parse_template_tokens_strict_with_options(
            src,
            ParseCompatOptions {
                skip_func_check: true,
                known_functions: &[],
                check_variables: false,
                visible_variables: &[],
            },
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn parse_template_tokens_strict_reports_undefined_variable_in_checked_mode() {
        let src = "{{ $x }}";
        let err = parse_template_tokens_strict_with_options(
            src,
            ParseCompatOptions {
                skip_func_check: true,
                known_functions: &[],
                check_variables: true,
                visible_variables: &[],
            },
        )
        .expect_err("must fail");
        assert_eq!(err.code, "undefined_variable");
        assert!(err.message.contains("undefined variable \"$x\""));
    }

    #[test]
    fn parse_template_tokens_strict_variable_scope_ends_on_end_action() {
        let src = "{{ with $x := 4 }}{{ end }}{{ $x }}";
        let err = parse_template_tokens_strict_with_options(
            src,
            ParseCompatOptions {
                skip_func_check: true,
                known_functions: &[],
                check_variables: true,
                visible_variables: &[],
            },
        )
        .expect_err("must fail");
        assert_eq!(err.code, "undefined_variable");
        assert!(err.message.contains("undefined variable \"$x\""));
    }

    #[test]
    fn parse_template_tokens_strict_variable_declared_in_if_is_visible_in_else_like_go_parse() {
        let src = "{{ if .X }}{{ $x := 1 }}{{ else }}{{ $x }}{{ end }}";
        parse_template_tokens_strict_with_options(
            src,
            ParseCompatOptions {
                skip_func_check: true,
                known_functions: &[],
                check_variables: true,
                visible_variables: &[],
            },
        )
        .expect("must parse");
    }

    #[test]
    fn parse_template_tokens_strict_rejects_duplicate_nonempty_define() {
        let src = "{{define \"a\"}}a{{end}}{{define \"a\"}}b{{end}}";
        let err = parse_template_tokens_strict(src).expect_err("must fail");
        assert_eq!(err.code, "multiple_template_definition");
    }

    #[test]
    fn parse_template_tokens_strict_allows_duplicate_when_one_define_is_empty() {
        parse_template_tokens_strict("{{define \"a\"}}{{end}}{{define \"a\"}}b{{end}}")
            .expect("must parse");
        parse_template_tokens_strict("{{define \"a\"}}a{{end}}{{define \"a\"}}{{end}}")
            .expect("must parse");
    }
}
