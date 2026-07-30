//! The tools a model calls to understand a helm-apps chart.
//!
//! Every tool answers in text rather than in JSON blobs: a model reading a
//! chart is reading YAML anyway, and a rendered report costs far fewer tokens
//! than the same facts wrapped in objects. Failures that the model could act on
//! -- a chart path that is not a chart, an app that does not exist -- come back
//! as `isError` content with the reason, not as JSON-RPC errors, so the model
//! can correct the call instead of seeing the connection fault.

use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{limit_schema, optional_str, optional_u32, required_str, truncate};
use crate::helm_overrides::ValueOverrides;
use crate::lsp::{ChartValuesSource, ReleaseIdentity};
use crate::mcp::ServerContext;

pub(crate) const NAME: &str = "helm_apps";

pub(crate) fn tool() -> JsonValue {
    json!({
        "name": NAME,
        "description": "\
    Understand a Helm chart built on the helm-apps library chart. Such a chart has no per-app \
    templates: every app is a values entry under an `apps-*` group (say `apps-stateless.api`), and \
    the library renders it. Reading values.yaml directly misleads, because `_include` profiles, \
`_includeFile` references and env maps all resolve at render time.

  overview                              groups, apps, environments, library version, violations
  apps                                  every app as group.app, with its enabled state
  resolve   group+app                   the app's values as the library actually sees them
  origin    group+app                   where each of those values came from
  render    group+app                   the Kubernetes manifests the app produces
  query_manifests query                 jq over manifests from every enabled app
  lint                                  violations of the helm-apps contract
  diff      group+app+from_env+to_env    how the app differs between two environments
  query     query                       jq over the whole resolved values tree
  contract                              the library's own rules: groups, functions, env selection
  template  name                        source of a library template or `define`

Start with overview. `chart` may be omitted when happ was started with --chart.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": [
                        "overview", "apps", "resolve", "render", "query_manifests", "origin",
                        "lint", "diff", "query", "contract", "template",
                    ],
                    "description": "Operation to run.",
                },
                "chart": chart_schema(),
                "env": env_schema(),
                "group": group_schema(),
                "app": app_schema(),
                "from_env": {
                    "type": "string",
                    "description": "For op='diff': the baseline environment, e.g. 'dev'.",
                },
                "to_env": {
                    "type": "string",
                    "description": "For op='diff': the environment compared against it.",
                },
                "query": {
                    "type": "string",
                    "description": "For op='query': a jq expression over the resolved values root, \
                                    e.g. '[.[\"apps-stateless\"] | to_entries[] | select(.value.enabled) | .key]'. \
                                    Group names contain '-', which jq reads as subtraction, so quote \
                                    them: .[\"apps-stateless\"]. For op='query_manifests': jq over an array \
                                    of {group, app, manifest} records from every enabled app. For \
                                    op='contract' with name='functions': keep only functions whose name \
                                    contains this.",
                },
                "name": {
                    "type": "string",
                    "description": "For op='template': a library template path or a `define` name \
                                    such as 'fl.value'. For op='contract': one of groups, \
                                    functions, env.",
                },
                "values_path": {
                    "type": "string",
                    "description": "For op='resolve' and op='origin': dot-separated subpath inside \
                                    the app, e.g. 'containers.main.env'. Keys containing dots need \
                                    op='query'.",
                },
                "apply_includes": {
                    "type": "boolean",
                    "description": "For op='resolve': expand `_include` profiles (default true). \
                                    False shows the app's values as literally written. Files named \
                                    by `_include_files` are loaded either way, since that is where \
                                    a modular chart keeps its apps.",
                },
                "apply_env_resolution": {
                    "type": "boolean",
                    "description": "For op='resolve': collapse env maps (default true). False \
                                    shows every environment's branch side by side.",
                },
                "renderer": {
                    "type": "string",
                    "enum": ["fast", "helm", "werf"],
                    "description": "For op='render': 'fast' (default) links helm's own engine and \
                                    needs no tooling on PATH; 'helm' and 'werf' shell out to the \
                                    real binaries. All three are given the same values.",
                },
                "set_file": {
                    "type": "object",
                    "description": "Values read from files, the way `helm --set-file` does: keys \
                                    are dotted paths, values are file paths. For anything with \
                                    newlines in it, such as a CA bundle.",
                },
                "kind": {
                    "type": "string",
                    "description": "For op='render' or op='query_manifests': keep only this Kubernetes \
                                    kind, e.g. 'Deployment'. The manifest query applies this filter \
                                    before jq so unrelated documents never enter the query input.",
                },
                "resource": {
                    "type": "string",
                    "description": "For op='render' or op='query_manifests': keep only the resource \
                                    with this metadata.name. Combines with kind and is applied before jq.",
                },
                "values_files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Extra values files layered on the chart's own, in order, the \
                                    way `helm -f` does. Relative paths resolve against the chart \
                                    directory. Applies to every op.",
                },
                "set": {
                    "type": "object",
                    "description": "Values to override, the way `helm --set` does: keys are dotted \
                                    paths such as 'global.vars.HOST' or 'hosts[0].name', with \
                                    '\\\\.' keeping a dot inside a key. Applied after values_files. \
                                    Applies to every op.",
                },
                "set_string": {
                    "type": "object",
                    "description": "Like `set`, but every value stays a string, the way \
                                    `helm --set-string` does.",
                },
                "release_name": {
                    "type": "string",
                    "description": "For op='render': the Helm release name, read by templates as \
                                    `$.Release.Name`.",
                },
                "namespace": {
                    "type": "string",
                    "description": "For op='render': the namespace to render into, read as \
                                    `$.Release.Namespace`. helm-apps stamps it onto the bindings \
                                    it generates, and charts commonly derive values from it.",
                },
                "limit": limit_schema("output lines"),
            },
            "required": ["op"],
        },
    })
}

pub(crate) fn call(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let op = required_str(args, "op")?;
    let text = match op.as_str() {
        "overview" => chart_overview(context, args)?,
        "apps" => list_entities(context, args)?,
        "resolve" => resolve_entity(context, args)?,
        // Capped by the op itself: it knows the answer is a list of named
        // resources, so it can say what a cap left out.
        "render" => return render_entity_manifest(context, args),
        "query_manifests" => query_manifests(context, args)?,
        "origin" => explain_value_origin(context, args)?,
        "lint" => lint_values(context, args)?,
        "diff" => diff_entity_envs(context, args)?,
        "query" => query_values(context, args)?,
        "contract" => library_reference(args)?,
        "template" => library_template(args)?,
        other => {
            return Err(format!(
                "unknown op '{other}' -- expected one of: overview, apps, resolve, render, \
                 query_manifests, origin, lint, diff, query, contract, template"
            ))
        }
    };
    Ok(truncate(text, args, "output lines"))
}

fn chart_schema() -> JsonValue {
    json!({
        "type": "string",
        "description": "Path to the chart directory, or to a values file inside it. Optional when \
                        happ was started with --chart.",
    })
}

fn env_schema() -> JsonValue {
    json!({
        "type": "string",
        "description": "Environment to resolve for (the chart's `global.env`). Defaults to the \
                        chart's own global.env; op='overview' lists the ones it declares.",
    })
}

fn group_schema() -> JsonValue {
    json!({
        "type": "string",
        "description": "Values group holding the app, e.g. 'apps-stateless'.",
    })
}

fn app_schema() -> JsonValue {
    json!({ "type": "string", "description": "App name inside the group, e.g. 'api'." })
}

// --- tools ------------------------------------------------------------------

