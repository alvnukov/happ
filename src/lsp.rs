use crate::go_compat::parse::parse_action_compat;
use crate::gotemplates::{scan_template_actions, GoTemplateScanError};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};
use lsp_types::{
    notification::{Notification as LspNotificationTrait, PublishDiagnostics},
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, Position, PublishDiagnosticsParams, Range, Uri,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use serde_yaml::{Mapping as YamlMapping, Number as YamlNumber, Value as YamlValue};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("lsp currently supports only stdio transport")]
    UnsupportedTransport,
    #[error(transparent)]
    Protocol(#[from] lsp_server::ProtocolError),
    #[error("lsp transport error: {0}")]
    Transport(String),
}

#[derive(Default)]
struct ServerState {
    documents: HashMap<String, DocumentState>,
}

#[derive(Clone)]
struct DocumentState {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListEntitiesParams {
    uri: Option<String>,
    text: Option<String>,
    env: Option<String>,
    apply_includes: Option<bool>,
    apply_env_resolution: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListEntitiesResult {
    groups: Vec<EntityGroup>,
    enabled_entities: Vec<EnabledEntityRef>,
    default_env: String,
    used_env: String,
    env_discovery: EnvironmentDiscovery,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntityGroup {
    name: String,
    apps: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnabledEntityRef {
    group: String,
    app: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveEntityParams {
    uri: Option<String>,
    text: Option<String>,
    group: String,
    app: String,
    env: Option<String>,
    apply_includes: Option<bool>,
    apply_env_resolution: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveEntityResult {
    entity: JsonValue,
    default_env: String,
    used_env: String,
    env_discovery: EnvironmentDiscovery,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderEntityManifestParams {
    uri: Option<String>,
    text: Option<String>,
    group: String,
    app: String,
    env: Option<String>,
    renderer: Option<String>,
    apply_includes: Option<bool>,
    apply_env_resolution: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderEntityManifestResult {
    manifest: String,
    default_env: String,
    used_env: String,
    env_discovery: EnvironmentDiscovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestPreviewRenderer {
    Fast,
    Helm,
    Werf,
}

impl ManifestPreviewRenderer {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("fast") => Ok(Self::Fast),
            Some("helm") => Ok(Self::Helm),
            Some("werf") => Ok(Self::Werf),
            Some(other) => Err(format!("unsupported manifest preview renderer: {other}")),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TemplateAssistParams {
    uri: Option<String>,
    text: Option<String>,
    line: u32,
    character: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateAssistResult {
    inside_template: bool,
    completions: Vec<TemplateAssistCompletion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizeValuesIncludesParams {
    uri: Option<String>,
    text: Option<String>,
    min_profile_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OptimizeValuesIncludesResult {
    optimized_text: String,
    profiles_added: usize,
    changed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateAssistCompletion {
    label: String,
    insert_text: String,
    detail: String,
    kind: String,
    replace_start: u32,
    replace_end: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HappPreviewThemeResult {
    ui: HappPreviewThemeUi,
    syntax: HappPreviewThemeSyntax,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HappPreviewThemeUi {
    bg: String,
    surface: String,
    surface2: String,
    surface3: String,
    surface4: String,
    text: String,
    muted: String,
    accent: String,
    accent2: String,
    border: String,
    danger: String,
    ok: String,
    title: String,
    control_hover_border: String,
    control_focus_border: String,
    control_focus_ring: String,
    quick_env_bg: String,
    quick_env_border: String,
    quick_env_text: String,
    quick_env_hover_bg: String,
    quick_env_hover_border: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HappPreviewThemeSyntax {
    key: String,
    bool: String,
    number: String,
    comment: String,
    string: String,
    block: String,
}

#[derive(Debug)]
struct ResolvedEntityContext {
    root: JsonValue,
    entity: JsonValue,
    global: JsonValue,
    apply_includes: bool,
    default_env: String,
    used_env: String,
    env_discovery: EnvironmentDiscovery,
}

struct ParsedSourceValuesRoot {
    root_map: JsonMap<String, JsonValue>,
    chart_root: Option<PathBuf>,
    include_base_dir: Option<PathBuf>,
    overrides: HashMap<PathBuf, String>,
}

#[derive(Debug, Serialize, Clone)]
struct EnvironmentDiscovery {
    literals: Vec<String>,
    regexes: Vec<String>,
}

pub fn run(args: crate::cli::LspArgs) -> Result<(), Error> {
    if !args.stdio {
        return Err(Error::UnsupportedTransport);
    }
    maybe_start_parent_watchdog(args.parent_pid);

    let (connection, io_threads) = Connection::stdio();
    let server_capabilities = json!({
        "textDocumentSync": 1,
        "experimental": {
            "helmAppsFullLanguageFeatures": false,
            "status": "in-progress",
            "customMethods": [
                "happ/listEntities",
                "happ/resolveEntity",
                "happ/renderEntityManifest",
                "happ/getPreviewTheme",
                "happ/templateAssist",
                "happ/optimizeValuesIncludes"
            ]
        }
    });
    let _initialize_params = connection.initialize(server_capabilities)?;

    let mut state = ServerState::default();
    event_loop(&connection, &mut state)?;
    io_threads
        .join()
        .map_err(|err| Error::Transport(format!("join io threads: {err}")))?;
    Ok(())
}

#[cfg(unix)]
fn maybe_start_parent_watchdog(parent_pid: Option<u32>) {
    let Some(pid) = parent_pid else {
        return;
    };
    if pid == 0 {
        return;
    }
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(2));
        let ok = unsafe { libc::kill(pid as i32, 0) } == 0;
        if ok {
            continue;
        }
        if matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        ) {
            continue;
        }
        std::process::exit(0);
    });
}

#[cfg(not(unix))]
fn maybe_start_parent_watchdog(_parent_pid: Option<u32>) {}

fn event_loop(connection: &Connection, state: &mut ServerState) -> Result<(), Error> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }
                handle_request(connection, state, &req)?;
            }
            Message::Notification(notif) => {
                handle_notification(connection, state, &notif)?;
            }
            Message::Response(_) => {
                // This server does not send requests to client.
            }
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &ServerState,
    req: &Request,
) -> Result<(), Error> {
    match req.method.as_str() {
        "happ/listEntities" => {
            let params: ListEntitiesParams = match serde_json::from_value(req.params.clone()) {
                Ok(v) => v,
                Err(err) => {
                    return send_error(
                        connection,
                        req.id.clone(),
                        -32602,
                        format!("invalid params for happ/listEntities: {err}"),
                    );
                }
            };
            match list_entities_request(state, params) {
                Ok(result) => {
                    let value = serde_json::to_value(result).unwrap_or(JsonValue::Null);
                    send_ok(connection, req.id.clone(), value)
                }
                Err(err) => send_error(connection, req.id.clone(), -32001, err),
            }
        }
        "happ/resolveEntity" => {
            let params: ResolveEntityParams = match serde_json::from_value(req.params.clone()) {
                Ok(v) => v,
                Err(err) => {
                    return send_error(
                        connection,
                        req.id.clone(),
                        -32602,
                        format!("invalid params for happ/resolveEntity: {err}"),
                    );
                }
            };
            match resolve_entity_request(state, params) {
                Ok(result) => {
                    let value = serde_json::to_value(result).unwrap_or(JsonValue::Null);
                    send_ok(connection, req.id.clone(), value)
                }
                Err(err) => send_error(connection, req.id.clone(), -32001, err),
            }
        }
        "happ/renderEntityManifest" => {
            let params: RenderEntityManifestParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(v) => v,
                    Err(err) => {
                        return send_error(
                            connection,
                            req.id.clone(),
                            -32602,
                            format!("invalid params for happ/renderEntityManifest: {err}"),
                        );
                    }
                };
            match render_entity_manifest_request(state, params) {
                Ok(result) => {
                    let value = serde_json::to_value(result).unwrap_or(JsonValue::Null);
                    send_ok(connection, req.id.clone(), value)
                }
                Err(err) => send_error(connection, req.id.clone(), -32001, err),
            }
        }
        "happ/getPreviewTheme" => {
            let value = serde_json::to_value(preview_theme_request()).unwrap_or(JsonValue::Null);
            send_ok(connection, req.id.clone(), value)
        }
        "happ/templateAssist" => {
            let params: TemplateAssistParams = match serde_json::from_value(req.params.clone()) {
                Ok(v) => v,
                Err(err) => {
                    return send_error(
                        connection,
                        req.id.clone(),
                        -32602,
                        format!("invalid params for happ/templateAssist: {err}"),
                    );
                }
            };
            match template_assist_request(state, params) {
                Ok(result) => {
                    let value = serde_json::to_value(result).unwrap_or(JsonValue::Null);
                    send_ok(connection, req.id.clone(), value)
                }
                Err(err) => send_error(connection, req.id.clone(), -32001, err),
            }
        }
        "happ/optimizeValuesIncludes" => {
            let params: OptimizeValuesIncludesParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(v) => v,
                    Err(err) => {
                        return send_error(
                            connection,
                            req.id.clone(),
                            -32602,
                            format!("invalid params for happ/optimizeValuesIncludes: {err}"),
                        );
                    }
                };
            match optimize_values_includes_request(state, params) {
                Ok(result) => {
                    let value = serde_json::to_value(result).unwrap_or(JsonValue::Null);
                    send_ok(connection, req.id.clone(), value)
                }
                Err(err) => send_error(connection, req.id.clone(), -32001, err),
            }
        }
        _ => send_error(
            connection,
            req.id.clone(),
            -32601,
            format!("method not implemented: {}", req.method),
        ),
    }
}

fn handle_notification(
    connection: &Connection,
    state: &mut ServerState,
    notif: &Notification,
) -> Result<(), Error> {
    match notif.method.as_str() {
        "textDocument/didOpen" => {
            let params: DidOpenTextDocumentParams =
                match serde_json::from_value(notif.params.clone()) {
                    Ok(v) => v,
                    Err(_) => return Ok(()),
                };
            let uri = params.text_document.uri;
            let text = params.text_document.text;
            state
                .documents
                .insert(uri.to_string(), DocumentState { text: text.clone() });
            publish_document_diagnostics(connection, &uri, &text)?;
        }
        "textDocument/didChange" => {
            let params: DidChangeTextDocumentParams =
                match serde_json::from_value(notif.params.clone()) {
                    Ok(v) => v,
                    Err(_) => return Ok(()),
                };
            let uri = params.text_document.uri;
            let Some(doc) = state.documents.get_mut(&uri.to_string()) else {
                return Ok(());
            };
            let Some(last) = params.content_changes.last() else {
                return Ok(());
            };
            doc.text = last.text.clone();
            publish_document_diagnostics(connection, &uri, &doc.text)?;
        }
        "textDocument/didClose" => {
            let params: DidCloseTextDocumentParams =
                match serde_json::from_value(notif.params.clone()) {
                    Ok(v) => v,
                    Err(_) => return Ok(()),
                };
            let uri = params.text_document.uri;
            state.documents.remove(&uri.to_string());
            publish_diagnostics(connection, &uri, Vec::new())?;
        }
        _ => {
            // no-op
        }
    }
    Ok(())
}

fn resolve_request_text(
    state: &ServerState,
    uri: Option<&str>,
    text: Option<String>,
) -> Result<String, String> {
    if let Some(text) = text {
        return Ok(text);
    }
    let Some(uri) = uri else {
        return Err("either 'text' or 'uri' must be provided".to_string());
    };
    if let Some(doc) = state.documents.get(uri) {
        return Ok(doc.text.clone());
    }
    if let Some(path) = file_path_from_uri_string(uri) {
        return std::fs::read_to_string(&path)
            .map_err(|err| format!("read document from {uri}: {err}"));
    }
    Err(format!("document not found in LSP state: {uri}"))
}

fn list_entities_request(
    state: &ServerState,
    params: ListEntitiesParams,
) -> Result<ListEntitiesResult, String> {
    let text = resolve_request_text(state, params.uri.as_deref(), params.text)?;

    let request_uri = params
        .uri
        .as_ref()
        .and_then(|value| value.parse::<Uri>().ok());
    let apply_includes = params.apply_includes.unwrap_or(true);
    let apply_env = params.apply_env_resolution.unwrap_or(true);

    let root_map = if apply_includes {
        parse_and_expand_values_root(request_uri.as_ref(), &text)
            .ok_or_else(|| "failed to parse values root with include expansion".to_string())?
    } else {
        parse_yaml_map_to_json_map(&text)?
    };

    let values = JsonValue::Object(root_map);
    let env_discovery = discover_environments(&values);
    let default_env = detect_default_env(&values, &env_discovery);
    let used_env = params.env.unwrap_or_else(|| default_env.clone());
    let visible_values = if apply_env {
        resolve_env_maps(&values, &used_env)
    } else {
        values
    };

    Ok(ListEntitiesResult {
        groups: collect_entity_groups(&visible_values),
        enabled_entities: collect_enabled_entities(&visible_values),
        default_env,
        used_env,
        env_discovery,
    })
}

fn resolve_entity_request(
    state: &ServerState,
    params: ResolveEntityParams,
) -> Result<ResolveEntityResult, String> {
    let context = resolve_entity_context(
        state,
        params.uri,
        params.text,
        params.group,
        params.app,
        params.env,
        params.apply_includes,
        params.apply_env_resolution,
    )?;
    Ok(ResolveEntityResult {
        entity: context.entity,
        default_env: context.default_env,
        used_env: context.used_env,
        env_discovery: context.env_discovery,
    })
}

fn render_entity_manifest_request(
    state: &ServerState,
    params: RenderEntityManifestParams,
) -> Result<RenderEntityManifestResult, String> {
    let request_uri = params
        .uri
        .as_ref()
        .and_then(|value| value.parse::<Uri>().ok());
    let current_path = params.uri.as_deref().and_then(file_path_from_uri_string);
    let renderer = ManifestPreviewRenderer::parse(params.renderer.as_deref())?;
    let request_text = resolve_request_text(state, params.uri.as_deref(), params.text.clone())?;
    let context = resolve_entity_context(
        state,
        params.uri.clone(),
        params.text,
        params.group.clone(),
        params.app.clone(),
        params.env,
        params.apply_includes,
        params.apply_env_resolution,
    )?;
    let parsed_source_root = parse_source_values_root(request_uri.as_ref(), &request_text)
        .ok_or_else(|| "failed to parse values root for manifest preview".to_string())?;
    let fast_source_root = if renderer == ManifestPreviewRenderer::Fast {
        Some(build_fast_manifest_source_root(&parsed_source_root)?)
    } else {
        None
    };
    let parsed_source_root_json = JsonValue::Object(parsed_source_root.root_map.clone());
    let chart_root = parsed_source_root
        .chart_root
        .ok_or_else(|| "chart root not found for manifest preview".to_string())?;
    let manifest = render_manifest_for_entity(
        &chart_root,
        current_path.as_deref(),
        &params.group,
        &params.app,
        fast_source_root
            .as_ref()
            .unwrap_or(&parsed_source_root_json),
        &context.root,
        &context.used_env,
        renderer,
    )?;
    Ok(RenderEntityManifestResult {
        manifest,
        default_env: context.default_env,
        used_env: context.used_env,
        env_discovery: context.env_discovery,
    })
}

fn optimize_values_includes_request(
    state: &ServerState,
    params: OptimizeValuesIncludesParams,
) -> Result<OptimizeValuesIncludesResult, String> {
    let text = resolve_request_text(state, params.uri.as_deref(), params.text)?;
    let values: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|err| format!("parse values yaml: {err}"))?;
    if !values.is_mapping() {
        return Err("values document must be a YAML map".to_string());
    }
    let min_profile_bytes = params.min_profile_bytes.unwrap_or(24).max(1);
    let (mut optimized, report) =
        crate::output::optimize_values_with_include_profiles(&values, min_profile_bytes);
    normalize_include_fields_yaml(&mut optimized);
    let optimized_text = crate::output::values_yaml(&optimized)
        .map_err(|err| format!("serialize optimized values: {err}"))?;
    Ok(OptimizeValuesIncludesResult {
        optimized_text,
        profiles_added: report.profiles_added,
        changed: optimized != values,
    })
}

fn preview_theme_request() -> HappPreviewThemeResult {
    HappPreviewThemeResult {
        ui: HappPreviewThemeUi {
            bg: "#1e1f22".into(),
            surface: "#2b2d30".into(),
            surface2: "#323437".into(),
            surface3: "#25272a".into(),
            surface4: "#2f3238".into(),
            text: "#bcbec4".into(),
            muted: "#7e8288".into(),
            accent: "#7aa2ff".into(),
            accent2: "#6ed1bb".into(),
            border: "#3c3f41".into(),
            danger: "#ff8f8f".into(),
            ok: "#7ad8ab".into(),
            title: "#f3f4f7".into(),
            control_hover_border: "#455368".into(),
            control_focus_border: "#7f9de2".into(),
            control_focus_ring: "rgba(126,156,233,.24)".into(),
            quick_env_bg: "#20242b".into(),
            quick_env_border: "#353c48".into(),
            quick_env_text: "#cdd3dd".into(),
            quick_env_hover_bg: "#2b3240".into(),
            quick_env_hover_border: "#6a7890".into(),
        },
        syntax: HappPreviewThemeSyntax {
            key: "#d19a66".into(),
            bool: "#c678dd".into(),
            number: "#d19a66".into(),
            comment: "#6a8f74".into(),
            string: "#98c379".into(),
            block: "#9aa5b1".into(),
        },
    }
}

fn resolve_entity_context(
    state: &ServerState,
    uri: Option<String>,
    text: Option<String>,
    group: String,
    app: String,
    env: Option<String>,
    apply_includes: Option<bool>,
    apply_env_resolution: Option<bool>,
) -> Result<ResolvedEntityContext, String> {
    let text = resolve_request_text(state, uri.as_deref(), text)?;

    let apply_includes = apply_includes.unwrap_or(true);
    let apply_env = apply_env_resolution.unwrap_or(true);
    let request_uri = uri.as_ref().and_then(|value| value.parse::<Uri>().ok());

    let root_map = if apply_includes {
        parse_and_expand_values_root(request_uri.as_ref(), &text)
            .ok_or_else(|| "failed to parse values root with include expansion".to_string())?
    } else {
        parse_yaml_map_to_json_map(&text)?
    };

    let expanded = JsonValue::Object(root_map.clone());

    let env_discovery = discover_environments(&expanded);
    let default_env = detect_default_env(&expanded, &env_discovery);
    let used_env = env.unwrap_or_else(|| default_env.clone());

    let root = if apply_env {
        resolve_env_maps(&expanded, &used_env)
    } else {
        expanded
    };
    let entity = read_entity(&root, &group, &app)?;
    let global = read_global(&root);

    Ok(ResolvedEntityContext {
        root,
        entity,
        global,
        apply_includes,
        default_env,
        used_env,
        env_discovery,
    })
}

fn collect_entity_groups(values: &JsonValue) -> Vec<EntityGroup> {
    let Some(root) = as_obj(values) else {
        return Vec::new();
    };
    let mut group_names: Vec<String> = root.keys().cloned().collect();
    group_names.sort();

    let mut groups = Vec::new();
    for group_name in group_names {
        if group_name == "global" {
            continue;
        }
        let Some(group_map) = root.get(&group_name).and_then(as_obj) else {
            continue;
        };
        let mut apps: Vec<String> = group_map
            .iter()
            .filter_map(|(name, value)| {
                if name == "__GroupVars__" || !value.is_object() {
                    return None;
                }
                Some(name.clone())
            })
            .collect();
        apps.sort();
        if apps.is_empty() {
            continue;
        }
        groups.push(EntityGroup {
            name: group_name,
            apps,
        });
    }
    groups
}

fn collect_enabled_entities(values: &JsonValue) -> Vec<EnabledEntityRef> {
    let Some(root) = as_obj(values) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut group_names: Vec<&String> = root.keys().collect();
    group_names.sort();

    for group_name in group_names {
        if group_name == "global" {
            continue;
        }
        let Some(group_obj) = root.get(group_name).and_then(as_obj) else {
            continue;
        };
        let mut app_names: Vec<&String> = group_obj.keys().collect();
        app_names.sort();
        for app_name in app_names {
            if app_name == "__GroupVars__" {
                continue;
            }
            let Some(app_value) = group_obj.get(app_name) else {
                continue;
            };
            if !app_value.is_object() || !entity_enabled_in_resolved_root(app_value) {
                continue;
            }
            out.push(EnabledEntityRef {
                group: group_name.clone(),
                app: app_name.clone(),
            });
        }
    }

    out
}

fn render_manifest_for_entity(
    chart_root: &Path,
    current_path: Option<&Path>,
    group: &str,
    app: &str,
    source_root: &JsonValue,
    resolved_root: &JsonValue,
    env: &str,
    renderer: ManifestPreviewRenderer,
) -> Result<String, String> {
    if renderer != ManifestPreviewRenderer::Fast {
        return render_manifest_for_entity_via_cli(
            chart_root,
            current_path,
            group,
            app,
            resolved_root,
            env,
            renderer,
        );
    }

    let values_json =
        build_manifest_render_values_root(source_root, resolved_root, group, app, env)?;
    let temp_dir = tempfile::Builder::new()
        .prefix("happ-lsp-preview-values-")
        .tempdir()
        .map_err(|e| format!("create temp dir for preview values: {e}"))?;
    let values_path = temp_dir.path().join("values.preview.json");
    let values_text = serde_json::to_string_pretty(&values_json)
        .map_err(|e| format!("encode preview values json: {e}"))?;
    std::fs::write(&values_path, values_text.as_bytes())
        .map_err(|e| format!("write preview values json: {e}"))?;
    let chart_dir_text = chart_root.to_string_lossy().to_string();

    let import_args = crate::cli::ImportArgs {
        path: chart_dir_text.clone(),
        env: env.to_string(),
        group_name: "apps-k8s-manifests".into(),
        group_type: "apps-k8s-manifests".into(),
        min_include_bytes: 24,
        include_status: false,
        output: None,
        out_chart_dir: None,
        chart_name: None,
        library_chart_path: None,
        import_strategy: "helpers".into(),
        allow_template_includes: Vec::new(),
        unsupported_template_mode: "error".into(),
        verify_equivalence: false,
        release_name: "happ-lsp-preview".into(),
        namespace: None,
        values_files: vec![values_path.to_string_lossy().to_string()],
        set_values: Vec::new(),
        set_string_values: Vec::new(),
        set_file_values: Vec::new(),
        set_json_values: Vec::new(),
        kube_version: None,
        api_versions: Vec::new(),
        include_crds: false,
        write_rendered_output: None,
    };

    let rendered = crate::source::render_chart_raw(&import_args, &chart_dir_text)
        .map_err(|e| format!("render preview manifest: {e}"))?;
    if rendered.trim().is_empty() {
        return Err("render preview manifest returned empty output".to_string());
    }
    Ok(rendered)
}

fn render_manifest_for_entity_via_cli(
    chart_root: &Path,
    current_path: Option<&Path>,
    group: &str,
    app: &str,
    resolved_root: &JsonValue,
    env: &str,
    renderer: ManifestPreviewRenderer,
) -> Result<String, String> {
    let current_path = current_path.ok_or_else(|| {
        format!(
            "file-backed document is required for {} manifest preview",
            manifest_renderer_label(renderer)
        )
    })?;
    let values_files = resolve_manifest_values_files(chart_root, current_path)?;
    let set_values =
        build_manifest_entity_isolation_set_values_from_resolved_root(resolved_root, group, app)?;
    let work_dir = match renderer {
        ManifestPreviewRenderer::Werf => resolve_werf_project_dir(chart_root),
        _ => chart_root.to_path_buf(),
    };
    let command = manifest_renderer_command(renderer);
    let args = build_manifest_backend_args(renderer, &work_dir, &values_files, &set_values, env);
    let output = Command::new(command)
        .args(&args)
        .current_dir(&work_dir)
        .output()
        .map_err(|err| {
            format!(
                "{} failed: spawn {}: {err}",
                manifest_renderer_label(renderer),
                command
            )
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!(
                "{} failed: {}",
                manifest_renderer_label(renderer),
                if stderr.is_empty() {
                    "render returned empty output".to_string()
                } else {
                    stderr
                }
            ));
        }
        return Ok(format!("{stdout}\n"));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    };
    Err(format!(
        "{} failed: {}",
        manifest_renderer_label(renderer),
        if message.is_empty() {
            format!("process exited with status {}", output.status)
        } else {
            message
        }
    ))
}

fn build_manifest_render_values_root(
    source_root: &JsonValue,
    resolved_root: &JsonValue,
    group: &str,
    app: &str,
    env: &str,
) -> Result<JsonValue, String> {
    let mut values = source_root.clone();
    let root = values
        .as_object_mut()
        .ok_or_else(|| "manifest preview values root must be a YAML map".to_string())?;
    ensure_global_includes_map(root);

    let global = root
        .entry("global".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !global.is_object() {
        *global = JsonValue::Object(JsonMap::new());
    }
    if let Some(global_obj) = global.as_object_mut() {
        global_obj.insert("env".to_string(), JsonValue::String(env.to_string()));
    }

    let resolved_groups = as_obj(resolved_root)
        .ok_or_else(|| "resolved manifest preview values root must be a YAML map".to_string())?;
    for (group_name, group_value) in resolved_groups {
        if group_name == "global" {
            continue;
        }
        let Some(group_obj) = as_obj(group_value) else {
            continue;
        };
        for (app_name, app_value) in group_obj {
            if app_name == "__GroupVars__" || !app_value.is_object() {
                continue;
            }
            if group_name == group && app_name == app {
                continue;
            }
            if entity_enabled_in_resolved_root(app_value) {
                let _ = upsert_entity_enabled_flag(&mut values, group_name, app_name, false, None);
            }
        }
    }

    if !upsert_entity_enabled_flag(&mut values, group, app, true, None) {
        return Err(format!(
            "unable to isolate entity {}.{} for manifest render",
            group, app
        ));
    }

    ensure_fast_preview_werf_context(&mut values, env);

    Ok(values)
}

fn materialize_root_level_include_profiles(source_root: &JsonValue) -> JsonValue {
    let Some(root_map) = as_obj(source_root).cloned() else {
        return source_root.clone();
    };
    let Some(includes_map) = root_map
        .get("global")
        .and_then(as_obj)
        .and_then(|global| global.get("_includes"))
        .and_then(as_obj)
    else {
        return source_root.clone();
    };

    let include_names = normalize_include(root_map.get("_include"));
    if include_names.is_empty() {
        return source_root.clone();
    }

    let mut merged: JsonMap<String, JsonValue> = JsonMap::new();
    let mut cache: HashMap<String, JsonMap<String, JsonValue>> = HashMap::new();
    for include_name in include_names {
        if let Ok(profile) = resolve_profile(&include_name, includes_map, &mut cache, &mut Vec::new())
        {
            merged = merge_maps(&merged, &profile);
        }
    }

    let mut current = root_map;
    current = merge_maps(&merged, &current);
    current.remove("_include");
    JsonValue::Object(current)
}

fn build_fast_manifest_source_root(parsed: &ParsedSourceValuesRoot) -> Result<JsonValue, String> {
    let assembled = assemble_root_level_values_layers(
        &parsed.root_map,
        parsed.include_base_dir.as_deref(),
        &parsed.overrides,
    )?;
    Ok(materialize_root_level_include_profiles(&JsonValue::Object(
        assembled,
    )))
}

fn assemble_root_level_values_layers(
    source_root: &JsonMap<String, JsonValue>,
    include_base_dir: Option<&Path>,
    overrides: &HashMap<PathBuf, String>,
) -> Result<JsonMap<String, JsonValue>, String> {
    let mut root = source_root.clone();

    if let Some(global) = root.get_mut("global").and_then(JsonValue::as_object_mut) {
        if let Some(includes) = global.get_mut("_includes").and_then(JsonValue::as_object_mut) {
            let include_from_file = includes
                .get("_include_from_file")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            if let Some(raw_path) = include_from_file {
                if let Some((_loaded_path, loaded_map)) =
                    load_yaml_map_from_file(&raw_path, include_base_dir, overrides, &mut HashSet::new())?
                {
                    let normalized = normalize_global_includes_payload(&loaded_map);
                    let merged = merge_maps(&normalized, includes);
                    *includes = merged;
                }
                includes.remove("_include_from_file");
            }
        }
    }

    let include_from_file = root
        .get("_include_from_file")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if let Some(raw_path) = include_from_file {
        if let Some((_loaded_path, loaded_map)) =
            load_yaml_map_from_file(&raw_path, include_base_dir, overrides, &mut HashSet::new())?
        {
            root = merge_maps(&loaded_map, &root);
        }
        root.remove("_include_from_file");
    }

    let root_include_files = normalize_include_files(root.get("_include_files"));
    if !root_include_files.is_empty() {
        let mut merged_layers: JsonMap<String, JsonValue> = JsonMap::new();
        for raw_path_value in root_include_files {
            let raw_path = raw_path_value.trim();
            if raw_path.is_empty() {
                continue;
            }
            if let Some((_loaded_path, loaded_map)) =
                load_yaml_map_from_file(raw_path, include_base_dir, overrides, &mut HashSet::new())?
            {
                merged_layers = merge_maps(&merged_layers, &loaded_map);
            }
        }
        root = merge_maps(&merged_layers, &root);
        root.remove("_include_files");
    }

    Ok(root)
}

fn ensure_fast_preview_werf_context(values: &mut JsonValue, env: &str) {
    let Some(root) = values.as_object_mut() else {
        return;
    };
    let werf = root
        .entry("werf".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !werf.is_object() {
        *werf = JsonValue::Object(JsonMap::new());
    }
    let Some(werf_obj) = werf.as_object_mut() else {
        return;
    };
    if !env.trim().is_empty() {
        werf_obj.insert("env".to_string(), JsonValue::String(env.to_string()));
    }
    werf_obj
        .entry("repo".to_string())
        .or_insert_with(|| JsonValue::String(String::new()));
}

fn build_manifest_entity_isolation_set_values_from_resolved_root(
    resolved_root: &JsonValue,
    group: &str,
    app: &str,
) -> Result<Vec<String>, String> {
    let mut out = vec![build_enabled_set_value(group, app, true)];
    let groups = as_obj(resolved_root)
        .ok_or_else(|| "resolved manifest preview values root must be a YAML map".to_string())?;
    let mut target_found = false;

    let mut group_names: Vec<&String> = groups.keys().collect();
    group_names.sort();
    for group_name in group_names {
        if group_name == "global" {
            continue;
        }
        let Some(group_obj) = groups.get(group_name).and_then(as_obj) else {
            continue;
        };
        let mut app_names: Vec<&String> = group_obj.keys().collect();
        app_names.sort();
        for app_name in app_names {
            if app_name == "__GroupVars__" {
                continue;
            }
            let Some(app_value) = group_obj.get(app_name) else {
                continue;
            };
            if !app_value.is_object() {
                continue;
            }
            if group_name == group && app_name == app {
                target_found = true;
                continue;
            }
            if entity_enabled_in_resolved_root(app_value) {
                out.push(build_enabled_set_value(group_name, app_name, false));
            }
        }
    }

    if !target_found {
        return Err(format!(
            "unable to isolate entity {}.{} for manifest render",
            group, app
        ));
    }
    Ok(out)
}

fn build_enabled_set_value(group: &str, app: &str, enabled: bool) -> String {
    format!(
        "{}.{}.enabled={}",
        escape_helm_set_path_segment(group),
        escape_helm_set_path_segment(app),
        if enabled { "true" } else { "false" }
    )
}

fn escape_helm_set_path_segment(segment: &str) -> String {
    segment
        .replace('\\', "\\\\")
        .replace('.', "\\.")
        .replace(',', "\\,")
        .replace('=', "\\=")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn manifest_renderer_command(renderer: ManifestPreviewRenderer) -> &'static str {
    match renderer {
        ManifestPreviewRenderer::Fast | ManifestPreviewRenderer::Helm => "helm",
        ManifestPreviewRenderer::Werf => "werf",
    }
}

fn manifest_renderer_label(renderer: ManifestPreviewRenderer) -> &'static str {
    match renderer {
        ManifestPreviewRenderer::Fast => "fast render",
        ManifestPreviewRenderer::Helm => "helm template",
        ManifestPreviewRenderer::Werf => "werf render",
    }
}

fn build_manifest_backend_args(
    renderer: ManifestPreviewRenderer,
    chart_dir: &Path,
    values_files: &[PathBuf],
    set_values: &[String],
    env: &str,
) -> Vec<String> {
    let mut value_args: Vec<String> = Vec::new();
    for value_file in values_files {
        value_args.push("--values".to_string());
        value_args.push(value_file.to_string_lossy().to_string());
    }

    let mut set_args: Vec<String> = Vec::new();
    for current in set_values {
        if current.trim().is_empty() {
            continue;
        }
        set_args.push("--set".to_string());
        set_args.push(current.trim().to_string());
    }

    let normalized_env = env.trim();
    let mut with_env = |mut args: Vec<String>| {
        if !normalized_env.is_empty() {
            args.push("--set-string".to_string());
            args.push(format!("global.env={normalized_env}"));
        }
        args
    };

    match renderer {
        ManifestPreviewRenderer::Helm | ManifestPreviewRenderer::Fast => with_env({
            let mut args = vec![
                "template".to_string(),
                "helm-apps-preview".to_string(),
                chart_dir.to_string_lossy().to_string(),
            ];
            args.extend(value_args);
            args.extend(set_args);
            args
        }),
        ManifestPreviewRenderer::Werf => with_env({
            let mut args = vec![
                "render".to_string(),
                "--dir".to_string(),
                chart_dir.to_string_lossy().to_string(),
                "--dev".to_string(),
                "--ignore-secret-key".to_string(),
                "--loose-giterminism".to_string(),
            ];
            args.extend(value_args);
            args.extend(set_args);
            if !normalized_env.is_empty() {
                args.push("--env".to_string());
                args.push(normalized_env.to_string());
            }
            args
        }),
    }
}

fn resolve_werf_project_dir(chart_root: &Path) -> PathBuf {
    let mut current = chart_root.to_path_buf();
    loop {
        if current.join("werf.yaml").exists() {
            return current;
        }
        if !current.pop() {
            return chart_root.to_path_buf();
        }
    }
}

fn resolve_manifest_values_files(
    chart_root: &Path,
    current_path: &Path,
) -> Result<Vec<PathBuf>, String> {
    let current_path = normalize_fs_path(current_path);
    let root_documents = find_helm_apps_root_documents(chart_root)?;
    let primary_values = find_primary_values_file(chart_root).map(|path| normalize_fs_path(&path));
    let include_owners = collect_include_owners_for_chart(chart_root)?;
    Ok(select_manifest_values_files(
        &current_path,
        &root_documents,
        primary_values.as_ref(),
        include_owners.get(&current_path),
    ))
}

fn select_manifest_values_files(
    current_path: &Path,
    root_documents: &[PathBuf],
    primary_values: Option<&PathBuf>,
    include_owners: Option<&BTreeSet<PathBuf>>,
) -> Vec<PathBuf> {
    let mut owner_candidates: Vec<PathBuf> = include_owners
        .map(|owners| owners.iter().cloned().collect())
        .unwrap_or_default();
    owner_candidates.sort();

    if !owner_candidates.is_empty() {
        if let Some(primary) = primary_values {
            if owner_candidates
                .iter()
                .any(|candidate| candidate == primary)
            {
                return vec![primary.clone()];
            }
        }
        return vec![owner_candidates[0].clone()];
    }

    if root_documents.iter().any(|root| root == current_path) {
        if let Some(primary) = primary_values {
            if primary != current_path {
                return vec![primary.clone()];
            }
        }
        return vec![current_path.to_path_buf()];
    }

    if let Some(primary) = primary_values {
        return vec![primary.clone()];
    }

    vec![current_path.to_path_buf()]
}

fn find_helm_apps_root_documents(chart_root: &Path) -> Result<Vec<PathBuf>, String> {
    let yaml_files = collect_yaml_files(chart_root)?;
    let mut candidate_roots: Vec<PathBuf> = Vec::new();
    let mut included_by_other_documents: HashSet<PathBuf> = HashSet::new();

    for file_path in yaml_files {
        let text = std::fs::read_to_string(&file_path)
            .map_err(|err| format!("read chart yaml '{}': {err}", file_path.display()))?;
        if looks_like_helm_apps_values_text(&text) {
            candidate_roots.push(file_path.clone());
        }
        let lines: Vec<&str> = text.lines().collect();
        let base_dir = file_path.parent();
        for file_ref in collect_include_file_refs(&lines) {
            if is_templated_include_path(&file_ref.path) {
                continue;
            }
            for candidate in build_include_candidates(&file_ref.path, base_dir) {
                let normalized = normalize_fs_path(&candidate);
                if normalized != file_path {
                    included_by_other_documents.insert(normalized);
                }
            }
        }
    }

    candidate_roots.sort();
    candidate_roots.dedup();
    Ok(candidate_roots
        .into_iter()
        .filter(|path| !included_by_other_documents.contains(path))
        .collect())
}

fn collect_include_owners_for_chart(
    chart_root: &Path,
) -> Result<HashMap<PathBuf, BTreeSet<PathBuf>>, String> {
    let mut owners: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
    for root in find_helm_apps_root_documents(chart_root)? {
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut queue = vec![root.clone()];
        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            let text = match std::fs::read_to_string(&current) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let lines: Vec<&str> = text.lines().collect();
            let base_dir = current.parent();
            for file_ref in collect_include_file_refs(&lines) {
                if is_templated_include_path(&file_ref.path) {
                    continue;
                }
                for candidate in build_include_candidates(&file_ref.path, base_dir) {
                    let normalized = normalize_fs_path(&candidate);
                    if !normalized.exists() {
                        continue;
                    }
                    owners
                        .entry(normalized.clone())
                        .or_default()
                        .insert(root.clone());
                    if !visited.contains(&normalized) {
                        queue.push(normalized.clone());
                    }
                    break;
                }
            }
        }
    }
    Ok(owners)
}

fn collect_yaml_files(chart_root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir)
            .map_err(|err| format!("read chart dir '{}': {err}", dir.display()))?
        {
            let entry = entry.map_err(|err| format!("read chart dir entry: {err}"))?;
            let file_type = entry
                .file_type()
                .map_err(|err| format!("read file type '{}': {err}", entry.path().display()))?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_type.is_dir() {
                if matches!(
                    file_name.as_ref(),
                    ".git" | "node_modules" | "vendor" | "tmp" | ".werf" | "templates"
                ) {
                    continue;
                }
                walk(&entry.path(), out)?;
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let lower = file_name.to_ascii_lowercase();
            if lower.ends_with(".yaml") || lower.ends_with(".yml") {
                out.push(normalize_fs_path(&entry.path()));
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    walk(chart_root, &mut out)?;
    out.sort();
    Ok(out)
}

fn entity_enabled_in_resolved_root(entity: &JsonValue) -> bool {
    as_obj(entity)
        .and_then(|obj| obj.get("enabled"))
        .and_then(JsonValue::as_bool)
        == Some(true)
}

fn upsert_entity_enabled_flag(
    values: &mut JsonValue,
    group: &str,
    app: &str,
    enabled: bool,
    seed_entity: Option<&JsonValue>,
) -> bool {
    let Some(root) = values.as_object_mut() else {
        return false;
    };

    let group_value = root
        .entry(group.to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !group_value.is_object() {
        *group_value = JsonValue::Object(JsonMap::new());
    };
    let Some(group_obj) = group_value.as_object_mut() else {
        return false;
    };

    let mut created = false;
    let app_value = group_obj.entry(app.to_string()).or_insert_with(|| {
        created = true;
        seed_entity
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| JsonValue::Object(JsonMap::new()))
    });
    if created && !app_value.is_object() {
        *app_value = JsonValue::Object(JsonMap::new());
    }
    if !app_value.is_object() {
        *app_value = seed_entity
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| JsonValue::Object(JsonMap::new()));
    };
    let Some(app_obj) = app_value.as_object_mut() else {
        return false;
    };
    app_obj.insert("enabled".to_string(), JsonValue::Bool(enabled));
    true
}

fn json_to_yaml_value(value: &JsonValue) -> Result<YamlValue, String> {
    match value {
        JsonValue::Null => Ok(YamlValue::Null),
        JsonValue::Bool(v) => Ok(YamlValue::Bool(*v)),
        JsonValue::Number(n) => {
            if let Some(v) = n.as_i64() {
                return Ok(YamlValue::Number(YamlNumber::from(v)));
            }
            if let Some(v) = n.as_u64() {
                return Ok(YamlValue::Number(YamlNumber::from(v)));
            }
            if let Some(v) = n.as_f64() {
                if !v.is_finite() {
                    return Err("non-finite float is not supported in preview values".to_string());
                }
                return Ok(YamlValue::Number(YamlNumber::from(v)));
            }
            Err(format!("unsupported json number: {n}"))
        }
        JsonValue::String(v) => Ok(YamlValue::String(v.clone())),
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_yaml_value(item)?);
            }
            Ok(YamlValue::Sequence(out))
        }
        JsonValue::Object(map) => {
            let mut out = YamlMapping::new();
            for (k, v) in map {
                out.insert(YamlValue::String(k.clone()), json_to_yaml_value(v)?);
            }
            Ok(YamlValue::Mapping(out))
        }
    }
}

fn read_entity(values: &JsonValue, group: &str, app: &str) -> Result<JsonValue, String> {
    let root = as_obj(values).ok_or_else(|| "values must be map".to_string())?;
    let group_map = root
        .get(group)
        .and_then(as_obj)
        .ok_or_else(|| format!("group not found: {group}"))?;
    let app_map = group_map
        .get(app)
        .and_then(as_obj)
        .ok_or_else(|| format!("app not found at {group}.{app}"))?;
    Ok(JsonValue::Object(app_map.clone()))
}

fn read_global(values: &JsonValue) -> JsonValue {
    let root = match as_obj(values) {
        Some(v) => v,
        None => return JsonValue::Object(JsonMap::new()),
    };
    root.get("global")
        .and_then(as_obj)
        .map(|m| JsonValue::Object(m.clone()))
        .unwrap_or_else(|| JsonValue::Object(JsonMap::new()))
}

fn build_manifest_preview_values(
    group: &str,
    app: &str,
    entity: &JsonValue,
    global: &JsonValue,
    _apply_includes: bool,
    env: &str,
) -> JsonValue {
    let mut root = JsonMap::new();
    root.insert("global".to_string(), global.clone());
    build_manifest_preview_values_with_root(
        group,
        app,
        entity,
        global,
        &JsonValue::Object(root),
        _apply_includes,
        env,
    )
}

fn build_manifest_preview_values_with_root(
    group: &str,
    app: &str,
    entity: &JsonValue,
    global: &JsonValue,
    root: &JsonValue,
    _apply_includes: bool,
    env: &str,
) -> JsonValue {
    let required_keys = required_preview_global_keys(entity);
    let global_map = build_preview_global_map(global, env, &required_keys);
    let mut required_root_keys = required_preview_root_keys(entity);
    required_root_keys.insert(group.to_string());
    let preview_entity = force_entity_enabled_for_preview(entity);
    let mut out = build_preview_values_tree(group, app, &preview_entity, global_map);
    inject_preview_root_keys(
        &mut out,
        root,
        &required_root_keys,
        group,
        app,
        &preview_entity,
    );
    normalize_include_fields_for_render(&mut out);
    out
}

fn force_entity_enabled_for_preview(entity: &JsonValue) -> JsonValue {
    let Some(entity_map) = entity.as_object() else {
        return entity.clone();
    };
    let mut next = entity_map.clone();
    next.insert("enabled".to_string(), JsonValue::Bool(true));
    JsonValue::Object(next)
}

fn required_preview_global_keys(entity: &JsonValue) -> BTreeSet<String> {
    let mut required_keys = BTreeSet::from([
        "env".to_string(),
        "validation".to_string(),
        "labels".to_string(),
        "deploy".to_string(),
        "releases".to_string(),
    ]);
    collect_global_keys_referenced(entity, &mut required_keys);
    required_keys
}

fn required_preview_root_keys(entity: &JsonValue) -> BTreeSet<String> {
    let mut required_keys = BTreeSet::new();
    collect_root_keys_referenced(entity, &mut required_keys);
    required_keys.remove("global");
    required_keys
}

fn build_preview_global_map(
    global: &JsonValue,
    env: &str,
    required_keys: &BTreeSet<String>,
) -> JsonMap<String, JsonValue> {
    let source_global = as_obj(global).cloned().unwrap_or_default();
    let mut global_map = JsonMap::new();
    for key in required_keys {
        if let Some(value) = source_global.get(key) {
            global_map.insert(key.clone(), value.clone());
        }
    }
    // Manifest preview receives already-resolved entity; keep include storage minimal
    // to avoid unrelated include payload validation side-effects.
    global_map.insert("_includes".to_string(), JsonValue::Object(JsonMap::new()));
    global_map.insert("env".to_string(), JsonValue::String(env.to_string()));
    global_map
}

fn build_preview_values_tree(
    group: &str,
    app: &str,
    entity: &JsonValue,
    global_map: JsonMap<String, JsonValue>,
) -> JsonValue {
    let mut app_map = JsonMap::new();
    app_map.insert(app.to_string(), entity.clone());

    let mut values_map = JsonMap::new();
    values_map.insert("global".to_string(), JsonValue::Object(global_map));
    values_map.insert(group.to_string(), JsonValue::Object(app_map));
    JsonValue::Object(values_map)
}

fn inject_preview_root_keys(
    values: &mut JsonValue,
    root: &JsonValue,
    required_root_keys: &BTreeSet<String>,
    group: &str,
    app: &str,
    entity: &JsonValue,
) {
    let Some(values_map) = as_obj(values).cloned() else {
        return;
    };
    let Some(root_map) = as_obj(root) else {
        return;
    };
    let mut next_values = values_map;
    for key in required_root_keys {
        let Some(source_value) = root_map.get(key) else {
            continue;
        };
        if key == group {
            let Some(source_group) = as_obj(source_value) else {
                continue;
            };
            let mut merged_group = source_group.clone();
            merged_group.insert(app.to_string(), entity.clone());
            next_values.insert(key.clone(), JsonValue::Object(merged_group));
            continue;
        }
        next_values.insert(key.clone(), source_value.clone());
    }
    *values = JsonValue::Object(next_values);
}

fn normalize_include_fields_for_render(value: &mut JsonValue) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                normalize_include_fields_for_render(item);
            }
        }
        JsonValue::Object(map) => {
            for (key, nested) in map.iter_mut() {
                if include_key_requires_list(key.as_str()) && matches!(nested, JsonValue::String(_))
                {
                    let values = normalized_include_entries(nested.as_str().unwrap_or_default());
                    *nested = JsonValue::Array(values.into_iter().map(JsonValue::String).collect());
                }
                normalize_include_fields_for_render(nested);
            }
        }
        _ => {}
    }
}

