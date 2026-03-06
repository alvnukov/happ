use happ::gotemplates::{render_template_native, NativeRenderError};
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
        "{{print 0b_101}}",
        "{{print 0b_1_0_1}}",
        "{{print 0377}}",
        "{{print 0o377}}",
        "{{print 0O377}}",
        "{{print 0o_377}}",
        "{{print 0o_3_7_7}}",
        "{{print 0x123}}",
        "{{print 0X123ABC}}",
        "{{print 0x_123}}",
        "{{print 0x1_23}}",
        "{{print 0_0_1_2_3.4}}",
        "{{print +0x1.ep+2}}",
        "{{print +0X1.EP+2}}",
        "{{print +0x_1.e_0p+0_2}}",
        "{{print '\\n'}}",
        "{{print '\\x41'}}",
        "{{print '\\u263A'}}",
        "{{print '\\U0001F600'}}",
        "{{println 1 2 3}}",
        "{{printf \"%04x\" 127}}",
        "{{printf \"%f\" 1.2}}",
        "{{printf \"%.2f\" 1.2}}",
        "{{printf \"%e\" 1.2}}",
        "{{printf \"%E\" 1.2}}",
        "{{printf \"%g\" 3.5}}",
        "{{printf \"%G\" 1234567.0}}",
        "{{printf \"%b\" 1.0}}",
        "{{printf \"%.4b\" -1.0}}",
        "{{printf \"%+.3x\" 1.0}}",
        "{{printf \"%#.0x\" 123.0}}",
        "{{printf \"%#.4e\" 1.0}}",
        "{{printf \"%#g\" 1230000.0}}",
        "{{printf `%T` 0xef}}",
        "{{printf \"%o\" 9}}",
        "{{printf \"%b\" 9}}",
        "{{printf \"%04x\" -1}}",
        "{{printf \"%d\" \"7\"}}",
        "{{(1)}}",
        "{{\"aaa\"|printf}}",
        "{{print nil}}",
        "{{html \"<tag attr='x'>&\\\"\"}}",
        "{{html nil}}",
        "{{html .Empty0}}",
        "{{printf \"<script>alert(\\\"XSS\\\");</script>\" | html}}",
        "{{js \"<tag>&'\\\"=\\n\"}}",
        "{{js \"It'd be nice.\"}}",
        "{{js \"<html>\"}}",
        "{{urlquery \"http://www.example.org/\"}}",
        "{{urlquery (slice \"日本\" 1 2)}}",
        "{{urlquery .Empty0}}",
        "{{urlquery .Unknown}}",
        "{{index .SI 0}}",
        "{{index .MSI `one`}}",
        "{{index .MSI `XXX`}}",
        "{{index .MRep (slice \"日本\" 1 2)}}",
        "{{slice .SI}}",
        "{{slice .SI 1}}",
        "{{slice .SI 1 2}}",
        "{{slice .S 1 2}}",
        "{{len .SI}}",
        "{{len .MSI}}",
        "{{$x := 2}}{{$x = 3}}{{$x}}",
        "{{range $x, $y := .SI}}<{{$x}}={{$y}}>{{end}}",
        "{{range .MSI}}-{{.}}-{{else}}EMPTY{{end}}",
        "{{$имя := .данные.ключ}}{{$имя}}",
        "{{if eq 1 3}}{{else if eq 3 3}}3{{end}}",
        "{{if false}}FALSE{{else if true}}TRUE{{end}}",
        "{{not true}} {{not false}}",
        "{{and false 0}} {{and 1 0}} {{and 0 true}} {{and 1 1}}",
        "{{and 1}}",
        "{{or 0 0}} {{or 1 0}} {{or 0 true}} {{or 1 1}}",
        "{{or 0}}",
        "{{not 1}}",
        "{{and 1 .Unknown}}",
        "{{or 0 .Unknown}}",
        "{{.Unknown.x 2}}",
        "{{if true | not | and 1}}TRUE{{else}}FALSE{{end}}",
        "{{$i := 0}}{{$x := 0}}{{range $i = .AI}}{{end}}{{$i}}",
        "{{$k := 0}}{{$v := 0}}{{range $k, $v = .AI}}{{$k}}={{$v}} {{end}}",
        "{{or 0 1 (index nil 0)}}",
        "{{and 1 0 (index nil 0)}}",
    ];
    let go_results = runner
        .render_batch(&cases, &data)
        .expect("go render should succeed for sourced subset");
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
            "go should succeed for sourced subset: {src}; err={}",
            go.err
        );
        let go_out = &go.out;
        assert_eq!(rust_out, *go_out, "rust/go output mismatch for: {src}");
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
        "{{print 1__2}}",
        "{{print 12_}}",
        "{{print 0x1._p2}}",
        "{{print 0x1.p_2}}",
        "{{true|printf}}",
        "{{1|printf}}",
        "{{1.1|printf}}",
        "{{print '\\400'}}",
        "{{print '\\777'}}",
        "{{nil}}",
        "{{if nil}}TRUE{{end}}",
        "{{with nil}}TRUE{{end}}",
        "{{range nil}}x{{end}}",
        "{{(nil)}}",
        "{{print (nil)}}",
        "{{1 | nil}}",
        "{{1 | (nil)}}",
        "{{1 | \"x\"}}",
        "{{1 | (\"x\")}}",
        "{{1 | (printf)}}",
        "{{nil 1}}",
        "{{1 2}}",
        "{{(1) 2}}",
        "{{\"x\" 1}}",
        "{{(printf) 2}}",
        "{{$x := 1}}{{$x 2}}",
        "{{$x := 1}}{{1 | $x}}",
        "{{.MSI.one 2}}",
        "{{.MSI.missing 2}}",
        "{{$m := .MSI}}{{$m.one 2}}",
        "{{$m := .MSI}}{{$m.missing 2}}",
        "{{$m := .MSI}}{{1 | $m.one}}",
        "{{$x := 1}}{{$x.y 2}}",
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
        "{{lt true false}}",
        "{{lt true 1}}",
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
        "{{call}}",
        "{{call nil}}",
        "{{call \"x\"}}",
        "{{call (\"x\")}}",
        "{{call 1}}",
        "{{call (1)}}",
        "{{call .Unknown}}",
        "{{call (.Unknown)}}",
        "{{1 | call .Unknown}}",
        "{{1 | call \"x\"}}",
        "{{len 3}}",
        "{{range 3}}{{.}}{{end}}",
        "{{range \"abc\"}}{{.}}{{end}}",
        "{{range 1.5}}{{.}}{{end}}",
        "{{break}}",
        "{{continue}}",
        "{{or 0 0 (index nil 0)}}",
        "{{and 1 1 (index nil 0)}}",
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
        let rust_err = match rust {
            Ok(out) => panic!("rust should fail for sourced negative case: {src}; out={out}"),
            Err(err) => err,
        };
        assert!(
            !go.ok,
            "go should fail for sourced negative case: {src}; out={}",
            go.out
        );

        let rust_class = classify_exec_error_rust(&rust_err);
        let go_class = classify_exec_error_go(&go.err);
        assert_eq!(
            rust_class, go_class,
            "rust/go exec error class mismatch for: {src}; rust={rust_err:?}; go={}",
            go.err
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecErrorClass {
    Parse,
    WrongArgCount,
    Index,
    Slice,
    Len,
    Range,
    Compare,
    Other,
}

fn classify_exec_reason(reason: &str) -> ExecErrorClass {
    if reason.contains("illegal number syntax")
        || reason.contains("bad number syntax")
        || reason.contains("unterminated quoted string")
        || reason.contains("unterminated character constant")
        || reason.contains("unexpected ")
        || reason.contains("missing value for ")
        || reason.contains("non executable command in pipeline stage")
    {
        return ExecErrorClass::Parse;
    }
    if reason.contains("wrong number of args") {
        return ExecErrorClass::WrongArgCount;
    }
    if reason.contains("error calling index:") {
        return ExecErrorClass::Index;
    }
    if reason.contains("error calling slice:") {
        return ExecErrorClass::Slice;
    }
    if reason.contains("error calling len:") {
        return ExecErrorClass::Len;
    }
    if reason.contains("range can't iterate over") {
        return ExecErrorClass::Range;
    }
    if reason.contains("error calling eq:")
        || reason.contains("error calling ne:")
        || reason.contains("error calling lt:")
        || reason.contains("error calling le:")
        || reason.contains("error calling gt:")
        || reason.contains("error calling ge:")
    {
        return ExecErrorClass::Compare;
    }
    ExecErrorClass::Other
}

fn classify_exec_error_rust(err: &NativeRenderError) -> ExecErrorClass {
    match err {
        NativeRenderError::Parse(_) => ExecErrorClass::Parse,
        NativeRenderError::UnsupportedAction { reason, .. } => classify_exec_reason(reason),
        NativeRenderError::MissingValue { .. }
        | NativeRenderError::TemplateNotFound { .. }
        | NativeRenderError::TemplateRecursionLimit { .. } => ExecErrorClass::Other,
    }
}

fn classify_exec_error_go(err: &str) -> ExecErrorClass {
    if err.contains(": parse:")
        || err.contains("bad number syntax")
        || err.contains("illegal number syntax")
        || err.contains("unterminated quoted string")
        || err.contains("unterminated character constant")
        || err.contains("unclosed action")
        || err.contains("unexpected ")
        || err.contains("missing value for ")
    {
        return ExecErrorClass::Parse;
    }
    if err.contains("{{break}} outside {{range}}") || err.contains("{{continue}} outside {{range}}")
    {
        return ExecErrorClass::Parse;
    }
    classify_exec_reason(err)
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
        "MRep": { "�": "hit" },
        "данные": { "ключ": "значение" },
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
