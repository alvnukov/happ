use happ::go_compat::template::Template;
use happ::gotemplates::NativeRenderError;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Clone)]
struct Case {
    src: &'static str,
    name: &'static str,
    option: Option<&'static str>,
    data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Parse,
    Execute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RustOutcome {
    Ok(String),
    Err(Stage, String),
}

#[derive(Debug, serde::Deserialize)]
struct GoBatchResult {
    ok: bool,
    #[serde(default)]
    out: String,
    #[serde(default)]
    err: String,
    #[serde(default)]
    stage: String,
}

#[test]
fn go_compat_template_api_matches_go_text_template_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoTemplateRunner::new().expect("prepare go template runner");
    let cases = vec![
        Case {
            src: r#"{{define "main"}}hello {{.name}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"name":"zol"}),
        },
        Case {
            src: r#"{{define "x"}}{{.v}}{{end}}{{define "main"}}[{{template "x" .}}]{{end}}"#,
            name: "main",
            option: None,
            data: json!({"v":"ok"}),
        },
        Case {
            src: r#"{{define "main"}}{{$x := 1}}{{$x = 2}}{{$x}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.missing}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.missing}}{{end}}"#,
            name: "main",
            option: Some("missingkey=error"),
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            name: "main",
            option: Some("missingkey=default"),
            data: json!({"m":{"a":1}}),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            name: "main",
            option: Some("missingkey=zero"),
            data: json!({"m":{"a":1}}),
        },
        Case {
            src: r#"{{define "\x61"}}A{{end}}{{define "main"}}{{template "\x61" .}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "\q"}}bad{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "\07"}}bad{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call ext}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | call ext}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call nil}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call (nil)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call (printf)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call "x"}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call ("x")}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call 1}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call (1)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call .fn}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"fn":"ext"}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | call .fn}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"fn":"ext"}),
        },
        Case {
            src: r#"{{define "main"}}{{call (.fn)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"fn":"ext"}),
        },
        Case {
            src: r#"{{define "main"}}{{call ((.fn))}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"fn":"ext"}),
        },
        Case {
            src: r#"{{define "main"}}{{call .missing}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | call .missing}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{call (.missing)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | call "x"}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | nil}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | (nil)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{1 | (printf)}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{nil 1}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{(printf) 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{"x" 1}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{$x := 1}}{{1 | $x}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.x 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"x": 7}),
        },
        Case {
            src: r#"{{define "main"}}{{.x 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.a.b 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"a": {}}),
        },
        Case {
            src: r#"{{define "main"}}{{.a.b 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{$m := .m}}{{$m.a 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"m":{"a":1}}),
        },
        Case {
            src: r#"{{define "main"}}{{$m := .m}}{{1 | $m.a}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({"m":{"a":1}}),
        },
        Case {
            src: r#"{{define "main"}}{{$x := 1}}{{$x.y 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}{{.x 2}}{{end}}"#,
            name: "main",
            option: None,
            data: json!(null),
        },
        Case {
            src: r#"{{define "main""#,
            name: "main",
            option: None,
            data: json!({}),
        },
        Case {
            src: r#"{{define "main"}}x{{end}}"#,
            name: "missing",
            option: None,
            data: json!({}),
        },
    ];

    let go_results = runner
        .execute_batch(&cases)
        .expect("go template runner must return results");
    assert_eq!(
        go_results.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_results.len(),
        cases.len()
    );

    for (idx, case) in cases.iter().enumerate() {
        let rust = run_rust_case(case);
        let go = &go_results[idx];
        if go.ok {
            match rust {
                RustOutcome::Ok(out) => {
                    assert_eq!(out, go.out, "output mismatch for src={}", case.src)
                }
                RustOutcome::Err(stage, reason) => panic!(
                    "go succeeded but rust failed at {:?}: {}; src={}",
                    stage, reason, case.src
                ),
            }
            continue;
        }

        let rust_err = match rust {
            RustOutcome::Ok(out) => panic!(
                "go failed but rust succeeded with out={out}; go_stage={}; go_err={}; src={}",
                go.stage, go.err, case.src
            ),
            RustOutcome::Err(stage, reason) => (stage, reason),
        };
        let go_stage = parse_go_stage(&go.stage);
        assert_eq!(
            rust_err.0, go_stage,
            "stage mismatch for src={}; rust_reason={}; go_err={}",
            case.src, rust_err.1, go.err
        );
    }
}

fn run_rust_case(case: &Case) -> RustOutcome {
    let mut tpl = Template::new("root");
    if let Some(opt) = case.option {
        if let Err(err) = tpl.option(opt) {
            return RustOutcome::Err(Stage::Parse, err.to_string());
        }
    }
    if let Err(err) = tpl.parse(case.src) {
        return RustOutcome::Err(Stage::Parse, err.code.to_string());
    }
    match tpl.execute_template(case.name, &case.data) {
        Ok(out) => RustOutcome::Ok(out),
        Err(err) => RustOutcome::Err(Stage::Execute, classify_rust_exec_error(&err)),
    }
}

fn classify_rust_exec_error(err: &NativeRenderError) -> String {
    match err {
        NativeRenderError::TemplateNotFound { name } => format!("template not found: {name}"),
        NativeRenderError::Parse(e) => format!("parse: {}", e.code),
        NativeRenderError::UnsupportedAction { reason, .. } => reason.clone(),
        NativeRenderError::MissingValue { path, .. } => format!("missing value: {path}"),
        NativeRenderError::TemplateRecursionLimit { name, depth } => {
            format!("template recursion limit: {name} depth={depth}")
        }
    }
}

fn parse_go_stage(s: &str) -> Stage {
    match s {
        "parse" => Stage::Parse,
        "execute" => Stage::Execute,
        other => panic!("unknown go stage: {other}"),
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoTemplateRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoTemplateRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_compat_template.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn execute_batch(&self, cases: &[Case]) -> Result<Vec<GoBatchResult>, String> {
        let payload: Vec<serde_json::Value> = cases
            .iter()
            .map(|c| {
                serde_json::json!({
                    "src": c.src,
                    "name": c.name,
                    "option": c.option.unwrap_or(""),
                    "data": c.data,
                })
            })
            .collect();
        let json_payload =
            serde_json::to_string(&payload).map_err(|e| format!("serialize payload: {e}"))?;
        let encoded = base64_encode(json_payload.as_bytes());

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

type caseInput struct {
    Src    string `json:"src"`
    Name   string `json:"name"`
    Option string `json:"option"`
    Data   any    `json:"data"`
}

type result struct {
    Ok    bool   `json:"ok"`
    Out   string `json:"out,omitempty"`
    Err   string `json:"err,omitempty"`
    Stage string `json:"stage,omitempty"`
}

func main() {
    if len(os.Args) != 2 {
        fmt.Print("need encoded payload")
        os.Exit(3)
    }
    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    var cases []caseInput
    if err := json.Unmarshal(data, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]result, 0, len(cases))
    for _, c := range cases {
        t := template.New("root")
        if c.Option != "" {
            t = t.Option(c.Option)
        }
        parsed, err := t.Parse(c.Src)
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error(), Stage: "parse"})
            continue
        }
        var buf bytes.Buffer
        if err := parsed.ExecuteTemplate(&buf, c.Name, c.Data); err != nil {
            out = append(out, result{Ok: false, Err: err.Error(), Stage: "execute"})
            continue
        }
        out = append(out, result{Ok: true, Out: buf.String()})
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
