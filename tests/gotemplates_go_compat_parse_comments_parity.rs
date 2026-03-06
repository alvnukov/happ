use happ::gotemplates::go_compat::parse::{parse, walk_list, Mode, Node, WalkControl};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, serde::Deserialize)]
struct GoCaseResult {
    ok: bool,
    #[serde(default)]
    comments: usize,
    #[serde(default)]
    err: String,
}

#[test]
fn go_compat_parse_comments_mode_matches_go_parse_mode() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoParseCommentsRunner::new().expect("prepare go parse runner");
    let cases = vec![
        (r#"a{{/* c */}}b"#, false),
        (r#"a{{/* c */}}b"#, true),
        (r#"{{if .x}}{{/*a*/}}x{{else}}{{/*b*/}}y{{end}}"#, false),
        (r#"{{if .x}}{{/*a*/}}x{{else}}{{/*b*/}}y{{end}}"#, true),
        (
            r#"{{define "x"}}{{/*d*/}}X{{end}}{{template "x" .}}"#,
            false,
        ),
        (r#"{{define "x"}}{{/*d*/}}X{{end}}{{template "x" .}}"#, true),
        (r#"{{/* only */}}"#, false),
        (r#"{{/* only */}}"#, true),
        (r#"{{/* broken }}"#, false),
        (r#"{{/* broken }}"#, true),
    ];

    let go_results = runner
        .run_batch(&cases)
        .expect("go parse comments batch should succeed");
    assert_eq!(
        go_results.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_results.len(),
        cases.len()
    );

    for (idx, (src, parse_comments)) in cases.iter().enumerate() {
        let mode = if *parse_comments {
            Mode::PARSE_COMMENTS
        } else {
            Mode::default()
        };
        let rust = parse("main", src, "{{", "}}", mode, &[]);
        let go = &go_results[idx];

        assert_eq!(
            rust.is_ok(),
            go.ok,
            "ok/err mismatch for src={src}; parse_comments={parse_comments}; go_err={}",
            go.err
        );

        if let Ok(trees) = rust {
            let rust_comments: usize = trees
                .values()
                .map(|tree| {
                    let mut count = 0usize;
                    walk_list(&tree.root, &mut |node| {
                        if matches!(node, Node::Comment(_)) {
                            count += 1;
                        }
                        WalkControl::Continue
                    });
                    count
                })
                .sum();
            assert_eq!(
                rust_comments, go.comments,
                "comment count mismatch for src={src}; parse_comments={parse_comments}"
            );
        }
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoParseCommentsRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoParseCommentsRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_compat_parse_comments.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn run_batch(&self, cases: &[(&str, bool)]) -> Result<Vec<GoCaseResult>, String> {
        let payload: Vec<serde_json::Value> = cases
            .iter()
            .map(|(src, parse_comments)| {
                serde_json::json!({
                    "src": src,
                    "parse_comments": parse_comments,
                })
            })
            .collect();
        let payload_json =
            serde_json::to_string(&payload).map_err(|e| format!("serialize payload: {e}"))?;
        let encoded = base64_encode(payload_json.as_bytes());

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
        serde_json::from_slice::<Vec<GoCaseResult>>(&output.stdout)
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

type parseCase struct {
    Src           string `json:"src"`
    ParseComments bool   `json:"parse_comments"`
}

type result struct {
    Ok       bool   `json:"ok"`
    Comments int    `json:"comments,omitempty"`
    Err      string `json:"err,omitempty"`
}

func countCommentsList(list *p.ListNode) int {
    if list == nil {
        return 0
    }
    sum := 0
    for _, node := range list.Nodes {
        switch n := node.(type) {
        case *p.CommentNode:
            sum++
        case *p.IfNode:
            sum += countCommentsList(n.List)
            sum += countCommentsList(n.ElseList)
        case *p.RangeNode:
            sum += countCommentsList(n.List)
            sum += countCommentsList(n.ElseList)
        case *p.WithNode:
            sum += countCommentsList(n.List)
            sum += countCommentsList(n.ElseList)
        case *p.ListNode:
            sum += countCommentsList(n)
        }
    }
    return sum
}

func main() {
    if len(os.Args) != 2 {
        fmt.Print("need encoded payload")
        os.Exit(3)
    }
    data, err := base64.StdEncoding.DecodeString(os.Args[1])
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(4)
    }
    var cases []parseCase
    if err := json.Unmarshal(data, &cases); err != nil {
        fmt.Print(err.Error())
        os.Exit(5)
    }

    out := make([]result, 0, len(cases))
    for _, c := range cases {
        tr := p.New("main")
        if c.ParseComments {
            tr.Mode = p.ParseComments
        }
        treeSet := map[string]*p.Tree{}
        _, err := tr.Parse(c.Src, "", "", treeSet, map[string]any{})
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }
        comments := 0
        for _, tree := range treeSet {
            comments += countCommentsList(tree.Root)
        }
        out = append(out, result{Ok: true, Comments: comments})
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
