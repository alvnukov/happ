//! The embedded helm-apps chart, published as readable MCP resources.
//!
//! Every fact happ knows about the library -- which `apps-*` groups exist, how
//! `fl.value` picks an env, what `_include` expands to -- ultimately comes from
//! these templates. Handing the model the source lets it check the contract
//! against the version actually vendored into this binary rather than against
//! whatever it remembers about helm-apps.

use serde_json::{json, Value as JsonValue};

use super::protocol::Failure;

const URI_PREFIX: &str = "happ://helm-apps/";

pub(crate) fn catalog() -> Vec<JsonValue> {
    crate::assets::embedded_helm_apps_paths()
        .into_iter()
        .map(|path| {
            json!({
                "uri": format!("{URI_PREFIX}{path}"),
                "name": path.clone(),
                "title": format!("helm-apps: {path}"),
                "description": describe(&path),
                "mimeType": mime_type(&path),
            })
        })
        .collect()
}

pub(crate) fn read(params: &JsonValue) -> Result<JsonValue, Failure> {
    let uri = params
        .get("uri")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| Failure::invalid_params("resources/read requires a 'uri'"))?;

    let path = uri.strip_prefix(URI_PREFIX).ok_or_else(|| {
        Failure::invalid_params(format!(
            "unknown resource '{uri}' -- happ serves only {URI_PREFIX}* resources"
        ))
    })?;

    let contents = crate::assets::embedded_helm_apps_file(path).ok_or_else(|| {
        Failure::invalid_params(format!(
            "no such file in the embedded helm-apps chart: {path}"
        ))
    })?;

    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": mime_type(path),
            "text": contents,
        }],
    }))
}

fn mime_type(path: &str) -> &'static str {
    if path.ends_with(".yaml") || path.ends_with(".yml") {
        "application/yaml"
    } else {
        // .tpl files are Go templates; no registered type fits, and text/plain
        // is what keeps clients from trying to parse them.
        "text/plain"
    }
}

/// A one-line orientation for the file, so a model can pick the right template
/// out of the listing without reading all of them.
fn describe(path: &str) -> String {
    if path == "Chart.yaml" {
        return "Library chart metadata: name, version, type.".to_string();
    }
    if path == "values.yaml" {
        return "Default values of the library chart itself.".to_string();
    }
    if let Some(name) = path
        .strip_prefix("templates/fl-functions/_")
        .and_then(|rest| rest.strip_suffix(".tpl"))
    {
        return format!("Library function `fl.{name}`.");
    }
    if let Some(name) = path
        .strip_prefix("templates/fl-snippets/_")
        .and_then(|rest| rest.strip_suffix(".tpl"))
    {
        return format!("Manifest snippet helper `fl.{name}`.");
    }
    if let Some(group) = path
        .strip_prefix("templates/_")
        .and_then(|rest| rest.strip_suffix(".tpl"))
    {
        return format!("Templates behind the `{group}` values group.");
    }
    format!("helm-apps chart file {path}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_the_embedded_chart() {
        let listed = catalog();
        assert!(!listed.is_empty(), "no embedded resources published");
        assert!(listed
            .iter()
            .any(|entry| entry["uri"] == format!("{URI_PREFIX}Chart.yaml")));
        for entry in &listed {
            assert!(entry["uri"]
                .as_str()
                .is_some_and(|uri| uri.starts_with(URI_PREFIX)));
            assert!(entry["mimeType"].is_string());
        }
    }

    #[test]
    fn reading_a_listed_resource_returns_its_text() {
        let result = read(&json!({ "uri": format!("{URI_PREFIX}Chart.yaml") })).expect("read");
        let text = result["contents"][0]["text"].as_str().expect("text");
        assert!(text.contains("version"));
    }

    #[test]
    fn reading_outside_the_embedded_chart_is_refused() {
        let escape = read(&json!({ "uri": format!("{URI_PREFIX}../../etc/passwd") }));
        assert!(escape.is_err(), "path traversal must not resolve");

        let foreign = read(&json!({ "uri": "file:///etc/passwd" }));
        assert!(foreign.is_err(), "non-happ URIs must not resolve");
    }

    #[test]
    fn descriptions_name_the_library_function_a_template_defines() {
        assert_eq!(
            describe("templates/fl-functions/_value.tpl"),
            "Library function `fl.value`."
        );
        assert_eq!(
            describe("templates/_apps-stateless.tpl"),
            "Templates behind the `apps-stateless` values group."
        );
    }
}
