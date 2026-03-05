use serde::Deserialize;
use serde_yaml::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::ImportArgs;
use crate::gotemplates::{
    contains_template_markup, escape_template_action, normalize_values_global_context,
    parse_template_tokens, GoTemplateToken,
};
use crate::templateanalyzer::collect_include_names_in_action;
use crate::templatepolicy::is_supported_include;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("no YAML files found at {0}")]
    NoYamlFiles(String),
    #[error("helm template failed: {0}")]
    Helm(String),
    #[error("unsupported source template includes: {0}")]
    UnsupportedTemplateIncludes(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderInvocation {
    program: String,
    args: Vec<String>,
}

pub fn load_documents_for_chart(args: &ImportArgs) -> Result<Vec<Value>, Error> {
    let rendered = render_chart(args, &args.path)?;
    let mut docs = parse_documents(&rendered)?;
    rehydrate_templated_extra_objects(args, &mut docs)?;
    Ok(docs)
}

pub fn load_documents_for_manifests(path: &str) -> Result<Vec<Value>, Error> {
    let files = collect_manifest_files(path)?;
    if files.is_empty() {
        return Err(Error::NoYamlFiles(path.to_string()));
    }
    let mut out = Vec::new();
    for file in files {
        let data = fs::read_to_string(&file)?;
        out.extend(parse_documents(&data)?);
    }
    Ok(flatten_k8s_lists(out))
}

pub fn parse_documents(stream: &str) -> Result<Vec<Value>, Error> {
    let mut docs = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(stream) {
        let v: Value = Value::deserialize(doc)?;
        if !v.is_null() {
            docs.push(v);
        }
    }
    Ok(flatten_k8s_lists(docs))
}

pub fn render_chart(args: &ImportArgs, chart_path: &str) -> Result<String, Error> {
    let mut last_error = String::new();
    for inv in render_invocations(args, chart_path) {
        let output = match Command::new(&inv.program).args(&inv.args).output() {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                last_error = format!("{} not found", inv.program);
                continue;
            }
            Err(e) => return Err(Error::Io(e)),
        };
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            last_error = if err.is_empty() {
                format!("{} exited with status {}", inv.program, output.status)
            } else {
                format!("{}: {err}", inv.program)
            };
            continue;
        }
        let rendered = String::from_utf8_lossy(&output.stdout).to_string();
        if let Some(path) = &args.write_rendered_output {
            fs::write(path, rendered.as_bytes())?;
        }
        return Ok(rendered);
    }
    Err(Error::Helm(if last_error.is_empty() {
        "no renderer available".to_string()
    } else {
        last_error
    }))
}

fn render_invocations(args: &ImportArgs, chart_path: &str) -> Vec<RenderInvocation> {
    let mut out = Vec::with_capacity(2);

    let mut werf = RenderInvocation {
        program: "werf".to_string(),
        args: vec![
            "render".to_string(),
            "--release".to_string(),
            args.release_name.clone(),
            chart_path.to_string(),
        ],
    };
    apply_render_flags(&mut werf.args, args);
    out.push(werf);

    let mut helm = RenderInvocation {
        program: "helm".to_string(),
        args: vec![
            "template".to_string(),
            args.release_name.clone(),
            chart_path.to_string(),
        ],
    };
    apply_render_flags(&mut helm.args, args);
    out.push(helm);

    out
}

fn apply_render_flags(cmd_args: &mut Vec<String>, args: &ImportArgs) {
    if let Some(ns) = &args.namespace {
        if !ns.trim().is_empty() {
            cmd_args.push("--namespace".to_string());
            cmd_args.push(ns.clone());
        }
    }
    for v in &args.values_files {
        cmd_args.push("--values".to_string());
        cmd_args.push(v.clone());
    }
    for v in &args.set_values {
        cmd_args.push("--set".to_string());
        cmd_args.push(v.clone());
    }
    for v in &args.set_string_values {
        cmd_args.push("--set-string".to_string());
        cmd_args.push(v.clone());
    }
    for v in &args.set_file_values {
        cmd_args.push("--set-file".to_string());
        cmd_args.push(v.clone());
    }
    for v in &args.set_json_values {
        cmd_args.push("--set-json".to_string());
        cmd_args.push(v.clone());
    }
    if let Some(kv) = &args.kube_version {
        if !kv.trim().is_empty() {
            cmd_args.push("--kube-version".to_string());
            cmd_args.push(kv.clone());
        }
    }
    for v in &args.api_versions {
        cmd_args.push("--api-versions".to_string());
        cmd_args.push(v.clone());
    }
    if args.include_crds {
        cmd_args.push("--include-crds".to_string());
    }
}

