use crate::chart_ir::{
    encode_node, ChartIr, ChartIrBackend, ChartIrDiagnostic, ChartIrDocument, ChartIrIdentity,
    ChartIrProvenance, ChartIrSeverity, ChartIrSource,
};
use crate::cli::ImportArgs;
use crate::go_compat::ffi_runtime::{
    cache_dir_from_env, env_u64_or, env_usize_or, fnv1a64,
    helper_binary_name as os_helper_binary_name, set_executable_permissions_if_needed,
};
use crate::process_guard::{wait_child_with_timeout_limited, ChildWaitError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_HELM_IR_TIMEOUT_MS: u64 = 120_000;
const HELM_IR_POLL_INTERVAL_MS: u64 = 10;
const EMBEDDED_HELM_IR_HELPER_BIN_ZSTD: &[u8] =
    include_bytes!(env!("HAPP_HELM_IR_HELPER_BIN_ZSTD"));
const DEFAULT_HELM_IR_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_HELM_IR_MAX_STDOUT_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_HELM_IR_MAX_STDERR_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_HELM_IR_MAX_DOCUMENTS: usize = 200_000;
const DEFAULT_HELM_IR_MAX_DIAGNOSTICS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelmIrFfiError {
    Unavailable(String),
    Render(String),
    Decode(String),
}

static HELM_IR_HELPER_BINARY: OnceLock<Result<PathBuf, String>> = OnceLock::new();

#[derive(Debug, Serialize)]
struct HelmIrRequest<'a> {
    mode: &'a str,
    chart_path: &'a str,
    release_name: &'a str,
    namespace: Option<&'a str>,
    values_files: &'a [String],
    set_values: &'a [String],
    set_string_values: &'a [String],
    set_file_values: &'a [String],
    set_json_values: &'a [String],
    kube_version: Option<&'a str>,
    api_versions: &'a [String],
    include_crds: bool,
}

#[derive(Debug, Deserialize)]
struct HelmIrResponse {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    documents: Vec<HelmIrDocument>,
    #[serde(default)]
    diagnostics: Vec<HelmIrDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct HelmIrDocument {
    identity: HelmIrIdentity,
    body: serde_json::Value,
    #[serde(default)]
    template_file: Option<String>,
    #[serde(default)]
    include_chain: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HelmIrIdentity {
    #[serde(default)]
    api_version: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelmIrDiagnostic {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    document_index: Option<usize>,
}

pub fn load_chart_ir_via_helm_goffi(args: &ImportArgs) -> Result<ChartIr, HelmIrFfiError> {
    let response = invoke_helper(args, "ir")?;
    build_chart_ir_from_response(args, response)
}

pub fn render_chart_raw_via_helm_goffi(args: &ImportArgs) -> Result<String, HelmIrFfiError> {
    let output = run_helper(&build_request_payload(args, "raw")?)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let reason = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("helper exited with status {}", output.status)
        };
        return Err(HelmIrFfiError::Render(reason));
    }
    let rendered = String::from_utf8_lossy(&output.stdout).to_string();
    if rendered.trim().is_empty() {
        return Err(HelmIrFfiError::Render(
            "render returned empty output".to_string(),
        ));
    }
    Ok(rendered)
}

fn invoke_helper(args: &ImportArgs, mode: &'static str) -> Result<HelmIrResponse, HelmIrFfiError> {
    let payload = build_request_payload(args, mode)?;
    let output = run_helper(&payload)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let reason = if stderr.is_empty() {
            format!("helper exited with status {}", output.status)
        } else {
            format!("helper exited with status {}: {stderr}", output.status)
        };
        return Err(HelmIrFfiError::Unavailable(reason));
    }

    let response: HelmIrResponse = serde_json::from_slice(&output.stdout).map_err(|err| {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        HelmIrFfiError::Unavailable(format!("invalid helper response: {err}; stdout={stdout}"))
    })?;

    if !response.ok {
        let message = normalize_helper_error(&response.error);
        return Err(match response.kind.as_str() {
            "decode" => HelmIrFfiError::Decode(message),
            "render" | "values" | "load" | "" => HelmIrFfiError::Render(message),
            _ => HelmIrFfiError::Render(message),
        });
    }
    Ok(response)
}

fn build_request_payload(args: &ImportArgs, mode: &'static str) -> Result<Vec<u8>, HelmIrFfiError> {
    serde_json::to_vec(&HelmIrRequest {
        mode,
        chart_path: &args.path,
        release_name: &args.release_name,
        namespace: args.namespace.as_deref(),
        values_files: &args.values_files,
        set_values: &args.set_values,
        set_string_values: &args.set_string_values,
        set_file_values: &args.set_file_values,
        set_json_values: &args.set_json_values,
        kube_version: args.kube_version.as_deref(),
        api_versions: &args.api_versions,
        include_crds: args.include_crds,
    })
    .map_err(|err| HelmIrFfiError::Unavailable(format!("serialize request: {err}")))
}

