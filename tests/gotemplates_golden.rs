use happ::gotemplates::parse_template_tokens_strict;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct GoldenCase {
    name: String,
    src: String,
    expected_code: Option<String>,
}

#[test]
fn gotemplates_golden_cases_match_expected_codes() {
    let cases = load_golden_cases();
    for case in &cases {
        let rust_code = parse_template_tokens_strict(&case.src)
            .err()
            .map(|e| e.code.to_string());
        assert_eq!(
            rust_code, case.expected_code,
            "unexpected rust parser result for case: {}",
            case.name
        );
    }
}

#[test]
fn gotemplates_golden_cases_match_go_oracle() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let cases = load_golden_cases();
    let runner = GoParseRunner::new().expect("prepare go parser runner");
    let case_srcs: Vec<&str> = cases.iter().map(|c| c.src.as_str()).collect();
    let go_codes = runner
        .parse_error_codes(&case_srcs)
        .expect("go parser should return code mapping");
    assert_eq!(
        go_codes.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_codes.len(),
        cases.len()
    );

    for (idx, case) in cases.iter().enumerate() {
        let go_code = go_codes[idx].clone();
        assert_eq!(
            go_code, case.expected_code,
            "golden drift from go parser for case: {}",
            case.name
        );
    }
}

fn load_golden_cases() -> Vec<GoldenCase> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("tests/gotemplates/golden/parser_cases.yaml");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden fixture {}: {e}", path.display()));
    serde_yaml::from_str::<Vec<GoldenCase>>(&raw)
        .unwrap_or_else(|e| panic!("parse golden fixture {}: {e}", path.display()))
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

    fn parse_error_codes(&self, src_list: &[&str]) -> Result<Vec<Option<String>>, String> {
        let payload =
            serde_json::to_string(src_list).map_err(|e| format!("serialize cases: {e}"))?;
        let encoded = base64_encode(payload.as_bytes());
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
        Ok(raw
            .into_iter()
            .map(|msg| {
                if msg.is_empty() {
                    None
                } else {
                    map_go_error_to_code(&msg).map(ToString::to_string)
                }
            })
            .collect())
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
    if msg.contains("unexpected {{end}}") {
        return Some("unexpected_end_action");
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
    if msg.contains("unexpected <if> in input") {
        return Some("unexpected_token");
    }
    if msg.contains("unexpected <with> in input") {
        return Some("unexpected_token");
    }
    if msg.contains("missing value for block clause") {
        return Some("missing_value_for_context");
    }
    if msg.contains("{{break}} outside {{range}}") {
        return Some("break_outside_range");
    }
    if msg.contains("{{continue}} outside {{range}}") {
        return Some("continue_outside_range");
    }
    if msg.contains("unclosed left paren") {
        return Some("unclosed_left_paren");
    }
    if msg.contains("unexpected right paren") {
        return Some("unexpected_right_paren");
    }
    if msg.contains("unterminated raw quoted string") {
        return Some("unterminated_raw_quoted_string");
    }
    if msg.contains("bad number syntax") {
        return Some("bad_number_syntax");
    }
    if msg.contains("illegal number syntax") {
        return Some("bad_number_syntax");
    }
    if msg.contains("unexpected <.> after term") {
        return Some("unexpected_dot_after_term");
    }
    if msg.contains("unexpected . after term") {
        return Some("unexpected_dot_after_term");
    }
    if msg.contains("non executable command in pipeline") {
        return Some("non_executable_command_in_pipeline");
    }
    if msg.contains("missing value for parenthesized pipeline") {
        return Some("missing_value_for_context");
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
    "text/template"
)

func main() {
    if len(os.Args) != 2 {
        fmt.Print("missing input list")
        os.Exit(3)
    }
    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }

    var srcList []string
    if err := json.Unmarshal(data, &srcList); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]string, 0, len(srcList))
    for _, src := range srcList {
        _, err = template.New("x").Funcs(template.FuncMap{
            "include": func(name string, data any) string { return "" },
        }).Parse(src)
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
