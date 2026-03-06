use super::*;

#[test]
fn parse_splits_define_into_tree_set() {
    let trees = parse(
        "main",
        r#"left {{define "x"}}X{{end}} right"#,
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let main = trees.get("main").expect("main tree");
    let x = trees.get("x").expect("x tree");
    assert_eq!(main.root.to_source(), "left  right");
    assert_eq!(x.root.to_source(), "X");
}

#[test]
fn parse_accepts_custom_delimiters_and_normalizes_actions() {
    let trees = parse(
        "main",
        "<<if .x>>A<<else>>B<<end>>",
        "<<",
        ">>",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(main.root.to_source(), "{{if .x}}A{{else}}B{{end}}");
}

#[test]
fn parse_uses_default_delimiters_when_empty_delims_passed() {
    let trees = parse("main", "{{if .x}}A{{end}}", "", "", Mode::default(), &[])
        .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(main.root.to_source(), "{{if .x}}A{{end}}");
}

#[test]
fn parse_mode_skip_func_check_matches_go_behavior() {
    let strict = parse("main", "{{foo}}", "{{", "}}", Mode::default(), &[]);
    assert!(strict.is_err());
    assert_eq!(
        strict.expect_err("strict mode must fail").code,
        "undefined_function"
    );

    let skip = parse("main", "{{foo}}", "{{", "}}", Mode::SKIP_FUNC_CHECK, &[]);
    assert!(
        skip.is_ok(),
        "skip function check mode must accept unknown function"
    );
}

#[test]
fn parse_comments_mode_keeps_comment_nodes() {
    let trees = parse(
        "main",
        r#"a{{/* x */}}b"#,
        "{{",
        "}}",
        Mode::PARSE_COMMENTS,
        &[],
    )
    .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(main.root.nodes.len(), 3);
    assert!(matches!(main.root.nodes[1], Node::Comment(_)));
}

#[test]
fn default_mode_drops_comment_nodes() {
    let trees = parse("main", r#"a{{/* x */}}b"#, "{{", "}}", Mode::default(), &[])
        .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(main.root.nodes.len(), 2);
    assert!(matches!(main.root.nodes[0], Node::Text(_)));
    assert!(matches!(main.root.nodes[1], Node::Text(_)));
}

#[test]
fn parse_classifies_control_and_template_nodes() {
    let src = r#"{{if .x}}a{{else}}b{{end}}{{template "x" .}}{{range .r}}{{break}}{{continue}}{{end}}{{with .w}}{{end}}{{block "b" .}}x{{end}}"#;
    let trees = parse("main", src, "{{", "}}", Mode::default(), &[]).expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    let b = trees.get("b").expect("block tree");

    assert!(main.root.nodes.iter().any(|n| matches!(n, Node::If(_))));
    assert!(main
        .root
        .nodes
        .iter()
        .any(|n| matches!(n, Node::Template(_))));
    assert!(main.root.nodes.iter().any(|n| matches!(n, Node::Range(_))));
    assert!(main.root.nodes.iter().any(|n| matches!(n, Node::With(_))));
    assert!(
        !main.root.nodes.iter().any(|n| matches!(n, Node::Block(_))),
        "block must be rewritten into template + define tree like Go"
    );

    let if_node = main
        .root
        .nodes
        .iter()
        .find_map(|n| match n {
            Node::If(v) => Some(v),
            _ => None,
        })
        .expect("if node");
    assert!(!if_node.list.nodes.is_empty());
    assert!(if_node.else_list.is_some());

    let range_node = main
        .root
        .nodes
        .iter()
        .find_map(|n| match n {
            Node::Range(v) => Some(v),
            _ => None,
        })
        .expect("range node");
    assert!(range_node
        .list
        .nodes
        .iter()
        .any(|n| matches!(n, Node::Break(_))));
    assert!(range_node
        .list
        .nodes
        .iter()
        .any(|n| matches!(n, Node::Continue(_))));

    assert_eq!(b.root.to_source(), "x");
    assert!(main.root.to_source().contains("{{template \"b\" .}}"));
    assert!(!main.root.to_source().contains("{{block"));
}

#[test]
fn parse_else_if_is_nested_if_in_else_list() {
    let src = r#"{{if .a}}A{{else if .b}}B{{else}}C{{end}}"#;
    let trees = parse("main", src, "{{", "}}", Mode::default(), &[]).expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(
        main.root.to_source(),
        "{{if .a}}A{{else}}{{if .b}}B{{else}}C{{end}}{{end}}"
    );
    let if_node = main
        .root
        .nodes
        .iter()
        .find_map(|n| match n {
            Node::If(v) => Some(v),
            _ => None,
        })
        .expect("if node");
    let else_list = if_node.else_list.as_ref().expect("else list");
    assert_eq!(else_list.nodes.len(), 1);
    let nested = match &else_list.nodes[0] {
        Node::If(v) => v,
        other => panic!("expected nested if node, got {other:?}"),
    };
    assert_eq!(nested.list.to_source(), "B");
    assert_eq!(
        nested.else_list.as_ref().expect("nested else").to_source(),
        "C"
    );
}

#[test]
fn parse_rewrites_block_to_template_and_extracts_tree() {
    let trees = parse(
        "main",
        r#"{{block "x" .}}BODY{{end}}"#,
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");

    let main = trees.get("main").expect("main tree");
    let x = trees.get("x").expect("x tree");
    assert_eq!(main.root.to_source(), r#"{{template "x" .}}"#);
    assert_eq!(x.root.to_source(), "BODY");
}

#[test]
fn block_rewrite_keeps_pipeline_tail() {
    let trees = parse(
        "main",
        r#"{{block "x" (print .a)}}{{end}}"#,
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    assert_eq!(main.root.to_source(), r#"{{template "x" (print .a)}}"#);
}

#[test]
fn block_rewrite_decodes_escaped_name_literal() {
    let trees = parse(
        "main",
        r#"{{block "\x78" .}}BODY{{end}}"#,
        "{{",
        "}}",
        Mode::default(),
        &[],
    )
    .expect("parse must succeed");
    let main = trees.get("main").expect("main tree");
    let x = trees.get("x").expect("decoded x tree");

    assert_eq!(main.root.to_source(), r#"{{template "x" .}}"#);
    assert_eq!(x.root.to_source(), "BODY");
}