fn run_helper(payload: &[u8]) -> Result<crate::process_guard::ChildOutput, HelmIrFfiError> {
    let helper = ensure_helper_binary()?;
    if payload.len() > helm_ir_max_request_bytes() {
        return Err(HelmIrFfiError::Unavailable(format!(
            "helm helper request is too large: {} bytes (max {})",
            payload.len(),
            helm_ir_max_request_bytes()
        )));
    }

    let mut child = Command::new(&helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            HelmIrFfiError::Unavailable(format!(
                "spawn helm goffi helper {}: {err}",
                helper.display()
            ))
        })?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err(HelmIrFfiError::Unavailable(
            "helper stdin is unavailable".to_string(),
        ));
    };
    stdin
        .write_all(&payload)
        .map_err(|err| HelmIrFfiError::Unavailable(format!("write helper request: {err}")))?;
    drop(stdin);

    let output = wait_child_with_timeout_limited(
        child,
        helm_ir_timeout(),
        Duration::from_millis(HELM_IR_POLL_INTERVAL_MS),
        helm_ir_max_stdout_bytes(),
        helm_ir_max_stderr_bytes(),
    )
    .map_err(map_wait_error)?;
    Ok(output)
}

fn build_chart_ir_from_response(
    args: &ImportArgs,
    response: HelmIrResponse,
) -> Result<ChartIr, HelmIrFfiError> {
    if response.documents.len() > helm_ir_max_documents() {
        return Err(HelmIrFfiError::Decode(format!(
            "helper returned too many documents: {} (max {})",
            response.documents.len(),
            helm_ir_max_documents()
        )));
    }
    if response.diagnostics.len() > helm_ir_max_diagnostics() {
        return Err(HelmIrFfiError::Decode(format!(
            "helper returned too many diagnostics: {} (max {})",
            response.diagnostics.len(),
            helm_ir_max_diagnostics()
        )));
    }

    let mut ir = ChartIr::new(ChartIrSource {
        backend: ChartIrBackend::HelmGoFfi,
        chart_path: Some(args.path.clone()),
        release_name: Some(args.release_name.clone()),
    });

    for doc in response.documents {
        let body_yaml = serde_yaml::to_value(doc.body)
            .map_err(|err| HelmIrFfiError::Decode(format!("document decode: {err}")))?;
        ir.documents.push(ChartIrDocument {
            identity: ChartIrIdentity {
                api_version: doc.identity.api_version,
                kind: doc.identity.kind,
                name: doc.identity.name,
                namespace: doc.identity.namespace,
            },
            body: encode_node(&body_yaml),
            provenance: Some(ChartIrProvenance {
                template_file: doc.template_file,
                include_chain: doc.include_chain,
            }),
        });
    }

    ir.diagnostics = response
        .diagnostics
        .into_iter()
        .map(|d| ChartIrDiagnostic {
            severity: match d.severity.trim().to_ascii_lowercase().as_str() {
                "error" => ChartIrSeverity::Error,
                "warn" | "warning" => ChartIrSeverity::Warn,
                _ => ChartIrSeverity::Info,
            },
            code: if d.code.trim().is_empty() {
                "helm_goffi".to_string()
            } else {
                d.code
            },
            message: d.message,
            document_index: d.document_index,
        })
        .collect();

    Ok(ir)
}

fn map_wait_error(err: ChildWaitError) -> HelmIrFfiError {
    match err {
        ChildWaitError::StdoutUnavailable => {
            HelmIrFfiError::Unavailable("helper stdout is unavailable".to_string())
        }
        ChildWaitError::StderrUnavailable => {
            HelmIrFfiError::Unavailable("helper stderr is unavailable".to_string())
        }
        ChildWaitError::StdoutLimitExceeded { limit } => {
            HelmIrFfiError::Unavailable(format!("helper stdout exceeded {limit} bytes"))
        }
        ChildWaitError::StderrLimitExceeded { limit } => {
            HelmIrFfiError::Unavailable(format!("helper stderr exceeded {limit} bytes"))
        }
        ChildWaitError::Timeout { timeout, stderr } => {
            let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
            let mut reason = format!("helper timed out after {}ms", timeout.as_millis());
            if !stderr.is_empty() {
                reason.push_str(": ");
                reason.push_str(&stderr);
            }
            HelmIrFfiError::Unavailable(reason)
        }
        ChildWaitError::WaitFailed { reason } => {
            HelmIrFfiError::Unavailable(format!("helper wait failed: {reason}"))
        }
    }
}

