use crate::gotemplates::utf8scan::push_utf8_char_from_bytes;

// Go parity reference: stdlib text/template/parse/parse.go + parse/lex.go
// command and pipeline token boundaries.

pub fn strip_outer_parens(s: &str) -> Option<&str> {
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

pub fn split_pipeline_commands_owned(inner: &str) -> Vec<String> {
    split_pipeline_commands_borrowed(inner)
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

pub fn split_pipeline_commands_borrowed<'a>(inner: &'a str) -> Vec<&'a str> {
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
                            out.push(cmd);
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
            out.push(cmd);
        }
    }

    out
}

pub fn split_command_tokens_exec(command: &str) -> Vec<String> {
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

pub fn split_command_tokens_simple(command: &str) -> Vec<String> {
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

    while i < bytes.len() {
        match state {
            State::Normal => {
                if bytes[i].is_ascii_whitespace() {
                    if !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
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
    use super::{
        split_command_tokens_exec, split_command_tokens_simple, split_pipeline_commands_borrowed,
        strip_outer_parens,
    };

    #[test]
    fn strip_outer_parens_only_unwraps_full_expression() {
        assert_eq!(strip_outer_parens("(1)"), Some("1"));
        assert_eq!(strip_outer_parens("((a.b))"), Some("(a.b)"));
        assert_eq!(strip_outer_parens("(a) + (b)"), None);
        assert_eq!(strip_outer_parens("(a"), None);
    }

    #[test]
    fn split_pipeline_commands_respects_quotes_and_comments() {
        let cmds =
            split_pipeline_commands_borrowed(r#"print "a|b" | printf "%s" /* x|y */ | quote"#);
        assert_eq!(
            cmds,
            vec!["print \"a|b\"", "printf \"%s\" /* x|y */", "quote"]
        );
    }

    #[test]
    fn split_command_tokens_exec_preserves_unicode_and_commas() {
        let tokens = split_command_tokens_exec(r#"sum (index .m "ключ") , 2 "#);
        assert_eq!(tokens, vec!["sum", "(index .m \"ключ\")", ",", "2"]);
    }

    #[test]
    fn split_command_tokens_simple_keeps_parenthesized_chunks_split_by_space() {
        let tokens = split_command_tokens_simple(r#"include "x" (printf "%s" "a")"#);
        assert_eq!(tokens, vec!["include", "\"x\"", "(printf", "\"%s\"", "\"a\")"]);
    }
}
