use happ::gotemplates::{parse_template_tokens_strict_with_options, ParseCompatOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn gotemplates_matches_go_parse_subset_cases() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseRunner::new().expect("prepare go parser runner");
    let known_functions = ["printf", "print", "contains", "привет"];

    let cases = vec![
        r#"{{.X}}"#,
        r#"{{printf `%d` 23}}"#,
        r#"{{.X|.Y}}"#,
        r#"{{if .X}}hello{{end}}"#,
        r#"{{if .X}}true{{else}}false{{end}}"#,
        r#"{{if .X}}true{{else if .Y}}false{{end}}"#,
        r#"{{range .X}}hello{{end}}"#,
        r#"{{range .X}}true{{else}}false{{end}}"#,
        r#"{{range $x := .SI}}{{.}}{{end}}"#,
        r#"{{range $x, $y := .SI}}{{.}}{{end}}"#,
        r#"{{template `x`}}"#,
        r#"{{template `x` .Y}}"#,
        r#"{{with .X}}hello{{end}}"#,
        r#"{{with .X}}hello{{else}}goodbye{{end}}"#,
        r#"{{with .X}}hello{{else with .Y}}goodbye{{end}}"#,
        r#"{{}}"#,
        r#"{{end}}"#,
        r#"{{else}}"#,
        r#"hello{{range .x}}"#,
        r#"hello{{undefined}}"#,
        r#"{{$x}}"#,
        r#"{{with $x := 4}}{{end}}{{$x}}"#,
        r#"{{привет .}}"#,
        r#"{{$имя := .данные.ключ}}{{$имя}}"#,
        r#"{{template $v}}"#,
        r#"{{with $x.Y := 4}}{{end}}"#,
        r#"{{printf 3, 4}}"#,
        r#"{{with $v, $u := 3}}{{end}}"#,
        r#"{{range $u, $v, $w := 3}}{{end}}"#,
        r#"{{printf (printf .).}}"#,
        r#"{{printf 3`x`}}"#,
        r#"{{printf `x`.}}"#,
        r#"{{if .X}}a{{else if .Y}}b{{end}}{{end}}"#,
        r#"{{range .}}{{end}} {{break}}"#,
        r#"{{range .}}{{else}}{{break}}{{end}}"#,
        r#"{{12|false}}"#,
        r#"{{printf "%d" ( ) }}"#,
        r#"{{range $k,}}{{end}}"#,
        r#"{{range $k, $v := }}{{end}}"#,
        r#"{{range $k, .}}{{end}}"#,
        r#"{{range $k, 123 := .}}{{end}}"#,
        r#"{{define "a"}}a{{end}}{{define "a"}}b{{end}}"#,
        r#"{{define "a"}}{{end}}{{define "a"}}b{{end}}"#,
        r#"{{define "a"}}a{{end}}{{define "a"}}{{end}}"#,
    ];
    let go_codes = runner
        .parse_error_codes(&cases, &known_functions)
        .expect("go parser should return mapped codes");
    assert_eq!(
        go_codes.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_codes.len(),
        cases.len()
    );

    for (idx, src) in cases.iter().enumerate() {
        let rust_code = parse_template_tokens_strict_with_options(
            src,
            ParseCompatOptions {
                skip_func_check: false,
                known_functions: &known_functions,
                check_variables: true,
                visible_variables: &[],
            },
        )
        .err()
        .map(|e| e.code.to_string());

        let go_code = go_codes[idx].clone();

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
        let program = tmp.path().join("parsecases.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn parse_error_codes(
        &self,
        cases: &[&str],
        funcs: &[&str],
    ) -> Result<Vec<Option<String>>, String> {
        let cases_json =
            serde_json::to_string(cases).map_err(|e| format!("serialize cases: {e}"))?;
        let encoded_cases = base64_encode(cases_json.as_bytes());
        let funcs_arg = funcs.join(",");
        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_cases)
            .arg(funcs_arg)
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
    if msg.contains("undefined variable") {
        return Some("undefined_variable");
    }
    if msg.contains("unexpected EOF") {
        return Some("unexpected_eof");
    }
    if msg.contains("unexpected {{else}}") {
        return Some("unexpected_else_action");
    }
    if msg.contains("expected end; found {{else}}") {
        return Some("unexpected_else_action");
    }
    if msg.contains("unexpected {{end}}") {
        return Some("unexpected_end_action");
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
    if msg.contains("unexpected \":=\" in operand") {
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
    if msg.contains("unexpected <.> in operand") {
        return Some("unexpected_dot_in_operand");
    }
    if msg.contains("unexpected <if> in input") || msg.contains("unexpected <with> in input") {
        return Some("unexpected_token");
    }
    if msg.contains("{{break}} outside {{range}}") {
        return Some("break_outside_range");
    }
    if msg.contains("{{continue}} outside {{range}}") {
        return Some("continue_outside_range");
    }
    if msg.contains("non executable command in pipeline") {
        return Some("non_executable_command_in_pipeline");
    }
    if msg.contains("missing value for parenthesized pipeline")
        || msg.contains("missing value for range")
        || msg.contains("missing value for command")
        || msg.contains("missing value for block clause")
    {
        return Some("missing_value_for_context");
    }
    if msg.contains("bad number syntax") || msg.contains("illegal number syntax") {
        return Some("bad_number_syntax");
    }
    if msg.contains("unterminated quoted string") {
        return Some("unterminated_quoted_string");
    }
    if msg.contains("unterminated character constant") {
        return Some("unterminated_character_constant");
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
    if len(os.Args) != 3 {
        fmt.Print("need cases and funcs")
        os.Exit(3)
    }
    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    funcsArg := os.Args[2]
    funcs := map[string]any{}
    if strings.TrimSpace(funcsArg) != "" {
        for _, name := range strings.Split(funcsArg, ",") {
            n := strings.TrimSpace(name)
            if n == "" {
                continue
            }
            funcs[n] = func(args ...any) any { return nil }
        }
    }
    var cases []string
    if err := json.Unmarshal(data, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }
    out := make([]string, 0, len(cases))
    for _, src := range cases {
        tr := p.New("x")
        _, err = tr.Parse(src, "", "", map[string]*p.Tree{}, funcs)
        if err != nil {
            out = append(out, err.Error())
        } else {
            out = append(out, "")
        }
    }
    encoded, err := json.Marshal(out)
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }
    fmt.Print(string(encoded))
}
"#
}
