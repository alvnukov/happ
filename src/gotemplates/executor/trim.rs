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

fn has_left_trim_marker(action: &str) -> bool {
    if action.len() < 4 {
        return false;
    }
    let bytes = action.as_bytes();
    bytes.get(2) == Some(&b'-') && bytes.get(3).is_some_and(u8::is_ascii_whitespace)
}

fn has_right_trim_marker(action: &str) -> bool {
    if action.len() < 4 || !action.ends_with("}}") {
        return false;
    }
    let bytes = action.as_bytes();
    let dash = bytes.len().saturating_sub(3);
    let prev = bytes.len().saturating_sub(4);
    bytes.get(dash).copied() == Some(b'-')
        && bytes
            .get(prev)
            .copied()
            .is_some_and(|b| b.is_ascii_whitespace())
}

fn trim_left_ascii_whitespace(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
        } else {
            break;
        }
    }
    &s[i..]
}

fn trim_right_ascii_whitespace_in_place(out: &mut String) {
    let bytes = out.as_bytes();
    let mut end = bytes.len();
    while end > 0 {
        if bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        } else {
            break;
        }
    }
    out.truncate(end);
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
