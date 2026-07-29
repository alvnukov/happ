//! One language-intelligence surface over many language servers.
//!
//! happ already speaks LSP for helm-apps values files. Rather than expose that
//! one server and stop there, the bridge treats it as the first of many
//! providers: helm-apps is answered in-process, and any other language is
//! answered by the server that language's ecosystem already ships -- `gopls`,
//! `rust-analyzer`, and so on -- spawned on demand and driven over LSP stdio.
//!
//! A model therefore learns one set of operations and gets Go, Rust and
//! helm-apps from it. Adding a language is a table entry in [`registry`], not a
//! new tool.

mod child;
mod inproc;
pub(crate) mod registry;

use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) use registry::{detect_language, LanguageSpec};

/// What every provider must answer, whether it lives in this process or not.
pub(crate) trait LspProvider {
    /// Sends an LSP request and waits for its response.
    fn request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String>;

    /// Makes a document's current text known to the server. In-process
    /// providers that read from disk may treat this as a no-op.
    fn open(&mut self, path: &Path) -> Result<(), String>;

    /// Diagnostics for one file. Kept off `request` because servers publish
    /// them as notifications rather than answering a request for them.
    fn diagnostics(&mut self, path: &Path) -> Result<Vec<Diagnostic>, String>;

    /// LSP methods this provider can actually answer, for capability reporting.
    fn supported_methods(&self) -> Vec<String>;

    /// What the server has complained about, if anything.
    ///
    /// A server that cannot load a workspace still answers every request --
    /// emptily. Without this, "no symbols here" is indistinguishable from
    /// "this file is empty", and the caller believes the wrong one.
    fn health(&self) -> Option<String> {
        None
    }
}

/// A finding reported against a file, normalised across providers.
#[derive(Debug, Clone)]
pub(crate) struct Diagnostic {
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) severity: String,
    pub(crate) code: Option<String>,
    pub(crate) message: String,
}

/// Which server answers for a workspace, keyed so two Go projects open side by
/// side get one `gopls` each rather than sharing one with the wrong root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderKey {
    language: String,
    root: PathBuf,
}

/// Lazily started providers, reused across tool calls.
///
/// Starting `gopls` costs seconds and indexing costs more, so the first call
/// for a project pays and every later one does not. Providers live until the
/// MCP server exits.
#[derive(Default)]
pub(crate) struct ProviderPool {
    providers: RefCell<HashMap<ProviderKey, Box<dyn LspProvider>>>,
    /// Languages whose server failed to start, with the reason, so repeated
    /// calls report the cause instead of retrying a doomed spawn each time.
    failed: RefCell<HashMap<ProviderKey, String>>,
    overrides: HashMap<String, Vec<String>>,
}

impl ProviderPool {
    /// `overrides` maps a language id to the command line to run instead of the
    /// built-in default, so an unusual toolchain layout does not need a rebuild.
    pub(crate) fn new(overrides: HashMap<String, Vec<String>>) -> Self {
        Self {
            providers: RefCell::new(HashMap::new()),
            failed: RefCell::new(HashMap::new()),
            overrides,
        }
    }

    /// The command line to run, with the program resolved to a full path when
    /// it lives in a toolchain's default location rather than on PATH.
    pub(crate) fn command_for(&self, spec: &LanguageSpec) -> Vec<String> {
        let mut command = self
            .overrides
            .get(spec.id)
            .cloned()
            .unwrap_or_else(|| spec.default_command());

        if let Some(program) = command.first_mut() {
            if let Some(found) = registry::resolve_program(spec.id, program) {
                *program = found.display().to_string();
            }
        }
        command
    }

    /// Runs `action` against the provider serving `file`, starting it if needed.
    pub(crate) fn with_provider<T>(
        &self,
        spec: &LanguageSpec,
        file: &Path,
        action: impl FnOnce(&mut dyn LspProvider) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = spec.workspace_root(file);
        let key = ProviderKey {
            language: spec.id.to_string(),
            root: root.clone(),
        };

        if let Some(reason) = self.failed.borrow().get(&key) {
            return Err(reason.clone());
        }

        let mut providers = self.providers.borrow_mut();
        if !providers.contains_key(&key) {
            let started = if spec.in_process {
                Ok(Box::new(inproc::HappProvider::default()) as Box<dyn LspProvider>)
            } else {
                child::ChildProvider::start(spec, &self.command_for(spec), &root)
                    .map(|provider| Box::new(provider) as Box<dyn LspProvider>)
            };
            match started {
                Ok(provider) => {
                    providers.insert(key.clone(), provider);
                }
                Err(err) => {
                    let reason = format!(
                        "{} language support is unavailable: {err}\n{}",
                        spec.id, spec.install_hint
                    );
                    self.failed.borrow_mut().insert(key, reason.clone());
                    return Err(reason);
                }
            }
        }

        let provider = providers
            .get_mut(&key)
            .ok_or_else(|| format!("no provider for {}", spec.id))?;
        action(provider.as_mut())
    }

