//! happ answering for helm-apps, without spawning anything.
//!
//! The helm-apps language server is this binary. Routing its requests through a
//! child process would cost a fork and a handshake to reach code already
//! linked in, so this provider calls [`crate::lsp::dispatch_request`] directly
//! -- the exact function an editor's requests land in.

use serde_json::Value as JsonValue;
use std::path::Path;

use super::{Diagnostic, LspProvider};

#[derive(Default)]
pub(crate) struct HappProvider {
    state: crate::lsp::ServerState,
}

impl LspProvider for HappProvider {
    fn request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        crate::lsp::dispatch_request(&self.state, method, &params)
            .map_err(|failure| failure.message)
    }

    fn open(&mut self, _path: &Path) -> Result<(), String> {
        // happ's request handlers read the file themselves when the caller
        // names a URI they have no buffer for, so there is nothing to sync.
        Ok(())
    }

    fn diagnostics(&mut self, path: &Path) -> Result<Vec<Diagnostic>, String> {
        let source = crate::lsp::locate_chart_values(path)?;
        Ok(crate::lsp::analysis_diagnostics(&source)?
            .iter()
            .map(|raw| Diagnostic {
                line: raw["line"].as_u64().unwrap_or(1) as u32,
                column: raw["column"].as_u64().unwrap_or(1) as u32,
                severity: raw["severity"].as_str().unwrap_or("info").to_string(),
                code: raw["code"].as_str().map(ToString::to_string),
                message: raw["message"].as_str().unwrap_or_default().to_string(),
            })
            .collect())
    }

    fn supported_methods(&self) -> Vec<String> {
        let mut methods: Vec<String> = crate::lsp::SUPPORTED_REQUESTS
            .iter()
            .map(|method| (*method).to_string())
            .collect();
        methods.push("textDocument/publishDiagnostics".to_string());
        methods
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chart_with(values: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\n",
        )
        .expect("chart");
        std::fs::write(dir.path().join("values.yaml"), values).expect("values");
        dir
    }

    #[test]
    fn requests_reach_the_same_handlers_an_editor_uses() {
        let chart = chart_with("global:\n  env: dev\napps-stateless:\n  api:\n    enabled: true\n");
        let uri = super::super::path_to_uri(&chart.path().join("values.yaml")).expect("uri");

        let mut provider = HappProvider::default();
        let result = provider
            .request("happ/listEntities", json!({ "uri": uri }))
            .expect("listEntities");

        assert_eq!(result["groups"][0]["name"], "apps-stateless");
        assert_eq!(result["enabledEntities"][0]["app"], "api");
    }

    #[test]
    fn an_unknown_method_is_refused_rather_than_answered_emptily() {
        let mut provider = HappProvider::default();
        let err = provider
            .request("textDocument/definition", json!({}))
            .err()
            .expect("unsupported method must fail");
        assert!(err.contains("not implemented"), "{err}");
    }

    #[test]
    fn diagnostics_come_back_for_a_chart_that_breaks_the_contract() {
        let chart = chart_with("global:\n  env: dev\napps-statelss:\n  api:\n    enabled: true\n");
        let mut provider = HappProvider::default();
        let found = provider
            .diagnostics(&chart.path().join("values.yaml"))
            .expect("diagnostics");
        assert!(
            found
                .iter()
                .any(|d| d.code.as_deref() == Some("E_UNKNOWN_APPS_GROUP")),
            "{found:?}"
        );
    }
}
