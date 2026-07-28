use serde_json::{Map as JsonMap, Value as JsonValue};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LoadedLibraryValues {
    pub chart_root: PathBuf,
    pub values: Value,
    pub selected_env: Option<String>,
}

pub fn load_library_chart_values_for_export(
    chart_path: &str,
    env: Option<&str>,
) -> Result<LoadedLibraryValues, String> {
    let chart_root = find_chart_root_from_path(Path::new(chart_path))
        .ok_or_else(|| format!("chart root not found for '{}'", chart_path))?;
    let values_path = find_primary_values_file(&chart_root)
        .ok_or_else(|| format!("values.yaml not found in '{}'", chart_root.display()))?;
    let values_text = fs::read_to_string(&values_path)
        .map_err(|err| format!("read values file '{}': {err}", values_path.display()))?;
    let root_map = parse_yaml_map_to_json_map(&values_text)?;
    let with_files =
        expand_values_with_file_includes(&root_map, values_path.parent(), &HashMap::new())?;
    let expanded = expand_includes_in_values(&with_files)?;
    let expanded_root = JsonValue::Object(expanded);

    let env_discovery = discover_environments(&expanded_root);
    let existing_global_env = expanded_root
        .as_object()
        .and_then(|root| root.get("global"))
        .and_then(as_obj)
        .and_then(|global| global.get("env"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let has_env_context = existing_global_env.is_some()
        || !env_discovery.literals.is_empty()
        || !env_discovery.regexes.is_empty();
    let selected_env = if let Some(explicit) = env.map(str::trim).filter(|value| !value.is_empty())
    {
        Some(explicit.to_string())
    } else if has_env_context {
        Some(detect_default_env(&expanded_root, &env_discovery))
    } else {
        None
    };

    let mut materialized = if let Some(selected_env) = selected_env.as_deref() {
        resolve_env_maps(&expanded_root, selected_env)
    } else {
        expanded_root
    };
    if let Some(selected_env) = selected_env.as_deref() {
        ensure_global_env_literal(&mut materialized, selected_env);
    }
    strip_internal_include_fields(&mut materialized);
    expand_value_references(&mut materialized)?;

    let values = json_value_to_yaml_value(&materialized)?;
    Ok(LoadedLibraryValues {
        chart_root,
        values,
        selected_env,
    })
}

fn parse_yaml_map_to_json_map(text: &str) -> Result<JsonMap<String, JsonValue>, String> {
    let yaml: Value = serde_yaml::from_str(text).map_err(|err| err.to_string())?;
    let root_json: JsonValue = serde_json::to_value(yaml).map_err(|err| err.to_string())?;
    as_obj(&root_json)
        .cloned()
        .ok_or_else(|| "values document must be a YAML map".to_string())
}

fn find_chart_root_from_path(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        if current.join("Chart.yaml").exists() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn find_primary_values_file(chart_root: &Path) -> Option<PathBuf> {
    let candidates = [
        chart_root.join("values.yaml"),
        chart_root.join("values.yml"),
    ];
    candidates.into_iter().find(|candidate| candidate.exists())
}

fn expand_values_with_file_includes(
    values: &JsonMap<String, JsonValue>,
    include_base_dir: Option<&Path>,
    overrides: &HashMap<PathBuf, String>,
) -> Result<JsonMap<String, JsonValue>, String> {
    let mut injected_includes: JsonMap<String, JsonValue> = JsonMap::new();
    let mut file_stack: HashSet<PathBuf> = HashSet::new();
    let processed = process_file_include_node(
        &JsonValue::Object(values.clone()),
        include_base_dir,
        &[],
        &mut injected_includes,
        overrides,
        &mut file_stack,
    )?;
    let mut root = as_obj(&processed)
        .cloned()
        .ok_or_else(|| "expanded values must stay a YAML map".to_string())?;
    ensure_global_includes_map(&mut root);
    if !injected_includes.is_empty() {
        let global = root
            .entry("global".to_string())
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        if let Some(global_map) = as_obj(global).cloned() {
            let mut global_map_mut = global_map;
            let includes = global_map_mut
                .entry("_includes".to_string())
                .or_insert_with(|| JsonValue::Object(JsonMap::new()));
            if let Some(includes_map) = as_obj(includes).cloned() {
                let mut includes_map_mut = includes_map;
                for (name, payload) in injected_includes {
                    includes_map_mut.insert(name, payload);
                }
                global_map_mut.insert("_includes".to_string(), JsonValue::Object(includes_map_mut));
            }
            root.insert("global".to_string(), JsonValue::Object(global_map_mut));
        }
    }
    Ok(root)
}

fn process_file_include_node(
    node: &JsonValue,
    include_base_dir: Option<&Path>,
    path_segments: &[String],
    injected_includes: &mut JsonMap<String, JsonValue>,
    overrides: &HashMap<PathBuf, String>,
    file_stack: &mut HashSet<PathBuf>,
) -> Result<JsonValue, String> {
    match node {
        JsonValue::Array(items) => Ok(JsonValue::Array(items.clone())),
        JsonValue::Object(map) => {
            let mut current = map.clone();

            let include_from_file = current
                .get("_include_from_file")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            if let Some(raw_path) = include_from_file {
                current.remove("_include_from_file");
                let loaded =
                    load_yaml_map_from_file(&raw_path, include_base_dir, overrides, file_stack)
                        .ok();
                if let Some((_loaded_path, loaded_map)) = loaded.flatten() {
                    let loaded_processed = process_file_include_node(
                        &JsonValue::Object(loaded_map),
                        include_base_dir,
                        path_segments,
                        injected_includes,
                        overrides,
                        file_stack,
                    )?;
                    let mut include_payload =
                        as_obj(&loaded_processed).cloned().unwrap_or_default();
                    if is_direct_global_includes_path(path_segments) {
                        include_payload = normalize_global_includes_payload(&include_payload);
                    }
                    current = merge_maps(&include_payload, &current);
                }
            } else {
                current.remove("_include_from_file");
            }

            if current.contains_key("_include_files") {
                let file_refs = normalize_include_files(current.get("_include_files"));
                let mut include_names: Vec<String> = Vec::new();
                for raw_path_value in file_refs {
                    let raw_path = raw_path_value.trim();
                    let include_name = include_name_from_path(raw_path);
                    let loaded =
                        load_yaml_map_from_file(raw_path, include_base_dir, overrides, file_stack)
                            .ok();
                    if let Some((_loaded_path, loaded_map)) = loaded.flatten() {
                        let loaded_processed = process_file_include_node(
                            &JsonValue::Object(loaded_map),
                            include_base_dir,
                            &[],
                            injected_includes,
                            overrides,
                            file_stack,
                        )?;
                        if let Some(processed_map) = as_obj(&loaded_processed).cloned() {
                            injected_includes
                                .insert(include_name.clone(), JsonValue::Object(processed_map));
                            include_names.push(include_name);
                        }
                    }
                }
                let mut merged_include = include_names;
                merged_include.extend(normalize_include(current.get("_include")));
                if !merged_include.is_empty() {
                    current.insert(
                        "_include".to_string(),
                        JsonValue::Array(
                            merged_include
                                .into_iter()
                                .map(JsonValue::String)
                                .collect::<Vec<JsonValue>>(),
                        ),
                    );
                }
                current.remove("_include_files");
            }

            let mut out = JsonMap::new();
            for (key, value) in current {
                if let JsonValue::Object(_) = value {
                    let mut next_path = path_segments.to_vec();
                    next_path.push(key.clone());
                    out.insert(
                        key,
                        process_file_include_node(
                            &value,
                            include_base_dir,
                            &next_path,
                            injected_includes,
                            overrides,
                            file_stack,
                        )?,
                    );
                    continue;
                }
                out.insert(key, value);
            }
            Ok(JsonValue::Object(out))
        }
        _ => Ok(node.clone()),
    }
}

fn expand_includes_in_values(
    root: &JsonMap<String, JsonValue>,
) -> Result<JsonMap<String, JsonValue>, String> {
    let includes_map = root
        .get("global")
        .and_then(as_obj)
        .and_then(|global| global.get("_includes"))
        .and_then(as_obj)
        .cloned()
        .unwrap_or_default();
    let mut cache: HashMap<String, JsonMap<String, JsonValue>> = HashMap::new();
    let expanded = expand_node(&JsonValue::Object(root.clone()), &includes_map, &mut cache)?;
    as_obj(&expanded)
        .cloned()
        .ok_or_else(|| "expanded values must stay map".to_string())
}

fn expand_node(
    node: &JsonValue,
    includes_map: &JsonMap<String, JsonValue>,
    cache: &mut HashMap<String, JsonMap<String, JsonValue>>,
) -> Result<JsonValue, String> {
    match node {
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(expand_node(item, includes_map, cache)?);
            }
            Ok(JsonValue::Array(out))
        }
        JsonValue::Object(map) => {
            let mut current = map.clone();
            if current.contains_key("_include") {
                let mut merged = JsonMap::new();
                for include_name in normalize_include(current.get("_include")) {
                    if let Ok(profile) =
                        resolve_profile(&include_name, includes_map, cache, &mut Vec::new())
                    {
                        merged = merge_maps(&merged, &profile);
                    }
                }
                current = merge_maps(&merged, &current);
                current.remove("_include");
            }

            let mut out = JsonMap::new();
            for (key, value) in current {
                if key == "_includes" {
                    out.insert(key, value);
                } else {
                    out.insert(key, expand_node(&value, includes_map, cache)?);
                }
            }
            Ok(JsonValue::Object(out))
        }
        _ => Ok(node.clone()),
    }
}

fn resolve_profile(
    name: &str,
    includes_map: &JsonMap<String, JsonValue>,
    cache: &mut HashMap<String, JsonMap<String, JsonValue>>,
    stack: &mut Vec<String>,
) -> Result<JsonMap<String, JsonValue>, String> {
    if let Some(cached) = cache.get(name) {
        return Ok(cached.clone());
    }
    if stack.iter().any(|current| current == name) {
        let mut cycle = stack.clone();
        cycle.push(name.to_string());
        return Err(format!("include cycle detected: {}", cycle.join(" -> ")));
    }
    let Some(profile) = includes_map.get(name).and_then(as_obj) else {
        return Ok(JsonMap::new());
    };

    stack.push(name.to_string());
    let mut merged = JsonMap::new();
    for child in normalize_include(profile.get("_include")) {
        if let Ok(child_map) = resolve_profile(&child, includes_map, cache, stack) {
            merged = merge_maps(&merged, &child_map);
        }
    }
    stack.pop();

    merged = merge_maps(&merged, profile);
    merged.remove("_include");
    cache.insert(name.to_string(), merged.clone());
    Ok(merged)
}

fn normalize_include(value: Option<&JsonValue>) -> Vec<String> {
    match value {
        Some(JsonValue::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![trimmed.to_string()]
            }
        }
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let raw = item.as_str()?;
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_include_files(value: Option<&JsonValue>) -> Vec<String> {
    normalize_include(value)
}

fn merge_maps(
    base: &JsonMap<String, JsonValue>,
    incoming: &JsonMap<String, JsonValue>,
) -> JsonMap<String, JsonValue> {
    let mut out = base.clone();
    for (key, value) in incoming {
        if key == "_include" {
            let mut merged = normalize_include(out.get(key));
            merged.extend(normalize_include(Some(value)));
            out.insert(
                key.clone(),
                JsonValue::Array(merged.into_iter().map(JsonValue::String).collect()),
            );
            continue;
        }

        match (out.get(key), value) {
            (Some(JsonValue::Object(current)), JsonValue::Object(incoming_map)) => {
                out.insert(
                    key.clone(),
                    JsonValue::Object(merge_maps(current, incoming_map)),
                );
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

fn ensure_global_includes_map(root: &mut JsonMap<String, JsonValue>) {
    let global = root
        .entry("global".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !global.is_object() {
        *global = JsonValue::Object(JsonMap::new());
    }
    if let JsonValue::Object(global_map) = global {
        if !global_map
            .get("_includes")
            .is_some_and(JsonValue::is_object)
        {
            global_map.insert("_includes".to_string(), JsonValue::Object(JsonMap::new()));
        }
    }
}

fn normalize_global_includes_payload(
    loaded_map: &JsonMap<String, JsonValue>,
) -> JsonMap<String, JsonValue> {
    if let Some(includes) = loaded_map
        .get("global")
        .and_then(as_obj)
        .and_then(|global| global.get("_includes"))
        .and_then(as_obj)
    {
        return includes.clone();
    }
    loaded_map.clone()
}

fn read_text_from_path_with_overrides(
    path: &Path,
    overrides: &HashMap<PathBuf, String>,
) -> Result<String, String> {
    let normalized = normalize_fs_path(path);
    if let Some(text) = overrides.get(&normalized) {
        return Ok(text.clone());
    }
    fs::read_to_string(path)
        .map_err(|err| format!("read include file '{}': {}", path.display(), err))
}

fn load_yaml_map_from_file(
    raw_path: &str,
    base_dir: Option<&Path>,
    overrides: &HashMap<PathBuf, String>,
    file_stack: &mut HashSet<PathBuf>,
) -> Result<Option<(PathBuf, JsonMap<String, JsonValue>)>, String> {
    if is_templated_include_path(raw_path) {
        return Ok(None);
    }
    let candidates = build_include_candidates(raw_path, base_dir);
    for candidate in candidates {
        let normalized = normalize_fs_path(&candidate);
        if file_stack.contains(&normalized) {
            return Err(format!(
                "_include file cycle detected: {}",
                normalized.display()
            ));
        }
        file_stack.insert(normalized.clone());
        let loaded = read_text_from_path_with_overrides(&candidate, overrides);
        file_stack.remove(&normalized);
        let text = match loaded {
            Ok(value) => value,
            Err(message) => {
                let is_not_found = message.contains("No such file")
                    || message.contains("not a directory")
                    || message.contains("os error 2")
                    || message.contains("os error 20");
                if is_not_found {
                    continue;
                }
                return Err(message);
            }
        };
        let parsed = parse_yaml_map_to_json_map(&text)?;
        return Ok(Some((candidate, parsed)));
    }
    Ok(None)
}

fn normalize_fs_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn include_name_from_path(path_value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_templated_include_path(path_value: &str) -> bool {
    path_value.contains("{{") || path_value.contains("}}")
}

fn build_include_candidates(raw_path: &str, base_dir: Option<&Path>) -> Vec<PathBuf> {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        return vec![path.to_path_buf()];
    }
    if let Some(base) = base_dir {
        return vec![base.join(path)];
    }
    vec![path.to_path_buf()]
}

fn is_direct_global_includes_path(path_segments: &[String]) -> bool {
    path_segments.len() == 2 && path_segments[0] == "global" && path_segments[1] == "_includes"
}

fn discover_environments(values: &JsonValue) -> EnvironmentDiscovery {
    let mut literals: HashSet<String> = HashSet::new();
    let mut regexes: HashSet<String> = HashSet::new();

    if let Some(global_env) = values
        .as_object()
        .and_then(|root| root.get("global"))
        .and_then(as_obj)
        .and_then(|global| global.get("env"))
        .and_then(JsonValue::as_str)
    {
        let trimmed = global_env.trim();
        if !trimmed.is_empty() {
            literals.insert(trimmed.to_string());
        }
    }

    walk_maps(values, &mut |map| {
        if !looks_like_env_map(map) {
            return;
        }
        for key in map.keys() {
            if key == "_default" {
                continue;
            }
            if looks_like_regex_pattern(key) {
                regexes.insert(key.clone());
            } else {
                literals.insert(key.clone());
            }
        }
    });

    let mut literals_vec: Vec<String> = literals.into_iter().collect();
    literals_vec.sort();
    let mut regexes_vec: Vec<String> = regexes.into_iter().collect();
    regexes_vec.sort();
    EnvironmentDiscovery {
        literals: literals_vec,
        regexes: regexes_vec,
    }
}

fn detect_default_env(values: &JsonValue, env_discovery: &EnvironmentDiscovery) -> String {
    if let Some(global_env) = values
        .as_object()
        .and_then(|root| root.get("global"))
        .and_then(as_obj)
        .and_then(|global| global.get("env"))
        .and_then(JsonValue::as_str)
    {
        let trimmed = global_env.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    env_discovery
        .literals
        .first()
        .cloned()
        .unwrap_or_else(|| "dev".to_string())
}

fn resolve_env_maps(value: &JsonValue, env: &str) -> JsonValue {
    match value {
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .map(|item| resolve_env_maps(item, env))
                .collect(),
        ),
        JsonValue::Object(map) => {
            if looks_like_env_map(map) {
                let selected = select_env_value(map, env);
                if selected == JsonValue::Object(map.clone()) {
                    let mut out = JsonMap::new();
                    for (key, value) in map {
                        out.insert(key.clone(), resolve_env_maps(value, env));
                    }
                    return JsonValue::Object(out);
                }
                return resolve_env_maps(&selected, env);
            }
            let mut out = JsonMap::new();
            for (key, value) in map {
                out.insert(key.clone(), resolve_env_maps(value, env));
            }
            JsonValue::Object(out)
        }
        _ => value.clone(),
    }
}

fn looks_like_env_map(map: &JsonMap<String, JsonValue>) -> bool {
    if map.contains_key("_default") {
        return true;
    }
    map.keys().any(|key| looks_like_regex_pattern(key))
}

fn looks_like_regex_pattern(key: &str) -> bool {
    if key.is_empty() || key == "_default" {
        return false;
    }
    if key.starts_with('^') || key.ends_with('$') {
        return true;
    }
    if key.contains(".*") || key.contains(".+") || key.contains(".?") {
        return true;
    }
    key.chars()
        .any(|ch| matches!(ch, '[' | ']' | '(' | ')' | '|' | '\\'))
}

fn select_env_value(map: &JsonMap<String, JsonValue>, env: &str) -> JsonValue {
    if let Some(value) = map.get(env) {
        return value.clone();
    }
    for (key, value) in map {
        if key == "_default" || !looks_like_regex_pattern(key) {
            continue;
        }
        if let Ok(regex) = regex::Regex::new(key) {
            if regex.is_match(env) {
                return value.clone();
            }
        }
    }
    if let Some(value) = map.get("_default") {
        return value.clone();
    }
    JsonValue::Object(map.clone())
}

// Mirrors "fl._expandValueReferences" from
// charts/helm-apps/templates/fl-functions/_value.tpl (helm-apps >= 1.9.0).
// Contract: docs/reference-values.md#param-value-references.
//
// One deliberate difference from the chart: the escaped marker is left intact
// instead of being unescaped. The exported ordinary chart still inlines
// fl-functions/_value.tpl, so exported values are fed through `fl.value` once
// more at render time. Unescaping here would let that second pass resolve a
// marker the source chart renders literally.
const VALUE_REF_OPEN: &str = "$fl.value{";
const VALUE_REF_ESCAPED_OPEN: &str = "$$fl.value{";
const VALUE_REF_ESCAPE_PLACEHOLDER: &str = "\u{0}happ-escaped-fl-value\u{0}";
const VALUE_REF_DOCS: &str = "docs/reference-values.md#param-value-references";

fn value_reference_regex() -> Option<&'static regex::Regex> {
    static VALUE_REF_RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    VALUE_REF_RE
        .get_or_init(|| {
            regex::Regex::new(r"\$fl\.value\{[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*\}").ok()
        })
        .as_ref()
}

fn expand_value_references(root: &mut JsonValue) -> Result<(), String> {
    let snapshot = root.clone();
    expand_value_references_in_node(root, &snapshot)
}

fn expand_value_references_in_node(node: &mut JsonValue, root: &JsonValue) -> Result<(), String> {
    match node {
        JsonValue::String(text) => {
            if text.contains(VALUE_REF_OPEN) {
                let mut stack = Vec::new();
                *text = expand_value_references_in_string(text, root, &mut stack)?;
            }
            Ok(())
        }
        JsonValue::Array(items) => {
            for item in items {
                expand_value_references_in_node(item, root)?;
            }
            Ok(())
        }
        JsonValue::Object(map) => {
            for (_, value) in map.iter_mut() {
                expand_value_references_in_node(value, root)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn expand_value_references_in_string(
    text: &str,
    root: &JsonValue,
    stack: &mut Vec<String>,
) -> Result<String, String> {
    let mut work = text.replace(VALUE_REF_ESCAPED_OPEN, VALUE_REF_ESCAPE_PLACEHOLDER);
    if !work.contains(VALUE_REF_OPEN) {
        return Ok(work.replace(VALUE_REF_ESCAPE_PLACEHOLDER, VALUE_REF_ESCAPED_OPEN));
    }
    let regex = value_reference_regex()
        .ok_or_else(|| "E_VALUE_REF_SYNTAX: internal reference pattern is invalid".to_string())?;

    let mut references: Vec<String> = Vec::new();
    for found in regex.find_iter(&work) {
        let reference = found.as_str().to_string();
        if !references.contains(&reference) {
            references.push(reference);
        }
    }

    let mut residue = work.clone();
    for reference in &references {
        residue = residue.replace(reference.as_str(), "");
    }
    if residue.contains(VALUE_REF_OPEN) {
        return Err(format!(
            "E_VALUE_REF_SYNTAX: invalid $fl.value reference syntax in '{text}'; \
             use $fl.value{{global.path}} with dot-separated [A-Za-z0-9_-] path segments ({VALUE_REF_DOCS})"
        ));
    }

    for reference in &references {
        let path = reference
            .strip_prefix(VALUE_REF_OPEN)
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or_default();
        if stack.iter().any(|entry| entry == path) {
            let mut chain = stack.clone();
            chain.push(path.to_string());
            return Err(format!(
                "E_VALUE_REF_CYCLE: cyclic $fl.value reference detected: {}; \
                 remove the reference cycle ({VALUE_REF_DOCS})",
                chain.join(" -> ")
            ));
        }
        let target = lookup_value_reference_path(root, path).ok_or_else(|| {
            format!(
                "E_VALUE_REF_NOT_FOUND: $fl.value reference path not found: {path}; \
                 define the path under .Values or fix the reference ({VALUE_REF_DOCS})"
            )
        })?;
        stack.push(path.to_string());
        let resolved = render_value_reference_target(target, root, stack)?;
        stack.pop();
        work = work.replace(reference.as_str(), &resolved);
    }

    Ok(work.replace(VALUE_REF_ESCAPE_PLACEHOLDER, VALUE_REF_ESCAPED_OPEN))
}

fn lookup_value_reference_path<'a>(root: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let mut current = root;
    for segment in path.split('.') {
        current = as_obj(current)?.get(segment)?;
    }
    Some(current)
}

// "fl._renderValue" only emits scalars: maps, slices and nil render as empty
// output, so reference substitution must collapse them the same way to keep
// `library export-ordinary` equivalent to a real Helm render.
fn render_value_reference_target(
    value: &JsonValue,
    root: &JsonValue,
    stack: &mut Vec<String>,
) -> Result<String, String> {
    match value {
        JsonValue::String(text) => expand_value_references_in_string(text, root, stack),
        JsonValue::Bool(flag) => Ok(flag.to_string()),
        JsonValue::Number(number) => Ok(number.to_string()),
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => Ok(String::new()),
    }
}

fn walk_maps(value: &JsonValue, on_map: &mut dyn FnMut(&JsonMap<String, JsonValue>)) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                walk_maps(item, on_map);
            }
        }
        JsonValue::Object(map) => {
            on_map(map);
            for value in map.values() {
                walk_maps(value, on_map);
            }
        }
        _ => {}
    }
}

fn ensure_global_env_literal(root: &mut JsonValue, env: &str) {
    let Some(root_map) = root.as_object_mut() else {
        return;
    };
    let global = root_map
        .entry("global".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !global.is_object() {
        *global = JsonValue::Object(JsonMap::new());
    }
    if let Some(global_map) = global.as_object_mut() {
        global_map.insert("env".to_string(), JsonValue::String(env.to_string()));
    }
}

fn strip_internal_include_fields(node: &mut JsonValue) {
    match node {
        JsonValue::Array(items) => {
            for item in items {
                strip_internal_include_fields(item);
            }
        }
        JsonValue::Object(map) => {
            for value in map.values_mut() {
                strip_internal_include_fields(value);
            }
            map.remove("_include");
            map.remove("_include_files");
            map.remove("_include_from_file");
            if let Some(global) = map.get_mut("global").and_then(JsonValue::as_object_mut) {
                global.remove("_includes");
            }
        }
        _ => {}
    }
}

fn as_obj(value: &JsonValue) -> Option<&JsonMap<String, JsonValue>> {
    match value {
        JsonValue::Object(map) => Some(map),
        _ => None,
    }
}

fn json_value_to_yaml_value(value: &JsonValue) -> Result<Value, String> {
    Ok(crate::chart_ir::json_to_yaml_value(value))
}

#[derive(Debug, Clone)]
struct EnvironmentDiscovery {
    literals: Vec<String>,
    regexes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_library_chart_values_expands_includes_files_and_env() {
        let td = TempDir::new().expect("tmp");
        let chart_root = td.path().join("chart");
        fs::create_dir_all(chart_root.join("templates")).expect("mkdir templates");
        fs::write(
            chart_root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\ntype: application\nversion: 0.1.0\n",
        )
        .expect("write chart yaml");
        fs::write(
            chart_root.join("defaults.yaml"),
            r#"
default_api:
  labels: |
    role: api
"#,
        )
        .expect("write defaults");
        fs::write(
            chart_root.join("extra-service.yaml"),
            r#"
apps-services:
  api:
    enabled: true
    name: api
    selector: |
      app: api
    ports:
      _default: |
        - name: http
          port: 80
          targetPort: 80
      prod: |
        - name: http
          port: 8080
          targetPort: 8080
"#,
        )
        .expect("write include file");
        fs::write(
            chart_root.join("values.yaml"),
            r#"
global:
  env: prod
  _includes:
    _include_from_file: defaults.yaml
apps-stateless:
  api:
    _include: default_api
    enabled: true
    replicas:
      _default: 1
      prod: 2
    containers:
      app:
        image:
          name: nginx
          staticTag:
            _default: latest
            prod: "1.27"
_include_files:
  - extra-service.yaml
"#,
        )
        .expect("write values");

        let loaded =
            load_library_chart_values_for_export(chart_root.to_str().expect("chart path"), None)
                .expect("load values");
        assert_eq!(loaded.selected_env.as_deref(), Some("prod"));

        let root = loaded.values.as_mapping().expect("root mapping");
        let global = root
            .get(Value::String("global".into()))
            .and_then(Value::as_mapping)
            .expect("global");
        assert_eq!(
            global
                .get(Value::String("env".into()))
                .and_then(Value::as_str),
            Some("prod")
        );
        assert!(
            !global.contains_key(Value::String("_includes".into())),
            "flattened export must not keep global._includes"
        );

        let apps_stateless = root
            .get(Value::String("apps-stateless".into()))
            .and_then(Value::as_mapping)
            .expect("apps-stateless");
        let api = apps_stateless
            .get(Value::String("api".into()))
            .and_then(Value::as_mapping)
            .expect("api");
        assert!(
            !api.contains_key(Value::String("_include".into())),
            "flattened export must not keep app-level _include"
        );
        assert_eq!(
            api.get(Value::String("replicas".into()))
                .and_then(Value::as_i64),
            Some(2)
        );
        assert!(api
            .get(Value::String("labels".into()))
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("role: api")));
        let containers = api
            .get(Value::String("containers".into()))
            .and_then(Value::as_mapping)
            .expect("containers");
        let app = containers
            .get(Value::String("app".into()))
            .and_then(Value::as_mapping)
            .expect("app");
        let image = app
            .get(Value::String("image".into()))
            .and_then(Value::as_mapping)
            .expect("image");
        assert_eq!(
            image
                .get(Value::String("staticTag".into()))
                .and_then(Value::as_str),
            Some("1.27")
        );

        let apps_services = root
            .get(Value::String("apps-services".into()))
            .and_then(Value::as_mapping)
            .expect("apps-services");
        let service = apps_services
            .get(Value::String("api".into()))
            .and_then(Value::as_mapping)
            .expect("service");
        assert!(service
            .get(Value::String("ports".into()))
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("8080")));
    }

    fn json_root_from_yaml(text: &str) -> JsonValue {
        let yaml: Value = serde_yaml::from_str(text).expect("parse yaml");
        serde_json::to_value(yaml).expect("yaml to json")
    }

    fn expanded_string_at(root: &JsonValue, path: &str) -> String {
        lookup_value_reference_path(root, path)
            .and_then(JsonValue::as_str)
            .unwrap_or_else(|| panic!("string at {path}"))
            .to_string()
    }

    #[test]
    fn expands_value_references_embedded_multiple_and_recursive() {
        let mut root = json_root_from_yaml(
            r#"
global:
  vars:
    registry: registry.example.com
    replicas: 3
    debug: true
    imageName: '$fl.value{global.vars.registry}/api'
app:
  image: '$fl.value{global.vars.imageName}:1.0'
  replicas: '$fl.value{global.vars.replicas}'
  debug: '$fl.value{global.vars.debug}'
  pair: '$fl.value{global.vars.registry} + $fl.value{global.vars.replicas}'
  repeated: '$fl.value{global.vars.replicas}/$fl.value{global.vars.replicas}'
"#,
        );
        expand_value_references(&mut root).expect("expand references");

        assert_eq!(
            expanded_string_at(&root, "global.vars.imageName"),
            "registry.example.com/api"
        );
        assert_eq!(
            expanded_string_at(&root, "app.image"),
            "registry.example.com/api:1.0",
            "recursive reference must resolve through the referenced value"
        );
        assert_eq!(expanded_string_at(&root, "app.replicas"), "3");
        assert_eq!(expanded_string_at(&root, "app.debug"), "true");
        assert_eq!(
            expanded_string_at(&root, "app.pair"),
            "registry.example.com + 3",
            "multiple references inside one string must all resolve"
        );
        assert_eq!(expanded_string_at(&root, "app.repeated"), "3/3");
    }

    // The exported chart still runs `fl.value` over these values, so the escape
    // must survive export untouched and be unescaped by that later pass.
    #[test]
    fn keeps_escaped_value_reference_marker_for_the_downstream_render() {
        let mut root = json_root_from_yaml(
            r#"
global:
  vars:
    registry: registry.example.com
app:
  literal: '$$fl.value{global.vars.registry}'
  mixed: '$$fl.value{global.vars.registry} -> $fl.value{global.vars.registry}'
"#,
        );
        expand_value_references(&mut root).expect("expand references");

        assert_eq!(
            expanded_string_at(&root, "app.literal"),
            "$$fl.value{global.vars.registry}"
        );
        assert_eq!(
            expanded_string_at(&root, "app.mixed"),
            "$$fl.value{global.vars.registry} -> registry.example.com"
        );
    }

    #[test]
    fn keeps_escaped_marker_reached_through_a_reference() {
        let mut root = json_root_from_yaml(
            r#"
global:
  vars:
    literal: '$$fl.value{global.vars.registry}'
    registry: registry.example.com
app:
  image: '$fl.value{global.vars.literal}'
"#,
        );
        expand_value_references(&mut root).expect("expand references");

        assert_eq!(
            expanded_string_at(&root, "app.image"),
            "$$fl.value{global.vars.registry}",
            "escape must survive substitution, not just top-level expansion"
        );
    }

    #[test]
    fn reports_missing_value_reference_path() {
        let mut root = json_root_from_yaml(
            r#"
app:
  image: '$fl.value{global.vars.missing}'
"#,
        );
        let err = expand_value_references(&mut root).expect_err("missing path must fail");
        assert!(
            err.contains("E_VALUE_REF_NOT_FOUND"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("global.vars.missing"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reports_invalid_value_reference_syntax() {
        let mut root = json_root_from_yaml(
            r#"
app:
  image: '$fl.value{global vars}'
"#,
        );
        let err = expand_value_references(&mut root).expect_err("invalid syntax must fail");
        assert!(
            err.contains("E_VALUE_REF_SYNTAX"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reports_cyclic_value_reference() {
        let mut root = json_root_from_yaml(
            r#"
global:
  a: '$fl.value{global.b}'
  b: '$fl.value{global.a}'
"#,
        );
        let err = expand_value_references(&mut root).expect_err("cycle must fail");
        assert!(err.contains("E_VALUE_REF_CYCLE"), "unexpected error: {err}");
    }

    #[test]
    fn reports_self_referencing_value_reference_as_cycle() {
        let mut root = json_root_from_yaml(
            r#"
global:
  a: '$fl.value{global.a}'
"#,
        );
        let err = expand_value_references(&mut root).expect_err("self reference must fail");
        assert!(err.contains("E_VALUE_REF_CYCLE"), "unexpected error: {err}");
    }

    #[test]
    fn renders_non_scalar_value_reference_target_as_empty_like_fl_render_value() {
        let mut root = json_root_from_yaml(
            r#"
global:
  vars:
    block:
      key: value
app:
  image: 'prefix-$fl.value{global.vars.block}-suffix'
"#,
        );
        expand_value_references(&mut root).expect("expand references");
        assert_eq!(
            expanded_string_at(&root, "app.image"),
            "prefix--suffix",
            "map targets render empty in fl._renderValue, export must match"
        );
    }

    #[test]
    fn load_library_chart_values_resolves_value_references_after_env_maps() {
        let td = TempDir::new().expect("tmp");
        let chart_root = td.path().join("chart");
        fs::create_dir_all(chart_root.join("templates")).expect("mkdir templates");
        fs::write(
            chart_root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\ntype: application\nversion: 0.1.0\n",
        )
        .expect("write chart yaml");
        fs::write(
            chart_root.join("values.yaml"),
            r#"
global:
  env: prod
  vars:
    registry:
      _default: registry.dev.example.com
      prod: registry.prod.example.com
    replicas:
      _default: 1
      prod: 5
apps-stateless:
  api:
    enabled: true
    replicas: '$fl.value{global.vars.replicas}'
    containers:
      app:
        image:
          name: '$fl.value{global.vars.registry}/api'
          staticTag: "1.27"
"#,
        )
        .expect("write values");

        let loaded =
            load_library_chart_values_for_export(chart_root.to_str().expect("chart path"), None)
                .expect("load values");
        assert_eq!(loaded.selected_env.as_deref(), Some("prod"));

        let root = loaded.values.as_mapping().expect("root mapping");
        let api = root
            .get(Value::String("apps-stateless".into()))
            .and_then(Value::as_mapping)
            .and_then(|group| group.get(Value::String("api".into())))
            .and_then(Value::as_mapping)
            .expect("api");
        assert_eq!(
            api.get(Value::String("replicas".into()))
                .and_then(Value::as_str),
            Some("5"),
            "reference target must be env-resolved before substitution"
        );

        let image_name = api
            .get(Value::String("containers".into()))
            .and_then(Value::as_mapping)
            .and_then(|containers| containers.get(Value::String("app".into())))
            .and_then(Value::as_mapping)
            .and_then(|container| container.get(Value::String("image".into())))
            .and_then(Value::as_mapping)
            .and_then(|image| image.get(Value::String("name".into())))
            .and_then(Value::as_str)
            .expect("image name");
        assert_eq!(image_name, "registry.prod.example.com/api");
    }
}
