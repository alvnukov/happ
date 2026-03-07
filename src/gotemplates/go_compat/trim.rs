// Go parity reference: stdlib text/template/parse/lex.go trim marker rules.

pub fn has_left_trim_marker(action: &str) -> bool {
    if action.len() < 4 {
        return false;
    }
    let bytes = action.as_bytes();
    bytes.get(2) == Some(&b'-') && bytes.get(3).is_some_and(u8::is_ascii_whitespace)
}

pub fn has_right_trim_marker(action: &str) -> bool {
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

pub fn trim_left_ascii_whitespace(s: &str) -> &str {
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

pub fn trim_right_ascii_whitespace_in_place(out: &mut String) {
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
    use super::{
        has_left_trim_marker, has_right_trim_marker, trim_left_ascii_whitespace,
        trim_right_ascii_whitespace_in_place,
    };

    #[test]
    fn trim_marker_detection_matches_go_lex_shape() {
        assert!(has_left_trim_marker("{{- .v}}"));
        assert!(!has_left_trim_marker("{{-.v}}"));
        assert!(has_right_trim_marker("{{.v -}}"));
        assert!(!has_right_trim_marker("{{.v-}}"));
    }

    #[test]
    fn ascii_whitespace_trim_helpers_work_in_place() {
        let mut right = " x \n\t".to_string();
        trim_right_ascii_whitespace_in_place(&mut right);
        assert_eq!(right, " x");
        assert_eq!(trim_left_ascii_whitespace(" \n\t x"), "x");
    }
}