fn chart_overview(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let entities = crate::lsp::analysis_list_entities(&source, optional_str(args, "env"))?;
    let diagnostics = crate::lsp::analysis_diagnostics(&source)?;
    let metadata = chart_metadata(&source);

    let enabled: Vec<(String, String)> = entities["enabledEntities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    Some((
                        entry["group"].as_str()?.to_string(),
                        entry["app"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut groups = serde_json::Map::new();
    if let Some(listed) = entities["groups"].as_array() {
        for group in listed {
            let Some(group_name) = group["name"].as_str() else {
                continue;
            };
            let mut apps = serde_json::Map::new();
            for app in group["apps"].as_array().unwrap_or(&Vec::new()) {
                let Some(app_name) = app.as_str() else {
                    continue;
                };
                let state = if enabled
                    .iter()
                    .any(|(g, a)| g == group_name && a == app_name)
                {
                    "enabled"
                } else {
                    "disabled"
                };
                apps.insert(app_name.to_string(), JsonValue::String(state.to_string()));
            }
            groups.insert(group_name.to_string(), JsonValue::Object(apps));
        }
    }

    let report = json!({
        "chart": {
            "name": metadata.name,
            "version": metadata.version,
            "root": source.chart_root.display().to_string(),
            "valuesFile": source.values_path.display().to_string(),
        },
        "library": {
            "helmAppsEmbeddedInHapp": crate::assets::embedded_helm_apps_version(),
            "helmAppsDeclaredByChart": metadata.helm_apps_dependency,
        },
        "environments": {
            "resolvedFor": entities["usedEnv"].clone(),
            "chartDefault": entities["defaultEnv"].clone(),
            "declared": entities["envDiscovery"]["literals"].clone(),
            "regexKeys": entities["envDiscovery"]["regexes"].clone(),
        },
        "groups": JsonValue::Object(groups),
        "contractViolations": diagnostic_counts(&diagnostics),
    });

    let mut out = to_yaml(&report)?;
    out.push_str(&format!(
        "\n# Values are resolved for env '{}'; pass env= to see another.\n\
         # Every app is listed above; op='lint' explains the {} violations.\n",
        entities["usedEnv"].as_str().unwrap_or_default(),
        diagnostics.len(),
    ));

    // A chart whose only production branch is written as `^prod.*$` declares no
    // literal `prod` anywhere, so a reader of `declared` alone would conclude
    // the environment does not exist.
    if let Some(patterns) = entities["envDiscovery"]["regexes"].as_array() {
        if !patterns.is_empty() {
            out.push_str(
                "# 'declared' lists only environments written out literally. The regex keys \
                 above\n# match further environment names -- any name matching one is valid \
                 in env=.\n",
            );
        }
    }
    Ok(out)
}

fn list_entities(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let entities = crate::lsp::analysis_list_entities(&source, optional_str(args, "env"))?;
    let used_env = entities["usedEnv"].as_str().unwrap_or_default().to_string();

    let enabled: Vec<String> = entities["enabledEntities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| {
                    Some(format!(
                        "{}.{}",
                        entry["group"].as_str()?,
                        entry["app"].as_str()?
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut lines = vec![format!(
        "# apps in this chart, resolved for env '{used_env}'"
    )];
    let mut total = 0usize;
    for group in entities["groups"].as_array().unwrap_or(&Vec::new()) {
        let Some(group_name) = group["name"].as_str() else {
            continue;
        };
        for app in group["apps"].as_array().unwrap_or(&Vec::new()) {
            let Some(app_name) = app.as_str() else {
                continue;
            };
            let reference = format!("{group_name}.{app_name}");
            let state = if enabled.contains(&reference) {
                "enabled"
            } else {
                "disabled"
            };
            lines.push(format!("{reference}\t{state}"));
            total += 1;
        }
    }

    if total == 0 {
        return Ok(format!(
            "No apps found in {}.\nThe chart declares no `apps-*` group with app entries; \
             op='overview' shows what it does contain.",
            source.values_path.display()
        ));
    }

    lines.push(format!(
        "\n{total} apps, {} enabled for '{used_env}'.",
        enabled.len()
    ));
    Ok(lines.join("\n"))
}

fn resolve_entity(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let group = required_str(args, "group")?;
    let app = required_str(args, "app")?;

    let resolved = crate::lsp::analysis_resolve_entity(
        &source,
        group.clone(),
        app.clone(),
        optional_str(args, "env"),
        args.get("apply_includes").and_then(JsonValue::as_bool),
        args.get("apply_env_resolution")
            .and_then(JsonValue::as_bool),
    )
    .map_err(|err| explain_missing_entity(err, &source, &group, &app))?;

    let used_env = resolved["usedEnv"].as_str().unwrap_or_default();
    let mut entity = resolved["entity"].clone();

    if let Some(values_path) = optional_str(args, "values_path") {
        entity = select_subpath(&entity, &values_path)
            .ok_or_else(|| explain_missing_subpath(&entity, &values_path, &group, &app))?;
    }

    Ok(format!(
        "# {group}.{app} resolved for env '{used_env}'{}\n{}{}",
        blocking_error_banner(&source),
        to_yaml(&entity)?,
        unknown_env_notice(
            &resolved["envDiscovery"],
            optional_str(args, "env").as_deref()
        ),
    ))
}

/// Warns when the chart carries an error that stops it rendering at all.
///
/// happ resolves values more leniently than the library renders them, so a
/// chart with a fatal contract violation still produces a clean-looking
/// `resolve` answer. Acting on those values would be acting on something that
/// never reaches a cluster, so the answer has to carry the caveat with it.
/// Says where each of an app's values came from.
///
/// On a chart whose apps inherit through several `_include` profiles, knowing
/// what a value is settles far less than knowing which profile set it: that is
/// the difference between reading the answer and being able to change it.
fn explain_value_origin(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let group = required_str(args, "group")?;
    let app = required_str(args, "app")?;
    let values_path = optional_str(args, "values_path");

    let (origins, used_env, discovery) = crate::lsp::analysis_value_origins(
        &source,
        &group,
        &app,
        optional_str(args, "env"),
        values_path.as_deref(),
    )
    .map_err(|err| explain_missing_entity(err, &source, &group, &app))?;

    let mut lines = vec![format!(
        "# where {group}.{app} gets its values, for env '{used_env}'"
    )];

    // Once each, at the top. A layer is written in one place however many
    // values it supplies, and repeating its file and line under all 192 of
    // them was a third of the answer spent saying the same thing.
    let mut sites: Vec<(&str, &str)> = Vec::new();
    for origin in &origins {
        let Some(defined_in) = origin.defined_in.as_deref() else {
            continue;
        };
        if !sites.iter().any(|(layer, _)| *layer == origin.from) {
            sites.push((&origin.from, defined_in));
        }
    }
    if !sites.is_empty() {
        lines.push("# layers, and where each is written:".to_string());
        for (layer, defined_in) in &sites {
            lines.push(format!("#   {layer} -- {defined_in}"));
        }
    }

    for origin in &origins {
        let mut source_note = origin.from.clone();
        if !origin.via.is_empty() {
            // The chain reads outermost first, which is the order the app
            // itself names them in.
            source_note.push_str(&format!(" (via {})", origin.via.join(" -> ")));
        }
        if let Some(selector) = &origin.selector {
            source_note.push_str(&format!(", env key '{selector}'"));
        }
        lines.push(format!(
            "{}: {}\n    from {source_note}",
            origin.path,
            render_origin_value(&origin.value),
        ));
    }
    if values_path.is_none() {
        lines.push(format!(
            "\n{} values. Narrow with values_path.",
            origins.len()
        ));
    }
    Ok(format!(
        "{}{}",
        lines.join("\n"),
        unknown_env_notice(&discovery, optional_str(args, "env").as_deref())
    ))
}

/// Renders a resolved value on one line, since the point here is the source.
fn render_origin_value(value: &JsonValue) -> String {
    let text = match value {
        JsonValue::Null => "(absent for this env)".to_string(),
        JsonValue::String(text) => text.clone(),
        other => other.to_string(),
    };
    let flattened = text.replace('\n', " ");
    let trimmed = flattened.trim();
    if trimmed.chars().count() > 90 {
        let head: String = trimmed.chars().take(90).collect();
        return format!("{head}...");
    }
    trimmed.to_string()
}

fn blocking_error_banner(source: &ChartValuesSource) -> String {
    let Ok(diagnostics) = crate::lsp::analysis_diagnostics(source) else {
        return String::new();
    };
    let blocking: Vec<&JsonValue> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == "error")
        .collect();
    if blocking.is_empty() {
        return String::new();
    }
    format!(
        "\n# WARNING: this chart has {} error(s) and will not render as written; \
         op='lint' lists them.\n# First: {}",
        blocking.len(),
        blocking
            .first()
            .and_then(|first| first["message"].as_str())
            .unwrap_or_default(),
    )
}

fn render_entity_manifest(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let group = required_str(args, "group")?;
    let app = required_str(args, "app")?;
    let renderer = optional_str(args, "renderer");

    // Settled before rendering, because a disabled app is the explanation for
    // most of the ways this render can fail: the preview forces the app on, and
    // the library then rightly objects to an app that was never configured
    // because it was never meant to run here.
    let asked_env = optional_str(args, "env");
    let asked_env_for_notice = asked_env.clone();
    let enabled = asked_env
        .clone()
        .or_else(|| chart_default_env(&source))
        .map(|env| entity_enabled(&source, &group, &app, &env))
        .unwrap_or(true);

    let rendered = crate::lsp::analysis_render_entity_manifest(
        &source,
        group.clone(),
        app.clone(),
        asked_env,
        renderer.clone(),
    )
    .map_err(|err| explain_missing_entity(err, &source, &group, &app))
    .map_err(|err| {
        let cause = summarize_render_failure(&err);
        if enabled {
            return cause;
        }
        format!(
            "{group}.{app} is DISABLED in this environment, so it deploys nothing here.\n\
             The preview forces it on to show what it would render, and the library then \
             reported:\n\n{cause}"
        )
    })?;

    let used_env = rendered["usedEnv"].as_str().unwrap_or_default();
    let manifest = rendered["manifest"].as_str().unwrap_or_default();
    if manifest.trim().is_empty() {
        return Ok(format!(
            "{group}.{app} renders no manifests for env '{used_env}'.\n\
             The app is most likely disabled for this environment -- op='apps' shows its \
             enabled state.",
        ));
    }

    // The preview forces the app on so that a disabled app can still be
    // inspected -- useful, but silently misleading unless it is said out loud,
    // because the manifests below are not what this environment deploys.
    let disabled_note = if enabled && entity_enabled(&source, &group, &app, used_env) {
        String::new()
    } else {
        format!(
            "\n# NOTE: {group}.{app} is DISABLED in env '{used_env}'. Nothing below is deployed \
             there; this is a preview of what it would render if enabled.",
        )
    };

    let renderer = renderer.as_deref().unwrap_or("fast");
    // Every renderer is now handed the same values root; what is left between
    // them is the engine, and the fast one is helm's own, linked in. Saying it
    // is "an approximation" sent callers to `helm` for a fidelity they already
    // had, at several times the cost.
    let fidelity = if renderer == "fast" {
        "\n# The fast renderer runs helm's own engine in-process, on the same values \
         renderer='helm' would be given; it needs no helm on PATH."
    } else {
        ""
    };

    let narrowed = narrow_rendered(
        manifest,
        optional_str(args, "kind").as_deref(),
        optional_str(args, "resource").as_deref(),
    )?;
    let incomplete = describe_containers_without_image(&narrowed);
    let unknown_env =
        unknown_env_notice(&rendered["envDiscovery"], asked_env_for_notice.as_deref());

    let header = format!(
        "# {group}.{app} rendered for env '{used_env}' by the {renderer} renderer\
         {disabled_note}{fidelity}{incomplete}{unknown_env}",
    );
    Ok(truncate_render(narrowed, args, &header))
}

/// One rendered document, kept next to the text it came from.
struct RenderedDocument {
    kind: String,
    name: String,
    text: String,
}

/// Splits a render into documents, discarding the empty ones a chart emits.
fn rendered_documents(manifest: &str) -> Vec<RenderedDocument> {
    use serde::Deserialize;

    let mut out = Vec::new();
    // Split on the separator rather than reserialising: what the caller wants
    // to read is the chart's own formatting, comments and ordering included.
    for chunk in manifest.split("\n---") {
        let text = chunk.trim_start_matches("---").trim();
        if text.is_empty() {
            continue;
        }
        let Some(doc) = serde_yaml::Deserializer::from_str(text)
            .next()
            .and_then(|document| serde_yaml::Value::deserialize(document).ok())
        else {
            continue;
        };
        let Some(kind) = doc.get("kind").and_then(|kind| kind.as_str()) else {
            continue;
        };
        out.push(RenderedDocument {
            kind: kind.to_string(),
            name: doc
                .get("metadata")
                .and_then(|meta| meta.get("name"))
                .and_then(|name| name.as_str())
                .unwrap_or("<unnamed>")
                .to_string(),
            text: text.to_string(),
        });
    }
    out
}

/// Keeps the documents the caller asked about.
///
/// An app of this chart renders up to eight resources and a question is almost
/// always about one of them; rendering all of them to read a Deployment costs
/// the model several thousand tokens it then has to skip past.
fn narrow_rendered(
    manifest: &str,
    kind: Option<&str>,
    name: Option<&str>,
) -> Result<String, String> {
    if kind.is_none() && name.is_none() {
        return Ok(manifest.to_string());
    }
    let documents = rendered_documents(manifest);
    let matches = |doc: &RenderedDocument| {
        kind.is_none_or(|wanted| doc.kind.eq_ignore_ascii_case(wanted))
            && name.is_none_or(|wanted| doc.name == wanted)
    };
    let kept: Vec<&RenderedDocument> = documents.iter().filter(|doc| matches(doc)).collect();
    if kept.is_empty() {
        let asked = match (kind, name) {
            (Some(kind), Some(name)) => format!("{kind}/{name}"),
            (Some(kind), None) => kind.to_string(),
            (None, Some(name)) => name.to_string(),
            (None, None) => String::new(),
        };
        return Err(format!(
            "this app renders no '{asked}'. It renders: {}",
            document_index(&documents).join(", ")
        ));
    }
    Ok(kept
        .iter()
        .map(|doc| doc.text.as_str())
        .collect::<Vec<&str>>()
        .join("\n---\n"))
}

/// The documents a render produced, as `Kind/name`.
fn document_index(documents: &[RenderedDocument]) -> Vec<String> {
    documents
        .iter()
        .map(|doc| format!("{}/{}", doc.kind, doc.name))
        .collect()
}

/// Caps a render, and says what was cut in terms the caller can ask for.
///
/// The generic cap ends mid-document and advises raising `limit`, which asks
/// for the whole thing again at a higher price. A render is a list of named
/// resources, so the useful thing to hand back is that list.
fn truncate_render(manifest: String, args: &JsonValue, header: &str) -> String {
    let limit = optional_u32(args, "limit")
        .map(|value| value as usize)
        .unwrap_or(RENDER_MAX_LINES)
        .max(1);
    let header_lines = header.lines().count();
    let body_limit = limit.saturating_sub(header_lines).max(1);
    let lines: Vec<&str> = manifest.lines().collect();
    if lines.len() <= body_limit {
        return format!("{header}\n{manifest}");
    }

    let documents = rendered_documents(&manifest);
    let index = if documents.is_empty() {
        String::new()
    } else {
        format!(
            "\n# It renders {}: {}.\n# Narrow with kind= or resource= rather than raising limit.",
            documents.len(),
            document_index(&documents).join(", ")
        )
    };
    format!(
        "{header}{index}\n{}\n\n[truncated: showing {body_limit} of {} manifest lines]",
        lines[..body_limit].join("\n"),
        lines.len(),
    )
}

/// A rendered app is the one answer that routinely runs past the common cap,
/// and half a Deployment is worse than none.
const RENDER_MAX_LINES: usize = 600;

/// Warns about containers the render left with no image.
///
/// A render that succeeds is not the same as a render that deploys: helm-apps
/// takes the image from werf metadata or from values the deployment supplies,
/// so asking about a chart without them produces a workload the API server
/// rejects outright. Saying so beats letting `image:` pass for a value.
fn describe_containers_without_image(manifest: &str) -> String {
    let missing = containers_without_image(manifest);
    if missing.is_empty() {
        return String::new();
    }
    format!(
        "\n# INCOMPLETE: no image on {}. Kubernetes rejects such a workload. \
         helm-apps takes the image from werf metadata or from values the deployment \
         supplies -- pass them with values_files or set.",
        missing.join(", ")
    )
}

/// Every container in the manifest with no usable image, as `Kind/name: container`.
fn containers_without_image(manifest: &str) -> Vec<String> {
    use serde::Deserialize;

    let mut missing = Vec::new();
    for document in serde_yaml::Deserializer::from_str(manifest) {
        let Ok(doc) = serde_yaml::Value::deserialize(document) else {
            continue;
        };
        let kind = doc
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("resource");
        let name = doc
            .get("metadata")
            .and_then(|meta| meta.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        let Some(pod_spec) = pod_spec_of(&doc) else {
            continue;
        };
        for field in ["initContainers", "containers"] {
            let Some(containers) = pod_spec.get(field).and_then(|v| v.as_sequence()) else {
                continue;
            };
            for container in containers {
                if has_image(container) {
                    continue;
                }
                let container_name = container
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unnamed>");
                missing.push(format!("{kind}/{name}: container {container_name}"));
            }
        }
    }
    missing
}

/// The pod spec a workload carries, wherever its kind keeps one.
fn pod_spec_of(doc: &serde_yaml::Value) -> Option<&serde_yaml::Value> {
    let spec = doc.get("spec")?;
    // Pod itself, then the one template every controller wraps it in, then the
    // extra job layer a CronJob adds.
    if spec.get("containers").is_some() {
        return Some(spec);
    }
    if let Some(template) = spec.get("template").and_then(|t| t.get("spec")) {
        return Some(template);
    }
    spec.get("jobTemplate")?
        .get("spec")?
        .get("template")?
        .get("spec")
}

fn has_image(container: &serde_yaml::Value) -> bool {
    container
        .get("image")
        .and_then(|v| v.as_str())
        .is_some_and(|image| !image.trim().is_empty())
}

/// Puts the cause of a failed render first.
///
/// A Go template failure inside a library chart arrives as a chain of
/// "error calling include:" frames -- thousands of characters of plumbing whose
/// last line is the only thing that says what is wrong. helm-apps also raises
/// its own errors as `[helm-apps:CODE] message | path=... | hint=... | docs=...`,
/// which is precisely the answer, buried in the middle of the chain.
fn summarize_render_failure(err: &str) -> String {
    let trace = err.trim();

    let cause = library_error_marker(trace)
        .or_else(|| deepest_template_frame(trace))
        .unwrap_or_else(|| trace.chars().take(400).collect());

    if cause == trace {
        return trace.to_string();
    }
    format!("{cause}\n\n--- full renderer trace ---\n{trace}")
}

/// Extracts a `[helm-apps:CODE] ...` diagnostic, which the library formats as
/// one line carrying the path, a hint and a docs anchor.
fn library_error_marker(trace: &str) -> Option<String> {
    let start = trace.find("[helm-apps:")?;
    let rest = &trace[start..];
    let end = rest.find('\n').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// The innermost `executing ...: <message>` frame -- the point where the
/// template engine actually gave up.
fn deepest_template_frame(trace: &str) -> Option<String> {
    let frames: Vec<String> = trace
        .split("executing ")
        .skip(1)
        .filter_map(|frame| {
            let message = frame.rsplit_once(": ")?.1.trim();
            let message = message.trim_end_matches("error calling include:").trim();
            (!message.is_empty()).then(|| message.to_string())
        })
        .collect();
    frames.last().cloned()
}

/// Chart-level wiring the values file cannot show.
///
/// A library chart renders nothing by itself: the consumer chart has to call
/// `apps-utils.init-library` from a template. Miss that and every app is
/// silently ignored -- `helm template` prints nothing at all and says nothing
/// about why, which is among the hardest helm-apps mistakes to spot.
fn chart_wiring_findings(source: &ChartValuesSource) -> Vec<String> {
    let mut findings = Vec::new();

    let declares_apps = crate::lsp::analysis_list_entities(source, None)
        .ok()
        .and_then(|entities| {
            entities["groups"]
                .as_array()
                .map(|groups| !groups.is_empty())
        })
        .unwrap_or(false);
    if !declares_apps {
        return findings;
    }

    let templates = source.chart_root.join("templates");
    let initialises = read_dir_texts(&templates)
        .iter()
        .any(|text| text.contains("apps-utils.init-library"));
    if !initialises {
        findings.push(format!(
            "{}: chart declares apps-* groups but no template calls \
             `apps-utils.init-library`, so the library renders nothing at all. Add \
             templates/init-helm-apps-library.yaml containing: \
             {{{{- include \"apps-utils.init-library\" $ }}}}",
            source.chart_root.display()
        ));
    }

    findings
}

fn read_dir_texts(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect()
}

/// The env the chart resolves to when the caller names none.
fn chart_default_env(source: &ChartValuesSource) -> Option<String> {
    crate::lsp::analysis_list_entities(source, None)
        .ok()?
        .get("usedEnv")?
        .as_str()
        .map(ToString::to_string)
}

/// Whether the app is switched on for `env`, which decides whether a rendered
/// preview describes reality or a hypothetical.
fn entity_enabled(source: &ChartValuesSource, group: &str, app: &str, env: &str) -> bool {
    let Ok(entities) = crate::lsp::analysis_list_entities(source, Some(env.to_string())) else {
        return true;
    };
    entities["enabledEntities"]
        .as_array()
        .is_some_and(|enabled| {
            enabled
                .iter()
                .any(|entry| entry["group"] == group && entry["app"] == app)
        })
}

fn lint_values(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let diagnostics = crate::lsp::analysis_diagnostics(&source)?;
    let file = source.values_path.display().to_string();
    let wiring = chart_wiring_findings(&source);

    if diagnostics.is_empty() && wiring.is_empty() {
        return Ok(format!(
            "No findings: {file} matches the helm-apps contract."
        ));
    }

    let mut lines: Vec<String> = diagnostics
        .iter()
        .map(|diagnostic| {
            let code = diagnostic["code"]
                .as_str()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default();
            format!(
                "{file}:{}:{} {}{code} {}",
                diagnostic["line"].as_u64().unwrap_or(0),
                diagnostic["column"].as_u64().unwrap_or(0),
                diagnostic["severity"].as_str().unwrap_or("info"),
                diagnostic["message"].as_str().unwrap_or_default(),
            )
        })
        .collect();

    for finding in &wiring {
        lines.push(format!("{finding} [E_LIBRARY_NOT_INITIALISED]"));
    }

    let counts = diagnostic_counts(&diagnostics);
    let errors = counts["errors"].as_u64().unwrap_or(0) as usize + wiring.len();
    lines.push(format!(
        "\n{} findings: {errors} errors, {} warnings, {} info.",
        diagnostics.len() + wiring.len(),
        counts["warnings"],
        counts["info"],
    ));
    Ok(lines.join("\n"))
}

fn diff_entity_envs(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let group = required_str(args, "group")?;
    let app = required_str(args, "app")?;
    let from_env = required_str(args, "from_env")?;
    let to_env = required_str(args, "to_env")?;

    let from = entity_for_env(&source, &group, &app, &from_env)?;
    let to = entity_for_env(&source, &group, &app, &to_env)?;

    let changes = crate::dyfflike::changes_between_docs(
        std::slice::from_ref(&from),
        std::slice::from_ref(&to),
        crate::dyfflike::DiffOptions::default(),
    );

    if changes.is_empty() {
        return Ok(format!(
            "{group}.{app} resolves identically for '{from_env}' and '{to_env}'."
        ));
    }

    let mut lines = vec![format!("# {group}.{app}: {from_env} -> {to_env}")];
    for change in &changes {
        let path = change.path.strip_prefix("doc[0].").unwrap_or(&change.path);
        lines.push(match change.kind {
            crate::dyfflike::ChangeKind::Changed => format!(
                "changed  {path}: {} -> {}",
                render_scalar(change.from.as_ref()),
                render_scalar(change.to.as_ref())
            ),
            crate::dyfflike::ChangeKind::Added => format!(
                "added    {path}: {} (absent in {from_env})",
                render_scalar(change.to.as_ref())
            ),
            crate::dyfflike::ChangeKind::Removed => format!(
                "removed  {path}: {} (absent in {to_env})",
                render_scalar(change.from.as_ref())
            ),
        });
    }
    lines.push(format!("\n{} differences.", changes.len()));
    Ok(lines.join("\n"))
}

/// Runs one jq expression over Kubernetes objects from every enabled app.
///
/// The render stays server-side: callers pay for the query result rather than
/// concatenated YAML from the whole fleet. Provenance is kept beside each
/// object because Kubernetes metadata alone cannot identify its values entry.
fn query_manifests(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let query = required_str(args, "query")?;
    let asked_env = optional_str(args, "env");
    let renderer = optional_str(args, "renderer");
    let kind = optional_str(args, "kind");
    let resource = optional_str(args, "resource");
    let entities = crate::lsp::analysis_list_entities(&source, asked_env.clone())?;
    let used_env = entities["usedEnv"].as_str().unwrap_or_default();
    let enabled = entities["enabledEntities"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut records = Vec::new();
    for entity in enabled {
        let Some(group) = entity["group"].as_str() else {
            continue;
        };
        let Some(app) = entity["app"].as_str() else {
            continue;
        };
        let rendered = crate::lsp::analysis_render_entity_manifest(
            &source,
            group.to_string(),
            app.to_string(),
            asked_env.clone(),
            renderer.clone(),
        )
        .map_err(|err| format!("{group}.{app}: {}", summarize_render_failure(&err)))?;
        let manifest = rendered["manifest"].as_str().unwrap_or_default();
        if manifest.trim().is_empty() {
            continue;
        }
        let documents = crate::query::parse_input_docs_prefer_yaml(manifest)
            .map_err(|err| format!("{group}.{app}: parse rendered manifests: {err}"))?;
        for manifest in documents {
            if !manifest_matches(&manifest, kind.as_deref(), resource.as_deref()) {
                continue;
            }
            records.push(json!({
                "group": group,
                "app": app,
                "manifest": manifest,
            }));
        }
    }

    let manifest_count = records.len();
    let results = crate::query::run_query_stream(&query, vec![JsonValue::Array(records)])
        .map_err(|err| explain_query_failure(&err.to_string(), &query))?;
    let notice = unknown_env_notice(&entities["envDiscovery"], asked_env.as_deref());
    if results.is_empty() {
        return Ok(format!(
            "Query matched nothing in {manifest_count} manifests from {} enabled apps for env \
             '{used_env}'.{notice}",
            enabled.len(),
        ));
    }

    let output = crate::query::format_output_json_lines(&results, false, false)
        .map_err(|err| format!("format query output: {err}"))?;
    Ok(format!("{output}{notice}"))
}

fn manifest_matches(manifest: &JsonValue, kind: Option<&str>, resource: Option<&str>) -> bool {
    let actual_kind = manifest.get("kind").and_then(JsonValue::as_str);
    let actual_name = manifest
        .get("metadata")
        .and_then(|metadata| metadata.get("name"))
        .and_then(JsonValue::as_str);
    kind.is_none_or(|wanted| actual_kind.is_some_and(|actual| actual.eq_ignore_ascii_case(wanted)))
        && resource.is_none_or(|wanted| actual_name == Some(wanted))
}

fn query_values(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let query = required_str(args, "query")?;
    let asked_env = optional_str(args, "env");
    let (root, used_env, discovery) =
        crate::lsp::analysis_resolved_root(&source, asked_env.clone(), None, None)?;
    let notice = unknown_env_notice(&discovery, asked_env.as_deref());

    let results = crate::query::run_query_stream(&query, vec![root])
        .map_err(|err| explain_query_failure(&err.to_string(), &query))?;
    if results.is_empty() {
        return Ok(format!(
            "Query matched nothing in the values resolved for env '{used_env}'.{notice}"
        ));
    }

    let output = crate::query::format_output_json_lines(&results, false, false)
        .map_err(|err| format!("format query output: {err}"))?;
    Ok(format!("{output}{notice}"))
}

/// Warns when the environment asked for is one the chart never mentions.
///
/// `global.env` is a free-form string, so a misspelt one is not an error: every
/// env map simply falls through to `_default` and the answer comes back looking
/// like a real one. happ knows which names the chart writes down, so it can say
/// so where Helm cannot.
fn unknown_env_notice(discovery: &JsonValue, asked: Option<&str>) -> String {
    let Some(asked) = asked.map(str::trim).filter(|env| !env.is_empty()) else {
        return String::new();
    };
    let literals: Vec<&str> = discovery["literals"]
        .as_array()
        .map(|names| names.iter().filter_map(JsonValue::as_str).collect())
        .unwrap_or_default();
    if literals.is_empty() || literals.contains(&asked) {
        return String::new();
    }
    // A chart may select `prod` through `^prod.*$` and never write it out, so a
    // regex key anywhere means happ cannot claim the name is unknown.
    if discovery["regexes"]
        .as_array()
        .is_some_and(|patterns| !patterns.is_empty())
    {
        return String::new();
    }
    format!(
        "\n# NOTE: this chart never mentions env '{asked}'. It declares: {}.\n\
         # Every env map fell through to _default, so these are the fallback values.\n",
        literals.join(", ")
    )
}

/// Explains the one jq mistake this contract guarantees.
///
/// Every group in a helm-apps chart is named `apps-something`, and jq reads the
/// hyphen as subtraction: `.apps-stateless` asks for `.apps` minus a function
/// called `stateless`, and reports `stateless/0 is not defined`, which says
/// nothing about the dot the caller actually needs.
fn explain_query_failure(err: &str, query: &str) -> String {
    let base = format!("query failed: {err}");
    let bare_group = query.split(['|', '(', ')', '[', ']', ' ']).any(|token| {
        token
            .strip_prefix('.')
            .is_some_and(|name| name.starts_with("apps-"))
    });
    if !bare_group {
        return base;
    }
    format!(
        "{base}\nGroup names contain '-', which jq reads as subtraction. \
         Quote the key: .[\"apps-stateless\"] rather than .apps-stateless."
    )
}

fn library_reference(args: &JsonValue) -> Result<String, String> {
    let topic = optional_str(args, "name").unwrap_or_else(|| "all".to_string());
    let version = crate::assets::embedded_helm_apps_version().unwrap_or_else(|| "unknown".into());
    let mut sections: Vec<String> = Vec::new();

    if matches!(topic.as_str(), "all" | "groups") {
        let mut lines = vec![
            "## Built-in `apps-*` groups".to_string(),
            String::new(),
            "Top-level values keys the library renders. An `apps-*` key that is not one of these \
             is dropped, or fails the render when `global.validation.strict` is on, unless the \
             block declares `__GroupVars__.type` to borrow another group's templates."
                .to_string(),
            String::new(),
        ];
        for group in crate::lsp::builtin_app_groups() {
            lines.push(format!("- {group}"));
        }
        sections.push(lines.join("\n"));
    }

    if matches!(topic.as_str(), "all" | "functions") {
        // 135 functions is 14 KB, and a caller after `fl.value` was paying for
        // all of them. `query` is a plain substring: a reader who knows the
        // prefix wants that family, and one who knows nothing still gets the
        // whole list by leaving it out.
        let wanted = optional_str(args, "query").map(|query| query.to_lowercase());
        let matching: Vec<(String, String)> = library_defined_templates()
            .into_iter()
            .filter(|(name, _)| {
                wanted
                    .as_ref()
                    .is_none_or(|query| name.to_lowercase().contains(query))
            })
            .collect();

        let mut lines = vec!["## Template functions the library defines".to_string()];
        if let Some(query) = &wanted {
            lines.push(format!(
                "\n{} of {} match '{query}'.",
                matching.len(),
                library_defined_templates().len()
            ));
        }
        lines.push(String::new());
        lines.push("Callable as `{{ include \"<name>\" (list $ ...) }}`.".to_string());
        lines.push(String::new());
        for (name, origin) in &matching {
            let arity = crate::lsp::library_include_signature(name)
                .map(|(min, max)| match max {
                    Some(max) if max == min => format!(" -- takes {min} args"),
                    Some(max) => format!(" -- takes {min}..{max} args"),
                    None => format!(" -- takes at least {min} args"),
                })
                .unwrap_or_default();
            lines.push(format!("- `{name}`{arity}  (happ://helm-apps/{origin})"));
        }
        if matching.is_empty() {
            lines.push("Nothing matched. Leave 'query' out for the whole list.".to_string());
        }
        sections.push(lines.join("\n"));
    }

    if matches!(topic.as_str(), "all" | "env") {
        sections.push(
            "## How `global.env` selects a value\n\
             \n\
             Any values map may be an env map. The library picks a branch in this order:\n\
             \n\
             1. the key equal to `global.env`;\n\
             2. a regex key, anchored to the whole env name -- `stage` never matches `stage-eu`, \
             write `^stage-.*$`;\n\
             3. `_default`.\n\
             \n\
             When several regex keys match the same env the render fails with \
             `E_ENV_REGEX_AMBIGUOUS`; op='lint' reports it before Helm does. op='resolve' shows \
             the branch that wins for an environment, and op='diff' shows where two environments \
             part ways.\n\
             \n\
             Include profiles are a separate mechanism: a profile defined under `global._includes` \
             is pulled into an app with `_include`, and op='resolve' shows the result after both \
             include expansion and env selection."
                .to_string(),
        );
    }

    if sections.is_empty() {
        return Err(format!(
            "unknown topic '{topic}' -- expected one of: all, groups, functions, env"
        ));
    }

    Ok(format!(
        "# helm-apps {version} (embedded in happ {})\n\n{}",
        env!("CARGO_PKG_VERSION"),
        sections.join("\n\n")
    ))
}

/// The source of one library template, addressed either by the `define` name a
/// chart calls or by its path in the chart.
fn library_template(args: &JsonValue) -> Result<String, String> {
    // Every answer here prints the file as `happ://helm-apps/<path>`, and that
    // is the string a reader copies back. Refusing the identifier the tool
    // itself hands out is a round trip spent on punctuation.
    let name = required_str(args, "name")?
        .trim_start_matches("happ://helm-apps/")
        .to_string();

    if let Some(source) = crate::assets::embedded_helm_apps_file(&name) {
        return Ok(format!("# happ://helm-apps/{name}\n{source}"));
    }

    for (defined, path) in library_defined_templates() {
        if defined != name {
            continue;
        }
        let Some(source) = crate::assets::embedded_helm_apps_file(&path) else {
            continue;
        };
        // `extract_define_blocks` hands back the whole `define ... end` block,
        // wrapper included, so adding a wrapper here would emit the header and
        // the `end` twice and produce a fragment that does not parse. When
        // extraction finds nothing there is no block to quote, so say which
        // file to read instead of passing the entire file off as one `define`.
        let Some(body) = crate::templateanalyzer::extract_define_blocks(source).remove(&name)
        else {
            return Ok(format!(
                "# `{name}` is defined in happ://helm-apps/{path}, but its `define` block could not be isolated.\n# Read that file with op='template' and name='{path}'."
            ));
        };
        return Ok(format!(
            "# `{name}`, defined in happ://helm-apps/{path}\n{body}"
        ));
    }

    let near: Vec<String> = library_defined_templates()
        .into_iter()
        .map(|(defined, _)| defined)
        .filter(|defined| defined.contains(&name) || name.contains(defined))
        .take(10)
        .collect();
    if near.is_empty() {
        return Err(format!(
            "no library template named '{name}' -- op='contract' with name='functions' lists them all"
        ));
    }
    Err(format!(
        "no library template named exactly '{name}'. Close matches: {}",
        near.join(", ")
    ))
}

// --- shared helpers ---------------------------------------------------------

/// Template names the vendored library actually defines, with the file each
/// comes from. Derived from the embedded chart so the reference can never
/// describe a version this binary does not ship.
fn library_defined_templates() -> Vec<(String, String)> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for path in crate::assets::embedded_helm_apps_paths() {
        if !path.ends_with(".tpl") {
            continue;
        }
        let Some(source) = crate::assets::embedded_helm_apps_file(&path) else {
            continue;
        };
        for name in crate::templateanalyzer::extract_define_blocks(source).into_keys() {
            found.entry(name).or_insert_with(|| path.clone());
        }
    }
    found.into_iter().collect()
}

struct ChartMetadata {
    name: Option<String>,
    version: Option<String>,
    helm_apps_dependency: Option<String>,
}

fn chart_metadata(source: &ChartValuesSource) -> ChartMetadata {
    let Ok(text) = std::fs::read_to_string(source.chart_root.join("Chart.yaml")) else {
        return ChartMetadata {
            name: None,
            version: None,
            helm_apps_dependency: None,
        };
    };
    let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return ChartMetadata {
            name: None,
            version: None,
            helm_apps_dependency: None,
        };
    };

    let read = |key: &str| {
        parsed
            .get(key)
            .and_then(serde_yaml::Value::as_str)
            .map(ToString::to_string)
    };
    let helm_apps_dependency = parsed
        .get("dependencies")
        .and_then(serde_yaml::Value::as_sequence)
        .and_then(|dependencies| {
            dependencies.iter().find_map(|dependency| {
                let name = dependency.get("name").and_then(serde_yaml::Value::as_str)?;
                if name != "helm-apps" {
                    return None;
                }
                dependency
                    .get("version")
                    .and_then(serde_yaml::Value::as_str)
                    .map(ToString::to_string)
            })
        });

    ChartMetadata {
        name: read("name"),
        version: read("version"),
        helm_apps_dependency,
    }
}

fn entity_for_env(
    source: &ChartValuesSource,
    group: &str,
    app: &str,
    env: &str,
) -> Result<serde_yaml::Value, String> {
    let resolved = crate::lsp::analysis_resolve_entity(
        source,
        group.to_string(),
        app.to_string(),
        Some(env.to_string()),
        None,
        None,
    )
    .map_err(|err| explain_missing_entity(err, source, group, app))?;
    Ok(crate::chart_ir::json_to_yaml_value(&resolved["entity"]))
}

/// Turns "app not found" into something the model can act on, by naming what
/// the chart does contain.
fn explain_missing_entity(
    err: String,
    source: &ChartValuesSource,
    group: &str,
    app: &str,
) -> String {
    // Only the two errors that really are a name that missed. Matching any
    // "not found" once decorated a werf failure about a file outside the
    // project with a list of apps, one of which was the app being rendered.
    if !(err.starts_with("group not found") || err.starts_with("app not found at")) {
        return err;
    }
    let Ok(entities) = crate::lsp::analysis_list_entities(source, None) else {
        return err;
    };
    let known: Vec<String> = entities["groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|entry| {
                    let group_name = entry["name"].as_str()?;
                    let apps: Vec<String> = entry["apps"]
                        .as_array()?
                        .iter()
                        .filter_map(|app| Some(format!("{group_name}.{}", app.as_str()?)))
                        .collect();
                    Some(apps)
                })
                .flatten()
                .collect()
        })
        .unwrap_or_default();

    if known.is_empty() {
        return format!("{err} -- this chart declares no apps at all");
    }
    // A chart of 86 apps used to answer a one-character typo with every one of
    // them on a single line. What the caller needs is the name they meant.
    if known.len() <= NEAR_MISS_LIMIT {
        return format!(
            "{err}\nThis chart has no '{group}.{app}'. It contains: {}",
            known.join(", ")
        );
    }
    format!(
        "{err}\nThis chart has no '{group}.{app}'. Closest: {}.\nop='apps' lists all {}.",
        nearest_entities(&known, group, app).join(", "),
        known.len()
    )
}

/// How many apps are worth naming before a list stops being an answer.
const NEAR_MISS_LIMIT: usize = 8;

/// The apps closest to the one that was asked for.
///
/// Ranked by edit distance on the app name, with apps outside the requested
/// group pushed down -- a typo in the app name is the common case, and a right
/// name in the wrong group is the next one.
fn nearest_entities(known: &[String], group: &str, app: &str) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> = known
        .iter()
        .map(|entity| {
            let (entity_group, entity_app) = entity
                .split_once('.')
                .unwrap_or((entity.as_str(), entity.as_str()));
            let mut score = edit_distance(entity_app, app);
            if entity_group != group {
                score += 1 + app.len() / 4;
            }
            (score, entity)
        })
        .collect();
    scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    scored
        .into_iter()
        .take(NEAR_MISS_LIMIT)
        .map(|(_, entity)| entity.clone())
        .collect()
}

/// Levenshtein distance, over chars so a multi-byte name cannot panic.
fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0usize; right_chars.len() + 1];

    for (i, left_char) in left.chars().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right_chars.iter().enumerate() {
            let substitute = previous[j] + usize::from(left_char != *right_char);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_chars.len()]
}

