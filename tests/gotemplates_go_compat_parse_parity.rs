use happ::go_compat::parse::{parse, Mode};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Clone, Copy)]
enum FuncCheckMode {
    Strict,
    Skip,
}

#[test]
fn go_compat_parse_matches_go_parse_modes_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseRunner::new().expect("prepare go parser runner");
    let cases = vec![
        (
            r#"{{foo}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("undefined_function"),
        ),
        (r#"{{foo}}"#, FuncCheckMode::Skip, vec![], None),
        (r#"{{foo}}"#, FuncCheckMode::Strict, vec!["foo"], None),
        (r#"{{if .X}}x{{end}}"#, FuncCheckMode::Strict, vec![], None),
        (
            r#"{{range .X}}{{break 20}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("unexpected_token"),
        ),
        (
            r#"{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("unexpected_end_action"),
        ),
        (
            r#"{{range .}}{{else}}{{break}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("break_outside_range"),
        ),
        (
            r#"{{define "a"}}a{{end}}{{define "a"}}b{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("multiple_template_definition"),
        ),
        (
            r#"{{}}"#,
            FuncCheckMode::Strict,
            vec![],
            Some("missing_value_for_context"),
        ),
    ];

    let go_request: Vec<serde_json::Value> = cases
        .iter()
        .map(|(src, mode, funcs, _)| {
            serde_json::json!({
                "src": src,
                "skip_func_check": matches!(*mode, FuncCheckMode::Skip),
                "funcs": funcs,
                "left_delim": "",
                "right_delim": "",
            })
        })
        .collect();
    let go_codes = runner
        .parse_error_codes(&go_request)
        .expect("go parser should return mapped codes");
    assert_eq!(
        go_codes.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_codes.len(),
        cases.len()
    );

    for (idx, (src, mode, funcs, expected_code)) in cases.iter().enumerate() {
        let parse_mode = if matches!(*mode, FuncCheckMode::Skip) {
            Mode::SKIP_FUNC_CHECK
        } else {
            Mode::default()
        };
        let rust_code = parse("main", src, "{{", "}}", parse_mode, funcs)
            .err()
            .map(|e| e.code.to_string());
        let go_code = go_codes[idx].clone();
        assert_eq!(
            go_code,
            expected_code.map(ToString::to_string),
            "go oracle mismatch for mode case: {src}"
        );
        assert_eq!(
            rust_code, go_code,
            "go_compat/go mismatch for mode case: {src}"
        );
    }
}

#[test]
fn go_compat_parse_matches_go_with_custom_delimiters_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseRunner::new().expect("prepare go parser runner");
    let cases = [
        (
            r#"<<if .X>>x<<end>>"#,
            "<<",
            ">>",
            FuncCheckMode::Strict,
            vec![],
            None,
        ),
        (
            r#"<<foo>>"#,
            "<<",
            ">>",
            FuncCheckMode::Strict,
            vec![],
            Some("undefined_function"),
        ),
        (r#"[[foo]]"#, "[[", "]]", FuncCheckMode::Skip, vec![], None),
    ];

    let go_request: Vec<serde_json::Value> = cases
        .iter()
        .map(|(src, left, right, mode, funcs, _)| {
            serde_json::json!({
                "src": src,
                "skip_func_check": matches!(*mode, FuncCheckMode::Skip),
                "funcs": funcs,
                "left_delim": left,
                "right_delim": right,
            })
        })
        .collect();
    let go_codes = runner
        .parse_error_codes(&go_request)
        .expect("go parser should return mapped codes");
    assert_eq!(go_codes.len(), cases.len());

    for (idx, (src, left, right, mode, funcs, expected_code)) in cases.iter().enumerate() {
        let parse_mode = if matches!(*mode, FuncCheckMode::Skip) {
            Mode::SKIP_FUNC_CHECK
        } else {
            Mode::default()
        };
        let rust_code = parse("main", src, left, right, parse_mode, funcs)
            .err()
            .map(|e| e.code.to_string());
        let go_code = go_codes[idx].clone();
        assert_eq!(
            go_code,
            expected_code.map(ToString::to_string),
            "go oracle mismatch for custom delimiter case: {src}"
        );
        assert_eq!(
            rust_code, go_code,
            "go_compat/go mismatch for custom delimiter case: {src}"
        );
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
        let program = tmp.path().join("go_compat_parse_modes.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn parse_error_codes(
        &self,
        cases: &[serde_json::Value],
    ) -> Result<Vec<Option<String>>, String> {
        let cases_json =
            serde_json::to_string(cases).map_err(|e| format!("serialize cases: {e}"))?;
        let encoded_cases = base64_encode(cases_json.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_cases)
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
    if msg.contains("function \"") && msg.contains("not defined") {
        return Some("undefined_function");
    }
    if msg.contains("unexpected \"") && msg.contains(" in {{break}}") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected \"") && msg.contains(" in {{continue}}") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected token") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected {{end}}") {
        return Some("unexpected_end_action");
    }
    if msg.contains("{{break}} outside {{range}}") {
        return Some("break_outside_range");
    }
    if msg.contains("multiple definition of template") {
        return Some("multiple_template_definition");
    }
    if msg.contains("missing value for parenthesized pipeline")
        || msg.contains("missing value for range")
        || msg.contains("missing value for command")
        || msg.contains("missing value for block clause")
    {
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
    "strings"
    p "text/template/parse"
)

func main() {
    if len(os.Args) != 2 {
        fmt.Print("need encoded cases")
        os.Exit(3)
    }

    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }

    type parseCase struct {
        Src           string   `json:"src"`
        SkipFuncCheck bool     `json:"skip_func_check"`
        Funcs         []string `json:"funcs"`
        LeftDelim     string   `json:"left_delim"`
        RightDelim    string   `json:"right_delim"`
    }
    var cases []parseCase
    if err := json.Unmarshal(data, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]string, len(cases))
    for i, c := range cases {
        tr := p.New("x")
        if c.SkipFuncCheck {
            tr.Mode = p.SkipFuncCheck
        }
        funcs := map[string]any{}
        for _, name := range c.Funcs {
            n := strings.TrimSpace(name)
            if n == "" {
                continue
            }
            funcs[n] = func(args ...any) any { return nil }
        }
        _, err = tr.Parse(c.Src, c.LeftDelim, c.RightDelim, map[string]*p.Tree{}, funcs)
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
