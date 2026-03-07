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
