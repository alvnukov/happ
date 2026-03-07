use super::report::{ActionParseReport, ControlAction, ParseCompatOptions};
use crate::go_compat::scan::GoTemplateScanError;

pub fn parse_action_compat(
    action: &str,
    action_start: usize,
) -> Result<ControlAction, GoTemplateScanError> {
    crate::gotemplates::parser::parse_action_compat(action, action_start)
}

pub fn parse_action_report_with_options(
    action: &str,
    action_start: usize,
    options: ParseCompatOptions<'_>,
) -> Result<ActionParseReport, GoTemplateScanError> {
    crate::gotemplates::parser::parse_action_report_with_options(action, action_start, options)
}
