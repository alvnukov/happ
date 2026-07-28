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
//! Two responsibilities are deliberately kept apart. *Detecting* that a map is
//! an env map at all is happ-specific guesswork — the chart knows it from the
//! call site, happ has to infer it from the key shapes, which is what
//! [`looks_like_regex_pattern`] is for. *Selecting* inside a map that already
//! looks like an env map follows the chart literally: every key is a regex
//! there, so `a.c` matches env `abc` the same way it does under Helm.

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

/// Replaces every env map in the tree with the value selected for `env`.
pub(crate) fn resolve_env_maps(value: &JsonValue, env: &str) -> JsonValue {
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
                // `select_env_value` hands the map back untouched when nothing
                // matched and there is no `_default`; recursing on that would
                // never terminate, so descend into its entries instead.
                if selected != JsonValue::Object(map.clone()) {
                    return resolve_env_maps(&selected, env);
                }
            }
            let mut out = JsonMap::new();
            for (key, child) in map {
                out.insert(key.clone(), resolve_env_maps(child, env));
            }
            JsonValue::Object(out)
        }
        _ => value.clone(),
    }
}

/// Reports every env map the resolver would traverse whose regex keys are
/// ambiguous — the condition the chart turns into `E_ENV_REGEX_AMBIGUOUS`.
///
/// Only branches [`resolve_env_maps`] actually keeps are inspected: an
/// ambiguity inside a value that env selection discards never reaches Helm
/// either.
pub(crate) fn find_ambiguous_env_regexes(value: &JsonValue, env: &str) -> Vec<AmbiguousEnvRegex> {
    let mut found = Vec::new();
    collect_ambiguous_env_regexes(value, env, &mut Vec::new(), &mut found);
    found
}

fn collect_ambiguous_env_regexes(
    value: &JsonValue,
    env: &str,
    path: &mut Vec<String>,
    found: &mut Vec<AmbiguousEnvRegex>,
) {
    match value {
        JsonValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                collect_ambiguous_env_regexes(item, env, path, found);
                path.pop();
            }
        }
        JsonValue::Object(map) => {
            if looks_like_env_map(map) {
                let patterns = matching_env_regexes(map, env);
                if patterns.len() > 1 {
                    found.push(AmbiguousEnvRegex {
                        path: path.join("."),
                        patterns,
                    });
                }
                let selected = select_env_value(map, env);
                if selected != JsonValue::Object(map.clone()) {
                    collect_ambiguous_env_regexes(&selected, env, path, found);
                    return;
                }
            }
            for (key, child) in map {
                path.push(key.clone());
                collect_ambiguous_env_regexes(child, env, path, found);
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

/// Selects the value an env map yields for `env`.
///
/// Returns the map itself when it turned out to hold no value for `env` and no
/// `_default` — the caller decides whether to treat it as plain values.
pub(crate) fn select_env_value(map: &JsonMap<String, JsonValue>, env: &str) -> JsonValue {
    if let Some(value) = map.get(env) {
        return value.clone();
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
            return value.clone();
        }
    }
    if let Some(value) = map.get(DEFAULT_ENV_KEY) {
        return value.clone();
    }
    JsonValue::Object(map.clone())
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
        assert_eq!(select_env_value(&map, "stage-eu"), json!(1));
        assert_eq!(select_env_value(&map, "stage"), json!(7));
    }

    #[test]
    fn exact_key_wins_over_a_matching_regex() {
        let map = map_of(json!({"_default": 1, "^prod.*$": 2, "prod": 3}));
        assert_eq!(select_env_value(&map, "prod"), json!(3));
        assert_eq!(select_env_value(&map, "prod-eu"), json!(2));
    }

    #[test]
    fn default_is_the_last_resort() {
        let map = map_of(json!({"_default": 1, "^prod.*$": 2}));
        assert_eq!(select_env_value(&map, "dev"), json!(1));
    }

    #[test]
    fn map_without_any_match_is_returned_untouched() {
        let map = map_of(json!({"^prod.*$": 2}));
        assert_eq!(select_env_value(&map, "dev"), json!({"^prod.*$": 2}));
    }

    #[test]
    fn dot_in_a_literal_looking_key_still_matches_as_a_regex() {
        // The chart treats every key of an env map as a regex; the shape
        // heuristic only decides whether the map is an env map at all.
        let map = map_of(json!({"_default": 1, "a.c": 2}));
        assert_eq!(select_env_value(&map, "abc"), json!(2));
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
        assert_eq!(select_env_value(&map, "stage-eu"), json!(9));
    }

    #[test]
    fn empty_env_never_matches_a_regex() {
        let map = map_of(json!({"_default": 1, "^.*$": 2}));
        assert!(matching_env_regexes(&map, "").is_empty());
        assert_eq!(select_env_value(&map, ""), json!(1));
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
