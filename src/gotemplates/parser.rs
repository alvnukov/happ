use super::GoTemplateScanError;
pub use crate::go_compat::parse::report::ParseCompatOptions;
pub(crate) use crate::go_compat::parse::report::{
    ActionParseReport, ControlAction, ControlKind, VariableRef,
};

pub(crate) fn parse_action_compat(
    action: &str,
    action_start: usize,
) -> Result<ControlAction, GoTemplateScanError> {
    crate::go_compat::parse::parse_action_compat(action, action_start)
}

pub(crate) fn parse_action_compat_with_options(
    action: &str,
    action_start: usize,
    options: ParseCompatOptions<'_>,
) -> Result<ControlAction, GoTemplateScanError> {
    crate::go_compat::parse::parse_action_compat_with_options(action, action_start, options)
}

pub(crate) fn parse_action_report_with_options(
    action: &str,
    action_start: usize,
    options: ParseCompatOptions<'_>,
) -> Result<ActionParseReport, GoTemplateScanError> {
    crate::go_compat::parse::parse_action_report_with_options(action, action_start, options)
}
