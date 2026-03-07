use super::{MissingValueMode, NativeRenderOptions};
use crate::go_compat::ffi_runtime::{
    cache_dir_from_env, env_bool, env_u64_or, env_usize_or, fnv1a64,
    helper_binary_name as os_helper_binary_name,
};
use crate::go_compat::functions::collect_function_calls_in_template;
use crate::process_guard::{wait_child_with_timeout_limited, ChildWaitError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

// Go parity reference: go/src/text/template/*.go (runtime execution via native Go helper).
const GO_FFI_HELPER_SOURCE: &str = include_str!("go_ffi_helper/main.go");
const DEFAULT_GO_FFI_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_GO_FFI_MAX_PARALLEL: usize = 4;
const GO_FFI_POLL_INTERVAL_MS: u64 = 5;
const DEFAULT_GO_FFI_MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_GO_FFI_MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_GO_FFI_MAX_STDERR_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GoFfiError {
    Unavailable(String),
    Parse(String),
    Execute(String),
}

pub(super) fn parse_error_code(message: &str) -> &'static str {
    if message.starts_with("undefined variable \"") {
        return "undefined_variable";
    }
    if message.contains("{{break}} outside {{range}}") {
        return "break_outside_range";
    }
    if message.contains("{{continue}} outside {{range}}") {
        return "continue_outside_range";
    }
    if message.contains("non executable command in pipeline stage") {
        return "non_executable_command_in_pipeline";
    }
    "go_ffi_parse"
}

#[derive(Debug, Serialize)]
struct GoFfiRequest<'a> {
    template: &'a str,
    data: &'a Value,
    missing_key: &'static str,
    functions: &'a [String],
}

#[derive(Debug, Deserialize)]
struct GoFfiResponse {
    ok: bool,
    #[serde(default)]
    output: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    kind: String,
}

static GO_FFI_HELPER_BINARY: OnceLock<Result<PathBuf, String>> = OnceLock::new();
static GO_FFI_CONCURRENCY: OnceLock<GoFfiConcurrency> = OnceLock::new();

struct GoFfiConcurrency {
    limit: usize,
    in_flight: Mutex<usize>,
    cv: Condvar,
}

impl GoFfiConcurrency {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            in_flight: Mutex::new(0),
            cv: Condvar::new(),
        }
    }
}

struct GoFfiSlotGuard {
    state: &'static GoFfiConcurrency,
}

impl Drop for GoFfiSlotGuard {
    fn drop(&mut self) {
        let mut in_flight = self
            .state
            .in_flight
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *in_flight > 0 {
            *in_flight -= 1;
        }
        self.state.cv.notify_one();
    }
}

fn acquire_go_ffi_slot() -> GoFfiSlotGuard {
    let state = GO_FFI_CONCURRENCY.get_or_init(|| GoFfiConcurrency::new(go_ffi_max_parallel()));
    let mut in_flight = state
        .in_flight
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    while *in_flight >= state.limit {
        in_flight = state
            .cv
            .wait(in_flight)
            .unwrap_or_else(|poison| poison.into_inner());
    }
    *in_flight += 1;
    drop(in_flight);
    GoFfiSlotGuard { state }
}

pub(super) fn render_template_via_go_ffi(
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
) -> Result<String, GoFfiError> {
    let helper = ensure_helper_binary()?;
    let function_names = collect_function_calls_in_template(src);
    let _slot = acquire_go_ffi_slot();
    let payload = serde_json::to_vec(&GoFfiRequest {
        template: src,
        data: root,
        missing_key: missing_key_option(options.missing_value_mode),
        functions: &function_names,
    })
    .map_err(|err| GoFfiError::Unavailable(format!("request serialization failed: {err}")))?;
    if payload.len() > go_ffi_max_request_bytes() {
        return Err(GoFfiError::Unavailable(format!(
            "go ffi request is too large: {} bytes (max {})",
            payload.len(),
            go_ffi_max_request_bytes()
        )));
    }

    let mut child = Command::new(&helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            GoFfiError::Unavailable(format!(
                "failed to spawn go ffi helper {}: {err}",
                helper.display()
            ))
        })?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err(GoFfiError::Unavailable(
            "failed to acquire go ffi helper stdin".to_string(),
        ));
    };
    stdin
        .write_all(&payload)
        .map_err(|err| GoFfiError::Unavailable(format!("failed to write go ffi request: {err}")))?;
    drop(stdin);

    let output = wait_child_with_timeout_limited(
        child,
        go_ffi_timeout(),
        Duration::from_millis(GO_FFI_POLL_INTERVAL_MS),
        go_ffi_max_stdout_bytes(),
        go_ffi_max_stderr_bytes(),
    )
    .map_err(map_wait_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GoFfiError::Unavailable(format!(
            "go ffi helper failed with status {}{}",
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        )));
    }

    let response: GoFfiResponse = serde_json::from_slice(&output.stdout).map_err(|err| {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        GoFfiError::Unavailable(format!(
            "go ffi helper returned invalid json: {err}; stdout={stdout}"
        ))
    })?;

    if response.ok {
        return Ok(response.output);
    }

    let message = normalize_go_template_error(&response.error);
    match response.kind.as_str() {
        "parse" => Err(GoFfiError::Parse(message)),
        "execute" | "" => Err(GoFfiError::Execute(message)),
        _ => Err(GoFfiError::Execute(message)),
    }
}

