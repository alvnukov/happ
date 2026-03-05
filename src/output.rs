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
    #[error("library chart: {0}")]
    Library(String),
}

pub fn values_yaml(values: &Value) -> Result<String, Error> {
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
    let text = serde_yaml::to_string(&Value::Mapping(ordered))?;
    Ok(text.trim_start_matches("---\n").to_string())
}

pub fn write_values(path: Option<&str>, values: &Value) -> Result<(), Error> {
    let body = values_yaml(values)?;
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
    write_values(
        Some(&Path::new(out_dir).join("values.yaml").to_string_lossy()),
        values,
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
        selected.insert(name.clone(), block.clone());
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
        )
        .expect("generate");
        assert!(out.join("Chart.yaml").exists());
        assert!(out.join("values.yaml").exists());
        assert!(out.join("templates/init-helm-apps-library.yaml").exists());
        assert!(out.join("charts/helm-apps/Chart.yaml").exists());
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
}
