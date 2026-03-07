use crate::go_compat::parse::{ListNode, Node, NodeType, Tree};
use std::collections::{BTreeMap, BTreeSet};

use super::invocations::unresolved_template_diagnostics;
use super::syntax::extract_template_name;
use super::types::TreeAnalysis;

pub fn analyze_trees(trees: &BTreeMap<String, Tree>) -> TreeAnalysis {
    let mut node_counts = BTreeMap::new();
    let mut max_depth = 0usize;
    let mut template_invocations = BTreeSet::new();

    for tree in trees.values() {
        analyze_list(
            &tree.root,
            0,
            &mut node_counts,
            &mut max_depth,
            &mut template_invocations,
        );
    }

    TreeAnalysis {
        tree_count: trees.len(),
        node_counts,
        max_depth,
        template_invocations,
    }
}

pub fn unresolved_template_invocations(trees: &BTreeMap<String, Tree>) -> BTreeSet<String> {
    unresolved_template_diagnostics(trees)
        .into_iter()
        .map(|d| d.called_template)
        .collect()
}

fn analyze_list(
    list: &ListNode,
    depth: usize,
    node_counts: &mut BTreeMap<NodeType, usize>,
    max_depth: &mut usize,
    template_invocations: &mut BTreeSet<String>,
) {
    *max_depth = (*max_depth).max(depth);

    for node in &list.nodes {
        *node_counts.entry(node.node_type()).or_insert(0) += 1;

        match node {
            Node::If(v) => {
                analyze_list(
                    &v.list,
                    depth + 1,
                    node_counts,
                    max_depth,
                    template_invocations,
                );
                if let Some(else_list) = &v.else_list {
                    analyze_list(
                        else_list,
                        depth + 1,
                        node_counts,
                        max_depth,
                        template_invocations,
                    );
                }
            }
            Node::Range(v) => {
                analyze_list(
                    &v.list,
                    depth + 1,
                    node_counts,
                    max_depth,
                    template_invocations,
                );
                if let Some(else_list) = &v.else_list {
                    analyze_list(
                        else_list,
                        depth + 1,
                        node_counts,
                        max_depth,
                        template_invocations,
                    );
                }
            }
            Node::With(v) => {
                analyze_list(
                    &v.list,
                    depth + 1,
                    node_counts,
                    max_depth,
                    template_invocations,
                );
                if let Some(else_list) = &v.else_list {
                    analyze_list(
                        else_list,
                        depth + 1,
                        node_counts,
                        max_depth,
                        template_invocations,
                    );
                }
            }
            Node::Block(v) => {
                if let Some(name) = extract_template_name(&v.text, "block") {
                    template_invocations.insert(name);
                }
                analyze_list(
                    &v.list,
                    depth + 1,
                    node_counts,
                    max_depth,
                    template_invocations,
                );
            }
            Node::Define(v) => {
                analyze_list(
                    &v.list,
                    depth + 1,
                    node_counts,
                    max_depth,
                    template_invocations,
                );
            }
            Node::Template(v) => {
                if let Some(name) = extract_template_name(&v.text, "template") {
                    template_invocations.insert(name);
                }
            }
            Node::Text(_)
            | Node::Action(_)
            | Node::Else(_)
            | Node::End(_)
            | Node::Break(_)
            | Node::Continue(_)
            | Node::Comment(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::go_compat::parse::parse;
    use crate::go_compat::parse::Mode;

    #[test]
    fn analyze_trees_collects_counts_depth_and_template_calls() {
        let trees = parse(
            "main",
            r#"{{define "sub"}}x{{end}}{{if .x}}{{template "sub" .}}{{else}}{{template "missing" .}}{{end}}"#,
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let analysis = analyze_trees(&trees);
        assert_eq!(analysis.tree_count, 2);
        assert_eq!(analysis.count(NodeType::If), 1);
        assert_eq!(analysis.count(NodeType::Template), 2);
        assert!(analysis.max_depth >= 1);
        assert!(analysis.template_invocations.contains("sub"));
        assert!(analysis.template_invocations.contains("missing"));
    }

    #[test]
    fn unresolved_template_invocations_returns_only_missing_names() {
        let trees = parse(
            "main",
            r#"{{define "main"}}{{template "known" .}}{{template "missing" .}}{{end}}{{define "known"}}K{{end}}"#,
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let unresolved = unresolved_template_invocations(&trees);
        assert!(unresolved.contains("missing"));
        assert!(!unresolved.contains("known"));
        assert!(!unresolved.contains("main"));
    }
}
