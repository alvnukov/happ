//! The tools an MCP client sees.
//!
//! Deliberately two, not twenty. Every tool's name, description and schema sits
//! in the model's context on *every* request, so a wide surface is a standing
//! token tax and a standing source of near-miss tool choices. Both tools here
//! take an `op` and carry a compact operation table in their description, which
//! costs a fraction of the same operations spelled out as separate tools and
//! keeps related work in one place a model can reason about.
//!
//! - [`helm_apps`] answers about a chart: apps, environments, resolved values,
//!   rendered manifests.
//! - [`code`] answers about source, in any language happ can reach a language
//!   server for -- including helm-apps itself.

pub(crate) mod code;
pub(crate) mod helm_apps;

use serde_json::{json, Value as JsonValue};

use super::protocol::Failure;
use super::ServerContext;

pub(crate) fn catalog() -> Vec<JsonValue> {
    vec![helm_apps::tool(), code::tool()]
}

pub(crate) fn call(context: &ServerContext, params: &JsonValue) -> Result<JsonValue, Failure> {
    let name = params
        .get("name")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| Failure::invalid_params("tools/call requires a 'name'"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outcome = match name {
        helm_apps::NAME => helm_apps::call(context, &args),
        code::NAME => code::call(context, &args),
        _ => {
            return Err(Failure::invalid_params(format!(
                "unknown tool '{name}' -- this server offers '{}' and '{}'",
                helm_apps::NAME,
                code::NAME
            )))
        }
    };

    Ok(match outcome {
        Ok(text) => text_result(text, false),
        Err(message) => text_result(message, true),
    })
}

/// Tool results come back as text, and failures the model could recover from --
/// a path that is not a chart, a symbol that does not exist -- come back as
/// `isError` content rather than as JSON-RPC errors, so the model reads the
/// reason and corrects the call instead of seeing the connection fault.
fn text_result(text: String, is_error: bool) -> JsonValue {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

// --- argument helpers shared by both tools ----------------------------------

pub(crate) fn required_str(args: &JsonValue, key: &str) -> Result<String, String> {
    optional_str(args, key).ok_or_else(|| format!("missing required argument '{key}'"))
}

pub(crate) fn optional_str(args: &JsonValue, key: &str) -> Option<String> {
    args.get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn optional_u32(args: &JsonValue, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(JsonValue::as_u64)
        .map(|value| value as u32)
}

/// Caps an answer, because a rendered chart or a symbol-heavy file can run to
/// thousands of lines and the model pays for every one of them.
pub(crate) fn truncate(text: String, args: &JsonValue, what: &str) -> String {
    const DEFAULT_MAX_LINES: usize = 300;
    let limit = optional_u32(args, "limit")
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_LINES)
        .max(1);

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= limit {
        return text;
    }
    format!(
        "{}\n\n[truncated: showing {limit} of {} {what}; raise 'limit' or narrow the request]",
        lines[..limit].join("\n"),
        lines.len(),
    )
}

pub(crate) fn limit_schema(what: &str) -> JsonValue {
    json!({
        "type": "integer",
        "description": format!("Maximum {what} to return (default 300)."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surface_stays_small_on_purpose() {
        let listed = catalog();
        assert_eq!(
            listed.len(),
            2,
            "every added tool costs context on every request -- add an op instead"
        );
    }

    #[test]
    fn both_tools_document_their_operations_in_their_description() {
        for entry in catalog() {
            let description = entry["description"].as_str().unwrap_or_default();
            let ops = entry["inputSchema"]["properties"]["op"]["enum"]
                .as_array()
                .expect("every tool is op-based");
            for op in ops {
                let op = op.as_str().unwrap_or_default();
                assert!(
                    description.contains(op),
                    "op '{op}' is dispatchable but undocumented in the tool description"
                );
            }
        }
    }

    #[test]
    fn unknown_tools_fail_at_the_protocol_level() {
        let context = ServerContext::for_tests();
        assert!(call(&context, &json!({ "name": "nope" })).is_err());
    }

    #[test]
    fn output_is_capped_and_says_so() {
        let long = (1..=500)
            .map(|n| n.to_string())
            .collect::<Vec<String>>()
            .join("\n");
        let capped = truncate(long, &json!({ "limit": 10 }), "lines");
        assert!(capped.contains("[truncated: showing 10 of 500 lines"));
        assert!(!capped.contains("\n11\n"));
    }

    #[test]
    fn output_under_the_cap_is_left_alone() {
        let short = "one\ntwo".to_string();
        assert_eq!(truncate(short.clone(), &json!({}), "lines"), short);
    }
}
