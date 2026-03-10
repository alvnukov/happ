use happ::output::{optimize_values_with_include_profiles, values_yaml};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;

fn k(name: &str) -> Value {
    Value::String(name.to_string())
}

fn parse_yaml(src: &str) -> Value {
    serde_yaml::from_str(src).expect("yaml parse")
}

fn to_json_without_global_includes(value: &Value) -> serde_json::Value {
    let mut cloned = value.clone();
    strip_global_includes(&mut cloned);
    serde_json::to_value(&cloned).expect("yaml->json")
}

fn strip_global_includes(value: &mut Value) {
    let Some(root) = value.as_mapping_mut() else {
        return;
    };
    let Some(global) = root.get_mut(k("global")).and_then(Value::as_mapping_mut) else {
        return;
    };
    global.remove(k("_includes"));
}

fn include_refs(value: &Value, group: &str, app: &str) -> Vec<String> {
    value
        .as_mapping()
        .and_then(|root| root.get(k(group)))
        .and_then(Value::as_mapping)
        .and_then(|group_map| group_map.get(k(app)))
        .and_then(Value::as_mapping)
        .and_then(|entity| entity.get(k("_include")))
        .map(normalize_include_refs)
        .unwrap_or_default()
}

fn normalize_include_refs(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![trimmed.to_string()]
            }
        }
        Value::Sequence(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::trim))
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn merge_maps(base: &Mapping, incoming: &Mapping) -> Mapping {
    let mut out = base.clone();
    for (key, value) in incoming {
        if key.as_str() == Some("_include") {
            let mut merged = out.get(key).map(normalize_include_refs).unwrap_or_default();
            merged.extend(normalize_include_refs(value));
            out.insert(
                key.clone(),
                Value::Sequence(merged.into_iter().map(Value::String).collect()),
            );
            continue;
        }
        match (out.get(key), value) {
            (Some(Value::Mapping(current_map)), Value::Mapping(incoming_map)) => {
                out.insert(
                    key.clone(),
                    Value::Mapping(merge_maps(current_map, incoming_map)),
                );
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

fn resolve_profile(
    name: &str,
    includes: &Mapping,
    cache: &mut BTreeMap<String, Mapping>,
    stack: &mut Vec<String>,
) -> Result<Mapping, String> {
    if let Some(cached) = cache.get(name) {
        return Ok(cached.clone());
    }
    if stack.iter().any(|s| s == name) {
        let mut cycle = stack.clone();
        cycle.push(name.to_string());
        return Err(format!("include cycle detected: {}", cycle.join(" -> ")));
    }

    let Some(profile) = includes.get(k(name)).and_then(Value::as_mapping) else {
        return Ok(Mapping::new());
    };

    stack.push(name.to_string());
    let mut merged = Mapping::new();
    if let Some(v) = profile.get(k("_include")) {
        for child in normalize_include_refs(v) {
            let child_map = resolve_profile(&child, includes, cache, stack)?;
            merged = merge_maps(&merged, &child_map);
        }
    }
    stack.pop();

    merged = merge_maps(&merged, profile);
    merged.remove(k("_include"));
    cache.insert(name.to_string(), merged.clone());
    Ok(merged)
}

fn expand_node(
    node: &Value,
    includes: &Mapping,
    cache: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    match node {
        Value::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(expand_node(item, includes, cache)?);
            }
            Ok(Value::Sequence(out))
        }
        Value::Mapping(map) => {
            let mut current = map.clone();
            if let Some(include_ref) = current.get(k("_include")).cloned() {
                let mut merged = Mapping::new();
                for include_name in normalize_include_refs(&include_ref) {
                    let profile = resolve_profile(&include_name, includes, cache, &mut Vec::new())?;
                    merged = merge_maps(&merged, &profile);
                }
                current.remove(k("_include"));
                current = merge_maps(&merged, &current);
            }
            let mut out = Mapping::new();
            for (key, value) in current {
                if key.as_str() == Some("_includes") {
                    out.insert(key, value);
                } else {
                    out.insert(key, expand_node(&value, includes, cache)?);
                }
            }
            Ok(Value::Mapping(out))
        }
        _ => Ok(node.clone()),
    }
}

fn expand_includes(value: &Value) -> Value {
    let Some(root) = value.as_mapping() else {
        return value.clone();
    };
    let includes = root
        .get(k("global"))
        .and_then(Value::as_mapping)
        .and_then(|g| g.get(k("_includes")))
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    expand_node(value, &includes, &mut BTreeMap::new()).expect("expand includes")
}

fn assert_semantically_equal_with_includes(original: &Value, optimized: &Value) {
    let expanded = expand_includes(optimized);
    let left = to_json_without_global_includes(original);
    let right = to_json_without_global_includes(&expanded);
    assert_eq!(left, right, "optimized values changed semantic content");
}

#[test]
fn include_optimization_extracts_multiple_profiles_for_complex_overlaps() {
    let input = parse_yaml(
        r#"
global:
  env: prod
apps-stateless:
  api:
    enabled: true
    pod:
      labels:
        team: core
        tier: backend
      resources:
        requests:
          cpu: "250m"
          memory: "256Mi"
        limits:
          cpu: "500m"
          memory: "512Mi"
      securityContext:
        runAsNonRoot: true
    service:
      enabled: true
      type: ClusterIP
      ports:
        http: 8080
        grpc: 9090
    replicas: 3
  web:
    enabled: true
    pod:
      labels:
        team: core
        tier: backend
      resources:
        requests:
          cpu: "250m"
          memory: "256Mi"
        limits:
          cpu: "500m"
          memory: "512Mi"
      securityContext:
        runAsNonRoot: true
    service:
      enabled: true
      type: ClusterIP
      ports:
        http: 8080
        grpc: 9090
    replicas: 2
  worker:
    enabled: true
    pod:
      labels:
        team: core
        tier: backend
      resources:
        requests:
          cpu: "250m"
          memory: "256Mi"
        limits:
          cpu: "500m"
          memory: "512Mi"
      securityContext:
        runAsNonRoot: true
    queue:
      concurrency: 5
      maxRetries: 7
"#,
    );

    let (optimized, report) = optimize_values_with_include_profiles(&input, 24);
    assert!(
        report.profiles_added >= 2,
        "expected at least two profiles for subset overlaps, got {}",
        report.profiles_added
    );

    let api_refs = include_refs(&optimized, "apps-stateless", "api");
    let web_refs = include_refs(&optimized, "apps-stateless", "web");
    let worker_refs = include_refs(&optimized, "apps-stateless", "worker");
    assert!(
        api_refs.len() >= 2 && web_refs.len() >= 2,
        "api/web should reference multiple profiles for efficient collapse"
    );
    assert!(
        !worker_refs.is_empty(),
        "worker should still reuse common include profile"
    );

    let original_yaml = values_yaml(&input).expect("serialize original");
    let optimized_yaml = values_yaml(&optimized).expect("serialize optimized");
    assert!(
        optimized_yaml.len() * 100 <= original_yaml.len() * 95,
        "expected at least 5% size reduction, original={}, optimized={}",
        original_yaml.len(),
        optimized_yaml.len()
    );

    assert_semantically_equal_with_includes(&input, &optimized);
}

#[test]
fn include_optimization_keeps_existing_profiles_and_avoids_name_collisions() {
    let input = parse_yaml(
        r#"
global:
  _includes:
    default_apps_stateless:
      legacy:
        enabled: true
apps-stateless:
  api:
    config:
      flags:
        a: true
        b: true
      limits:
        cpu: "300m"
        memory: "256Mi"
    replicas: 2
  web:
    config:
      flags:
        a: true
        b: true
      limits:
        cpu: "300m"
        memory: "256Mi"
    replicas: 1
"#,
    );

    let (optimized, report) = optimize_values_with_include_profiles(&input, 24);
    assert!(report.profiles_added >= 1);
    let includes = optimized
        .as_mapping()
        .and_then(|root| root.get(k("global")))
        .and_then(Value::as_mapping)
        .and_then(|global| global.get(k("_includes")))
        .and_then(Value::as_mapping)
        .expect("_includes map");
    assert!(includes.contains_key(k("default_apps_stateless")));
    assert!(includes.contains_key(k("default_apps_stateless_2")));

    assert_semantically_equal_with_includes(&input, &optimized);
}

#[test]
fn include_optimization_is_deterministic_and_collapses_each_group_independently() {
    let input = parse_yaml(
        r#"
global:
  env: dev
apps-stateless:
  api:
    enabled: true
    image:
      repository: ghcr.io/acme/app
      pullPolicy: IfNotPresent
    probes:
      readiness:
        path: /ready
        port: http
      liveness:
        path: /live
        port: http
    replicas: 2
  web:
    enabled: true
    image:
      repository: ghcr.io/acme/app
      pullPolicy: IfNotPresent
    probes:
      readiness:
        path: /ready
        port: http
      liveness:
        path: /live
        port: http
    replicas: 1
apps-cron:
  daily-cleanup:
    enabled: true
    image:
      repository: ghcr.io/acme/cron
      pullPolicy: IfNotPresent
    schedule: "0 3 * * *"
    backoffLimit: 2
  weekly-compact:
    enabled: true
    image:
      repository: ghcr.io/acme/cron
      pullPolicy: IfNotPresent
    schedule: "0 2 * * 0"
    backoffLimit: 2
"#,
    );

    let (optimized_a, report_a) = optimize_values_with_include_profiles(&input, 24);
    let (optimized_b, report_b) = optimize_values_with_include_profiles(&input, 24);
    assert_eq!(optimized_a, optimized_b, "optimizer must be deterministic");
    assert_eq!(report_a, report_b, "report must be deterministic");

    let includes = optimized_a
        .as_mapping()
        .and_then(|root| root.get(k("global")))
        .and_then(Value::as_mapping)
        .and_then(|global| global.get(k("_includes")))
        .and_then(Value::as_mapping)
        .expect("_includes map");
    let names: Vec<String> = includes
        .keys()
        .filter_map(|k| k.as_str().map(ToString::to_string))
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("default_apps_stateless")),
        "missing profile for apps-stateless: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.starts_with("default_apps_cron")),
        "missing profile for apps-cron: {:?}",
        names
    );

    assert!(!include_refs(&optimized_a, "apps-stateless", "api").is_empty());
    assert!(!include_refs(&optimized_a, "apps-cron", "daily-cleanup").is_empty());
    assert_semantically_equal_with_includes(&input, &optimized_a);
}

#[test]
fn include_optimization_respects_threshold_for_small_repeated_chunks() {
    let input = parse_yaml(
        r#"
global:
  env: dev
apps-stateless:
  api:
    enabled: true
    tracing: true
    replicas: 2
  web:
    enabled: true
    tracing: true
    replicas: 1
  worker:
    enabled: true
    tracing: true
    replicas: 1
"#,
    );

    let (optimized, report) = optimize_values_with_include_profiles(&input, 1024);
    assert_eq!(report.profiles_added, 0);
    assert_eq!(optimized, input);
}