fn ensure_helper_binary() -> Result<PathBuf, HelmIrFfiError> {
    HELM_IR_HELPER_BINARY
        .get_or_init(extract_embedded_helper_binary)
        .as_ref()
        .map(PathBuf::clone)
        .map_err(|reason| HelmIrFfiError::Unavailable(reason.clone()))
}

fn extract_embedded_helper_binary() -> Result<PathBuf, String> {
    let cache_dir = helper_cache_dir();
    fs::create_dir_all(&cache_dir).map_err(|err| {
        format!(
            "create helm goffi helper cache dir {}: {err}",
            cache_dir.display()
        )
    })?;
    let stamp_payload = helper_stamp_payload();
    let stamp_path = cache_dir.join("happ_helm_ir_helper.stamp");
    let bin_path = cache_dir.join(helper_binary_name());
    if bin_path.exists()
        && fs::read_to_string(&stamp_path)
            .ok()
            .as_deref()
            .is_some_and(|s| s == stamp_payload)
    {
        return Ok(bin_path);
    }
    let helper_bin = decode_embedded_helper_binary()
        .map_err(|err| format!("decode embedded helm goffi helper (zstd): {err}"))?;
    let temp_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = cache_dir.join(format!(
        "{}.{}.{}.tmp",
        helper_binary_name(),
        std::process::id(),
        temp_suffix
    ));
    fs::write(&temp_path, helper_bin).map_err(|err| {
        format!(
            "write embedded helm goffi helper {}: {err}",
            temp_path.display()
        )
    })?;
    set_executable_permissions_if_needed(&temp_path)
        .map_err(|err| format!("set executable permissions {}: {err}", temp_path.display()))?;
    fs::rename(&temp_path, &bin_path).map_err(|err| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "move embedded helm goffi helper {} -> {}: {err}",
            temp_path.display(),
            bin_path.display()
        )
    })?;
    let _ = fs::write(stamp_path, stamp_payload);
    Ok(bin_path)
}

fn helper_cache_dir() -> PathBuf {
    cache_dir_from_env("HAPP_HELM_IR_FFI_CACHE_DIR", "happ-helm-ir-ffi")
}

fn helper_binary_name() -> String {
    os_helper_binary_name("happ-helm-ir-ffi")
}

fn helper_stamp_payload() -> String {
    format!(
        "helper_bin_zstd_hash={:016x}\nos={}\narch={}\n",
        fnv1a64(EMBEDDED_HELM_IR_HELPER_BIN_ZSTD),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn decode_embedded_helper_binary() -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = zstd::stream::read::Decoder::new(EMBEDDED_HELM_IR_HELPER_BIN_ZSTD)?;
    let mut helper_bin = Vec::new();
    decoder.read_to_end(&mut helper_bin)?;
    Ok(helper_bin)
}

fn helm_ir_timeout() -> Duration {
    Duration::from_millis(env_u64_or(
        "HAPP_HELM_IR_FFI_TIMEOUT_MS",
        DEFAULT_HELM_IR_TIMEOUT_MS,
    ))
}

fn helm_ir_max_request_bytes() -> usize {
    env_usize_or(
        "HAPP_HELM_IR_FFI_MAX_REQUEST_BYTES",
        DEFAULT_HELM_IR_MAX_REQUEST_BYTES,
    )
}

fn helm_ir_max_stdout_bytes() -> usize {
    env_usize_or(
        "HAPP_HELM_IR_FFI_MAX_STDOUT_BYTES",
        DEFAULT_HELM_IR_MAX_STDOUT_BYTES,
    )
}

fn helm_ir_max_stderr_bytes() -> usize {
    env_usize_or(
        "HAPP_HELM_IR_FFI_MAX_STDERR_BYTES",
        DEFAULT_HELM_IR_MAX_STDERR_BYTES,
    )
}

fn helm_ir_max_documents() -> usize {
    env_usize_or("HAPP_HELM_IR_MAX_DOCUMENTS", DEFAULT_HELM_IR_MAX_DOCUMENTS)
}

fn helm_ir_max_diagnostics() -> usize {
    env_usize_or(
        "HAPP_HELM_IR_MAX_DIAGNOSTICS",
        DEFAULT_HELM_IR_MAX_DIAGNOSTICS,
    )
}

fn normalize_helper_error(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "unknown helper error".to_string();
    }
    trimmed.to_string()
}