fn map_wait_error(err: ChildWaitError) -> GoFfiError {
    match err {
        ChildWaitError::StdoutUnavailable => {
            GoFfiError::Unavailable("failed to acquire go ffi helper stdout".to_string())
        }
        ChildWaitError::StderrUnavailable => {
            GoFfiError::Unavailable("failed to acquire go ffi helper stderr".to_string())
        }
        ChildWaitError::StdoutLimitExceeded { limit } => {
            GoFfiError::Unavailable(format!("go ffi helper stdout exceeded {limit} bytes"))
        }
        ChildWaitError::StderrLimitExceeded { limit } => {
            GoFfiError::Unavailable(format!("go ffi helper stderr exceeded {limit} bytes"))
        }
        ChildWaitError::Timeout { timeout, stderr } => {
            GoFfiError::Unavailable(format_timeout_error(timeout, &stderr))
        }
        ChildWaitError::WaitFailed { reason } => {
            GoFfiError::Unavailable(format!("go ffi helper wait failed: {reason}"))
        }
    }
}

fn format_timeout_error(timeout: Duration, stderr: &[u8]) -> String {
    let mut out = format!("go ffi helper timed out after {}ms", timeout.as_millis());
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr.is_empty() {
        out.push_str(": ");
        out.push_str(&stderr);
    }
    out
}

fn normalize_go_template_error(raw: &str) -> String {
    let mut message = raw.trim();

    if let Some(rest) = message.strip_prefix("template: happ-go-ffi:") {
        message = rest;
        if let Some((_, tail)) = message.split_once(": ") {
            message = tail;
        }
        if let Some((lead, tail)) = message.split_once(": ") {
            if lead.chars().all(|c| c.is_ascii_digit()) {
                message = tail;
            }
        }
    }

    if let Some(rest) = message.strip_prefix("executing \"happ-go-ffi\" at <") {
        if let Some((_, tail)) = rest.split_once(">: ") {
            message = tail;
        }
    }

    let mut out = if let Some(name) = parse_function_not_defined(message) {
        format!("\"{name}\" is not a defined function")
    } else {
        message.to_string()
    };
    out = out.replace("; should be of type ", "; should be ");
    if out.starts_with("can't evaluate field ") && out.ends_with(" in type interface {}") {
        out = out.replace(" in type interface {}", " in type []interface {}");
    }
    if let Some(rest) = out.strip_prefix("error calling ") {
        if let Some((name, tail)) = rest.split_once(": ") {
            let canonical = format!("\"{name}\" is not a defined function");
            if tail == canonical {
                out = canonical;
            }
        }
    }
    if let Some(raw) = out
        .strip_prefix("illegal number syntax: \"")
        .and_then(|s| s.strip_suffix('"'))
    {
        out = format!("illegal number syntax: {raw}");
    }
    out
}

fn parse_function_not_defined(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("function \"")?;
    rest.strip_suffix("\" not defined")
}

fn ensure_helper_binary() -> Result<PathBuf, GoFfiError> {
    GO_FFI_HELPER_BINARY
        .get_or_init(build_helper_binary)
        .as_ref()
        .map(PathBuf::clone)
        .map_err(|reason| GoFfiError::Unavailable(reason.clone()))
}

fn build_helper_binary() -> Result<PathBuf, String> {
    if env_bool("HAPP_GO_FFI_DISABLE") {
        return Err("disabled via HAPP_GO_FFI_DISABLE".to_string());
    }
    let go_bin = env::var("HAPP_GO_BIN").unwrap_or_else(|_| "go".to_string());
    let cache_dir = helper_cache_dir();
    fs::create_dir_all(&cache_dir).map_err(|err| {
        format!(
            "failed to create go ffi cache dir {}: {err}",
            cache_dir.display()
        )
    })?;

    let bin_path = cache_dir.join(helper_binary_name());
    let stamp_path = helper_stamp_path(&cache_dir);
    let expected_stamp = helper_stamp_payload();
    if Path::new(&bin_path).exists()
        && fs::read_to_string(&stamp_path)
            .ok()
            .as_deref()
            .is_some_and(|s| s == expected_stamp)
    {
        return Ok(bin_path);
    }

    let src_path = cache_dir.join("happ_go_ffi_helper.go");
    fs::write(&src_path, GO_FFI_HELPER_SOURCE).map_err(|err| {
        format!(
            "failed to write go ffi helper source {}: {err}",
            src_path.display()
        )
    })?;

    let output = Command::new(&go_bin)
        .arg("build")
        .arg("-trimpath")
        .arg("-o")
        .arg(&bin_path)
        .arg(&src_path)
        .output()
        .map_err(|err| format!("failed to run {go_bin} build: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("go build failed with status {}", output.status)
        } else {
            format!("go build failed with status {}: {stderr}", output.status)
        });
    }

    if !Path::new(&bin_path).exists() {
        return Err(format!(
            "go ffi helper binary is missing after build: {}",
            bin_path.display()
        ));
    }
    let _ = fs::write(&stamp_path, expected_stamp);
    Ok(bin_path)
}

