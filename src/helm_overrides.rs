//! Helm's `--values`, `--set` and `--set-string` applied to a values tree.
//!
//! A helm-apps chart is rarely rendered from its own values alone: CI passes
//! further `-f` files and `--set` flags, and an analysis that ignores them
//! answers about a chart nobody deploys. This mirrors what Helm does to the
//! values before templating, so `resolve`, `query`, `lint` and `diff` see the
//! same tree `helm template` would.
//!
//! Only the value layer is modelled. Helm applies files in the order given and
//! `--set` after all of them, which is the order [`ValueOverrides::apply`]
//! follows.

use serde_json::{Map as JsonMap, Value as JsonValue};
use std::path::{Path, PathBuf};

/// Caller-supplied overrides, in the order Helm applies them.
#[derive(Debug, Default, Clone)]
pub(crate) struct ValueOverrides {
    /// `--values` / `-f`, applied in order; a later file wins.
    pub(crate) files: Vec<PathBuf>,
    /// `--set`, whose values keep whatever type they were written with.
    pub(crate) set: Vec<(String, JsonValue)>,
    /// `--set-string`, whose values are always strings.
    pub(crate) set_string: Vec<(String, String)>,
}

impl ValueOverrides {
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty() && self.set.is_empty() && self.set_string.is_empty()
    }

    /// Layers every override onto `root`, in Helm's order.
    pub(crate) fn apply(&self, root: &mut JsonMap<String, JsonValue>) -> Result<(), String> {
        for file in &self.files {
            let overlay = read_values_file(file)?;
            merge_maps(root, overlay);
        }
        for (path, value) in &self.set {
            assign_path(root, path, value.clone())?;
        }
        for (path, value) in &self.set_string {
            assign_path(root, path, JsonValue::String(value.clone()))?;
        }
        Ok(())
    }
}

fn read_values_file(path: &Path) -> Result<JsonMap<String, JsonValue>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("read values file '{}': {err}", path.display()))?;
    // An empty `-f` file is legal and contributes nothing, but YAML parses it
    // as null rather than as an empty map.
    let parsed: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|err| format!("parse values file '{}': {err}", path.display()))?;
    if parsed.is_null() {
        return Ok(JsonMap::new());
    }
    let json: JsonValue = serde_json::to_value(parsed)
        .map_err(|err| format!("convert values file '{}': {err}", path.display()))?;
    match json {
        JsonValue::Object(map) => Ok(map),
        _ => Err(format!(
            "values file '{}' must hold a YAML map at its root",
            path.display()
        )),
    }
}

/// Layers `source` onto `target` the way Helm's own `mergeMaps` does: maps are
/// merged key by key, and anything else replaces what was there outright.
///
/// A list therefore never merges element-wise -- overriding one entry of a list
/// means restating the list, which is the behaviour that surprises people about
/// Helm but is the behaviour they get.
pub(crate) fn merge_maps(
    target: &mut JsonMap<String, JsonValue>,
    source: JsonMap<String, JsonValue>,
) {
    for (key, value) in source {
        match (target.get_mut(&key), value) {
            (Some(JsonValue::Object(existing)), JsonValue::Object(overlay)) => {
                merge_maps(existing, overlay);
            }
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

/// One step of a `--set` path.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

/// Splits a `--set` path into its steps.
///
/// `\.` keeps a dot inside a key, which charts with keys such as
/// `config.yaml` need, and `[0]` indexes into a list.
fn parse_path(path: &str) -> Result<Vec<Step>, String> {
    if path.trim().is_empty() {
        return Err("empty --set path".to_string());
    }
    let mut steps = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped) => current.push(escaped),
                None => return Err(format!("--set path '{path}' ends in a trailing backslash")),
            },
            '.' => {
                // The dot in `a[0].b` only separates; the index already ended
                // the step, so there is no key to close here.
                if current.is_empty() && matches!(steps.last(), Some(Step::Index(_))) {
                    continue;
                }
                steps.push(Step::Key(std::mem::take(&mut current)));
            }
            '[' => {
                if !current.is_empty() || steps.is_empty() {
                    steps.push(Step::Key(std::mem::take(&mut current)));
                }
                let mut digits = String::new();
                loop {
                    match chars.next() {
                        Some(']') => break,
                        Some(digit) => digits.push(digit),
                        None => return Err(format!("--set path '{path}' has an unclosed '['")),
                    }
                }
                let index: usize = digits.parse().map_err(|_| {
                    format!("--set path '{path}' has a non-numeric index '{digits}'")
                })?;
                steps.push(Step::Index(index));
            }
            _ => current.push(ch),
        }
    }
    steps.push(Step::Key(current));

    // A path ending in `]` leaves an empty trailing key behind.
    if matches!(steps.last(), Some(Step::Key(key)) if key.is_empty()) && steps.len() > 1 {
        steps.pop();
    }
    if steps
        .iter()
        .any(|step| matches!(step, Step::Key(key) if key.is_empty()))
    {
        return Err(format!("--set path '{path}' has an empty key"));
    }
    Ok(steps)
}