fn normalize_include_fields_yaml(value: &mut YamlValue) {
    match value {
        YamlValue::Sequence(items) => {
            for item in items {
                normalize_include_fields_yaml(item);
            }
        }
        YamlValue::Mapping(map) => {
            for (key, nested) in map.iter_mut() {
                if key.as_str().is_some_and(include_key_requires_list)
                    && matches!(nested, YamlValue::String(_))
                {
                    let values = normalized_include_entries(nested.as_str().unwrap_or_default());
                    *nested =
                        YamlValue::Sequence(values.into_iter().map(YamlValue::String).collect());
                }
                normalize_include_fields_yaml(nested);
            }
        }
        _ => {}
    }
}

fn include_key_requires_list(key: &str) -> bool {
    matches!(key, "_include" | "_include_files")
}

fn normalized_include_entries(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![trimmed.to_string()]
    }
}

fn collect_global_keys_referenced(value: &JsonValue, out: &mut BTreeSet<String>) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_global_keys_referenced(item, out);
            }
        }
        JsonValue::Object(map) => {
            for item in map.values() {
                collect_global_keys_referenced(item, out);
            }
        }
        JsonValue::String(text) => collect_global_keys_from_template_string(text, out),
        _ => {}
    }
}

fn collect_root_keys_referenced(value: &JsonValue, out: &mut BTreeSet<String>) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_root_keys_referenced(item, out);
            }
        }
        JsonValue::Object(map) => {
            for item in map.values() {
                collect_root_keys_referenced(item, out);
            }
        }
        JsonValue::String(text) => collect_root_keys_from_template_string(text, out),
        _ => {}
    }
}