fn helper_cache_dir() -> PathBuf {
    cache_dir_from_env("HAPP_GO_FFI_CACHE_DIR", "happ-go-ffi")
}

fn helper_binary_name() -> String {
    os_helper_binary_name("happ-go-ffi")
}

fn helper_stamp_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("happ_go_ffi_helper.stamp")
}

fn helper_stamp_payload() -> String {
    // Stamp lets us reuse the helper across happ process restarts and rebuild only on source change.
    format!(
        "helper_hash={:016x}\nos={}\narch={}\n",
        fnv1a64(GO_FFI_HELPER_SOURCE.as_bytes()),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn missing_key_option(mode: MissingValueMode) -> &'static str {
    match mode {
        MissingValueMode::GoDefault => "default",
        MissingValueMode::GoZero => "zero",
        MissingValueMode::Error => "error",
    }
}

fn go_ffi_timeout() -> Duration {
    Duration::from_millis(env_u64_or(
        "HAPP_GO_FFI_TIMEOUT_MS",
        DEFAULT_GO_FFI_TIMEOUT_MS,
    ))
}

fn go_ffi_max_parallel() -> usize {
    let default = thread::available_parallelism()
        .map(|n| n.get().min(DEFAULT_GO_FFI_MAX_PARALLEL))
        .unwrap_or(DEFAULT_GO_FFI_MAX_PARALLEL);
    env_usize_or("HAPP_GO_FFI_MAX_PARALLEL", default).max(1)
}

fn go_ffi_max_request_bytes() -> usize {
    env_usize_or("HAPP_GO_FFI_MAX_REQUEST_BYTES", DEFAULT_GO_FFI_MAX_REQUEST_BYTES)
}

fn go_ffi_max_stdout_bytes() -> usize {
    env_usize_or("HAPP_GO_FFI_MAX_STDOUT_BYTES", DEFAULT_GO_FFI_MAX_STDOUT_BYTES)
}

fn go_ffi_max_stderr_bytes() -> usize {
    env_usize_or("HAPP_GO_FFI_MAX_STDERR_BYTES", DEFAULT_GO_FFI_MAX_STDERR_BYTES)
}

#[cfg(test)]
mod tests {
    use super::{normalize_go_template_error, parse_error_code, parse_function_not_defined};

    #[test]
    fn normalize_error_strips_go_template_wrappers() {
        let raw = "template: happ-go-ffi:1: function \"tpl\" not defined";
        assert_eq!(
            normalize_go_template_error(raw),
            "\"tpl\" is not a defined function"
        );
    }

    #[test]
    fn normalize_error_collapses_error_calling_wrapper() {
        let raw = "template: happ-go-ffi:1: executing \"happ-go-ffi\" at <tpl>: error calling tpl: \"tpl\" is not a defined function";
        assert_eq!(
            normalize_go_template_error(raw),
            "\"tpl\" is not a defined function"
        );
    }

    #[test]
    fn normalize_error_unquotes_illegal_number_syntax() {
        let raw = "template: happ-go-ffi:1: illegal number syntax: \"09\"";
        assert_eq!(
            normalize_go_template_error(raw),
            "illegal number syntax: 09"
        );
    }

    #[test]
    fn parse_error_code_maps_known_categories() {
        assert_eq!(
            parse_error_code("undefined variable \"$x\""),
            "undefined_variable"
        );
        assert_eq!(
            parse_error_code("template: {{break}} outside {{range}}"),
            "break_outside_range"
        );
        assert_eq!(
            parse_error_code("template: {{continue}} outside {{range}}"),
            "continue_outside_range"
        );
        assert_eq!(
            parse_error_code("non executable command in pipeline stage 2"),
            "non_executable_command_in_pipeline"
        );
        assert_eq!(parse_error_code("something else"), "go_ffi_parse");
    }

    #[test]
    fn parse_function_not_defined_extracts_name() {
        assert_eq!(
            parse_function_not_defined("function \"tpl\" not defined"),
            Some("tpl")
        );
        assert_eq!(parse_function_not_defined("other error"), None);
    }
}
