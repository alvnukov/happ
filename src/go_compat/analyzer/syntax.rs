use crate::go_compat::compat::parse_go_quoted_prefix;

pub(crate) fn extract_template_name(action: &str, keyword: &str) -> Option<String> {
    let inner = action_inner_trimmed(action)?;
    let rest = inner.strip_prefix(keyword)?.trim_start();
    let (name, _) = parse_go_quoted_prefix(rest)?;
    Some(name)
}

pub(crate) fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let bytes = source.as_bytes();
    let clamped = offset.min(bytes.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for b in bytes.iter().take(clamped) {
        if *b == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn action_inner_trimmed(action: &str) -> Option<&str> {
    let mut inner = action;
    if let Some(rest) = inner.strip_prefix("{{") {
        inner = rest;
    } else {
        return None;
    }
    if let Some(rest) = inner.strip_suffix("}}") {
        inner = rest;
    } else {
        return None;
    }

    let mut trimmed = inner.trim();
    if let Some(rest) = trimmed.strip_prefix('-') {
        trimmed = rest.trim_start();
    }
    if let Some(rest) = trimmed.strip_suffix('-') {
        trimmed = rest.trim_end();
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_template_name_supports_raw_and_escaped_strings() {
        assert_eq!(
            extract_template_name(r#"{{template "abc" .}}"#, "template").as_deref(),
            Some("abc")
        );
        assert_eq!(
            extract_template_name(r#"{{template "a\"b" .}}"#, "template").as_deref(),
            Some("a\"b")
        );
        assert_eq!(
            extract_template_name(r#"{{template "\x61" .}}"#, "template").as_deref(),
            Some("a")
        );
        assert_eq!(
            extract_template_name("{{template `raw value` .}}", "template").as_deref(),
            Some("raw value")
        );
    }

    #[test]
    fn offset_to_line_col_uses_one_based_positions_and_byte_columns() {
        let src = "a\nBC\n";
        assert_eq!(offset_to_line_col(src, 0), (1, 1));
        assert_eq!(offset_to_line_col(src, 1), (1, 2));
        assert_eq!(offset_to_line_col(src, 2), (2, 1));
        assert_eq!(offset_to_line_col(src, 3), (2, 2));

        let utf = "a\nйz";
        assert_eq!(offset_to_line_col(utf, 2), (2, 1));
        assert_eq!(offset_to_line_col(utf, 3), (2, 2));
        assert_eq!(offset_to_line_col(utf, 4), (2, 3));
    }
}
