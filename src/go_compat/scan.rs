use crate::go_compat::parse::report::ParseCompatOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoTemplateToken {
    Literal(String),
    Action(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoTemplateActionSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoTemplateScanError {
    pub code: &'static str,
    pub message: String,
    pub offset: usize,
}

pub fn parse_template_tokens_strict_with_options_and_delims(
    src: &str,
    left_delim: &str,
    right_delim: &str,
    options: ParseCompatOptions<'_>,
) -> Result<Vec<GoTemplateToken>, GoTemplateScanError> {
    crate::gotemplates::parse_template_tokens_strict_with_options_and_delims(
        src,
        left_delim,
        right_delim,
        options,
    )
}
