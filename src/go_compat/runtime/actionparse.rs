use super::{ActionKind, ElseClause, NativeRenderError};
use crate::go_compat::actionparse::{
    parse_action_kind as go_parse_action_kind, ParsedActionKind, ParsedElseClause,
};

pub(super) fn parse_action_kind(action: &str) -> Result<ActionKind, NativeRenderError> {
    let parsed =
        go_parse_action_kind(action).map_err(|err| NativeRenderError::UnsupportedAction {
            action: action.to_string(),
            reason: err.reason,
        })?;
    Ok(match parsed {
        ParsedActionKind::Noop => ActionKind::Noop,
        ParsedActionKind::Output(s) => ActionKind::Output(s),
        ParsedActionKind::If(s) => ActionKind::If(s),
        ParsedActionKind::With(s) => ActionKind::With(s),
        ParsedActionKind::Range(s) => ActionKind::Range(s),
        ParsedActionKind::Else(ParsedElseClause::Plain) => ActionKind::Else(ElseClause::Plain),
        ParsedActionKind::Else(ParsedElseClause::If(s)) => ActionKind::Else(ElseClause::If(s)),
        ParsedActionKind::Else(ParsedElseClause::With(s)) => ActionKind::Else(ElseClause::With(s)),
        ParsedActionKind::End => ActionKind::End,
        ParsedActionKind::Define { name } => ActionKind::Define { name },
        ParsedActionKind::Block { name, arg } => ActionKind::Block { name, arg },
        ParsedActionKind::Template { name, arg } => ActionKind::Template { name, arg },
        ParsedActionKind::Break => ActionKind::Break,
        ParsedActionKind::Continue => ActionKind::Continue,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{ActionKind, ElseClause, NativeRenderError};
    use super::parse_action_kind;

    #[test]
    fn parses_template_and_block_actions() {
        let template = parse_action_kind(r#"{{ template "x" .Values }}"#).expect("must parse");
        assert_eq!(
            template,
            ActionKind::Template {
                name: "x".to_string(),
                arg: Some(".Values".to_string()),
            }
        );

        let block = parse_action_kind(r#"{{ block "x" . }}"#).expect("must parse");
        assert_eq!(
            block,
            ActionKind::Block {
                name: "x".to_string(),
                arg: ".".to_string(),
            }
        );
    }

    #[test]
    fn parses_else_if_and_else_with() {
        assert_eq!(
            parse_action_kind(r#"{{ else if .enabled }}"#).expect("must parse"),
            ActionKind::Else(ElseClause::If(".enabled".to_string()))
        );
        assert_eq!(
            parse_action_kind(r#"{{ else with .ctx }}"#).expect("must parse"),
            ActionKind::Else(ElseClause::With(".ctx".to_string()))
        );
    }

    #[test]
    fn rejects_invalid_define_name() {
        let err = parse_action_kind(r#"{{ define x }}"#).expect_err("must fail");
        assert!(matches!(err, NativeRenderError::UnsupportedAction { .. }));
    }
}
