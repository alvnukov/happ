use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::borrow::Cow;

pub const CHART_IR_VERSION: &str = "v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartIr {
    pub version: String,
    pub source: ChartIrSource,
    pub documents: Vec<ChartIrDocument>,
    pub diagnostics: Vec<ChartIrDiagnostic>,
}

impl ChartIr {
    pub fn new(source: ChartIrSource) -> Self {
        Self {
            version: CHART_IR_VERSION.to_string(),
            source,
            documents: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartIrSource {
    pub backend: ChartIrBackend,
    pub chart_path: Option<String>,
    pub release_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChartIrBackend {
    RenderedYaml,
    HelmGoFfi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartIrDocument {
    pub identity: ChartIrIdentity,
    pub body: IrNode,
    pub provenance: Option<ChartIrProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChartIrIdentity {
    pub api_version: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChartIrProvenance {
    pub template_file: Option<String>,
    pub include_chain: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChartIrDiagnostic {
    pub severity: ChartIrSeverity,
    pub code: String,
    pub message: String,
    pub document_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChartIrSeverity {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum IrNode {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Seq(Vec<IrNode>),
    Map(Vec<IrMapEntry>),
    Tagged(IrTagged),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IrMapEntry {
    pub key: String,
    pub value: IrNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IrTagged {
    pub tag: String,
    pub value: Box<IrNode>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid document body type: expected map at document index {document_index}")]
    InvalidDocumentBody { document_index: usize },
}

pub fn encode_document(doc: &Value) -> ChartIrDocument {
    ChartIrDocument {
        identity: extract_identity(doc),
        body: encode_node(doc),
        provenance: None,
    }
}

pub fn decode_ir_documents(ir: &ChartIr) -> Result<Vec<Value>, Error> {
    let mut out = Vec::with_capacity(ir.documents.len());
    for (idx, doc) in ir.documents.iter().enumerate() {
        let decoded = decode_node(&doc.body);
        if !decoded.is_mapping() {
            return Err(Error::InvalidDocumentBody {
                document_index: idx,
            });
        }
        out.push(decoded);
    }
    Ok(out)
}

pub fn encode_documents(docs: &[Value]) -> Vec<ChartIrDocument> {
    docs.iter().map(encode_document).collect()
}

pub fn encode_node(node: &Value) -> IrNode {
    match node {
        Value::Null => IrNode::Null,
        Value::Bool(v) => IrNode::Bool(*v),
        Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                IrNode::Int(i)
            } else if let Some(u) = v.as_u64() {
                IrNode::Uint(u)
            } else if let Some(f) = v.as_f64() {
                IrNode::Float(f)
            } else {
                IrNode::String(v.to_string())
            }
        }
        Value::String(v) => IrNode::String(v.clone()),
        Value::Sequence(items) => IrNode::Seq(items.iter().map(encode_node).collect()),
        Value::Mapping(items) => IrNode::Map(
            items
                .iter()
                .map(|(k, v)| IrMapEntry {
                    key: normalize_key(k).into_owned(),
                    value: encode_node(v),
                })
                .collect(),
        ),
        Value::Tagged(tagged) => IrNode::Tagged(IrTagged {
            tag: tagged.tag.to_string(),
            value: Box::new(encode_node(&tagged.value)),
        }),
    }
}

pub fn decode_node(node: &IrNode) -> Value {
    match node {
        IrNode::Null => Value::Null,
        IrNode::Bool(v) => Value::Bool(*v),
        IrNode::Int(v) => Value::Number((*v).into()),
        IrNode::Uint(v) => Value::Number((*v).into()),
        IrNode::Float(v) => {
            serde_yaml::to_value(*v).unwrap_or_else(|_| Value::String(v.to_string()))
        }
        IrNode::String(v) => Value::String(v.clone()),
        IrNode::Seq(items) => Value::Sequence(items.iter().map(decode_node).collect()),
        IrNode::Map(items) => {
            let mut out = Mapping::new();
            for item in items {
                out.insert(Value::String(item.key.clone()), decode_node(&item.value));
            }
            Value::Mapping(out)
        }
        IrNode::Tagged(tagged) => Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
            tag: serde_yaml::value::Tag::new(tagged.tag.clone()),
            value: decode_node(&tagged.value),
        })),
    }
}

fn extract_identity(doc: &Value) -> ChartIrIdentity {
    let Some(map) = doc.as_mapping() else {
        return ChartIrIdentity {
            api_version: None,
            kind: None,
            name: None,
            namespace: None,
        };
    };
    let metadata = map
        .get(Value::String("metadata".to_string()))
        .and_then(Value::as_mapping);
    ChartIrIdentity {
        api_version: get_map_string(map, "apiVersion"),
        kind: get_map_string(map, "kind"),
        name: metadata.and_then(|m| get_map_string(m, "name")),
        namespace: metadata.and_then(|m| get_map_string(m, "namespace")),
    }
}

fn get_map_string(map: &Mapping, key: &str) -> Option<String> {
    map.get(Value::String(key.to_string()))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn normalize_key(key: &Value) -> Cow<'_, str> {
    if let Some(s) = key.as_str() {
        return Cow::Borrowed(s);
    }
    match key {
        Value::Null => Cow::Borrowed("null"),
        Value::Bool(v) => Cow::Owned(v.to_string()),
        Value::Number(v) => Cow::Owned(v.to_string()),
        Value::String(v) => Cow::Borrowed(v),
        Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            let rendered = serde_yaml::to_string(key)
                .unwrap_or_else(|_| "<non-string-key>".to_string())
                .replace('\n', " ");
            Cow::Owned(rendered.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip_preserves_identity_and_body() {
        let src: Value = serde_yaml::from_str(
            r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: demo
spec:
  replicas: 2
"#,
        )
        .expect("yaml");
        let encoded = encode_document(&src);
        assert_eq!(encoded.identity.kind.as_deref(), Some("Deployment"));
        assert_eq!(encoded.identity.name.as_deref(), Some("web"));

        let mut ir = ChartIr::new(ChartIrSource {
            backend: ChartIrBackend::RenderedYaml,
            chart_path: None,
            release_name: None,
        });
        ir.documents.push(encoded);
        let docs = decode_ir_documents(&ir).expect("decode");
        assert_eq!(docs.len(), 1);
        assert_eq!(
            docs[0].get("kind").and_then(Value::as_str).expect("kind"),
            "Deployment"
        );
    }

    #[test]
    fn decode_ir_documents_rejects_non_mapping_documents() {
        let mut ir = ChartIr::new(ChartIrSource {
            backend: ChartIrBackend::RenderedYaml,
            chart_path: None,
            release_name: None,
        });
        ir.documents.push(ChartIrDocument {
            identity: ChartIrIdentity {
                api_version: None,
                kind: None,
                name: None,
                namespace: None,
            },
            body: IrNode::String("x".into()),
            provenance: None,
        });
        let err = decode_ir_documents(&ir).expect_err("must fail");
        assert!(matches!(
            err,
            Error::InvalidDocumentBody { document_index: 0 }
        ));
    }
}
