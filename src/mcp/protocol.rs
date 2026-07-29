//! JSON-RPC 2.0 over stdio, framed the way MCP frames it.
//!
//! MCP's stdio transport is newline-delimited JSON, not the `Content-Length`
//! framing LSP uses, so this cannot ride on `lsp_server`. The rules that matter:
//! stdout carries protocol frames and nothing else, one frame per line with no
//! embedded newlines, and a notification (a message without `id`) is never
//! answered.

use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::io::{BufRead, Write};

use super::{resources, tools, Error, ServerContext};

/// Protocol revisions this server implements, newest first.
///
/// A client naming one of these gets it back; anything else is answered with
/// the newest, which is what the spec asks servers to do when they cannot speak
/// the requested revision.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const PARSE_ERROR: i64 = -32700;

pub(crate) fn serve_stdio(context: &ServerContext) -> Result<(), Error> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let lines = stdin.lock().lines();

    for line in lines {
        let line = line.map_err(|err| Error::Transport(format!("read stdin: {err}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_line(context, &line) else {
            continue;
        };
        write_frame(&mut stdout, &response)?;
    }

    Ok(())
}

fn write_frame(out: &mut impl Write, message: &JsonValue) -> Result<(), Error> {
    let encoded = serde_json::to_string(message)
        .map_err(|err| Error::Transport(format!("encode response: {err}")))?;
    writeln!(out, "{encoded}").map_err(|err| Error::Transport(format!("write stdout: {err}")))?;
    out.flush()
        .map_err(|err| Error::Transport(format!("flush stdout: {err}")))
}

/// Answers one frame, or `None` when the frame was a notification.
fn handle_line(context: &ServerContext, line: &str) -> Option<JsonValue> {
    let message: JsonValue = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(err) => {
            return Some(error_response(
                JsonValue::Null,
                PARSE_ERROR,
                format!("invalid JSON frame: {err}"),
            ));
        }
    };

    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(JsonValue::as_str) else {
        // A response to something we never asked for: MCP servers may receive
        // these when a client replies to sampling requests we do not make.
        return None;
    };
    let params = message
        .get("params")
        .cloned()
        .unwrap_or(JsonValue::Object(JsonMap::new()));

    // Notifications carry no id and take no reply -- including
    // `notifications/initialized`, which is the client acknowledging our
    // handshake.
    let id = id.filter(|id| !id.is_null())?;

    Some(match dispatch(context, method, &params) {
        Ok(result) => success_response(id, result),
        Err(failure) => error_response(id, failure.code, failure.message),
    })
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: i64,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_PARAMS,
            message: message.into(),
        }
    }
}

fn dispatch(
    context: &ServerContext,
    method: &str,
    params: &JsonValue,
) -> Result<JsonValue, Failure> {
    match method {
        "initialize" => Ok(initialize_result(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::catalog() })),
        "tools/call" => tools::call(context, params),
        "resources/list" => Ok(json!({ "resources": resources::catalog() })),
        "resources/read" => resources::read(params),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err(Failure {
            code: METHOD_NOT_FOUND,
            message: format!("method not implemented: {method}"),
        }),
    }
}

fn initialize_result(params: &JsonValue) -> JsonValue {
    let requested = params
        .get("protocolVersion")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let version = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|candidate| **candidate == requested)
        .copied()
        .or_else(|| SUPPORTED_PROTOCOL_VERSIONS.first().copied())
        .unwrap_or("2025-06-18");

    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "listChanged": false, "subscribe": false },
        },
        "serverInfo": {
            "name": "happ",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": instructions(),
    })
}

/// Told to the model once, at connect time, so it does not have to infer the
/// call order from tool descriptions alone.
fn instructions() -> String {
    let library = crate::assets::embedded_helm_apps_version().unwrap_or_else(|| "unknown".into());
    format!(
        "happ answers two kinds of question, through two tools.\n\
         \n\
         `helm_apps` reads Helm charts built on the helm-apps library chart (embedded version \
         {library}). Such a chart has no per-app templates: every app is a values entry under an \
         `apps-*` group, for example `apps-stateless.api`, and the library renders it. Reading \
         values.yaml directly is misleading, because `_include` profiles, `_includeFile` \
         references and env maps all resolve at render time -- so use op='resolve' and \
         op='render', not a file read. Start at op='overview'.\n\
         \n\
         `code` answers about source in any language happ can reach a language server for: Go via \
         gopls, Rust via rust-analyzer, and others. It starts the server itself and keeps it warm. \
         Address a symbol by name rather than by position where you can. op='languages' reports \
         what is installed.\n\
         \n\
         Both tools take an `op`, and their descriptions list every operation. The \
         `happ://helm-apps/...` resources carry the library's own template source when you need \
         the contract verbatim."
    )
}

fn success_response(id: JsonValue, result: JsonValue) -> JsonValue {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: JsonValue, code: i64, message: String) -> JsonValue {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ServerContext {
        ServerContext::for_tests()
    }

    fn request(method: &str, params: JsonValue) -> String {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string()
    }

    #[test]
    fn initialize_echoes_a_protocol_version_the_client_asked_for() {
        let response = handle_line(
            &context(),
            &request("initialize", json!({ "protocolVersion": "2024-11-05" })),
        )
        .expect("response");
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "happ");
    }

    #[test]
    fn initialize_falls_back_to_the_newest_supported_version() {
        let response = handle_line(
            &context(),
            &request("initialize", json!({ "protocolVersion": "1988-01-01" })),
        )
        .expect("response");
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn notifications_are_never_answered() {
        let notification =
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
        assert!(handle_line(&context(), &notification).is_none());
    }

    #[test]
    fn unknown_methods_report_method_not_found() {
        let response =
            handle_line(&context(), &request("does/not/exist", json!({}))).expect("response");
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn malformed_frames_report_a_parse_error_instead_of_dropping_the_connection() {
        let response = handle_line(&context(), "{not json").expect("response");
        assert_eq!(response["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn every_tool_is_listed_with_a_schema() {
        let response =
            handle_line(&context(), &request("tools/list", json!({}))).expect("response");
        let listed = response["result"]["tools"].as_array().expect("tools array");
        assert!(!listed.is_empty());
        for tool in listed {
            assert!(tool["name"].is_string(), "tool without a name: {tool}");
            assert!(
                tool["description"].as_str().is_some_and(|d| !d.is_empty()),
                "tool without a description: {tool}"
            );
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn frames_never_contain_a_newline_that_would_split_the_message() {
        let response =
            handle_line(&context(), &request("initialize", json!({}))).expect("response");
        let encoded = serde_json::to_string(&response).expect("encode");
        assert!(!encoded.contains('\n'));
    }
}
