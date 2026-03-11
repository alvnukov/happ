use crate::gotemplates::{scan_template_actions as scan_go_template_actions, GoTemplateScanError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
enum TemplateBlockFrame {
    Define { name: String, start: usize },
    Other,
}

#[derive(Debug, Clone)]
struct LineIndex {
    newline_offsets: Vec<usize>,
}

impl LineIndex {
    fn new(src: &str) -> Self {
        let mut newline_offsets = Vec::new();
        for (idx, b) in src.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                newline_offsets.push(idx);
            }
        }
        Self { newline_offsets }
    }

    fn line_col(&self, byte_offset: usize) -> (usize, usize) {
        let line_idx = self
            .newline_offsets
            .partition_point(|&offset| offset < byte_offset);
        let line_start = if line_idx == 0 {
            0usize
        } else {
            self.newline_offsets[line_idx - 1] + 1
        };
        (
            line_idx + 1,
            byte_offset.saturating_sub(line_start).saturating_add(1),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateDiagnostic {
    pub code: String,
    pub message: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateAnalysis {
    pub include_names: Vec<String>,
    pub values_paths: Vec<Vec<String>>,
    pub define_blocks: BTreeMap<String, String>,
    pub include_graph: BTreeMap<String, Vec<String>>,
    pub unresolved_local_includes: Vec<String>,
    pub diagnostics: Vec<TemplateDiagnostic>,
    pub include_cycles: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChartTemplateAnalysis {
    pub files: BTreeMap<String, TemplateAnalysis>,
    pub define_blocks: BTreeMap<String, String>,
    pub include_graph: BTreeMap<String, Vec<String>>,
    pub unresolved_includes: Vec<String>,
    pub include_cycles: Vec<Vec<String>>,
}

pub fn analyze_template(src: &str) -> TemplateAnalysis {
    let line_index = LineIndex::new(src);
    let (spans, scan_errors) = scan_go_template_actions(src);
    let mut diagnostics = scan_errors_to_diagnostics(&line_index, &scan_errors);
    let mut include_names = BTreeSet::new();
    let mut values_paths = BTreeSet::new();
    for span in spans {
        let action = &src[span.start..span.end];
        if let Some(nested_offset) = find_nested_left_delim_in_action(action) {
            diagnostics.push(make_diagnostic(
                &line_index,
                span.start + nested_offset,
                "nested_action_before_close",
                "found new '{{' before closing current template action".to_string(),
            ));
            let Some(nested_fragment) = action.get(nested_offset..) else {
                continue;
            };
            for name in collect_include_names_in_template(nested_fragment) {
                let _ = include_names.insert(name);
            }
            for path in collect_values_paths_in_template(nested_fragment) {
                let _ = values_paths.insert(path.join("."));
            }
            continue;
        }
        let (action_include_names, action_diagnostics) =
            collect_include_names_in_action_with_diagnostics(&line_index, action, span.start);
        for name in action_include_names {
            let _ = include_names.insert(name);
        }
        diagnostics.extend(action_diagnostics);
        for path in collect_values_paths_in_action(action) {
            let _ = values_paths.insert(path.join("."));
        }
    }
    let define_blocks = extract_define_blocks(src);
    let include_graph = build_include_graph(&define_blocks);
    let include_cycles = detect_include_cycles(&include_graph);
    let defined: BTreeSet<String> = define_blocks.keys().cloned().collect();
    let unresolved_local_includes: Vec<String> = include_names
        .iter()
        .filter(|name| !defined.contains(*name))
        .cloned()
        .collect();
    diagnostics.sort_by(|a, b| {
        (a.line, a.column, a.code.as_str(), a.message.as_str()).cmp(&(
            b.line,
            b.column,
            b.code.as_str(),
            b.message.as_str(),
        ))
    });
    diagnostics.dedup();

    TemplateAnalysis {
        include_names: include_names.into_iter().collect(),
        values_paths: values_paths
            .into_iter()
            .map(|path| path.split('.').map(ToString::to_string).collect())
            .collect(),
        define_blocks,
        include_graph,
        unresolved_local_includes,
        diagnostics,
        include_cycles,
    }
}

pub fn analyze_chart_templates(files: &BTreeMap<String, String>) -> ChartTemplateAnalysis {
    let mut file_reports = BTreeMap::new();
    let mut define_blocks = BTreeMap::new();
    let mut all_include_names = BTreeSet::new();
    for (path, body) in files {
        let analyzed = analyze_template(body);
        for include_name in &analyzed.include_names {
            let _ = all_include_names.insert(include_name.clone());
        }
        for (name, block) in &analyzed.define_blocks {
            define_blocks
                .entry(name.clone())
                .or_insert_with(|| block.clone());
        }
        file_reports.insert(path.clone(), analyzed);
    }

    let include_graph = build_include_graph(&define_blocks);
    let defined: BTreeSet<String> = define_blocks.keys().cloned().collect();
    let unresolved_includes: Vec<String> = all_include_names
        .into_iter()
        .filter(|name| !defined.contains(name))
        .collect();
    let include_cycles = detect_include_cycles(&include_graph);

    ChartTemplateAnalysis {
        files: file_reports,
        define_blocks,
        include_graph,
        unresolved_includes,
        include_cycles,
    }
}

pub fn collect_include_names_in_action(action: &str) -> Vec<String> {
    let bytes = action.as_bytes();
    let mut out = Vec::new();
    let include = b"include";
    let mut i = 0usize;
    while i + include.len() <= bytes.len() {
        if !bytes_starts_with_at(bytes, i, include) {
            i += 1;
            continue;
        }
        if i > 0 && is_include_ident_char(bytes[i - 1]) {
            i += 1;
            continue;
        }

        let mut j = i + include.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            i += 1;
            continue;
        }

        let start = j + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != quote {
            end += 1;
        }
        if end < bytes.len() {
            if let Some(name) = action.get(start..end) {
                out.push(name.to_string());
            }
            i = end + 1;
            continue;
        }
        break;
    }
    out
}

pub fn collect_include_names_in_template(src: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for action in template_actions(src) {
        for name in collect_include_names_in_action(action) {
            let _ = out.insert(name);
        }
    }
    out.into_iter().collect()
}

pub fn collect_values_paths_in_action(action: &str) -> Vec<Vec<String>> {
    let mut out = BTreeSet::new();
    let bytes = action.as_bytes();
    for marker in ["$.Values.", ".Values."] {
        let mut cursor = 0usize;
        while cursor < action.len() {
            let Some(slice) = action.get(cursor..) else {
                break;
            };
            let Some(rel) = slice.find(marker) else {
                break;
            };
            let marker_pos = cursor + rel;
            if !is_values_marker_boundary(bytes, marker_pos) {
                cursor = next_char_boundary(action, marker_pos + marker.len());
                continue;
            }
            let start = marker_pos + marker.len();
            let mut end = start;
            while end < action.len() {
                let b = action.as_bytes()[end];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
                    end += 1;
                    continue;
                }
                break;
            }
            if end > start {
                let Some(raw) = action.get(start..end) else {
                    cursor = next_char_boundary(action, end.max(start.saturating_add(1)));
                    continue;
                };
                if !raw.starts_with('.') && !raw.ends_with('.') && !raw.contains("..") {
                    let segs: Vec<String> = raw
                        .split('.')
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .collect();
                    if !segs.is_empty() {
                        let _ = out.insert(segs.join("."));
                    }
                }
            }
            cursor = next_char_boundary(action, end.max(start.saturating_add(1)));
        }
    }
    out.into_iter()
        .map(|path| path.split('.').map(ToString::to_string).collect())
        .collect()
}

pub fn collect_values_paths_in_template(src: &str) -> Vec<Vec<String>> {
    let mut out = BTreeSet::new();
    for action in template_actions(src) {
        for path in collect_values_paths_in_action(action) {
            let _ = out.insert(path.join("."));
        }
    }
    out.into_iter()
        .map(|path| path.split('.').map(ToString::to_string).collect())
        .collect()
}

pub fn extract_define_blocks(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<TemplateBlockFrame> = Vec::new();
    let mut cursor = 0usize;
    while cursor < src.len() {
        let Some(slice) = src.get(cursor..) else {
            break;
        };
        let Some(open_rel) = slice.find("{{") else {
            break;
        };
        let open = cursor + open_rel;
        let action_start = open + 2;
        let Some(close_rel) = src[action_start..].find("}}") else {
            break;
        };
        let close = action_start + close_rel;
        let full_close = close + 2;
        let action = src[action_start..close].trim();
        let token = template_action_token(action);
        match token.as_str() {
            "define" => {
                if let Some(name) = template_define_name(action) {
                    stack.push(TemplateBlockFrame::Define { name, start: open });
                } else {
                    stack.push(TemplateBlockFrame::Other);
                }
            }
            "if" | "range" | "with" | "block" => stack.push(TemplateBlockFrame::Other),
            "end" => {
                let Some(frame) = stack.pop() else {
                    cursor = full_close;
                    continue;
                };
                let TemplateBlockFrame::Define { name, start } = frame else {
                    cursor = full_close;
                    continue;
                };
                out.entry(name)
                    .or_insert_with(|| src[start..full_close].to_string());
            }
            _ => {}
        }
        cursor = full_close;
    }
    out
}

fn template_actions(src: &str) -> Vec<&str> {
    let (spans, _) = scan_go_template_actions(src);
    spans
        .into_iter()
        .map(|span| &src[span.start..span.end])
        .collect()
}

fn template_action_token(action: &str) -> String {
    let trimmed = action.trim().trim_start_matches('-').trim_start();
    trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn find_nested_left_delim_in_action(action: &str) -> Option<usize> {
    if !(action.starts_with("{{") && action.ends_with("}}")) || action.len() < 4 {
        return None;
    }
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
    }
    let bytes = action.as_bytes();
    let mut i = 2usize;
    let end = action.len().saturating_sub(2);
    let mut state = State::Normal;
    while i < end {
        match state {
            State::Normal => {
                if i + 1 < end && bytes[i] == b'{' && bytes[i + 1] == b'{' {
                    return Some(i);
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
        }
    }
    None
}

fn template_define_name(action: &str) -> Option<String> {
    let trimmed = action.trim().trim_start_matches('-').trim_start();
    let rest = trimmed.strip_prefix("define")?.trim_start();
    let mut chars = rest.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    for ch in chars {
        if ch == quote {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

fn is_include_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
}

fn bytes_starts_with_at(bytes: &[u8], offset: usize, needle: &[u8]) -> bool {
    bytes
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|slice| slice == needle)
}

fn next_char_boundary(text: &str, mut idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn is_values_marker_boundary(bytes: &[u8], marker_pos: usize) -> bool {
    if marker_pos == 0 {
        return true;
    }
    let prev = bytes[marker_pos - 1];
    !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' || prev == b'$')
}

fn build_include_graph(define_blocks: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut graph = BTreeMap::new();
    for (define_name, define_body) in define_blocks {
        graph.insert(
            define_name.clone(),
            collect_include_names_in_template(define_body),
        );
    }
    graph
}

fn detect_include_cycles(graph: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    fn canonical_cycle(cycle: &[String]) -> String {
        if cycle.len() <= 1 {
            return cycle.join("->");
        }
        let mut core = cycle.to_vec();
        if core.first() == core.last() {
            let _ = core.pop();
        }
        if core.is_empty() {
            return String::new();
        }
        let mut best = core.clone();
        for shift in 1..core.len() {
            let mut rotated = core[shift..].to_vec();
            rotated.extend_from_slice(&core[..shift]);
            if rotated < best {
                best = rotated;
            }
        }
        best.push(best[0].clone());
        best.join("->")
    }

    let mut colors: BTreeMap<String, Color> =
        graph.keys().map(|k| (k.clone(), Color::White)).collect();
    let mut stack: Vec<String> = Vec::new();
    let mut cycle_keys = BTreeSet::new();
    let mut cycle_store: BTreeMap<String, Vec<String>> = BTreeMap::new();

    fn dfs(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
        colors: &mut BTreeMap<String, Color>,
        stack: &mut Vec<String>,
        cycle_keys: &mut BTreeSet<String>,
        cycle_store: &mut BTreeMap<String, Vec<String>>,
    ) {
        colors.insert(node.to_string(), Color::Gray);
        stack.push(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !graph.contains_key(neighbor) {
                    continue;
                }
                let neighbor_color = colors.get(neighbor).copied().unwrap_or(Color::White);
                if neighbor_color == Color::White {
                    dfs(neighbor, graph, colors, stack, cycle_keys, cycle_store);
                    continue;
                }
                if neighbor_color == Color::Gray {
                    let Some(pos) = stack.iter().position(|v| v == neighbor) else {
                        continue;
                    };
                    let mut cycle = stack[pos..].to_vec();
                    cycle.push(neighbor.clone());
                    let key = canonical_cycle(&cycle);
                    if cycle_keys.insert(key.clone()) {
                        cycle_store.insert(key, cycle);
                    }
                }
            }
        }

        let _ = stack.pop();
        colors.insert(node.to_string(), Color::Black);
    }

    let nodes: Vec<String> = graph.keys().cloned().collect();
    for node in nodes {
        let color = colors.get(&node).copied().unwrap_or(Color::White);
        if color == Color::White {
            dfs(
                &node,
                graph,
                &mut colors,
                &mut stack,
                &mut cycle_keys,
                &mut cycle_store,
            );
        }
    }

    cycle_store.into_values().collect()
}

fn collect_include_names_in_action_with_diagnostics(
    line_index: &LineIndex,
    action: &str,
    action_start: usize,
) -> (Vec<String>, Vec<TemplateDiagnostic>) {
    let bytes = action.as_bytes();
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    let include = b"include";
    let mut i = 0usize;
    while i + include.len() <= bytes.len() {
        if !bytes_starts_with_at(bytes, i, include) {
            i += 1;
            continue;
        }
        if i > 0 && is_include_ident_char(bytes[i - 1]) {
            i += 1;
            continue;
        }

        let mut j = i + include.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            diagnostics.push(make_diagnostic(
                line_index,
                action_start + i,
                "invalid_include_call",
                "include call is missing template name".to_string(),
            ));
            break;
        }
        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            diagnostics.push(make_diagnostic(
                line_index,
                action_start + i,
                "invalid_include_call",
                "include call must use quoted template name".to_string(),
            ));
            i = j + 1;
            continue;
        }

        let start = j + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != quote {
            end += 1;
        }
        if end < bytes.len() {
            if let Some(name) = action.get(start..end) {
                out.push(name.to_string());
            }
            i = end + 1;
            continue;
        }
        diagnostics.push(make_diagnostic(
            line_index,
            action_start + i,
            "invalid_include_call",
            "include call has unterminated quoted template name".to_string(),
        ));
        break;
    }
    (out, diagnostics)
}

fn make_diagnostic(
    line_index: &LineIndex,
    byte_offset: usize,
    code: &str,
    message: String,
) -> TemplateDiagnostic {
    let (line, column) = line_index.line_col(byte_offset);
    TemplateDiagnostic {
        code: code.to_string(),
        message,
        line,
        column,
    }
}

fn scan_errors_to_diagnostics(
    line_index: &LineIndex,
    errors: &[GoTemplateScanError],
) -> Vec<TemplateDiagnostic> {
    errors
        .iter()
        .map(|err| make_diagnostic(line_index, err.offset, err.code, err.message.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_extraction_reads_only_quoted_include_name_without_trailing_context_dot() {
        let names = collect_include_names_in_action(r#"{{ include "foo.bar" . }}"#);
        assert_eq!(names, vec!["foo.bar".to_string()]);
    }

    #[test]
    fn include_extraction_ignores_escaped_literal_include_text() {
        let names = collect_include_names_in_template(
            r#"
global:
  x: '{{ include "foo.a" . }}'
  y: '{{ "{{" }} include "foo.b" . {{ "}}" }}'
"#,
        );
        assert_eq!(names, vec!["foo.a".to_string()]);
    }

    #[test]
    fn include_extraction_in_action_skips_identifier_prefixed_tokens() {
        let names = collect_include_names_in_action(
            r#"{{ myinclude "x.a" . }} {{ includeX "x.b" . }} {{ include "x.c" . }}"#,
        );
        assert_eq!(names, vec!["x.c".to_string()]);
    }

    #[test]
    fn include_extraction_returns_empty_for_unterminated_quote() {
        let names = collect_include_names_in_action(r#"{{ include "foo.bar . }}"#);
        assert!(names.is_empty());
    }

    #[test]
    fn include_extraction_handles_unicode_action_without_panicking() {
        let names = collect_include_names_in_action(
            r#"{{- fail (printf "Не установлены лимиты по памяти %s" $.CurrentApp.name) }}"#,
        );
        assert!(names.is_empty());
    }

    #[test]
    fn include_extraction_template_dedupes_and_sorts_names() {
        let names = collect_include_names_in_template(
            r#"
{{ include "z.b" . }}
{{ include "a.a" . }}
{{ include "z.b" . }}
"#,
        );
        assert_eq!(names, vec!["a.a".to_string(), "z.b".to_string()]);
    }

    #[test]
    fn include_extraction_template_keeps_parsed_actions_before_broken_tail() {
        let names = collect_include_names_in_template(
            r#"
{{ include "ok.a" . }}
{{ include "broken.a" . 
"#,
        );
        assert_eq!(names, vec!["ok.a".to_string()]);
    }

    #[test]
    fn values_paths_extracts_dot_and_root_values_variants() {
        let paths = collect_values_paths_in_template(
            r#"{{ default .Values.cluster.name $.Values.serviceAccount.name }}"#,
        );
        assert_eq!(
            paths,
            vec![
                vec!["cluster".to_string(), "name".to_string()],
                vec!["serviceAccount".to_string(), "name".to_string()]
            ]
        );
    }

    #[test]
    fn values_paths_in_action_supports_hyphen_and_underscore_segments() {
        let paths =
            collect_values_paths_in_action(r#"{{ $.Values.cluster-security.node_name.value }}"#);
        assert_eq!(
            paths,
            vec![vec![
                "cluster-security".to_string(),
                "node_name".to_string(),
                "value".to_string(),
            ]]
        );
    }

    #[test]
    fn values_paths_ignores_invalid_or_broken_paths() {
        let paths = collect_values_paths_in_action(
            r#"{{ .Values..broken }} {{ .Values.good..bad }} {{ .Values.also-bad. }}"#,
        );
        assert!(paths.is_empty());
    }

    #[test]
    fn values_paths_handle_non_ascii_segment_without_panicking() {
        let paths = collect_values_paths_in_action(r#"{{ .Values.Ж }}"#);
        assert!(paths.is_empty());
    }

    #[test]
    fn values_paths_template_dedupes_and_sorts() {
        let paths = collect_values_paths_in_template(
            r#"
{{ .Values.z.last }}
{{ $.Values.a.first }}
{{ .Values.z.last }}
"#,
        );
        assert_eq!(
            paths,
            vec![
                vec!["a".to_string(), "first".to_string()],
                vec!["z".to_string(), "last".to_string()],
            ]
        );
    }

    #[test]
    fn values_paths_ignore_non_values_markers() {
        let paths = collect_values_paths_in_template(
            r#"{{ .Value.one }} {{ $Values.two }} {{ Values.three }}"#,
        );
        assert!(paths.is_empty());
    }

    #[test]
    fn values_paths_ignore_non_root_values_chains() {
        let paths = collect_values_paths_in_template(
            r#"{{ .Thing.Values.fake }} {{ my.Values.fake2 }} {{ $.Values.real.path }}"#,
        );
        assert_eq!(paths, vec![vec!["real".to_string(), "path".to_string()]]);
    }

    #[test]
    fn define_block_extraction_keeps_nested_if_range_and_end_balance() {
        let blocks = extract_define_blocks(
            r#"
{{- define "foo.a" -}}
{{- if .Values.enabled -}}
{{ include "foo.b" . }}
{{- end -}}
{{- end -}}
{{- define "foo.b" -}}OK{{- end -}}
"#,
        );
        assert!(blocks.contains_key("foo.a"));
        assert!(blocks.contains_key("foo.b"));
        assert!(blocks["foo.a"].contains(r#"define "foo.a""#));
        assert!(blocks["foo.a"].contains(r#"include "foo.b""#));
    }

    #[test]
    fn define_block_extraction_uses_first_duplicate_definition() {
        let blocks = extract_define_blocks(
            r#"
{{- define "dup.a" -}}first{{- end -}}
{{- define "dup.a" -}}second{{- end -}}
"#,
        );
        assert!(blocks["dup.a"].contains("first"));
        assert!(!blocks["dup.a"].contains("second"));
    }

    #[test]
    fn define_block_extraction_supports_single_quoted_define_name() {
        let blocks = extract_define_blocks(
            r#"
{{- define 'single.q' -}}OK{{- end -}}
"#,
        );
        assert!(blocks.contains_key("single.q"));
    }

    #[test]
    fn define_block_extraction_skips_malformed_define_name() {
        let blocks = extract_define_blocks(
            r#"
{{- define bad.name -}}BAD{{- end -}}
{{- define "ok.name" -}}OK{{- end -}}
"#,
        );
        assert!(!blocks.contains_key("bad.name"));
        assert!(blocks.contains_key("ok.name"));
    }

    #[test]
    fn analyze_template_returns_combined_report() {
        let analyzed = analyze_template(
            r#"
{{- define "foo.a" -}}
{{ include "foo.b" . }}
{{ default .Values.cluster.name $.Values.serviceAccount.name }}
{{- end -}}
"#,
        );
        assert_eq!(analyzed.include_names, vec!["foo.b".to_string()]);
        assert_eq!(
            analyzed.values_paths,
            vec![
                vec!["cluster".to_string(), "name".to_string()],
                vec!["serviceAccount".to_string(), "name".to_string()],
            ]
        );
        assert!(analyzed.define_blocks.contains_key("foo.a"));
    }

    #[test]
    fn analyze_template_matches_individual_collectors() {
        let src = r#"
{{- define "x.a" -}}
{{ include "x.b" . }}
{{ .Values.cluster.name }}
{{- end -}}
"#;
        let analyzed = analyze_template(src);
        assert_eq!(
            analyzed.include_names,
            collect_include_names_in_template(src)
        );
        assert_eq!(analyzed.values_paths, collect_values_paths_in_template(src));
        assert_eq!(analyzed.define_blocks, extract_define_blocks(src));
    }

    #[test]
    fn analyze_template_reports_diagnostics_for_broken_actions_and_invalid_include() {
        let analyzed = analyze_template(
            r#"
{{ include "broken.a" . {{ include "ok.a" . }}
{{ include not_quoted . }}
"#,
        );
        assert_eq!(analyzed.include_names, vec!["ok.a".to_string()]);
        assert!(analyzed.diagnostics.iter().any(|d| {
            d.code == "nested_action_before_close" && d.message.contains("new '{{' before closing")
        }));
        assert!(analyzed.diagnostics.iter().any(|d| {
            d.code == "invalid_include_call" && d.message.contains("must use quoted template name")
        }));
    }

    #[test]
    fn analyze_template_reports_unterminated_action_with_precise_location() {
        let analyzed = analyze_template(
            r#"
kind: ConfigMap
data:
  key: {{ include "broken.name" . 
"#,
        );
        assert!(analyzed
            .diagnostics
            .iter()
            .any(|d| d.code == "unterminated_action" && d.line == 4 && d.column == 8));
    }

    #[test]
    fn analyze_template_with_unicode_action_does_not_panic() {
        let analyzed = analyze_template(
            r#"{{- fail (printf "Не установлены лимиты по памяти %s" $.CurrentApp.name) }}"#,
        );
        assert!(analyzed.include_names.is_empty());
    }

    #[test]
    fn analyze_template_detects_local_unresolved_and_cycle() {
        let analyzed = analyze_template(
            r#"
{{ include "missing.a" . }}
{{- define "recursion" -}}{{ include "recursion" . }}{{- end -}}
"#,
        );
        assert_eq!(
            analyzed.unresolved_local_includes,
            vec!["missing.a".to_string()]
        );
        assert!(analyzed
            .include_cycles
            .iter()
            .any(|c| { c.len() == 2 && c[0] == "recursion" && c[1] == "recursion" }));
    }

    #[test]
    fn chart_analysis_detects_recursion_cycle() {
        let mut files = BTreeMap::new();
        files.insert(
            "templates/base".to_string(),
            r#"{{include "recursion" . }}"#.to_string(),
        );
        files.insert(
            "templates/recursion".to_string(),
            r#"{{define "recursion"}}{{include "recursion" . }}{{end}}"#.to_string(),
        );
        let analyzed = analyze_chart_templates(&files);
        assert!(analyzed.unresolved_includes.is_empty());
        assert!(analyzed
            .include_cycles
            .iter()
            .any(|c| { c.len() == 2 && c[0] == "recursion" && c[1] == "recursion" }));
    }

    #[test]
    fn chart_analysis_reports_unresolved_include_across_files() {
        let mut files = BTreeMap::new();
        files.insert(
            "templates/cm.yaml".to_string(),
            r#"{{ include "missing.name" . }}"#.to_string(),
        );
        files.insert(
            "templates/_helpers.tpl".to_string(),
            r#"{{ define "present.name" }}x{{ end }}"#.to_string(),
        );
        let analyzed = analyze_chart_templates(&files);
        assert_eq!(
            analyzed.unresolved_includes,
            vec!["missing.name".to_string()]
        );
    }

    #[test]
    fn chart_analysis_keeps_first_define_by_file_order() {
        let mut files = BTreeMap::new();
        files.insert(
            "a-helpers.tpl".to_string(),
            r#"{{ define "dup.name" }}first{{ end }}"#.to_string(),
        );
        files.insert(
            "z-helpers.tpl".to_string(),
            r#"{{ define "dup.name" }}second{{ end }}"#.to_string(),
        );
        let analyzed = analyze_chart_templates(&files);
        assert!(analyzed.define_blocks["dup.name"].contains("first"));
        assert!(!analyzed.define_blocks["dup.name"].contains("second"));
    }

    #[test]
    fn chart_analysis_detects_multiple_distinct_cycles_without_duplicates() {
        let mut files = BTreeMap::new();
        files.insert(
            "templates/a.tpl".to_string(),
            r#"{{ define "a" }}{{ include "b" . }}{{ end }}"#.to_string(),
        );
        files.insert(
            "templates/b.tpl".to_string(),
            r#"{{ define "b" }}{{ include "a" . }}{{ end }}"#.to_string(),
        );
        files.insert(
            "templates/c.tpl".to_string(),
            r#"{{ define "c" }}{{ include "d" . }}{{ end }}"#.to_string(),
        );
        files.insert(
            "templates/d.tpl".to_string(),
            r#"{{ define "d" }}{{ include "c" . }}{{ end }}"#.to_string(),
        );
        let analyzed = analyze_chart_templates(&files);
        assert_eq!(analyzed.include_cycles.len(), 2);
        assert!(analyzed
            .include_cycles
            .iter()
            .any(|c| c == &vec!["a".to_string(), "b".to_string(), "a".to_string()]));
        assert!(analyzed
            .include_cycles
            .iter()
            .any(|c| c == &vec!["c".to_string(), "d".to_string(), "c".to_string()]));
    }
}