fn collect_global_keys_from_template_string(text: &str, out: &mut BTreeSet<String>) {
    static GLOBAL_KEY_RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let Some(re) = GLOBAL_KEY_RE
        .get_or_init(|| regex::Regex::new(r"(?:\$?\s*\.)?Values\.global\.([A-Za-z0-9_-]+)").ok())
    else {
        return;
    };
    for captures in re.captures_iter(text) {
        if let Some(m) = captures.get(1) {
            let key = m.as_str().trim();
            if !key.is_empty() {
                out.insert(key.to_string());
            }
        }
    }
}

fn collect_root_keys_from_template_string(text: &str, out: &mut BTreeSet<String>) {
    static ROOT_KEY_RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let Some(re) = ROOT_KEY_RE
        .get_or_init(|| regex::Regex::new(r"(?:\$?\s*\.)?Values\.([A-Za-z0-9_-]+)").ok())
    else {
        return;
    };
    for captures in re.captures_iter(text) {
        if let Some(m) = captures.get(1) {
            let key = m.as_str().trim();
            if !key.is_empty() {
                out.insert(key.to_string());
            }
        }
    }
}

fn publish_document_diagnostics(
    connection: &Connection,
    uri: &Uri,
    text: &str,
) -> Result<(), Error> {
    if !is_helm_apps_values_source(uri, text) {
        return publish_diagnostics(connection, uri, Vec::new());
    }
    let diagnostics = build_diagnostics(uri, text);
    publish_diagnostics(connection, uri, diagnostics)
}

fn is_helm_apps_values_source(uri: &Uri, text: &str) -> bool {
    if looks_like_helm_apps_values_text(text) {
        return true;
    }
    file_path_from_uri_string(&uri.to_string())
        .as_deref()
        .is_some_and(is_werf_secret_values_file)
}

fn publish_diagnostics(
    connection: &Connection,
    uri: &Uri,
    diagnostics: Vec<Diagnostic>,
) -> Result<(), Error> {
    let params = PublishDiagnosticsParams::new(uri.clone(), diagnostics, None);
    let params_value = serde_json::to_value(params)
        .map_err(|e| Error::Transport(format!("serialize diagnostics: {e}")))?;
    let notification = Notification::new(
        <PublishDiagnostics as LspNotificationTrait>::METHOD.to_string(),
        params_value,
    );
    connection
        .sender
        .send(Message::Notification(notification))
        .map_err(|e| Error::Transport(format!("send diagnostics: {e}")))?;
    Ok(())
}

fn build_diagnostics(uri: &Uri, text: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut diagnostics = Vec::new();

    let local_defs = collect_local_include_definitions(&lines);
    let usages = collect_include_usages(&lines);
    let include_file_refs = collect_include_file_refs(&lines);
    let stitched_names = collect_stitched_include_name_context(uri, text);
    let mut defined_names: HashSet<String> = local_defs.iter().map(|d| d.name.clone()).collect();
    defined_names.extend(stitched_names.defined_names);

    for usage in &usages {
        if defined_names.contains(&usage.name) {
            continue;
        }
        diagnostics.push(make_diagnostic(
            usage.line,
            &lines,
            &usage.name,
            DiagnosticSeverity::WARNING,
            format!("Unresolved include profile: {}", usage.name),
            Some("E_UNRESOLVED_INCLUDE".to_string()),
        ));
    }

    let mut used_names: HashSet<String> = usages.iter().map(|u| u.name.clone()).collect();
    used_names.extend(stitched_names.used_names);
    for def in &local_defs {
        if !def.emit_unused_diagnostic {
            continue;
        }
        if used_names.contains(&def.name) {
            continue;
        }
        diagnostics.push(make_diagnostic(
            def.line,
            &lines,
            &def.name,
            DiagnosticSeverity::INFORMATION,
            format!("Unused include profile: {}", def.name),
            None,
        ));
    }

    let include_base_dirs = include_base_dirs_for_diagnostics(uri);
    for file_ref in include_file_refs {
        if is_templated_include_path(&file_ref.path) {
            continue;
        }
        let candidates = build_include_candidates_for_diagnostics(&file_ref.path, &include_base_dirs);
        let found = candidates.iter().any(|candidate| candidate.exists());
        if found {
            continue;
        }
        diagnostics.push(make_diagnostic(
            file_ref.line,
            &lines,
            &file_ref.path,
            DiagnosticSeverity::WARNING,
            format!("Include file not found: {}", file_ref.path),
            Some("E_INCLUDE_FILE_NOT_FOUND".to_string()),
        ));
    }

    diagnostics.extend(build_template_diagnostics(uri, text, &lines));

    diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplatePathRoot {
    Values,
    CurrentApp,
    Release,
    Chart,
    Capabilities,
    Werf,
}

#[derive(Debug, Clone)]
struct TemplatePathRef {
    root: TemplatePathRoot,
    segments: Vec<String>,
    full: String,
}

#[derive(Debug, Clone)]
struct TemplatePathCompletionContext {
    root: TemplatePathRoot,
    parent_segments: Vec<String>,
    query: String,
    replace_start_byte: usize,
    replace_end_byte: usize,
}

#[derive(Debug, Clone)]
struct IncludeCallRef {
    name: String,
    list_arg_count: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct IncludeArity {
    min: usize,
    max: Option<usize>,
}

fn template_assist_request(
    state: &ServerState,
    params: TemplateAssistParams,
) -> Result<TemplateAssistResult, String> {
    let request_uri = params
        .uri
        .as_ref()
        .and_then(|value| value.parse::<Uri>().ok());
    let text = resolve_request_text(state, params.uri.as_deref(), params.text)?;

    let line_index = TextLineIndex::new(&text);
    let lines: Vec<&str> = text.split('\n').collect();
    let line = params.line as usize;
    let line_text = lines.get(line).copied().unwrap_or_default();
    let cursor_col_utf16 = params.character as usize;
    let cursor_offset = line_index.offset_for_line_utf16(line_text, line, cursor_col_utf16);

    let Some((action_start, action_end)) = find_template_action_at_cursor(&text, cursor_offset)
    else {
        return Ok(TemplateAssistResult {
            inside_template: false,
            completions: Vec::new(),
        });
    };

    let action_text = &text[action_start..action_end];
    let cursor_in_action = cursor_offset.saturating_sub(action_start);

    let expanded_values =
        parse_and_expand_values_root(request_uri.as_ref(), &text).map(JsonValue::Object);
    let mut completions = Vec::new();

    if let Some(path_ctx) = find_template_path_completion_context(action_text, cursor_in_action) {
        let replace_start_abs = action_start + path_ctx.replace_start_byte;
        let replace_end_abs = action_start + path_ctx.replace_end_byte;
        let replace_start = line_index.utf16_col_for_offset(line_text, line, replace_start_abs);
        let replace_end = line_index.utf16_col_for_offset(line_text, line, replace_end_abs);

        let candidate_keys = match path_ctx.root {
            TemplatePathRoot::Values => expanded_values.as_ref().map_or_else(Vec::new, |root| {
                keys_for_values_path(root, &path_ctx.parent_segments)
            }),
            TemplatePathRoot::CurrentApp => {
                expanded_values.as_ref().map_or_else(Vec::new, |root| {
                    resolve_current_app_value_at_line(root, &lines, line)
                        .as_ref()
                        .and_then(|app_obj| object_at_path(app_obj, &path_ctx.parent_segments))
                        .map(sorted_keys)
                        .map(|mut keys| {
                            if path_ctx.parent_segments.is_empty() {
                                add_missing_keys(&mut keys, CURRENT_APP_RUNTIME_KEYS);
                            }
                            keys
                        })
                        .unwrap_or_else(|| {
                            if path_ctx.parent_segments.is_empty() {
                                CURRENT_APP_RUNTIME_KEYS
                                    .iter()
                                    .map(|key| (*key).to_string())
                                    .collect()
                            } else {
                                Vec::new()
                            }
                        })
                })
            }
            TemplatePathRoot::Release => {
                keys_for_builtin_root_path(TemplatePathRoot::Release, &path_ctx.parent_segments)
            }
            TemplatePathRoot::Chart => {
                keys_for_builtin_root_path(TemplatePathRoot::Chart, &path_ctx.parent_segments)
            }
            TemplatePathRoot::Capabilities => keys_for_builtin_root_path(
                TemplatePathRoot::Capabilities,
                &path_ctx.parent_segments,
            ),
            TemplatePathRoot::Werf => {
                keys_for_builtin_root_path(TemplatePathRoot::Werf, &path_ctx.parent_segments)
            }
        };

        for key in candidate_keys {
            if !path_ctx.query.is_empty() && !key.starts_with(&path_ctx.query) {
                continue;
            }
            completions.push(TemplateAssistCompletion {
                label: key.clone(),
                insert_text: key,
                detail: match path_ctx.root {
                    TemplatePathRoot::Values => "$.Values".to_string(),
                    TemplatePathRoot::CurrentApp => "$.CurrentApp".to_string(),
                    TemplatePathRoot::Release => "$.Release".to_string(),
                    TemplatePathRoot::Chart => "$.Chart".to_string(),
                    TemplatePathRoot::Capabilities => "$.Capabilities".to_string(),
                    TemplatePathRoot::Werf => "$.werf".to_string(),
                },
                kind: "property".to_string(),
                replace_start,
                replace_end,
            });
        }
    } else {
        let cursor_utf16 = params.character;
        completions.push(TemplateAssistCompletion {
            label: "fl.value".to_string(),
            insert_text: "include \"fl.value\" (list $ . ${1:value})".to_string(),
            detail: "Render template-aware value".to_string(),
            kind: "snippet".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "fl.valueQuoted".to_string(),
            insert_text: "include \"fl.valueQuoted\" (list $ . ${1:value})".to_string(),
            detail: "Render value and quote result".to_string(),
            kind: "snippet".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.Values".to_string(),
            insert_text: "$.Values".to_string(),
            detail: "Root values map".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.CurrentApp".to_string(),
            insert_text: "$.CurrentApp".to_string(),
            detail: "Current app map".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.Release".to_string(),
            insert_text: "$.Release".to_string(),
            detail: "Helm release metadata".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.Chart".to_string(),
            insert_text: "$.Chart".to_string(),
            detail: "Helm chart metadata".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.Capabilities".to_string(),
            insert_text: "$.Capabilities".to_string(),
            detail: "Helm cluster capabilities".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
        completions.push(TemplateAssistCompletion {
            label: "$.werf".to_string(),
            insert_text: "$.werf".to_string(),
            detail: "Werf runtime context".to_string(),
            kind: "keyword".to_string(),
            replace_start: cursor_utf16,
            replace_end: cursor_utf16,
        });
    }

    Ok(TemplateAssistResult {
        inside_template: true,
        completions,
    })
}

fn build_template_diagnostics(_uri: &Uri, text: &str, lines: &[&str]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let line_index = TextLineIndex::new(text);
    let expanded_values = parse_and_expand_values_root(Some(_uri), text).map(JsonValue::Object);

    let (spans, scan_errors) = scan_template_actions(text);
    for err in scan_errors {
        let line = line_index.line_for_offset(err.offset);
        diagnostics.push(make_diagnostic(
            line,
            lines,
            "{{",
            DiagnosticSeverity::WARNING,
            format!("Template syntax error: {}", err.message),
            Some("E_TPL_PARSE".to_string()),
        ));
    }

    for typo in collect_single_left_delim_typos(text, &spans) {
        diagnostics.push(make_diagnostic_at_offset(
            &line_index,
            lines,
            typo.offset,
            typo.len,
            DiagnosticSeverity::WARNING,
            "Possible template typo: expected '{{ ... }}', found '{ ... }}'".to_string(),
            Some("E_TPL_SINGLE_LEFT_DELIM".to_string()),
        ));
    }

    for span in &spans {
        let action = &text[span.start..span.end];
        let line = line_index.line_for_offset(span.start);

        if let Err(err) = parse_action_rust_native(action, span.start) {
            diagnostics.push(make_diagnostic(
                line,
                lines,
                "{{",
                DiagnosticSeverity::WARNING,
                format!(
                    "Possible template issue: {} (soft check for values rendered via fl.value)",
                    err.message
                ),
                Some("E_TPL_SOFT_PARSE".to_string()),
            ));
        }

        for local_values_ref in collect_local_values_refs_in_action(action) {
            diagnostics.push(make_diagnostic(
                line,
                lines,
                &local_values_ref,
                DiagnosticSeverity::WARNING,
                format!(
                    "Local template path '{}' is not supported in library context; use '$.Values...'",
                    local_values_ref
                ),
                Some("E_TPL_LOCAL_VALUES_CONTEXT".to_string()),
            ));
        }

        for include_call in collect_include_calls_in_action(action) {
            let Some(signature) = library_include_arity(&include_call.name) else {
                continue;
            };
            let Some(argc) = include_call.list_arg_count else {
                continue;
            };
            if argc < signature.min || signature.max.is_some_and(|max| argc > max) {
                let expected = match signature.max {
                    Some(max) if max == signature.min => format!("exactly {}", signature.min),
                    Some(max) => format!("{}..{}", signature.min, max),
                    None => format!("at least {}", signature.min),
                };
                diagnostics.push(make_diagnostic(
                    line,
                    lines,
                    &include_call.name,
                    DiagnosticSeverity::WARNING,
                    format!(
                        "Library include '{}' expects list args count {}, got {}",
                        include_call.name, expected, argc
                    ),
                    Some("E_INCLUDE_ARGC".to_string()),
                ));
            }
        }

        let Some(root_value) = expanded_values.as_ref() else {
            continue;
        };
        for path_ref in collect_template_path_refs_in_action(action) {
            match path_ref.root {
                TemplatePathRoot::Values => {
                    if !value_has_path_or_virtual(
                        root_value,
                        TemplatePathRoot::Values,
                        &path_ref.segments,
                    ) {
                        diagnostics.push(make_diagnostic(
                            line,
                            lines,
                            &path_ref.full,
                            DiagnosticSeverity::WARNING,
                            format!(
                                "Unknown template path: {} (not found in $.Values)",
                                path_ref.full
                            ),
                            Some("E_TPL_UNKNOWN_VALUES_PATH".to_string()),
                        ));
                    }
                }
                TemplatePathRoot::CurrentApp => {
                    let Some(current_app) =
                        resolve_current_app_value_at_line(root_value, lines, line)
                    else {
                        diagnostics.push(make_diagnostic(
                            line,
                            lines,
                            &path_ref.full,
                            DiagnosticSeverity::WARNING,
                            format!(
                                "Template path {} uses $.CurrentApp outside app scope",
                                path_ref.full
                            ),
                            Some("E_TPL_CURRENT_APP_SCOPE".to_string()),
                        ));
                        continue;
                    };
                    if !value_has_path_or_virtual(
                        &current_app,
                        TemplatePathRoot::CurrentApp,
                        &path_ref.segments,
                    ) {
                        diagnostics.push(make_diagnostic(
                            line,
                            lines,
                            &path_ref.full,
                            DiagnosticSeverity::WARNING,
                            format!(
                                "Unknown template path: {} (not found in $.CurrentApp)",
                                path_ref.full
                            ),
                            Some("E_TPL_UNKNOWN_CURRENT_APP_PATH".to_string()),
                        ));
                    }
                }
                TemplatePathRoot::Release
                | TemplatePathRoot::Chart
                | TemplatePathRoot::Capabilities
                | TemplatePathRoot::Werf => {
                    if !builtin_root_has_path(path_ref.root, &path_ref.segments) {
                        let root_name = template_root_label(path_ref.root);
                        diagnostics.push(make_diagnostic(
                            line,
                            lines,
                            &path_ref.full,
                            DiagnosticSeverity::WARNING,
                            format!(
                                "Unknown template path: {} (not found in {})",
                                path_ref.full, root_name
                            ),
                            Some("E_TPL_UNKNOWN_BUILTIN_PATH".to_string()),
                        ));
                    }
                }
            }
        }
    }

    diagnostics
}

#[derive(Debug, Clone, Copy)]
struct SingleLeftDelimTypo {
    offset: usize,
    len: usize,
}

fn collect_single_left_delim_typos(
    text: &str,
    spans: &[crate::gotemplates::GoTemplateActionSpan],
) -> Vec<SingleLeftDelimTypo> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut span_idx = 0usize;

    while i + 1 < bytes.len() {
        while span_idx < spans.len() && i >= spans[span_idx].end {
            span_idx += 1;
        }
        if span_idx < spans.len() && i >= spans[span_idx].start && i < spans[span_idx].end {
            i = spans[span_idx].end;
            continue;
        }

        if bytes[i] != b'{' || bytes[i + 1] == b'{' {
            i += 1;
            continue;
        }

        let search_from = i + 1;
        let Some(close_rel) = text[search_from..].find("}}") else {
            i += 1;
            continue;
        };
        let close_start = search_from + close_rel;
        let close_end = close_start + 2;
        let Some(inner) = text.get(search_from..close_start) else {
            i += 1;
            continue;
        };
        let synthesized = format!("{{{{{inner}}}}}");
        if parse_action_rust_native(&synthesized, i).is_ok() {
            out.push(SingleLeftDelimTypo {
                offset: i,
                len: close_end.saturating_sub(i),
            });
            i = close_end;
            continue;
        }

        i += 1;
    }

    out
}

fn parse_action_rust_native(action: &str, action_start: usize) -> Result<(), GoTemplateScanError> {
    // LSP soft diagnostics use Rust-native parser only (no Go FFI runtime).
    parse_action_compat(action, action_start).map(|_| ())
}

fn library_include_arity(name: &str) -> Option<IncludeArity> {
    match name {
        "fl.value" | "fl.valueQuoted" | "fl.valueSingleQuoted" | "fl.isTrue" | "fl.isFalse" => {
            Some(IncludeArity {
                min: 3,
                max: Some(4),
            })
        }
        "fl.currentEnv" => Some(IncludeArity {
            min: 1,
            max: Some(1),
        }),
        "fl.expandIncludesInValues" => Some(IncludeArity {
            min: 2,
            max: Some(2),
        }),
        "apps-utils.error" => Some(IncludeArity {
            min: 5,
            max: Some(6),
        }),
        _ => None,
    }
}

const CURRENT_APP_RUNTIME_KEYS: &[&str] = &[
    "CurrentAppVersion",
    "CurrentReleaseVersion",
    "__AppName__",
    "__Rendered__",
    "_currentContainersType",
    "__annotations__",
    "_options",
];

const VALUES_RUNTIME_ROOT_KEYS: &[&str] = &[
    "werf",
    "deploy",
    "releases",
    "global",
    "enabled",
    "helm-apps",
];

fn template_root_label(root: TemplatePathRoot) -> &'static str {
    match root {
        TemplatePathRoot::Values => "$.Values",
        TemplatePathRoot::CurrentApp => "$.CurrentApp",
        TemplatePathRoot::Release => "$.Release",
        TemplatePathRoot::Chart => "$.Chart",
        TemplatePathRoot::Capabilities => "$.Capabilities",
        TemplatePathRoot::Werf => "$.werf",
    }
}

fn builtin_root_has_path(root: TemplatePathRoot, segments: &[String]) -> bool {
    let Some(schema) = builtin_root_schema(root) else {
        return false;
    };
    value_has_path(schema, segments)
}

fn keys_for_builtin_root_path(root: TemplatePathRoot, parent_segments: &[String]) -> Vec<String> {
    let Some(schema) = builtin_root_schema(root) else {
        return Vec::new();
    };
    object_at_path(schema, parent_segments)
        .map(sorted_keys)
        .unwrap_or_default()
}

fn builtin_root_schema(root: TemplatePathRoot) -> Option<&'static JsonValue> {
    match root {
        TemplatePathRoot::Release => {
            static RELEASE: std::sync::OnceLock<JsonValue> = std::sync::OnceLock::new();
            Some(RELEASE.get_or_init(|| {
                json!({
                    "Name": "",
                    "Namespace": "",
                    "Service": "",
                    "IsInstall": true,
                    "IsUpgrade": false,
                    "Revision": 1
                })
            }))
        }
        TemplatePathRoot::Chart => {
            static CHART: std::sync::OnceLock<JsonValue> = std::sync::OnceLock::new();
            Some(CHART.get_or_init(|| {
                json!({
                    "Name": "",
                    "Version": "",
                    "AppVersion": "",
                    "Type": "",
                    "Description": "",
                    "Home": "",
                    "Icon": "",
                    "ApiVersion": "",
                    "Keywords": [],
                    "Sources": [],
                    "Maintainers": [],
                    "Annotations": {}
                })
            }))
        }
        TemplatePathRoot::Capabilities => {
            static CAPABILITIES: std::sync::OnceLock<JsonValue> = std::sync::OnceLock::new();
            Some(CAPABILITIES.get_or_init(|| {
                json!({
                    "KubeVersion": {
                        "Version": "",
                        "Major": "",
                        "Minor": "",
                        "GitVersion": "",
                        "GitCommit": "",
                        "GitTreeState": "",
                        "BuildDate": "",
                        "GoVersion": "",
                        "Compiler": "",
                        "Platform": ""
                    },
                    "HelmVersion": {
                        "Version": "",
                        "GitCommit": "",
                        "GitTreeState": "",
                        "GoVersion": ""
                    },
                    "APIVersions": {
                        "Has": ""
                    }
                })
            }))
        }
        TemplatePathRoot::Werf => {
            static WERF: std::sync::OnceLock<JsonValue> = std::sync::OnceLock::new();
            Some(WERF.get_or_init(|| {
                json!({
                    "env": "",
                    "namespace": "",
                    "name": "",
                    "repo": "",
                    "tag": "",
                    "commit": ""
                })
            }))
        }
        _ => None,
    }
}

fn parse_source_values_root(uri: Option<&Uri>, text: &str) -> Option<ParsedSourceValuesRoot> {
    let mut overrides: HashMap<PathBuf, String> = HashMap::new();
    let mut current_base_dir: Option<PathBuf> = None;
    let mut root_source_path: Option<PathBuf> = None;
    let mut root_map: Option<JsonMap<String, JsonValue>> = None;
    let mut chart_root: Option<PathBuf> = None;

    if let Some(uri) = uri {
        if let Some(current_path) = file_path_from_uri_string(&uri.to_string()) {
            current_base_dir = current_path.parent().map(Path::to_path_buf);
            overrides.insert(normalize_fs_path(&current_path), text.to_string());

            if let Some(found_chart_root) = find_chart_root_from_path(&current_path) {
                chart_root = Some(found_chart_root.clone());
                if let Some(root_values_path) = find_primary_values_file(&found_chart_root) {
                    let root_text =
                        read_text_from_path_with_overrides(&root_values_path, &overrides).ok()?;
                    if let Ok(parsed_root_map) = parse_yaml_map_to_json_map(&root_text) {
                        root_source_path = Some(root_values_path);
                        root_map = Some(parsed_root_map);
                    }
                }

                if root_map.is_none() {
                    let parsed_current = parse_yaml_map_to_json_map(text).ok()?;
                    root_source_path = Some(current_path.clone());
                    root_map = Some(parsed_current);
                }

                if let Some(secret_values_path) = find_werf_secret_values_file(&found_chart_root) {
                    let secret_path_norm = normalize_fs_path(&secret_values_path);
                    let root_path_norm = root_source_path.as_deref().map(normalize_fs_path);
                    if root_path_norm.as_ref() != Some(&secret_path_norm) {
                        let secret_text =
                            read_text_from_path_with_overrides(&secret_values_path, &overrides)
                                .ok()?;
                        let secret_map = parse_yaml_map_to_json_map(&secret_text).ok()?;
                        let merged_base = root_map.take().unwrap_or_default();
                        root_map = Some(merge_maps(&merged_base, &secret_map));
                    }
                }
            }
        }
    }

    let root_map = match root_map {
        Some(root_map) => root_map,
        None => parse_yaml_map_to_json_map(text).ok()?,
    };
    let include_base_dir = root_source_path
        .as_deref()
        .and_then(Path::parent)
        .or(current_base_dir.as_deref())
        .map(Path::to_path_buf);

    Some(ParsedSourceValuesRoot {
        root_map,
        chart_root,
        include_base_dir,
        overrides,
    })
}

fn parse_values_root_with_file_includes(
    uri: Option<&Uri>,
    text: &str,
) -> Option<(JsonMap<String, JsonValue>, Option<PathBuf>)> {
    let parsed = parse_source_values_root(uri, text)?;
    let with_files = expand_values_with_file_includes(
        &parsed.root_map,
        parsed.include_base_dir.as_deref(),
        &parsed.overrides,
    )
    .ok()?;
    Some((with_files, parsed.chart_root))
}

fn parse_and_expand_values_root(
    uri: Option<&Uri>,
    text: &str,
) -> Option<JsonMap<String, JsonValue>> {
    let (with_files, _) = parse_values_root_with_file_includes(uri, text)?;
    expand_includes_in_values(&with_files).ok()
}

fn parse_yaml_map_to_json_map(text: &str) -> Result<JsonMap<String, JsonValue>, String> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).map_err(|err| err.to_string())?;
    let root_json: JsonValue = serde_json::to_value(yaml).map_err(|err| err.to_string())?;
    as_obj(&root_json)
        .cloned()
        .ok_or_else(|| "values document must be a YAML map".to_string())
}

