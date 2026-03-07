use crate::go_compat::parse::NodeType;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeAnalysis {
    pub tree_count: usize,
    pub node_counts: BTreeMap<NodeType, usize>,
    pub max_depth: usize,
    pub template_invocations: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateInvocationSite {
    pub tree_name: String,
    pub action_text: String,
    pub called_template: Option<String>,
    pub from_block: bool,
    pub pos: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedTemplateDiagnostic {
    pub tree_name: String,
    pub called_template: String,
    pub action_text: String,
    pub pos: usize,
    pub line: usize,
    pub column: usize,
}

impl TreeAnalysis {
    pub fn count(&self, node_type: NodeType) -> usize {
        self.node_counts.get(&node_type).copied().unwrap_or(0)
    }
}
