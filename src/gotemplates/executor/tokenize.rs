use crate::gotemplates::utf8scan::push_utf8_char_from_bytes;

pub(super) fn strip_outer_parens(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
        return None;
    }
    let mut depth = 0i32;
    let bytes = trimmed.as_bytes();
    for (i, ch) in bytes.iter().enumerate() {
        match *ch {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 && i + 1 < bytes.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(&trimmed[1..trimmed.len() - 1])
}

pub(super) fn split_pipeline_commands(inner: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
        Comment,
    }

    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut paren_depth: i32 = 0;
    let mut state = State::Normal;

    while i < bytes.len() {
        match state {
            State::Normal => {
                if starts_with(bytes, i, b"/*") {
                    state = State::Comment;
                    i += 2;
                    continue;
                }
                match bytes[i] {
                    b'\'' => {
                        state = State::SingleQuote;
                        i += 1;
                    }
                    b'"' => {
                        state = State::DoubleQuote;
                        i += 1;
                    }
                    b'`' => {
                        state = State::RawQuote;
                        i += 1;
                    }
                    b'(' => {
                        paren_depth += 1;
                        i += 1;
                    }
                    b')' => {
                        if paren_depth > 0 {
                            paren_depth -= 1;
                        }
                        i += 1;
                    }
                    b'|' if paren_depth == 0 => {
                        let cmd = inner[start..i].trim();
                        if !cmd.is_empty() {
                            out.push(cmd.to_string());
                        }
                        start = i + 1;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            State::SingleQuote => {
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == b'\'' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::DoubleQuote => {
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == b'"' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::RawQuote => {
                if bytes[i] == b'`' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::Comment => {
                if starts_with(bytes, i, b"*/") {
                    state = State::Normal;
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
    }

    if start <= inner.len() {
        let cmd = inner[start..].trim();
        if !cmd.is_empty() {
            out.push(cmd.to_string());
        }
    }
    out
}

pub(super) fn split_command_tokens(command: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        RawQuote,
    }

    let bytes = command.as_bytes();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;
    let mut state = State::Normal;
    let mut paren_depth = 0i32;

    while i < bytes.len() {
        match state {
            State::Normal => {
                if bytes[i].is_ascii_whitespace() && paren_depth == 0 {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
                    i += 1;
                    continue;
                }
                if bytes[i] == b',' && paren_depth == 0 {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
                    out.push(",".to_string());
                    i += 1;
                    continue;
                }
                match bytes[i] {
                    b'\'' => {
                        state = State::SingleQuote;
                        buf.push('\'');
                        i += 1;
                    }
                    b'"' => {
                        state = State::DoubleQuote;
                        buf.push('"');
                        i += 1;
                    }
                    b'`' => {
                        state = State::RawQuote;
                        buf.push('`');
                        i += 1;
                    }
                    b'(' => {
                        paren_depth += 1;
                        buf.push('(');
                        i += 1;
                    }
                    b')' => {
                        if paren_depth > 0 {
                            paren_depth -= 1;
                        }
                        buf.push(')');
                        i += 1;
                    }
                    _ => {
                        i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    }
                }
            }
            State::SingleQuote => {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push('\\');
                    i += 1;
                    i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    continue;
                }
                if bytes[i] == b'\'' {
                    buf.push('\'');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
            State::DoubleQuote => {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push('\\');
                    i += 1;
                    i = push_utf8_char_from_bytes(bytes, i, &mut buf);
                    continue;
                }
                if bytes[i] == b'"' {
                    buf.push('"');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
            State::RawQuote => {
                if bytes[i] == b'`' {
                    buf.push('`');
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                i = push_utf8_char_from_bytes(bytes, i, &mut buf);
            }
        }
    }

    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn starts_with(haystack: &[u8], offset: usize, needle: &[u8]) -> bool {
    haystack
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|chunk| chunk == needle)
}

#[cfg(test)]
mod tests {
    use super::{split_command_tokens, split_pipeline_commands, strip_outer_parens};

    #[test]
    fn strip_outer_parens_only_unwraps_full_expression() {
        assert_eq!(strip_outer_parens("(1)"), Some("1"));
        assert_eq!(strip_outer_parens("((a.b))"), Some("(a.b)"));
        assert_eq!(strip_outer_parens("(a) + (b)"), None);
        assert_eq!(strip_outer_parens("(a"), None);
    }

    #[test]
    fn split_pipeline_commands_respects_quotes_and_treats_comments_as_text() {
        let cmds = split_pipeline_commands(r#"print "a|b" | printf "%s" /* x|y */ | quote"#);
        assert_eq!(
            cmds,
            vec!["print \"a|b\"", "printf \"%s\" /* x|y */", "quote"]
        );
    }

    #[test]
    fn split_command_tokens_preserves_unicode_and_commas() {
        let tokens = split_command_tokens(r#"sum (index .m "ключ") , 2 "#);
        assert_eq!(tokens, vec!["sum", "(index .m \"ключ\")", ",", "2"]);
    }
}
