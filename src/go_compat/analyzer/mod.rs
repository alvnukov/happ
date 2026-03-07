mod aggregate;
mod engine;
mod invocations;
mod syntax;
mod types;

pub use aggregate::{analyze_trees, unresolved_template_invocations};
pub use engine::{Analyzer, TemplateSetAnalysis};
pub use invocations::{collect_template_invocation_sites, unresolved_template_diagnostics};
pub use types::{TemplateInvocationSite, TreeAnalysis, UnresolvedTemplateDiagnostic};
