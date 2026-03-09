use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::templateanalyzer::{
    collect_include_names_in_template, collect_values_paths_in_template, extract_define_blocks,
};
use crate::templatepolicy::is_supported_library_include;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("yaml format: {0}")]
    YamlFormat(String),
    #[error("library chart: {0}")]
    Library(String),
}

pub fn values_yaml(values: &Value) -> Result<String, Error> {
    values_yaml_with_yaml_anchors(values, false)
}

pub fn values_yaml_with_yaml_anchors(values: &Value, yaml_anchors: bool) -> Result<String, Error> {
    let mut root = values.as_mapping().cloned().unwrap_or_default();
    let mut ordered = Mapping::new();
    if let Some(g) = root.remove(Value::String("global".into())) {
        ordered.insert(Value::String("global".into()), g);
    }
    let mut keys: Vec<String> = root
        .keys()
        .filter_map(|k| k.as_str().map(ToString::to_string))
        .collect();
    keys.sort();
    for k in keys {
        if let Some(v) = root.remove(Value::String(k.clone())) {
            ordered.insert(Value::String(k), v);
        }
    }
    let ordered_value = Value::Mapping(ordered);
    if yaml_anchors {
        let json = serde_json::to_value(&ordered_value)
            .map_err(|e| Error::YamlFormat(format!("YAML->JSON conversion error: {e}")))?;
        let text = zq::format_output_yaml_documents_with_options(
            std::slice::from_ref(&json),
            zq::YamlFormatOptions::default().with_yaml_anchors(true),
        )
        .map_err(|e| Error::YamlFormat(format!("yaml anchors encode error: {e}")))?;
        return Ok(text.trim_start_matches("---\n").to_string());
    }
    let text = serde_yaml::to_string(&ordered_value)?;
    Ok(text.trim_start_matches("---\n").to_string())
}

pub fn write_values(path: Option<&str>, values: &Value) -> Result<(), Error> {
    write_values_with_yaml_anchors(path, values, false)
}

pub fn write_values_with_yaml_anchors(
    path: Option<&str>,
    values: &Value,
    yaml_anchors: bool,
) -> Result<(), Error> {
    let body = values_yaml_with_yaml_anchors(values, yaml_anchors)?;
    if let Some(p) = path {
        fs::write(p, body.as_bytes())?;
    } else {
        let mut out = io::stdout();
        out.write_all(body.as_bytes())?;
    }
    Ok(())
}

pub fn generate_consumer_chart(
    out_dir: &str,
    chart_name: Option<&str>,
    values: &Value,
    library_chart_path: Option<&str>,
    yaml_anchors: bool,
) -> Result<(), Error> {
    let chart_name = chart_name
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            Path::new(out_dir)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("happ-imported")
                .to_string()
        });

    fs::create_dir_all(Path::new(out_dir).join("templates"))?;
    fs::create_dir_all(Path::new(out_dir).join("charts"))?;

    let chart_yaml = format!(
        "apiVersion: v2\nname: {}\nversion: 0.1.0\ntype: application\n",
        chart_name
    );
    fs::write(Path::new(out_dir).join("Chart.yaml"), chart_yaml.as_bytes())?;
    fs::write(
        Path::new(out_dir).join("templates/init-helm-apps-library.yaml"),
        b"{{- include \"apps-utils.init-library\" $ }}\n",
    )?;
    let mut values_for_chart = values.clone();
    normalize_library_template_strings_value(&mut values_for_chart);
    write_values_with_yaml_anchors(
        Some(&Path::new(out_dir).join("values.yaml").to_string_lossy()),
        &values_for_chart,
        yaml_anchors,
    )?;

    let dst = Path::new(out_dir).join("charts/helm-apps");
    let src = resolve_library_path(library_chart_path)?;
    if let Some(src) = src {
        copy_dir(&src, &dst)?;
    } else if crate::assets::has_helm_apps_chart() {
        crate::assets::extract_helm_apps_chart(&dst)?;
    } else {
        return Err(Error::Library(
            "embedded helm-apps chart is unavailable and no local library chart path was resolved"
                .to_string(),
        ));
    }
    Ok(())
}

