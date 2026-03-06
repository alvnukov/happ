use happ::gotemplates::{render_template_native, NativeRenderError};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn native_executor_matches_go_std_cmp_success_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoExecRunner::new().expect("prepare go executor runner");
    let data = json!({
        "arr": [1, 2],
        "s": "xy"
    });

    // Sourced from Go text/template/exec_test.go cmpTests subset.
    let cases = vec![
        "{{eq true true}}",
        "{{eq true false}}",
        "{{eq 1 1}}",
        "{{eq 1 2}}",
        "{{eq 3 4 5 6 3}}",
        "{{eq 3 4 5 6 7}}",
        "{{eq `xy` `xy`}}",
        "{{eq `xy` `xyz`}}",
        "{{ne 1 2}}",
        "{{lt 1 2}}",
        "{{le 1 1}}",
        "{{le `xyz` `xy`}}",
        "{{gt 2 1}}",
        "{{ge 1 1}}",
        "{{ge `xyz` `xy`}}",
        "{{eq nil nil}}",
        "{{eq (index `x` 0) 'x'}}",
        "{{eq (slice `日本` 1 2) (slice `日本` 1 2)}}",
        "{{lt (slice `ab` 0 1) (slice `ab` 1 2)}}",
        "{{eq (slice `日本` 1 2) `x`}}",
    ];
    let go_results = runner
        .render_batch(&cases, &data)
        .expect("go render should succeed for sourced cmp subset");
    assert_eq!(
        go_results.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_results.len(),
        cases.len()
    );

    for (idx, src) in cases.iter().enumerate() {
        let rust_out = render_template_native(src, &data).expect("rust render should succeed");
        let go = &go_results[idx];
        assert!(
            go.ok,
            "go should succeed for sourced cmp subset: {src}; err={}",
            go.err
        );
        let go_out = &go.out;
        assert_eq!(rust_out, *go_out, "rust/go output mismatch for: {src}");
    }
}

#[test]
fn native_executor_matches_go_std_cmp_failure_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoExecRunner::new().expect("prepare go executor runner");
    let data = json!({
        "arr": [1, 2],
        "s": "xy"
    });

    // Sourced from Go text/template/exec_test.go cmpTests subset.
    let failing_cases = vec![
        "{{eq 2 2.0}}",
        "{{lt true true}}",
        "{{eq `xy` 1}}",
        "{{eq .arr `xy`}}",
        "{{eq .arr .arr}}",
        "{{eq (slice `日本` 1 2) .arr}}",
    ];
    let go_results = runner
        .render_batch(&failing_cases, &data)
        .expect("go render batch should complete");
    assert_eq!(
        go_results.len(),
        failing_cases.len(),
        "go batch size mismatch: got={} want={}",
        go_results.len(),
        failing_cases.len()
    );

    for (idx, src) in failing_cases.iter().enumerate() {
        let rust = render_template_native(src, &data);
        let go = &go_results[idx];
        let rust_err = rust.expect_err("rust should fail for sourced cmp negative case");
        assert!(
            !go.ok,
            "go should fail for sourced cmp negative case: {src}; out={}",
            go.out
        );

        let rust_class = classify_cmp_error_rust(&rust_err);
        let go_class = classify_cmp_error_go(&go.err);
        assert_eq!(
            rust_class, go_class,
            "rust/go cmp error class mismatch for: {src}; rust={rust_err:?}; go={}",
            go.err
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpErrorClass {
    NonComparable,
    InvalidType,
    IncompatibleTypes,
    Other,
}

fn classify_cmp_reason(reason: &str) -> CmpErrorClass {
    if reason.contains("non-comparable type") || reason.contains("non-comparable types") {
        return CmpErrorClass::NonComparable;
    }
    if reason.contains("invalid type for comparison") {
        return CmpErrorClass::InvalidType;
    }
    if reason.contains("incompatible types for comparison") {
        return CmpErrorClass::IncompatibleTypes;
    }
    CmpErrorClass::Other
}

fn classify_cmp_error_rust(err: &NativeRenderError) -> CmpErrorClass {
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => classify_cmp_reason(reason),
        _ => CmpErrorClass::Other,
    }
}

fn classify_cmp_error_go(err: &str) -> CmpErrorClass {
    classify_cmp_reason(err)
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoExecRunner {
    _tmp: TempDir,
    program: PathBuf,
}

#[derive(Debug, serde::Deserialize)]
struct GoBatchResult {
    ok: bool,
    #[serde(default)]
    out: String,
    #[serde(default)]
    err: String,
}

impl GoExecRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("execcheck.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn render_batch(
        &self,
        templates: &[&str],
        data: &serde_json::Value,
    ) -> Result<Vec<GoBatchResult>, String> {
        let templates_json =
            serde_json::to_string(templates).map_err(|e| format!("serialize templates: {e}"))?;
        let encoded_templates = base64_encode(templates_json.as_bytes());
        let data_json = serde_json::to_string(data).map_err(|e| format!("serialize data: {e}"))?;
        let encoded_data = base64_encode(data_json.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_templates)
            .arg(encoded_data)
            .output()
            .map_err(|e| format!("go run failed to start: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "go render failed: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice::<Vec<GoBatchResult>>(&output.stdout)
            .map_err(|e| format!("decode go results: {e}"))
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0usize;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | input[i + 2] as u32;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }

    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }

    out
}

fn go_program_source() -> &'static str {
    r#"package main

import (
    "bytes"
    "encoding/base64"
    "encoding/json"
    "fmt"
    "os"
    "text/template"
)

func main() {
    if len(os.Args) != 3 {
        fmt.Print("need templates and data")
        os.Exit(3)
    }
    templatesBytes, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    dataBytes, err := base64.StdEncoding.DecodeString(os.Args[2])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }
    var data any
    if err := json.Unmarshal(dataBytes, &data); err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }

    var templates []string
    if err := json.Unmarshal(templatesBytes, &templates); err != nil {
        fmt.Print(err.Error())
        os.Exit(8)
    }

    type result struct {
        Ok bool `json:"ok"`
        Out string `json:"out,omitempty"`
        Err string `json:"err,omitempty"`
    }

    out := make([]result, 0, len(templates))
    for _, src := range templates {
        t, err := template.New("x").Parse(src)
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }
        var buf bytes.Buffer
        if err := t.Execute(&buf, data); err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }
        out = append(out, result{Ok: true, Out: buf.String()})
    }
    encoded, err := json.Marshal(out)
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(9)
    }
    fmt.Print(string(encoded))
}
"#
}
