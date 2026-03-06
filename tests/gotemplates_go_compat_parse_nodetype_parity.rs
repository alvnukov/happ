use happ::gotemplates::go_compat::parse::{parse, walk_list, Mode, Node, WalkControl};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, serde::Deserialize)]
struct GoCountsResult {
    ok: bool,
    #[serde(default)]
    counts: BTreeMap<String, usize>,
    #[serde(default)]
    err: String,
}

#[test]
fn go_compat_parse_node_type_counts_match_go_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoCountsRunner::new().expect("prepare go parse runner");
    let cases = vec![
        (r#"{{if .x}}a{{else}}b{{end}}{{template "x" .}}"#, false),
        (
            r#"{{range .r}}{{if .x}}{{break}}{{end}}{{continue}}{{end}}"#,
            false,
        ),
        (
            r#"{{with .w}}{{template "x" .}}{{end}}{{define "x"}}z{{end}}"#,
            false,
        ),
        (
            r#"{{if .x}}{{/*a*/}}{{end}}{{range .r}}{{/*b*/}}{{end}}"#,
            true,
        ),
    ];

    let go_results = runner
        .run_batch(&cases)
        .expect("go parse batch should succeed");
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
        let rust = parse("main", src, "{{", "}}", mode, &[]).expect("rust parse must succeed");
        let go = &go_results[idx];
        assert!(go.ok, "go parse failed for src={src}; err={}", go.err);

        let rust_counts = count_rust_nodes_flat(&rust);
        assert_eq!(
            rust_counts, go.counts,
            "node counts mismatch for src={src}; parse_comments={parse_comments}"
        );
    }
}

fn count_rust_nodes_flat(
    trees: &BTreeMap<String, happ::gotemplates::go_compat::parse::Tree>,
) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::from([
        ("if".to_string(), 0),
        ("range".to_string(), 0),
        ("with".to_string(), 0),
        ("template".to_string(), 0),
        ("break".to_string(), 0),
        ("continue".to_string(), 0),
        ("comment".to_string(), 0),
    ]);
    for tree in trees.values() {
        walk_list(&tree.root, &mut |node| {
            match node {
                Node::If(_) => *out.get_mut("if").expect("if key") += 1,
                Node::Range(_) => *out.get_mut("range").expect("range key") += 1,
                Node::With(_) => *out.get_mut("with").expect("with key") += 1,
                Node::Template(_) => *out.get_mut("template").expect("template key") += 1,
                Node::Break(_) => *out.get_mut("break").expect("break key") += 1,
                Node::Continue(_) => *out.get_mut("continue").expect("continue key") += 1,
                Node::Comment(_) => *out.get_mut("comment").expect("comment key") += 1,
                Node::Text(_)
                | Node::Action(_)
                | Node::Block(_)
                | Node::Define(_)
                | Node::Else(_)
                | Node::End(_) => {}
            }
            WalkControl::Continue
        });
    }
    out
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoCountsRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoCountsRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_compat_parse_nodetype.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn run_batch(&self, cases: &[(&str, bool)]) -> Result<Vec<GoCountsResult>, String> {
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
        serde_json::from_slice::<Vec<GoCountsResult>>(&output.stdout)
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
    Ok     bool           `json:"ok"`
    Counts map[string]int `json:"counts,omitempty"`
    Err    string         `json:"err,omitempty"`
}

func ensureCounts() map[string]int {
    return map[string]int{
        "if":       0,
        "range":    0,
        "with":     0,
        "template": 0,
        "break":    0,
        "continue": 0,
        "comment":  0,
    }
}

func walkList(list *p.ListNode, counts map[string]int) {
    if list == nil {
        return
    }
    for _, node := range list.Nodes {
        switch n := node.(type) {
        case *p.IfNode:
            counts["if"]++
            walkList(n.List, counts)
            walkList(n.ElseList, counts)
        case *p.RangeNode:
            counts["range"]++
            walkList(n.List, counts)
            walkList(n.ElseList, counts)
        case *p.WithNode:
            counts["with"]++
            walkList(n.List, counts)
            walkList(n.ElseList, counts)
        case *p.TemplateNode:
            counts["template"]++
        case *p.BreakNode:
            counts["break"]++
        case *p.ContinueNode:
            counts["continue"]++
        case *p.CommentNode:
            counts["comment"]++
        case *p.ListNode:
            walkList(n, counts)
        }
    }
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
        counts := ensureCounts()
        for _, tree := range treeSet {
            walkList(tree.Root, counts)
        }
        out = append(out, result{Ok: true, Counts: counts})
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