pub fn copy_chart_crds_if_any(source_chart_path: &str, out_dir: &str) -> Result<bool, Error> {
    let src_crds = Path::new(source_chart_path).join("crds");
    if !src_crds.exists() || !src_crds.is_dir() {
        return Ok(false);
    }
    let dst_crds = Path::new(out_dir).join("crds");
    copy_dir(&src_crds, &dst_crds)?;
    Ok(true)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedIncludeHelpersSync {
    pub added: Vec<String>,
    pub missing: Vec<String>,
}

pub fn sync_imported_include_helpers_from_source_chart(
    source_chart_path: &str,
    out_dir: &str,
    values_yaml: &str,
) -> Result<ImportedIncludeHelpersSync, Error> {
    let wanted: BTreeSet<String> = collect_include_names_from_values(values_yaml)
        .into_iter()
        .filter(|name| !is_supported_library_include(name))
        .collect();
    if wanted.is_empty() {
        return Ok(ImportedIncludeHelpersSync::default());
    }

    let src_templates = Path::new(source_chart_path).join("templates");
    if !src_templates.is_dir() {
        return Ok(ImportedIncludeHelpersSync {
            added: Vec::new(),
            missing: wanted.into_iter().collect(),
        });
    }

    let mut files = Vec::new();
    collect_template_files(&src_templates, &mut files)?;
    files.sort();

    let mut define_blocks: BTreeMap<String, String> = BTreeMap::new();
    for file in files {
        let content = match fs::read_to_string(&file) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for (name, block) in extract_define_blocks(&content) {
            define_blocks.entry(name).or_insert(block);
        }
    }

    let mut selected: BTreeMap<String, String> = BTreeMap::new();
    let mut missing = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut queue: Vec<String> = wanted.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let Some(block) = define_blocks.get(&name) else {
            let _ = missing.insert(name);
            continue;
        };
        selected.insert(name.clone(), normalize_imported_helper_block(block));
        for dep in collect_include_names_from_values(block) {
            if is_supported_library_include(&dep) {
                continue;
            }
            if !visited.contains(&dep) {
                queue.push(dep);
            }
        }
    }

    let mut added = Vec::new();
    let imported_tpl = Path::new(out_dir).join("templates/imported-source-includes.tpl");
    let mut body = String::from(
        "{{/*\nAuto-imported helper templates copied from source chart.\nGenerated by happ.\n*/}}\n\n",
    );
    for (name, block) in &selected {
        added.push(name.clone());
        body.push_str(block);
        if !block.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    }
    let missing: Vec<String> = missing.into_iter().collect();

    if !added.is_empty() {
        fs::write(&imported_tpl, body.as_bytes())?;
    } else if imported_tpl.exists() {
        let _ = fs::remove_file(imported_tpl);
    }
    Ok(ImportedIncludeHelpersSync { added, missing })
}

pub fn ensure_values_examples_for_imported_helpers(out_dir: &str) -> Result<Vec<String>, Error> {
    let helper_tpl = Path::new(out_dir).join("templates/imported-source-includes.tpl");
    if !helper_tpl.is_file() {
        return Ok(Vec::new());
    }
    let values_path = Path::new(out_dir).join("values.yaml");
    if !values_path.is_file() {
        return Ok(Vec::new());
    }
    let helper_body = fs::read_to_string(&helper_tpl)?;
    let paths = collect_values_paths_in_template(&helper_body);
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let values_src = fs::read_to_string(&values_path)?;
    let mut values: Value = serde_yaml::from_str(&values_src)?;
    let added = ensure_value_paths_present_with_examples(&mut values, &paths);
    if !added.is_empty() {
        let out = values_yaml(&values)?;
        fs::write(values_path, out.as_bytes())?;
    }
    Ok(added)
}

