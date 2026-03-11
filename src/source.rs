use serde::Deserialize;
use serde_yaml::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::chart_ir::{decode_ir_documents, encode_documents, ChartIr};
use crate::cli::ImportArgs;
use crate::gotemplates::{
    contains_template_markup, escape_template_action, normalize_values_global_context,
    parse_template_tokens, GoTemplateToken,
};
use crate::templateanalyzer::collect_include_names_in_action;
use crate::templatepolicy::is_supported_include;

const DEFAULT_MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_MAX_MANIFEST_FILE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_MANIFEST_FILES: usize = 50_000;
const DEFAULT_MAX_MANIFEST_WALK_DEPTH: usize = 128;
const DEFAULT_MAX_YAML_DOCS_PER_STREAM: usize = 100_000;
const DEFAULT_MAX_VALUES_FILE_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_MAX_CHART_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("no YAML files found at {0}")]
    NoYamlFiles(String),
    #[error("chart model build failed: {0}")]
    ChartModel(String),
    #[error("unsupported source template includes: {0}")]
    UnsupportedTemplateIncludes(String),
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error(transparent)]
    ChartIr(#[from] crate::chart_ir::Error),
}

pub fn load_documents_for_chart(args: &ImportArgs) -> Result<Vec<Value>, Error> {
    let ir = load_chart_ir_for_chart(args)?;
    decode_ir_documents(&ir).map_err(Error::from)
}

pub fn load_chart_ir_for_chart(args: &ImportArgs) -> Result<ChartIr, Error> {
    enforce_chart_source_safety(args)?;
    let mut ir =
        crate::go_compat::helm_ir_ffi::load_chart_ir_via_helm_goffi(args).map_err(|err| {
            let message = match err {
                crate::go_compat::helm_ir_ffi::HelmIrFfiError::Unavailable(reason)
                | crate::go_compat::helm_ir_ffi::HelmIrFfiError::Render(reason)
                | crate::go_compat::helm_ir_ffi::HelmIrFfiError::Decode(reason) => reason,
            };
            Error::ChartModel(augment_renderer_error_message(&message))
        })?;
    let mut docs = decode_ir_documents(&ir)?;
    rehydrate_templated_extra_objects(args, &mut docs)?;
    ir.documents = encode_documents(&docs);
    Ok(ir)
}

pub fn load_documents_for_manifests(path: &str) -> Result<Vec<Value>, Error> {
    let files = collect_manifest_files(path)?;
    if files.is_empty() {
        return Err(Error::NoYamlFiles(path.to_string()));
    }
    if files.len() > max_manifest_files() {
        return Err(Error::ResourceLimit(format!(
            "too many YAML files: {} (max {})",
            files.len(),
            max_manifest_files()
        )));
    }
    let mut out = Vec::new();
    for file in files {
        let data = read_text_file_with_limit(&file, max_manifest_file_bytes())?;
        let docs = parse_documents(&data)?;
        if out.len().saturating_add(docs.len()) > max_yaml_docs_per_stream() {
            return Err(Error::ResourceLimit(format!(
                "too many YAML documents while loading manifests (max {})",
                max_yaml_docs_per_stream()
            )));
        }
        out.extend(docs);
    }
    Ok(flatten_k8s_lists(out))
}

pub fn parse_documents(stream: &str) -> Result<Vec<Value>, Error> {
    let mut docs = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(stream) {
        let v: Value = Value::deserialize(doc)?;
        if !v.is_null() {
            if docs.len() >= max_yaml_docs_per_stream() {
                return Err(Error::ResourceLimit(format!(
                    "too many YAML documents in a single stream (max {})",
                    max_yaml_docs_per_stream()
                )));
            }
            docs.push(v);
        }
    }
    Ok(flatten_k8s_lists(docs))
}

pub fn render_chart(args: &ImportArgs, chart_path: &str) -> Result<String, Error> {
    let mut render_args = args.clone();
    render_args.path = chart_path.to_string();
    let ir = load_chart_ir_for_chart(&render_args)?;
    let docs = decode_ir_documents(&ir)?;
    let rendered = render_documents_yaml_stream(&docs)?;
    if let Some(path) = &args.write_rendered_output {
        fs::write(path, rendered.as_bytes())?;
    }
    Ok(rendered)
}

