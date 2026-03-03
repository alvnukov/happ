use serde_json::Value as JsonValue;

pub type Error = zq::QueryError;

#[allow(dead_code)]
pub fn run_json_query(query: &str, input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::run_native_json_query(query, input)
}

#[allow(dead_code)]
pub fn run_yaml_query(query: &str, input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::run_native_yaml_query(query, input)
}

pub fn run_query_stream(
    query: &str,
    input_stream: Vec<JsonValue>,
) -> Result<Vec<JsonValue>, Error> {
    zq::run_native_query_stream(query, input_stream)
}

pub fn parse_input_docs_prefer_json(input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::parse_native_input_docs_prefer_json(input)
}

pub fn parse_input_docs_prefer_yaml(input: &str) -> Result<Vec<JsonValue>, Error> {
    zq::parse_native_input_docs_prefer_yaml(input)
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
}