fn resolve_library_path(explicit: Option<&str>) -> Result<Option<PathBuf>, Error> {
    if let Some(p) = explicit {
        let pb = PathBuf::from(p);
        if pb.join("Chart.yaml").exists() {
            return Ok(Some(pb));
        }
        return Err(Error::Library(format!(
            "explicit path '{}' does not contain Chart.yaml",
            p
        )));
    }
    let candidate = PathBuf::from("charts/helm-apps");
    if candidate.join("Chart.yaml").exists() {
        return Ok(Some(candidate));
    }
    Ok(None)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), Error> {
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;
    copy_dir_inner(src, dst)
}

fn copy_dir_inner(src: &Path, dst: &Path) -> Result<(), Error> {
    for e in fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let target = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_inner(&p, &target)?;
        } else {
            fs::copy(&p, &target)?;
        }
    }
    Ok(())
}

fn collect_template_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    for e in fs::read_dir(path)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            collect_template_files(&p, out)?;
            continue;
        }
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "tpl" | "yaml" | "yml" | "txt") {
            out.push(p);
        }
    }
    Ok(())
}

fn collect_include_names_from_values(values_yaml: &str) -> Vec<String> {
    collect_include_names_in_template(values_yaml)
}

fn normalize_imported_helper_block(block: &str) -> String {
    rewrite_template_actions(block, normalize_values_scope_in_action)
}

fn normalize_library_template_strings_value(value: &mut Value) {
    match value {
        Value::String(src) => {
            *src = normalize_library_template_string(src);
        }
        Value::Mapping(map) => {
            for item in map.values_mut() {
                normalize_library_template_strings_value(item);
            }
        }
        Value::Sequence(seq) => {
            for item in seq {
                normalize_library_template_strings_value(item);
            }
        }
        _ => {}
    }
}

fn normalize_library_template_string(src: &str) -> String {
    rewrite_template_actions(src, normalize_library_action_context)
}

fn normalize_library_action_context(inner: &str) -> String {
    let root_normalized = normalize_values_scope_in_action(inner);
    normalize_include_context_to_root_in_action(&root_normalized)
}

fn rewrite_template_actions(src: &str, rewrite_inner: fn(&str) -> String) -> String {
    if !src.contains("{{") {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + 16);
    let mut cursor = 0usize;
    while cursor < src.len() {
        let Some(open_rel) = src[cursor..].find("{{") else {
            out.push_str(&src[cursor..]);
            break;
        };
        let open = cursor + open_rel;
        out.push_str(&src[cursor..open]);
        let action_start = open + 2;
        let Some(action_end) = find_template_action_close(src, action_start) else {
            out.push_str(&src[open..]);
            break;
        };
        let inner = &src[action_start..action_end];
        out.push_str("{{");
        if is_comment_action(inner) {
            out.push_str(inner);
        } else {
            out.push_str(&rewrite_inner(inner));
        }
        out.push_str("}}");
        cursor = action_end + 2;
    }
    out
}

