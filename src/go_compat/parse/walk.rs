use super::node::{ListNode, Node};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkControl {
    Continue,
    SkipChildren,
    Stop,
}

pub fn walk_list<F>(list: &ListNode, visitor: &mut F)
where
    F: FnMut(&Node) -> WalkControl,
{
    let _ = walk_list_inner(list, visitor);
}

fn walk_list_inner<F>(list: &ListNode, visitor: &mut F) -> bool
where
    F: FnMut(&Node) -> WalkControl,
{
    for node in &list.nodes {
        match visitor(node) {
            WalkControl::Stop => return true,
            WalkControl::SkipChildren => continue,
            WalkControl::Continue => {}
        }

        let stopped = match node {
            Node::If(v) => {
                walk_list_inner(&v.list, visitor)
                    || v.else_list
                        .as_ref()
                        .is_some_and(|else_list| walk_list_inner(else_list, visitor))
            }
            Node::Range(v) => {
                walk_list_inner(&v.list, visitor)
                    || v.else_list
                        .as_ref()
                        .is_some_and(|else_list| walk_list_inner(else_list, visitor))
            }
            Node::With(v) => {
                walk_list_inner(&v.list, visitor)
                    || v.else_list
                        .as_ref()
                        .is_some_and(|else_list| walk_list_inner(else_list, visitor))
            }
            Node::Block(v) => walk_list_inner(&v.list, visitor),
            Node::Define(v) => walk_list_inner(&v.list, visitor),
            Node::Text(_)
            | Node::Action(_)
            | Node::Template(_)
            | Node::Else(_)
            | Node::End(_)
            | Node::Break(_)
            | Node::Continue(_)
            | Node::Comment(_) => false,
        };

        if stopped {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::go_compat::parse::{parse, Mode};

    #[test]
    fn walk_list_visits_nested_nodes_in_preorder() {
        let trees = parse(
            "main",
            r#"{{if .a}}A{{range .r}}{{continue}}{{end}}{{else}}B{{end}}"#,
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");
        let main = trees.get("main").expect("main tree");

        let mut kinds = Vec::new();
        walk_list(&main.root, &mut |node| {
            kinds.push(node.node_type());
            WalkControl::Continue
        });

        use crate::go_compat::parse::NodeType;
        assert_eq!(kinds.first().copied(), Some(NodeType::If));
        assert!(kinds.contains(&NodeType::Range));
        assert!(kinds.contains(&NodeType::Continue));
        assert!(kinds.contains(&NodeType::Text));
    }

    #[test]
    fn walk_list_stop_short_circuits() {
        let trees = parse(
            "main",
            r#"a{{if .x}}b{{end}}c"#,
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");
        let main = trees.get("main").expect("main tree");

        let mut visited = 0usize;
        walk_list(&main.root, &mut |_node| {
            visited += 1;
            WalkControl::Stop
        });

        assert_eq!(visited, 1);
    }
}
