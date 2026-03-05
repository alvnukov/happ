use happ::gotemplates::render_template_native;
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
        "{{eq `xy` `xy`}}",
        "{{eq `xy` `xyz`}}",
        "{{ne 1 2}}",
        "{{lt 1 2}}",
        "{{le 1 1}}",
        "{{gt 2 1}}",
        "{{ge 1 1}}",
        "{{eq nil nil}}",
        "{{eq (index `x` 0) 'x'}}",
    ];

    for src in cases {
        let rust_out = render_template_native(src, &data).expect("rust render should succeed");
        let go_out = runner
            .render(src, &data)
            .expect("go render should succeed for sourced cmp subset");
        assert_eq!(rust_out, go_out, "rust/go output mismatch for: {src}");
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
        "{{eq .arr .arr}}",
    ];

    for src in failing_cases {
        let rust = render_template_native(src, &data);
        let go = runner.render(src, &data);
        assert!(
            rust.is_err(),
            "rust should fail for sourced cmp negative case: {src}"
        );
        assert!(
            go.is_err(),
            "go should fail for sourced cmp negative case: {src}"
        );
    }
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

impl GoExecRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("execcheck.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn render(&self, src: &str, data: &serde_json::Value) -> Result<String, String> {
        let encoded_src = base64_encode(src.as_bytes());
        let data_json = serde_json::to_string(data).map_err(|e| format!("serialize data: {e}"))?;
        let encoded_data = base64_encode(data_json.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_src)
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
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        fmt.Print("need template and data")
        os.Exit(3)
    }
    srcBytes, err := base64.StdEncoding.DecodeString(os.Args[1])
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

    t, err := template.New("x").Parse(string(srcBytes))
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(2)
    }
    var buf bytes.Buffer
    if err := t.Execute(&buf, data); err != nil {
        fmt.Print(err.Error())
        os.Exit(7)
    }
    fmt.Print(buf.String())
}
"#
}