fn find_template_action_close(src: &str, action_start: usize) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        Raw,
    }

    let bytes = src.as_bytes();
    let mut state = State::Normal;
    let mut i = action_start;
    while i + 1 < bytes.len() {
        let ch = bytes[i];
        match state {
            State::Normal => match ch {
                b'\'' => {
                    state = State::Single;
                    i += 1;
                    continue;
                }
                b'"' => {
                    state = State::Double;
                    i += 1;
                    continue;
                }
                b'`' => {
                    state = State::Raw;
                    i += 1;
                    continue;
                }
                b'}' if bytes[i + 1] == b'}' => return Some(i),
                _ => {}
            },
            State::Single => {
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if ch == b'\'' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Double => {
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if ch == b'"' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Raw => {
                if ch == b'`' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
        }
        i += 1;
    }
    None
}

fn is_comment_action(inner: &str) -> bool {
    let trimmed = inner.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '-');
    trimmed.starts_with("/*")
}

fn normalize_values_scope_in_action(inner: &str) -> String {
    const ROOT_TOKENS: [&[u8]; 6] = [
        b".Values",
        b".Release",
        b".Chart",
        b".Capabilities",
        b".Files",
        b".Template",
    ];

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        Raw,
    }

    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 8);
    let mut state = State::Normal;
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        match state {
            State::Single => {
                out.push(ch);
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i]);
                } else if ch == b'\'' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Double => {
                out.push(ch);
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i]);
                } else if ch == b'"' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Raw => {
                out.push(ch);
                if ch == b'`' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Normal => {}
        }

        match ch {
            b'\'' => {
                state = State::Single;
                out.push(ch);
                i += 1;
                continue;
            }
            b'"' => {
                state = State::Double;
                out.push(ch);
                i += 1;
                continue;
            }
            b'`' => {
                state = State::Raw;
                out.push(ch);
                i += 1;
                continue;
            }
            _ => {}
        }

        let mut matched = false;
        for token in ROOT_TOKENS {
            if starts_with_root_ref(bytes, i, token)
                && should_rewrite_root_ref(bytes, i, token.len())
            {
                out.push(b'$');
                out.extend_from_slice(token);
                i += token.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        out.push(ch);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| inner.to_string())
}

fn normalize_include_context_to_root_in_action(inner: &str) -> String {
    if !inner.contains("include") && !inner.contains("template") {
        return inner.to_string();
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        Raw,
    }

    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 4);
    let mut state = State::Normal;
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        match state {
            State::Single => {
                out.push(ch);
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i]);
                } else if ch == b'\'' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Double => {
                out.push(ch);
                if ch == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i]);
                } else if ch == b'"' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Raw => {
                out.push(ch);
                if ch == b'`' {
                    state = State::Normal;
                }
                i += 1;
                continue;
            }
            State::Normal => {}
        }

        match ch {
            b'\'' => {
                state = State::Single;
                out.push(ch);
                i += 1;
                continue;
            }
            b'"' => {
                state = State::Double;
                out.push(ch);
                i += 1;
                continue;
            }
            b'`' => {
                state = State::Raw;
                out.push(ch);
                i += 1;
                continue;
            }
            _ => {}
        }

        if let Some(dot_idx) = match_include_context_dot(bytes, i) {
            out.extend_from_slice(&bytes[i..dot_idx]);
            out.push(b'$');
            i = dot_idx + 1;
            continue;
        }

        out.push(ch);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| inner.to_string())
}

fn starts_with_root_ref(bytes: &[u8], idx: usize, token: &[u8]) -> bool {
    bytes.get(idx..).is_some_and(|tail| tail.starts_with(token))
}

fn should_rewrite_root_ref(bytes: &[u8], idx: usize, token_len: usize) -> bool {
    let prev = idx.checked_sub(1).and_then(|i| bytes.get(i)).copied();
    if prev.is_some_and(is_ref_name_char) || prev.is_some_and(|b| b == b'$' || b == b'.') {
        return false;
    }
    let next = bytes.get(idx + token_len).copied();
    if next.is_some_and(is_ref_name_char) {
        return false;
    }
    true
}