/// Writes `value` at `path`, creating the maps and lists along the way.
fn assign_path(
    root: &mut JsonMap<String, JsonValue>,
    path: &str,
    value: JsonValue,
) -> Result<(), String> {
    let steps = parse_path(path)?;
    let Some((Step::Key(first), rest)) = steps.split_first() else {
        return Err(format!("--set path '{path}' must start with a key"));
    };
    let slot = root.entry(first.clone()).or_insert(JsonValue::Null);
    assign_steps(slot, rest, value, path)
}

fn assign_steps(
    slot: &mut JsonValue,
    steps: &[Step],
    value: JsonValue,
    path: &str,
) -> Result<(), String> {
    let Some((step, rest)) = steps.split_first() else {
        *slot = value;
        return Ok(());
    };
    match step {
        Step::Key(key) => {
            // Anything that is not already a map is replaced, because `--set`
            // states what the value is rather than asking to merge into it.
            if !slot.is_object() {
                *slot = JsonValue::Object(JsonMap::new());
            }
            let map = slot
                .as_object_mut()
                .ok_or_else(|| format!("--set path '{path}' could not open a map at '{key}'"))?;
            let child = map.entry(key.clone()).or_insert(JsonValue::Null);
            assign_steps(child, rest, value, path)
        }
        Step::Index(index) => {
            if !slot.is_array() {
                *slot = JsonValue::Array(Vec::new());
            }
            let items = slot
                .as_array_mut()
                .ok_or_else(|| format!("--set path '{path}' could not open a list at [{index}]"))?;
            if *index >= items.len() {
                // Helm grows a list to reach the index, leaving holes null.
                items.resize(index + 1, JsonValue::Null);
            }
            let Some(child) = items.get_mut(*index) else {
                return Err(format!("--set path '{path}' index {index} is out of range"));
            };
            assign_steps(child, rest, value, path)
        }
    }
}

