use happ::gotemplates::{parse_template_tokens_strict_with_options, ParseCompatOptions};
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
fn gotemplates_parser_modes_match_go_parse_modes() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseModeRunner::new().expect("prepare go parser runner");
    let cases = vec![
        (
            r#"{{foo}}"#,
            FuncCheckMode::Strict,
            vec![],
            false,
            Some("undefined_function"),
        ),
        (r#"{{foo}}"#, FuncCheckMode::Skip, vec![], false, None),
        (
            r#"{{foo}}"#,
            FuncCheckMode::Strict,
            vec!["foo"],
            false,
            None,
        ),
        (
            r#"{{range .X}}{{break 20}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            false,
            Some("unexpected_token"),
        ),
        (
            r#"{{range .X}}{{break 20}}{{end}}"#,
            FuncCheckMode::Strict,
            vec!["break"],
            false,
            None,
        ),
        (
            r#"{{range .X}}{{continue 20}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            false,
            Some("unexpected_token"),
        ),
        (
            r#"{{range .X}}{{continue 20}}{{end}}"#,
            FuncCheckMode::Strict,
            vec!["continue"],
            false,
            None,
        ),
        (
            r#"{{$x := 1}}{{$x}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            None,
        ),
        (
            r#"{{$x}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            Some("undefined_variable"),
        ),
        (
            r#"{{with $x := 4}}{{end}}{{$x}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            Some("undefined_variable"),
        ),
        (
            r#"{{if .X}}{{$x := 1}}{{else}}{{$x}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            None,
        ),
        (
            r#"{{with $v, $u := 3}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            Some("too_many_declarations"),
        ),
        (
            r#"{{range $u, $v, $w := 3}}{{end}}"#,
            FuncCheckMode::Strict,
            vec![],
            true,
            Some("too_many_declarations"),
        ),
    ];

    let go_request: Vec<serde_json::Value> = cases
        .iter()
        .map(|(src, mode, funcs, _check_variables, _expected_code)| {
            serde_json::json!({
                "src": src,
                "skip_func_check": matches!(*mode, FuncCheckMode::Skip),
                "funcs": funcs,
            })
        })
        .collect();
    let go_codes = runner
        .parse_error_codes(&go_request)
        .expect("go parser should return code mapping");
    assert_eq!(
        go_codes.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_codes.len(),
        cases.len()
    );

    for (idx, (src, mode, funcs, check_variables, expected_code)) in cases.iter().enumerate() {
        let opts = ParseCompatOptions {
            skip_func_check: matches!(*mode, FuncCheckMode::Skip),
            known_functions: funcs,
            check_variables: *check_variables,
            visible_variables: &[],
        };
        let rust_code = parse_template_tokens_strict_with_options(src, opts)
            .err()
            .map(|e| e.code.to_string());
        let go_code = go_codes[idx].clone();

        assert_eq!(
            go_code,
            expected_code.map(ToString::to_string),
            "go oracle mismatch for mode case: {src}"
        );
        assert_eq!(rust_code, go_code, "rust/go mode mismatch for case: {src}");
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoParseModeRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoParseModeRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("parsemodecheck.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn parse_error_codes(&self, cases: &[serde_json::Value]) -> Result<Vec<Option<String>>, String> {
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
    if msg.contains("undefined variable") {
        return Some("undefined_variable");
    }
    if msg.contains("too many declarations") {
        return Some("too_many_declarations");
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

        _, err = tr.Parse(c.Src, "", "", map[string]*p.Tree{}, funcs)
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
