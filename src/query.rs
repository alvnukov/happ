use serde_json::Value as JsonValue;

pub type Error = zq::QueryError;

fn map_engine_error(err: zq::EngineError) -> Error {
    match err {
        zq::EngineError::Query(inner) => inner,
        zq::EngineError::OutputEncode(msg) | zq::EngineError::OutputYamlEncode(msg) => {
            Error::Runtime(msg)
        }
        other => Error::Unsupported(other.to_string()),
    }
}

fn run_single_doc_query(query: &str, input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::run_jq(
        query,
        input,
        zq::QueryOptions {
            doc_mode: zq::DocMode::First,
            library_path: Vec::new(),
        },
    )
    .map_err(map_engine_error)
}

#[allow(dead_code)]
pub fn run_json_query(query: &str, input: &str) -> Result<Vec<JsonValue>, Error> {
    run_single_doc_query(query, input)
}

#[allow(dead_code)]
pub fn run_yaml_query(query: &str, input: &str) -> Result<Vec<JsonValue>, Error> {
    run_single_doc_query(query, input)
}

pub fn run_query_stream(
    query: &str,
    input_stream: Vec<JsonValue>,
) -> Result<Vec<JsonValue>, Error> {
    zq::run_jq_stream_with_paths_options(query, input_stream, &[], zq::EngineRunOptions::default())
        .map_err(map_engine_error)
}

pub fn parse_input_docs_prefer_json(input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::parse_native_input_docs_prefer_json(input)
}

pub fn parse_input_docs_prefer_yaml(input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::parse_native_input_docs_prefer_yaml(input)
}

pub fn parse_doc_mode(doc_mode: &str, doc_index: Option<usize>) -> Result<zq::DocMode, Error> {
    zq::parse_doc_mode(doc_mode, doc_index).map_err(map_engine_error)
}

pub fn format_query_error(tool: &str, query: &str, input: &str, err: &Error) -> String {
    zq::format_query_error_with_sources(tool, query, input, err)
}

pub fn format_output_json_lines(
    values: &[JsonValue],
    compact: bool,
    raw_output: bool,
) -> Result<String, Error> {
    zq::format_output_json_lines(values, compact, raw_output).map_err(map_engine_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yaml_multi_doc() {
        let input = "a: 1\n---\na: 2\n";
        let docs = parse_input_docs_prefer_yaml(input).expect("parse");
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn run_identity_query() {
        let input = vec![serde_json::json!({"a": 1})];
        let out = run_query_stream(".", input).expect("query");
        assert_eq!(out, vec![serde_json::json!({"a": 1})]);
    }

    #[test]
    fn run_single_doc_helpers_are_equivalent() {
        let input = r#"{"a":1}"#;
        let from_json = run_json_query(".", input).expect("json helper");
        let from_yaml = run_yaml_query(".", input).expect("yaml helper");
        assert_eq!(from_json, from_yaml);
    }
}
