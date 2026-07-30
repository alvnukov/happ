//! Env-map resolution: the single source of truth for helm-apps env selection.
//!
//! Mirrors "fl.value" and "_fl.getValueRegex" from
//! charts/helm-apps/templates/fl-functions/_value.tpl (helm-apps >= 1.9.0).
//! Contract: docs/reference-values.md#param-global-env.
//!
//! Selection order, exactly as the chart resolves it:
//!
//! 1. the key equal to `global.env`;
//! 2. a regex key, anchored to the whole env name;
//! 3. `_default`.
//!
//! Two responsibilities are deliberately kept apart.
//!
//! *Detecting* that a map is an env map follows the chart wherever the chart
//! states it. The library decides by call site, not by shape: every map handed
//! to "fl.value" is resolved by environment, and one that names no value for
//! the current env renders as empty. [`env_selected_value_keys`] reads those
//! call sites out of the embedded chart, so an app value such as
//! `priorityClassName: {dc1: ...}` resolves even though it carries neither
//! `_default` nor a regex key. Outside an app the library reads values
//! directly — `global.labels.addEnv` is not a "fl.value" site — so there happ
//! falls back to inferring from key shapes, which is what
//! [`looks_like_regex_pattern`] is for.
//!
//! *Selecting* inside a map that is an env map follows the chart literally:
//! every key is a regex there, so `a.c` matches env `abc` the same way it does
//! under Helm.

use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::HashSet;

/// The env key holding the fallback value.
const DEFAULT_ENV_KEY: &str = "_default";

/// Error code the chart raises when several regex keys match the current env.
pub(crate) const AMBIGUOUS_ENV_REGEX_CODE: &str = "E_ENV_REGEX_AMBIGUOUS";

/// Contract anchor for env maps, quoted in diagnostics.
pub(crate) const ENV_DOCS: &str = "docs/reference-values.md#param-global-env";

/// Env names found in a values tree, split by how they were written.
#[derive(Debug, Serialize, Clone)]
pub(crate) struct EnvironmentDiscovery {
    pub(crate) literals: Vec<String>,
    pub(crate) regexes: Vec<String>,
}

/// One env map whose regex keys are ambiguous for the current env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbiguousEnvRegex {
    /// Dotted path to the env map inside the values tree.
    pub(crate) path: String,
    /// Every anchored pattern that matched, sorted.
    pub(crate) patterns: Vec<String>,
}

/// Renders an ambiguity the way the chart reports it, so the same text is
/// greppable whether it came from `helm template` or from happ.
pub(crate) fn ambiguous_env_regex_message(found: &AmbiguousEnvRegex) -> String {
    format!(
        "[helm-apps:{AMBIGUOUS_ENV_REGEX_CODE}] multiple env regex keys match current global.env: [{}] | path={} | hint=leave only one matching regex key for this value | docs={ENV_DOCS}",
        found.patterns.join(" "),
        found.path,
    )
}

