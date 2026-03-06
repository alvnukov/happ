use serde_json::Value;

use super::layout::truncate_runes;
use super::{
    format_printf_mismatch, value_as_byte_slice, value_as_string_bytes, value_number_to_rune_go,
};

pub(super) fn format_q_verb_go(
    arg: &Option<Value>,
    plus: bool,
    sharp: bool,
    precision: Option<usize>,
) -> Option<String> {
    let Some(value) = arg.as_ref() else {
        return None;
    };
    format_q_value_ref(value, plus, sharp, precision)
}

fn format_q_value_ref(
    value: &Value,
    plus: bool,
    sharp: bool,
    precision: Option<usize>,
) -> Option<String> {
    if let Some(bytes) = value_as_byte_slice(value) {
        let bytes = if let Some(p) = precision {
            truncate_bytes_by_runes_go(&bytes, p)
        } else {
            bytes
        };
        if let Ok(s) = std::str::from_utf8(&bytes) {
            return Some(quote_string_go(s, plus, sharp));
        }
        return Some(quote_bytes_go(&bytes));
    }
    if let Some(bytes) = value_as_string_bytes(value) {
        let bytes = if let Some(p) = precision {
            truncate_bytes_by_runes_go(&bytes, p)
        } else {
            bytes
        };
        if let Ok(s) = std::str::from_utf8(&bytes) {
            return Some(quote_string_go(s, plus, sharp));
        }
        return Some(quote_bytes_go(&bytes));
    }
    match value {
        Value::String(s) => {
            let rendered = if let Some(p) = precision {
                truncate_runes(s, p)
            } else {
                s.to_string()
            };
            Some(quote_string_go(&rendered, plus, sharp))
        }
        Value::Array(items) => Some(format_q_array_go(items, plus, sharp, precision)),
        Value::Number(n) => value_number_to_rune_go(n).map(|ch| quote_rune_go(ch, plus)),
        _ => None,
    }
}

fn quote_string_go(s: &str, plus: bool, sharp: bool) -> String {
    if sharp && can_backquote_string(s) {
        let mut raw = String::with_capacity(s.len() + 2);
        raw.push('`');
        raw.push_str(s);
        raw.push('`');
        return raw;
    }
    if plus {
        return quote_string_ascii_go(s);
    }
    format!("{s:?}")
}

fn format_q_array_go(items: &[Value], plus: bool, sharp: bool, precision: Option<usize>) -> String {
    let mut out = String::from("[");
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let rendered = format_q_value_ref(item, plus, sharp, precision)
            .unwrap_or_else(|| format_printf_mismatch('q', &Some(item.clone())));
        out.push_str(&rendered);
    }
    out.push(']');
    out
}

fn truncate_bytes_by_runes_go(bytes: &[u8], max_runes: usize) -> Vec<u8> {
    if max_runes == 0 {
        return Vec::new();
    }
    let mut i = 0usize;
    let mut n = 0usize;
    while i < bytes.len() && n < max_runes {
        let w = utf8_rune_width_go(bytes, i);
        i = i.saturating_add(w);
        n += 1;
    }
    bytes[..i.min(bytes.len())].to_vec()
}

fn utf8_rune_width_go(bytes: &[u8], i: usize) -> usize {
    let Some(&b0) = bytes.get(i) else {
        return 0;
    };
    if b0 < 0x80 {
        return 1;
    }
    for width in 2..=4usize {
        if i + width > bytes.len() {
            break;
        }
        let part = &bytes[i..i + width];
        if let Ok(s) = std::str::from_utf8(part) {
            if s.chars().count() == 1 {
                return width;
            }
        }
    }
    1
}

fn quote_rune_go(ch: char, ascii_only: bool) -> String {
    let mut out = String::with_capacity(12);
    out.push('\'');
    match ch {
        '\u{0007}' => out.push_str("\\a"),
        '\u{0008}' => out.push_str("\\b"),
        '\u{000C}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{000B}' => out.push_str("\\v"),
        '\\' => out.push_str("\\\\"),
        '\'' => out.push_str("\\'"),
        c if ascii_only => push_go_escaped_rune_ascii(&mut out, c),
        c => push_go_escaped_rune(&mut out, c),
    }
    out.push('\'');
    out
}

fn push_go_escaped_rune(out: &mut String, ch: char) {
    let code = ch as u32;
    if ch.is_control() {
        if code <= 0xFF {
            out.push_str(&format!("\\x{code:02x}"));
        } else if code <= 0xFFFF {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push_str(&format!("\\U{code:08x}"));
        }
        return;
    }

    let escaped = ch.escape_debug().to_string();
    if escaped.len() == 1 {
        out.push(ch);
        return;
    }
    if escaped == "\\0" {
        out.push_str("\\x00");
        return;
    }
    if let Some(hex) = escaped
        .strip_prefix("\\u{")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            if v <= 0xFFFF {
                out.push_str(&format!("\\u{v:04x}"));
            } else {
                out.push_str(&format!("\\U{v:08x}"));
            }
            return;
        }
    }
    out.push_str(&escaped);
}

fn push_go_escaped_rune_ascii(out: &mut String, ch: char) {
    let code = ch as u32;
    if ch.is_ascii_graphic() || ch == ' ' {
        out.push(ch);
        return;
    }
    if ch.is_control() {
        if code <= 0xFF {
            out.push_str(&format!("\\x{code:02x}"));
        } else if code <= 0xFFFF {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push_str(&format!("\\U{code:08x}"));
        }
        return;
    }
    if code <= 0xFFFF {
        out.push_str(&format!("\\u{code:04x}"));
    } else {
        out.push_str(&format!("\\U{code:08x}"));
    }
}

fn quote_bytes_go(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    out.push('"');
    for &b in bytes {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x0C => out.push_str("\\f"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x0B => out.push_str("\\v"),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

fn can_backquote_string(s: &str) -> bool {
    if s.contains('`') || s.contains('\r') {
        return false;
    }
    s.chars().all(|ch| ch == '\t' || !ch.is_control())
}

fn quote_string_ascii_go(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\u{0007}' => out.push_str("\\a"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{000B}' => out.push_str("\\v"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            ch if ch.is_ascii_graphic() || ch == ' ' => out.push(ch),
            ch => {
                let code = ch as u32;
                if code <= 0xFFFF {
                    out.push_str("\\u");
                    out.push_str(&format!("{code:04x}"));
                } else {
                    out.push_str("\\U");
                    out.push_str(&format!("{code:08x}"));
                }
            }
        }
    }
    out.push('"');
    out
}
