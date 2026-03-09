use serde_yaml::Value;

use crate::chart_ir::{decode_ir_documents, ChartIr};
use crate::cli::ImportArgs;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Source(#[from] crate::source::Error),
    #[error(transparent)]
    Ir(#[from] crate::chart_ir::Error),
    #[error("convert: {0}")]
    Convert(String),
}

#[derive(Debug, Clone)]
pub struct ChartAnalysisResult {
    pub ir: ChartIr,
    pub documents: Vec<Value>,
    pub values: Value,
}

pub fn analyze_chart(args: &ImportArgs) -> Result<ChartAnalysisResult, Error> {
    let ir = crate::source::load_chart_ir_for_chart(args)?;
    analyze_chart_ir(args, ir)
}

pub fn analyze_chart_ir(args: &ImportArgs, ir: ChartIr) -> Result<ChartAnalysisResult, Error> {
    let documents = decode_ir_documents(&ir)?;
    let values = crate::convert::build_values(args, &documents).map_err(Error::Convert)?;
    Ok(ChartAnalysisResult {
        ir,
        documents,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart_ir::{ChartIr, ChartIrBackend, ChartIrSource};
    use serde_yaml::{Mapping, Value};

    #[test]
    fn analyze_chart_ir_builds_values_from_ir_documents() {
        let args = ImportArgs {
            path: "/tmp/chart".into(),
            env: "dev".into(),
            group_name: "apps-k8s-manifests".into(),
            group_type: "apps-k8s-manifests".into(),
            min_include_bytes: 24,
            include_status: false,
            output: None,
            out_chart_dir: None,
            chart_name: None,
            library_chart_path: None,
            import_strategy: "raw".into(),
            allow_template_includes: Vec::new(),
            unsupported_template_mode: "error".into(),
            verify_equivalence: false,
            release_name: "imported".into(),
            namespace: None,
            values_files: Vec::new(),
            set_values: Vec::new(),
            set_string_values: Vec::new(),
            set_file_values: Vec::new(),
            set_json_values: Vec::new(),
            kube_version: None,
            api_versions: Vec::new(),
            include_crds: false,
            write_rendered_output: None,
        };
        let mut doc = Mapping::new();
        doc.insert(
            Value::String("apiVersion".into()),
            Value::String("v1".into()),
        );
        doc.insert(
            Value::String("kind".into()),
            Value::String("ConfigMap".into()),
        );
        let mut md = Mapping::new();
        md.insert(Value::String("name".into()), Value::String("demo".into()));
        doc.insert(Value::String("metadata".into()), Value::Mapping(md));
        doc.insert(Value::String("data".into()), Value::Mapping(Mapping::new()));

        let mut ir = ChartIr::new(ChartIrSource {
            backend: ChartIrBackend::RenderedYaml,
            chart_path: Some("/tmp/chart".into()),
            release_name: Some("imported".into()),
        });
        ir.documents
            .push(crate::chart_ir::encode_document(&Value::Mapping(doc)));

        let analyzed = analyze_chart_ir(&args, ir).expect("analyze");
        let txt = serde_yaml::to_string(&analyzed.values).expect("yaml");
        assert!(txt.contains("apps-k8s-manifests"));
        assert!(txt.contains("ConfigMap"));
    }
}
