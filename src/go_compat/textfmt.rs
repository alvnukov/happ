use crate::go_compat::valuefmt::format_value_like_go;
use crate::go_compat::typedvalue::{decode_go_string_bytes_value, go_string_bytes_len};
use serde_json::Value;

// Go parity reference: stdlib text/template/funcs.go.
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

pub fn builtin_print(args: &[Option<Value>], with_newline: bool) -> String {
    let mut out = eval_args_text(args, MissingValueRender::Nil);
    if with_newline {
        out.push('\n');
    }
    out
}

pub fn builtin_urlquery(args: &[Option<Value>]) -> String {
    query_escape_bytes(&eval_args_bytes(args, MissingValueRender::NoValue))
}

pub fn builtin_html(args: &[Option<Value>]) -> String {
    html_escape(&eval_args_text(args, MissingValueRender::NoValue))
}

pub fn builtin_js(args: &[Option<Value>]) -> String {
    js_escape(&eval_args_text(args, MissingValueRender::NoValue))
}

pub fn format_value_for_print(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => format_value_like_go(other),
    }
}

#[derive(Debug, Clone, Copy)]
enum MissingValueRender {
    Nil,
    NoValue,
}

fn eval_args_text(args: &[Option<Value>], missing: MissingValueRender) -> String {
    let mut joined = String::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let (piece, cur_is_string) = render_arg_text(arg, missing);
        if idx > 0 && !prev_is_string && !cur_is_string {
            joined.push(' ');
        }
        joined.push_str(&piece);
        prev_is_string = cur_is_string;
    }
    joined
}

fn eval_args_bytes(args: &[Option<Value>], missing: MissingValueRender) -> Vec<u8> {
    let mut joined = Vec::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let (piece, cur_is_string) = match arg {
            None => (
                match missing {
                    MissingValueRender::Nil => b"<nil>".as_slice().to_vec(),
                    MissingValueRender::NoValue => b"<no value>".as_slice().to_vec(),
                },
                false,
            ),
            Some(Value::String(s)) => (s.as_bytes().to_vec(), true),
            Some(v) => {
                if let Some(bytes) = decode_go_string_bytes_value(v) {
                    (bytes, true)
                } else {
                    (format_value_like_go(v).into_bytes(), false)
                }
            }
        };
        if idx > 0 && !prev_is_string && !cur_is_string {
            joined.push(b' ');
        }
        joined.extend_from_slice(&piece);
        prev_is_string = cur_is_string;
    }
    joined
}

fn render_arg_text(arg: &Option<Value>, missing: MissingValueRender) -> (String, bool) {
    match arg {
        None => (
            match missing {
                MissingValueRender::Nil => "<nil>".to_string(),
                MissingValueRender::NoValue => "<no value>".to_string(),
            },
            false,
        ),
        Some(Value::Null) if matches!(missing, MissingValueRender::Nil) => {
            ("<nil>".to_string(), false)
        }
        Some(v) => (
            format_value_like_go(v),
            matches!(v, Value::String(_)) || go_string_bytes_len(v).is_some(),
        ),
    }
}

fn query_escape_bytes(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() + input.len() / 3);
    for b in input {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex_upper((*b >> 4) & 0x0F));
                out.push(hex_upper(*b & 0x0F));
            }
        }
    }
    out
}