fn diagnostic_counts(diagnostics: &[JsonValue]) -> JsonValue {
    let count = |severity: &str| {
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["severity"] == severity)
            .count()
    };
    json!({
        "errors": count("error"),
        "warnings": count("warning"),
        "info": count("info") + count("hint"),
    })
}

fn select_subpath(value: &JsonValue, path: &str) -> Option<JsonValue> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        current = match current {
            JsonValue::Object(map) => map.get(segment)?,
            JsonValue::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current.clone())
}

/// Says where a `values_path` stopped matching, and what was there instead.
///
/// A wrong app name has been answered with near misses for a while; a wrong
/// path was answered with nothing at all, which left `envvars` for `envVars`
/// costing a round trip that carried no information.
fn explain_missing_subpath(value: &JsonValue, path: &str, group: &str, app: &str) -> String {
    let mut current = value;
    let mut walked: Vec<&str> = Vec::new();
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        let next = match current {
            JsonValue::Object(map) => map.get(segment),
            JsonValue::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index)),
            _ => None,
        };
        let Some(next) = next else {
            let where_it_stopped = if walked.is_empty() {
                format!("{group}.{app}")
            } else {
                format!("{group}.{app}.{}", walked.join("."))
            };
            let available = match current {
                JsonValue::Object(map) => {
                    let keys: Vec<String> = map.keys().cloned().collect();
                    let near = nearest_by_name(&keys, segment);
                    if keys.len() <= NEAR_MISS_LIMIT {
                        format!(" It holds: {}.", keys.join(", "))
                    } else {
                        format!(" Closest of its {} keys: {}.", keys.len(), near.join(", "))
                    }
                }
                JsonValue::Array(items) => format!(" It is a list of {}.", items.len()),
                other => format!(" It is not a map but {other}."),
            };
            return format!(
                "no such path inside {group}.{app}: {path}\n'{segment}' is not in \
                 {where_it_stopped}.{available}"
            );
        };
        walked.push(segment);
        current = next;
    }
    format!("no such path inside {group}.{app}: {path}")
}

