use crate::go_compat::parserbridge::GoTemplateScanError;

use super::node::ListNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mode(u8);

impl Mode {
    pub const PARSE_COMMENTS: Self = Self(1 << 0);
    pub const SKIP_FUNC_CHECK: Self = Self(1 << 1);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for Mode {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mode {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
    pub offset: usize,
}

impl ParseError {
    pub(crate) fn from_scan(err: GoTemplateScanError) -> Self {
        Self {
            code: err.code,
            message: err.message.to_string(),
            offset: err.offset,
        }
    }

    pub(crate) fn unexpected_eof_for_define() -> Self {
        Self {
            code: "unexpected_eof",
            message: "unexpected EOF while searching matching {{end}} for {{define}}".to_string(),
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub name: String,
    pub parse_name: String,
    pub root: ListNode,
    pub mode: Mode,
    pub text: String,
}

impl Tree {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            parse_name: name.to_string(),
            root: ListNode::default(),
            mode: Mode::default(),
            text: String::new(),
        }
    }

    pub fn copy(&self) -> Self {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::go_compat::parse::node::{Node, TextNode};

    #[test]
    fn tree_copy_preserves_content() {
        let mut tree = Tree::new("main");
        tree.root.nodes.push(Node::Text(TextNode {
            pos: 0,
            text: "x".to_string(),
        }));
        let copied = tree.copy();
        assert_eq!(copied.name, "main");
        assert_eq!(copied.root.to_source(), "x");
    }
}
