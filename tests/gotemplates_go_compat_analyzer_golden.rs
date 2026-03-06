use happ::gotemplates::go_compat::analyzer::{
    analyze_trees, collect_template_invocation_sites, unresolved_template_diagnostics,
};
use happ::gotemplates::go_compat::parse::{parse, Mode};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    root().join("tests/gotemplates/go_compat_analyzer/fixtures")
}

fn golden_dir() -> PathBuf {
    root().join("tests/gotemplates/go_compat_analyzer/golden")
}

fn read_fixture(case: &str) -> String {
    fs::read_to_string(fixtures_dir().join(format!("{case}.tpl"))).expect("read fixture")
}

#[derive(Debug, Serialize)]
struct SiteSnapshot {
    tree_name: String,
    action_text: String,
    called_template: Option<String>,
    from_block: bool,
    line: usize,
    column: usize,
}

#[derive(Debug, Serialize)]
struct DiagnosticSnapshot {
    tree_name: String,
    called_template: String,
    action_text: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Serialize)]
struct Snapshot {
    tree_count: usize,
    max_depth: usize,
    node_counts: BTreeMap<String, usize>,
    template_invocations: Vec<String>,
    invocation_sites: Vec<SiteSnapshot>,
    unresolved: Vec<DiagnosticSnapshot>,
}

fn snapshot_for(src: &str, left_delim: &str, right_delim: &str) -> Snapshot {
    let trees = parse(
        "main",
        src,
        left_delim,
        right_delim,
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let analysis = analyze_trees(&trees);
    let mut node_counts = BTreeMap::new();
    for (node_type, count) in analysis.node_counts {
        node_counts.insert(format!("{node_type:?}"), count);
    }

    let template_invocations = analysis.template_invocations.into_iter().collect();
    let invocation_sites = collect_template_invocation_sites(&trees)
        .into_iter()
        .map(|s| SiteSnapshot {
            tree_name: s.tree_name,
            action_text: s.action_text,
            called_template: s.called_template,
            from_block: s.from_block,
            line: s.line,
            column: s.column,
        })
        .collect();
    let unresolved = unresolved_template_diagnostics(&trees)
        .into_iter()
        .map(|d| DiagnosticSnapshot {
            tree_name: d.tree_name,
            called_template: d.called_template,
            action_text: d.action_text,
            line: d.line,
            column: d.column,
        })
        .collect();

    Snapshot {
        tree_count: analysis.tree_count,
        max_depth: analysis.max_depth,
        node_counts,
        template_invocations,
        invocation_sites,
        unresolved,
    }
}

fn to_snapshot_json(src: &str, left_delim: &str, right_delim: &str) -> String {
    let snap = snapshot_for(src, left_delim, right_delim);
    let mut out = serde_json::to_string_pretty(&snap).expect("serialize snapshot");
    out.push('\n');
    out
}

fn assert_golden(case: &str, left_delim: &str, right_delim: &str) {
    let input = read_fixture(case);
    let actual = to_snapshot_json(&input, left_delim, right_delim);
    let path = golden_dir().join(format!("{case}.json"));

    if std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1") {
        fs::create_dir_all(golden_dir()).expect("create golden dir");
        fs::write(&path, actual.as_bytes()).expect("update golden");
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read golden {}: {e}. Run UPDATE_GOLDEN=1 cargo test -q gotemplates_go_compat_analyzer_golden",
            path.display()
        )
    });

    assert_eq!(
        expected, actual,
        "golden mismatch for case '{case}'. If intentional: UPDATE_GOLDEN=1 cargo test -q gotemplates_go_compat_analyzer_golden"
    );
}

#[test]
fn golden_nested_invocations() {
    assert_golden("nested_invocations", "{{", "}}");
}

#[test]
fn golden_custom_delims() {
    assert_golden("custom_delims", "<<", ">>");
}
