// Runtime backend selection for compatibility logic.
//
// This interface allows switching between:
// - Go-compat behavior (current canonical path)
// - Native Rust behavior (future path)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogicBackend {
    #[default]
    GoCompat,
    RustNative,
}
