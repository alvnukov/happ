use crate::gotemplates::go_compat::trim::{
    has_left_trim_marker, has_right_trim_marker, trim_left_ascii_whitespace,
    trim_right_ascii_whitespace_in_place,
};
use crate::gotemplates::GoTemplateToken;

pub(super) fn apply_lexical_trims(tokens: &mut [GoTemplateToken]) {
    for i in 0..tokens.len() {
        let action = match &tokens[i] {
            GoTemplateToken::Action(a) => a.clone(),
            GoTemplateToken::Literal(_) => continue,
        };
        if has_left_trim_marker(&action) && i > 0 {
            if let GoTemplateToken::Literal(prev) = &mut tokens[i - 1] {
                trim_right_ascii_whitespace_in_place(prev);
            }
        }
        if has_right_trim_marker(&action) && i + 1 < tokens.len() {
            if let GoTemplateToken::Literal(next) = &mut tokens[i + 1] {
                *next = trim_left_ascii_whitespace(next).to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_lexical_trims;
    use crate::gotemplates::GoTemplateToken;

    #[test]
    fn lexical_trims_handle_left_and_right_markers() {
        let mut tokens = vec![
            GoTemplateToken::Literal("x ".to_string()),
            GoTemplateToken::Action("{{- .v -}}".to_string()),
            GoTemplateToken::Literal(" y".to_string()),
        ];
        apply_lexical_trims(&mut tokens);
        assert_eq!(tokens[0], GoTemplateToken::Literal("x".to_string()));
        assert_eq!(tokens[2], GoTemplateToken::Literal("y".to_string()));
    }
}
