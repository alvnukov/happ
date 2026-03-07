use happ::go_compat::template::Template;
use happ::gotemplates::{NativeFunctionResolverError, NativeRenderError, NativeRenderOptions};
use serde_json::{json, Number, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Clone)]
struct Case {
    src: &'static str,
    name: &'static str,
    left_delim: &'static str,
    right_delim: &'static str,
    option: Option<&'static str>,
    funcs: &'static [&'static str],
    data: serde_json::Value,
}

impl Case {
    fn new(src: &'static str, data: serde_json::Value) -> Self {
        Self {
            src,
            name: "main",
            left_delim: "{{",
            right_delim: "}}",
            option: None,
            funcs: &[],
            data,
        }
    }

    fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    fn delims(mut self, left_delim: &'static str, right_delim: &'static str) -> Self {
        self.left_delim = left_delim;
        self.right_delim = right_delim;
        self
    }

    fn option(mut self, option: &'static str) -> Self {
        self.option = Some(option);
        self
    }

    fn funcs(mut self, funcs: &'static [&'static str]) -> Self {
        self.funcs = funcs;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Parse,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorClass {
    Syntax,
    UndefinedFunction,
    MissingValue,
    TemplateNotFound,
    WrongArgCount,
    Index,
    Slice,
    Compare,
    FunctionCall,
    Other,
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
fn go_compat_template_exec_matrix_matches_go_text_template_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoTemplateRunner::new().expect("prepare go template runner");
    let cases = vec![
        Case::new(
            r#"{{define "main"}}hello {{.name}}{{end}}"#,
            json!({"name":"zol"}),
        ),
        Case::new(
            r#"{{define "x"}}{{.v}}{{end}}{{define "main"}}[{{template "x" .}}]{{end}}"#,
            json!({"v":"ok"}),
        ),
        Case::new(
            r#"{{define "main"}}{{$x := 1}}{{$x = 2}}{{$x}}{{end}}"#,
            json!({}),
        ),
        Case::new(
            r#"{{define "main"}}{{range $i, $v := .items}}{{$i}}={{$v}};{{else}}EMPTY{{end}}{{end}}"#,
            json!({"items":[10,20]}),
        ),
        Case::new(
            r#"{{define "main"}}{{if eq .a .b}}EQ{{else}}NE{{end}}{{end}}"#,
            json!({"a":1,"b":2}),
        ),
        Case::new(r#"{{define "main"}}{{and 0 .missing}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{or 1 .missing}}{{end}}"#, json!({})),
        Case::new(
            r#"{{define "main"}}{{len (slice .arr 1 3)}}{{end}}"#,
            json!({"arr":[1,2,3,4]}),
        ),
        Case::new(
            r#"{{define "main"}}{{printf "%[2]*.[1]*f" 2 6 12.0}}{{end}}"#,
            json!({}),
        ),
        Case::new(r#"{{define "main"}}{{.foo}}{{end}}"#, json!(null)),
        Case::new(r#"{{define "main"}}{{(.).foo}}{{end}}"#, json!(null)),
        Case::new(
            r#"{{define "\x61"}}A{{end}}{{define "main"}}{{template "\x61" .}}{{end}}"#,
            json!({}),
        ),
        Case::new(
            r#"<<define "main">><<if .ok>>OK<<else>>NO<<end>><<end>>"#,
            json!({"ok": true}),
        )
        .delims("<<", ">>"),
        Case::new(
            r#"{{define "main"}}{{ext .name}}{{end}}"#,
            json!({"name":"zol"}),
        )
        .funcs(&["ext"]),
        Case::new(r#"{{define "main"}}{{sum 1 2 3}}{{end}}"#, json!({})).funcs(&["sum"]),
        Case::new(
            r#"{{define "main"}}{{failif .v}}{{end}}"#,
            json!({"v":"boom"}),
        )
        .funcs(&["failif"]),
        Case::new(r#"{{define "main"}}{{call}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call ext}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | call ext}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call nil}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call (nil)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call (printf)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call "x"}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call ("x")}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call 1}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call (1)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call .fn}}{{end}}"#, json!({"fn":"ext"})),
        Case::new(r#"{{define "main"}}{{1 | call .fn}}{{end}}"#, json!({"fn":"ext"})),
        Case::new(r#"{{define "main"}}{{call (.fn)}}{{end}}"#, json!({"fn":"ext"})),
        Case::new(
            r#"{{define "main"}}{{call ((.fn))}}{{end}}"#,
            json!({"fn":"ext"}),
        ),
        Case::new(r#"{{define "main"}}{{call .missing}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | call .missing}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{call (.missing)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | call "x"}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main""#, json!({})),
        Case::new(r#"{{define "main"}}{{nope 1}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}x{{end}}"#, json!({})).name("missing"),
        Case::new(r#"{{define "main"}}{{.missing}}{{end}}"#, json!({})).option("missingkey=error"),
        Case::new(
            r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            json!({"m":{"a":1}}),
        )
        .option("missingkey=default"),
        Case::new(
            r#"{{define "main"}}{{.m.missing.y}}{{end}}"#,
            json!({"m":{"a":1}}),
        )
        .option("missingkey=zero"),
        Case::new(r#"{{define "main"}}{{len 1 2}}{{end}}"#, json!({})),
        Case::new(
            r#"{{define "main"}}{{index .arr 10}}{{end}}"#,
            json!({"arr":[1,2,3]}),
        ),
        Case::new(
            r#"{{define "main"}}{{slice .arr -1}}{{end}}"#,
            json!({"arr":[1,2,3]}),
        ),
        Case::new(r#"{{define "main"}}{{lt true 1}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | nil}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | (nil)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{1 | (printf)}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{"x" 1}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{nil 1}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{(printf) 2}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{$x := 1}}{{$x 2}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{$x := 1}}{{1 | $x}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{.x 2}}{{end}}"#, json!({"x": 7})),
        Case::new(r#"{{define "main"}}{{.x 2}}{{end}}"#, json!({})),
        Case::new(r#"{{define "main"}}{{.a.b 2}}{{end}}"#, json!({"a": {}})),
        Case::new(r#"{{define "main"}}{{.a.b 2}}{{end}}"#, json!({})),
        Case::new(
            r#"{{define "main"}}{{$m := .m}}{{$m.a 2}}{{end}}"#,
            json!({"m":{"a":1}}),
        ),
        Case::new(
            r#"{{define "main"}}{{$m := .m}}{{1 | $m.a}}{{end}}"#,
            json!({"m":{"a":1}}),
        ),
        Case::new(r#"{{define "main"}}{{$x := 1}}{{$x.y 2}}{{end}}"#, json!({})),
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
                    assert_eq!(out, go.out, "output mismatch for src={}", case.src);
                }
                RustOutcome::Err(stage, reason) => panic!(
                    "go succeeded but rust failed at {:?}: {}; src={}",
                    stage, reason, case.src
                ),
            }
            continue;
        }

        let (rust_stage, rust_reason) = match rust {
            RustOutcome::Ok(out) => panic!(
                "go failed but rust succeeded with out={out}; go_stage={}; go_err={}; src={}",
                go.stage, go.err, case.src
            ),
            RustOutcome::Err(stage, reason) => (stage, reason),
        };
        let go_stage = parse_go_stage(&go.stage);
        assert_eq!(
            rust_stage, go_stage,
            "stage mismatch for src={}; rust_reason={}; go_err={}",
            case.src, rust_reason, go.err
        );

        let rust_class = classify_error(go_stage, &rust_reason);
        let go_class = classify_error(go_stage, &go.err);
        assert_eq!(
            rust_class, go_class,
            "error class mismatch for src={}; rust_reason={}; go_err={}",
            case.src, rust_reason, go.err
        );
    }
}

fn run_rust_case(case: &Case) -> RustOutcome {
    let mut tpl = Template::new("root");
    tpl.delims(case.left_delim, case.right_delim);
    tpl.funcs(case.funcs.iter().copied());
    if let Some(spec) = case.option {
        if let Err(err) = tpl.option(spec) {
            return RustOutcome::Err(Stage::Parse, err.to_string());
        }
    }
    if let Err(err) = tpl.parse(case.src) {
        return RustOutcome::Err(Stage::Parse, format!("{}:{}", err.code, err.message));
    }
    let rendered = if case.funcs.is_empty() {
        tpl.execute_template(case.name, &case.data)
    } else {
        tpl.execute_template_with_resolver(
            case.name,
            &case.data,
            NativeRenderOptions::default(),
            Some(&rust_external_resolver),
        )
    };
    match rendered {
        Ok(out) => RustOutcome::Ok(out),
        Err(err) => RustOutcome::Err(Stage::Execute, classify_rust_exec_error(&err)),
    }
}

fn rust_external_resolver(
    name: &str,
    args: &[Option<Value>],
) -> Result<Option<Value>, NativeFunctionResolverError> {
    match name {
        "ext" => {
            let Some(value) = args.first().cloned().flatten() else {
                return Err(NativeFunctionResolverError::Failed {
                    reason: "missing argument".to_string(),
                });
            };
            Ok(Some(Value::String(format!(
                "{}-ok",
                sprint_like_go(&value)
            ))))
        }
        "sum" => {
            let mut sum: i64 = 0;
            for arg in args {
                let Some(v) = arg.as_ref() else {
                    return Err(NativeFunctionResolverError::Failed {
                        reason: "nil argument".to_string(),
                    });
                };
                let n = value_to_i64(v).ok_or_else(|| NativeFunctionResolverError::Failed {
                    reason: "non-integer argument".to_string(),
                })?;
                sum = sum.saturating_add(n);
            }
            Ok(Some(Value::Number(Number::from(sum))))
        }
        "failif" => {
            let Some(value) = args.first().cloned().flatten() else {
                return Err(NativeFunctionResolverError::Failed {
                    reason: "missing argument".to_string(),
                });
            };
            if sprint_like_go(&value) == "boom" {
                return Err(NativeFunctionResolverError::Failed {
                    reason: "boom".to_string(),
                });
            }
            Ok(Some(Value::String(sprint_like_go(&value))))
        }
        _ => Err(NativeFunctionResolverError::UnknownFunction),
    }
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| {
                n.as_f64().and_then(|f| {
                    if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                        Some(f as i64)
                    } else {
                        None
                    }
                })
            }),
        _ => None,
    }
}

fn sprint_like_go(v: &Value) -> String {
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn classify_rust_exec_error(err: &NativeRenderError) -> String {
    match err {
        NativeRenderError::TemplateNotFound { name } => format!("template not found: {name}"),
        NativeRenderError::Parse(e) => format!("parse: {}:{}", e.code, e.message),
        NativeRenderError::UnsupportedAction { reason, .. } => reason.clone(),
        NativeRenderError::MissingValue { path, .. } => format!("missing value: {path}"),
        NativeRenderError::TemplateRecursionLimit { name, depth } => {
            format!("template recursion limit: {name} depth={depth}")
        }
    }
}

fn classify_error(stage: Stage, reason: &str) -> ErrorClass {
    match stage {
        Stage::Parse => classify_parse_error(reason),
        Stage::Execute => classify_exec_error(reason),
    }
}

fn classify_parse_error(reason: &str) -> ErrorClass {
    if reason.contains("undefined_function")
        || reason.contains("function is not defined")
        || reason.contains("function \"")
    {
        return ErrorClass::UndefinedFunction;
    }
    ErrorClass::Syntax
}

fn classify_exec_error(reason: &str) -> ErrorClass {
    if reason.contains("missing value:") || reason.contains("map has no entry for key") {
        return ErrorClass::MissingValue;
    }
    if reason.contains("template not found:") || reason.contains("no template ") {
        return ErrorClass::TemplateNotFound;
    }
    if reason.contains("wrong number of args for") {
        return ErrorClass::WrongArgCount;
    }
    if reason.contains("error calling index:") {
        return ErrorClass::Index;
    }
    if reason.contains("error calling slice:") {
        return ErrorClass::Slice;
    }
    if reason.contains("error calling lt:")
        || reason.contains("error calling le:")
        || reason.contains("error calling gt:")
        || reason.contains("error calling ge:")
        || reason.contains("error calling eq:")
        || reason.contains("error calling ne:")
    {
        return ErrorClass::Compare;
    }
    if reason.contains("error calling ") {
        return ErrorClass::FunctionCall;
    }
    ErrorClass::Other
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
        let program = tmp.path().join("go_compat_template_exec_matrix.go");
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
                    "left_delim": c.left_delim,
                    "right_delim": c.right_delim,
                    "option": c.option.unwrap_or(""),
                    "funcs": c.funcs,
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
    "errors"
    "fmt"
    "os"
    t "text/template"
)

type inCase struct {
    Src string `json:"src"`
    Name string `json:"name"`
    LeftDelim string `json:"left_delim"`
    RightDelim string `json:"right_delim"`
    Option string `json:"option"`
    Funcs []string `json:"funcs"`
    Data any `json:"data"`
}

type outCase struct {
    Ok bool `json:"ok"`
    Out string `json:"out,omitempty"`
    Err string `json:"err,omitempty"`
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
        tpl := t.New("root").Delims(c.LeftDelim, c.RightDelim)
        if len(c.Funcs) > 0 {
            tpl = tpl.Funcs(buildFuncMap(c.Funcs))
        }
        if c.Option != "" {
            tpl = tpl.Option(c.Option)
        }
        parsed, err := tpl.Parse(c.Src)
        if err != nil {
            out = append(out, outCase{Ok: false, Err: err.Error(), Stage: "parse"})
            continue
        }

        var buf bytes.Buffer
        if err := parsed.ExecuteTemplate(&buf, c.Name, c.Data); err != nil {
            out = append(out, outCase{Ok: false, Err: err.Error(), Stage: "execute"})
            continue
        }

        out = append(out, outCase{Ok: true, Out: buf.String()})
    }

    data, err := json.Marshal(out)
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }
    os.Stdout.Write(data)
}

func buildFuncMap(names []string) t.FuncMap {
    out := make(t.FuncMap, len(names))
    for _, name := range names {
        switch name {
        case "ext":
            out[name] = func(v any) string {
                return fmt.Sprintf("%v-ok", v)
            }
        case "sum":
            out[name] = func(vals ...any) (int64, error) {
                var sum int64
                for _, raw := range vals {
                    n, ok := asInt64(raw)
                    if !ok {
                        return 0, errors.New("non-integer argument")
                    }
                    sum += n
                }
                return sum, nil
            }
        case "failif":
            out[name] = func(v any) (string, error) {
                s := fmt.Sprint(v)
                if s == "boom" {
                    return "", errors.New("boom")
                }
                return s, nil
            }
        }
    }
    return out
}

func asInt64(v any) (int64, bool) {
    switch n := v.(type) {
    case int:
        return int64(n), true
    case int8:
        return int64(n), true
    case int16:
        return int64(n), true
    case int32:
        return int64(n), true
    case int64:
        return n, true
    case uint:
        return int64(n), true
    case uint8:
        return int64(n), true
    case uint16:
        return int64(n), true
    case uint32:
        return int64(n), true
    case uint64:
        return int64(n), true
    case float64:
        i := int64(n)
        if float64(i) == n {
            return i, true
        }
        return 0, false
    default:
        return 0, false
    }
}
"#
}
