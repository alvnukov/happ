// Canonical runtime API surface for Go-template-compatible rendering.
// Runtime execution currently delegates to gotemplates executor while parse
// and scan behavior are migrated under go_compat.
pub use crate::gotemplates::{
    render_template_native_with_resolver, FunctionDispatchMode, MissingValueMode,
    NativeFunctionResolver, NativeRenderError, NativeRenderOptions,
};