/// Collects every env name a values tree mentions, so a UI can offer them.
///
/// Keys are classified by [`looks_like_regex_pattern`]: a dropdown wants the
/// concrete `prod`, not the pattern `^prod-.*$`.
pub(crate) fn discover_environments(values: &JsonValue) -> EnvironmentDiscovery {
    let mut literals: HashSet<String> = HashSet::new();
    let mut regexes: HashSet<String> = HashSet::new();

    if let Some(global_env) = global_env(values) {
        literals.insert(global_env);
    }

    walk_maps(values, &mut |map| {
        if !looks_like_env_map(map) {
            return;
        }
        for key in map.keys() {
            if key == DEFAULT_ENV_KEY {
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

/// Picks the env to resolve against when the caller did not name one.
pub(crate) fn detect_default_env(
    values: &JsonValue,
    env_discovery: &EnvironmentDiscovery,
) -> String {
    global_env(values)
        .or_else(|| env_discovery.literals.first().cloned())
        .unwrap_or_else(|| "dev".to_string())
}

/// Reads a non-empty `global.env` out of a values tree.
pub(crate) fn global_env(values: &JsonValue) -> Option<String> {
    values
        .as_object()
        .and_then(|root| root.get("global"))
        .and_then(JsonValue::as_object)
        .and_then(|global| global.get("env"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Where in the values tree a value sits, which decides whether the `fl.value`
/// contract applies to it or whether happ has to fall back on key shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Chart-level keys such as `global`, which the library reads directly.
    Outside,
    /// A top-level `apps-*` group; its children are app names.
    Group,
    /// One app; its children are the app's value keys.
    AppRoot,
    /// An app value, where the key names a `fl.value` site.
    AppValue,
}

impl Scope {
    /// The scope a child key falls into.
    ///
    /// An app's own name is not a value key -- a chart may well have an app
    /// called `labels` -- so a group's children step through [`Scope::AppRoot`]
    /// before anything counts as a value.
    fn enter(self, key: &str) -> Self {
        match self {
            Scope::Outside if key.starts_with("apps-") => Scope::Group,
            Scope::Outside => Scope::Outside,
            Scope::Group => Scope::AppRoot,
            Scope::AppRoot | Scope::AppValue => Scope::AppValue,
        }
    }
}

/// Value keys the library hands to `fl.value`, so that whatever sits there is
/// resolved by environment no matter how its keys are written.
///
/// Read out of the embedded chart rather than listed here, so the set always
/// describes the library version this binary ships. A plain map at one of these
/// keys renders as nothing under Helm -- `fl._renderValue` emits only scalars --
/// which is why every map at such a key can be read as an env map.
fn env_selected_value_keys() -> &'static HashSet<String> {
    static KEYS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let Ok(call) = regex::Regex::new(
            r#"include\s+"fl\.value(?:Quoted|SingleQuoted)?"\s+\(list\s+\$\s+\S+\s+\.([A-Za-z0-9_.]+)\)"#,
        ) else {
            return HashSet::new();
        };
        let mut keys = HashSet::new();
        for path in crate::assets::embedded_helm_apps_paths() {
            if !path.ends_with(".tpl") {
                continue;
            }
            let Some(source) = crate::assets::embedded_helm_apps_file(&path) else {
                continue;
            };
            for captures in call.captures_iter(source) {
                // `.storage.size` puts the env map at `size`, so only the last
                // segment names the key an env map can be found under.
                let Some(last) = captures[1].rsplit('.').next() else {
                    continue;
                };
                if !last.is_empty() {
                    keys.insert(last.to_string());
                }
            }
        }
        keys
    })
}

/// Replaces every env map in the tree with the value selected for `env`.
pub(crate) fn resolve_env_maps(value: &JsonValue, env: &str) -> JsonValue {
    resolve_env_maps_at(value, env, None, Scope::Outside).unwrap_or(JsonValue::Null)
}

/// Resolves one value, knowing the key it sits under and where it sits.
///
/// `None` means the library renders nothing here, so the caller drops the key
/// entirely -- an app that names no value for the current env has no such
/// field, rather than a field holding the unresolved branches.
fn resolve_env_maps_at(
    value: &JsonValue,
    env: &str,
    key: Option<&str>,
    scope: Scope,
) -> Option<JsonValue> {
    match value {
        JsonValue::Array(items) => Some(JsonValue::Array(
            items
                .iter()
                .filter_map(|item| resolve_env_maps_at(item, env, key, scope))
                .collect(),
        )),
        JsonValue::Object(map) => {
            if is_env_map(map, key, scope) {
                return match select_env_value(map, env) {
                    Some(selected) => resolve_env_maps_at(&selected, env, key, scope),
                    // A `fl.value` site that matches nothing and has no
                    // `_default` renders empty, and the field disappears.
                    None if is_contract_env_site(key, scope) => None,
                    // Everywhere else happ inferred the env map from key
                    // shapes alone, so it keeps what it cannot resolve rather
                    // than delete values it may have misread.
                    None => Some(value.clone()),
                };
            }
            let mut out = JsonMap::new();
            for (child_key, child) in map {
                if let Some(resolved) =
                    resolve_env_maps_at(child, env, Some(child_key), scope.enter(child_key))
                {
                    out.insert(child_key.clone(), resolved);
                }
            }
            Some(JsonValue::Object(out))
        }
        _ => Some(value.clone()),
    }
}

/// Whether `key` names a value the library resolves by environment, at a place
/// in the tree where the library is the one reading it.
fn is_contract_env_site(key: Option<&str>, scope: Scope) -> bool {
    scope == Scope::AppValue && key.is_some_and(|key| env_selected_value_keys().contains(key))
}

/// Whether a map should be read as an env map, by contract where the library
/// states one and by key shapes everywhere else.
fn is_env_map(map: &JsonMap<String, JsonValue>, key: Option<&str>, scope: Scope) -> bool {
    is_contract_env_site(key, scope) || looks_like_env_map(map)
}

/// Reports every env map the resolver would traverse whose regex keys are
/// ambiguous — the condition the chart turns into `E_ENV_REGEX_AMBIGUOUS`.
///
/// Only branches [`resolve_env_maps`] actually keeps are inspected: an
/// ambiguity inside a value that env selection discards never reaches Helm
/// either.
pub(crate) fn find_ambiguous_env_regexes(value: &JsonValue, env: &str) -> Vec<AmbiguousEnvRegex> {
    let mut found = Vec::new();
    collect_ambiguous_env_regexes(
        value,
        env,
        None,
        Scope::Outside,
        &mut Vec::new(),
        &mut found,
    );
    found
}

fn collect_ambiguous_env_regexes(
    value: &JsonValue,
    env: &str,
    key: Option<&str>,
    scope: Scope,
    path: &mut Vec<String>,
    found: &mut Vec<AmbiguousEnvRegex>,
) {
    match value {
        JsonValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                collect_ambiguous_env_regexes(item, env, key, scope, path, found);
                path.pop();
            }
        }
        JsonValue::Object(map) => {
            if is_env_map(map, key, scope) {
                let patterns = matching_env_regexes(map, env);
                if patterns.len() > 1 {
                    found.push(AmbiguousEnvRegex {
                        path: path.join("."),
                        patterns,
                    });
                }
                if let Some(selected) = select_env_value(map, env) {
                    collect_ambiguous_env_regexes(&selected, env, key, scope, path, found);
                    return;
                }
                if is_contract_env_site(key, scope) {
                    // The library renders nothing here, so nothing below can
                    // reach Helm either.
                    return;
                }
            }
            for (child_key, child) in map {
                path.push(child_key.clone());
                collect_ambiguous_env_regexes(
                    child,
                    env,
                    Some(child_key),
                    scope.enter(child_key),
                    path,
                    found,
                );
                path.pop();
            }
        }
        _ => {}
    }
}

/// Whether a map should be read as an env map rather than as plain values.
pub(crate) fn looks_like_env_map(map: &JsonMap<String, JsonValue>) -> bool {
    if map.contains_key(DEFAULT_ENV_KEY) {
        return true;
    }
    map.keys().any(|key| looks_like_regex_pattern(key))
}

/// Whether a key was written as a pattern rather than as a plain env name.
pub(crate) fn looks_like_regex_pattern(key: &str) -> bool {
    if key.is_empty() || key == DEFAULT_ENV_KEY {
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

/// Anchors an env regex key the way `_fl.getValueRegex` does: strip one leading
/// `^` and one trailing `$`, then require the pattern to span the whole env
/// name. A key of `stage` therefore never matches env `stage-eu`.
pub(crate) fn anchor_env_regex(key: &str) -> String {
    let body = key.strip_prefix('^').unwrap_or(key);
    let body = body.strip_suffix('$').unwrap_or(body);
    format!("^{body}$")
}

/// Anchored regex keys of `map` matching `env`, sorted for a stable report.
///
/// Empty when an exact `env` key exists: the chart short-circuits on it and
/// never runs the regex pass, so those keys cannot conflict.
pub(crate) fn matching_env_regexes(map: &JsonMap<String, JsonValue>, env: &str) -> Vec<String> {
    if env.is_empty() || map.contains_key(env) {
        return Vec::new();
    }
    let mut matched: Vec<String> = map
        .keys()
        .filter(|key| key.as_str() != DEFAULT_ENV_KEY)
        .filter_map(|key| {
            let anchored = anchor_env_regex(key);
            let regex = regex::Regex::new(&anchored).ok()?;
            regex.is_match(env).then_some(anchored)
        })
        .collect();
    matched.sort();
    matched.dedup();
    matched
}

/// Which key of an env map the chart picks, for reporting rather than
/// resolving.
///
/// `None` when the value at `key` is not an env map at all, or is one that
/// names nothing for `env` — the two cases a reader has to tell apart when a
/// field turns out to be absent.
pub(crate) fn app_env_selection(value: &JsonValue, env: &str, key: &str) -> Option<String> {
    let map = value.as_object()?;
    if !is_env_map(map, Some(key), Scope::AppValue) {
        return None;
    }
    if map.contains_key(env) {
        return Some(env.to_string());
    }
    if let Some(pattern) = matching_env_regexes(map, env).first() {
        return Some(pattern.clone());
    }
    map.contains_key(DEFAULT_ENV_KEY)
        .then(|| DEFAULT_ENV_KEY.to_string())
}

/// Whether the value at `key` inside an app is read as an env map at all.
pub(crate) fn is_app_env_map(value: &JsonValue, key: &str) -> bool {
    value
        .as_object()
        .is_some_and(|map| is_env_map(map, Some(key), Scope::AppValue))
}

/// Selects the value an env map yields for `env`.
///
/// `None` when the map holds no value for `env` and no `_default`, which under
/// Helm renders as empty — the caller decides whether that means "drop this
/// field" or "happ misread a plain map, keep it".
pub(crate) fn select_env_value(map: &JsonMap<String, JsonValue>, env: &str) -> Option<JsonValue> {
    if let Some(value) = map.get(env) {
        return Some(value.clone());
    }
    // Several matches are an error under Helm; happ picks the first anchored
    // pattern so callers that only report (the LSP) keep working, while callers
    // that must not guess (the exporter) check `find_ambiguous_env_regexes`.
    if let Some(pattern) = matching_env_regexes(map, env).first() {
        if let Some(value) = map
            .iter()
            .find(|(key, _)| anchor_env_regex(key) == *pattern)
            .map(|(_, value)| value)
        {
            return Some(value.clone());
        }
    }
    map.get(DEFAULT_ENV_KEY).cloned()
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
            for child in map.values() {
                walk_maps(child, on_map);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map_of(value: JsonValue) -> JsonMap<String, JsonValue> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn regex_keys_are_anchored_to_the_whole_env_name() {
        assert_eq!(anchor_env_regex("stage"), "^stage$");
        assert_eq!(anchor_env_regex("^stage-.*$"), "^stage-.*$");
        assert_eq!(anchor_env_regex("^stage"), "^stage$");
        assert_eq!(anchor_env_regex(".*-eu"), "^.*-eu$");
    }

    #[test]
    fn partial_regex_match_does_not_select_a_value() {
        // Verified against helm-apps 1.9.0: `helm template` renders 1 here,
        // because `(dev|stage)` is anchored to `^(dev|stage)$`.
        let map = map_of(json!({"_default": 1, "(dev|stage)": 7}));
        assert_eq!(select_env_value(&map, "stage-eu"), Some(json!(1)));
        assert_eq!(select_env_value(&map, "stage"), Some(json!(7)));
    }

    #[test]
    fn exact_key_wins_over_a_matching_regex() {
        let map = map_of(json!({"_default": 1, "^prod.*$": 2, "prod": 3}));
        assert_eq!(select_env_value(&map, "prod"), Some(json!(3)));
        assert_eq!(select_env_value(&map, "prod-eu"), Some(json!(2)));
    }

    #[test]
    fn default_is_the_last_resort() {
        let map = map_of(json!({"_default": 1, "^prod.*$": 2}));
        assert_eq!(select_env_value(&map, "dev"), Some(json!(1)));
    }

    #[test]
    fn map_without_any_match_selects_nothing() {
        let map = map_of(json!({"^prod.*$": 2}));
        assert_eq!(select_env_value(&map, "dev"), None);
    }

    #[test]
    fn dot_in_a_literal_looking_key_still_matches_as_a_regex() {
        // The chart treats every key of an env map as a regex; the shape
        // heuristic only decides whether the map is an env map at all.
        let map = map_of(json!({"_default": 1, "a.c": 2}));
        assert_eq!(select_env_value(&map, "abc"), Some(json!(2)));
    }

    #[test]
    fn matching_regexes_are_reported_sorted_and_anchored() {
        let map = map_of(json!({"_default": 1, "^stage-.*$": 2, ".*-eu": 3}));
        assert_eq!(
            matching_env_regexes(&map, "stage-eu"),
            vec!["^.*-eu$".to_string(), "^stage-.*$".to_string()],
        );
    }

    #[test]
    fn exact_key_suppresses_the_ambiguity_report() {
        let map = map_of(json!({"stage-eu": 9, "^stage-.*$": 2, ".*-eu": 3}));
        assert!(matching_env_regexes(&map, "stage-eu").is_empty());
        assert_eq!(select_env_value(&map, "stage-eu"), Some(json!(9)));
    }

    #[test]
    fn empty_env_never_matches_a_regex() {
        let map = map_of(json!({"_default": 1, "^.*$": 2}));
        assert!(matching_env_regexes(&map, "").is_empty());
        assert_eq!(select_env_value(&map, ""), Some(json!(1)));
    }

    #[test]
    fn ambiguous_env_regexes_are_found_with_their_path() {
        let values = json!({
            "global": {"env": "stage-eu"},
            "apps-stateless": {"api": {"replicas": {
                "_default": 1, "^stage-.*$": 2, ".*-eu": 3,
            }}},
        });
        let found = find_ambiguous_env_regexes(&values, "stage-eu");
        assert_eq!(
            found,
            vec![AmbiguousEnvRegex {
                path: "apps-stateless.api.replicas".to_string(),
                patterns: vec!["^.*-eu$".to_string(), "^stage-.*$".to_string()],
            }],
        );
    }

    #[test]
    fn ambiguity_inside_a_discarded_branch_is_not_reported() {
        let values = json!({"replicas": {
            "_default": 1,
            "prod": {"nested": {"_default": 5, "^pr.*$": 6, ".*d$": 7}},
        }});
        // env `dev` selects `_default`, so the conflict under `prod` is dead.
        assert!(find_ambiguous_env_regexes(&values, "dev").is_empty());
        // env `prod` selects that branch, and the conflict becomes reachable.
        assert_eq!(
            find_ambiguous_env_regexes(&values, "prod"),
            vec![AmbiguousEnvRegex {
                path: "replicas.nested".to_string(),
                patterns: vec!["^.*d$".to_string(), "^pr.*$".to_string()],
            }],
        );
    }

    #[test]
    fn ambiguity_message_matches_the_chart_wording() {
        let message = ambiguous_env_regex_message(&AmbiguousEnvRegex {
            path: "apps-stateless.api.replicas".to_string(),
            patterns: vec!["^.*-eu$".to_string(), "^stage-.*$".to_string()],
        });
        assert!(message.contains("[helm-apps:E_ENV_REGEX_AMBIGUOUS]"));
        assert!(message
            .contains("multiple env regex keys match current global.env: [^.*-eu$ ^stage-.*$]"));
        assert!(message.contains("path=apps-stateless.api.replicas"));
        assert!(message.contains("hint=leave only one matching regex key for this value"));
        assert!(message.contains("docs=docs/reference-values.md#param-global-env"));
    }

    #[test]
    fn resolve_replaces_env_maps_throughout_the_tree() {
        let values = json!({
            "apps-stateless": {"api": {
                "replicas": {"_default": 1, "^prod.*$": 4},
                "list": [{"_default": "a", "prod": "b"}],
            }},
        });
        assert_eq!(
            resolve_env_maps(&values, "prod-eu"),
            json!({"apps-stateless": {"api": {"replicas": 4, "list": ["a"]}}}),
        );
    }

    #[test]
    fn resolve_keeps_a_map_that_selects_nothing() {
        let values = json!({"ports": {"^prod.*$": 8080}});
        assert_eq!(
            resolve_env_maps(&values, "dev"),
            json!({"ports": {"^prod.*$": 8080}}),
        );
    }

    /// The library derives this set from its own templates, so at minimum the
    /// keys the reported chart tripped over have to be in it.
    #[test]
    fn env_selected_keys_come_from_the_embedded_chart() {
        let keys = env_selected_value_keys();
        for key in ["priorityClassName", "replicas", "annotations", "labels"] {
            assert!(keys.contains(key), "{key} missing from {keys:?}");
        }
        // `.storage.size` names its env map `size`, not `storage.size`.
        assert!(keys.contains("size"), "{keys:?}");
        assert!(!keys.contains("storage.size"), "{keys:?}");
    }

    /// `priorityClassName: {dc1: ...}` has neither `_default` nor a
    /// regex-shaped key, so the shape heuristic read it as plain values while
    /// both renderers resolved it. The library decides by call site, not shape.
    #[test]
    fn a_literal_only_env_map_resolves_at_a_library_value_site() {
        let values = json!({
            "apps-stateless": {"api": {"priorityClassName": {"dc1": "production-medium"}}},
        });
        assert_eq!(
            resolve_env_maps(&values, "dc1"),
            json!({"apps-stateless": {"api": {"priorityClassName": "production-medium"}}}),
        );
        // No key for `dev` and no `_default`: `fl.value` renders empty, so the
        // field is simply absent -- which is what `helm template` produces.
        assert_eq!(
            resolve_env_maps(&values, "dev"),
            json!({"apps-stateless": {"api": {}}}),
        );
    }

    /// The same key outside an app is read by the library directly, never
    /// through `fl.value`: `global.labels.addEnv` must survive intact.
    #[test]
    fn a_library_value_key_outside_an_app_is_left_alone() {
        let values = json!({"global": {"labels": {"addEnv": "true"}}});
        assert_eq!(
            resolve_env_maps(&values, "dc1"),
            json!({"global": {"labels": {"addEnv": "true"}}}),
        );
    }

    /// Only the app's own values are contract sites; the group map holding
    /// them is not one, whatever the apps are called.
    #[test]
    fn an_app_named_like_a_library_value_key_is_not_collapsed() {
        let values = json!({"apps-stateless": {"labels": {"replicas": 2}}});
        assert_eq!(
            resolve_env_maps(&values, "dc1"),
            json!({"apps-stateless": {"labels": {"replicas": 2}}}),
        );
    }

    #[test]
    fn discovery_splits_literal_and_regex_keys() {
        let values = json!({
            "global": {"env": "dev"},
            "replicas": {"_default": 1, "prod": 2, "^stage-.*$": 3},
        });
        let discovery = discover_environments(&values);
        assert_eq!(
            discovery.literals,
            vec!["dev".to_string(), "prod".to_string()]
        );
        assert_eq!(discovery.regexes, vec!["^stage-.*$".to_string()]);
    }

    #[test]
    fn default_env_prefers_global_env_then_first_literal() {
        let with_global = json!({"global": {"env": "prod"}, "replicas": {"_default": 1, "dev": 2}});
        let discovery = discover_environments(&with_global);
        assert_eq!(detect_default_env(&with_global, &discovery), "prod");

        let without_global = json!({"replicas": {"_default": 1, "qa": 2}});
        let discovery = discover_environments(&without_global);
        assert_eq!(detect_default_env(&without_global, &discovery), "qa");

        let empty = json!({});
        let discovery = discover_environments(&empty);
        assert_eq!(detect_default_env(&empty, &discovery), "dev");
    }
}
