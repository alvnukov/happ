use happ::gotemplates::parse_template_tokens_strict;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn gotemplates_strict_parser_matches_go_for_core_error_cases() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseRunner::new().expect("prepare go parser runner");

    let cases = vec![
        (r#"{{ include "a" . }}"#, None),
        (r#"{{ include "a" . "#, Some("unterminated_action")),
        (
            r#"{{/* comment */ x }}"#,
            Some("comment_ends_before_closing_delimiter"),
        ),
        (
            r#"{{ include "a" {{ .Values.x }} }}"#,
            Some("unexpected_left_delim_in_operand"),
        ),
        (
            r#"{{ .Values.bad..path }}"#,
            Some("unexpected_dot_in_operand"),
        ),
        (r#"{{ include "a }}"#, Some("unterminated_quoted_string")),
        (r#"{{ if .Values.x }}{{ end }}"#, None),
        (r#"{{ if .Values.x }}"#, Some("unexpected_eof")),
        (r#"{{ else }}"#, Some("unexpected_else_action")),
        (r#"{{ else if .Cond }}"#, Some("unexpected_else_action")),
        (r#"{{ end }}"#, Some("unexpected_end_action")),
        (r#"{{ define "a" }}{{ end }}"#, None),
        (r#"{{ define "a" }}"#, Some("unexpected_eof")),
        (r#"{{ break }}"#, Some("break_outside_range")),
        (r#"{{ continue }}"#, Some("continue_outside_range")),
        (r#"{{ range .Items }}{{ break }}{{ end }}"#, None),
        (
            r#"{{ range .Items }}x{{ else }}{{ break }}{{ end }}"#,
            Some("break_outside_range"),
        ),
        (
            r#"{{ range .Items }}x{{ else if .Cond }}y{{ end }}"#,
            Some("unexpected_token"),
        ),
        (
            r#"{{ range .Items }}x{{ else with .Ctx }}y{{ end }}"#,
            Some("unexpected_token"),
        ),
        (
            r#"{{ with .Ctx }}x{{ else if .Cond }}y{{ end }}"#,
            Some("unexpected_token"),
        ),
        (
            r#"{{ if .Cond }}x{{ else with .Ctx }}y{{ end }}"#,
            Some("unexpected_token"),
        ),
        (
            r#"{{ if .Cond }}x{{ else }}y{{ else }}z{{ end }}"#,
            Some("unexpected_else_action"),
        ),
        (r#"{{ block "x" . }}{{ end }}"#, None),
        (r#"{{ block "x" }}"#, Some("missing_value_for_context")),
        (r#"{{ template "x" . }}"#, None),
        (r#"{{ template "x" }}"#, None),
        (
            r#"{{ define "x" }}{{ else }}{{ end }}"#,
            Some("unexpected_else_action"),
        ),
    ];

    let templates: Vec<&str> = cases.iter().map(|(src, _)| *src).collect();
    let go_codes = runner
        .parse_error_codes(&templates)
        .expect("go parser should return code mapping");
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
        let program = tmp.path().join("parsecheck.go");
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
    if msg.contains("unclosed action") {
        return Some("unterminated_action");
    }
    if msg.contains("unexpected {{else}}") {
        return Some("unexpected_else_action");
    }
    if msg.contains("expected end; found {{else}}") {
        return Some("unexpected_else_action");
    }
    if msg.contains("unexpected <if> in input") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected <with> in input") {
        return Some("unexpected_token");
    }
    if msg.contains("missing value for block clause") {
        return Some("missing_value_for_context");
    }
    if msg.contains("unexpected {{end}}") {
        return Some("unexpected_end_action");
    }
    if msg.contains("{{break}} outside {{range}}") {
        return Some("break_outside_range");
    }
    if msg.contains("{{continue}} outside {{range}}") {
        return Some("continue_outside_range");
    }
    if msg.contains("comment ends before closing delimiter") {
        return Some("comment_ends_before_closing_delimiter");
    }
    if msg.contains("unexpected \"{\" in operand") {
        return Some("unexpected_left_delim_in_operand");
    }
    if msg.contains("unexpected <.> in operand") {
        return Some("unexpected_dot_in_operand");
    }
    if msg.contains("unterminated quoted string") {
        return Some("unterminated_quoted_string");
    }
    if msg.contains("unterminated character constant") {
        return Some("unterminated_character_constant");
    }
    if msg.contains("unexpected . after term") || msg.contains("unexpected <.> after term") {
        return Some("unexpected_dot_after_term");
    }
    if msg.contains("illegal number syntax") || msg.contains("bad number syntax") {
        return Some("bad_number_syntax");
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
    "text/template"
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
        _, err = template.New("x").Funcs(template.FuncMap{
            "include": func(name string, data any) string { return "" },
        }).Parse(src)
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
