// Go parity reference: stdlib text/template/funcs.go (JSEscape/jsIsSpecial).
//
// This module keeps Unicode classification rules that are specific to Go
// escaping behavior, so executor code can stay focused on rendering flow.
pub fn js_requires_escape(ch: char) -> bool {
    if ch.is_control() {
        return true;
    }
    if ch != ' ' && ch.is_whitespace() {
        return true;
    }
    let code = ch as u32;
    if is_unicode_noncharacter(code) {
        return true;
    }
    matches!(
        code,
        0x00AD
            | 0x061C
            | 0x180E
            | 0x06DD
            | 0x070F
            | 0x08E2
            | 0xFEFF
            | 0x0600..=0x0605
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFFF9..=0xFFFB
    )
}

fn is_unicode_noncharacter(code: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&code) || (code <= 0x10FFFF && (code & 0xFFFE) == 0xFFFE)
}

#[cfg(test)]
mod tests {
    use super::js_requires_escape;

    #[test]
    fn js_requires_escape_matches_go_sensitive_runes() {
        for ch in ['\u{00A0}', '\u{200B}', '\u{2028}', '\u{2029}', '\u{FFFE}'] {
            assert!(js_requires_escape(ch), "must escape U+{:04X}", ch as u32);
        }
        for ch in ['Ā', 'Ж', '🙂'] {
            assert!(!js_requires_escape(ch), "must keep U+{:04X}", ch as u32);
        }
    }
}