/// The names closest to the one that missed, by edit distance.
fn nearest_by_name(known: &[String], wanted: &str) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> = known
        .iter()
        .map(|name| (edit_distance(name, wanted), name))
        .collect();
    scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    scored
        .into_iter()
        .take(NEAR_MISS_LIMIT)
        .map(|(_, name)| name.clone())
        .collect()
}

/// Renders a diffed value on one line, since the report is line-oriented.
fn render_scalar(value: Option<&serde_yaml::Value>) -> String {
    let Some(value) = value else {
        return "null".to_string();
    };
    match value {
        serde_yaml::Value::String(text) => text.clone(),
        serde_yaml::Value::Null => "null".to_string(),
        other => serde_json::to_value(other)
            .ok()
            .map(|json| json.to_string())
            .unwrap_or_else(|| "<unrenderable>".to_string()),
    }
}

fn to_yaml(value: &JsonValue) -> Result<String, String> {
    serde_yaml::to_string(&crate::chart_ir::json_to_yaml_value(value))
        .map_err(|err| format!("serialize result as YAML: {err}"))
}

fn chart_source(context: &ServerContext, args: &JsonValue) -> Result<ChartValuesSource, String> {
    let path = optional_str(args, "chart")
        .map(PathBuf::from)
        .or_else(|| context.default_chart.clone())
        .ok_or_else(|| {
            "no chart to work on: pass 'chart', or start happ with --chart".to_string()
        })?;
    let source = crate::lsp::locate_chart_values(&path)?;
    let overrides = value_overrides(args, &source.chart_root)?;
    let release = ReleaseIdentity {
        name: optional_str(args, "release_name"),
        namespace: optional_str(args, "namespace"),
    };
    Ok(source.with_overrides(overrides).with_release(release))
}

