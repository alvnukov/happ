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

    for (src, mode, funcs, check_variables, expected_code) in cases {
        let opts = ParseCompatOptions {
            skip_func_check: matches!(mode, FuncCheckMode::Skip),
            known_functions: &funcs,
            check_variables,
            visible_variables: &[],
        };
        let rust_code = parse_template_tokens_strict_with_options(src, opts)
            .err()
            .map(|e| e.code.to_string());
        let go_code = runner
            .parse_error_code(src, mode, &funcs)
            .expect("go parser should return code mapping");

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

    fn parse_error_code(
        &self,
        src: &str,
        mode: FuncCheckMode,
        funcs: &[&str],
    ) -> Result<Option<String>, String> {
        let encoded_src = base64_encode(src.as_bytes());
        let mode_arg = match mode {
            FuncCheckMode::Strict => "strict",
            FuncCheckMode::Skip => "skip",
        };
        let funcs_arg = funcs.join(",");

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(mode_arg)
            .arg(encoded_src)
            .arg(funcs_arg)
            .output()
            .map_err(|e| format!("go run failed to start: {e}"))?;

        if output.status.success() {
            return Ok(None);
        }

        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if raw.is_empty() {
            return Err(format!(
                "go run failed without parser output: status={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(map_go_error_to_code(&raw).map(ToString::to_string))
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
    "fmt"
    "os"
    "strings"
    p "text/template/parse"
)

func main() {
    if len(os.Args) != 4 {
        fmt.Print("need mode, src, funcs")
        os.Exit(3)
    }

    mode := os.Args[1]
    data, err := base64.StdEncoding.DecodeString(os.Args[2])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    funcNames := os.Args[3]

    tr := p.New("x")
    if mode == "skip" {
        tr.Mode = p.SkipFuncCheck
    }

    funcs := map[string]any{}
    if strings.TrimSpace(funcNames) != "" {
        for _, name := range strings.Split(funcNames, ",") {
            n := strings.TrimSpace(name)
            if n == "" {
                continue
            }
            funcs[n] = func(args ...any) any { return nil }
        }
    }

    _, err = tr.Parse(string(data), "", "", map[string]*p.Tree{}, funcs)
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(2)
    }
}
"#
}