fn normalize_fs_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
    for candidate in candidates {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn find_werf_secret_values_file(chart_root: &Path) -> Option<PathBuf> {
    let candidates = [
        chart_root.join("secret-values.yaml"),
        chart_root.join("secret-values.yml"),
    ];
    for candidate in candidates {
        if candidate.exists() && is_werf_secret_values_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_werf_secret_values_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let normalized = name.to_ascii_lowercase();
            normalized == "secret-values.yaml" || normalized == "secret-values.yml"
        })
        .unwrap_or(false)
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
                .and_then(|value| value.as_str())
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
            .is_some_and(|value| value.is_object())
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
    std::fs::read_to_string(path)
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

fn keys_for_values_path(value: &JsonValue, parent_segments: &[String]) -> Vec<String> {
    if let Some(map) = object_at_path(value, parent_segments) {
        let mut keys = sorted_keys(map);
        if parent_segments.is_empty() {
            add_missing_keys(&mut keys, VALUES_RUNTIME_ROOT_KEYS);
        }
        return keys;
    }
    if parent_segments.is_empty() {
        return VALUES_RUNTIME_ROOT_KEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect();
    }
    Vec::new()
}

fn add_missing_keys(keys: &mut Vec<String>, extra: &[&str]) {
    let mut seen: HashSet<String> = keys.iter().cloned().collect();
    for key in extra {
        if seen.insert((*key).to_string()) {
            keys.push((*key).to_string());
        }
    }
    keys.sort();
}

fn value_has_path_or_virtual(
    value: &JsonValue,
    root: TemplatePathRoot,
    segments: &[String],
) -> bool {
    if segments.is_empty() {
        return true;
    }
    if value_has_path(value, segments) {
        return true;
    }
    let Some(first) = segments.first().map(String::as_str) else {
        return false;
    };
    match root {
        TemplatePathRoot::Values => {
            if is_dynamic_values_path(segments) {
                return true;
            }
            let top_exists = has_top_level_key(value, first);
            if top_exists {
                return segments.len() == 1;
            }
            VALUES_RUNTIME_ROOT_KEYS.iter().any(|key| *key == first) && segments.len() == 1
        }
        TemplatePathRoot::CurrentApp => {
            if matches!(
                first,
                "CurrentAppVersion"
                    | "CurrentReleaseVersion"
                    | "__AppName__"
                    | "__Rendered__"
                    | "_currentContainersType"
                    | "__annotations__"
                    | "_options"
            ) {
                return true;
            }
            if is_dynamic_current_app_key(first) {
                return true;
            }
            let top_exists = has_top_level_key(value, first);
            if top_exists {
                return segments.len() == 1;
            }
            false
        }
        TemplatePathRoot::Release
        | TemplatePathRoot::Chart
        | TemplatePathRoot::Capabilities
        | TemplatePathRoot::Werf => false,
    }
}

fn has_top_level_key(value: &JsonValue, key: &str) -> bool {
    as_obj(value).is_some_and(|map| map.contains_key(key))
}

fn is_dynamic_values_path(segments: &[String]) -> bool {
    segments.len() == 2 && segments[0] == "global" && segments[1] == "env"
}

fn is_dynamic_current_app_key(key: &str) -> bool {
    matches!(
        key,
        // Runtime/default keys provided by library flow and pre-render hooks.
        "name"
            | "service"
            | "ingress"
            | "labels"
            | "annotations"
            | "werfSkipLogs"
            | "restartOnDeploy"
            | "randomName"
            | "_options"
    )
}

fn resolve_current_app_value_at_line(
    root: &JsonValue,
    lines: &[&str],
    line: usize,
) -> Option<JsonValue> {
    let path = key_path_at_line(lines, line);
    if path.len() >= 2 && path[0] != "global" && path[1] != "__GroupVars__" {
        if let Some(value) = as_obj(root)
            .and_then(|root_map| root_map.get(&path[0]))
            .and_then(as_obj)
            .and_then(|group_map| group_map.get(&path[1]))
            .cloned()
        {
            return Some(value);
        }
    }
    if path.len() >= 3 && path[0] == "global" && path[1] == "_includes" {
        return resolve_include_profile_from_root(root, &path[2]).or_else(|| {
            as_obj(root)
                .and_then(|root_map| root_map.get("global"))
                .and_then(as_obj)
                .and_then(|global_map| global_map.get("_includes"))
                .and_then(as_obj)
                .and_then(|includes_map| includes_map.get(&path[2]))
                .cloned()
        });
    }
    if let Some(top_key) = path.first() {
        if !is_reserved_top_level_key(top_key) {
            if let Some(top_level) = as_obj(root)
                .and_then(|root_map| root_map.get(top_key))
                .cloned()
            {
                return Some(top_level);
            }
            if let Some(include_profile) = resolve_include_profile_from_root(root, top_key) {
                return Some(include_profile);
            }
            if let Some(include_profile) = as_obj(root)
                .and_then(|root_map| root_map.get("global"))
                .and_then(as_obj)
                .and_then(|global_map| global_map.get("_includes"))
                .and_then(as_obj)
                .and_then(|includes_map| includes_map.get(top_key))
                .cloned()
            {
                return Some(include_profile);
            }
        }
    }
    None
}

fn resolve_include_profile_from_root(root: &JsonValue, profile_name: &str) -> Option<JsonValue> {
    let includes_map = as_obj(root)
        .and_then(|root_map| root_map.get("global"))
        .and_then(as_obj)
        .and_then(|global_map| global_map.get("_includes"))
        .and_then(as_obj)?;
    let resolved = resolve_profile(
        profile_name,
        includes_map,
        &mut HashMap::new(),
        &mut Vec::new(),
    )
    .ok()?;
    Some(JsonValue::Object(resolved))
}

fn key_path_at_line(lines: &[&str], line: usize) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let blocked = block_scalar_content_lines(lines);
    let mut stack: Vec<(usize, String)> = Vec::new();
    let end = line.min(lines.len().saturating_sub(1));
    for (index, current_line) in lines.iter().enumerate().take(end + 1) {
        if blocked.get(index).copied().unwrap_or(false) {
            continue;
        }
        let trimmed = current_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((indent, key, _)) = parse_key_line(current_line) else {
            continue;
        };
        while stack
            .last()
            .is_some_and(|(stack_indent, _)| *stack_indent >= indent)
        {
            stack.pop();
        }
        stack.push((indent, key.to_string()));
        if index == end {
            break;
        }
    }
    stack.into_iter().map(|(_, key)| key).collect()
}

fn find_template_action_at_cursor(text: &str, cursor_offset: usize) -> Option<(usize, usize)> {
    let (spans, _errors) = scan_template_actions(text);
    for span in spans {
        if cursor_offset >= span.start && cursor_offset <= span.end {
            return Some((span.start, span.end));
        }
    }
    if cursor_offset > text.len() {
        return None;
    }
    let before = text.get(..cursor_offset)?;
    let open = before.rfind("{{")?;
    if before.get(open..).is_some_and(|tail| tail.contains("}}")) {
        return None;
    }
    Some((open, cursor_offset))
}

fn find_template_path_completion_context(
    action: &str,
    cursor_in_action: usize,
) -> Option<TemplatePathCompletionContext> {
    if cursor_in_action > action.len() {
        return None;
    }
    let before = action.get(..cursor_in_action)?;
    let candidates = [
        ("$.Values", TemplatePathRoot::Values),
        ("$.CurrentApp", TemplatePathRoot::CurrentApp),
        ("$.Release", TemplatePathRoot::Release),
        ("$.Chart", TemplatePathRoot::Chart),
        ("$.Capabilities", TemplatePathRoot::Capabilities),
        ("$.werf", TemplatePathRoot::Werf),
        (".CurrentApp", TemplatePathRoot::CurrentApp),
    ];

    let mut best: Option<(usize, TemplatePathCompletionContext)> = None;
    for (marker, root) in candidates {
        let Some(pos) = before.rfind(marker) else {
            continue;
        };
        if !is_path_marker_boundary(action.as_bytes(), pos) {
            continue;
        }
        let tail = &before[pos + marker.len()..];
        if !tail.is_empty() && !tail.starts_with('.') {
            continue;
        }
        let raw_path = tail.strip_prefix('.').unwrap_or("");
        if raw_path.starts_with('.') || raw_path.contains("..") {
            continue;
        }
        if !raw_path
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
        {
            continue;
        }

        let (parent_segments, query, replace_start_byte) =
            if raw_path.is_empty() || raw_path.ends_with('.') {
                (
                    split_path_segments(raw_path.trim_end_matches('.')),
                    String::new(),
                    cursor_in_action,
                )
            } else if let Some(last_dot) = raw_path.rfind('.') {
                let parent = &raw_path[..last_dot];
                let query = &raw_path[last_dot + 1..];
                (
                    split_path_segments(parent),
                    query.to_string(),
                    pos + marker.len() + 1 + last_dot + 1,
                )
            } else {
                (Vec::new(), raw_path.to_string(), pos + marker.len() + 1)
            };

        let ctx = TemplatePathCompletionContext {
            root,
            parent_segments,
            query,
            replace_start_byte,
            replace_end_byte: cursor_in_action,
        };
        if best.as_ref().is_none_or(|(best_pos, _)| pos > *best_pos) {
            best = Some((pos, ctx));
        }
    }

    best.map(|(_, ctx)| ctx)
}

fn collect_template_path_refs_in_action(action: &str) -> Vec<TemplatePathRef> {
    let mut out = Vec::new();
    let markers = [
        ("$.Values.", TemplatePathRoot::Values),
        ("$.CurrentApp.", TemplatePathRoot::CurrentApp),
        ("$.Release.", TemplatePathRoot::Release),
        ("$.Chart.", TemplatePathRoot::Chart),
        ("$.Capabilities.", TemplatePathRoot::Capabilities),
        ("$.werf.", TemplatePathRoot::Werf),
        (".CurrentApp.", TemplatePathRoot::CurrentApp),
    ];
    let bytes = action.as_bytes();

    for (marker, root) in markers {
        let mut cursor = 0usize;
        while cursor < action.len() {
            let Some(slice) = action.get(cursor..) else {
                break;
            };
            let Some(rel) = slice.find(marker) else {
                break;
            };
            let marker_pos = cursor + rel;
            if !is_path_marker_boundary(bytes, marker_pos) {
                cursor = next_char_boundary(action, marker_pos + marker.len());
                continue;
            }

            let start = marker_pos;
            let mut end = marker_pos + marker.len();
            while end < action.len() {
                let b = action.as_bytes()[end];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
                    end += 1;
                    continue;
                }
                break;
            }
            if end <= marker_pos + marker.len() {
                cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
                continue;
            }
            let Some(raw) = action.get(marker_pos + marker.len()..end) else {
                cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
                continue;
            };
            if raw.starts_with('.') || raw.ends_with('.') || raw.contains("..") {
                cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
                continue;
            }
            let segments = split_path_segments(raw);
            if segments.is_empty() {
                cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
                continue;
            }
            out.push(TemplatePathRef {
                root,
                segments,
                full: action
                    .get(start..end)
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            });
            cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
        }
    }

    out
}

fn collect_local_values_refs_in_action(action: &str) -> Vec<String> {
    let mut out = Vec::new();
    let marker = ".Values";
    let bytes = action.as_bytes();
    let mut cursor = 0usize;

    while cursor < action.len() {
        let Some(slice) = action.get(cursor..) else {
            break;
        };
        let Some(rel) = slice.find(marker) else {
            break;
        };
        let marker_pos = cursor + rel;
        if !is_path_marker_boundary(bytes, marker_pos) {
            cursor = next_char_boundary(action, marker_pos + marker.len());
            continue;
        }
        let marker_end = marker_pos + marker.len();
        if marker_end < action.len() {
            let next = action.as_bytes()[marker_end];
            if next != b'.' {
                cursor = next_char_boundary(action, marker_end);
                continue;
            }
        }
        let mut end = marker_end;
        while end < action.len() {
            let b = action.as_bytes()[end];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
                end += 1;
                continue;
            }
            break;
        }
        if let Some(path) = action.get(marker_pos..end) {
            out.push(path.to_string());
        }
        cursor = next_char_boundary(action, end.max(marker_pos + marker.len()));
    }

    out
}

fn next_char_boundary(text: &str, mut idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn collect_include_calls_in_action(action: &str) -> Vec<IncludeCallRef> {
    let mut out = Vec::new();
    let bytes = action.as_bytes();
    let include = b"include";
    let mut i = 0usize;
    while i + include.len() <= bytes.len() {
        if !bytes_starts_with_at(bytes, i, include) {
            i += 1;
            continue;
        }
        if i > 0 {
            let prev = bytes[i - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' || prev == b'.' {
                i += 1;
                continue;
            }
        }
        let mut j = i + include.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            i += 1;
            continue;
        }
        let start_name = j + 1;
        let mut end_name = start_name;
        while end_name < bytes.len() && bytes[end_name] != quote {
            end_name += 1;
        }
        if end_name >= bytes.len() {
            break;
        }
        let Some(name) = action.get(start_name..end_name).map(ToString::to_string) else {
            i = end_name.saturating_add(1);
            continue;
        };
        j = end_name + 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let list_arg_count = if j < bytes.len() && bytes[j] == b'(' {
            parse_list_arg_count_from_parenthesized_expr(action, j).map(|(_, count)| count)
        } else {
            None
        };
        out.push(IncludeCallRef {
            name,
            list_arg_count,
        });
        i = end_name + 1;
    }
    out
}

