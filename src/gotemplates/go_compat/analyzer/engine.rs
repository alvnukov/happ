use crate::gotemplates::go_compat::parse::{parse, Mode, ParseError, Tree};
use std::collections::BTreeMap;

use super::aggregate::{analyze_trees, unresolved_template_invocations};
use super::invocations::{collect_template_invocation_sites, unresolved_template_diagnostics};
use super::types::{TemplateInvocationSite, TreeAnalysis, UnresolvedTemplateDiagnostic};

#[derive(Debug, Clone, Copy, Default)]
pub struct Analyzer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSetAnalysis {
    pub trees: BTreeMap<String, Tree>,
    pub analysis: TreeAnalysis,
    pub invocation_sites: Vec<TemplateInvocationSite>,
    pub unresolved: Vec<UnresolvedTemplateDiagnostic>,
}

impl Analyzer {
    pub const fn new() -> Self {
        Self
    }

    pub fn analyze_source(
        &self,
        name: &str,
        text: &str,
        left_delim: &str,
        right_delim: &str,
        mode: Mode,
        known_functions: &[&str],
    ) -> Result<TemplateSetAnalysis, ParseError> {
        let trees = parse(
            name,
            text,
            left_delim,
            right_delim,
            mode,
            known_functions,
        )?;
        Ok(self.analyze_parsed(trees))
    }

    pub fn analyze_parsed(&self, trees: BTreeMap<String, Tree>) -> TemplateSetAnalysis {
        let analysis = analyze_trees(&trees);
        let invocation_sites = collect_template_invocation_sites(&trees);
        let unresolved = unresolved_template_diagnostics(&trees);
        TemplateSetAnalysis {
            trees,
            analysis,
            invocation_sites,
            unresolved,
        }
    }

    pub fn analyze_trees(&self, trees: &BTreeMap<String, Tree>) -> TreeAnalysis {
        analyze_trees(trees)
    }

    pub fn collect_template_invocation_sites(
        &self,
        trees: &BTreeMap<String, Tree>,
    ) -> Vec<TemplateInvocationSite> {
        collect_template_invocation_sites(trees)
    }

    pub fn unresolved_template_diagnostics(
        &self,
        trees: &BTreeMap<String, Tree>,
    ) -> Vec<UnresolvedTemplateDiagnostic> {
        unresolved_template_diagnostics(trees)
    }

    pub fn unresolved_template_invocations(
        &self,
        trees: &BTreeMap<String, Tree>,
    ) -> std::collections::BTreeSet<String> {
        unresolved_template_invocations(trees)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_source_collects_counts_and_unresolved() {
        let analyzer = Analyzer::new();
        let result = analyzer
            .analyze_source(
                "main",
                r#"{{define "main"}}{{template "known" .}}{{template "missing" .}}{{end}}{{define "known"}}K{{end}}"#,
                "{{",
                "}}",
                Mode::default(),
                &[],
            )
            .expect("parse must succeed");

        assert_eq!(result.analysis.tree_count, 2);
        assert_eq!(result.analysis.count(crate::gotemplates::go_compat::parse::NodeType::Template), 2);
        assert_eq!(result.unresolved.len(), 1);
        assert_eq!(result.unresolved[0].called_template, "missing");
    }

    #[test]
    fn analyze_parsed_matches_individual_helpers() {
        let analyzer = Analyzer::new();
        let trees = parse(
            "main",
            r#"{{define "main"}}{{template "a" .}}{{end}}{{define "a"}}A{{end}}"#,
            "{{",
            "}}",
            Mode::default(),
            &[],
        )
        .expect("parse must succeed");

        let via_bundle = analyzer.analyze_parsed(trees.clone());
        assert_eq!(via_bundle.analysis, analyze_trees(&trees));
        assert_eq!(
            via_bundle.invocation_sites,
            collect_template_invocation_sites(&trees)
        );
        assert_eq!(
            via_bundle.unresolved,
            unresolved_template_diagnostics(&trees)
        );
    }
}