/// Reads a `--set` value the way Helm types it: `null`, `true`/`false` and
/// whole numbers keep their type, anything else stays a string.
///
/// A leading zero marks a string, so a zero-padded value such as an octal file
/// mode survives instead of turning into a different number.
pub(crate) fn typed_set_value(raw: &str) -> JsonValue {
    match raw {
        "null" => return JsonValue::Null,
        "true" => return JsonValue::Bool(true),
        "false" => return JsonValue::Bool(false),
        _ => {}
    }
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if digits.len() > 1 && digits.starts_with('0') {
        return JsonValue::String(raw.to_string());
    }
    match raw.parse::<i64>() {
        Ok(number) => JsonValue::Number(number.into()),
        Err(_) => JsonValue::String(raw.to_string()),
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
    fn maps_merge_key_by_key_and_lists_replace() {
        let mut target = map_of(json!({
            "global": {"env": "dev", "vars": {"A": 1, "B": 2}},
            "ports": [80, 443],
        }));
        merge_maps(
            &mut target,
            map_of(json!({
                "global": {"vars": {"B": 20, "C": 30}},
                "ports": [8080],
            })),
        );
        assert_eq!(
            JsonValue::Object(target),
            json!({
                "global": {"env": "dev", "vars": {"A": 1, "B": 20, "C": 30}},
                "ports": [8080],
            }),
        );
    }

    #[test]
    fn a_scalar_replaces_a_map_outright() {
        let mut target = map_of(json!({"tls": {"enabled": true}}));
        merge_maps(&mut target, map_of(json!({"tls": "off"})));
        assert_eq!(JsonValue::Object(target), json!({"tls": "off"}));
    }

    #[test]
    fn set_paths_split_on_dots_and_indices() {
        assert_eq!(
            parse_path("a.b").expect("path"),
            vec![Step::Key("a".into()), Step::Key("b".into())],
        );
        assert_eq!(
            parse_path("a[2].b").expect("path"),
            vec![Step::Key("a".into()), Step::Index(2), Step::Key("b".into())],
        );
        assert_eq!(
            parse_path("a[0]").expect("path"),
            vec![Step::Key("a".into()), Step::Index(0)]
        );
    }

    /// helm-apps charts really do carry keys such as `config.yaml`, so the
    /// escape is not academic.
    #[test]
    fn an_escaped_dot_stays_inside_the_key() {
        assert_eq!(
            parse_path(r"files.config\.yaml.content").expect("path"),
            vec![
                Step::Key("files".into()),
                Step::Key("config.yaml".into()),
                Step::Key("content".into()),
            ],
        );
    }

    #[test]
    fn malformed_set_paths_are_rejected() {
        assert!(parse_path("").is_err());
        assert!(parse_path("a..b").is_err());
        assert!(parse_path("a[1").is_err());
        assert!(parse_path("a[x]").is_err());
        assert!(parse_path(r"a\").is_err());
    }

    #[test]
    fn set_creates_the_path_it_writes_to() {
        let mut root = JsonMap::new();
        assign_path(&mut root, "global.vars.HOST", json!("0.0.0.0")).expect("assign");
        assert_eq!(
            JsonValue::Object(root),
            json!({"global": {"vars": {"HOST": "0.0.0.0"}}}),
        );
    }

    #[test]
    fn set_grows_a_list_to_reach_its_index() {
        let mut root = JsonMap::new();
        assign_path(&mut root, "hosts[2].name", json!("c")).expect("assign");
        assert_eq!(
            JsonValue::Object(root),
            json!({"hosts": [null, null, {"name": "c"}]}),
        );
    }

    #[test]
    fn set_replaces_whatever_stood_in_the_way() {
        let mut root = map_of(json!({"image": "nginx"}));
        assign_path(&mut root, "image.tag", json!("1.2")).expect("assign");
        assert_eq!(JsonValue::Object(root), json!({"image": {"tag": "1.2"}}));
    }

    #[test]
    fn set_values_are_typed_the_way_helm_types_them() {
        assert_eq!(typed_set_value("null"), JsonValue::Null);
        assert_eq!(typed_set_value("true"), json!(true));
        assert_eq!(typed_set_value("3"), json!(3));
        assert_eq!(typed_set_value("-3"), json!(-3));
        assert_eq!(typed_set_value("1.2"), json!("1.2"));
        assert_eq!(typed_set_value("0755"), json!("0755"));
        assert_eq!(typed_set_value("0"), json!(0));
        assert_eq!(typed_set_value("api"), json!("api"));
    }

    #[test]
    fn overrides_apply_files_first_then_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = dir.path().join("a.yaml");
        std::fs::write(&first, "global:\n  env: dev\n  vars:\n    A: 1\n").expect("write");
        let second = dir.path().join("b.yaml");
        std::fs::write(&second, "global:\n  env: dc1\n").expect("write");

        let overrides = ValueOverrides {
            files: vec![first, second],
            set: vec![("global.vars.A".to_string(), json!(9))],
            set_string: vec![("global.vars.B".to_string(), "2".to_string())],
        };
        let mut root = JsonMap::new();
        overrides.apply(&mut root).expect("apply");
        assert_eq!(
            JsonValue::Object(root),
            json!({"global": {"env": "dc1", "vars": {"A": 9, "B": "2"}}}),
        );
    }

    #[test]
    fn an_empty_values_file_contributes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = dir.path().join("empty.yaml");
        std::fs::write(&empty, "# nothing here\n").expect("write");
        let overrides = ValueOverrides {
            files: vec![empty],
            ..ValueOverrides::default()
        };
        let mut root = map_of(json!({"global": {"env": "dev"}}));
        overrides.apply(&mut root).expect("apply");
        assert_eq!(JsonValue::Object(root), json!({"global": {"env": "dev"}}));
    }

    #[test]
    fn a_missing_values_file_is_reported_with_its_path() {
        let overrides = ValueOverrides {
            files: vec![PathBuf::from("/nonexistent/values.yaml")],
            ..ValueOverrides::default()
        };
        let err = overrides
            .apply(&mut JsonMap::new())
            .err()
            .expect("missing file must fail");
        assert!(err.contains("/nonexistent/values.yaml"), "{err}");
    }

    #[test]
    fn a_values_file_that_is_not_a_map_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let list = dir.path().join("list.yaml");
        std::fs::write(&list, "- a\n- b\n").expect("write");
        let overrides = ValueOverrides {
            files: vec![list],
            ..ValueOverrides::default()
        };
        let err = overrides
            .apply(&mut JsonMap::new())
            .err()
            .expect("a list root must fail");
        assert!(err.contains("must hold a YAML map"), "{err}");
    }
}
