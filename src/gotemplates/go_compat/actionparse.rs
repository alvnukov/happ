use crate::gotemplates::compat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedElseClause {
    Plain,
    If(String),
    With(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedActionKind {
    Noop,
    Output(String),
    If(String),
    With(String),
    Range(String),
    Else(ParsedElseClause),
    End,
    Define { name: String },
    Block { name: String, arg: String },
    Template { name: String, arg: Option<String> },
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionParseError {
    pub reason: String,
}

pub fn parse_action_kind(action: &str) -> Result<ParsedActionKind, ActionParseError> {
    let Some(inner) = action_inner(action) else {
        return Err(ActionParseError {
            reason: "invalid action delimiters".to_string(),
        });
    };
    if inner.is_empty() || inner.starts_with("/*") {
        return Ok(ParsedActionKind::Noop);
    }

    if inner == "end" {
        return Ok(ParsedActionKind::End);
    }
    if inner == "else" {
        return Ok(ParsedActionKind::Else(ParsedElseClause::Plain));
    }
    if let Some(expr) = inner.strip_prefix("else if ") {
        return Ok(ParsedActionKind::Else(ParsedElseClause::If(
            expr.trim().to_string(),
        )));
    }
    if let Some(expr) = inner.strip_prefix("else with ") {
        return Ok(ParsedActionKind::Else(ParsedElseClause::With(
            expr.trim().to_string(),
        )));
    }
    if let Some(expr) = inner.strip_prefix("if ") {
        return Ok(ParsedActionKind::If(expr.trim().to_string()));
    }
    if let Some(expr) = inner.strip_prefix("with ") {
        return Ok(ParsedActionKind::With(expr.trim().to_string()));
    }
    if let Some(expr) = inner.strip_prefix("range ") {
        return Ok(ParsedActionKind::Range(expr.trim().to_string()));
    }
    if let Some(rest) = inner.strip_prefix("define ") {
        let name = parse_quoted_name(rest).ok_or_else(|| ActionParseError {
            reason: "define name must be a quoted string".to_string(),
        })?;
        return Ok(ParsedActionKind::Define { name });
    }
    if let Some(rest) = inner.strip_prefix("block ") {
        let (name, arg) = parse_block_invocation_clause(rest).ok_or_else(|| ActionParseError {
            reason: "block clause must be: block \"name\" arg".to_string(),
        })?;
        return Ok(ParsedActionKind::Block { name, arg });
    }
    if let Some(rest) = inner.strip_prefix("template ") {
        let (name, arg) = parse_template_invocation_clause(rest).ok_or_else(|| ActionParseError {
            reason: "template clause must be: template \"name\" [arg]".to_string(),
        })?;
        return Ok(ParsedActionKind::Template { name, arg });
    }
    if inner == "break" {
        return Ok(ParsedActionKind::Break);
    }
    if inner == "continue" {
        return Ok(ParsedActionKind::Continue);
    }
    Ok(ParsedActionKind::Output(inner.to_string()))
}

fn parse_quoted_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    compat::decode_go_string_literal(trimmed)
}

fn parse_template_invocation_clause(raw: &str) -> Option<(String, Option<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (name, tail) = compat::parse_go_quoted_prefix(trimmed)?;
    let tail = tail.trim();
    let arg = if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    };
    Some((name, arg))
}

fn parse_block_invocation_clause(raw: &str) -> Option<(String, String)> {
    let (name, arg) = parse_template_invocation_clause(raw)?;
    Some((name, arg?.trim().to_string()))
}

fn action_inner(action: &str) -> Option<&str> {
    if !(action.starts_with("{{") && action.ends_with("}}")) || action.len() < 4 {
        return None;
    }
    let inner = &action[2..action.len() - 2];
    let bytes = inner.as_bytes();
    let mut start = 0usize;
    let mut end = inner.len();

    if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1].is_ascii_whitespace() {
        start = 1;
    }
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while start < end && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end > start && bytes[end - 1] == b'-' {
        end -= 1;
        while start < end && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
    }
    Some(&inner[start..end])
}

#[cfg(test)]
mod tests {
    use super::{parse_action_kind, ParsedActionKind, ParsedElseClause};

    #[test]
    fn parses_template_and_block_actions() {
        let template = parse_action_kind(r#"{{ template "x" .Values }}"#).expect("must parse");
        assert_eq!(
            template,
            ParsedActionKind::Template {
                name: "x".to_string(),
                arg: Some(".Values".to_string()),
            }
        );

        let block = parse_action_kind(r#"{{ block "x" . }}"#).expect("must parse");
        assert_eq!(
            block,
            ParsedActionKind::Block {
                name: "x".to_string(),
                arg: ".".to_string(),
            }
        );
    }

    #[test]
    fn parses_else_if_and_else_with() {
        assert_eq!(
            parse_action_kind(r#"{{ else if .enabled }}"#).expect("must parse"),
            ParsedActionKind::Else(ParsedElseClause::If(".enabled".to_string()))
        );
        assert_eq!(
            parse_action_kind(r#"{{ else with .ctx }}"#).expect("must parse"),
            ParsedActionKind::Else(ParsedElseClause::With(".ctx".to_string()))
        );
    }

    #[test]
    fn rejects_invalid_define_name() {
        let err = parse_action_kind(r#"{{ define x }}"#).expect_err("must fail");
        assert_eq!(err.reason, "define name must be a quoted string");
    }
}