fn augment_renderer_error_message(err: &str) -> String {
    let trimmed = err.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let missing_paths = extract_missing_values_paths(trimmed);
    if missing_paths.is_empty() {
        return trimmed.to_string();
    }

    let mut out = String::from(trimmed);
    out.push_str("\n\nhapp hint: custom values are missing for these template paths:\n");
    for path in &missing_paths {
        out.push_str("- ");
        out.push_str(path);
        out.push('\n');
    }
    out.push_str(
        "Provide them via source chart values (--values / --set / --set-string), then retry.",
    );
    out
}

fn extract_missing_values_paths(err: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let bytes = err.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let candidate = err[start..j].trim();
        if let Some(path) = normalize_values_path_candidate(candidate) {
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
        i = j + 1;
    }
    out
}

fn normalize_values_path_candidate(candidate: &str) -> Option<String> {
    if let Some(rest) = candidate.strip_prefix("$.Values") {
        if rest.is_empty() || rest.starts_with('.') {
            return Some(format!("$.Values{rest}"));
        }
    }
    if let Some(rest) = candidate.strip_prefix(".Values") {
        if rest.is_empty() || rest.starts_with('.') {
            return Some(format!("$.Values{rest}"));
        }
    }
    None
}

fn render_documents_yaml_stream(docs: &[Value]) -> Result<String, Error> {
    let mut out = String::new();
    for (idx, doc) in docs.iter().enumerate() {
        if idx > 0 {
            out.push_str("---\n");
        }
        let mut body = serde_yaml::to_string(doc)?;
        if body.starts_with("---\n") {
            body = body.replacen("---\n", "", 1);
        }
        out.push_str(&body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

pub fn collect_manifest_files(path: &str) -> Result<Vec<PathBuf>, Error> {
    let p = Path::new(path);
    if p.is_file() {
        return Ok(vec![p.to_path_buf()]);
    }
    let mut out = Vec::new();
    walk_yaml_files(p, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn walk_yaml_files(path: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), Error> {
    if depth > max_manifest_walk_depth() {
        return Err(Error::ResourceLimit(format!(
            "manifest directory nesting is too deep (max depth {})",
            max_manifest_walk_depth()
        )));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_yaml_files(&p, out, depth + 1)?;
            continue;
        }
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            let low = name.to_ascii_lowercase();
            if low.ends_with(".yaml") || low.ends_with(".yml") {
                if out.len() >= max_manifest_files() {
                    return Err(Error::ResourceLimit(format!(
                        "too many YAML files discovered (max {})",
                        max_manifest_files()
                    )));
                }
                out.push(p);
            }
        }
    }
    Ok(())
}

fn flatten_k8s_lists(docs: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for doc in docs {
        if doc.get("kind").and_then(|k| k.as_str()) == Some("List") {
            if let Some(items) = doc.get("items").and_then(|v| v.as_sequence()) {
                for item in items {
                    if item.is_mapping() {
                        out.push(item.clone());
                    }
                }
            }
        } else if doc.is_mapping() {
            out.push(doc);
        }
    }
    out
}

#[derive(Debug, Clone)]
struct ExtraObjectTemplate {
    kind: String,
    name: Option<String>,
    has_templated_name: bool,
    value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtraObjectIdentity {
    kind: String,
    name: Option<String>,
}

fn extra_object_identity(doc: &Value) -> Option<ExtraObjectIdentity> {
    let kind = doc
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if kind.is_empty() {
        return None;
    }
    let name = doc
        .get("metadata")
        .and_then(Value::as_mapping)
        .and_then(|m| m.get(Value::String("name".to_string())))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Some(ExtraObjectIdentity { kind, name })
}

fn find_matching_template_index(
    templates: &[ExtraObjectTemplate],
    used: &[bool],
    identity: &ExtraObjectIdentity,
) -> Option<usize> {
    let exact = templates.iter().enumerate().find_map(|(idx, template)| {
        if used[idx] || template.kind != identity.kind {
            return None;
        }
        if let (Some(template_name), Some(doc_name)) =
            (template.name.as_ref(), identity.name.as_ref())
        {
            if template_name == doc_name {
                return Some(idx);
            }
        }
        None
    });
    if exact.is_some() {
        return exact;
    }

    templates.iter().enumerate().find_map(|(idx, template)| {
        if used[idx] || template.kind != identity.kind || !template.has_templated_name {
            return None;
        }
        Some(idx)
    })
}

fn rehydrate_templated_extra_objects(args: &ImportArgs, docs: &mut [Value]) -> Result<(), Error> {
    let templates = load_templated_extra_objects_from_values_files(&args.values_files);
    if templates.is_empty() || docs.is_empty() {
        return Ok(());
    }

    if unsupported_template_mode_is_error(args) {
        let unsupported = collect_unsupported_template_includes(&templates, args);
        if !unsupported.is_empty() {
            return Err(Error::UnsupportedTemplateIncludes(format!(
                "{}. Decide explicitly: allow with --allow-template-include <NAME|PREFIX*> (repeatable), or keep as literals with --unsupported-template-mode escape",
                unsupported.join(", ")
            )));
        }
    }

    let mut used = vec![false; templates.len()];
    for doc in docs {
        let Some(identity) = extra_object_identity(doc) else {
            continue;
        };
        let Some(idx) = find_matching_template_index(&templates, &used, &identity) else {
            continue;
        };
        let mut path = Vec::new();
        apply_templated_scalars(args, doc, &templates[idx].value, &mut path);
        used[idx] = true;
    }
    Ok(())
}

fn load_templated_extra_objects_from_values_files(
    values_files: &[String],
) -> Vec<ExtraObjectTemplate> {
    let mut out = Vec::new();
    for path in values_files {
        let content = match fs::read_to_string(path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let root: Value = match serde_yaml::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(extra_seq) = root.get("extraObjects").and_then(Value::as_sequence) else {
            continue;
        };
        for item in extra_seq {
            let Some(kind) = item.get("kind").and_then(Value::as_str) else {
                continue;
            };
            if !has_templated_scalar(item) {
                continue;
            }
            let name = item
                .get("metadata")
                .and_then(Value::as_mapping)
                .and_then(|m| m.get(Value::String("name".to_string())))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let has_templated_name = name.as_ref().is_some_and(|n| contains_template_markup(n));
            out.push(ExtraObjectTemplate {
                kind: kind.to_string(),
                name,
                has_templated_name,
                value: item.clone(),
            });
        }
    }
    out
}

fn has_templated_scalar(v: &Value) -> bool {
    match v {
        Value::String(s) => contains_template_markup(s),
        Value::Sequence(seq) => seq.iter().any(has_templated_scalar),
        Value::Mapping(map) => map.values().any(has_templated_scalar),
        _ => false,
    }
}

fn unsupported_template_mode_is_error(args: &ImportArgs) -> bool {
    !args
        .unsupported_template_mode
        .eq_ignore_ascii_case("escape")
}

fn should_skip_template_rehydrate(path: &[String]) -> bool {
    path.len() == 2 && path[0] == "metadata" && path[1] == "name"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateActionSupport {
    Portable,
    Unsupported,
}

fn normalize_template_string(src: &str, args: &ImportArgs) -> Option<String> {
    if !contains_template_markup(src) {
        return None;
    }
    let tokens = parse_template_tokens(src)?;
    let mut out = String::new();
    let mut has_action = false;
    for token in tokens {
        match token {
            GoTemplateToken::Literal(v) => out.push_str(&v),
            GoTemplateToken::Action(v) => {
                has_action = true;
                let action = normalize_values_global_context(&v);
                match classify_template_action(&action, args) {
                    TemplateActionSupport::Portable => out.push_str(&action),
                    TemplateActionSupport::Unsupported => {
                        out.push_str(&escape_template_action(&action))
                    }
                }
            }
        }
    }
    has_action.then_some(out)
}

fn classify_template_action(action: &str, args: &ImportArgs) -> TemplateActionSupport {
    for include_name in collect_include_names_in_action(action) {
        if !is_supported_include(&include_name, &args.allow_template_includes) {
            return TemplateActionSupport::Unsupported;
        }
    }
    TemplateActionSupport::Portable
}

fn collect_unsupported_template_includes(
    templates: &[ExtraObjectTemplate],
    args: &ImportArgs,
) -> Vec<String> {
    let mut out = BTreeSet::new();
    for t in templates {
        collect_unsupported_includes_from_value(&t.value, args, &mut out);
    }
    out.into_iter().collect()
}

fn collect_unsupported_includes_from_value(
    v: &Value,
    args: &ImportArgs,
    out: &mut BTreeSet<String>,
) {
    match v {
        Value::String(s) => collect_unsupported_includes_from_string(s, args, out),
        Value::Sequence(seq) => {
            for item in seq {
                collect_unsupported_includes_from_value(item, args, out);
            }
        }
        Value::Mapping(map) => {
            for item in map.values() {
                collect_unsupported_includes_from_value(item, args, out);
            }
        }
        _ => {}
    }
}

fn collect_unsupported_includes_from_string(
    s: &str,
    args: &ImportArgs,
    out: &mut BTreeSet<String>,
) {
    if !contains_template_markup(s) {
        return;
    }
    let Some(tokens) = parse_template_tokens(s) else {
        return;
    };
    for token in tokens {
        let GoTemplateToken::Action(action) = token else {
            continue;
        };
        for include_name in collect_include_names_in_action(&action) {
            if !is_supported_include(&include_name, &args.allow_template_includes) {
                let _ = out.insert(include_name);
            }
        }
    }
}

fn apply_templated_scalars(
    args: &ImportArgs,
    dst: &mut Value,
    src: &Value,
    path: &mut Vec<String>,
) {
    match (dst, src) {
        (Value::String(dst_s), Value::String(src_s)) => {
            if contains_template_markup(src_s) && !should_skip_template_rehydrate(path) {
                if let Some(normalized) = normalize_template_string(src_s, args) {
                    *dst_s = normalized;
                }
            }
        }
        (Value::Mapping(dst_m), Value::Mapping(src_m)) => {
            for (key, src_val) in src_m {
                let Some(key_s) = key.as_str() else {
                    continue;
                };
                let key_v = Value::String(key_s.to_string());
                let Some(dst_val) = dst_m.get_mut(&key_v) else {
                    continue;
                };
                path.push(key_s.to_string());
                apply_templated_scalars(args, dst_val, src_val, path);
                let _ = path.pop();
            }
        }
        (Value::Sequence(dst_seq), Value::Sequence(src_seq)) => {
            for (idx, (dst_val, src_val)) in dst_seq.iter_mut().zip(src_seq.iter()).enumerate() {
                path.push(format!("[{idx}]"));
                apply_templated_scalars(args, dst_val, src_val, path);
                let _ = path.pop();
            }
        }
        _ => {}
    }
}

pub fn read_input(path: &str) -> Result<String, Error> {
    if path == "-" {
        return read_stdin_with_limit(max_input_bytes());
    }
    read_text_file_with_limit(Path::new(path), max_input_bytes())
}

pub fn validate_values_file(path: &str) -> Result<(), Error> {
    let src = read_text_file_with_limit(Path::new(path), max_values_file_bytes())?;
    for doc in serde_yaml::Deserializer::from_str(&src) {
        let _: Value = Value::deserialize(doc)?;
    }
    Ok(())
}

fn enforce_chart_source_safety(args: &ImportArgs) -> Result<(), Error> {
    let chart_path = Path::new(&args.path);
    if chart_path.is_file() {
        let meta = fs::metadata(chart_path)
            .map_err(|e| Error::ChartModel(format!("chart file '{}': {e}", args.path)))?;
        let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        if len > max_chart_archive_bytes() {
            return Err(Error::ChartModel(format!(
                "chart archive '{}' is too large: {} bytes (max {}). Extract it manually or raise HAPP_MAX_CHART_ARCHIVE_BYTES.",
                args.path,
                len,
                max_chart_archive_bytes()
            )));
        }
    }
    if !chart_path.is_dir() && !chart_path.is_file() {
        return Err(Error::ChartModel(format!(
            "chart path '{}' does not exist or is not a regular file/directory",
            args.path
        )));
    }
    let mut total_values_bytes = 0usize;
    for values_path in &args.values_files {
        let meta = fs::metadata(values_path)
            .map_err(|e| Error::ChartModel(format!("values file '{}': {e}", values_path)))?;
        let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        if len > max_values_file_bytes() {
            return Err(Error::ChartModel(format!(
                "values file '{}' is too large: {} bytes (max {})",
                values_path,
                len,
                max_values_file_bytes()
            )));
        }
        total_values_bytes = total_values_bytes.saturating_add(len);
        if total_values_bytes > max_values_file_bytes().saturating_mul(8) {
            return Err(Error::ChartModel(format!(
                "total values files size is too large: {} bytes (max {})",
                total_values_bytes,
                max_values_file_bytes().saturating_mul(8)
            )));
        }
    }
    Ok(())
}

fn read_stdin_with_limit(limit: usize) -> Result<String, Error> {
    let mut input = Vec::new();
    let mut buf = [0u8; 8192];
    let mut stdin = io::stdin();
    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if input.len().saturating_add(n) > limit {
            return Err(Error::ResourceLimit(format!(
                "stdin is too large (max {} bytes)",
                limit
            )));
        }
        input.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(input).map_err(|e| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("input is not valid UTF-8: {e}"),
        ))
    })
}

fn read_text_file_with_limit(path: &Path, limit: usize) -> Result<String, Error> {
    let meta = fs::metadata(path)?;
    let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if len > limit {
        return Err(Error::ResourceLimit(format!(
            "file '{}' is too large: {} bytes (max {})",
            path.display(),
            len,
            limit
        )));
    }
    fs::read_to_string(path).map_err(Error::from)
}

fn max_input_bytes() -> usize {
    env_usize_or("HAPP_MAX_INPUT_BYTES", DEFAULT_MAX_INPUT_BYTES)
}

fn max_manifest_file_bytes() -> usize {
    env_usize_or(
        "HAPP_MAX_MANIFEST_FILE_BYTES",
        DEFAULT_MAX_MANIFEST_FILE_BYTES,
    )
}

fn max_manifest_files() -> usize {
    env_usize_or("HAPP_MAX_MANIFEST_FILES", DEFAULT_MAX_MANIFEST_FILES)
}

fn max_manifest_walk_depth() -> usize {
    env_usize_or(
        "HAPP_MAX_MANIFEST_WALK_DEPTH",
        DEFAULT_MAX_MANIFEST_WALK_DEPTH,
    )
}

fn max_yaml_docs_per_stream() -> usize {
    env_usize_or(
        "HAPP_MAX_YAML_DOCS_PER_STREAM",
        DEFAULT_MAX_YAML_DOCS_PER_STREAM,
    )
}

fn max_values_file_bytes() -> usize {
    env_usize_or("HAPP_MAX_VALUES_FILE_BYTES", DEFAULT_MAX_VALUES_FILE_BYTES)
}

fn max_chart_archive_bytes() -> usize {
    env_usize_or(
        "HAPP_MAX_CHART_ARCHIVE_BYTES",
        DEFAULT_MAX_CHART_ARCHIVE_BYTES,
    )
}

fn env_usize_or(name: &str, default: usize) -> usize {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ImportArgs;
    use tempfile::TempDir;

    #[test]
    fn parse_documents_flattens_k8s_list() {
        let src = r#"
apiVersion: v1
kind: List
items:
  - apiVersion: v1
    kind: ConfigMap
    metadata:
      name: a
"#;
        let docs = parse_documents(src).expect("parse");
        assert_eq!(docs.len(), 1);
        assert_eq!(
            docs[0].get("kind").and_then(|v| v.as_str()),
            Some("ConfigMap")
        );
    }

    #[test]
    fn collect_manifest_files_walks_yaml_only() {
        let td = TempDir::new().expect("tmp");
        fs::write(td.path().join("a.yaml"), "a: 1").expect("w");
        fs::write(td.path().join("b.txt"), "x").expect("w");
        fs::create_dir_all(td.path().join("sub")).expect("mk");
        fs::write(td.path().join("sub/c.yml"), "c: 1").expect("w");

        let files = collect_manifest_files(td.path().to_str().expect("path")).expect("collect");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn rehydrates_templated_extra_object_scalars_from_values_file() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values-extra.yaml");
        fs::write(
            &values_path,
            r#"
extraObjects:
  - apiVersion: v1
    kind: Secret
    metadata:
      name: admin-secret
    stringData:
      username: admin
      password: '{{ .Values.cluster.security.config.admin_password }}'
  - apiVersion: batch/v1
    kind: Job
    metadata:
      name: '{{ include "opensearch-cluster.cluster-name" . }}-create-snapshot-policy'
    spec:
      template:
        spec:
          serviceAccountName: '{{ include "opensearch-cluster.serviceAccountName" . }}'
          volumes:
            - name: admin-credentials
              secret:
                secretName: "{{ .Release.Name }}-admin-password"
"#,
        )
        .expect("write values");

        let mut args = minimal_import_args();
        args.values_files = vec![values_path.to_string_lossy().to_string()];
        args.unsupported_template_mode = "escape".to_string();

        let mut docs = vec![
            serde_yaml::from_str::<Value>(
                r#"
apiVersion: v1
kind: Secret
metadata:
  name: admin-secret
stringData:
  username: admin
  password: ''
"#,
            )
            .expect("secret doc"),
            serde_yaml::from_str::<Value>(
                r#"
apiVersion: batch/v1
kind: Job
metadata:
  name: opensearch-cluster-release-create-snapshot-policy
spec:
  template:
    spec:
      serviceAccountName: opensearch-cluster-release
      volumes:
        - name: admin-credentials
          secret:
            secretName: opensearch-cluster-release-admin-password
"#,
            )
            .expect("job doc"),
        ];

        rehydrate_templated_extra_objects(&args, &mut docs).expect("rehydrate");

        let secret_password = docs[0]
            .get("stringData")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("password".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            secret_password,
            "{{ $.Values.cluster.security.config.admin_password }}"
        );

        let job_secret_name = docs[1]
            .get("spec")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("template".to_string())))
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("spec".to_string())))
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("volumes".to_string())))
            .and_then(Value::as_sequence)
            .and_then(|seq| seq.first())
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("secret".to_string())))
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("secretName".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(job_secret_name, "{{ .Release.Name }}-admin-password");

        let job_service_account = docs[1]
            .get("spec")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("template".to_string())))
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("spec".to_string())))
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("serviceAccountName".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            job_service_account,
            "{{ \"{{\" }} include \"opensearch-cluster.serviceAccountName\" . {{ \"}}\" }}"
        );

        let job_name = docs[1]
            .get("metadata")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("name".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            job_name,
            "opensearch-cluster-release-create-snapshot-policy"
        );
    }

    #[test]
    fn normalize_template_string_keeps_portable_part_for_mixed_templates() {
        let args = minimal_import_args();
        let source =
            r#"{{ include "opensearch-cluster.cluster-name" . }}__happ_sep__{{ .Release.Name }}"#;
        let normalized = normalize_template_string(source, &args).expect("normalized");
        assert_eq!(
            normalized,
            "{{ \"{{\" }} include \"opensearch-cluster.cluster-name\" . {{ \"}}\" }}__happ_sep__{{ .Release.Name }}"
        );
    }

    #[test]
    fn rehydrate_prefers_exact_name_match_over_templated_name_fallback() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values-extra.yaml");
        fs::write(
            &values_path,
            r#"
extraObjects:
  - apiVersion: v1
    kind: ConfigMap
    metadata:
      name: '{{ include "x.cmName" . }}'
    data:
      mode: '{{ .Values.global.fromFallback }}'
  - apiVersion: v1
    kind: ConfigMap
    metadata:
      name: cm-prod
    data:
      mode: '{{ .Values.global.fromExact }}'
"#,
        )
        .expect("write values");

        let mut args = minimal_import_args();
        args.values_files = vec![values_path.to_string_lossy().to_string()];
        args.unsupported_template_mode = "escape".to_string();

        let mut docs = vec![serde_yaml::from_str::<Value>(
            r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm-prod
data:
  mode: rendered
"#,
        )
        .expect("doc")];

        rehydrate_templated_extra_objects(&args, &mut docs).expect("rehydrate");

        let mode = docs[0]
            .get("data")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("mode".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(mode, "{{ $.Values.global.fromExact }}");
    }

    #[test]
    fn rehydrate_uses_templated_name_fallback_when_exact_name_absent() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values-extra.yaml");
        fs::write(
            &values_path,
            r#"
extraObjects:
  - apiVersion: v1
    kind: ConfigMap
    metadata:
      name: '{{ include "x.cmName" . }}'
    data:
      mode: '{{ .Values.global.fromFallback }}'
"#,
        )
        .expect("write values");

        let mut args = minimal_import_args();
        args.values_files = vec![values_path.to_string_lossy().to_string()];
        args.unsupported_template_mode = "escape".to_string();

        let mut docs = vec![serde_yaml::from_str::<Value>(
            r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm-stage
data:
  mode: rendered
"#,
        )
        .expect("doc")];

        rehydrate_templated_extra_objects(&args, &mut docs).expect("rehydrate");

        let mode = docs[0]
            .get("data")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("mode".to_string())))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(mode, "{{ $.Values.global.fromFallback }}");
    }

    #[test]
    fn normalize_template_string_uses_global_values_context() {
        let args = minimal_import_args();
        let source = r#"{{ .Values.cluster.security.config.s3_access_key }}:{{ $.Values.ok }}"#;
        let normalized = normalize_template_string(source, &args).expect("normalized");
        assert_eq!(
            normalized,
            "{{ $.Values.cluster.security.config.s3_access_key }}:{{ $.Values.ok }}"
        );
    }

    #[test]
    fn normalize_template_string_keeps_allowed_extra_include_templates() {
        let mut args = minimal_import_args();
        args.allow_template_includes = vec!["opensearch-cluster.*".to_string()];
        let source = r#"{{ include "opensearch-cluster.cluster-name" . }}-{{ .Release.Name }}"#;
        let normalized = normalize_template_string(source, &args).expect("normalized");
        assert_eq!(normalized, source);
    }

    #[test]
    fn normalize_template_string_escapes_all_unsupported_actions() {
        let args = minimal_import_args();
        let source = r#"{{ include "opensearch-cluster.cluster-name" . }}{{ include "x.y" . }}"#;
        let normalized = normalize_template_string(source, &args).expect("normalized");
        assert_eq!(
            normalized,
            "{{ \"{{\" }} include \"opensearch-cluster.cluster-name\" . {{ \"}}\" }}{{ \"{{\" }} include \"x.y\" . {{ \"}}\" }}"
        );
    }

    #[test]
    fn normalize_template_string_converts_values_inside_escaped_actions() {
        let args = minimal_import_args();
        let source = r#"{{ include "x.y" . }}={{ .Values.cluster.security.config.s3_access_key }}"#;
        let normalized = normalize_template_string(source, &args).expect("normalized");
        assert_eq!(
            normalized,
            "{{ \"{{\" }} include \"x.y\" . {{ \"}}\" }}={{ $.Values.cluster.security.config.s3_access_key }}"
        );
    }

    #[test]
    fn rehydrate_errors_on_unsupported_includes_by_default() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values-extra.yaml");
        fs::write(
            &values_path,
            r#"
extraObjects:
  - apiVersion: batch/v1
    kind: Job
    metadata:
      name: job-a
    spec:
      template:
        spec:
          serviceAccountName: '{{ include "opensearch-cluster.serviceAccountName" . }}'
"#,
        )
        .expect("write values");

        let mut args = minimal_import_args();
        args.values_files = vec![values_path.to_string_lossy().to_string()];

        let mut docs = vec![serde_yaml::from_str::<Value>(
            r#"
apiVersion: batch/v1
kind: Job
metadata:
  name: job-a
spec:
  template:
    spec:
      serviceAccountName: default
"#,
        )
        .expect("job doc")];

        let err = rehydrate_templated_extra_objects(&args, &mut docs).expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("opensearch-cluster.serviceAccountName"));
        assert!(msg.contains("--allow-template-include"));
        assert!(msg.contains("--unsupported-template-mode escape"));
    }

    #[test]
    fn render_documents_yaml_stream_emits_multi_doc_yaml() {
        let docs = vec![
            serde_yaml::from_str::<Value>(
                r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: a
"#,
            )
            .expect("doc1"),
            serde_yaml::from_str::<Value>(
                r#"
apiVersion: v1
kind: Secret
metadata:
  name: b
"#,
            )
            .expect("doc2"),
        ];
        let rendered = render_documents_yaml_stream(&docs).expect("render");
        assert!(rendered.contains("kind: ConfigMap"));
        assert!(rendered.contains("---\n"));
        assert!(rendered.contains("kind: Secret"));
    }

    #[test]
    fn validate_values_file_detects_invalid_yaml() {
        let td = TempDir::new().expect("tmp");
        let p = td.path().join("values.yaml");
        fs::write(&p, "global:\n  env: [dev\n").expect("write");
        let err = validate_values_file(p.to_str().expect("path")).expect_err("must fail");
        assert!(matches!(err, Error::Yaml(_)));
    }

    #[test]
    fn extract_include_names_parses_quoted_calls_only() {
        let names = collect_include_names_in_action(
            r#"{{ include "a.b" . }} {{ include 'x.y' . }} {{ myinclude "z" . }} {{ include not_quoted . }}"#,
        );
        assert_eq!(names, vec!["a.b", "x.y"]);
    }

    #[test]
    fn is_user_allowed_include_supports_exact_and_prefix_patterns() {
        let patterns = vec![
            " opensearch-cluster.* ".to_string(),
            "custom.helper".to_string(),
        ];
        assert!(crate::templatepolicy::is_user_allowed_include(
            "opensearch-cluster.cluster-name",
            &patterns,
        ));
        assert!(crate::templatepolicy::is_user_allowed_include(
            "custom.helper",
            &patterns
        ));
        assert!(!crate::templatepolicy::is_user_allowed_include(
            "custom.helper.v2",
            &patterns
        ));
    }

    #[test]
    fn normalize_template_string_returns_none_for_unbalanced_actions() {
        let args = minimal_import_args();
        let src = r#"{{ include "x.y" . "#;
        assert!(normalize_template_string(src, &args).is_none());
    }

    #[test]
    fn collect_unsupported_template_includes_dedupes_nested_values() {
        let args = minimal_import_args();
        let templates = vec![ExtraObjectTemplate {
            kind: "Secret".to_string(),
            name: Some("s1".to_string()),
            has_templated_name: false,
            value: serde_yaml::from_str::<Value>(
                r#"
metadata:
  labels:
    a: '{{ include "foo.a" . }}'
spec:
  arr:
    - '{{ include "foo.a" . }}'
    - '{{ include "foo.b" . }}'
  keep: '{{ include "apps.render" . }}'
"#,
            )
            .expect("template"),
        }];
        let unsupported = collect_unsupported_template_includes(&templates, &args);
        assert_eq!(unsupported, vec!["foo.a", "foo.b"]);
    }

    #[test]
    fn apply_templated_scalars_keeps_metadata_name_and_updates_other_scalars() {
        let args = minimal_import_args();
        let mut dst = serde_yaml::from_str::<Value>(
            r#"
apiVersion: v1
kind: Secret
metadata:
  name: rendered-secret
stringData:
  password: ""
"#,
        )
        .expect("dst");
        let src = serde_yaml::from_str::<Value>(
            r#"
apiVersion: v1
kind: Secret
metadata:
  name: '{{ include "foo.secret.name" . }}'
stringData:
  password: "{{ .Values.global.adminPassword }}"
"#,
        )
        .expect("src");
        apply_templated_scalars(&args, &mut dst, &src, &mut Vec::new());

        let name = dst
            .get("metadata")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("name".to_string())))
            .and_then(Value::as_str)
            .expect("name");
        assert_eq!(name, "rendered-secret");

        let password = dst
            .get("stringData")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("password".to_string())))
            .and_then(Value::as_str)
            .expect("password");
        assert_eq!(password, "{{ $.Values.global.adminPassword }}");
    }

    #[test]
    fn unsupported_template_mode_is_error_is_case_insensitive() {
        let mut args = minimal_import_args();
        args.unsupported_template_mode = "EsCaPe".to_string();
        assert!(!unsupported_template_mode_is_error(&args));
        args.unsupported_template_mode = "error".to_string();
        assert!(unsupported_template_mode_is_error(&args));
    }

    #[test]
    fn extract_missing_values_paths_collects_unique_values_refs() {
        let err = r#"template: gotpl:1: at <$.Values.cluster.security.config.admin_password>: nil pointer
template: gotpl:2: at <.Values.serviceAccount.name>: nil pointer
template: gotpl:3: at <$.Values.cluster.security.config.admin_password>: nil pointer"#;
        assert_eq!(
            extract_missing_values_paths(err),
            vec![
                "$.Values.cluster.security.config.admin_password".to_string(),
                "$.Values.serviceAccount.name".to_string(),
            ]
        );
    }

    #[test]
    fn augment_renderer_error_message_appends_user_hint_for_missing_values() {
        let err = r#"template: gotpl:1: executing "x" at <$.Values.cluster.security.config.admin_password>: nil pointer"#;
        let augmented = augment_renderer_error_message(err);
        assert!(augmented.contains("happ hint: custom values are missing"));
        assert!(augmented.contains("$.Values.cluster.security.config.admin_password"));
        assert!(augmented.contains("--values / --set / --set-string"));

        let plain = augment_renderer_error_message("renderer: random failure");
        assert_eq!(plain, "renderer: random failure");
    }

    #[test]
    fn enforce_chart_source_safety_allows_regular_chart_archive_file() {
        let td = TempDir::new().expect("tmp");
        let chart = td.path().join("chart.tgz");
        fs::write(&chart, b"not-real-archive").expect("write");

        let mut args = minimal_import_args();
        args.path = chart.to_string_lossy().to_string();
        enforce_chart_source_safety(&args).expect("file path should be allowed");
    }

    #[test]
    fn enforce_chart_source_safety_rejects_missing_path() {
        let mut args = minimal_import_args();
        args.path = "/definitely/missing/chart".to_string();
        let err = enforce_chart_source_safety(&args).expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("does not exist"));
    }

    fn minimal_import_args() -> ImportArgs {
        ImportArgs {
            path: "./chart".into(),
            env: "dev".into(),
            group_name: "apps-k8s-manifests".into(),
            group_type: "apps-k8s-manifests".into(),
            min_include_bytes: 24,
            include_status: false,
            output: None,
            out_chart_dir: None,
            chart_name: None,
            library_chart_path: None,
            import_strategy: "raw".into(),
            allow_template_includes: Vec::new(),
            unsupported_template_mode: "error".into(),
            verify_equivalence: false,
            release_name: "inspect".into(),
            namespace: None,
            values_files: Vec::new(),
            set_values: Vec::new(),
            set_string_values: Vec::new(),
            set_file_values: Vec::new(),
            set_json_values: Vec::new(),
            kube_version: None,
            api_versions: Vec::new(),
            include_crds: false,
            write_rendered_output: None,
        }
    }
}
