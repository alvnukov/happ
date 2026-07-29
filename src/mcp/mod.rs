//! MCP (Model Context Protocol) server mode.
//!
//! Publishes the helm-apps analysis happ already performs for the editor --
//! include expansion, env-map resolution, manifest preview, values diagnostics
//! -- as MCP tools, so a model can navigate a chart by asking questions instead
//! of shelling out to the CLI and re-parsing its output.
//!
//! The tools are deliberately chart-shaped rather than command-shaped: a model
//! that can already run `happ` in a terminal gains nothing from a wrapper
//! around `happ jq`, but it cannot on its own know that `apps-stateless.api` is
//! an app, that `_default` is an env fallback, or that `_include` pulls in a
//! profile defined three files away. Those are the facts the tools answer.

mod bridge;
mod protocol;
mod resources;
mod setup;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;

use bridge::ProviderPool;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("mcp currently supports only stdio transport")]
    UnsupportedTransport,
    #[error("mcp transport error: {0}")]
    Transport(String),
    #[error("mcp: {0}")]
    Setup(String),
}

/// Everything a tool call needs that does not come from its own arguments.
pub(crate) struct ServerContext {
    /// Chart used when a call omits `chart`, from `--chart`.
    pub(crate) default_chart: Option<PathBuf>,
    /// Language servers, started on first use and kept warm afterwards.
    pub(crate) providers: ProviderPool,
}

impl ServerContext {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            default_chart: None,
            providers: ProviderPool::default(),
        }
    }
}

pub fn run(args: crate::cli::McpArgs) -> Result<(), Error> {
    match args.command.as_ref() {
        Some(crate::cli::McpCommand::Setup(setup)) => return setup::run(setup, &args),
        Some(crate::cli::McpCommand::Remove(remove)) => return setup::remove(remove),
        None => {}
    }
    if !args.stdio {
        return Err(Error::UnsupportedTransport);
    }
    crate::process_guard::watch_parent(args.parent_pid);

    let context = ServerContext {
        default_chart: args.chart.as_deref().map(PathBuf::from),
        providers: ProviderPool::new(parse_language_server_overrides(&args.language_servers)?),
    };
    protocol::serve_stdio(&context)
}

/// Reads `--language-server go=/opt/bin/gopls` into a lookup the pool consults
/// before falling back to the registry default.
fn parse_language_server_overrides(raw: &[String]) -> Result<HashMap<String, Vec<String>>, Error> {
    let mut out = HashMap::new();
    for entry in raw {
        let (language, command) = entry.split_once('=').ok_or_else(|| {
            Error::Setup(format!(
                "invalid --language-server '{entry}': expected <language>=<command>"
            ))
        })?;
        let parts: Vec<String> = command
            .split_whitespace()
            .map(ToString::to_string)
            .collect();
        if language.trim().is_empty() || parts.is_empty() {
            return Err(Error::Setup(format!(
                "invalid --language-server '{entry}': both a language and a command are required"
            )));
        }
        out.insert(language.trim().to_string(), parts);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_server_overrides_split_on_the_first_equals() {
        let parsed = parse_language_server_overrides(&["go=/opt/bin/gopls -rpc.trace".to_string()])
            .expect("parse");
        assert_eq!(
            parsed.get("go"),
            Some(&vec![
                "/opt/bin/gopls".to_string(),
                "-rpc.trace".to_string()
            ])
        );
    }

    #[test]
    fn a_malformed_override_is_refused_at_startup() {
        assert!(parse_language_server_overrides(&["gopls".to_string()]).is_err());
        assert!(parse_language_server_overrides(&["go=".to_string()]).is_err());
    }
}