fn parse_list_arg_count_from_parenthesized_expr(
    action: &str,
    open_paren: usize,
) -> Option<(usize, usize)> {
    if action.as_bytes().get(open_paren).copied() != Some(b'(') {
        return None;
    }
    let close = find_matching_paren(action, open_paren)?;
    let inner = action.get(open_paren + 1..close)?.trim();
    if !inner.starts_with("list") {
        return None;
    }
    let rest = inner.get("list".len()..)?.trim();
    if rest.is_empty() {
        return Some((close + 1, 0));
    }
    Some((close + 1, count_top_level_tokens(rest)))
}

fn find_matching_paren(src: &str, open_paren: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open_paren;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            if b == b'`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        match b {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'`' => in_backtick = true,
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn count_top_level_tokens(src: &str) -> usize {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut count = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'\\' {
                i = i.saturating_add(2);
                continue;
            }
            if b == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            if b == b'`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        if b.is_ascii_whitespace() && depth == 0 {
            i += 1;
            continue;
        }
        count += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if in_single {
                if b == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if b == b'\'' {
                    in_single = false;
                }
                i += 1;
                continue;
            }
            if in_double {
                if b == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if b == b'"' {
                    in_double = false;
                }
                i += 1;
                continue;
            }
            if in_backtick {
                if b == b'`' {
                    in_backtick = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b'`' => in_backtick = true,
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if b.is_ascii_whitespace() && depth == 0 {
                break;
            }
            i += 1;
        }
    }
    count
}

fn is_path_marker_boundary(bytes: &[u8], marker_pos: usize) -> bool {
    if marker_pos == 0 {
        return true;
    }
    let prev = bytes[marker_pos - 1];
    !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' || prev == b'$')
}

fn bytes_starts_with_at(bytes: &[u8], offset: usize, needle: &[u8]) -> bool {
    bytes
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|slice| slice == needle)
}

fn split_path_segments(path: &str) -> Vec<String> {
    path.split('.')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

fn value_has_path(value: &JsonValue, segments: &[String]) -> bool {
    let mut current = value;
    for segment in segments {
        let Some(map) = as_obj(current) else {
            return false;
        };
        let Some(next) = map.get(segment) else {
            return false;
        };
        current = next;
    }
    true
}

fn object_at_path<'a>(
    value: &'a JsonValue,
    segments: &[String],
) -> Option<&'a JsonMap<String, JsonValue>> {
    let mut current = value;
    for segment in segments {
        let map = as_obj(current)?;
        current = map.get(segment)?;
    }
    as_obj(current)
}

fn sorted_keys(map: &JsonMap<String, JsonValue>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

#[derive(Debug, Clone)]
struct TextLineIndex {
    starts: Vec<usize>,
}

impl TextLineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (idx, b) in text.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                starts.push(idx + 1);
            }
        }
        Self { starts }
    }

    fn line_start(&self, line: usize) -> usize {
        *self
            .starts
            .get(line)
            .unwrap_or_else(|| self.starts.last().unwrap_or(&0))
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1)
    }

    fn utf16_col_for_offset(&self, line_text: &str, line: usize, offset: usize) -> u32 {
        let line_start = self.line_start(line);
        let in_line = offset.saturating_sub(line_start).min(line_text.len());
        utf16_len(char_boundary_prefix(line_text, in_line)) as u32
    }

    fn offset_for_line_utf16(&self, line_text: &str, line: usize, utf16_col: usize) -> usize {
        self.line_start(line) + utf16_col_to_byte(line_text, utf16_col)
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn char_boundary_prefix(text: &str, mut end: usize) -> &str {
    if end > text.len() {
        end = text.len();
    }
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or("")
}

fn utf16_col_to_byte(s: &str, utf16_col: usize) -> usize {
    let mut col = 0usize;
    for (idx, ch) in s.char_indices() {
        if col >= utf16_col {
            return idx;
        }
        col += ch.len_utf16();
        if col > utf16_col {
            return idx + ch.len_utf8();
        }
    }
    s.len()
}

fn collect_stitched_include_name_context(uri: &Uri, text: &str) -> StitchedIncludeNameContext {
    let context = build_stitched_values_context(uri, text);
    let mut unique_defs: HashSet<(String, PathBuf, usize)> = HashSet::new();
    let mut unique_usages: HashSet<(String, PathBuf, usize)> = HashSet::new();
    let mut defined_names = HashSet::new();
    let mut used_names = HashSet::new();

    for definition in context.include_definitions {
        if !unique_defs.insert((
            definition.name.clone(),
            definition.source_file.clone(),
            definition.source_line,
        )) {
            continue;
        }
        defined_names.insert(definition.name);
    }
    for usage in context.include_usages {
        if !unique_usages.insert((
            usage.name.clone(),
            usage.source_file.clone(),
            usage.source_line,
        )) {
            continue;
        }
        used_names.insert(usage.name);
    }

    StitchedIncludeNameContext {
        defined_names,
        used_names,
    }
}

#[derive(Debug, Default)]
struct StitchedIncludeNameContext {
    defined_names: HashSet<String>,
    used_names: HashSet<String>,
}

#[derive(Debug, Default)]
struct StitchedValuesContext {
    include_definitions: Vec<StitchedIncludeDefinition>,
    include_usages: Vec<StitchedIncludeUsage>,
}

#[derive(Debug, Clone)]
struct StitchedIncludeDefinition {
    name: String,
    source_file: PathBuf,
    source_line: usize,
}

#[derive(Debug, Clone)]
struct StitchedIncludeUsage {
    name: String,
    source_file: PathBuf,
    source_line: usize,
}

fn build_stitched_values_context(uri: &Uri, text: &str) -> StitchedValuesContext {
    let mut context = StitchedValuesContext::default();
    let mut file_stack = HashSet::new();
    let mut overrides: HashMap<PathBuf, String> = HashMap::new();

    if let Some(current_path) = file_path_from_uri_string(&uri.to_string()) {
        let normalized_current = normalize_fs_path(&current_path);
        overrides.insert(normalized_current, text.to_string());

        if let Some(chart_root) = find_chart_root_from_path(&current_path) {
            if let Some(root_values_path) = find_primary_values_file(&chart_root) {
                if let Ok(root_text) =
                    read_text_from_path_with_overrides(&root_values_path, &overrides)
                {
                    collect_stitched_data_from_text(
                        &root_text,
                        &root_values_path,
                        &[],
                        &mut context,
                        &mut file_stack,
                        &overrides,
                    );
                    return context;
                }
            }
        }

        collect_stitched_data_from_text(
            text,
            &current_path,
            &[],
            &mut context,
            &mut file_stack,
            &overrides,
        );
        return context;
    }

    let fallback_path = PathBuf::from("values.yaml");
    collect_stitched_data_from_text(
        text,
        &fallback_path,
        &[],
        &mut context,
        &mut file_stack,
        &overrides,
    );
    context
}

fn collect_stitched_data_from_text(
    text: &str,
    source_path: &Path,
    parent_path: &[String],
    context: &mut StitchedValuesContext,
    file_stack: &mut HashSet<PathBuf>,
    overrides: &HashMap<PathBuf, String>,
) {
    let lines: Vec<&str> = text.split('\n').collect();
    for usage in collect_include_usages(&lines) {
        context.include_usages.push(StitchedIncludeUsage {
            name: usage.name,
            source_file: source_path.to_path_buf(),
            source_line: usage.line,
        });
    }

    let local_defs = if parent_path.is_empty() {
        collect_local_include_definitions(&lines)
    } else {
        collect_include_defs_for_from_file_parent(&lines, parent_path)
    };
    for def in local_defs {
        context.include_definitions.push(StitchedIncludeDefinition {
            name: def.name,
            source_file: source_path.to_path_buf(),
            source_line: def.line,
        });
    }

    let refs = collect_include_file_refs(&lines);
    let base_dir = source_path.parent();
    for mut file_ref in refs {
        if !parent_path.is_empty() {
            let mut effective_parent_path = parent_path.to_vec();
            effective_parent_path.extend(file_ref.parent_path);
            file_ref.parent_path = effective_parent_path;
        }
        collect_stitched_data_from_ref(&file_ref, base_dir, context, file_stack, overrides);
    }
}

fn collect_include_defs_for_from_file_parent(
    lines: &[&str],
    parent_path: &[String],
) -> Vec<IncludeDefinitionRef> {
    if !is_direct_global_includes_path(parent_path) {
        return Vec::new();
    }

    let (global_defs, has_global_includes_scope) = collect_global_include_definitions(lines);
    if has_global_includes_scope {
        return global_defs;
    }
    collect_top_level_map_entry_definitions(lines)
}

fn collect_stitched_data_from_ref(
    file_ref: &IncludeFileRef,
    base_dir: Option<&Path>,
    context: &mut StitchedValuesContext,
    file_stack: &mut HashSet<PathBuf>,
    overrides: &HashMap<PathBuf, String>,
) {
    if is_templated_include_path(&file_ref.path) {
        return;
    }
    let loaded = match load_include_text_from_file(&file_ref.path, base_dir, file_stack, overrides)
    {
        Ok(value) => value,
        Err(_) => None,
    };
    let Some((loaded_path, loaded_text)) = loaded else {
        return;
    };

    match file_ref.kind {
        IncludeFileRefKind::FilesList => {
            let include_name = include_name_from_path(&file_ref.path);
            context.include_definitions.push(StitchedIncludeDefinition {
                name: include_name.clone(),
                source_file: loaded_path.clone(),
                source_line: file_ref.line,
            });

            collect_stitched_data_from_text(
                &loaded_text,
                &loaded_path,
                &["global".to_string(), "_includes".to_string(), include_name],
                context,
                file_stack,
                overrides,
            );
        }
        IncludeFileRefKind::FromFile => {
            collect_stitched_data_from_text(
                &loaded_text,
                &loaded_path,
                &file_ref.parent_path,
                context,
                file_stack,
                overrides,
            );
        }
    }
}

fn is_direct_global_includes_path(path: &[String]) -> bool {
    path.len() == 2 && path[0] == "global" && path[1] == "_includes"
}

fn load_include_text_from_file(
    raw_path: &str,
    base_dir: Option<&Path>,
    file_stack: &mut HashSet<PathBuf>,
    overrides: &HashMap<PathBuf, String>,
) -> Result<Option<(PathBuf, String)>, String> {
    if is_templated_include_path(raw_path) {
        return Ok(None);
    }
    let candidates = build_include_candidates(raw_path, base_dir);
    for candidate in candidates {
        match load_include_text_from_absolute_file(&candidate, file_stack, overrides) {
            Ok(text) => return Ok(Some((candidate, text))),
            Err(FileIncludeLoadError::NotFound) => continue,
            Err(FileIncludeLoadError::Other(message)) => return Err(message),
        }
    }
    Ok(None)
}

enum FileIncludeLoadError {
    NotFound,
    Other(String),
}

fn load_include_text_from_absolute_file(
    file_path: &Path,
    file_stack: &mut HashSet<PathBuf>,
    overrides: &HashMap<PathBuf, String>,
) -> Result<String, FileIncludeLoadError> {
    let tracked = normalize_fs_path(file_path);
    if file_stack.contains(&tracked) {
        return Err(FileIncludeLoadError::Other(format!(
            "_include file cycle detected: {}",
            tracked.display()
        )));
    }
    file_stack.insert(tracked.clone());
    let load_result = (|| {
        if let Some(override_text) = overrides.get(&tracked) {
            return Ok(override_text.clone());
        }
        std::fs::read_to_string(file_path).map_err(|err| match err.kind() {
            ErrorKind::NotFound | ErrorKind::NotADirectory => FileIncludeLoadError::NotFound,
            _ => FileIncludeLoadError::Other(format!(
                "read include file '{}': {}",
                file_path.display(),
                err
            )),
        })
    })();
    file_stack.remove(&tracked);
    load_result
}

fn is_include_entry_helper_key(name: &str) -> bool {
    matches!(name, "_include" | "_include_from_file" | "_include_files")
}

fn looks_like_helm_apps_values_text(text: &str) -> bool {
    if text.contains("\nglobal:") && text.contains("\n  _includes:") {
        return true;
    }
    if text.contains("\nglobal:") && text.contains("\n  releases:") {
        return true;
    }
    text.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("apps-") && t.ends_with(':')
    })
}

