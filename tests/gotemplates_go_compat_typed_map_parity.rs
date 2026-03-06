use happ::gotemplates::go_compat::template::Template;
use serde_json::{Number, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Clone)]
struct Case {
    src: &'static str,
    kind: &'static str,
    option: Option<&'static str>,
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
fn go_compat_typed_map_semantics_match_go_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoTemplateRunner::new().expect("prepare go runner");
    let cases = vec![
        Case {
            src: r#"{{define "main"}}{{.m.missing}}|{{printf "%T" .m.missing}}{{end}}"#,
            kind: "map_string_int_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}|{{printf "%T" (index .m "missing")}}{{end}}"#,
            kind: "map_string_map_string_int_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{index .m "missing"}}|{{printf "%T" (index .m "missing")}}{{end}}"#,
            kind: "map_string_int_non_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{len .m}}{{end}}"#,
            kind: "map_string_int_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{range .m}}x{{else}}empty{{end}}{{end}}"#,
            kind: "map_string_int_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{index .m "missing"}}|{{printf "%T" (index .m "missing")}}{{end}}"#,
            kind: "map_string_any_non_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_non_nil",
            option: Some("missingkey=default"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_non_nil",
            option: Some("missingkey=error"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing}}{{end}}"#,
            kind: "map_string_any_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_nil",
            option: Some("missingkey=default"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_any_nil",
            option: Some("missingkey=error"),
        },
        Case {
            src: r#"{{define "main"}}{{index .m "missing"}}|{{printf "%T" (index .m "missing")}}{{end}}"#,
            kind: "map_string_any_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{printf "%#v|%T|%v" .m.missing .m.missing .m.missing}}{{end}}"#,
            kind: "map_string_bytes_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{len .m.missing}}{{end}}"#,
            kind: "map_string_bytes_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{printf "%#v|%T|%v" (index .m "missing") (index .m "missing") (index .m "missing")}}{{end}}"#,
            kind: "map_string_bytes_non_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing}}{{end}}"#,
            kind: "map_string_bytes_non_nil",
            option: Some("missingkey=default"),
        },
        Case {
            src: r#"{{define "main"}}{{printf "%#v|%T|%v" .m.missing .m.missing .m.missing}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{len .m.missing}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{printf "%#v|%T|%v" (index .m "missing") (index .m "missing") (index .m "missing")}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: None,
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=default"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=zero"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=default"),
        },
        Case {
            src: r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            kind: "map_string_slice_int_non_nil",
            option: Some("missingkey=error"),
        },
    ];

    let go_results = runner
        .execute_batch(&cases)
        .expect("go runner must return batch");
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
    let data = build_rust_data(case.kind);
    match tpl.execute_template("main", &data) {
        Ok(out) => RustOutcome::Ok(out),
        Err(err) => RustOutcome::Err(Stage::Execute, format!("{err:?}")),
    }
}

fn build_rust_data(kind: &str) -> Value {
    match kind {
        "map_string_int_non_nil" => {
            let mut entries = serde_json::Map::new();
            entries.insert("a".to_string(), Value::Number(Number::from(1)));
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("int", Some(entries)),
            );
            Value::Object(root)
        }
        "map_string_map_string_int_non_nil" => {
            let mut inner_entries = serde_json::Map::new();
            inner_entries.insert("y".to_string(), Value::Number(Number::from(2)));
            let mut outer_entries = serde_json::Map::new();
            outer_entries.insert(
                "x".to_string(),
                happ::gotemplates::encode_go_typed_map_value("int", Some(inner_entries)),
            );
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("map[string]int", Some(outer_entries)),
            );
            Value::Object(root)
        }
        "map_string_int_nil" => {
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("int", None),
            );
            Value::Object(root)
        }
        "map_string_any_non_nil" => {
            let mut nested = serde_json::Map::new();
            nested.insert("y".to_string(), Value::Number(Number::from(2)));
            let mut entries = serde_json::Map::new();
            entries.insert("x".to_string(), Value::Object(nested));
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("interface {}", Some(entries)),
            );
            Value::Object(root)
        }
        "map_string_any_nil" => {
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("interface {}", None),
            );
            Value::Object(root)
        }
        "map_string_bytes_non_nil" => {
            let mut entries = serde_json::Map::new();
            entries.insert(
                "x".to_string(),
                happ::gotemplates::encode_go_bytes_value(&[1, 2]),
            );
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("[]byte", Some(entries)),
            );
            Value::Object(root)
        }
        "map_string_slice_int_non_nil" => {
            let mut entries = serde_json::Map::new();
            entries.insert(
                "x".to_string(),
                happ::gotemplates::encode_go_typed_slice_value(
                    "int",
                    Some(vec![
                        Value::Number(Number::from(1)),
                        Value::Number(Number::from(2)),
                    ]),
                ),
            );
            let mut root = serde_json::Map::new();
            root.insert(
                "m".to_string(),
                happ::gotemplates::encode_go_typed_map_value("[]int", Some(entries)),
            );
            Value::Object(root)
        }
        other => panic!("unknown rust data kind: {other}"),
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
        let program = tmp.path().join("go_compat_typed_map.go");
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
                    "kind": c.kind,
                    "option": c.option.unwrap_or(""),
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

type inCase struct {
    Src    string `json:"src"`
    Kind   string `json:"kind"`
    Option string `json:"option"`
}

type outCase struct {
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

    payload, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }

    var cases []inCase
    if err := json.Unmarshal(payload, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]outCase, 0, len(cases))
    for _, c := range cases {
        tpl := template.New("root")
        if c.Option != "" {
            tpl = tpl.Option(c.Option)
        }
        tpl, err := tpl.Parse(c.Src)
        if err != nil {
            out = append(out, outCase{Ok: false, Err: err.Error(), Stage: "parse"})
            continue
        }

        data, err := buildData(c.Kind)
        if err != nil {
            out = append(out, outCase{Ok: false, Err: err.Error(), Stage: "execute"})
            continue
        }
        var buf bytes.Buffer
        if err := tpl.ExecuteTemplate(&buf, "main", data); err != nil {
            out = append(out, outCase{Ok: false, Err: err.Error(), Stage: "execute"})
            continue
        }
        out = append(out, outCase{Ok: true, Out: buf.String()})
    }

    enc := json.NewEncoder(os.Stdout)
    enc.SetEscapeHTML(false)
    if err := enc.Encode(out); err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }
}

func buildData(kind string) (any, error) {
    switch kind {
    case "map_string_int_non_nil":
        m := map[string]int{"a": 1}
        return map[string]any{"m": m}, nil
    case "map_string_map_string_int_non_nil":
        m := map[string]map[string]int{"x": map[string]int{"y": 2}}
        return map[string]any{"m": m}, nil
    case "map_string_int_nil":
        var m map[string]int
        return map[string]any{"m": m}, nil
    case "map_string_any_non_nil":
        m := map[string]any{"x": map[string]int{"y": 2}}
        return map[string]any{"m": m}, nil
    case "map_string_any_nil":
        var m map[string]any
        return map[string]any{"m": m}, nil
    case "map_string_bytes_non_nil":
        m := map[string][]byte{"x": []byte{1, 2}}
        return map[string]any{"m": m}, nil
    case "map_string_slice_int_non_nil":
        m := map[string][]int{"x": []int{1, 2}}
        return map[string]any{"m": m}, nil
    default:
        return nil, fmt.Errorf("unknown kind: %s", kind)
    }
}
"#
}
