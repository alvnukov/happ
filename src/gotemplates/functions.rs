use super::{collect_action_spans, compat, utf8scan::push_utf8_char_from_bytes};

const LEFT_DELIM: &str = "{{";
const RIGHT_DELIM: &str = "}}";
const LEFT_COMMENT: &str = "/*";
const RIGHT_COMMENT: &str = "*/";

pub fn normalize_values_global_context(action: &str) -> String {
    const TOKEN: &str = ".Values";
    if !action.contains(TOKEN) {
        return action.to_string();
    }
    let token_len = TOKEN.len();
    let bytes = action.as_bytes();
    let mut out = String::with_capacity(action.len() + 4);
    let mut last = 0usize;
    let mut changed = false;
    for (idx, _) in action.match_indices(TOKEN) {
        let prev_is_dollar = idx > 0 && bytes[idx - 1] == b'$';
        if prev_is_dollar {
            continue;
        }
        out.push_str(&action[last..idx]);
        out.push_str("$.Values");
        last = idx + token_len;
        changed = true;
    }
    if !changed {
        return action.to_string();
    }
    out.push_str(&action[last..]);
    out
}

pub fn escape_template_action(action: &str) -> String {
    if !(action.starts_with(LEFT_DELIM) && action.ends_with(RIGHT_DELIM)) || action.len() < 4 {
        return action.to_string();
    }
    let inner = &action[LEFT_DELIM.len()..action.len() - RIGHT_DELIM.len()];
    format!("{{{{ \"{{{{\" }}}}{inner}{{{{ \"}}}}\" }}}}")
}

pub fn collect_function_calls_in_action(action: &str) -> Vec<String> {
    let spans = collect_action_spans(action);
    if spans.len() != 1 || spans[0].start != 0 || spans[0].end != action.len() {
        return collect_function_calls_in_template(action);
    }

    let mut out = Vec::new();
    let Some(inner) = action_inner(action) else {
        return out;
    };

    collect_function_calls_in_single_action_inner(inner, &mut out);
    normalize_name_list(out)
}

pub fn collect_function_calls_in_template(src: &str) -> Vec<String> {
    let mut out = Vec::new();

    for span in collect_action_spans(src) {
        let action = &src[span.start..span.end];
        let Some(inner) = action_inner(action) else {
            continue;
        };
        collect_function_calls_in_single_action_inner(inner, &mut out);
    }
    normalize_name_list(out)
}

fn collect_function_calls_in_single_action_inner(inner: &str, out: &mut Vec<String>) {
    for command in split_pipeline_commands(inner) {
        if let Some(name) = command_function_name(command) {
            out.push(name);
        }
    }
}

fn normalize_name_list(mut out: Vec<String>) -> Vec<String> {
    out.sort_unstable();
    out.dedup();
    out
}

fn action_inner(action: &str) -> Option<&str> {
    let inner = action
        .strip_prefix(LEFT_DELIM)
        .and_then(|s| s.strip_suffix(RIGHT_DELIM))?;
    Some(strip_trim_markers(inner))
}

fn strip_trim_markers(inner: &str) -> &str {
    let mut start = 0usize;
    let mut end = inner.len();
    let bytes = inner.as_bytes();

    if bytes.len() >= 2 && bytes[0] == b'-' && is_space(bytes[1]) {
        start = 1;
    }

    while start < end && is_space(bytes[start]) {
        start += 1;
    }

    while start < end && is_space(bytes[end - 1]) {
        end -= 1;
    }
    if end > start && bytes[end - 1] == b'-' {
        end -= 1;
        while start < end && is_space(bytes[end - 1]) {
            end -= 1;
        }
    }

    &inner[start..end]
}

fn split_pipeline_commands(inner: &str) -> Vec<&str> {
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
                if starts_with(bytes, i, LEFT_COMMENT.as_bytes()) {
                    state = State::Comment;
                    i += LEFT_COMMENT.len();
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
                            out.push(cmd);
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
                if starts_with(bytes, i, RIGHT_COMMENT.as_bytes()) {
                    state = State::Normal;
                    i += RIGHT_COMMENT.len();
                    continue;
                }
                i += 1;
            }
        }
    }

    if start <= inner.len() {
        let cmd = inner[start..].trim();
        if !cmd.is_empty() {
            out.push(cmd);
        }
    }

    out
}