fn make_diagnostic(
    line: usize,
    lines: &[&str],
    token: &str,
    severity: DiagnosticSeverity,
    message: String,
    code: Option<String>,
) -> Diagnostic {
    let line_text = lines.get(line).copied().unwrap_or_default();
    let start = line_text.find(token).unwrap_or(0);
    let end = if start == 0 && token.is_empty() {
        line_text.len()
    } else if start == 0 && !line_text.starts_with(token) {
        line_text.len()
    } else {
        start + token.len()
    };
    Diagnostic {
        range: Range::new(
            Position::new(line as u32, start as u32),
            Position::new(line as u32, end.max(start) as u32),
        ),
        severity: Some(severity),
        code: code.map(lsp_types::NumberOrString::String),
        code_description: None,
        source: Some("happ".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn make_diagnostic_at_offset(
    line_index: &TextLineIndex,
    lines: &[&str],
    offset: usize,
    len: usize,
    severity: DiagnosticSeverity,
    message: String,
    code: Option<String>,
) -> Diagnostic {
    let line = line_index.line_for_offset(offset);
    let line_text = lines.get(line).copied().unwrap_or_default();
    let line_start = line_index.line_start(line);
    let start_in_line = offset.saturating_sub(line_start).min(line_text.len());
    let end_in_line = start_in_line.saturating_add(len).min(line_text.len());
    Diagnostic {
        range: Range::new(
            Position::new(line as u32, start_in_line as u32),
            Position::new(line as u32, end_in_line.max(start_in_line) as u32),
        ),
        severity: Some(severity),
        code: code.map(lsp_types::NumberOrString::String),
        code_description: None,
        source: Some("happ".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

#[derive(Debug, Clone)]
struct IncludeDefinitionRef {
    name: String,
    line: usize,
    emit_unused_diagnostic: bool,
}

#[derive(Debug, Clone)]
struct IncludeUsageRef {
    name: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct IncludeFileRef {
    path: String,
    line: usize,
    kind: IncludeFileRefKind,
    parent_path: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncludeFileRefKind {
    FromFile,
    FilesList,
}

fn collect_local_include_definitions(lines: &[&str]) -> Vec<IncludeDefinitionRef> {
    collect_include_definitions(lines, true)
}

fn collect_include_definitions(
    lines: &[&str],
    allow_top_level_profile_fallback: bool,
) -> Vec<IncludeDefinitionRef> {
    let (mut out, has_global_includes_scope) = collect_global_include_definitions(lines);
    if !allow_top_level_profile_fallback || has_global_includes_scope {
        return out;
    }
    for def in collect_top_level_include_definitions(lines) {
        if out.iter().any(|current| current.name == def.name) {
            continue;
        }
        out.push(def);
    }
    out
}

fn collect_global_include_definitions(lines: &[&str]) -> (Vec<IncludeDefinitionRef>, bool) {
    let mut out = Vec::new();
    let mut in_global = false;
    let mut in_includes = false;
    let mut has_global_includes_scope = false;
    let blocked = block_scalar_content_lines(lines);

    for (i, line) in lines.iter().enumerate() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((indent, key, _value)) = parse_key_line(line) else {
            continue;
        };
        if indent == 0 {
            in_global = key == "global";
            in_includes = false;
            continue;
        }
        if in_global && indent == 2 {
            in_includes = key == "_includes";
            if in_includes {
                has_global_includes_scope = true;
            }
            continue;
        }
        if in_global && in_includes && indent == 4 {
            if is_include_entry_helper_key(key) {
                continue;
            }
            out.push(IncludeDefinitionRef {
                name: key.to_string(),
                line: i,
                emit_unused_diagnostic: true,
            });
        }
    }

    (out, has_global_includes_scope)
}

fn collect_top_level_include_definitions(lines: &[&str]) -> Vec<IncludeDefinitionRef> {
    let mut out = Vec::new();
    let mut top_level_keys: Vec<(String, usize)> = Vec::new();
    let mut has_profile_like_top_level = false;
    let mut current_top_level: Option<String> = None;
    let blocked = block_scalar_content_lines(lines);

    for (i, line) in lines.iter().enumerate() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((indent, key, _value)) = parse_key_line(line) else {
            continue;
        };
        if indent == 0 {
            current_top_level = Some(key.to_string());
            top_level_keys.push((key.to_string(), i));
            continue;
        }
        if indent == 2 && current_top_level.is_some() && is_include_entry_helper_key(key) {
            has_profile_like_top_level = true;
        }
    }
    if has_profile_like_top_level {
        for (name, line) in top_level_keys {
            if is_include_entry_helper_key(&name) || is_reserved_top_level_key(&name) {
                continue;
            }
            out.push(IncludeDefinitionRef {
                name,
                line,
                emit_unused_diagnostic: false,
            });
        }
    }
    out
}

fn collect_top_level_map_entry_definitions(lines: &[&str]) -> Vec<IncludeDefinitionRef> {
    let mut out = Vec::new();
    let blocked = block_scalar_content_lines(lines);
    for (i, line) in lines.iter().enumerate() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((indent, key, _value)) = parse_key_line(line) else {
            continue;
        };
        if indent != 0 {
            continue;
        }
        if is_include_entry_helper_key(key) {
            continue;
        }
        out.push(IncludeDefinitionRef {
            name: key.to_string(),
            line: i,
            emit_unused_diagnostic: false,
        });
    }
    out
}

fn is_reserved_top_level_key(key: &str) -> bool {
    matches!(key, "global" | "enabled" | "werf" | "helm-apps")
}

fn collect_include_usages(lines: &[&str]) -> Vec<IncludeUsageRef> {
    let mut out = Vec::new();
    let blocked = block_scalar_content_lines(lines);

    for (i, line) in lines.iter().enumerate() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        if let Some((_indent, key, value)) = parse_key_line(line) {
            if key == "_include" {
                let inline = value.trim();
                if inline.starts_with('[') && inline.ends_with(']') {
                    let inside = &inline[1..inline.len() - 1];
                    for part in inside.split(',') {
                        let v = unquote(part.trim());
                        if is_include_token(&v) {
                            out.push(IncludeUsageRef { name: v, line: i });
                        }
                    }
                } else {
                    let v = unquote(inline);
                    if is_include_token(&v) {
                        out.push(IncludeUsageRef { name: v, line: i });
                    }
                }
                continue;
            }
        }

        let Some((item_indent, item_value)) = parse_list_item_token(line) else {
            continue;
        };
        let parent = find_parent_key_with_mask(lines, &blocked, i, item_indent);
        if parent.as_deref() == Some("_include") {
            out.push(IncludeUsageRef {
                name: item_value,
                line: i,
            });
        }
    }

    out
}

fn collect_include_file_refs(lines: &[&str]) -> Vec<IncludeFileRef> {
    let mut refs = Vec::new();
    let mut key_stack: Vec<(usize, String)> = Vec::new();
    let blocked = block_scalar_content_lines(lines);

    for (i, line) in lines.iter().enumerate() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((indent, key, value)) = parse_key_line(line) else {
            continue;
        };
        while key_stack
            .last()
            .is_some_and(|(stack_indent, _)| *stack_indent >= indent)
        {
            key_stack.pop();
        }
        key_stack.push((indent, key.to_string()));
        let parent_path: Vec<String> = key_stack[..key_stack.len() - 1]
            .iter()
            .map(|(_, current_key)| current_key.clone())
            .collect();

        if key == "_include_from_file" {
            let path = unquote(value.trim());
            if !path.is_empty() {
                refs.push(IncludeFileRef {
                    path,
                    line: i,
                    kind: IncludeFileRefKind::FromFile,
                    parent_path: parent_path.clone(),
                });
            }
            continue;
        }
        if key != "_include_files" {
            continue;
        }

        let tail = value.trim();
        if tail.starts_with('[') && tail.ends_with(']') {
            let inside = &tail[1..tail.len() - 1];
            for part in inside.split(',') {
                let path = unquote(part.trim());
                if !path.is_empty() {
                    refs.push(IncludeFileRef {
                        path,
                        line: i,
                        kind: IncludeFileRefKind::FilesList,
                        parent_path: parent_path.clone(),
                    });
                }
            }
            continue;
        }

        for (j, sub_line) in lines.iter().enumerate().skip(i + 1) {
            if blocked.get(j).copied().unwrap_or(false) {
                continue;
            }
            let t = sub_line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let sub_indent = count_indent(sub_line);
            if sub_indent <= indent {
                break;
            }
            if let Some((_li, raw)) = parse_list_item_raw(sub_line) {
                let path = unquote(raw.trim());
                if !path.is_empty() {
                    refs.push(IncludeFileRef {
                        path,
                        line: j,
                        kind: IncludeFileRefKind::FilesList,
                        parent_path: parent_path.clone(),
                    });
                }
            }
        }
    }

    refs
}

fn parse_key_line(line: &str) -> Option<(usize, &str, &str)> {
    let indent = count_indent(line);
    let rest = line.get(indent..)?;
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    let pos = rest.find(':')?;
    let key = rest.get(..pos)?.trim();
    if key.is_empty() {
        return None;
    }
    if !key.chars().all(is_key_char) {
        return None;
    }
    let value = rest.get(pos + 1..)?.trim_start();
    Some((indent, key, value))
}

fn parse_list_item_token(line: &str) -> Option<(usize, String)> {
    let (indent, raw) = parse_list_item_raw(line)?;
    let token = unquote(raw.trim());
    if !is_include_token(&token) {
        return None;
    }
    Some((indent, token))
}

fn parse_list_item_raw(line: &str) -> Option<(usize, &str)> {
    let indent = count_indent(line);
    let rest = line.get(indent..)?.trim_start();
    if !rest.starts_with("- ") {
        return None;
    }
    let value = rest.get(2..)?.trim();
    if value.is_empty() {
        return None;
    }
    Some((indent, value))
}

#[cfg(test)]
fn find_parent_key(lines: &[&str], line: usize, indent: usize) -> Option<String> {
    let blocked = block_scalar_content_lines(lines);
    find_parent_key_with_mask(lines, &blocked, line, indent)
}

fn find_parent_key_with_mask(
    lines: &[&str],
    blocked: &[bool],
    line: usize,
    indent: usize,
) -> Option<String> {
    for i in (0..line).rev() {
        if blocked.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((key_indent, key, _value)) = parse_key_line(lines[i]) else {
            continue;
        };
        if key_indent < indent {
            return Some(key.to_string());
        }
    }
    None
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

fn is_include_token(value: &str) -> bool {
    !value.is_empty() && value.chars().all(is_key_char)
}

fn count_indent(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn block_scalar_content_lines(lines: &[&str]) -> Vec<bool> {
    let mut blocked = vec![false; lines.len()];
    let mut i = 0usize;
    while i < lines.len() {
        let Some((header_indent, header_line)) = block_scalar_header(lines, i) else {
            i += 1;
            continue;
        };
        if header_line > i {
            blocked[header_line] = true;
        }
        let mut j = header_line + 1;
        while j < lines.len() {
            let current = lines[j];
            let trimmed = current.trim();
            if trimmed.is_empty() {
                blocked[j] = true;
                j += 1;
                continue;
            }
            let current_indent = count_indent(current);
            if current_indent <= header_indent {
                break;
            }
            blocked[j] = true;
            j += 1;
        }
        i = j;
    }
    blocked
}

fn block_scalar_header(lines: &[&str], index: usize) -> Option<(usize, usize)> {
    let line = *lines.get(index)?;
    if let Some(indent) = line_starts_inline_block_scalar(line) {
        return Some((indent, index));
    }
    let (indent, _key, value) = parse_key_line(line)?;
    if !value.trim().is_empty() {
        return None;
    }
    let next_line = *lines.get(index + 1)?;
    if count_indent(next_line) <= indent || !line_is_standalone_block_scalar_header(next_line) {
        return None;
    }
    Some((indent, index + 1))
}

fn line_starts_inline_block_scalar(line: &str) -> Option<usize> {
    let indent = count_indent(line);
    let rest = line.get(indent..)?.trim();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    let colon = rest.find(':')?;
    let after_colon = rest.get(colon + 1..)?.trim();
    if after_colon.is_empty() {
        return None;
    }
    let mut tokens = after_colon.split_whitespace();
    let first = tokens.next()?;
    let marker = if first.starts_with('&') || first.starts_with('!') {
        tokens.next().unwrap_or("")
    } else {
        first
    };
    if marker.starts_with('|') || marker.starts_with('>') {
        return Some(indent);
    }
    None
}

fn line_is_standalone_block_scalar_header(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    let mut tokens = trimmed.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    let marker = if first.starts_with('&') || first.starts_with('!') {
        tokens.next().unwrap_or("")
    } else {
        first
    };
    marker.starts_with('|') || marker.starts_with('>')
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

fn include_name_from_path(path_value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_templated_include_path(path_value: &str) -> bool {
    path_value.contains("{{") || path_value.contains("}}")
}

fn include_base_dirs_for_diagnostics(uri: &Uri) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(current_path) = file_path_from_uri_string(&uri.to_string()) else {
        return out;
    };

    if let Some(chart_root) = find_chart_root_from_path(&current_path) {
        if let Some(root_values_path) = find_primary_values_file(&chart_root) {
            if let Some(root_base_dir) = root_values_path.parent() {
                push_unique_path(&mut out, root_base_dir);
            }
        }
    }
    if let Some(current_base_dir) = current_path.parent() {
        push_unique_path(&mut out, current_base_dir);
    }

    out
}

fn build_include_candidates_for_diagnostics(raw_path: &str, base_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let trimmed = raw_path.trim();
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return vec![p.to_path_buf()];
    }
    if base_dirs.is_empty() {
        return vec![p.to_path_buf()];
    }
    let mut out = Vec::new();
    for base_dir in base_dirs {
        push_unique_path(&mut out, &base_dir.join(p));
    }
    out
}

fn push_unique_path(out: &mut Vec<PathBuf>, candidate: &Path) {
    let normalized = normalize_fs_path(candidate);
    if out.iter().any(|existing| normalize_fs_path(existing) == normalized) {
        return;
    }
    out.push(normalized);
}

fn file_path_from_uri_string(uri: &str) -> Option<PathBuf> {
    if uri == "file://" {
        return None;
    }
    let parsed = url::Url::parse(uri).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    parsed.to_file_path().ok()
}

fn build_include_candidates(raw_path: &str, base_dir: Option<&Path>) -> Vec<PathBuf> {
    let p = Path::new(raw_path);
    if p.is_absolute() {
        return vec![p.to_path_buf()];
    }
    if let Some(base) = base_dir {
        return vec![base.join(p)];
    }
    vec![p.to_path_buf()]
}

fn send_ok(connection: &Connection, id: RequestId, result: JsonValue) -> Result<(), Error> {
    let response = Response {
        id,
        result: Some(result),
        error: None,
    };
    connection
        .sender
        .send(Message::Response(response))
        .map_err(|e| Error::Transport(format!("send response: {e}")))?;
    Ok(())
}

fn send_error(
    connection: &Connection,
    id: RequestId,
    code: i32,
    message: String,
) -> Result<(), Error> {
    let response = Response {
        id,
        result: None,
        error: Some(ResponseError {
            code,
            message,
            data: None,
        }),
    };
    connection
        .sender
        .send(Message::Response(response))
        .map_err(|e| Error::Transport(format!("send response: {e}")))?;
    Ok(())
}

fn expand_includes_in_values(
    root: &JsonMap<String, JsonValue>,
) -> Result<JsonMap<String, JsonValue>, String> {
    let includes_map = root
        .get("global")
        .and_then(as_obj)
        .and_then(|g| g.get("_includes"))
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
                let mut merged: JsonMap<String, JsonValue> = JsonMap::new();
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
            for (k, v) in current {
                if k == "_includes" {
                    out.insert(k, v);
                } else {
                    out.insert(k, expand_node(&v, includes_map, cache)?);
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
    if stack.iter().any(|s| s == name) {
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
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_string()]
            }
        }
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|v| {
                let s = v.as_str()?;
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_include_files(value: Option<&JsonValue>) -> Vec<String> {
    match value {
        Some(JsonValue::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_string()]
            }
        }
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|v| {
                let s = v.as_str()?;
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            })
            .collect(),
        _ => Vec::new(),
    }
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
            let merged_json = JsonValue::Array(merged.into_iter().map(JsonValue::String).collect());
            out.insert(key.clone(), merged_json);
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

fn discover_environments(values: &JsonValue) -> EnvironmentDiscovery {
    let mut literals: HashSet<String> = HashSet::new();
    let mut regexes: HashSet<String> = HashSet::new();

    if let Some(global_env) = values
        .as_object()
        .and_then(|root| root.get("global"))
        .and_then(as_obj)
        .and_then(|g| g.get("env"))
        .and_then(|v| v.as_str())
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
        .and_then(|g| g.get("env"))
        .and_then(|v| v.as_str())
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
        JsonValue::Array(items) => {
            JsonValue::Array(items.iter().map(|v| resolve_env_maps(v, env)).collect())
        }
        JsonValue::Object(map) => {
            if looks_like_env_map(map) {
                let selected = select_env_value(map, env);
                if selected == JsonValue::Object(map.clone()) {
                    let mut out = JsonMap::new();
                    for (k, v) in map {
                        out.insert(k.clone(), resolve_env_maps(v, env));
                    }
                    return JsonValue::Object(out);
                }
                return resolve_env_maps(&selected, env);
            }
            let mut out = JsonMap::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_env_maps(v, env));
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
    map.keys().any(|k| looks_like_regex_pattern(k))
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
    if let Some(v) = map.get(env) {
        return v.clone();
    }
    for (k, v) in map {
        if k == "_default" || !looks_like_regex_pattern(k) {
            continue;
        }
        if let Ok(re) = regex::Regex::new(k) {
            if re.is_match(env) {
                return v.clone();
            }
        }
    }
    if let Some(v) = map.get("_default") {
        return v.clone();
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
            for v in map.values() {
                walk_maps(v, on_map);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn include_analysis_detects_unresolved_and_unused() {
        let src = r#"
global:
  _includes:
    base:
      enabled: true
apps-stateless:
  api:
    _include:
      - missing
"#;
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("Unresolved include profile: missing")));
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("Unused include profile: base")));
    }

    #[test]
    fn resolve_entity_expands_includes_and_env() {
        let src = r#"
global:
  env: prod
  _includes:
    base:
      resources:
        _default:
          cpu: "100m"
        prod:
          cpu: "200m"
apps-stateless:
  api:
    _include:
      - base
"#;
        let mut state = ServerState::default();
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        state.documents.insert(
            uri.to_string(),
            DocumentState {
                text: src.to_string(),
            },
        );
        let out = resolve_entity_request(
            &state,
            ResolveEntityParams {
                uri: Some(uri.to_string()),
                text: None,
                group: "apps-stateless".to_string(),
                app: "api".to_string(),
                env: None,
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
            },
        )
        .expect("resolve");
        let resources = out
            .entity
            .get("resources")
            .and_then(as_obj)
            .expect("resources");
        assert_eq!(resources.get("cpu").and_then(|v| v.as_str()), Some("200m"));
    }

    #[test]
    fn optimize_values_includes_request_extracts_common_payload() {
        let src = r#"
global:
  _includes: {}
apps-stateless:
  api:
    enabled: true
    image:
      name: nginx
      staticTag: latest
  web:
    enabled: true
    image:
      name: nginx
      staticTag: latest
"#;
        let out = optimize_values_includes_request(
            &ServerState::default(),
            OptimizeValuesIncludesParams {
                uri: None,
                text: Some(src.to_string()),
                min_profile_bytes: Some(1),
            },
        )
        .expect("optimize values");

        assert!(out.changed);
        assert!(out.profiles_added >= 1);

        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&out.optimized_text).expect("parse optimized yaml");
        let root = parsed.as_mapping().expect("root mapping");
        let global = root
            .get(serde_yaml::Value::String("global".into()))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("global mapping");
        let includes = global
            .get(serde_yaml::Value::String("_includes".into()))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("_includes mapping");
        assert!(!includes.is_empty());

        let apps_stateless = root
            .get(serde_yaml::Value::String("apps-stateless".into()))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("apps-stateless mapping");
        let api = apps_stateless
            .get(serde_yaml::Value::String("api".into()))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("api mapping");
        let include_refs = api
            .get(serde_yaml::Value::String("_include".into()))
            .and_then(serde_yaml::Value::as_sequence)
            .expect("_include refs");
        assert!(!include_refs.is_empty());
    }

    #[test]
    fn optimize_values_includes_request_rejects_non_mapping_document() {
        let err = optimize_values_includes_request(
            &ServerState::default(),
            OptimizeValuesIncludesParams {
                uri: None,
                text: Some("- one\n- two\n".to_string()),
                min_profile_bytes: Some(24),
            },
        )
        .expect_err("non-map must fail");
        assert!(err.contains("values document must be a YAML map"));
    }

    #[test]
    fn manifest_preview_values_do_not_encode_json_number_internal_representation() {
        let entity = json!({
            "containers": {
                "app-1": {
                    "ports": [
                        { "name": "http", "containerPort": 8080 }
                    ]
                }
            }
        });
        let global = json!({
            "validation": {
                "allowNativeListsInBuiltInListFields": true
            }
        });
        let values = build_manifest_preview_values(
            "apps-stateless",
            "app-1",
            &entity,
            &global,
            true,
            "prod",
        );
        let yaml_value = json_to_yaml_value(&values).expect("json->yaml");
        let yaml_text = serde_yaml::to_string(&yaml_value).expect("yaml");
        assert!(
            !yaml_text.contains("$serde_json::private::Number"),
            "must not leak serde_json::Number internals into YAML: {yaml_text}"
        );
    }

    #[test]
    fn manifest_preview_values_normalize_include_fields_and_skip_global_include_files() {
        let entity = json!({
            "containers": {
                "main": {
                    "_include_files": "configs/resource-assignment-kafka-consumer/ra_config.yaml"
                }
            }
        });
        let global = json!({
            "_include_files": "must-not-be-forwarded",
            "_includes": {},
            "validation": {
                "allowNativeListsInBuiltInListFields": true
            }
        });
        let values = build_manifest_preview_values(
            "apps-stateless",
            "app-1",
            &entity,
            &global,
            true,
            "prod",
        );
        let include_files = values
            .get("apps-stateless")
            .and_then(as_obj)
            .and_then(|group| group.get("app-1"))
            .and_then(as_obj)
            .and_then(|app| app.get("containers"))
            .and_then(as_obj)
            .and_then(|containers| containers.get("main"))
            .and_then(as_obj)
            .and_then(|main| main.get("_include_files"))
            .and_then(JsonValue::as_array)
            .expect("_include_files array");
        assert_eq!(include_files.len(), 1);
        assert_eq!(
            include_files.first().and_then(JsonValue::as_str),
            Some("configs/resource-assignment-kafka-consumer/ra_config.yaml")
        );

        let has_global_include_files = values
            .get("global")
            .and_then(as_obj)
            .is_some_and(|global_map| global_map.contains_key("_include_files"));
        assert!(!has_global_include_files);
    }

    #[test]
    fn manifest_preview_values_normalize_include_strings_including_empty_values() {
        let entity = json!({
            "containers": {
                "main": {
                    "_include": "   ",
                    "_include_files": " configs/a.yaml "
                }
            }
        });
        let global = json!({
            "validation": {
                "allowNativeListsInBuiltInListFields": true
            }
        });
        let values = build_manifest_preview_values(
            "apps-stateless",
            "app-1",
            &entity,
            &global,
            true,
            "prod",
        );
        let main = values
            .get("apps-stateless")
            .and_then(as_obj)
            .and_then(|group| group.get("app-1"))
            .and_then(as_obj)
            .and_then(|app| app.get("containers"))
            .and_then(as_obj)
            .and_then(|containers| containers.get("main"))
            .and_then(as_obj)
            .expect("main app");
        let include_refs = main
            .get("_include")
            .and_then(JsonValue::as_array)
            .expect("_include array");
        assert!(include_refs.is_empty());
        let include_files = main
            .get("_include_files")
            .and_then(JsonValue::as_array)
            .expect("_include_files array");
        assert_eq!(include_files.len(), 1);
        assert_eq!(
            include_files.first().and_then(JsonValue::as_str),
            Some("configs/a.yaml")
        );
    }

    #[test]
    fn optimize_values_includes_request_normalizes_scalar_include_fields_to_arrays() {
        let src = r#"
global:
  _includes: {}
apps-stateless:
  api:
    _include: base
    _include_files: "   "
"#;
        let out = optimize_values_includes_request(
            &ServerState::default(),
            OptimizeValuesIncludesParams {
                uri: None,
                text: Some(src.to_string()),
                min_profile_bytes: Some(65_536),
            },
        )
        .expect("optimize values");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&out.optimized_text).expect("parse optimized yaml");
        let root = parsed.as_mapping().expect("root mapping");
        let api = root
            .get(serde_yaml::Value::String("apps-stateless".into()))
            .and_then(serde_yaml::Value::as_mapping)
            .and_then(|apps| apps.get(serde_yaml::Value::String("api".into())))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("api mapping");
        let include_refs = api
            .get(serde_yaml::Value::String("_include".into()))
            .and_then(serde_yaml::Value::as_sequence)
            .expect("_include array");
        assert_eq!(include_refs.len(), 1);
        assert_eq!(
            include_refs.first().and_then(serde_yaml::Value::as_str),
            Some("base")
        );
        let include_files = api
            .get(serde_yaml::Value::String("_include_files".into()))
            .and_then(serde_yaml::Value::as_sequence)
            .expect("_include_files array");
        assert!(include_files.is_empty());
    }

    #[test]
    fn manifest_preview_values_force_requested_env() {
        let entity = json!({});
        let global = json!({
            "env": "dev",
            "validation": {
                "allowNativeListsInBuiltInListFields": true
            }
        });
        let values = build_manifest_preview_values(
            "apps-stateless",
            "app-1",
            &entity,
            &global,
            true,
            "prod",
        );
        let env_value = values
            .get("global")
            .and_then(as_obj)
            .and_then(|g| g.get("env"))
            .and_then(JsonValue::as_str);
        assert_eq!(env_value, Some("prod"));
    }

    #[test]
    fn manifest_preview_values_keep_referenced_global_keys() {
        let entity = json!({
            "data": {
                "token": "{{ .Values.global.authToken }}",
                "region": "{{ $.Values.global.region }}"
            }
        });
        let global = json!({
            "authToken": "secret",
            "region": "eu-west-1",
            "unused": "x",
            "validation": {
                "allowNativeListsInBuiltInListFields": true
            }
        });
        let values = build_manifest_preview_values(
            "apps-stateless",
            "app-1",
            &entity,
            &global,
            true,
            "prod",
        );
        let global_map = values.get("global").and_then(as_obj).expect("global map");
        assert_eq!(
            global_map.get("authToken").and_then(JsonValue::as_str),
            Some("secret")
        );
        assert_eq!(
            global_map.get("region").and_then(JsonValue::as_str),
            Some("eu-west-1")
        );
        assert!(!global_map.contains_key("unused"));
    }

    #[test]
    fn manifest_preview_values_keep_referenced_top_level_keys() {
        let entity = json!({
            "enabled": "{{ $.Values.deploy.enabled }}",
            "werfEnv": "{{ $.Values.werf.env }}"
        });
        let root = json!({
            "global": {
                "validation": {
                    "allowNativeListsInBuiltInListFields": true
                }
            },
            "deploy": {
                "enabled": true
            },
            "werf": {
                "env": "prod"
            },
            "unusedTopLevel": {
                "x": 1
            }
        });
        let global = root.get("global").cloned().expect("global");
        let values = build_manifest_preview_values_with_root(
            "apps-certificates",
            "apps-common",
            &entity,
            &global,
            &root,
            true,
            "prod",
        );

        assert_eq!(
            values
                .get("deploy")
                .and_then(as_obj)
                .and_then(|deploy| deploy.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            values
                .get("werf")
                .and_then(as_obj)
                .and_then(|werf| werf.get("env"))
                .and_then(JsonValue::as_str),
            Some("prod")
        );
        assert!(!values
            .as_object()
            .is_some_and(|map| map.contains_key("unusedTopLevel")));
    }

    #[test]
    fn manifest_preview_values_force_selected_entity_enabled() {
        let entity = json!({
            "enabled": false,
            "image": { "name": "nginx" }
        });
        let root = json!({
            "global": {
                "validation": {
                    "allowNativeListsInBuiltInListFields": true
                }
            },
            "apps-stateless": {
                "target": {
                    "enabled": false,
                    "image": { "name": "nginx" }
                },
                "other": {
                    "enabled": true
                }
            }
        });
        let global = root.get("global").cloned().expect("global");
        let values = build_manifest_preview_values_with_root(
            "apps-stateless",
            "target",
            &entity,
            &global,
            &root,
            true,
            "demo",
        );

        assert_eq!(
            values
                .get("apps-stateless")
                .and_then(as_obj)
                .and_then(|group| group.get("target"))
                .and_then(as_obj)
                .and_then(|app| app.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            values
                .get("apps-stateless")
                .and_then(as_obj)
                .and_then(|group| group.get("other"))
                .and_then(as_obj)
                .and_then(|app| app.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );
    }

    #[test]
    fn parse_key_and_include_list_tokens_validate_shape() {
        assert_eq!(
            parse_key_line("  good-key_1: value"),
            Some((2, "good-key_1", "value"))
        );
        assert!(parse_key_line("  bad/key: value").is_none());
        assert_eq!(
            parse_list_item_token("    - \"profile-a\""),
            Some((4, "profile-a".to_string()))
        );
        assert!(parse_list_item_token("    - bad/path").is_none());
    }

    #[test]
    fn find_parent_key_uses_nearest_less_indented_key() {
        let lines = vec![
            "global:",
            "  _includes:",
            "    base:",
            "      enabled: true",
        ];
        assert_eq!(find_parent_key(&lines, 3, 6).as_deref(), Some("base"));
        assert_eq!(find_parent_key(&lines, 1, 2).as_deref(), Some("global"));
    }

    #[test]
    fn block_scalar_content_mask_detects_pipe_and_anchor_headers() {
        let lines = vec![
            "root:",
            "  plain: value",
            "  one: |",
            "    - a",
            "    - b",
            "  two: &anchored >-",
            "    line1",
            "    line2",
            "  after: ok",
        ];
        let blocked = block_scalar_content_lines(&lines);
        assert!(!blocked[0]);
        assert!(!blocked[1]);
        assert!(!blocked[2]);
        assert!(blocked[3]);
        assert!(blocked[4]);
        assert!(!blocked[5]);
        assert!(blocked[6]);
        assert!(blocked[7]);
        assert!(!blocked[8]);
    }

    #[test]
    fn block_scalar_content_mask_detects_standalone_pipe_header() {
        let lines = vec![
            "root:",
            "  ports:",
            "    dev:",
            "      |",
            "        - a",
            "        - b",
            "    after: ok",
        ];
        let blocked = block_scalar_content_lines(&lines);
        assert!(!blocked[0]);
        assert!(!blocked[1]);
        assert!(!blocked[2]);
        assert!(blocked[3]);
        assert!(blocked[4]);
        assert!(blocked[5]);
        assert!(!blocked[6]);
    }

    #[test]
    fn diagnostics_ignore_include_lookalikes_inside_block_scalar() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  _includes:
    base:
      enabled: true
apps-stateless:
  app-1:
    _include:
      - base
    note: |
      _include:
        - fake-profile
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("fake-profile")),
            "block scalar payload must not be parsed as include usage"
        );
    }

    #[test]
    fn diagnostics_report_missing_include_file_but_skip_templated_path() {
        let td = TempDir::new().expect("tmp");
        let profiles_dir = td.path().join("profiles");
        fs::create_dir_all(&profiles_dir).expect("mkdir");
        fs::write(profiles_dir.join("base.yaml"), "x: 1\n").expect("write");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
global:
  _include_files:
    - profiles/base.yaml
    - profiles/missing.yaml
    - '{{ printf "profiles/%s.yaml" .Values.env }}'
apps-stateless:
  api:
    _include:
      - base
"#;
        let uri = format!("file:///{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        let missing: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.message.contains("Include file not found"))
            .collect();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].message.contains("profiles/missing.yaml"));
        assert!(diagnostics
            .iter()
            .all(|d| !d.message.contains("profiles/%s.yaml")));
    }

    #[test]
    fn diagnostics_do_not_treat_block_scalar_lists_as_include_files() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values.yaml");
        let defaults_path = td.path().join("defaults.yaml");
        fs::write(&defaults_path, "enabled: true\n").expect("write defaults");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
apps-stateless:
  app-1:
    _include_files:
      - defaults.yaml
    ports: |
      - containerPort: 8080
        name: http
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("containerPort")),
            "block scalar list items must not be treated as include files"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("name: http")),
            "block scalar list items must not be treated as include files"
        );
    }

    #[test]
    fn diagnostics_do_not_treat_standalone_block_scalar_lists_as_include_files() {
        let td = TempDir::new().expect("tmp");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
apps-stateless:
  app-1:
    ports:
      _default:
        |
          - containerPort: 8080
            name: http
      dev:
        |
          - containerPort: 8080
            name: althttp
          - containerPort: 5000
            name: debugport
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("containerPort")),
            "standalone block scalar list items must not be treated as include files"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("name: debugport")),
            "standalone block scalar list items must not be treated as include files"
        );
    }

    #[test]
    fn diagnostics_see_profiles_from_root_include_from_file_global_includes_wrapper() {
        let td = TempDir::new().expect("tmp");
        let defaults_path = td.path().join("helm-apps-defaults.yaml");
        fs::write(
            &defaults_path,
            r#"
global:
  _includes:
    apps-stateless-defaultApp:
      enabled: true
"#,
        )
        .expect("write defaults");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
_include_from_file: helm-apps-defaults.yaml
apps-stateless:
  api:
    _include:
      - apps-stateless-defaultApp
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: apps-stateless-defaultApp")));
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unused include profile: _include_from_file")));
    }

    #[test]
    fn diagnostics_see_profiles_from_plain_top_level_include_file_map() {
        let td = TempDir::new().expect("tmp");
        let defaults_path = td.path().join("helm-apps-defaults.yaml");
        fs::write(
            &defaults_path,
            r#"
helm-apps-defaults:
  enabled: false
apps-default-library-app:
  _include: ["helm-apps-defaults"]
apps-stateless-defaultApp:
  _include: ["apps-default-library-app"]
"#,
        )
        .expect("write defaults");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
global:
  _includes:
    _include_from_file: helm-apps-defaults.yaml
    default-app:
      _include: ["apps-stateless-defaultApp"]
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: apps-stateless-defaultApp")));
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unused include profile: _include_from_file")));
    }

    #[test]
    fn diagnostics_do_not_take_global_includes_from_nested_include_from_file_payload() {
        let td = TempDir::new().expect("tmp");
        let nested_path = td.path().join("nested.yaml");
        fs::write(
            &nested_path,
            r#"
global:
  _includes:
    from-nested:
      enabled: true
"#,
        )
        .expect("write nested");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
apps-stateless:
  api:
    _include_from_file: nested.yaml
    _include:
      - from-nested
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().any(|d| d
            .message
            .contains("Unresolved include profile: from-nested")));
    }

    #[test]
    fn diagnostics_resolve_chain_of_global_includes_include_from_file() {
        let td = TempDir::new().expect("tmp");
        let level1_path = td.path().join("level1.yaml");
        let level2_path = td.path().join("level2.yaml");
        fs::write(
            &level1_path,
            r#"
_include_from_file: level2.yaml
apps-default:
  enabled: true
"#,
        )
        .expect("write level1");
        fs::write(
            &level2_path,
            r#"
apps-stateless-defaultApp:
  _include: ["apps-default"]
"#,
        )
        .expect("write level2");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
global:
  _includes:
    _include_from_file: level1.yaml
apps-stateless:
  api:
    _include:
      - apps-stateless-defaultApp
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: apps-stateless-defaultApp")));
    }

    #[test]
    fn diagnostics_see_simple_top_level_profile_from_global_includes_include_from_file() {
        let td = TempDir::new().expect("tmp");
        let defaults_path = td.path().join("simple-defaults.yaml");
        fs::write(
            &defaults_path,
            r#"
simple-profile:
  enabled: false
"#,
        )
        .expect("write defaults");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");

        let src = r#"
global:
  _includes:
    _include_from_file: simple-defaults.yaml
apps-stateless:
  api:
    _include:
      - simple-profile
"#;
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: simple-profile")));
    }

    #[test]
    fn diagnostics_resolve_sha_profile_from_include_files() {
        let td = TempDir::new().expect("tmp");
        let include_path = td.path().join("defaults.yaml");
        fs::write(&include_path, "enabled: true\n").expect("write include");
        let values_path = td.path().join("values.yaml");
        fs::write(&values_path, "global: {}\n").expect("write values");
        let include_name = include_name_from_path("defaults.yaml");

        let src = format!(
            r#"
apps-stateless:
  api:
    _include_files:
      - defaults.yaml
    _include:
      - {include_name}
"#
        );
        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, &src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains(&format!("Unresolved include profile: {include_name}"))));
    }

    #[test]
    fn diagnostics_treat_top_level_include_file_keys_as_definitions() {
        let td = TempDir::new().expect("tmp");
        let include_path = td.path().join("helm-apps-defaults.yaml");
        fs::write(&include_path, "x: 1\n").expect("touch file");
        let src = r#"
helm-apps-defaults:
  enabled: false
apps-default-library-app:
  _include: ["helm-apps-defaults"]
apps-stateless-defaultApp:
  _include: ["apps-default-library-app"]
"#;
        let uri = format!("file://{}", include_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: helm-apps-defaults")));
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: apps-default-library-app")));
    }

    #[test]
    fn diagnostics_do_not_report_unused_for_top_level_include_file_profiles() {
        let td = TempDir::new().expect("tmp");
        let include_path = td.path().join("helm-apps-defaults.yaml");
        let src = r#"
apps-cronjobs-defaultCronJob:
  enabled: false
apps-secrets-defaultSecret:
  _include: ["apps-cronjobs-defaultCronJob"]
"#;
        fs::write(&include_path, src).expect("write include file");
        let uri = format!("file://{}", include_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unused include profile: apps-cronjobs-defaultCronJob")));
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unused include profile: apps-secrets-defaultSecret")));
    }

    fn has_diagnostic_code(diagnostics: &[Diagnostic], code: &str) -> bool {
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_ref().is_some_and(|current| {
                matches!(current, lsp_types::NumberOrString::String(current_code) if current_code == code)
            })
        })
    }

    #[test]
    fn diagnostics_report_unknown_values_path_in_template_action() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    name: '{{ $.Values.global.missing }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(
            &diagnostics,
            "E_TPL_UNKNOWN_VALUES_PATH"
        ));
    }

    #[test]
    fn diagnostics_report_unknown_current_app_path_in_template_action() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    name: '{{ $.CurrentApp.missing }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(
            &diagnostics,
            "E_TPL_UNKNOWN_CURRENT_APP_PATH"
        ));
    }

    #[test]
    fn diagnostics_report_local_values_context_usage() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    name: '{{ .Values.global.env }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(
            &diagnostics,
            "E_TPL_LOCAL_VALUES_CONTEXT"
        ));
    }

    #[test]
    fn diagnostics_soft_validate_template_syntax_in_any_string_value() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    badExpr: '{{ $.Values.global..env }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(&diagnostics, "E_TPL_SOFT_PARSE"));
    }

    #[test]
    fn diagnostics_detect_single_left_delimiter_template_typo() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    host: '*-{{ $.Values.global.env }}.apps.mrms.{ include "fl.value" (list $ . $.Values.global.base_url) }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(&diagnostics, "E_TPL_SINGLE_LEFT_DELIM"));
    }

    #[test]
    fn diagnostics_do_not_flag_plain_curly_braces_as_template_typo() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    note: '{example payload: true}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(!has_diagnostic_code(
            &diagnostics,
            "E_TPL_SINGLE_LEFT_DELIM"
        ));
    }

    #[test]
    fn diagnostics_allow_global_env_as_ci_injected_value() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  baseUrl: example.local