fn match_include_context_dot(bytes: &[u8], start: usize) -> Option<usize> {
    let keyword_len = if bytes
        .get(start..)
        .is_some_and(|tail| tail.starts_with(b"include"))
    {
        "include".len()
    } else if bytes
        .get(start..)
        .is_some_and(|tail| tail.starts_with(b"template"))
    {
        "template".len()
    } else {
        return None;
    };
    if !is_keyword_boundary(bytes, start, keyword_len) {
        return None;
    }
    let mut i = start + keyword_len;
    if i >= bytes.len() || !is_space_byte(bytes[i]) {
        return None;
    }
    while i < bytes.len() && is_space_byte(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    i += 1;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if ch == quote {
            i += 1;
            break;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    while i < bytes.len() && is_space_byte(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'.' {
        return None;
    }
    let next = bytes.get(i + 1).copied();
    if next.is_some_and(is_ref_name_char) || next.is_some_and(|b| b == b'.' || b == b'$') {
        return None;
    }
    Some(i)
}

fn is_keyword_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let prev = start.checked_sub(1).and_then(|i| bytes.get(i)).copied();
    if prev.is_some_and(is_ref_name_char) {
        return false;
    }
    let next = bytes.get(start + len).copied();
    if next.is_some_and(is_ref_name_char) {
        return false;
    }
    true
}

fn is_space_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_ref_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn ensure_value_paths_present_with_examples(
    root: &mut Value,
    paths: &[Vec<String>],
) -> Vec<String> {
    let mut added = BTreeSet::new();
    for segs in paths {
        if segs.is_empty() {
            continue;
        }
        if ensure_single_value_path_with_example(root, segs) {
            let _ = added.insert(segs.join("."));
        }
    }
    added.into_iter().collect()
}

fn ensure_single_value_path_with_example(node: &mut Value, segs: &[String]) -> bool {
    if segs.is_empty() {
        return false;
    }
    if !node.is_mapping() {
        *node = Value::Mapping(Mapping::new());
    }
    let Value::Mapping(map) = node else {
        return false;
    };
    let key = Value::String(segs[0].clone());
    if segs.len() == 1 {
        if map.contains_key(&key) {
            return false;
        }
        map.insert(key, Value::String("<example>".to_string()));
        return true;
    }
    if !map.contains_key(&key) {
        map.insert(key.clone(), Value::Mapping(Mapping::new()));
    }
    let Some(child) = map.get_mut(&key) else {
        return false;
    };
    if !child.is_mapping() {
        return false;
    }
    ensure_single_value_path_with_example(child, &segs[1..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn puts_global_first_in_values_yaml() {
        let mut root = Mapping::new();
        root.insert(
            Value::String("apps-k8s-manifests".into()),
            Value::Mapping(Mapping::new()),
        );
        root.insert(
            Value::String("global".into()),
            Value::Mapping(Mapping::new()),
        );
        let txt = values_yaml(&Value::Mapping(root)).expect("yaml");
        assert!(txt.starts_with("global:"));
    }

    #[test]
    fn creates_consumer_chart_files() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("chart");
        let mut root = Mapping::new();
        root.insert(
            Value::String("global".into()),
            Value::Mapping(Mapping::new()),
        );
        root.insert(
            Value::String("apps-k8s-manifests".into()),
            Value::Mapping(Mapping::new()),
        );
        generate_consumer_chart(
            out.to_str().expect("path"),
            Some("demo"),
            &Value::Mapping(root),
            None,
            false,
        )
        .expect("generate");
        assert!(out.join("Chart.yaml").exists());
        assert!(out.join("values.yaml").exists());
        assert!(out.join("templates/init-helm-apps-library.yaml").exists());
        assert!(out.join("charts/helm-apps/Chart.yaml").exists());
    }

    #[test]
    fn generate_consumer_chart_normalizes_include_context_for_library_values() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("chart");
        let values: Value = serde_yaml::from_str(
            r#"
global: {}
apps-k8s-manifests:
  demo:
    spec: |
      x: '{{ include "foo.bar" . }}'
      r: '{{ .Release.Name }}'
      y: '{{ "{{" }} include "foo.skip" . {{ "}}" }}'
"#,
        )
        .expect("parse values");

        generate_consumer_chart(out.to_str().expect("path"), Some("demo"), &values, None, false)
            .expect("generate");

        let saved = fs::read_to_string(out.join("values.yaml")).expect("read values");
        assert!(saved.contains(r#"include "foo.bar" $"#));
        assert!(saved.contains(r#"{{ $.Release.Name }}"#));
        assert!(saved.contains(r#"{{ "{{" }} include "foo.skip" . {{ "}}" }}"#));
    }

    #[test]
    fn rejects_invalid_explicit_library_path() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("chart");
        let mut root = Mapping::new();
        root.insert(
            Value::String("global".into()),
            Value::Mapping(Mapping::new()),
        );
        let err = generate_consumer_chart(
            out.to_str().expect("path"),
            Some("demo"),
            &Value::Mapping(root),
            Some("/definitely/not/exist"),
            false,
        )
        .expect_err("must fail");
        assert!(matches!(err, Error::Library(_)), "{err:?}");
    }

    #[test]
    fn copies_crds_from_source_chart_when_present() {
        let td = TempDir::new().expect("tmp");
        let src = td.path().join("src-chart");
        let out = td.path().join("out-chart");
        fs::create_dir_all(src.join("crds")).expect("mkdir");
        fs::write(
            src.join("crds/demo.example.com.yaml"),
            "kind: CustomResourceDefinition\n",
        )
        .expect("write");

        let copied = copy_chart_crds_if_any(src.to_str().expect("src"), out.to_str().expect("out"))
            .expect("copy");
        assert!(copied);
        assert!(out.join("crds/demo.example.com.yaml").exists());
    }

    #[test]
    fn collect_include_names_from_values_ignores_escaped_include_and_trailing_context_dot() {
        let names = collect_include_names_from_values(
            r#"
global:
  a: '{{ include "foo.bar" . }}'
  b: '{{ "{{" }} include "foo.baz" . {{ "}}" }}'
  c: '{{ include "apps-utils.init-library" $ }}'
"#,
        );
        assert_eq!(
            names,
            vec!["apps-utils.init-library".to_string(), "foo.bar".to_string()]
        );
    }

    #[test]
    fn sync_imported_include_helpers_copies_define_blocks_from_source_templates() {
        let td = TempDir::new().expect("tmp");
        let src = td.path().join("source-chart");
        let out = td.path().join("out-chart");
        fs::create_dir_all(src.join("templates")).expect("mkdir src templates");
        fs::create_dir_all(out.join("templates")).expect("mkdir out templates");
        fs::write(
            src.join("templates/_helpers.tpl"),
            r#"
{{- define "foo.cluster-name" -}}
{{- default (include "foo.name" .) .Values.cluster.name -}}
{{- end -}}
{{- define "foo.name" -}}
foo
{{- end -}}
{{- define "foo.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "foo.cluster-name" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}
"#,
        )
        .expect("write helpers");

        let sync = sync_imported_include_helpers_from_source_chart(
            src.to_str().expect("src"),
            out.to_str().expect("out"),
            r#"
global:
  n1: '{{ include "foo.cluster-name" . }}'
  n2: '{{ include "foo.serviceAccountName" . }}'
"#,
        )
        .expect("sync");
        assert_eq!(
            sync.added,
            vec![
                "foo.cluster-name".to_string(),
                "foo.name".to_string(),
                "foo.serviceAccountName".to_string()
            ]
        );
        assert!(sync.missing.is_empty());

        let out_tpl = fs::read_to_string(out.join("templates/imported-source-includes.tpl"))
            .expect("read imported tpl");
        assert!(out_tpl.contains(r#"define "foo.cluster-name""#));
        assert!(out_tpl.contains(r#"define "foo.name""#));
        assert!(out_tpl.contains(r#"define "foo.serviceAccountName""#));
        assert!(!out_tpl.contains(r#"define "foo.serviceAccountName.""#));
        assert!(out_tpl.contains("$.Values.cluster.name"));
        assert!(out_tpl.contains("$.Values.serviceAccount.create"));
        assert!(out_tpl.contains("$.Values.serviceAccount.name"));
        assert!(!out_tpl.contains(" .Values.serviceAccount.create "));
    }

    #[test]
    fn sync_imported_include_helpers_reports_missing_when_definition_not_found() {
        let td = TempDir::new().expect("tmp");
        let src = td.path().join("source-chart");
        let out = td.path().join("out-chart");
        fs::create_dir_all(src.join("templates")).expect("mkdir src templates");
        fs::create_dir_all(out.join("templates")).expect("mkdir out templates");
        fs::write(
            src.join("templates/_helpers.tpl"),
            "{{- define \"foo.a\" -}}A{{- end -}}\n",
        )
        .expect("write");

        let sync = sync_imported_include_helpers_from_source_chart(
            src.to_str().expect("src"),
            out.to_str().expect("out"),
            "global:\n  x: '{{ include \"foo.a\" . }}-{{ include \"foo.missing\" . }}'\n",
        )
        .expect("sync");
        assert_eq!(sync.added, vec!["foo.a".to_string()]);
        assert_eq!(sync.missing, vec!["foo.missing".to_string()]);
    }

    #[test]
    fn sync_imported_include_helpers_pulls_transitive_dependencies() {
        let td = TempDir::new().expect("tmp");
        let src = td.path().join("source-chart");
        let out = td.path().join("out-chart");
        fs::create_dir_all(src.join("templates")).expect("mkdir src templates");
        fs::create_dir_all(out.join("templates")).expect("mkdir out templates");
        fs::write(
            src.join("templates/_helpers.tpl"),
            r#"
{{- define "foo.a" -}}{{ include "foo.b" . }}{{- end -}}
{{- define "foo.b" -}}{{ .Values.cluster.name }}{{- end -}}
"#,
        )
        .expect("write");

        let sync = sync_imported_include_helpers_from_source_chart(
            src.to_str().expect("src"),
            out.to_str().expect("out"),
            "global:\n  x: '{{ include \"foo.a\" . }}'\n",
        )
        .expect("sync");
        assert_eq!(sync.added, vec!["foo.a".to_string(), "foo.b".to_string()]);
        assert!(sync.missing.is_empty());
    }

    #[test]
    fn ensure_values_examples_for_imported_helpers_adds_missing_paths() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("chart");
        fs::create_dir_all(out.join("templates")).expect("mkdir templates");
        fs::write(
            out.join("templates/imported-source-includes.tpl"),
            r#"
{{- define "foo.a" -}}
{{ .Values.cluster.name }}-{{ $.Values.serviceAccount.name }}
{{- end -}}
"#,
        )
        .expect("write tpl");
        fs::write(out.join("values.yaml"), "global:\n  env: dev\n").expect("write values");

        let added = ensure_values_examples_for_imported_helpers(out.to_str().expect("out"))
            .expect("ensure");
        assert_eq!(
            added,
            vec![
                "cluster.name".to_string(),
                "serviceAccount.name".to_string()
            ]
        );
        let values = fs::read_to_string(out.join("values.yaml")).expect("read values");
        assert!(values.contains("cluster:"));
        assert!(values.contains("serviceAccount:"));
        assert!(values.contains("name: <example>"));
    }

    #[test]
    fn normalize_imported_helper_block_rewrites_values_scope_only_in_actions() {
        let src = r#"
{{- define "foo.a" -}}
{{ .Values.cluster.name }}-{{ $.Values.serviceAccount.name }}
{{ printf "%s" ".Values.literal" }}
{{/* .Values.comment */}}
{{- if .Values.serviceAccount.create -}}ok{{- end -}}
{{- end -}}
"#;
        let normalized = normalize_imported_helper_block(src);
        assert!(normalized.contains("{{ $.Values.cluster.name }}"));
        assert!(normalized.contains("{{- if $.Values.serviceAccount.create -}}"));
        assert!(normalized.contains("{{ printf \"%s\" \".Values.literal\" }}"));
        assert!(normalized.contains("{{/* .Values.comment */}}"));
        assert!(!normalized.contains("{{ .Values.cluster.name }}"));
        assert!(normalized.contains("{{ $.Values.serviceAccount.name }}"));
    }

    #[test]
    fn rewrite_template_actions_handles_braces_inside_string_literals() {
        let src = r#"{{ "}}" }} include "foo.skip" . {{ .Values.name }}"#;
        let normalized = normalize_library_template_string(src);
        assert!(normalized.contains(r#"{{ "}}" }}"#));
        assert!(normalized.contains(r#"include "foo.skip" ."#));
        assert!(normalized.contains(r#"{{ $.Values.name }}"#));
    }
}