fn html_escape(input: &str) -> String {
    if !input
        .chars()
        .any(|ch| matches!(ch, '\'' | '"' | '&' | '<' | '>' | '\0'))
    {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len() + input.len() / 4);
    for ch in input.chars() {
        match ch {
            '\0' => out.push('\u{FFFD}'),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn js_escape(input: &str) -> String {
    if !input.chars().any(js_is_special) {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len() + input.len() / 4);
    for ch in input.chars() {
        if !js_is_special(ch) {
            out.push(ch);
            continue;
        }
        if ch.is_ascii() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '"' => out.push_str("\\\""),
                '<' => out.push_str("\\u003C"),
                '>' => out.push_str("\\u003E"),
                '&' => out.push_str("\\u0026"),
                '=' => out.push_str("\\u003D"),
                _ => {
                    let v = ch as u32;
                    out.push_str("\\u00");
                    out.push(hex_upper(((v >> 4) & 0x0F) as u8));
                    out.push(hex_upper((v & 0x0F) as u8));
                }
            }
            continue;
        }

        if js_requires_escape(ch) {
            let v = ch as u32;
            out.push('\\');
            out.push('u');
            if v <= 0xFFFF {
                let code = format!("{v:04X}");
                out.push_str(&code);
            } else {
                // Keep current behavior for non-BMP runes (Go writes \u%04X).
                let code = format!("{v:X}");
                out.push_str(&code);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn js_is_special(ch: char) -> bool {
    matches!(ch, '\\' | '\'' | '"' | '<' | '>' | '&' | '=') || ch < ' ' || !ch.is_ascii()
}

fn hex_upper(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => '0',
    }
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_html, builtin_js, builtin_print, builtin_urlquery, format_value_for_print,
        js_requires_escape,
    };
    use serde_json::Value;

    #[test]
    fn js_requires_escape_matches_go_sensitive_runes() {
        for ch in ['\u{00A0}', '\u{200B}', '\u{2028}', '\u{2029}', '\u{FFFE}'] {
            assert!(js_requires_escape(ch), "must escape U+{:04X}", ch as u32);
        }
        for ch in ['Ā', 'Ж', '🙂'] {
            assert!(!js_requires_escape(ch), "must keep U+{:04X}", ch as u32);
        }
    }

    #[test]
    fn builtin_urlquery_keeps_go_no_value_and_byte_semantics() {
        let out = builtin_urlquery(&[None, Some(Value::String("a b".to_string()))]);
        assert_eq!(out, "%3Cno+value%3Ea+b");
    }

    #[test]
    fn builtin_js_escapes_special_ascii_as_go_style() {
        let out = builtin_js(&[Some(Value::String("<x&'\\\"=\\n>".to_string()))]);
        assert_eq!(out, "\\u003Cx\\u0026\\'\\\\\\\"\\u003D\\\\n\\u003E");
    }

    #[test]
    fn builtin_js_escapes_go_non_print_unicode_runes() {
        let out = builtin_js(&[Some(Value::String(
            "\u{00A0}\u{200B}\u{2028}\u{2029}\u{FFFE}".to_string(),
        ))]);
        assert_eq!(out, "\\u00A0\\u200B\\u2028\\u2029\\uFFFE");

        let out = builtin_js(&[Some(Value::String("ĀЖ🙂".to_string()))]);
        assert_eq!(out, "ĀЖ🙂");
    }

    #[test]
    fn builtin_print_uses_nil_placeholder_like_go_fmt_sprint() {
        let out = builtin_print(&[None], false);
        assert_eq!(out, "<nil>");

        let out = builtin_print(&[Some(Value::Null)], false);
        assert_eq!(out, "<nil>");
    }

    #[test]
    fn html_uses_no_value_placeholder_from_eval_args_path() {
        let out = builtin_html(&[None]);
        assert_eq!(out, "&lt;no value&gt;");
    }

    #[test]
    fn builtin_print_matches_go_spacing_rules_for_non_strings() {
        let out = builtin_print(
            &[
                Some(Value::Number(1.into())),
                Some(Value::Number(2.into())),
                Some(Value::String("x".to_string())),
                Some(Value::Number(3.into())),
            ],
            false,
        );
        assert_eq!(out, "1 2x3");
    }

    #[test]
    fn format_value_for_print_keeps_nil_marker() {
        assert_eq!(format_value_for_print(None), "<nil>");
        assert_eq!(format_value_for_print(Some(&Value::Null)), "<nil>");
    }
}