apps-stateless:
  app-1:
    enabled: true
    envName: '{{ $.Values.global.env }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(!has_diagnostic_code(
            &diagnostics,
            "E_TPL_UNKNOWN_VALUES_PATH"
        ));
    }

    #[test]
    fn diagnostics_keep_enabled_path_validation_for_entities() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    shouldDeploy: '{{ $.Values.apps-stateless.app-2.enabled }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(
            &diagnostics,
            "E_TPL_UNKNOWN_VALUES_PATH"
        ));
    }

    #[test]
    fn diagnostics_report_wrong_arity_for_fl_value_include() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    name: '{{ include "fl.value" (list $ .) }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(&diagnostics, "E_INCLUDE_ARGC"));
    }

    #[test]
    fn template_assist_completes_values_path() {
        let src = r#"
global:
  env: dev
  labels:
    addEnv: false
apps-stateless:
  app-1:
    enabled: true
    name: '{{ $.Values.global. }}'
"#;
        let line = 8u32;
        let marker = "$.Values.global.";
        let line_text = src.lines().nth(line as usize).expect("line with marker");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;

        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: None,
                text: Some(src.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");

        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "env"));
        assert!(result.completions.iter().any(|it| it.label == "labels"));
    }

    #[test]
    fn template_assist_completes_current_app_path() {
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    service:
      enabled: true
    note: '{{ $.CurrentApp. }}'
"#;
        let line = 8u32;
        let marker = "$.CurrentApp.";
        let line_text = src.lines().nth(line as usize).expect("line with marker");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;

        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: None,
                text: Some(src.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");

        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "enabled"));
        assert!(result.completions.iter().any(|it| it.label == "service"));
    }

    #[test]
    fn template_assist_suggests_fl_value_snippet_inside_template_action() {
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    note: '{{ incl }}'
"#;
        let line = 6u32;
        let marker = "incl";
        let line_text = src.lines().nth(line as usize).expect("line with marker");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;

        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: None,
                text: Some(src.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");

        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "fl.value"));
        assert!(result
            .completions
            .iter()
            .any(|it| it.label == "$.CurrentApp"));
    }

    #[test]
    fn include_call_scanner_handles_unicode_actions_without_panicking() {
        let action = r#"{{- fail (printf "Не установлены лимиты по памяти для приложения %s" $.CurrentApp.name) }}"#;
        let calls = collect_include_calls_in_action(action);
        assert!(calls.is_empty());
    }

    #[test]
    fn template_action_lookup_handles_non_char_boundary_cursor() {
        let text = r#"value: '{{ "Ж" }}'"#;
        let non_char_boundary = text.find('Ж').expect("unicode marker") + 1;
        assert!(find_template_action_at_cursor(text, non_char_boundary).is_some());
    }

    #[test]
    fn utf16_col_for_offset_handles_non_char_boundary_offsets() {
        let text = "Жx";
        let idx = TextLineIndex::new(text);
        assert_eq!(idx.utf16_col_for_offset(text, 0, 1), 0);
        assert_eq!(idx.utf16_col_for_offset(text, 0, text.len()), 2);
    }

    #[test]
    fn diagnostics_validate_builtin_release_paths() {
        let uri = "file:///tmp/values.yaml".parse::<Uri>().expect("uri");
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    installLabel: '{{ $.Release.IsInstall }}'
    badLabel: '{{ $.Release.NotExists }}'
"#;
        let diagnostics = build_diagnostics(&uri, src);
        assert!(has_diagnostic_code(
            &diagnostics,
            "E_TPL_UNKNOWN_BUILTIN_PATH"
        ));
    }

    #[test]
    fn template_assist_completes_release_builtin_paths() {
        let src = r#"
global:
  env: dev
apps-stateless:
  app-1:
    enabled: true
    note: '{{ $.Release. }}'
"#;
        let line = 6u32;
        let marker = "$.Release.";
        let line_text = src.lines().nth(line as usize).expect("line with marker");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;
        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: None,
                text: Some(src.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");
        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "IsInstall"));
        assert!(result.completions.iter().any(|it| it.label == "Namespace"));
    }

    #[test]
    fn diagnostics_for_include_file_use_chart_values_and_resolved_current_app_context() {
        let td = TempDir::new().expect("tmp");
        let chart_yaml = td.path().join("Chart.yaml");
        fs::write(
            &chart_yaml,
            r#"
apiVersion: v2
name: test-chart
version: 0.1.0
"#,
        )
        .expect("write chart");
        let values_path = td.path().join("values.yaml");
        fs::write(
            &values_path,
            r#"
global:
  _includes:
    _include_from_file: defaults.yaml
deploy:
  enabled: true
apps-stateless:
  app-1:
    _include:
      - default-app
"#,
        )
        .expect("write values");
        let defaults_path = td.path().join("defaults.yaml");
        let defaults_text = r#"
base:
  _options:
    partOf: core
default-app:
  _include: ["base"]
  labels: |
    app.kubernetes.io/part-of: "{{ $.CurrentApp._options.partOf }}"
    app.kubernetes.io/version: "{{ $.CurrentApp.CurrentAppVersion }}"
    deploy-enabled: "{{ $.Values.deploy.enabled }}"
"#;
        fs::write(&defaults_path, defaults_text).expect("write defaults");

        let uri = format!("file://{}", defaults_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let diagnostics = build_diagnostics(&uri, defaults_text);

        assert!(diagnostics
            .iter()
            .all(|d| !matches!(d.code, Some(lsp_types::NumberOrString::String(ref c)) if c == "E_TPL_UNKNOWN_VALUES_PATH")));
        assert!(diagnostics
            .iter()
            .all(|d| !matches!(d.code, Some(lsp_types::NumberOrString::String(ref c)) if c == "E_TPL_UNKNOWN_CURRENT_APP_PATH")));
        assert!(diagnostics
            .iter()
            .all(|d| !matches!(d.code, Some(lsp_types::NumberOrString::String(ref c)) if c == "E_TPL_CURRENT_APP_SCOPE")));
    }

    #[test]
    fn secret_values_file_is_treated_as_helm_apps_values_source() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        let secret_path = td.path().join("secret-values.yaml");
        let uri = format!("file://{}", secret_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");

        assert!(is_helm_apps_values_source(
            &uri,
            "global:\n  minioSecrets:\n    accessKey: deadbeef\n",
        ));
    }

    #[test]
    fn parse_and_expand_values_root_merges_werf_secret_values_into_root_context() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        let values_path = td.path().join("values.yaml");
        let values_text = r#"
global:
  _includes: {}
apps-stateless:
  app-1:
    enabled: true
"#;
        fs::write(&values_path, values_text).expect("write values");
        fs::write(
            td.path().join("secret-values.yaml"),
            r#"
global:
  minioSecrets:
    accessKey:
      _default: encrypted
"#,
        )
        .expect("write secret values");

        let uri = format!("file://{}", values_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let merged = parse_and_expand_values_root(Some(&uri), values_text).expect("merged root");

        assert_eq!(
            merged
                .get("global")
                .and_then(as_obj)
                .and_then(|global| global.get("minioSecrets"))
                .and_then(as_obj)
                .and_then(|secrets| secrets.get("accessKey"))
                .and_then(as_obj)
                .and_then(|access_key| access_key.get("_default"))
                .and_then(JsonValue::as_str),
            Some("encrypted")
        );
    }

    #[test]
    fn parse_and_expand_values_root_prefers_open_secret_values_override() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes: {}
"#,
        )
        .expect("write values");
        let secret_path = td.path().join("secret-values.yaml");
        fs::write(
            &secret_path,
            r#"
global:
  apiTokens:
    shared:
      _default: old
"#,
        )
        .expect("write secret values");

        let secret_override = r#"
global:
  apiTokens:
    shared:
      _default: new
"#;
        let uri = format!("file://{}", secret_path.to_string_lossy())
            .parse::<Uri>()
            .expect("uri");
        let merged =
            parse_and_expand_values_root(Some(&uri), secret_override).expect("merged root");

        assert_eq!(
            merged
                .get("global")
                .and_then(as_obj)
                .and_then(|global| global.get("apiTokens"))
                .and_then(as_obj)
                .and_then(|tokens| tokens.get("shared"))
                .and_then(as_obj)
                .and_then(|shared| shared.get("_default"))
                .and_then(JsonValue::as_str),
            Some("new")
        );
    }

    #[test]
    fn template_assist_for_include_file_completes_values_from_primary_values_yaml() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes:
    _include_from_file: defaults.yaml
deploy:
  enabled: true
"#,
        )
        .expect("write values");
        let defaults_path = td.path().join("defaults.yaml");
        let defaults_text = r#"
default-app:
  labels: |
    deploy-enabled: "{{ $.Values.deploy. }}"
"#;
        fs::write(&defaults_path, defaults_text).expect("write defaults");

        let line = 3u32;
        let marker = "$.Values.deploy.";
        let line_text = defaults_text
            .lines()
            .nth(line as usize)
            .expect("marker line");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;
        let uri_text = format!("file://{}", defaults_path.to_string_lossy());

        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: Some(uri_text),
                text: Some(defaults_text.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");

        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "enabled"));
    }

    #[test]
    fn list_entities_for_include_file_reads_root_values_context() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes: {}
_include_files:
  - deployments-values.yaml
apps-stateless:
  root-app:
    enabled: false
"#,
        )
        .expect("write values");
        let include_path = td.path().join("deployments-values.yaml");
        let include_text = r#"
apps-stateless:
  include-app:
    enabled: true
"#;
        fs::write(&include_path, include_text).expect("write include values");

        let result = list_entities_request(
            &ServerState::default(),
            ListEntitiesParams {
                uri: Some(format!("file://{}", include_path.to_string_lossy())),
                text: Some(include_text.to_string()),
                env: None,
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
            },
        )
        .expect("list entities");

        let group = result
            .groups
            .iter()
            .find(|group| group.name == "apps-stateless")
            .expect("apps-stateless group");
        assert!(group.apps.iter().any(|app| app == "include-app"));
        assert!(result
            .enabled_entities
            .iter()
            .any(|entity| entity.group == "apps-stateless" && entity.app == "include-app"));
        assert!(!result
            .enabled_entities
            .iter()
            .any(|entity| entity.group == "apps-stateless" && entity.app == "root-app"));
    }

    #[test]
    fn resolve_entity_for_include_file_reads_root_values_context() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes: {}
_include_files:
  - deployments-values.yaml
"#,
        )
        .expect("write values");
        let include_path = td.path().join("deployments-values.yaml");
        let include_text = r#"
apps-stateless:
  include-app:
    enabled: true