    /// Servers currently running, with the workspace each serves and the LSP
    /// methods it turned out to support once it announced its capabilities.
    pub(crate) fn running(&self) -> Vec<RunningProvider> {
        let mut out: Vec<RunningProvider> = self
            .providers
            .borrow()
            .iter()
            .map(|(key, provider)| RunningProvider {
                language: key.language.clone(),
                root: key.root.clone(),
                methods: provider.supported_methods(),
            })
            .collect();
        out.sort_by(|a, b| (&a.language, &a.root).cmp(&(&b.language, &b.root)));
        out
    }
}

/// A provider that has already been started.
pub(crate) struct RunningProvider {
    pub(crate) language: String,
    pub(crate) root: PathBuf,
    pub(crate) methods: Vec<String>,
}

/// LSP counts lines and columns from zero; humans and models count from one.
/// Every position crossing the tool boundary is one-based.
pub(crate) fn to_lsp_position(line: u32, column: u32) -> JsonValue {
    serde_json::json!({
        "line": line.saturating_sub(1),
        "character": column.saturating_sub(1),
    })
}

pub(crate) fn from_lsp_position(position: &JsonValue) -> (u32, u32) {
    let line = position
        .get("line")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let character = position
        .get("character")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    (line as u32 + 1, character as u32 + 1)
}

/// A `file://` URI for `path`.
///
/// Relative paths are resolved against the working directory first, because
/// `Url::from_file_path` rejects them outright and a workspace root of `.` is
/// an ordinary thing for a caller to end up with.
pub(crate) fn path_to_uri(path: &Path) -> Result<String, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("resolve current directory: {err}"))?
            .join(path)
    };
    url::Url::from_file_path(&absolute)
        .map_err(|()| {
            format!(
                "path is not addressable as a file URI: {}",
                absolute.display()
            )
        })
        .map(|url| url.to_string())
}

pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

pub(crate) fn severity_label(severity: Option<u64>) -> String {
    match severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "info",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_still_becomes_a_usable_uri() {
        let uri = path_to_uri(Path::new(".")).expect("relative paths must resolve");
        assert!(uri.starts_with("file:///"), "{uri}");
        assert_eq!(
            uri_to_path(&uri).as_deref(),
            std::env::current_dir().ok().as_deref(),
            "the round trip must land back on the working directory"
        );
    }

    #[test]
    fn positions_round_trip_between_one_based_and_lsp() {
        let lsp = to_lsp_position(12, 5);
        assert_eq!(lsp["line"], 11);
        assert_eq!(lsp["character"], 4);
        assert_eq!(from_lsp_position(&lsp), (12, 5));
    }

    #[test]
    fn position_one_is_never_pushed_below_zero() {
        let lsp = to_lsp_position(0, 0);
        assert_eq!(lsp["line"], 0);
        assert_eq!(lsp["character"], 0);
    }

    #[test]
    fn a_failed_start_is_reported_without_respawning() {
        let pool = ProviderPool::new(HashMap::from([(
            "go".to_string(),
            vec!["definitely-not-a-real-language-server".to_string()],
        )]));
        let spec = registry::spec_for_language("go").expect("go spec");
        let file = PathBuf::from("/tmp/does-not-matter/main.go");

        let first = pool.with_provider(spec, &file, |_| Ok(()));
        let second = pool.with_provider(spec, &file, |_| Ok(()));
        let (Err(first), Err(second)) = (first, second) else {
            panic!("a missing language server must not start");
        };
        assert_eq!(first, second, "the cached reason must be reported verbatim");
        assert!(
            first.contains("go language support is unavailable"),
            "{first}"
        );
        assert!(first.contains("gopls"), "install hint expected: {first}");
    }
}
