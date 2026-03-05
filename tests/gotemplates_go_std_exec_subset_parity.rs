use happ::gotemplates::render_template_native;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn native_executor_matches_go_std_exec_success_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoExecRunner::new().expect("prepare go executor runner");
    let data = source_like_data();

    // Cases are copied from Go stdlib text/template/exec_test.go (execTests subset).
    let cases = vec![
        "{{print \"hello, print\"}}",
        "{{print 1 2 3}}",
        "{{index 1}}",
        "{{print 12_34}}",
        "{{print 0b101}}",
        "{{print 0B101}}",
        "{{print 0377}}",
        "{{print 0o377}}",
        "{{print 0O377}}",
        "{{print 0x123}}",
        "{{print 0X123ABC}}",
        "{{print +0x1.ep+2}}",
        "{{print +0X1.EP+2}}",
        "{{print '\\n'}}",
        "{{print '\\x41'}}",
        "{{print '\\u263A'}}",
        "{{print '\\U0001F600'}}",
        "{{println 1 2 3}}",
        "{{printf \"%04x\" 127}}",
        "{{printf \"%d\" \"7\"}}",
        "{{html \"<tag attr='x'>&\\\"\"}}",
        "{{js \"<tag>&'\\\"=\\n\"}}",
        "{{urlquery \"http://www.example.org/\"}}",
        "{{index .SI 0}}",
        "{{index .MSI `one`}}",
        "{{index .MSI `XXX`}}",
        "{{slice .SI}}",
        "{{slice .SI 1}}",
        "{{slice .SI 1 2}}",
        "{{slice .S 1 2}}",
        "{{len .SI}}",
        "{{len .MSI}}",
        "{{$x := 2}}{{$x = 3}}{{$x}}",
        "{{range $x, $y := .SI}}<{{$x}}={{$y}}>{{end}}",
        "{{range 3}}{{.}}{{end}}",
        "{{range $i, $v := 3}}{{$i}}={{$v}};{{end}}",
        "{{range 4}}{{if eq . 2}}{{break}}{{end}}{{.}}{{end}}",
        "{{range 4}}{{if eq . 2}}{{continue}}{{end}}{{.}}{{end}}",
        "{{range .MSI}}-{{.}}-{{else}}EMPTY{{end}}",
        "{{if eq 1 3}}{{else if eq 3 3}}3{{end}}",
        "{{not true}} {{not false}}",
        "{{and false 0}} {{and 1 0}} {{and 0 true}} {{and 1 1}}",
        "{{and 1}}",
        "{{or 0 0}} {{or 1 0}} {{or 0 true}} {{or 1 1}}",
        "{{or 0}}",
        "{{not 1}}",
        "{{and 1 .Unknown}}",
        "{{or 0 .Unknown}}",
        "{{if true | not | and 1}}TRUE{{else}}FALSE{{end}}",
        "{{$i := 0}}{{$x := 0}}{{range $i = .AI}}{{end}}{{$i}}",
        "{{$k := 0}}{{$v := 0}}{{range $k, $v = .AI}}{{$k}}={{$v}} {{end}}",
        "{{or 0 1 (index nil 0)}}",
        "{{and 1 0 (index nil 0)}}",
    ];

    for src in cases {
        let rust_out = render_template_native(src, &data).expect("rust render should succeed");
        let go_out = runner
            .render(src, &data)
            .expect("go render should succeed for sourced subset");
        assert_eq!(rust_out, go_out, "rust/go output mismatch for: {src}");
    }
}

#[test]
fn native_executor_matches_go_std_exec_failure_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoExecRunner::new().expect("prepare go executor runner");
    let data = source_like_data();

    // Cases are copied from Go stdlib text/template/exec_test.go (execTests subset, failing).
    let failing_cases = vec![
        "{{index .SI 10}}",
        "{{slice .SI -1}}",
        "{{slice .S 1 2 2}}",
        "{{and}}",
        "{{or}}",
        "{{not}}",
        "{{not 1 2}}",
        "{{eq}}",
        "{{eq 1}}",
        "{{ne}}",
        "{{ne 1}}",
        "{{ne 1 2 3}}",
        "{{lt}}",
        "{{lt 1}}",
        "{{le 1}}",
        "{{gt 1}}",
        "{{ge 1}}",
        "{{len}}",
        "{{len 1 2}}",
        "{{index}}",
        "{{index .SI \"1\"}}",
        "{{index .MSI 1}}",
        "{{slice}}",
        "{{printf}}",
        "{{len 3}}",
        "{{range 1.5}}{{.}}{{end}}",
        "{{break}}",
        "{{continue}}",
        "{{or 0 0 (index nil 0)}}",
        "{{and 1 1 (index nil 0)}}",
    ];

    for src in failing_cases {
        let rust = render_template_native(src, &data);
        let go = runner.render(src, &data);
        assert!(
            rust.is_err(),
            "rust should fail for sourced negative case: {src}"
        );
        assert!(
            go.is_err(),
            "go should fail for sourced negative case: {src}"
        );
    }
}

fn source_like_data() -> serde_json::Value {
    json!({
        "I": 17,
        "U16": 16,
        "X": "x",
        "S": "xyz",
        "U": { "V": "v" },
        "SI": [3, 4, 5],
        "AI": [3, 4, 5],
        "MSI": { "one": 1, "two": 2, "three": 3 },
        "MSIone": { "one": 1 },
        "MSIEmpty": {},
        "Empty0": null,
        "Empty3": [7, 8]
    })
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