"#;
        fs::write(&include_path, include_text).expect("write include values");

        let result = resolve_entity_request(
            &ServerState::default(),
            ResolveEntityParams {
                uri: Some(format!("file://{}", include_path.to_string_lossy())),
                text: Some(include_text.to_string()),
                group: "apps-stateless".to_string(),
                app: "include-app".to_string(),
                env: None,
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
            },
        )
        .expect("resolve entity");
        assert_eq!(
            result
                .entity
                .get("enabled")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn resolve_entity_expands_root_relative_nested_include_from_file_chain() {
        let td = TempDir::new().expect("tmp");
        fs::create_dir_all(td.path().join("profiles")).expect("mkdir profiles");
        fs::create_dir_all(td.path().join("common")).expect("mkdir common");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes:
    _include_from_file: profiles/defaults.yaml
apps-stateless:
  app-1:
    _include:
      - default-app
"#,
        )
        .expect("write values");
        fs::write(
            td.path().join("profiles/defaults.yaml"),
            r#"
default-app:
  _include_from_file: common/default-app-base.yaml
  enabled: true
"#,
        )
        .expect("write defaults");
        fs::write(
            td.path().join("common/default-app-base.yaml"),
            r#"
image:
  name: nginx
"#,
        )
        .expect("write base");

        let values_path = td.path().join("values.yaml");
        let result = resolve_entity_request(
            &ServerState::default(),
            ResolveEntityParams {
                uri: Some(format!("file://{}", values_path.to_string_lossy())),
                text: None,
                group: "apps-stateless".to_string(),
                app: "app-1".to_string(),
                env: None,
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
            },
        )
        .expect("resolve entity");

        assert_eq!(
            result
                .entity
                .get("image")
                .and_then(as_obj)
                .and_then(|image| image.get("name"))
                .and_then(JsonValue::as_str),
            Some("nginx")
        );
    }

    #[test]
    fn resolve_entity_accepts_scalar_include_files_and_loads_profile() {
        let td = TempDir::new().expect("tmp");
        fs::create_dir_all(td.path().join("profiles")).expect("mkdir profiles");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes: {}
apps-stateless:
  app-1:
    _include_files: profiles/default-app.yaml
"#,
        )
        .expect("write values");
        fs::write(
            td.path().join("profiles/default-app.yaml"),
            r#"
labels: |
  team: platform
"#,
        )
        .expect("write include profile");

        let values_path = td.path().join("values.yaml");
        let result = resolve_entity_request(
            &ServerState::default(),
            ResolveEntityParams {
                uri: Some(format!("file://{}", values_path.to_string_lossy())),
                text: None,
                group: "apps-stateless".to_string(),
                app: "app-1".to_string(),
                env: None,
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
            },
        )
        .expect("resolve entity");

        assert_eq!(
            result.entity.get("labels").and_then(JsonValue::as_str),
            Some("team: platform\n")
        );
    }

    #[test]
    fn template_assist_for_include_file_completes_current_app_runtime_and_options() {
        let td = TempDir::new().expect("tmp");
        fs::write(
            td.path().join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            td.path().join("values.yaml"),
            r#"
global:
  _includes:
    _include_from_file: defaults.yaml
apps-stateless:
  app-1:
    _include:
      - default-app
"#,
        )
        .expect("write values");
        let defaults_path = td.path().join("defaults.yaml");
        let defaults_text = r#"
base:
  _options:
    partOf: core
default-app:
  _include: ["base"]
  labels: |
    app-part-of: "{{ $.CurrentApp. }}"
"#;
        fs::write(&defaults_path, defaults_text).expect("write defaults");

        let line = 7u32;
        let marker = "$.CurrentApp.";
        let line_text = defaults_text
            .lines()
            .nth(line as usize)
            .expect("marker line");
        let character = (line_text.find(marker).expect("marker offset") + marker.len()) as u32;
        let uri_text = format!("file://{}", defaults_path.to_string_lossy());

        let result = template_assist_request(
            &ServerState::default(),
            TemplateAssistParams {
                uri: Some(uri_text),
                text: Some(defaults_text.to_string()),
                line,
                character,
            },
        )
        .expect("template assist");

        assert!(result.inside_template);
        assert!(result.completions.iter().any(|it| it.label == "_options"));
        assert!(result
            .completions
            .iter()
            .any(|it| it.label == "CurrentAppVersion"));
    }

    #[test]
    fn include_name_from_path_uses_sha256_of_raw_path() {
        assert_eq!(
            include_name_from_path("profiles/base.yaml"),
            "d584b060afcc4eff599e85c6284eae0b9ffe50a198ccc65d569d6eb4649b72bc"
        );
        assert_eq!(
            include_name_from_path("profiles/base.yml"),
            "c0566af2c84897f26ba75f08af4537c6dd1997700e21c2477c66bb99a7ba8449"
        );
        assert_eq!(
            include_name_from_path("profiles/UPPER.YAML"),
            "99a95bc88e935f834b683d9271bbf171acb92b18f6b4323da00692fe08f41d51"
        );
        assert_eq!(
            include_name_from_path("profiles/noext"),
            "1ae2a0772f82af36af3295361b35b22ef532e9ac1ef727fc7c8957f88852c96b"
        );
    }

    #[test]
    fn uri_and_candidate_helpers_handle_relative_and_absolute_paths() {
        assert!(file_path_from_uri_string("https://example.org/a.yaml").is_none());
        assert!(file_path_from_uri_string("file://").is_none());
        assert_eq!(
            file_path_from_uri_string("file:///tmp/happ-workspace%202/values.yaml"),
            Some(PathBuf::from("/tmp/happ-workspace 2/values.yaml"))
        );

        let base = Path::new("/tmp/happ-tests");
        let rel = build_include_candidates("profiles/base.yaml", Some(base));
        assert_eq!(rel, vec![base.join("profiles/base.yaml")]);

        let abs = build_include_candidates("/etc/hosts", Some(base));
        assert_eq!(abs, vec![Path::new("/etc/hosts").to_path_buf()]);
    }

    #[test]
    fn diagnostics_resolve_root_include_from_file_for_open_include_file_inside_path_with_spaces() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart with spaces");
        fs::create_dir_all(&chart_dir).expect("chart dir");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test\nversion: 0.1.0\n",
        )
        .expect("chart yaml");
        fs::write(
            chart_dir.join("values.yaml"),
            r#"
global:
  _includes:
    _include_from_file: defaults.yaml
_include_files:
  - deployments-values.yaml
"#,
        )
        .expect("root values");
        fs::write(
            chart_dir.join("defaults.yaml"),
            r#"
helm-apps-defaults:
  enabled: false
"#,
        )
        .expect("defaults");
        let deployments_src = r#"
apps-stateless:
  api-gateway:
    _include:
      - helm-apps-defaults
"#;
        let deployments_path = chart_dir.join("deployments-values.yaml");
        fs::write(&deployments_path, deployments_src).expect("deployments values");

        let uri = format!(
            "file://{}",
            deployments_path.to_string_lossy().replace(' ', "%20")
        )
        .parse::<Uri>()
        .expect("uri");
        let diagnostics = build_diagnostics(&uri, deployments_src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Unresolved include profile: helm-apps-defaults")));
    }

    #[test]
    fn diagnostics_do_not_report_existing_nested_include_file_inside_chart_with_spaces() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart with spaces");
        fs::create_dir_all(chart_dir.join("configs/rms-file-service")).expect("config dir");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test\nversion: 0.1.0\n",
        )
        .expect("chart yaml");
        fs::write(
            chart_dir.join("configs/rms-file-service/application.yaml"),
            "spring:\n  application:\n    name: rms-file-service\n",
        )
        .expect("config file");
        let deployments_src = r#"
apps-stateless:
  file-service:
    containers:
      main:
        configFilesYAML:
          application.yaml:
            content:
              _include_files:
                - configs/rms-file-service/application.yaml
"#;
        let deployments_path = chart_dir.join("deployments-values.yaml");
        fs::write(&deployments_path, deployments_src).expect("deployments values");

        let uri = format!(
            "file://{}",
            deployments_path.to_string_lossy().replace(' ', "%20")
        )
        .parse::<Uri>()
        .expect("uri");
        let diagnostics = build_diagnostics(&uri, deployments_src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Include file not found: configs/rms-file-service/application.yaml")));
    }

    #[test]
    fn diagnostics_resolve_include_files_relative_to_root_values_base_for_nested_values_docs() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart with spaces");
        fs::create_dir_all(chart_dir.join("nested")).expect("nested dir");
        fs::create_dir_all(chart_dir.join("configs/rms-file-service")).expect("config dir");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test\nversion: 0.1.0\n",
        )
        .expect("chart yaml");
        fs::write(
            chart_dir.join("values.yaml"),
            "global: {}\n_include_files:\n  - nested/deployments-values.yaml\n",
        )
        .expect("root values");
        fs::write(
            chart_dir.join("configs/rms-file-service/application.yaml"),
            "spring:\n  application:\n    name: rms-file-service\n",
        )
        .expect("config file");
        let deployments_src = r#"
apps-stateless:
  file-service:
    containers:
      main:
        configFilesYAML:
          application.yaml:
            content:
              _include_files:
                - configs/rms-file-service/application.yaml
"#;
        let deployments_path = chart_dir.join("nested/deployments-values.yaml");
        fs::write(&deployments_path, deployments_src).expect("deployments values");

        let uri = format!(
            "file://{}",
            deployments_path.to_string_lossy().replace(' ', "%20")
        )
        .parse::<Uri>()
        .expect("uri");
        let diagnostics = build_diagnostics(&uri, deployments_src);
        assert!(diagnostics.iter().all(|d| !d
            .message
            .contains("Include file not found: configs/rms-file-service/application.yaml")));
    }

    #[test]
    fn render_entity_manifest_request_uses_real_chart_files_and_disables_other_enabled_apps() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart");
        fs::create_dir_all(chart_dir.join("templates")).expect("mkdir templates");
        fs::create_dir_all(chart_dir.join("charts")).expect("mkdir charts");
        fs::create_dir_all(chart_dir.join("configs/rms-reporting")).expect("mkdir configs");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\n",
        )
        .expect("write chart");
        fs::write(
            chart_dir.join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("write init tpl");
        crate::assets::extract_helm_apps_chart(&chart_dir.join("charts/helm-apps"))
            .expect("extract embedded library");
        fs::write(
            chart_dir.join("configs/rms-reporting/application.yaml"),
            "preview: ok\n",
        )
        .expect("write config file");
        let values_text = r#"
global:
  env: demo
apps-stateless:
  rms-reporting:
    enabled: true
    containers:
      main:
        image:
          name: nginx
          staticTag: latest
        configFiles:
          application-prod.yaml:
            mountPath: /config/application.yaml
            content: '{{ $.Files.Get "configs/rms-reporting/application.yaml" }}'
  another-app:
    enabled: true
    containers:
      main:
        image:
          name: nginx
          staticTag: latest
"#;
        let values_path = chart_dir.join("values.yaml");
        fs::write(&values_path, values_text).expect("write values");

        let result = render_entity_manifest_request(
            &ServerState::default(),
            RenderEntityManifestParams {
                uri: Some(format!("file://{}", values_path.to_string_lossy())),
                text: Some(values_text.to_string()),
                group: "apps-stateless".to_string(),
                app: "rms-reporting".to_string(),
                env: Some("demo".to_string()),
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
                renderer: Some("fast".to_string()),
            },
        )
        .expect("render manifest");

        assert!(result.manifest.contains("preview: ok"));
        assert!(!result.manifest.contains("another-app"));
    }

    #[test]
    fn render_entity_manifest_request_keeps_include_files_for_chart_runtime_processing() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart");
        fs::create_dir_all(chart_dir.join("templates")).expect("mkdir templates");
        fs::create_dir_all(chart_dir.join("charts")).expect("mkdir charts");
        fs::create_dir_all(chart_dir.join("configs/rms-reporting")).expect("mkdir configs");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\ndependencies:\n- name: helm-apps\n  version: 0.1.0\n  repository: file://charts/helm-apps\n",
        )
        .expect("write chart");
        fs::write(
            chart_dir.join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("write init tpl");
        crate::assets::extract_helm_apps_chart(&chart_dir.join("charts/helm-apps"))
            .expect("extract embedded library");
        fs::write(
            chart_dir.join("configs/rms-reporting/application.yaml"),
            "spring:\n  graphql:\n    schema:\n      locations:\n        - classpath:graphql/**\n",
        )
        .expect("write config yaml");
        let values_text = r#"
global:
  env: demo
apps-stateless:
  rms-reporting:
    enabled: true
    containers:
      main:
        image:
          name: nginx
          staticTag: latest
        configFilesYAML:
          application.yaml:
            mountPath: /config/application.yaml
            content:
              _include_files:
                - configs/rms-reporting/application.yaml
"#;
        let values_path = chart_dir.join("values.yaml");
        fs::write(&values_path, values_text).expect("write values");

        let result = render_entity_manifest_request(
            &ServerState::default(),
            RenderEntityManifestParams {
                uri: Some(format!("file://{}", values_path.to_string_lossy())),
                text: Some(values_text.to_string()),
                group: "apps-stateless".to_string(),
                app: "rms-reporting".to_string(),
                env: Some("demo".to_string()),
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
                renderer: Some("fast".to_string()),
            },
        )
        .expect("render manifest");

        assert!(result.manifest.contains("classpath:graphql/**"));
        assert!(!result.manifest.contains("E_UNEXPECTED_LIST"));
    }

    #[test]
    fn render_entity_manifest_request_renders_entity_defined_in_root_include_files() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart");
        fs::create_dir_all(chart_dir.join("templates")).expect("mkdir templates");
        fs::create_dir_all(chart_dir.join("charts")).expect("mkdir charts");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\ndependencies:\n- name: helm-apps\n  version: 0.1.0\n  repository: file://charts/helm-apps\n",
        )
        .expect("write chart");
        fs::write(
            chart_dir.join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("write init tpl");
        crate::assets::extract_helm_apps_chart(&chart_dir.join("charts/helm-apps"))
            .expect("extract embedded library");
        fs::write(
            chart_dir.join("helm-apps-defaults.yaml"),
            r#"
global:
  _includes:
    java-backend-app:
      containers:
        main:
          image:
            name: nginx
            staticTag: latest
"#,
        )
        .expect("write defaults");
        let root_values = r#"
global:
  env: demo
  _includes:
    _include_from_file: helm-apps-defaults.yaml
_include_files:
  - deployments-values.yaml
"#;
        let deployments_values = r#"
apps-stateless:
  flight-service:
    enabled: true
    _include: ["java-backend-app"]
"#;
        let values_path = chart_dir.join("values.yaml");
        let deployments_path = chart_dir.join("deployments-values.yaml");
        fs::write(&values_path, root_values).expect("write root values");
        fs::write(&deployments_path, deployments_values).expect("write deployments values");

        let result = render_entity_manifest_request(
            &ServerState::default(),
            RenderEntityManifestParams {
                uri: Some(format!("file://{}", deployments_path.to_string_lossy())),
                text: Some(deployments_values.to_string()),
                group: "apps-stateless".to_string(),
                app: "flight-service".to_string(),
                env: Some("demo".to_string()),
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
                renderer: Some("fast".to_string()),
            },
        )
        .expect("render manifest");

        assert!(result.manifest.contains("nginx:latest"));
        assert!(!result.manifest.contains("index of untyped nil"));
    }

    #[test]
    fn render_entity_manifest_request_renders_cross_file_include_chain() {
        let td = TempDir::new().expect("tmp");
        let chart_dir = td.path().join("chart");
        fs::create_dir_all(chart_dir.join("templates")).expect("mkdir templates");
        fs::create_dir_all(chart_dir.join("charts")).expect("mkdir charts");
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-chart\nversion: 0.1.0\ndependencies:\n- name: helm-apps\n  version: 0.1.0\n  repository: file://charts/helm-apps\n",
        )
        .expect("write chart");
        fs::write(
            chart_dir.join("templates/init-helm-apps-library.yaml"),
            "{{- include \"apps-utils.init-library\" $ }}\n",
        )
        .expect("write init tpl");
        crate::assets::extract_helm_apps_chart(&chart_dir.join("charts/helm-apps"))
            .expect("extract embedded library");
        let root_values = r#"
global:
  env: demo
  _includes:
    default-app:
      replicas: 1
    default-container:
      image:
        name: nginx
        staticTag: latest
_include_files:
  - deployments-values.yaml
"#;
        let deployments_values = r#"
global:
  _includes:
    java-backend-app:
      _include:
        - default-app
      containers:
        main:
          _include:
            - default-container

apps-stateless:
  flight-service:
    enabled: true
    _include: ["java-backend-app"]
"#;
        let values_path = chart_dir.join("values.yaml");
        let deployments_path = chart_dir.join("deployments-values.yaml");
        fs::write(&values_path, root_values).expect("write root values");
        fs::write(&deployments_path, deployments_values).expect("write deployments values");

        let result = render_entity_manifest_request(
            &ServerState::default(),
            RenderEntityManifestParams {
                uri: Some(format!("file://{}", deployments_path.to_string_lossy())),
                text: Some(deployments_values.to_string()),
                group: "apps-stateless".to_string(),
                app: "flight-service".to_string(),
                env: Some("demo".to_string()),
                apply_includes: Some(true),
                apply_env_resolution: Some(true),
                renderer: Some("fast".to_string()),
            },
        )
        .expect("render manifest");

        assert!(result.manifest.contains("nginx:latest"));
        assert!(!result.manifest.contains("index of untyped nil"));
    }

    #[test]
    fn build_manifest_render_values_root_seeds_only_enabled_flag_for_missing_target_entity() {
        let source_root = json!({
            "global": { "env": "dev" },
            "apps-cronjobs": {
                "other": { "enabled": true }
            }
        });
        let resolved_root = json!({
            "global": { "env": "demo" },
            "apps-cronjobs": {
                "aodb-update-master-data-2": {
                    "enabled": true,
                    "schedule": "0 0 * * *"
                },
                "other": { "enabled": true }
            }
        });

        let out = build_manifest_render_values_root(
            &source_root,
            &resolved_root,
            "apps-cronjobs",
            "aodb-update-master-data-2",
            "demo",
        )
        .expect("build manifest render values");

        let out_root = as_obj(&out).expect("root map");
        let out_group = out_root
            .get("apps-cronjobs")
            .and_then(as_obj)
            .expect("apps-cronjobs group");
        assert_eq!(
            out_group
                .get("aodb-update-master-data-2")
                .and_then(as_obj)
                .and_then(|app| app.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            out_group
                .get("aodb-update-master-data-2")
                .and_then(as_obj)
                .map(|app| app.contains_key("schedule")),
            Some(false)
        );
        assert_eq!(
            out_group
                .get("other")
                .and_then(as_obj)
                .and_then(|app| app.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(false)
        );
    }

    #[test]
    fn build_fast_manifest_source_root_materializes_root_include_profiles() {
        let source_root = json!({
            "global": {
                "env": "dev",
                "_includes": {
                    "deployments-values": {
                        "apps-stateless": {
                            "flight-service": {
                                "_include": ["java-backend-app"],
                                "containers": {
                                    "main": {
                                        "envVars": {
                                            "SPRING_CONFIG_LOCATION": "/config/application.yaml"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "_include": ["deployments-values"],
            "apps-stateless": {
                "other": { "enabled": true }
            }
        });
        let parsed = ParsedSourceValuesRoot {
            root_map: source_root.as_object().cloned().expect("root map"),
            chart_root: None,
            include_base_dir: None,
            overrides: HashMap::new(),
        };

        let out = build_fast_manifest_source_root(&parsed).expect("build fast manifest source");

        let out_root = as_obj(&out).expect("root map");
        let out_group = out_root
            .get("apps-stateless")
            .and_then(as_obj)
            .expect("apps-stateless group");
        let target = out_group
            .get("flight-service")
            .and_then(as_obj)
            .expect("flight-service app");
        assert!(
            target
                .get("containers")
                .and_then(as_obj)
                .and_then(|containers| containers.get("main"))
                .and_then(as_obj)
                .and_then(|main| main.get("envVars"))
                .is_some()
        );
        assert_eq!(
            out_group
                .get("other")
                .and_then(as_obj)
                .and_then(|app| app.get("enabled"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );
    }

    #[test]
    fn build_manifest_entity_isolation_set_values_from_resolved_root_disables_only_active_siblings()
    {
        let resolved_root = json!({
            "global": { "env": "demo" },
            "apps-stateless": {
                "target": { "enabled": false },
                "enabled-a": { "enabled": true },
                "disabled-a": { "enabled": false }
            },
            "apps-cronjobs": {
                "enabled-b": { "enabled": true },
                "disabled-b": { "enabled": false }
            }
        });

        let set_values = build_manifest_entity_isolation_set_values_from_resolved_root(
            &resolved_root,
            "apps-stateless",
            "target",
        )
        .expect("build isolation set values");

        assert_eq!(
            set_values,
            vec![
                "apps-stateless.target.enabled=true".to_string(),
                "apps-cronjobs.enabled-b.enabled=false".to_string(),
                "apps-stateless.enabled-a.enabled=false".to_string(),
            ]
        );
    }

    #[test]
    fn select_manifest_values_files_prefers_primary_root_for_included_file_owned_by_primary() {
        let current_path = PathBuf::from("/tmp/chart/configs/application.yaml");
        let primary_values = PathBuf::from("/tmp/chart/values.yaml");
        let root_documents = vec![
            primary_values.clone(),
            PathBuf::from("/tmp/chart/deployments-values.yaml"),
        ];
        let include_owners = BTreeSet::from([
            primary_values.clone(),
            PathBuf::from("/tmp/chart/deployments-values.yaml"),
        ]);

        let selected = select_manifest_values_files(
            &current_path,
            &root_documents,
            Some(&primary_values),
            Some(&include_owners),
        );

        assert_eq!(selected, vec![primary_values]);
    }

    #[test]
    fn build_manifest_backend_args_for_helm_uses_values_sets_and_global_env() {
        let args = build_manifest_backend_args(
            ManifestPreviewRenderer::Helm,
            Path::new("/tmp/chart"),
            &[
                PathBuf::from("/tmp/chart/values.yaml"),
                PathBuf::from("/tmp/chart/deployments-values.yaml"),
            ],
            &[
                "apps-stateless.app-1.enabled=true".to_string(),
                "apps-stateless.app-2.enabled=false".to_string(),
            ],
            "demo",
        );

        assert_eq!(
            args,
            vec![
                "template".to_string(),
                "helm-apps-preview".to_string(),
                "/tmp/chart".to_string(),
                "--values".to_string(),
                "/tmp/chart/values.yaml".to_string(),
                "--values".to_string(),
                "/tmp/chart/deployments-values.yaml".to_string(),
                "--set".to_string(),
                "apps-stateless.app-1.enabled=true".to_string(),
                "--set".to_string(),
                "apps-stateless.app-2.enabled=false".to_string(),
                "--set-string".to_string(),
                "global.env=demo".to_string(),
            ]
        );
    }

    #[test]
    fn build_manifest_backend_args_for_werf_uses_dev_ignore_secret_key_and_env() {
        let args = build_manifest_backend_args(
            ManifestPreviewRenderer::Werf,
            Path::new("/tmp/project"),
            &[PathBuf::from("/tmp/project/.helm/values.yaml")],
            &["apps-stateless.app-1.enabled=true".to_string()],
            "demo",
        );

        assert_eq!(
            args,
            vec![
                "render".to_string(),
                "--dir".to_string(),
                "/tmp/project".to_string(),
                "--dev".to_string(),
                "--ignore-secret-key".to_string(),
                "--loose-giterminism".to_string(),
                "--values".to_string(),
                "/tmp/project/.helm/values.yaml".to_string(),
                "--set".to_string(),
                "apps-stateless.app-1.enabled=true".to_string(),
                "--env".to_string(),
                "demo".to_string(),
                "--set-string".to_string(),
                "global.env=demo".to_string(),
            ]
        );
    }
}