/// Reads `values_files`, `set` and `set_string` off the request.
///
/// Every op takes them, because a chart is judged by the values it is deployed
/// with: answering `lint` or `resolve` from values.yaml alone while `render`
/// sees the deployment's own `-f` files is the disagreement this closes.
fn value_overrides(
    args: &JsonValue,
    chart_root: &std::path::Path,
) -> Result<ValueOverrides, String> {
    let mut overrides = ValueOverrides::default();

    if let Some(entries) = args.get("values_files") {
        let entries = as_json_text_or_value(entries);
        let entries = entries
            .as_array()
            .ok_or_else(|| "'values_files' must be a list of paths".to_string())?;
        for entry in entries {
            let raw = entry
                .as_str()
                .ok_or_else(|| "'values_files' entries must be strings".to_string())?;
            overrides.files.push(resolve_values_file(raw, chart_root)?);
        }
    }

    for (field, into_string) in [("set", false), ("set_string", true)] {
        let Some(value) = args.get(field) else {
            continue;
        };
        let value = as_json_text_or_value(value);
        let map = value
            .as_object()
            .ok_or_else(|| format!("'{field}' must be an object of path -> value"))?;
        for (path, raw) in map {
            if into_string {
                overrides
                    .set_string
                    .push((path.clone(), scalar_to_string(raw)?));
            } else {
                // A JSON string is what a `--set` flag would have carried, so
                // it gets Helm's typing; anything already typed is kept.
                let typed = match raw {
                    JsonValue::String(text) => crate::helm_overrides::typed_set_value(text),
                    other => other.clone(),
                };
                overrides.set.push((path.clone(), typed));
            }
        }
    }

    // `--set-file` is how a deployment passes anything with newlines in it --
    // this chart's CA bundle arrives that way -- and there is no spelling of
    // `set` that stands in for it: a path written as a value silently becomes
    // that literal string.
    if let Some(value) = args.get("set_file") {
        let value = as_json_text_or_value(value);
        let map = value
            .as_object()
            .ok_or_else(|| "'set_file' must be an object of path -> file".to_string())?;
        for (path, raw) in map {
            let file = raw
                .as_str()
                .ok_or_else(|| format!("'set_file' value for '{path}' must be a path"))?;
            let resolved = resolve_values_file(file, chart_root)?;
            let contents = std::fs::read_to_string(&resolved)
                .map_err(|err| format!("read set_file '{}': {err}", resolved.display()))?;
            overrides.set_string.push((path.clone(), contents));
        }
    }

    // Applied for real deep inside the values pipeline, where a failure can
    // only surface as "failed to parse values root". Running them once here
    // against an empty tree costs nothing and reports the actual reason -- an
    // unreadable file, a `--set` path that does not parse -- to the caller who
    // can still fix it.
    if !overrides.is_empty() {
        overrides.apply(&mut serde_json::Map::new())?;
    }

    Ok(overrides)
}

/// Reads an argument that should be a list or an object, accepting the JSON
/// text of one as well.
///
/// Clients routinely send a structured argument as a string -- the schema says
/// array, the call carries `"[\"ci.yaml\"]"`. Rejecting that teaches the model
/// nothing it can act on, while the intent is unambiguous.
fn as_json_text_or_value(value: &JsonValue) -> std::borrow::Cow<'_, JsonValue> {
    let Some(text) = value.as_str() else {
        return std::borrow::Cow::Borrowed(value);
    };
    match serde_json::from_str::<JsonValue>(text) {
        Ok(parsed) if parsed.is_array() || parsed.is_object() => std::borrow::Cow::Owned(parsed),
        _ => std::borrow::Cow::Borrowed(value),
    }
}

/// Finds a caller-named values file, preferring one relative to the chart.
fn resolve_values_file(raw: &str, chart_root: &std::path::Path) -> Result<PathBuf, String> {
    let given = crate::lsp::expand_home_dir(std::path::Path::new(raw));
    if given.is_absolute() {
        return Ok(given);
    }
    let from_chart = chart_root.join(&given);
    if from_chart.exists() {
        return Ok(from_chart);
    }
    if given.exists() {
        return Ok(given);
    }
    Err(format!(
        "values file '{raw}' not found: tried '{}' and '{}'",
        from_chart.display(),
        given.display()
    ))
}

