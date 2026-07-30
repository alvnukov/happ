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

use super::{limit_schema, optional_str, required_str, truncate};
use crate::helm_overrides::ValueOverrides;
use crate::lsp::ChartValuesSource;
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
  render    group+app                   the Kubernetes manifests the app produces
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
                        "overview", "apps", "resolve", "render",
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
                                    e.g. '[.[\"apps-stateless\"] | to_entries[] | select(.value.enabled) | .key]'.",
                },
                "name": {
                    "type": "string",
                    "description": "For op='template': a library template path or a `define` name \
                                    such as 'fl.value'. For op='contract': one of groups, \
                                    functions, env.",
                },
                "values_path": {
                    "type": "string",
                    "description": "For op='resolve': dot-separated subpath inside the app, e.g. \
                                    'containers.main.env'. Keys containing dots need op='query'.",
                },
                "apply_includes": {
                    "type": "boolean",
                    "description": "For op='resolve': expand include profiles (default true). \
                                    False shows the values as literally written.",
                },
                "apply_env_resolution": {
                    "type": "boolean",
                    "description": "For op='resolve': collapse env maps (default true). False \
                                    shows every environment's branch side by side.",
                },
                "renderer": {
                    "type": "string",
                    "enum": ["fast", "helm", "werf"],
                    "description": "For op='render': 'fast' (default) renders in-process and needs \
                                    no tooling; 'helm' and 'werf' shell out and match the real \
                                    deployment exactly.",
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
        "render" => render_entity_manifest(context, args)?,
        "lint" => lint_values(context, args)?,
        "diff" => diff_entity_envs(context, args)?,
        "query" => query_values(context, args)?,
        "contract" => library_reference(args)?,
        "template" => library_template(args)?,
        other => {
            return Err(format!(
                "unknown op '{other}' -- expected one of: overview, apps, resolve, render, lint, \
                 diff, query, contract, template"
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
         # op='apps' for exact names, op='lint' for the {} violations above.\n",
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
            .ok_or_else(|| format!("no such path inside {group}.{app}: {values_path}"))?;
    }

    Ok(format!(
        "# {group}.{app} resolved for env '{used_env}'{}\n{}",
        blocking_error_banner(&source),
        to_yaml(&entity)?
    ))
}

/// Warns when the chart carries an error that stops it rendering at all.
///
/// happ resolves values more leniently than the library renders them, so a
/// chart with a fatal contract violation still produces a clean-looking
/// `resolve` answer. Acting on those values would be acting on something that
/// never reaches a cluster, so the answer has to carry the caveat with it.
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
    let fidelity = if renderer == "fast" {
        "\n# The fast renderer runs in-process and is an approximation; \
         renderer='helm' is authoritative."
    } else {
        ""
    };

    Ok(format!(
        "# {group}.{app} rendered for env '{used_env}' by the {renderer} renderer\
         {disabled_note}{fidelity}\n{manifest}",
    ))
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

fn query_values(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let source = chart_source(context, args)?;
    let query = required_str(args, "query")?;
    let (root, used_env) =
        crate::lsp::analysis_resolved_root(&source, optional_str(args, "env"), None, None)?;

    let results = crate::query::run_query_stream(&query, vec![root])
        .map_err(|err| format!("query failed: {err}"))?;
    if results.is_empty() {
        return Ok(format!(
            "Query matched nothing in the values resolved for env '{used_env}'."
        ));
    }

    crate::query::format_output_json_lines(&results, false, false)
        .map_err(|err| format!("format query output: {err}"))
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
        let mut lines = vec![
            "## Template functions the library defines".to_string(),
            String::new(),
            "Callable as `{{ include \"<name>\" (list $ ...) }}`.".to_string(),
            String::new(),
        ];
        for (name, origin) in library_defined_templates() {
            let arity = crate::lsp::library_include_signature(&name)
                .map(|(min, max)| match max {
                    Some(max) if max == min => format!(" -- takes {min} args"),
                    Some(max) => format!(" -- takes {min}..{max} args"),
                    None => format!(" -- takes at least {min} args"),
                })
                .unwrap_or_default();
            lines.push(format!("- `{name}`{arity}  (happ://helm-apps/{origin})"));
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
    let name = required_str(args, "name")?;

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
    if !err.contains("not found") {
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
    format!(
        "{err}\nThis chart has no '{group}.{app}'. It contains: {}",
        known.join(", ")
    )
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
    Ok(source.with_overrides(overrides))
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

/// Finds a caller-named values file, preferring one relative to the chart.
fn resolve_values_file(raw: &str, chart_root: &std::path::Path) -> Result<PathBuf, String> {
    let given = PathBuf::from(raw);
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

    #[test]
    fn an_unknown_template_suggests_near_misses() {
        let err = run(&context(), json!({ "op": "template", "name": "fl.valu" }))
            .err()
            .expect("unknown template must fail");
        assert!(err.contains("fl.value"), "{err}");
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
