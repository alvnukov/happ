use happ::go_compat::template::Template;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, serde::Deserialize)]
struct GoCaseResult {
    ok: bool,
    #[serde(default)]
    defined_templates: String,
    #[serde(default)]
    err: String,
}

#[test]
fn go_compat_defined_templates_matches_go() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoRunner::new().expect("prepare go runner");
    let cases = vec![
        r#"{{define "a"}}A{{end}}{{define "b"}}B{{end}}"#,
        r#"plain text"#,
        r#"{{define "main"}}M{{end}}{{define "sub"}}S{{end}}"#,
        r#"{{define "x"}}X{{end}}{{template "x" .}}"#,
        r#"{{define "\x61"}}A{{end}}"#,
    ];

    let go = runner.run_batch(&cases).expect("go run batch");
    assert_eq!(go.len(), cases.len());

    for (idx, src) in cases.iter().enumerate() {
        let mut tpl = Template::new("main");
        tpl.parse(src).expect("rust parse must succeed");
        let rust_defined = tpl.defined_templates();
        let rust_names = parse_defined_names(&rust_defined);

        assert!(
            go[idx].ok,
            "go parse failed for src={src}; err={}",
            go[idx].err
        );
        assert_eq!(
            rust_names,
            parse_defined_names(&go[idx].defined_templates),
            "defined template names mismatch for src={src}; rust={rust_defined}; go={}",
            go[idx].defined_templates
        );
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_defined_templates_parity.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn run_batch(&self, cases: &[&str]) -> Result<Vec<GoCaseResult>, String> {
        let payload = serde_json::to_string(cases).map_err(|e| format!("serialize: {e}"))?;
        let encoded = base64_encode(payload.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded)
            .output()
            .map_err(|e| format!("go run failed to start: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "go run failed: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        serde_json::from_slice::<Vec<GoCaseResult>>(&output.stdout)
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
    "encoding/base64"
    "encoding/json"
    "fmt"
    "os"
    t "text/template"
)

type result struct {
    Ok               bool   `json:"ok"`
    DefinedTemplates string `json:"defined_templates,omitempty"`
    Err              string `json:"err,omitempty"`
}

func main() {
    if len(os.Args) != 2 {
        fmt.Print("need encoded payload")
        os.Exit(3)
    }

    payload, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }

    var cases []string
    if err := json.Unmarshal(payload, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]result, 0, len(cases))
    for _, src := range cases {
        tpl := t.New("main")
        _, err := tpl.Parse(src)
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }
        out = append(out, result{Ok: true, DefinedTemplates: tpl.DefinedTemplates()})
    }

    enc := json.NewEncoder(os.Stdout)
    enc.SetEscapeHTML(false)
    if err := enc.Encode(out); err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }
}
"#
}

fn parse_defined_names(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        if i <= bytes.len() {
            out.insert(s[start..i].replace("\\\"", "\"").replace("\\\\", "\\"));
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    out
}
