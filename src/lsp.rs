use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};
use lsp_types::{
    notification::{Notification as LspNotificationTrait, PublishDiagnostics},
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, Position, PublishDiagnosticsParams, Range, Uri,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use serde_yaml::{Mapping as YamlMapping, Number as YamlNumber, Value as YamlValue};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    entity: JsonValue,
    global: JsonValue,
    apply_includes: bool,
    default_env: String,
    used_env: String,
    env_discovery: EnvironmentDiscovery,
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
            "customMethods": ["happ/resolveEntity", "happ/renderEntityManifest", "happ/getPreviewTheme"]
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
    thread::spawn(move || {
        loop {
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
        }
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
                            format!(
                                "invalid params for happ/renderEntityManifest: {err}"
                            ),
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
    let context = resolve_entity_context(
        state,
        params.uri,
        params.text,
        params.group.clone(),
        params.app.clone(),
        params.env,
        params.apply_includes,
        params.apply_env_resolution,
    )?;
    let manifest = render_manifest_for_entity(
        &params.group,
        &params.app,
        &context.entity,
        &context.global,
        context.apply_includes,
        &context.used_env,
    )?;
    Ok(RenderEntityManifestResult {
        manifest,
        default_env: context.default_env,
        used_env: context.used_env,
        env_discovery: context.env_discovery,
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
    let text = if let Some(text) = text {
        text
    } else if let Some(uri) = uri.as_ref() {
        state
            .documents
            .get(uri)
            .map(|d| d.text.clone())
            .ok_or_else(|| format!("document not found in LSP state: {uri}"))?
    } else {
        return Err("either 'text' or 'uri' must be provided".to_string());
    };

    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("yaml parse error: {e}"))?;
    let root_json: JsonValue =
        serde_json::to_value(yaml_value).map_err(|e| format!("json conversion error: {e}"))?;
    let root_map = as_obj(&root_json).ok_or_else(|| "values document must be a map".to_string())?;

    let apply_includes = apply_includes.unwrap_or(true);
    let apply_env = apply_env_resolution.unwrap_or(true);

    let expanded = if apply_includes {
        JsonValue::Object(expand_includes_in_values(root_map)?)
    } else {
        JsonValue::Object(root_map.clone())
    };

    let env_discovery = discover_environments(&expanded);
    let default_env = detect_default_env(&expanded, &env_discovery);
    let used_env = env.unwrap_or_else(|| default_env.clone());

    let entity = read_entity(&expanded, &group, &app)?;
    let entity = if apply_env {
        resolve_env_maps(&entity, &used_env)
    } else {
        entity
    };
    let global = read_global(&expanded);
    let global = if apply_env {
        resolve_env_maps(&global, &used_env)
    } else {
        global
    };

    Ok(ResolvedEntityContext {
        entity,
        global,
        apply_includes,
        default_env,
        used_env,
        env_discovery,
    })
}

fn render_manifest_for_entity(
    group: &str,
    app: &str,
    entity: &JsonValue,
    global: &JsonValue,
    apply_includes: bool,
    env: &str,
) -> Result<String, String> {
    let values_json = build_manifest_preview_values(group, app, entity, global, apply_includes, env);
    let values_yaml =
        json_to_yaml_value(&values_json).map_err(|e| format!("build values yaml for preview: {e}"))?;
    let temp_dir = tempfile::Builder::new()
        .prefix("happ-lsp-preview-")
        .tempdir()
        .map_err(|e| format!("create temp dir for preview chart: {e}"))?;
    let chart_dir = temp_dir.path().join("chart");
    let chart_dir_text = chart_dir.to_string_lossy().to_string();
    crate::output::generate_consumer_chart(
        &chart_dir_text,
        Some("happ-lsp-preview"),
        &values_yaml,
        None,
    )
    .map_err(|e| format!("prepare preview chart: {e}"))?;

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
        verify_equivalence: false,
        release_name: "happ-lsp-preview".into(),
        namespace: None,
        values_files: Vec::new(),
        set_values: Vec::new(),
        set_string_values: Vec::new(),
        set_file_values: Vec::new(),
        set_json_values: Vec::new(),
        kube_version: None,
        api_versions: Vec::new(),
        include_crds: false,
        write_rendered_output: None,
    };

    let rendered = crate::source::render_chart(&import_args, &chart_dir_text)
        .map_err(|e| format!("render preview manifest: {e}"))?;
    if rendered.trim().is_empty() {
        return Err("render preview manifest returned empty output".to_string());
    }
    Ok(rendered)
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
    apply_includes: bool,
    env: &str,
) -> JsonValue {
    let source_global = as_obj(global).cloned().unwrap_or_default();
    let mut required_keys: HashSet<String> = HashSet::from([
        "env".to_string(),
        "validation".to_string(),
        "labels".to_string(),
        "deploy".to_string(),
        "releases".to_string(),
    ]);
    collect_global_keys_referenced(entity, &mut required_keys);

    if !apply_includes {
        required_keys.insert("_includes".to_string());
        required_keys.insert("_include_from_file".to_string());
        required_keys.insert("_include_files".to_string());
    }

    let mut global_map = JsonMap::new();
    for key in required_keys {
        if let Some(value) = source_global.get(&key) {
            global_map.insert(key, value.clone());
        }
    }
    global_map.insert("env".to_string(), JsonValue::String(env.to_string()));

    let mut app_map = JsonMap::new();
    app_map.insert(app.to_string(), entity.clone());

    let mut values_map = JsonMap::new();
    values_map.insert("global".to_string(), JsonValue::Object(global_map));
    values_map.insert(group.to_string(), JsonValue::Object(app_map));
    JsonValue::Object(values_map)
}

fn collect_global_keys_referenced(value: &JsonValue, out: &mut HashSet<String>) {
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

fn collect_global_keys_from_template_string(text: &str, out: &mut HashSet<String>) {
    static GLOBAL_KEY_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = GLOBAL_KEY_RE.get_or_init(|| {
        regex::Regex::new(r"(?:\$?\s*\.)?Values\.global\.([A-Za-z0-9_-]+)").expect("regex")
    });
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
    if !looks_like_helm_apps_values_text(text) {
        return publish_diagnostics(connection, uri, Vec::new());
    }
    let diagnostics = build_diagnostics(uri, text);
    publish_diagnostics(connection, uri, diagnostics)
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
    let file_include_names: HashSet<String> = include_file_refs
        .iter()
        .filter(|r| r.kind == IncludeFileRefKind::IncludeFiles)
        .map(|r| include_name_from_path(&r.path))
        .collect();

    let mut defined_names: HashSet<String> = local_defs.iter().map(|d| d.name.clone()).collect();
    defined_names.extend(file_include_names.iter().cloned());

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

    let used_names: HashSet<String> = usages.iter().map(|u| u.name.clone()).collect();
    for def in &local_defs {
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

    let base_dir = uri_to_base_dir(uri);
    for file_ref in include_file_refs {
        if is_templated_include_path(&file_ref.path) {
            continue;
        }
        let candidates = build_include_candidates(&file_ref.path, base_dir.as_deref());
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

    diagnostics
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

#[derive(Debug, Clone)]
struct IncludeDefinitionRef {
    name: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct IncludeUsageRef {
    name: String,
    line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IncludeFileRefKind {
    IncludeFromFile,
    IncludeFiles,
}

#[derive(Debug, Clone)]
struct IncludeFileRef {
    kind: IncludeFileRefKind,
    path: String,
    line: usize,
}

fn collect_local_include_definitions(lines: &[&str]) -> Vec<IncludeDefinitionRef> {
    let mut out = Vec::new();
    let mut in_global = false;
    let mut in_includes = false;

    for (i, line) in lines.iter().enumerate() {
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
            continue;
        }
        if in_global && in_includes && indent == 4 {
            out.push(IncludeDefinitionRef {
                name: key.to_string(),
                line: i,
            });
        }
    }

    out
}

fn collect_include_usages(lines: &[&str]) -> Vec<IncludeUsageRef> {
    let mut out = Vec::new();

    for (i, line) in lines.iter().enumerate() {
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
        let parent = find_parent_key(lines, i, item_indent);
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

    for (i, line) in lines.iter().enumerate() {
        let Some((indent, key, value)) = parse_key_line(line) else {
            continue;
        };
        if key == "_include_from_file" {
            let path = unquote(value.trim());
            if !path.is_empty() {
                refs.push(IncludeFileRef {
                    kind: IncludeFileRefKind::IncludeFromFile,
                    path,
                    line: i,
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
                        kind: IncludeFileRefKind::IncludeFiles,
                        path,
                        line: i,
                    });
                }
            }
            continue;
        }

        for (j, sub_line) in lines.iter().enumerate().skip(i + 1) {
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
                        kind: IncludeFileRefKind::IncludeFiles,
                        path,
                        line: j,
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

fn find_parent_key(lines: &[&str], line: usize, indent: usize) -> Option<String> {
    for i in (0..line).rev() {
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
    let path = Path::new(path_value.trim());
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path_value.trim());
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".yaml") {
        return file_name[..file_name.len() - 5].to_string();
    }
    if lower.ends_with(".yml") {
        return file_name[..file_name.len() - 4].to_string();
    }
    file_name.to_string()
}

fn is_templated_include_path(path_value: &str) -> bool {
    path_value.contains("{{") || path_value.contains("}}")
}

fn uri_to_base_dir(uri: &Uri) -> Option<PathBuf> {
    let uri_str = uri.to_string();
    file_path_from_uri_string(&uri_str).and_then(|p| p.parent().map(Path::to_path_buf))
}

fn file_path_from_uri_string(uri: &str) -> Option<PathBuf> {
    if !uri.starts_with("file://") {
        return None;
    }
    let raw = uri.trim_start_matches("file://");
    if raw.is_empty() {
        return None;
    }
    if cfg!(windows) {
        Some(PathBuf::from(raw.trim_start_matches('/')))
    } else {
        Some(PathBuf::from(raw))
    }
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
                    let profile =
                        resolve_profile(&include_name, includes_map, cache, &mut Vec::new())?;
                    merged = merge_maps(&merged, &profile);
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
        let child_map = resolve_profile(&child, includes_map, cache, stack)?;
        merged = merge_maps(&merged, &child_map);
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
}
