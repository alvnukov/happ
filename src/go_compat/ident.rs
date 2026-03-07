// Go parity reference: stdlib text/template/parse/lex.go identifier rules.

pub fn is_identifier_start_char(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

pub fn is_identifier_continue_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

pub fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_identifier_start_char(first) {
        return false;
    }
    chars.all(is_identifier_continue_char)
}

#[cfg(test)]
mod tests {
    use super::{is_identifier_continue_char, is_identifier_name, is_identifier_start_char};

    #[test]
    fn identifier_char_classes_follow_go_shape() {
        assert!(is_identifier_start_char('_'));
        assert!(is_identifier_start_char('A'));
        assert!(!is_identifier_start_char('1'));
        assert!(is_identifier_continue_char('1'));
        assert!(is_identifier_continue_char('_'));
    }

    #[test]
    fn identifier_name_requires_valid_start_and_continue() {
        assert!(is_identifier_name("x"));
        assert!(is_identifier_name("_x1"));
        assert!(!is_identifier_name(""));
        assert!(!is_identifier_name("1x"));
        assert!(!is_identifier_name("x-y"));
    }
}
