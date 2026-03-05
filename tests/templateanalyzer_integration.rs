use happ::templateanalyzer::{
    analyze_template, collect_include_names_in_template, collect_values_paths_in_template,
    extract_define_blocks,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    root().join("tests/templateanalyzer/fixtures")
}

fn golden_dir() -> PathBuf {
    root().join("tests/templateanalyzer/golden")
}

fn read_fixture(case: &str) -> String {
    fs::read_to_string(fixtures_dir().join(format!("{case}.tpl"))).expect("read fixture")
}

#[derive(Debug, Serialize)]
struct DiagnosticSnapshot {
    code: String,
    message: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Serialize)]
struct Snapshot {
    include_names: Vec<String>,
    values_paths: Vec<Vec<String>>,
    define_blocks: BTreeMap<String, String>,
    include_graph: BTreeMap<String, Vec<String>>,
    unresolved_local_includes: Vec<String>,
    diagnostics: Vec<DiagnosticSnapshot>,
    include_cycles: Vec<Vec<String>>,
}

fn snapshot_for(src: &str) -> Snapshot {
    let analyzed = analyze_template(src);
    Snapshot {
        include_names: analyzed.include_names,
        values_paths: analyzed.values_paths,
        define_blocks: analyzed.define_blocks,
        include_graph: analyzed.include_graph,
        unresolved_local_includes: analyzed.unresolved_local_includes,
        diagnostics: analyzed
            .diagnostics
            .into_iter()
            .map(|d| DiagnosticSnapshot {
                code: d.code,
                message: d.message,
                line: d.line,
                column: d.column,
            })
            .collect(),
        include_cycles: analyzed.include_cycles,
    }
}

fn to_snapshot_json(src: &str) -> String {
    let snap = snapshot_for(src);
    let mut out = serde_json::to_string_pretty(&snap).expect("serialize snapshot");
    out.push('\n');
    out
}

fn assert_golden(case: &str) {
    let input = read_fixture(case);
    let actual = to_snapshot_json(&input);
    let path = golden_dir().join(format!("{case}.json"));

    if std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1") {
        fs::write(&path, actual.as_bytes()).expect("update golden");
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read golden {}: {e}. Run UPDATE_GOLDEN=1 cargo test -q templateanalyzer_integration",
            path.display()
        )
    });

    assert_eq!(
        expected, actual,
        "golden mismatch for case '{case}'. If intentional: UPDATE_GOLDEN=1 cargo test -q templateanalyzer_integration"
    );
}

#[test]
fn integration_analyze_template_matches_collectors_on_realistic_fixture() {
    let src = read_fixture("helpers_roundtrip");
    let analyzed = analyze_template(&src);

    assert_eq!(
        analyzed.include_names,
        collect_include_names_in_template(&src)
    );
    assert_eq!(
        analyzed.values_paths,
        collect_values_paths_in_template(&src)
    );
    assert_eq!(analyzed.define_blocks, extract_define_blocks(&src));
}

#[test]
fn golden_helpers_roundtrip() {
    assert_golden("helpers_roundtrip");
}

#[test]
fn golden_escaped_and_partial() {
    assert_golden("escaped_and_partial");
}

#[test]
fn golden_duplicate_and_malformed_defines() {
    assert_golden("duplicate_and_malformed_defines");
}
