use crate::gotemplates::go_compat::parse::{ListNode, Node, Tree};
use std::collections::{BTreeMap, BTreeSet};

use super::syntax::{extract_template_name, offset_to_line_col};
use super::types::{TemplateInvocationSite, UnresolvedTemplateDiagnostic};

pub fn collect_template_invocation_sites(
    trees: &BTreeMap<String, Tree>,
) -> Vec<TemplateInvocationSite> {
    let mut out = Vec::new();
    for tree in trees.values() {
        let source = tree.root.to_source();
        collect_template_invocations_in_list(&tree.name, &source, &tree.root, &mut out);
    }
    out.sort_by(|a, b| {
        a.tree_name
            .cmp(&b.tree_name)
            .then_with(|| a.pos.cmp(&b.pos))
            .then_with(|| a.action_text.cmp(&b.action_text))
    });
    out
}

pub fn unresolved_template_diagnostics(
    trees: &BTreeMap<String, Tree>,
) -> Vec<UnresolvedTemplateDiagnostic> {
    let defined: BTreeSet<String> = trees.keys().cloned().collect();
    let mut out: Vec<UnresolvedTemplateDiagnostic> = collect_template_invocation_sites(trees)
        .into_iter()
        .filter_map(|site| {
            let name = site.called_template?;
            if defined.contains(&name) {
                return None;
            }
            Some(UnresolvedTemplateDiagnostic {
                tree_name: site.tree_name,
                called_template: name,
                action_text: site.action_text,
                pos: site.pos,
                line: site.line,
                column: site.column,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.tree_name
            .cmp(&b.tree_name)
            .then_with(|| a.pos.cmp(&b.pos))
            .then_with(|| a.called_template.cmp(&b.called_template))
    });
    out
}

fn collect_template_invocations_in_list(
    tree_name: &str,
    source: &str,
    list: &ListNode,
    out: &mut Vec<TemplateInvocationSite>,
) {
    for node in &list.nodes {
        match node {
            Node::Template(v) => {
                let called_template = extract_template_name(&v.text, "template");
                let (line, column) = offset_to_line_col(source, v.pos);
                out.push(TemplateInvocationSite {
                    tree_name: tree_name.to_string(),
                    action_text: v.text.clone(),
                    called_template,
                    from_block: false,
                    pos: v.pos,
                    line,
                    column,
                });
            }
            Node::Block(v) => {
                let called_template = extract_template_name(&v.text, "block");
                let (line, column) = offset_to_line_col(source, v.pos);
                out.push(TemplateInvocationSite {
                    tree_name: tree_name.to_string(),
                    action_text: v.text.clone(),
                    called_template,
                    from_block: true,
                    pos: v.pos,
                    line,
                    column,
                });
                collect_template_invocations_in_list(tree_name, source, &v.list, out);
            }
            Node::If(v) => {
                collect_template_invocations_in_list(tree_name, source, &v.list, out);
                if let Some(else_list) = &v.else_list {
                    collect_template_invocations_in_list(tree_name, source, else_list, out);
                }
            }
            Node::Range(v) => {
                collect_template_invocations_in_list(tree_name, source, &v.list, out);
                if let Some(else_list) = &v.else_list {
                    collect_template_invocations_in_list(tree_name, source, else_list, out);
                }
            }
            Node::With(v) => {
                collect_template_invocations_in_list(tree_name, source, &v.list, out);
                if let Some(else_list) = &v.else_list {
                    collect_template_invocations_in_list(tree_name, source, else_list, out);
                }
            }
            Node::Define(v) => {
                collect_template_invocations_in_list(tree_name, source, &v.list, out)
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
    use crate::gotemplates::go_compat::parse::{parse, Mode};

    #[test]
    fn collect_template_invocation_sites_reports_positions_and_kinds() {
        let trees = parse(
            "main",
            "{{define \"main\"}}\n{{template \"missing\" .}}\n{{block \"blk\" .}}x{{end}}\n{{end}}",
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let sites = collect_template_invocation_sites(&trees);
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].tree_name, "main");
        assert_eq!(sites[0].called_template.as_deref(), Some("missing"));
        assert!(!sites[0].from_block);
        assert_eq!(sites[0].line, 2);
        assert_eq!(sites[0].column, 1);

        assert_eq!(sites[1].called_template.as_deref(), Some("blk"));
        assert!(!sites[1].from_block);
        assert_eq!(sites[1].line, 3);
        assert_eq!(sites[1].column, 1);
    }

    #[test]
    fn unresolved_template_diagnostics_preserve_location() {
        let trees = parse(
            "main",
            "{{define \"main\"}}\n{{template \"known\" .}}\n{{template \"missing\" .}}\n{{end}}{{define \"known\"}}K{{end}}",
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let diagnostics = unresolved_template_diagnostics(&trees);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].called_template, "missing");
        assert_eq!(diagnostics[0].tree_name, "main");
        assert_eq!(diagnostics[0].line, 3);
        assert_eq!(diagnostics[0].column, 1);
    }

    #[test]
    fn unresolved_template_diagnostics_use_byte_column_offsets() {
        let trees = parse(
            "main",
            "{{define \"main\"}}\nй{{template \"missing\" .}}\n{{end}}",
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let diagnostics = unresolved_template_diagnostics(&trees);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].column, 3);
    }
}
