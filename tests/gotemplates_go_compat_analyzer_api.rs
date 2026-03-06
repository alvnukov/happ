use happ::gotemplates::go_compat::analyzer::Analyzer;
use happ::gotemplates::go_compat::parse::NodeType;

#[test]
fn go_compat_analyzer_api_analyze_source_reports_expected_fields() {
    let analyzer = Analyzer::new();
    let result = analyzer
        .analyze_source(
            "main",
            r#"{{define "main"}}{{template "known" .}}{{template "missing" .}}{{end}}{{define "known"}}K{{end}}"#,
            "{{",
            "}}",
            happ::gotemplates::go_compat::parse::Mode::default(),
            &[],
        )
        .expect("parse must succeed");

    assert_eq!(result.analysis.tree_count, 2);
    assert_eq!(result.analysis.count(NodeType::Template), 2);
    assert_eq!(result.unresolved.len(), 1);
    assert_eq!(result.unresolved[0].called_template, "missing");
}

#[test]
fn go_compat_analyzer_api_supports_custom_delimiters() {
    let analyzer = Analyzer::new();
    let result = analyzer
        .analyze_source(
            "main",
            r#"<<define "main">><<template "missing" .>><<end>>"#,
            "<<",
            ">>",
            happ::gotemplates::go_compat::parse::Mode::default(),
            &[],
        )
        .expect("parse must succeed");

    assert_eq!(result.analysis.count(NodeType::Template), 1);
    assert_eq!(result.unresolved.len(), 1);
    assert_eq!(result.unresolved[0].line, 1);
    assert_eq!(result.unresolved[0].column, 1);
}