fn command_function_name(command: &str) -> Option<String> {
    if command.is_empty() {
        return None;
    }
    let tokens = split_command_tokens(command);
    if tokens.is_empty() {
        return None;
    }

    let mut start = 0usize;
    if tokens.len() >= 2 && tokens[0].starts_with('$') && tokens[1] == ":=" {
        start = 2;
    }
    if start >= tokens.len() {
        return None;
    }

    let mut candidate = String::new();
    for token in tokens.iter().skip(start) {
        let normalized = token.trim_start_matches('(');
        if normalized.is_empty() {
            continue;
        }
        candidate = normalized.trim_end_matches(')').to_string();
        break;
    }
    if candidate.is_empty() {
        return None;
    }
    if is_non_function_token(&candidate)
        || is_go_template_keyword(&candidate)
        || !is_identifier_name(&candidate)
    {
        return None;
    }
    Some(candidate)
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

    while i < bytes.len() {
        match state {
            State::Normal => {
                if bytes[i].is_ascii_whitespace() {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
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

fn is_non_function_token(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with('.') || t.starts_with('$') {
        return true;
    }
    if t.starts_with('"') || t.starts_with('\'') || t.starts_with('`') {
        return true;
    }
    if matches!(t, "true" | "false" | "nil") {
        return true;
    }
    if t.chars().next().is_some_and(|c| c.is_numeric()) {
        return true;
    }
    if compat::looks_like_numeric_literal(t) || compat::looks_like_char_literal(t) {
        return true;
    }
    false
}

fn is_go_template_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "else" | "end" | "range" | "with" | "define" | "block" | "template"
    )
}

fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

fn starts_with(haystack: &[u8], offset: usize, needle: &[u8]) -> bool {
    haystack
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|chunk| chunk == needle)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_values_global_context_keeps_root_and_rewrites_dot_values() {
        let src = "{{ .Values.a }}:{{ $.Values.b }}";
        assert_eq!(
            normalize_values_global_context(src),
            "{{ $.Values.a }}:{{ $.Values.b }}"
        );
    }

    #[test]
    fn escape_template_action_wraps_action_for_literal_output() {
        let src = "{{ include \"x\" . }}";
        assert_eq!(
            escape_template_action(src),
            "{{ \"{{\" }} include \"x\" . {{ \"}}\" }}"
        );
    }

    #[test]
    fn collect_function_calls_in_action_extracts_pipeline_functions() {
        let calls = collect_function_calls_in_action(r#"{{ include "x" . | indent 2 | quote }}"#);
        assert_eq!(
            calls,
            vec![
                "include".to_string(),
                "indent".to_string(),
                "quote".to_string()
            ]
        );
    }

    #[test]
    fn collect_function_calls_in_action_ignores_keywords_fields_and_literals() {
        let calls = collect_function_calls_in_action(
            r#"{{ if .Values.enabled }}{{ default "x" .Values.name }}{{ end }}"#,
        );
        assert_eq!(calls, vec!["default".to_string()]);
    }

    #[test]
    fn collect_function_calls_in_action_supports_assignment_head() {
        let calls =
            collect_function_calls_in_action(r#"{{ $v := default "x" .Values.name | quote }}"#);
        assert_eq!(calls, vec!["default".to_string(), "quote".to_string()]);
    }

    #[test]
    fn collect_function_calls_respects_parenthesized_subpipeline() {
        let calls = collect_function_calls_in_action(
            r#"{{ include "x" (printf "%s|%s" "a" "b") | quote }}"#,
        );
        assert_eq!(calls, vec!["include".to_string(), "quote".to_string()]);
    }

    #[test]
    fn collect_function_calls_handles_unicode_string_literals() {
        let calls = collect_function_calls_in_action(r#"{{ printf "%s" "日本語" | quote }}"#);
        assert_eq!(calls, vec!["printf".to_string(), "quote".to_string()]);
    }

    #[test]
    fn collect_function_calls_ignores_literals_that_look_like_functions() {
        let calls = collect_function_calls_in_action(r#"{{ +12 | printf "%d" | quote }}"#);
        assert_eq!(calls, vec!["printf".to_string(), "quote".to_string()]);

        let calls = collect_function_calls_in_action(r#"{{ nil | default "x" }}"#);
        assert_eq!(calls, vec!["default".to_string()]);
    }

    #[test]
    fn collect_function_calls_ignores_non_identifier_heads() {
        let calls = collect_function_calls_in_action(r#"{{ +foo "x" | quote }}"#);
        assert_eq!(calls, vec!["quote".to_string()]);
    }

    #[test]
    fn collect_function_calls_in_template_dedupes_and_aggregates() {
        let calls = collect_function_calls_in_template(
            r#"
{{ include "x" . | quote }}
{{ default "a" .Values.name }}
{{ include "x" . | quote }}
"#,
        );
        assert_eq!(
            calls,
            vec![
                "default".to_string(),
                "include".to_string(),
                "quote".to_string()
            ]
        );
    }
}
