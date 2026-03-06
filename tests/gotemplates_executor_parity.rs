use happ::gotemplates::render_template_native;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn native_executor_matches_go_for_supported_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoExecRunner::new().expect("prepare go executor runner");
    let data = json!({
        "a": { "b": "ok" },
        "flag": false,
        "alt": true,
        "user": {"name":"alice"},
        "items": ["x", "y"],
        "empty": [],
        "m": {"k":"v"},
        "m2": {"�":"hit"},
        "данные": {"ключ":"значение"},
        "s": "str",
        "n": 3,
        "t": true
    });

    let cases = vec![
        "hello",
        "A{{.a.b}}C",
        "{{.a.c}}",
        "x {{- .a.b -}} y",
        "{{\"abc\"}}",
        "{{`raw`}}",
        "{{3}}",
        "{{true}}",
        "{{.s}}:{{.n}}:{{.t}}",
        "{{if .flag}}yes{{else}}no{{end}}",
        "{{if .flag}}A{{else if .alt}}B{{else}}C{{end}}",
        "{{with .user}}{{.name}}{{else}}none{{end}}",
        "{{with .missing}}{{.name}}{{else with .user}}{{.name}}{{end}}",
        "{{range .items}}{{.}}{{else}}empty{{end}}",
        "{{range .empty}}{{.}}{{else}}empty{{end}}",
        "{{if .flag -}}A{{- else -}}B{{- end}}",
        "{{define \"t\"}}<{{.s}}>{{end}}{{template \"t\" .}}",
        "{{define \"name\"}}{{.name}}{{end}}{{template \"name\" .user}}",
        "pre{{define \"t\"}}X{{end}}post",
        "{{define \"inner\"}}[{{.name}}]{{end}}{{define \"outer\"}}{{template \"inner\" .}}{{end}}{{template \"outer\" .user}}",
        "{{print 1 2}}",
        "{{printf \"%s-%d\" .s 3}}",
        "{{3 | printf \"%d\"}}",
        "{{len .items}}",
        "{{index .items 1}}",
        "{{index .m \"k\"}}",
        "{{index .m \"z\"}}",
        "{{eq .missing nil}}",
        "{{or .missing \"x\"}}",
        "{{and .missing \"x\"}}",
        "{{printf \"%s\" (print .s .n)}}",
        "{{slice .items 1}}",
        "{{slice \"abcd\" 1 3}}",
        "{{slice \"日本\" 1 2}}",
        "{{eq (slice \"日本\" 1 2) (slice \"日本\" 1 2)}}",
        "{{lt (slice \"ab\" 0 1) (slice \"ab\" 1 2)}}",
        "{{index .m (slice \"kz\" 0 1)}}",
        "{{index .m (slice \"日本\" 1 2)}}",
        "{{index .m2 (slice \"日本\" 1 2)}}",
        "{{printf \"%T\" (slice \"日本\" 1 2)}}",
        "{{printf \"%x\" (slice \"日本\" 1 2)}}",
        "{{printf \"%q\" (slice \"日本\" 1 2)}}",
        "{{printf \"%#q\" (slice \"日本\" 1 2)}}",
        "{{urlquery (slice \"日本\" 1 2)}}",
        "{{urlquery \"a b\" \"+\"}}",
        "{{urlquery .missing}}",
        "{{block \"b\" .user}}{{.name}}{{end}}",
        "{{$x := .s}}{{$x}}",
        "{{$x := \"a\"}}{{$x = \"b\"}}{{$x}}",
        "{{with $x := .user}}{{$x.name}}{{end}}",
        "{{range $i, $v := .items}}{{$i}}={{$v}};{{end}}",
        "{{range $v := .empty}}x{{else}}{{$v}}{{end}}",
        "{{$имя := .данные.ключ}}{{$имя}}",
    ];
    let go_outputs = runner
        .render_batch(&cases, &data)
        .expect("go render should succeed for supported subset");
    assert_eq!(
        go_outputs.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_outputs.len(),
        cases.len()
    );

    for (idx, src) in cases.iter().enumerate() {
        let rust_out = render_template_native(src, &data).expect("rust render should succeed");
        let go_out = &go_outputs[idx];
        assert_eq!(rust_out, *go_out, "rust/go output mismatch for: {src}");
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

    fn render_batch(
        &self,
        templates: &[&str],
        data: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let cases_json =
            serde_json::to_string(templates).map_err(|e| format!("serialize templates: {e}"))?;
        let encoded_cases = base64_encode(cases_json.as_bytes());
        let data_json = serde_json::to_string(data).map_err(|e| format!("serialize data: {e}"))?;
        let encoded_data = base64_encode(data_json.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_cases)
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
        serde_json::from_slice::<Vec<String>>(&output.stdout)
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

    out := make([]string, 0, len(templates))
    for i, src := range templates {
        t, err := template.New("x").Parse(src)
        if err != nil {
            fmt.Printf("parse[%d]: %s", i, err.Error())
            os.Exit(2)
        }
        var buf bytes.Buffer
        if err := t.Execute(&buf, data); err != nil {
            fmt.Printf("exec[%d]: %s", i, err.Error())
            os.Exit(7)
        }
        out = append(out, buf.String())
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
