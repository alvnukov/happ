// Internal bridge while parse parity is still migrated from gotemplates parser.
// Keep direct gotemplates parser dependency centralized in one place.
pub(crate) use crate::go_compat::parse::report::{
    ActionParseReport, ControlAction, ControlKind, ParseCompatOptions,
};
pub(crate) use crate::gotemplates::parser::{parse_action_compat, parse_action_report_with_options};
pub(crate) use crate::gotemplates::{
    parse_template_tokens_strict_with_options_and_delims, GoTemplateScanError, GoTemplateToken,
};
