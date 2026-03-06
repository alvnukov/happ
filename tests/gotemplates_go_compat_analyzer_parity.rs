use happ::gotemplates::go_compat::analyzer::analyze_trees;
use happ::gotemplates::go_compat::parse::{parse, Mode, NodeType};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, serde::Deserialize)]
struct GoAnalyzerResult {
    ok: bool,
    #[serde(default)]
    counts: BTreeMap<String, usize>,
    #[serde(default)]
    max_depth: usize,
    #[serde(default)]
    invocations: Vec<String>,
    #[serde(default)]
    err: String,
}

#[derive(Debug, Clone)]
struct AnalyzerCase {
    src: &'static str,
    parse_comments: bool,
    left_delim: &'static str,
    right_delim: &'static str,
}

#[test]
fn go_compat_analyzer_matches_go_parse_subset() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoAnalyzerRunner::new().expect("prepare go analyzer runner");
    let cases = vec![
        AnalyzerCase {
            src: r#"{{if .x}}{{template "a" .}}{{else}}{{range .r}}{{continue}}{{end}}{{end}}{{define "a"}}ok{{end}}"#,
            parse_comments: false,
            left_delim: "{{",
            right_delim: "}}",
        },
        AnalyzerCase {
            src: r#"{{range .r}}{{/*c*/}}{{if .x}}{{template "x" .}}{{end}}{{end}}{{define "x"}}x{{end}}"#,
            parse_comments: true,
            left_delim: "{{",
            right_delim: "}}",
        },
        AnalyzerCase {
            src: r#"{{with .w}}{{if .x}}A{{else if .y}}{{template "n" .}}{{end}}{{end}}{{define "n"}}N{{end}}"#,
            parse_comments: false,
            left_delim: "{{",
            right_delim: "}}",
        },
        AnalyzerCase {
            src: r#"{{block "blk" .}}{{template "inner" .}}{{end}}{{define "inner"}}I{{end}}"#,
            parse_comments: false,
            left_delim: "{{",
            right_delim: "}}",
        },
        AnalyzerCase {
            src: r#"<<if .x>><<template "a" .>><<else>><<range .r>><<continue>><<end>><<end>><<define "a">>ok<<end>>"#,
            parse_comments: false,
            left_delim: "<<",
            right_delim: ">>",
        },
    ];

    let go_results = runner
        .run_batch(&cases)
        .expect("go analyzer batch should succeed");
    assert_eq!(go_results.len(), cases.len());

    for (idx, case) in cases.iter().enumerate() {
        let mode = if case.parse_comments {
            Mode::PARSE_COMMENTS
        } else {
            Mode::default()
        };
        let trees = parse(
            "main",
            case.src,
            case.left_delim,
            case.right_delim,
            mode,
            &[],
        )
        .expect("rust parse must succeed");
        let analysis = analyze_trees(&trees);

        let go = &go_results[idx];
        assert!(go.ok, "go parse failed for src={}; err={}", case.src, go.err);

        assert_eq!(
            analysis.count(NodeType::If),
            *go.counts.get("if").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::Range),
            *go.counts.get("range").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::With),
            *go.counts.get("with").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::Template),
            *go.counts.get("template").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::Break),
            *go.counts.get("break").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::Continue),
            *go.counts.get("continue").unwrap_or(&0)
        );
        assert_eq!(
            analysis.count(NodeType::Comment),
            *go.counts.get("comment").unwrap_or(&0)
        );
        assert_eq!(
            analysis.max_depth, go.max_depth,
            "max depth mismatch for src={}",
            case.src
        );

        let rust_invocations: BTreeSet<String> =
            analysis.template_invocations.into_iter().collect();
        let go_invocations: BTreeSet<String> = go.invocations.iter().cloned().collect();
        assert_eq!(
            rust_invocations, go_invocations,
            "template invocation set mismatch for src={}",
            case.src
        );
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoAnalyzerRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoAnalyzerRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("go_compat_analyzer_subset.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn run_batch(&self, cases: &[AnalyzerCase]) -> Result<Vec<GoAnalyzerResult>, String> {
        let payload: Vec<serde_json::Value> = cases
            .iter()
            .map(|case| {
                serde_json::json!({
                    "src": case.src,
                    "parse_comments": case.parse_comments,
                    "left_delim": case.left_delim,
                    "right_delim": case.right_delim,
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

        serde_json::from_slice::<Vec<GoAnalyzerResult>>(&output.stdout)
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
    "sort"
    p "text/template/parse"
)

type parseCase struct {
    Src           string `json:"src"`
    ParseComments bool   `json:"parse_comments"`
    LeftDelim     string `json:"left_delim"`
    RightDelim    string `json:"right_delim"`
}

type result struct {
    Ok          bool           `json:"ok"`
    Counts      map[string]int `json:"counts,omitempty"`
    MaxDepth    int            `json:"max_depth,omitempty"`
    Invocations []string       `json:"invocations,omitempty"`
    Err         string         `json:"err,omitempty"`
}

type stats struct {
    counts      map[string]int
    maxDepth    int
    invocations map[string]struct{}
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

func walkList(list *p.ListNode, depth int, st *stats) {
    if list == nil {
        return
    }
    if depth > st.maxDepth {
        st.maxDepth = depth
    }

    for _, node := range list.Nodes {
        switch n := node.(type) {
        case *p.IfNode:
            st.counts["if"]++
            walkList(n.List, depth+1, st)
            walkList(n.ElseList, depth+1, st)
        case *p.RangeNode:
            st.counts["range"]++
            walkList(n.List, depth+1, st)
            walkList(n.ElseList, depth+1, st)
        case *p.WithNode:
            st.counts["with"]++
            walkList(n.List, depth+1, st)
            walkList(n.ElseList, depth+1, st)
        case *p.TemplateNode:
            st.counts["template"]++
            if n.Name != "" {
                st.invocations[n.Name] = struct{}{}
            }
        case *p.BreakNode:
            st.counts["break"]++
        case *p.ContinueNode:
            st.counts["continue"]++
        case *p.CommentNode:
            st.counts["comment"]++
        }
    }
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

    var cases []parseCase
    if err := json.Unmarshal(payload, &cases); err != nil {
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
        _, err := tr.Parse(c.Src, c.LeftDelim, c.RightDelim, treeSet, map[string]any{})
        if err != nil {
            out = append(out, result{Ok: false, Err: err.Error()})
            continue
        }

        st := &stats{counts: ensureCounts(), invocations: map[string]struct{}{}}
        for _, tree := range treeSet {
            walkList(tree.Root, 0, st)
        }

        invocations := make([]string, 0, len(st.invocations))
        for name := range st.invocations {
            invocations = append(invocations, name)
        }
        sort.Strings(invocations)

        out = append(out, result{
            Ok:          true,
            Counts:      st.counts,
            MaxDepth:    st.maxDepth,
            Invocations: invocations,
        })
    }

    data, err := json.Marshal(out)
    if err != nil {
        fmt.Print(err.Error())
        os.Exit(6)
    }
    os.Stdout.Write(data)
}
"#
}