pub fn collect_manifest_files(path: &str) -> Result<Vec<PathBuf>, Error> {
    let p = Path::new(path);
    if p.is_file() {
        return Ok(vec![p.to_path_buf()]);
    }
    let mut out = Vec::new();
    walk_yaml_files(p, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_yaml_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            walk_yaml_files(&p, out)?;
            continue;
        }
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            let low = name.to_ascii_lowercase();
            if low.ends_with(".yaml") || low.ends_with(".yml") {
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
        let kind = doc
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if kind.is_empty() {
            continue;
        }
        let name = doc
            .get("metadata")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String("name".to_string())))
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let exact_idx = templates.iter().enumerate().find_map(|(i, t)| {
            if used[i] || t.kind != kind {
                return None;
            }
            if let (Some(tn), Some(dn)) = (t.name.as_ref(), name.as_ref()) {
                if tn == dn {
                    return Some(i);
                }
            }
            None
        });
        let fallback_idx = templates.iter().enumerate().find_map(|(i, t)| {
            if used[i] || t.kind != kind || !t.has_templated_name {
                return None;
            }
            Some(i)
        });
        let Some(idx) = exact_idx.or(fallback_idx) else {
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
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        return Ok(s);
    }
    Ok(fs::read_to_string(path)?)
}

pub fn validate_values_file(path: &str) -> Result<(), Error> {
    let src = fs::read_to_string(path)?;
    for doc in serde_yaml::Deserializer::from_str(&src) {
        let _: Value = Value::deserialize(doc)?;
    }
    Ok(())
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
    fn render_invocations_prefers_werf_then_helm() {
        let args = minimal_import_args();
        let inv = render_invocations(&args, "./chart");
        assert_eq!(inv.len(), 2);
        assert_eq!(inv[0].program, "werf");
        assert_eq!(inv[0].args[0], "render");
        assert_eq!(inv[1].program, "helm");
        assert_eq!(inv[1].args[0], "template");
    }

    #[test]
    fn render_invocations_apply_render_flags() {
        let mut args = minimal_import_args();
        args.namespace = Some("default".into());
        args.values_files = vec!["values.yaml".into()];
        args.set_values = vec!["a=b".into()];
        args.set_string_values = vec!["x=1".into()];
        args.set_file_values = vec!["k=path.txt".into()];
        args.set_json_values = vec!["obj={}".into()];
        args.kube_version = Some("1.29.0".into());
        args.api_versions = vec!["batch/v1".into()];
        args.include_crds = true;

        let inv = render_invocations(&args, "./chart");
        let helm = &inv[1].args;
        assert!(helm.windows(2).any(|w| w == ["--namespace", "default"]));
        assert!(helm.windows(2).any(|w| w == ["--values", "values.yaml"]));
        assert!(helm.windows(2).any(|w| w == ["--set", "a=b"]));
        assert!(helm.windows(2).any(|w| w == ["--set-string", "x=1"]));
        assert!(helm.windows(2).any(|w| w == ["--set-file", "k=path.txt"]));
        assert!(helm.windows(2).any(|w| w == ["--set-json", "obj={}"]));
        assert!(helm.windows(2).any(|w| w == ["--kube-version", "1.29.0"]));
        assert!(helm.windows(2).any(|w| w == ["--api-versions", "batch/v1"]));
        assert!(helm.contains(&"--include-crds".to_string()));
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
