use happ::gotemplates::go_compat::parse::{parse, Mode};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, serde::Deserialize)]
struct GoRoundtripResult {
    ok: bool,
    #[serde(default)]
    trees: BTreeMap<String, String>,
    #[serde(default)]
    err: String,
}

#[test]
fn go_compat_root_to_source_matches_go_tree_string_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoRoundtripRunner::new().expect("prepare go runner");
    let cases = vec![
        (r#"{{if .a}}A{{else}}B{{end}}"#, "{{", "}}"),
        (r#"{{if .a}}A{{else if .b}}B{{else}}C{{end}}"#, "{{", "}}"),
        (
            r#"{{if .a -}}A{{- else if .b -}}B{{- else -}}C{{- end}}"#,
            "{{",
            "}}",
        ),
        (r#"{{range .r}}{{continue}}{{else}}none{{end}}"#, "{{", "}}"),
        (
            r#"{{with .w}}{{template "x" .}}{{end}}{{define "x"}}X{{end}}"#,
            "{{",
            "}}",
        ),
        (r#"{{block "b" .}}Y{{end}}"#, "{{", "}}"),
        (r#"{{block "\x62" .}}Y{{end}}"#, "{{", "}}"),
        (r#"{{block "b" .x}}Y{{end}}"#, "{{", "}}"),
        (
            "{{define \"main\"}}\n{{template \"x\" .}}\n{{end}}{{define \"x\"}}X{{end}}",
            "{{",
            "}}",
        ),
        (r#"<<if .a>>A<<else>>B<<end>>"#, "<<", ">>"),
    ];

    let go_results = runner.run_batch(&cases).expect("go roundtrip batch");
    assert_eq!(go_results.len(), cases.len());

    for (idx, (src, left, right)) in cases.iter().enumerate() {
        let rust_trees =
            parse("main", src, left, right, Mode::default(), &[]).expect("rust parse must succeed");
        let rust_map: BTreeMap<String, String> = rust_trees
            .iter()
            .map(|(name, tree)| (name.clone(), tree.root.to_source()))
            .collect();

        let go = &go_results[idx];
        assert!(go.ok, "go parse failed for src={src}; err={}", go.err);
        assert_eq!(
            rust_map, go.trees,
            "tree root source mismatch for src={src}"
        );
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoRoundtripRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoRoundtripRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_compat_source_roundtrip.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn run_batch(&self, cases: &[(&str, &str, &str)]) -> Result<Vec<GoRoundtripResult>, String> {
        let payload: Vec<serde_json::Value> = cases
            .iter()
            .map(|(src, left, right)| {
                serde_json::json!({
                    "src": src,
                    "left_delim": left,
                    "right_delim": right,
                })
            })
            .collect();
        let payload = serde_json::to_string(&payload).map_err(|e| format!("serialize: {e}"))?;
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

        serde_json::from_slice::<Vec<GoRoundtripResult>>(&output.stdout)
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
    "encoding/base64"
    "encoding/json"
    "fmt"
    "os"
    p "text/template/parse"
)

type result struct {
    Ok    bool              `json:"ok"`
    Trees map[string]string `json:"trees,omitempty"`
    Err   string            `json:"err,omitempty"`
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

    type parseCase struct {
        Src string `json:"src"`
        LeftDelim string `json:"left_delim"`
        RightDelim string `json:"right_delim"`
    }
    var cases []parseCase
    if err := json.Unmarshal(payload, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]result, 0, len(cases))
    for _, c := range cases {
        tr := p.New("main")
        treeSet := map[string]*p.Tree{}
        _, err := tr.Parse(c.Src, c.LeftDelim, c.RightDelim, treeSet, map[string]any{})
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }

        trees := map[string]string{}
        for name, tree := range treeSet {
            trees[name] = tree.Root.String()
        }
        out = append(out, result{Ok: true, Trees: trees})
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
