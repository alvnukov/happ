// Runtime backend selection for compatibility logic.
//
// This interface allows switching between:
// - Go FFI behavior (experimental path backed by Go text/template helper)
// - Go-compat behavior (current canonical path)
// - Dual compare mode (run two engines and log mismatches)
// - Native Rust behavior (future path)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogicBackend {
    #[default]
    GoFfi,
    GoCompat,
    Dual,
    RustNative,
}

impl LogicBackend {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "go_ffi" | "goffi" | "ffi" => Some(Self::GoFfi),
            "go_compat" | "gocompat" | "compat" => Some(Self::GoCompat),
            "dual" | "compare" | "go_dual" => Some(Self::Dual),
            "rust_native" | "rustnative" | "native" => Some(Self::RustNative),
            _ => None,
        }
    }

    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("HAPP_TEMPLATE_BACKEND").ok()?;
        Self::parse(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::LogicBackend;

    #[test]
    fn parse_supports_known_backend_aliases() {
        assert_eq!(LogicBackend::parse("go_ffi"), Some(LogicBackend::GoFfi));
        assert_eq!(LogicBackend::parse("compat"), Some(LogicBackend::GoCompat));
        assert_eq!(LogicBackend::parse("dual"), Some(LogicBackend::Dual));
        assert_eq!(
            LogicBackend::parse("rust_native"),
            Some(LogicBackend::RustNative)
        );
    }

    #[test]
    fn parse_rejects_unknown_backend_names() {
        assert_eq!(LogicBackend::parse(""), None);
        assert_eq!(LogicBackend::parse("something_else"), None);
    }
}
