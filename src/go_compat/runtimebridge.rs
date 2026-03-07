// Internal bridge between go_compat template facade and gotemplates runtime.
// Keep cross-module dependency centralized in one place.
pub(crate) use crate::gotemplates::{
    render_template_native_with_resolver, FunctionDispatchMode, MissingValueMode,
    NativeFunctionResolver, NativeRenderError, NativeRenderOptions,
};
