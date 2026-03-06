use happ::gotemplates::parse_template_tokens_strict;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn gotemplates_error_matrix_matches_go_parse_package() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseRunner::new().expect("prepare go parser runner");
    let cases = vec![
        (r#"{{}} "#, Some("missing_value_for_context")),
        (r#"{{end}}"#, Some("unexpected_end_action")),
        (r#"{{else}}"#, Some("unexpected_else_action")),
        (r#"hello{{range .x}}"#, Some("unexpected_eof")),
        (r#"{{$x}}"#, Some("undefined_variable")),
        (
            r#"{{with $x := 4}}{{end}}{{$x}}"#,
            Some("undefined_variable"),
        ),
        (r#"{{template $v}}"#, Some("unexpected_token")),
        (r#"{{with $x.Y := 4}}{{end}}"#, Some("undefined_variable")),
        (r#"{{printf 3, 4}}"#, Some("unexpected_token")),
        (
            r#"{{with $v, $u := 3}}{{end}}"#,
            Some("too_many_declarations"),
        ),
        (
            r#"{{range $u, $v, $w := 3}}{{end}}"#,
            Some("too_many_declarations"),
        ),
        (
            r#"{{printf (printf .).}}"#,
            Some("unexpected_dot_in_operand"),
        ),
        (r#"{{printf 3`x`}}"#, Some("unexpected_token")),
        (r#"{{printf `x`.}}"#, Some("unexpected_dot_in_operand")),
        (
            r#"{{if .X}}a{{else if .Y}}b{{end}}{{end}}"#,
            Some("unexpected_end_action"),
        ),
        (
            r#"{{range .}}{{end}} {{break}}"#,
            Some("break_outside_range"),
        ),
        (
            r#"{{range .}}{{else}}{{break}}{{end}}"#,
            Some("break_outside_range"),
        ),
        (
            r#"{{12|false}}"#,
            Some("non_executable_command_in_pipeline"),
        ),
        (
            r#"{{1 | print | nil}}"#,
            Some("non_executable_command_in_pipeline"),
        ),
        (r#"{{unknownFn}}"#, Some("undefined_function")),
        (r#"{{1 | unknownFn}}"#, Some("undefined_function")),
        (r#"{{(unknownFn)}}"#, Some("undefined_function")),
        (r#"{{printf "%d" ( ) }}"#, Some("missing_value_for_context")),
        (r#"{{range $k,}}{{end}}"#, Some("missing_value_for_context")),
        (
            r#"{{range $k, $v := }}{{end}}"#,
            Some("missing_value_for_context"),
        ),
        (r#"{{range $k, .}}{{end}}"#, Some("unexpected_token")),
        (r#"{{range $k, 123 := .}}{{end}}"#, Some("unexpected_token")),
        (
            r#"{{define "a"}}a{{end}}{{define "a"}}b{{end}}"#,
            Some("multiple_template_definition"),
        ),
        (r#"{{define "a"}}{{end}}{{define "a"}}b{{end}}"#, None),
        (r#"{{define "a"}}a{{end}}{{define "a"}}{{end}}"#, None),
    ];

    let templates: Vec<&str> = cases.iter().map(|(src, _)| *src).collect();
    let go_codes = runner
        .parse_error_codes(&templates)
        .expect("go parser should return mapped codes");
    assert_eq!(
        go_codes.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_codes.len(),
        cases.len()
    );

    for (idx, (src, expected_code)) in cases.iter().enumerate() {
        let rust_code = parse_template_tokens_strict(src)
            .err()
            .map(|e| e.code.to_string());
        let go_code = go_codes[idx].clone();

        assert_eq!(
            go_code,
            expected_code.map(ToString::to_string),
            "go oracle mismatch for case: {src}"
        );
        assert_eq!(rust_code, go_code, "rust/go mismatch for case: {src}");
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoParseRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoParseRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("parsematrix.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn parse_error_codes(&self, cases: &[&str]) -> Result<Vec<Option<String>>, String> {
        let cases_json =
            serde_json::to_string(cases).map_err(|e| format!("serialize cases: {e}"))?;
        let encoded = base64_encode(cases_json.as_bytes());
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

        let raw = serde_json::from_slice::<Vec<String>>(&output.stdout)
            .map_err(|e| format!("decode go results: {e}"))?;
        let mut out = Vec::with_capacity(raw.len());
        for (idx, msg) in raw.into_iter().enumerate() {
            if msg.is_empty() {
                out.push(None);
                continue;
            }
            match map_go_error_to_code(&msg) {
                Some(code) => out.push(Some(code.to_string())),
                None => return Err(format!("unmapped go parse error at index {idx}: {msg}")),
            }
        }
        Ok(out)
    }
}

fn map_go_error_to_code(msg: &str) -> Option<&'static str> {
    if msg.contains("unexpected EOF") {
        return Some("unexpected_eof");
    }
    if msg.contains("unexpected {{else}}") {
        return Some("unexpected_else_action");
    }
    if msg.contains("unexpected {{end}}") {
        return Some("unexpected_end_action");
    }
    if msg.contains("undefined variable") {
        return Some("undefined_variable");
    }
    if msg.contains("too many declarations") {
        return Some("too_many_declarations");
    }
    if msg.contains("multiple definition of template") {
        return Some("multiple_template_definition");
    }
    if msg.contains("range can only initialize variables") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in command") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in operand") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in template clause") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in define clause") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in block clause") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \":=\" in operand") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected <.> in operand") {
        return Some("unexpected_dot_in_operand");
    }
    if msg.contains("unexpected \".\" in operand") {
        return Some("unexpected_dot_in_operand");
    }
    if msg.contains("bad number syntax") || msg.contains("illegal number syntax") {
        return Some("bad_number_syntax");
    }
    if msg.contains("{{break}} outside {{range}}") {
        return Some("break_outside_range");
    }
    if msg.contains("non executable command in pipeline") {
        return Some("non_executable_command_in_pipeline");
    }
    if msg.contains("missing value for parenthesized pipeline") {
        return Some("missing_value_for_context");
    }
    if msg.contains("missing value for range") {
        return Some("missing_value_for_context");
    }
    if msg.contains("missing value for command") {
        return Some("missing_value_for_context");
    }
    None
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
    p "text/template/parse"
)

func main() {
    if len(os.Args) != 2 {
        fmt.Print("missing input")
        os.Exit(3)
    }
    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    var cases []string
    if err := json.Unmarshal(data, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]string, len(cases))
    for i, src := range cases {
        tr := p.New("x")
        _, err = tr.Parse(src, "", "", map[string]*p.Tree{}, map[string]any{
            "printf": fmt.Sprintf,
            "print": fmt.Sprint,
        })
        if err != nil {
            out[i] = err.Error()
        }
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
