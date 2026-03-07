use super::{MissingValueMode, NativeRenderOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

// Go parity reference: go/src/text/template/*.go (runtime execution via native Go helper).
const GO_FFI_HELPER_SOURCE: &str = r#"package main

import (
	"bytes"
	"encoding/json"
	"io"
	"os"
	"text/template"
)

type request struct {
	Template   string      `json:"template"`
	Data       interface{} `json:"data"`
	MissingKey string      `json:"missing_key"`
}

type response struct {
	OK     bool   `json:"ok"`
	Output string `json:"output,omitempty"`
	Error  string `json:"error,omitempty"`
	Kind   string `json:"kind,omitempty"`
}

func main() {
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		writeErr("io", err)
		return
	}
	var req request
	if err := json.Unmarshal(input, &req); err != nil {
		writeErr("decode", err)
		return
	}

	tpl := template.New("happ-go-ffi")
	if req.MissingKey != "" {
		tpl = tpl.Option("missingkey=" + req.MissingKey)
	}
	parsed, err := tpl.Parse(req.Template)
	if err != nil {
		writeErr("parse", err)
		return
	}

	var out bytes.Buffer
	if err := parsed.Execute(&out, req.Data); err != nil {
		writeErr("execute", err)
		return
	}

	writeResponse(response{
		OK:     true,
		Output: out.String(),
	})
}

func writeErr(kind string, err error) {
	writeResponse(response{
		OK:    false,
		Error: err.Error(),
		Kind:  kind,
	})
}

func writeResponse(resp response) {
	enc := json.NewEncoder(os.Stdout)
	_ = enc.Encode(resp)
}
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GoFfiError {
    Unavailable(String),
    Render(String),
}

#[derive(Debug, Serialize)]
struct GoFfiRequest<'a> {
    template: &'a str,
    data: &'a Value,
    missing_key: &'static str,
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

pub(super) fn render_template_via_go_ffi(
    src: &str,
    root: &Value,
    options: NativeRenderOptions,
) -> Result<String, GoFfiError> {
    let helper = ensure_helper_binary()?;
    let payload = serde_json::to_vec(&GoFfiRequest {
        template: src,
        data: root,
        missing_key: missing_key_option(options.missing_value_mode),
    })
    .map_err(|err| GoFfiError::Unavailable(format!("request serialization failed: {err}")))?;

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

    let output = child
        .wait_with_output()
        .map_err(|err| GoFfiError::Unavailable(format!("go ffi helper wait failed: {err}")))?;
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

    let kind = if response.kind.is_empty() {
        "render"
    } else {
        response.kind.as_str()
    };
    let message = if response.error.is_empty() {
        "unknown go ffi error".to_string()
    } else {
        response.error
    };
    Err(GoFfiError::Render(format!(
        "go ffi {kind} error: {message}"
    )))
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

    let src_path = cache_dir.join("happ_go_ffi_helper.go");
    fs::write(&src_path, GO_FFI_HELPER_SOURCE).map_err(|err| {
        format!(
            "failed to write go ffi helper source {}: {err}",
            src_path.display()
        )
    })?;

    let bin_path = cache_dir.join(helper_binary_name());
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
    Ok(bin_path)
}

fn helper_cache_dir() -> PathBuf {
    if let Ok(raw) = env::var("HAPP_GO_FFI_CACHE_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    env::temp_dir().join("happ-go-ffi")
}

fn helper_binary_name() -> &'static str {
    if cfg!(windows) {
        "happ-go-ffi.exe"
    } else {
        "happ-go-ffi"
    }
}

fn missing_key_option(mode: MissingValueMode) -> &'static str {
    match mode {
        MissingValueMode::GoDefault => "default",
        MissingValueMode::GoZero => "zero",
        MissingValueMode::Error => "error",
    }
}

fn env_bool(name: &str) -> bool {
    let Ok(raw) = env::var(name) else {
        return false;
    };
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}
