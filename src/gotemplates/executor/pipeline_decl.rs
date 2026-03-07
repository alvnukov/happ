use crate::gotemplates::go_compat::pipeline_decl::{
    extract_pipeline_declaration as go_extract_pipeline_declaration,
    PipelineDeclMode as GoPipelineDeclMode, PipelineDeclaration as GoPipelineDeclaration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PipelineDeclMode {
    Declare,
    Assign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PipelineDeclaration {
    pub(super) names: Vec<String>,
    pub(super) mode: PipelineDeclMode,
}

pub(super) fn extract_pipeline_declaration(expr: &str) -> (Option<PipelineDeclaration>, String) {
    let (decl, runtime_expr) = go_extract_pipeline_declaration(expr);
    let decl = decl.map(map_decl_from_go);
    (decl, runtime_expr)
}

fn map_decl_from_go(decl: GoPipelineDeclaration) -> PipelineDeclaration {
    PipelineDeclaration {
        names: decl.names,
        mode: match decl.mode {
            GoPipelineDeclMode::Declare => PipelineDeclMode::Declare,
            GoPipelineDeclMode::Assign => PipelineDeclMode::Assign,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_pipeline_declaration, PipelineDeclMode};

    #[test]
    fn extracts_single_var_declaration() {
        let (decl, runtime) = extract_pipeline_declaration("$x := printf \"%s\" .v | quote");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$x".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Declare);
        assert_eq!(runtime, "printf \"%s\" .v | quote");
    }

    #[test]
    fn extracts_range_style_two_var_assignment() {
        let (decl, runtime) = extract_pipeline_declaration("$i, $v = range .items");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$i".to_string(), "$v".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Assign);
        assert_eq!(runtime, "range .items");
    }

    #[test]
    fn extracts_declarations_without_spaces_around_operators() {
        let (decl, runtime) = extract_pipeline_declaration("$x:=printf \"%s\" .v|quote");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$x".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Declare);
        assert_eq!(runtime, "printf \"%s\" .v | quote");

        let (decl, runtime) = extract_pipeline_declaration("$i,$v=range .items");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$i".to_string(), "$v".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Assign);
        assert_eq!(runtime, "range .items");
    }

    #[test]
    fn keeps_expression_without_declaration() {
        let (decl, runtime) = extract_pipeline_declaration("printf \"%s\" .v");
        assert!(decl.is_none());
        assert_eq!(runtime, "printf \"%s\" .v");
    }
}