fn scalar_to_string(value: &JsonValue) -> Result<String, String> {
    match value {
        JsonValue::String(text) => Ok(text.clone()),
        JsonValue::Number(number) => Ok(number.to_string()),
        JsonValue::Bool(flag) => Ok(flag.to_string()),
        other => Err(format!("'set_string' values must be scalars, got {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ServerContext {
        ServerContext::for_tests()
    }

    fn run(context: &ServerContext, args: JsonValue) -> Result<String, String> {
        call(context, &args)
    }

    /// A chart wired the way a real helm-apps consumer chart is: the library
    /// vendored under charts/, and a template that initialises it. Anything
    /// less renders nothing, which is itself one of the things under test.
    fn chart_fixture() -> tempfile::TempDir {
        let dir = bare_chart_fixture();
        std::fs::create_dir_all(dir.path().join("templates")).expect("templates dir");
        std::fs::write(
            dir.path().join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("init template");
        crate::assets::extract_helm_apps_chart(&dir.path().join("charts/helm-apps"))
            .expect("vendor the library");
        dir
    }

    /// The same chart with the library never initialised.
    fn bare_chart_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\ndependencies:\n  - name: helm-apps\n    version: 1.9.0\n",
        )
        .expect("write Chart.yaml");
        std::fs::write(
            dir.path().join("values.yaml"),
            "global:\n  env: dev\napps-stateless:\n  api:\n    enabled: true\n    replicas:\n      _default: 1\n      prod: 4\n  worker:\n    enabled: false\n",
        )
        .expect("write values.yaml");
        dir
    }

    fn chart_arg(chart: &tempfile::TempDir) -> String {
        chart.path().to_string_lossy().to_string()
    }

    #[test]
    fn every_documented_op_is_dispatchable() {
        let context = context();
        let ops = tool()["inputSchema"]["properties"]["op"]["enum"]
            .as_array()
            .expect("op enum")
            .iter()
            .filter_map(|op| op.as_str().map(ToString::to_string))
            .collect::<Vec<String>>();

        for op in ops {
            let outcome = run(&context, json!({ "op": op }));
            // Without a chart or required arguments these fail, but they must
            // fail on their own terms rather than as an unknown op.
            if let Err(message) = outcome {
                assert!(
                    !message.contains("unknown op"),
                    "op '{op}' is offered but not dispatched"
                );
            }
        }
    }

    #[test]
    fn an_unknown_op_lists_the_ones_that_exist() {
        let err = run(&context(), json!({ "op": "teleport" }))
            .err()
            .expect("unknown op must fail");
        assert!(err.contains("overview"), "{err}");
    }

    #[test]
    fn a_missing_chart_is_reported_with_the_path() {
        let err = run(
            &context(),
            json!({ "op": "overview", "chart": "/definitely/not/here" }),
        )
        .err()
        .expect("missing chart must fail");
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn a_call_with_no_chart_at_all_says_how_to_supply_one() {
        let err = run(&context(), json!({ "op": "overview" }))
            .err()
            .expect("no chart must fail");
        assert!(err.contains("--chart"), "{err}");
    }

    #[test]
    fn the_default_chart_is_used_when_the_call_omits_one() {
        let chart = chart_fixture();
        let context = ServerContext {
            default_chart: Some(chart.path().to_path_buf()),
            providers: crate::mcp::bridge::ProviderPool::default(),
        };
        let text = run(&context, json!({ "op": "apps" })).expect("apps");
        assert!(text.contains("apps-stateless.api"), "{text}");
    }

    #[test]
    fn overview_reports_groups_apps_and_declared_environments() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({ "op": "overview", "chart": chart_arg(&chart) }),
        )
        .expect("overview");
        assert!(text.contains("name: demo"), "{text}");
        assert!(text.contains("apps-stateless"), "{text}");
        assert!(text.contains("api: enabled"), "{text}");
        assert!(text.contains("worker: disabled"), "{text}");
        assert!(
            text.contains("prod"),
            "declared envs must include prod: {text}"
        );
    }

    /// The ops were once separate tools. Any leftover advice naming one sends
    /// the model chasing a tool that no longer exists.
    #[test]
    fn guidance_never_names_a_tool_that_was_folded_into_an_op() {
        let chart = chart_fixture();
        let retired = [
            "chart_overview",
            "list_entities",
            "resolve_entity",
            "render_entity_manifest",
            "lint_values",
            "diff_entity_envs",
            "query_values",
            "library_reference",
        ];

        let mut surfaces = vec![
            tool()["description"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            run(
                &context(),
                json!({ "op": "overview", "chart": chart_arg(&chart) }),
            )
            .expect("overview"),
            run(&context(), json!({ "op": "contract" })).expect("contract"),
        ];
        surfaces.push(
            run(
                &context(),
                json!({
                    "op": "render", "chart": chart_arg(&chart),
                    "group": "apps-stateless", "app": "worker",
                }),
            )
            .unwrap_or_default(),
        );

        for surface in surfaces {
            for name in retired {
                assert!(
                    !surface.contains(name),
                    "'{name}' is no longer callable but is still advertised in:\n{surface}"
                );
            }
        }
    }

    #[test]
    fn apps_lists_every_app_with_its_enabled_state() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({ "op": "apps", "chart": chart_arg(&chart) }),
        )
        .expect("apps");
        assert!(text.contains("apps-stateless.api\tenabled"), "{text}");
        assert!(text.contains("apps-stateless.worker\tdisabled"), "{text}");
    }

    #[test]
    fn resolve_collapses_the_env_map_for_the_requested_env() {
        let chart = chart_fixture();
        let context = context();

        let dev = run(
            &context,
            json!({ "op": "resolve", "chart": chart_arg(&chart), "group": "apps-stateless", "app": "api" }),
        )
        .expect("resolve dev");
        assert!(dev.contains("replicas: 1"), "{dev}");

        let prod = run(
            &context,
            json!({
                "op": "resolve", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "api", "env": "prod",
            }),
        )
        .expect("resolve prod");
        assert!(prod.contains("replicas: 4"), "{prod}");
    }

    #[test]
    fn resolve_without_env_resolution_keeps_every_branch_visible() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "resolve", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "api",
                "apply_env_resolution": false,
            }),
        )
        .expect("resolve raw");
        assert!(text.contains("_default"), "{text}");
        assert!(text.contains("prod"), "{text}");
    }

    #[test]
    fn asking_for_an_app_that_does_not_exist_lists_the_ones_that_do() {
        let chart = chart_fixture();
        let err = run(
            &context(),
            json!({
                "op": "resolve", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "nope",
            }),
        )
        .err()
        .expect("missing app must fail");
        assert!(err.contains("apps-stateless.api"), "{err}");
    }

    #[test]
    fn diff_names_the_value_on_both_sides() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "diff", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "api",
                "from_env": "dev", "to_env": "prod",
            }),
        )
        .expect("diff");
        assert!(text.contains("changed  replicas: 1 -> 4"), "{text}");
    }

    #[test]
    fn diff_between_identical_environments_says_so() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "diff", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "api",
                "from_env": "dev", "to_env": "dev",
            }),
        )
        .expect("diff");
        assert!(text.contains("resolves identically"), "{text}");
    }

    #[test]
    fn query_runs_over_the_resolved_tree() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "query", "chart": chart_arg(&chart),
                "query": ".[\"apps-stateless\"].api.replicas", "env": "prod",
            }),
        )
        .expect("query");
        assert_eq!(text.trim(), "4");
    }

    #[test]
    fn query_manifests_runs_across_enabled_apps_and_keeps_provenance() {
        let chart = chart_fixture();
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\napps-stateless:\n  api:\n    enabled: true\n    containers:\n      app:\n        image:\n          name: nginx\n  worker:\n    enabled: true\n    containers:\n      app:\n        image:\n          name: busybox\n  off:\n    enabled: false\n    containers:\n      app:\n        image:\n          name: ignored\n",
        )
        .expect("rewrite values");

        let text = run(
            &context(),
            json!({
                "op": "query_manifests",
                "chart": chart_arg(&chart),
                "kind": "Deployment",
                "query": "map({group, app, kind: .manifest.kind})",
            }),
        )
        .expect("query manifests");
        assert!(text.contains("apps-stateless"), "{text}");
        assert!(text.contains("api"), "{text}");
        assert!(text.contains("worker"), "{text}");
        assert!(
            !text.contains("off"),
            "disabled apps must be absent: {text}"
        );
        assert!(text.contains("Deployment"), "{text}");
    }

    #[test]
    fn query_manifests_filters_kind_and_resource_before_jq() {
        let deployment = json!({
            "kind": "Deployment",
            "metadata": { "name": "api" },
        });
        assert!(manifest_matches(
            &deployment,
            Some("deployment"),
            Some("api")
        ));
        assert!(!manifest_matches(&deployment, Some("Service"), Some("api")));
        assert!(!manifest_matches(
            &deployment,
            Some("Deployment"),
            Some("worker")
        ));
    }

    #[test]
    fn linting_a_clean_chart_says_so() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(text.contains("No findings"), "{text}");
    }

    #[test]
    fn linting_reports_an_unknown_apps_group_with_its_line() {
        let chart = chart_fixture();
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\napps-statelss:\n  api:\n    enabled: true\n",
        )
        .expect("rewrite values");
        let text = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(text.contains("E_UNKNOWN_APPS_GROUP"), "{text}");
        assert!(
            text.contains("apps-stateless"),
            "typo hint expected: {text}"
        );
    }

    #[test]
    fn contract_is_derived_from_the_embedded_chart() {
        let text = run(&context(), json!({ "op": "contract" })).expect("contract");
        assert!(text.contains("apps-stateless"), "{text}");
        assert!(text.contains("fl.value"), "{text}");
        assert!(text.contains("E_ENV_REGEX_AMBIGUOUS"), "{text}");
    }

    #[test]
    fn contract_can_be_narrowed_to_one_section() {
        let text =
            run(&context(), json!({ "op": "contract", "name": "env" })).expect("contract env");
        assert!(text.contains("_default"), "{text}");
        assert!(
            !text.contains("## Built-in"),
            "other sections must be omitted: {text}"
        );
    }

    #[test]
    fn contract_rejects_an_unknown_section() {
        assert!(run(&context(), json!({ "op": "contract", "name": "nonsense" })).is_err());
    }

    /// The values a chart is deployed with live outside it, so an analysis
    /// that reads only values.yaml describes a chart nobody ships.
    #[test]
    fn an_extra_values_file_reaches_every_op() {
        let chart = chart_fixture();
        let extra = chart.path().join("ci-values.yaml");
        std::fs::write(
            &extra,
            "apps-stateless:\n  api:\n    replicas:\n      _default: 7\n",
        )
        .expect("write extra values");

        let resolved = run(
            &context(),
            json!({
                "op": "resolve",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api",
                "values_path": "replicas",
                "values_files": ["ci-values.yaml"],
            }),
        )
        .expect("resolve");
        assert!(resolved.contains('7'), "{resolved}");

        // The same file has to reach a whole-tree op, not just entity lookup.
        let queried = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart_arg(&chart),
                "query": ".[\"apps-stateless\"].api.replicas",
                "values_files": ["ci-values.yaml"],
            }),
        )
        .expect("query");
        assert!(queried.contains('7'), "{queried}");
    }

    #[test]
    fn set_overrides_the_values_file_it_follows() {
        let chart = chart_fixture();
        let extra = chart.path().join("ci-values.yaml");
        std::fs::write(
            &extra,
            "global:\n  env: dev\n  vars:\n    HOST: from-file\n",
        )
        .expect("write extra values");

        let queried = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart_arg(&chart),
                "query": ".global.vars",
                "values_files": ["ci-values.yaml"],
                "set": {"global.vars.PORT": "8080"},
                "set_string": {"global.vars.HOST": "from-set"},
            }),
        )
        .expect("query");
        assert!(queried.contains("from-set"), "{queried}");
        assert!(queried.contains("8080"), "{queried}");
    }

    /// `--set` types its values the way Helm does, so a port is a number and
    /// a chart that compares it against one still works.
    #[test]
    fn set_values_keep_helms_typing_and_set_string_does_not() {
        let chart = chart_fixture();
        let queried = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart_arg(&chart),
                "query": "[.global.a, .global.b]",
                "set": {"global.a": "8080"},
                "set_string": {"global.b": "8080"},
            }),
        )
        .expect("query");
        assert!(queried.contains("8080"), "{queried}");
        assert!(
            queried.contains("\"8080\""),
            "set_string must stay a string: {queried}"
        );
    }

    #[test]
    fn a_missing_values_file_names_where_it_looked() {
        let chart = chart_fixture();
        let err = run(
            &context(),
            json!({
                "op": "apps",
                "chart": chart_arg(&chart),
                "values_files": ["nope.yaml"],
            }),
        )
        .err()
        .expect("a missing values file must fail");
        assert!(err.contains("nope.yaml"), "{err}");
    }

    /// A bad `--set` path is applied far inside the values pipeline, where the
    /// only error the caller could see is a generic parse failure.
    /// Clients send structured arguments as JSON text often enough that
    /// refusing them just costs the model a round trip.
    #[test]
    fn structured_arguments_are_accepted_as_json_text() {
        let chart = chart_fixture();
        let extra = chart.path().join("ci-values.yaml");
        std::fs::write(
            &extra,
            "global:\n  env: dev\n  vars:\n    HOST: from-file\n",
        )
        .expect("write extra values");

        let queried = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart_arg(&chart),
                "query": ".global.vars",
                "values_files": "[\"ci-values.yaml\"]",
                "set": "{\"global.vars.PORT\": 8080}",
            }),
        )
        .expect("query");
        assert!(queried.contains("from-file"), "{queried}");
        assert!(queried.contains("8080"), "{queried}");
    }

    #[test]
    fn a_malformed_set_path_reports_the_path_itself() {
        let chart = chart_fixture();
        let err = run(
            &context(),
            json!({
                "op": "apps",
                "chart": chart_arg(&chart),
                "set": {"global..vars": "x"},
            }),
        )
        .err()
        .expect("a malformed --set path must fail");
        assert!(err.contains("global..vars"), "{err}");
    }

    /// The render succeeded and the workload is still undeployable -- the exact
    /// shape a chart takes when its image comes from werf metadata nobody
    /// supplied.
    #[test]
    fn a_container_without_an_image_is_named() {
        let manifest = "\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: iam2-identity
spec:
  template:
    spec:
      initContainers:
      - name: wait-db
        image: busybox:1.36
      containers:
      - name: main
        image:
";
        assert_eq!(
            containers_without_image(manifest),
            vec!["Deployment/iam2-identity: container main".to_string()],
        );
        let note = describe_containers_without_image(manifest);
        assert!(note.contains("INCOMPLETE"), "{note}");
        assert!(note.contains("container main"), "{note}");
    }

    /// werf writes the same absence as an explicit null.
    #[test]
    fn an_explicitly_null_image_counts_as_missing() {
        let manifest = "kind: Pod\nmetadata:\n  name: p\nspec:\n  containers:\n  - name: main\n    image: null\n";
        assert_eq!(
            containers_without_image(manifest),
            vec!["Pod/p: container main".to_string()],
        );
    }

    #[test]
    fn a_cronjobs_containers_are_reached_through_its_job_template() {
        let manifest = "\
apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly
spec:
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: run
";
        assert_eq!(
            containers_without_image(manifest),
            vec!["CronJob/nightly: container run".to_string()],
        );
    }

    #[test]
    fn a_complete_manifest_gets_no_warning() {
        let manifest = "\
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
      - name: main
        image: registry.example/api:1.2.3
---
kind: Service
metadata:
  name: api
spec:
  ports:
  - port: 80
";
        assert!(containers_without_image(manifest).is_empty());
        assert!(describe_containers_without_image(manifest).is_empty());
    }

    /// `_include_files` says where an app is written; `_include` says what it
    /// inherits. Asking not to expand the second used to skip the first too,
    /// so a modular chart answered "no such app" for an app plainly in it.
    #[test]
    fn an_app_in_an_included_file_survives_apply_includes_false() {
        let chart = chart_fixture();
        std::fs::create_dir_all(chart.path().join("values/services")).expect("mkdir");
        std::fs::write(
            chart.path().join("values/services/api2.yaml"),
            "apps-stateless:\n  api2:\n    _include: [ \"shared\" ]\n    replicas: 3\n",
        )
        .expect("write service");
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\n  _includes:\n    shared:\n      enabled: true\n\
             apps-stateless:\n  api:\n    enabled: true\n_include_files:\n- values/services/api2.yaml\n",
        )
        .expect("write values");

        let literal = run(
            &context(),
            json!({
                "op": "resolve",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api2",
                "apply_includes": false,
            }),
        )
        .expect("the app lives in an included file, so it must resolve");
        assert!(literal.contains('3'), "{literal}");
        // Not expanding profiles is still what was asked for: the `_include`
        // reference stays, and what it would have brought in does not appear.
        assert!(literal.contains("shared"), "{literal}");
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("iam2-identity", "iam2-identit"), 1);
        assert_eq!(edit_distance("api", "api"), 0);
        assert_eq!(edit_distance("api", "pai"), 2);
        // Multi-byte names must not panic or count bytes.
        assert_eq!(edit_distance("привет", "привет"), 0);
        assert_eq!(edit_distance("привет", "приве"), 1);
    }

    /// A one-character typo used to be answered with every app in the chart on
    /// one line -- about 2.5 KB for the reported chart.
    #[test]
    fn a_near_miss_is_ranked_above_the_rest() {
        let known: Vec<String> = (0..40)
            .map(|n| format!("apps-stateless.filler-{n}"))
            .chain([
                "apps-stateless.iam2-identity".to_string(),
                "apps-jobs.iam2-identity-migrate".to_string(),
            ])
            .collect();

        let nearest = nearest_entities(&known, "apps-stateless", "iam2-identit");
        assert_eq!(
            nearest.first().map(String::as_str),
            Some("apps-stateless.iam2-identity"),
            "{nearest:?}"
        );
        assert!(nearest.len() <= NEAR_MISS_LIMIT, "{nearest:?}");
    }

    /// The right name in the wrong group is the other common mistake.
    #[test]
    fn an_app_in_another_group_is_still_offered() {
        let known: Vec<String> = (0..40)
            .map(|n| format!("apps-stateless.filler-{n}"))
            .chain(["apps-jobs.vega-bootstrap-db".to_string()])
            .collect();
        let nearest = nearest_entities(&known, "apps-stateless", "vega-bootstrap-db");
        assert_eq!(
            nearest.first().map(String::as_str),
            Some("apps-jobs.vega-bootstrap-db"),
            "{nearest:?}"
        );
    }

    /// A chart fixture whose value is inherited through a chain, so provenance
    /// has something to attribute.
    fn inherited_chart_fixture() -> tempfile::TempDir {
        let dir = chart_fixture();
        std::fs::write(
            dir.path().join("values.yaml"),
            "\
global:
  env: dev
  _includes:
    base-app:
      priorityClassName:
        _default: standard
        dc1: production-medium
      replicas: 1
    team-app:
      _include: [ \"base-app\" ]
      revisionHistoryLimit: 3
apps-stateless:
  api:
    _include: [ \"team-app\" ]
    enabled: true
    replicas: 4
",
        )
        .expect("write values");
        dir
    }

    /// Knowing what a value is settles far less than knowing which profile set
    /// it -- that is the difference between reading the answer and being able
    /// to change it.
    #[test]
    fn origin_names_the_profile_the_chain_and_the_env_key() {
        let chart = inherited_chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "origin",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api",
                "env": "dc1",
                "values_path": "priorityClassName",
            }),
        )
        .expect("origin");

        assert!(text.contains("production-medium"), "{text}");
        assert!(text.contains("from base-app"), "{text}");
        // Reached through team-app, which is what the app actually names.
        assert!(text.contains("via team-app"), "{text}");
        assert!(text.contains("env key 'dc1'"), "{text}");
        assert!(text.contains("values.yaml:"), "{text}");
    }

    /// The app's own value has to win over everything it inherits, and be
    /// attributed to the app rather than to a profile.
    #[test]
    fn a_value_the_app_writes_itself_is_attributed_to_the_app() {
        let chart = inherited_chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "origin",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api",
                "values_path": "replicas",
            }),
        )
        .expect("origin");
        assert!(text.contains("replicas: 4"), "{text}");
        assert!(text.contains("from api"), "{text}");
        assert!(!text.contains("via"), "{text}");
    }

    /// `_default` is a selection too, and saying which one was taken is the
    /// point of reporting the key at all.
    #[test]
    fn origin_reports_the_default_env_key_when_that_is_what_matched() {
        let chart = inherited_chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "origin",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api",
                "env": "dev",
                "values_path": "priorityClassName",
            }),
        )
        .expect("origin");
        assert!(text.contains("standard"), "{text}");
        assert!(text.contains("env key '_default'"), "{text}");
    }

    #[test]
    fn origin_rejects_a_path_the_app_does_not_have() {
        let chart = inherited_chart_fixture();
        let err = run(
            &context(),
            json!({
                "op": "origin",
                "chart": chart_arg(&chart),
                "group": "apps-stateless",
                "app": "api",
                "values_path": "nosuchthing",
            }),
        )
        .err()
        .expect("an unknown path must fail");
        assert!(err.contains("nosuchthing"), "{err}");
    }

    #[test]
    fn template_returns_the_source_of_a_library_define() {
        let text =
            run(&context(), json!({ "op": "template", "name": "fl.value" })).expect("template");
        assert!(text.contains("define \"fl.value\""), "{text}");
        assert!(
            text.contains("happ://helm-apps/templates/fl-functions/_value.tpl"),
            "{text}"
        );
    }

    /// The block used to be wrapped a second time, so the answer opened with
    /// `{{- define "fl.value" -}}{{- define "fl.value" }}` and closed with two
    /// `end`s -- a fragment no Go template parser accepts.
    #[test]
    fn a_quoted_define_is_emitted_exactly_once() {
        let text =
            run(&context(), json!({ "op": "template", "name": "fl.value" })).expect("template");
        assert_eq!(
            text.matches("define \"fl.value\"").count(),
            1,
            "the define header must appear once: {text}"
        );

        // Re-extracting the block out of the answer must yield the answer's own
        // template text, which only holds if the delimiters are balanced.
        let quoted = text
            .split_once('\n')
            .map(|(_, body)| body)
            .expect("a header line precedes the block");
        let reparsed = crate::templateanalyzer::extract_define_blocks(quoted)
            .remove("fl.value")
            .expect("the quoted block must parse as a define");
        assert_eq!(reparsed.trim(), quoted.trim(), "{text}");
    }

    #[test]
    fn template_also_takes_a_path_inside_the_chart() {
        let text = run(
            &context(),
            json!({ "op": "template", "name": "Chart.yaml" }),
        )
        .expect("template");
        assert!(text.contains("version"), "{text}");
    }

    /// Every answer from this op prints `happ://helm-apps/<path>`, and that is
    /// the string a reader copies back into the next call.
    #[test]
    fn the_uri_the_tool_prints_is_one_it_accepts() {
        let by_path = run(
            &context(),
            json!({ "op": "template", "name": "templates/fl-functions/_value.tpl" }),
        )
        .expect("by path");
        let by_uri = run(
            &context(),
            json!({ "op": "template",
                    "name": "happ://helm-apps/templates/fl-functions/_value.tpl" }),
        )
        .expect("by uri");
        assert_eq!(by_path, by_uri);
    }

    /// 135 functions is 14 KB, and a caller after one family was paying for
    /// every other.
    #[test]
    fn the_function_list_can_be_narrowed() {
        let all = run(&context(), json!({ "op": "contract", "name": "functions" })).expect("all");
        let some = run(
            &context(),
            json!({ "op": "contract", "name": "functions", "query": "fl.value" }),
        )
        .expect("some");
        assert!(
            some.len() * 4 < all.len(),
            "{} vs {}",
            some.len(),
            all.len()
        );
        assert!(some.contains("fl.value"), "{some}");
        assert!(!some.contains("apps-utils.init-library"), "{some}");

        let none = run(
            &context(),
            json!({ "op": "contract", "name": "functions", "query": "no-such-function" }),
        )
        .expect("none");
        assert!(none.contains("Nothing matched"), "{none}");
    }

    /// A deployment passes anything with newlines in it this way -- the chart
    /// measured here sends its CA bundle so -- and no spelling of `set` stands
    /// in: a path written as a value silently becomes that literal string.
    #[test]
    fn set_file_reads_the_file_helm_would_have_read() {
        let chart = chart_fixture();
        let bundle = chart.path().join("bundle.pem");
        std::fs::write(&bundle, "-----BEGIN CERTIFICATE-----\nmulti\nline\n").expect("write");

        let text = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart.path().to_string_lossy(),
                "query": ".global.trustedCA.bundle",
                "set_file": { "global.trustedCA.bundle": "bundle.pem" },
            }),
        )
        .expect("query");
        assert!(text.contains("BEGIN CERTIFICATE"), "{text}");
        assert!(text.contains("multi"), "{text}");

        let err = run(
            &context(),
            json!({
                "op": "query",
                "chart": chart.path().to_string_lossy(),
                "query": ".global",
                "set_file": { "global.trustedCA.bundle": "absent.pem" },
            }),
        )
        .err()
        .expect("missing file must fail");
        assert!(err.contains("absent.pem"), "{err}");
    }

    #[test]
    fn an_unknown_template_suggests_near_misses() {
        let err = run(&context(), json!({ "op": "template", "name": "fl.valu" }))
            .err()
            .expect("unknown template must fail");
        assert!(err.contains("fl.value"), "{err}");
    }

    /// Every group in this contract is named `apps-*`, so the one jq mistake a
    /// caller is guaranteed to make deserves the one hint that fixes it.
    #[test]
    fn a_bare_hyphenated_group_in_a_query_earns_the_quoting_hint() {
        let explained = explain_query_failure("stateless/0 is not defined", ".apps-stateless|keys");
        assert!(explained.contains("[\"apps-stateless\"]"), "{explained}");

        let unrelated = explain_query_failure("boom", "[.[\"apps-stateless\"]|keys]");
        assert_eq!(unrelated, "query failed: boom");
    }

    /// The decoration used to key off any "not found", which caught a werf
    /// failure about a file outside the project and answered it with a list of
    /// apps -- one of which was the app being rendered.
    #[test]
    fn only_a_name_that_missed_earns_the_app_suggestions() {
        let chart = chart_fixture();
        let source = crate::lsp::locate_chart_values(chart.path()).expect("chart source");
        let unrelated = "werf render failed: the file \"../x.json\" not found in the project \
                         directory"
            .to_string();
        assert_eq!(
            explain_missing_entity(unrelated.clone(), &source, "apps-stateless", "api"),
            unrelated
        );
        let missed = explain_missing_entity(
            "app not found at apps-stateless. api".to_string(),
            &source,
            "apps-stateless",
            "ap",
        );
        assert!(missed.contains("This chart has no"), "{missed}");
    }

    const TWO_DOCS: &str = "# a comment the caller wrote\n\
                            apiVersion: apps/v1\nkind: Deployment\n\
                            metadata:\n  name: api\nspec:\n  replicas: 2\n\
                            ---\napiVersion: v1\nkind: Service\n\
                            metadata:\n  name: api\nspec:\n  ports: []\n";

    #[test]
    fn a_render_can_be_narrowed_to_one_resource() {
        let only_service = narrow_rendered(TWO_DOCS, Some("service"), None).expect("service");
        assert!(only_service.contains("kind: Service"), "{only_service}");
        assert!(!only_service.contains("kind: Deployment"), "{only_service}");
        // The chart's own formatting survives, comments included.
        let with_comment = narrow_rendered(TWO_DOCS, Some("Deployment"), None).expect("deployment");
        assert!(
            with_comment.contains("# a comment the caller wrote"),
            "{with_comment}"
        );

        let by_name = narrow_rendered(TWO_DOCS, None, Some("api")).expect("by name");
        assert_eq!(rendered_documents(&by_name).len(), 2);

        let unfiltered = narrow_rendered(TWO_DOCS, None, None).expect("unfiltered");
        assert_eq!(unfiltered, TWO_DOCS);
    }

    /// Asking for a kind the app does not render is a question about what it
    /// does render, so the error answers that instead of just saying no.
    #[test]
    fn narrowing_to_nothing_lists_what_the_app_does_render() {
        let err = narrow_rendered(TWO_DOCS, Some("Ingress"), None)
            .err()
            .expect("no Ingress");
        assert!(err.contains("renders no 'Ingress'"), "{err}");
        assert!(err.contains("Deployment/api"), "{err}");
        assert!(err.contains("Service/api"), "{err}");
    }

    /// The generic cap ends mid-document and says to raise `limit`, which buys
    /// the whole thing again at a higher price. A render is a list of named
    /// resources, so the cap hands that list back.
    #[test]
    fn a_capped_render_names_the_resources_it_cut() {
        let capped = truncate_render(TWO_DOCS.to_string(), &json!({ "limit": 5 }), "# header");
        assert!(capped.contains("It renders 2:"), "{capped}");
        assert!(capped.contains("Deployment/api"), "{capped}");
        assert!(capped.contains("kind= or resource="), "{capped}");
        assert!(capped.contains("[truncated:"), "{capped}");

        let untouched = truncate_render(TWO_DOCS.to_string(), &json!({}), "# header");
        assert!(!untouched.contains("[truncated:"), "{untouched}");
    }

    /// A wrong app name has been answered with near misses for a while. A
    /// wrong path was answered with nothing, so `envvars` for `envVars` cost a
    /// round trip that carried no information back.
    #[test]
    fn a_values_path_that_missed_names_the_keys_that_were_there() {
        let entity = json!({
            "enabled": true,
            "containers": { "main": { "envVars": { "PORT": 8080 }, "image": "x" } }
        });

        let deep =
            explain_missing_subpath(&entity, "containers.main.envvars", "apps-stateless", "api");
        assert!(
            deep.contains("'envvars' is not in apps-stateless.api.containers.main"),
            "{deep}"
        );
        assert!(deep.contains("envVars"), "{deep}");

        let top = explain_missing_subpath(&entity, "replicaz", "apps-stateless", "api");
        assert!(
            top.contains("'replicaz' is not in apps-stateless.api."),
            "{top}"
        );
        assert!(top.contains("enabled"), "{top}");

        // Stopping on a scalar is a different mistake and reads differently.
        let through_scalar = explain_missing_subpath(
            &entity,
            "containers.main.image.name",
            "apps-stateless",
            "api",
        );
        assert!(through_scalar.contains("not a map"), "{through_scalar}");
    }

    #[test]
    fn an_env_the_chart_never_writes_down_is_called_out() {
        let discovery = json!({ "literals": ["dc1", "dev"], "regexes": [] });
        let notice = unknown_env_notice(&discovery, Some("produciton"));
        assert!(
            notice.contains("never mentions env 'produciton'"),
            "{notice}"
        );
        assert!(notice.contains("dc1, dev"), "{notice}");

        assert_eq!(unknown_env_notice(&discovery, Some("dc1")), "");
        assert_eq!(unknown_env_notice(&discovery, None), "");
        // A chart selecting `prod` through `^prod.*$` writes the name nowhere,
        // so a regex key anywhere means happ cannot call a name unknown.
        let with_regex = json!({ "literals": ["dev"], "regexes": ["^prod.*$"] });
        assert_eq!(unknown_env_notice(&with_regex, Some("prod-eu")), "");
    }

    /// A caller with no shell behind it writes the path it read in a README.
    #[test]
    fn a_leading_tilde_is_expanded_like_a_shell_would() {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }
        assert_eq!(
            crate::lsp::expand_home_dir(std::path::Path::new("~/chart")),
            std::path::PathBuf::from(&home).join("chart")
        );
        // `~user` is somebody else's lookup, and a bare path is already fine.
        for untouched in ["~other/chart", "/abs/chart", "rel/chart"] {
            assert_eq!(
                crate::lsp::expand_home_dir(std::path::Path::new(untouched)),
                std::path::PathBuf::from(untouched)
            );
        }
    }

    /// helm-apps ranges over `_include`, so the scalar form fails the render
    /// outright -- while happ's own values expander accepts it. Verified
    /// against `helm template` with helm-apps 1.9.0.
    #[test]
    fn a_scalar_include_is_reported_as_an_error() {
        let chart = chart_fixture();
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\n  _includes:\n    baseline:\n      replicas: 2\napps-stateless:\n  api:\n    enabled: true\n    _include: baseline\n",
        )
        .expect("rewrite values");

        let text = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(text.contains("E_INCLUDE_NOT_A_LIST"), "{text}");
        assert!(text.contains("1 errors"), "{text}");
    }

    #[test]
    fn the_list_form_of_include_is_accepted() {
        let chart = chart_fixture();
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\n  _includes:\n    baseline:\n      replicas: 2\napps-stateless:\n  api:\n    enabled: true\n    _include:\n      - baseline\n",
        )
        .expect("rewrite values");

        let text = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(text.contains("No findings"), "{text}");
    }

    /// A chart that never calls `apps-utils.init-library` renders nothing at
    /// all, and neither helm nor the values file says why.
    #[test]
    fn a_chart_that_never_initialises_the_library_is_reported() {
        let chart = bare_chart_fixture();
        let text = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(text.contains("E_LIBRARY_NOT_INITIALISED"), "{text}");
        assert!(text.contains("apps-utils.init-library"), "{text}");

        std::fs::create_dir_all(chart.path().join("templates")).expect("templates dir");
        std::fs::write(
            chart.path().join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("init template");

        let after = run(
            &context(),
            json!({ "op": "lint", "chart": chart_arg(&chart) }),
        )
        .expect("lint");
        assert!(after.contains("No findings"), "{after}");
    }

    #[test]
    fn resolving_a_chart_that_cannot_render_carries_the_warning() {
        let chart = chart_fixture();
        std::fs::write(
            chart.path().join("values.yaml"),
            "global:\n  env: dev\n  _includes:\n    baseline:\n      replicas: 2\napps-stateless:\n  api:\n    enabled: true\n    _include: baseline\n",
        )
        .expect("rewrite values");

        let text = run(
            &context(),
            json!({
                "op": "resolve", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "api",
            }),
        )
        .expect("resolve");
        assert!(text.contains("will not render as written"), "{text}");
        assert!(text.contains("op='lint'"), "{text}");
    }

    #[test]
    fn a_render_failure_leads_with_its_cause_not_the_trace() {
        let trace =
            "render preview manifest: chart model build failed: template: c/templates/i.yaml:1:4: \
                     executing \"c/templates/i.yaml\" at <include \"apps-utils.init-library\" $>: \
                     error calling include: template: c/charts/helm-apps/templates/x.tpl:42:32: \
                     executing \"fl._getJoinedIncludesInJson\" at <$includesNames>: \
                     range can't iterate over baseline";
        let summary = summarize_render_failure(trace);
        assert!(
            summary.starts_with("range can't iterate over baseline"),
            "{summary}"
        );
        assert!(summary.contains("full renderer trace"), "{summary}");
    }

    #[test]
    fn a_library_error_code_wins_over_the_template_frame() {
        let trace = "Error: execution error at (c/templates/i.yaml:1:4): \
                     [helm-apps:E_UNEXPECTED_LIST] native YAML list is not allowed here | \
                     path=Values.apps-configmaps.x.files | hint=use a block string | docs=docs/faq.md\n\
                     executing \"x\" at <y>: something else";
        let summary = summarize_render_failure(trace);
        assert!(
            summary.starts_with("[helm-apps:E_UNEXPECTED_LIST]"),
            "{summary}"
        );
        assert!(summary.contains("hint=use a block string"), "{summary}");
    }

    /// The preview forces a disabled app on, which is useful but reads as
    /// "this is deployed" unless it is labelled.
    #[test]
    fn rendering_a_disabled_app_says_it_is_disabled() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({
                "op": "render", "chart": chart_arg(&chart),
                "group": "apps-stateless", "app": "worker",
            }),
        )
        .unwrap_or_else(|err| err);
        assert!(
            text.contains("DISABLED") || text.contains("renders no manifests"),
            "a disabled app must never look deployed: {text}"
        );
    }

    #[test]
    fn overview_explains_that_regex_keys_widen_the_usable_environments() {
        let chart = chart_fixture();
        let text = run(
            &context(),
            json!({ "op": "overview", "chart": chart_arg(&chart) }),
        )
        .expect("overview");
        assert!(text.contains("regexKeys"), "{text}");
    }

    #[test]
    fn subpath_selection_walks_maps_and_sequences() {
        let value = json!({ "containers": [{ "name": "main" }] });
        assert_eq!(
            select_subpath(&value, "containers.0.name"),
            Some(json!("main"))
        );
        assert_eq!(select_subpath(&value, "containers.9"), None);
    }
}
