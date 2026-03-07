use happ::go_compat::analyzer::{
    collect_template_invocation_sites, unresolved_template_diagnostics,
};
use happ::go_compat::parse::{parse, Mode};

#[test]
fn go_compat_analyzer_diagnostics_report_unresolved_with_positions() {
    let trees = parse(
        "main",
        "{{define \"main\"}}\n{{template \"a\" .}}\n{{template \"missing\" .}}\n{{end}}{{define \"a\"}}A{{end}}",
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let sites = collect_template_invocation_sites(&trees);
    assert_eq!(sites.len(), 2);
    assert_eq!(sites[0].called_template.as_deref(), Some("a"));
    assert_eq!(sites[1].called_template.as_deref(), Some("missing"));

    let unresolved = unresolved_template_diagnostics(&trees);
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].tree_name, "main");
    assert_eq!(unresolved[0].called_template, "missing");
    assert_eq!(unresolved[0].line, 3);
    assert_eq!(unresolved[0].column, 1);
}

#[test]
fn go_compat_analyzer_diagnostics_decode_escaped_template_names() {
    let trees = parse(
        "main",
        "{{define \"main\"}}{{template \"\\x61\" .}}{{end}}{{define \"a\"}}A{{end}}",
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let unresolved = unresolved_template_diagnostics(&trees);
    assert!(unresolved.is_empty(), "escaped template name must resolve");
}

#[test]
fn go_compat_analyzer_diagnostics_report_unresolved_inside_block_body_tree() {
    let trees = parse(
        "main",
        "{{define \"main\"}}{{block \"blk\" .}}{{template \"missing\" .}}{{end}}{{end}}",
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let unresolved = unresolved_template_diagnostics(&trees);
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].tree_name, "blk");
    assert_eq!(unresolved[0].called_template, "missing");
    assert_eq!(unresolved[0].line, 1);
    assert_eq!(unresolved[0].column, 1);
}

#[test]
fn go_compat_analyzer_diagnostics_support_custom_delimiters() {
    let trees = parse(
        "main",
        "<<define \"main\">>\n<<template \"missing\" .>>\n<<end>>",
        "<<",
        ">>",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let unresolved = unresolved_template_diagnostics(&trees);
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].tree_name, "main");
    assert_eq!(unresolved[0].called_template, "missing");
    assert_eq!(unresolved[0].line, 2);
    assert_eq!(unresolved[0].column, 1);
}
