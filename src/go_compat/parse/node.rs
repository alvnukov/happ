#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeType {
    Text,
    Action,
    Define,
    Block,
    Bool,
    Chain,
    Command,
    Dot,
    Else,
    End,
    Field,
    Identifier,
    If,
    List,
    Nil,
    Number,
    Pipe,
    Range,
    String,
    Template,
    Variable,
    With,
    Comment,
    Break,
    Continue,
}

pub type Pos = usize;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListNode {
    pub nodes: Vec<Node>,
}

impl ListNode {
    pub fn to_source(&self) -> String {
        let mut out = String::new();
        append_list_source(self, &mut out);
        out
    }

    pub fn copy_list(&self) -> Self {
        self.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text(TextNode),
    Action(ActionNode),
    If(IfNode),
    Range(RangeNode),
    With(WithNode),
    Template(TemplateNode),
    Block(BlockNode),
    Define(DefineNode),
    Else(ElseNode),
    End(EndNode),
    Break(BreakNode),
    Continue(ContinueNode),
    Comment(CommentNode),
}

impl Node {
    pub fn node_type(&self) -> NodeType {
        match self {
            Node::Text(_) => NodeType::Text,
            Node::Action(_) => NodeType::Action,
            Node::If(_) => NodeType::If,
            Node::Range(_) => NodeType::Range,
            Node::With(_) => NodeType::With,
            Node::Template(_) => NodeType::Template,
            Node::Block(_) => NodeType::Block,
            Node::Define(_) => NodeType::Define,
            Node::Else(_) => NodeType::Else,
            Node::End(_) => NodeType::End,
            Node::Break(_) => NodeType::Break,
            Node::Continue(_) => NodeType::Continue,
            Node::Comment(_) => NodeType::Comment,
        }
    }

    pub fn position(&self) -> Pos {
        match self {
            Node::Text(n) => n.pos,
            Node::Action(n) => n.pos,
            Node::If(n) => n.pos,
            Node::Range(n) => n.pos,
            Node::With(n) => n.pos,
            Node::Template(n) => n.pos,
            Node::Block(n) => n.pos,
            Node::Define(n) => n.pos,
            Node::Else(n) => n.pos,
            Node::End(n) => n.pos,
            Node::Break(n) => n.pos,
            Node::Continue(n) => n.pos,
            Node::Comment(n) => n.pos,
        }
    }

    pub fn as_source(&self) -> &str {
        match self {
            Node::Text(n) => &n.text,
            Node::Action(n) => &n.text,
            Node::If(n) => &n.text,
            Node::Range(n) => &n.text,
            Node::With(n) => &n.text,
            Node::Template(n) => &n.text,
            Node::Block(n) => &n.text,
            Node::Define(n) => &n.text,
            Node::Else(n) => &n.text,
            Node::End(n) => &n.text,
            Node::Break(n) => &n.text,
            Node::Continue(n) => &n.text,
            Node::Comment(n) => &n.text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfNode {
    pub pos: Pos,
    pub text: String,
    pub list: ListNode,
    pub else_list: Option<ListNode>,
    pub else_action: Option<String>,
    pub end_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeNode {
    pub pos: Pos,
    pub text: String,
    pub list: ListNode,
    pub else_list: Option<ListNode>,
    pub else_action: Option<String>,
    pub end_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithNode {
    pub pos: Pos,
    pub text: String,
    pub list: ListNode,
    pub else_list: Option<ListNode>,
    pub else_action: Option<String>,
    pub end_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefineNode {
    pub pos: Pos,
    pub text: String,
    pub list: ListNode,
    pub end_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockNode {
    pub pos: Pos,
    pub text: String,
    pub list: ListNode,
    pub end_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseNode {
    pub pos: Pos,
    pub text: String,
    pub chained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueNode {
    pub pos: Pos,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentNode {
    pub pos: Pos,
    pub text: String,
}

fn append_list_source(list: &ListNode, out: &mut String) {
    for node in &list.nodes {
        append_node_source(node, out);
    }
}

fn append_node_source(node: &Node, out: &mut String) {
    match node {
        Node::Text(n) => out.push_str(&n.text),
        Node::Action(n) => out.push_str(&n.text),
        Node::Template(n) => out.push_str(&n.text),
        Node::Break(n) => out.push_str(&n.text),
        Node::Continue(n) => out.push_str(&n.text),
        Node::Comment(n) => out.push_str(&n.text),
        Node::Else(n) => out.push_str(&n.text),
        Node::End(n) => out.push_str(&n.text),
        Node::If(n) => {
            out.push_str(&n.text);
            append_list_source(&n.list, out);
            if let Some(else_list) = &n.else_list {
                if let Some(else_action) = &n.else_action {
                    out.push_str(else_action);
                }
                append_list_source(else_list, out);
            }
            append_control_end(&n.end_action, out);
        }
        Node::Range(n) => {
            out.push_str(&n.text);
            append_list_source(&n.list, out);
            if let Some(else_list) = &n.else_list {
                if let Some(else_action) = &n.else_action {
                    out.push_str(else_action);
                }
                append_list_source(else_list, out);
            }
            append_control_end(&n.end_action, out);
        }
        Node::With(n) => {
            out.push_str(&n.text);
            append_list_source(&n.list, out);
            if let Some(else_list) = &n.else_list {
                if let Some(else_action) = &n.else_action {
                    out.push_str(else_action);
                }
                append_list_source(else_list, out);
            }
            append_control_end(&n.end_action, out);
        }
        Node::Define(n) => {
            out.push_str(&n.text);
            append_list_source(&n.list, out);
            out.push_str(n.end_action.as_deref().unwrap_or("{{end}}"));
        }
        Node::Block(n) => {
            out.push_str(&n.text);
            append_list_source(&n.list, out);
            out.push_str(n.end_action.as_deref().unwrap_or("{{end}}"));
        }
    }
}

fn append_control_end(end_action: &Option<String>, out: &mut String) {
    if let Some(end_action) = end_action {
        out.push_str(end_action);
        return;
    }
    out.push_str("{{end}}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_copy_preserves_source() {
        let list = ListNode {
            nodes: vec![
                Node::Text(TextNode {
                    pos: 0,
                    text: "a".to_string(),
                }),
                Node::Action(ActionNode {
                    pos: 1,
                    text: "{{.x}}".to_string(),
                }),
            ],
        };
        let copied = list.copy_list();
        assert_eq!(copied.to_source(), list.to_source());
        assert_eq!(copied.nodes[1].position(), 1);
    }

    #[test]
    fn to_source_reconstructs_structured_if_else() {
        let list = ListNode {
            nodes: vec![Node::If(IfNode {
                pos: 0,
                text: "{{if .x}}".to_string(),
                list: ListNode {
                    nodes: vec![Node::Text(TextNode {
                        pos: 9,
                        text: "A".to_string(),
                    })],
                },
                else_list: Some(ListNode {
                    nodes: vec![Node::Text(TextNode {
                        pos: 20,
                        text: "B".to_string(),
                    })],
                }),
                else_action: Some("{{else}}".to_string()),
                end_action: Some("{{end}}".to_string()),
            })],
        };
        assert_eq!(list.to_source(), "{{if .x}}A{{else}}B{{end}}");
    }
}
