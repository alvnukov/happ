use crate::gotemplates::typedvalue::{decode_go_string_bytes_value, go_string_bytes_len};
use serde_json::Value;

pub(super) fn builtin_print(args: &[Option<Value>], with_newline: bool) -> String {
    let mut out = String::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let piece = format_value_for_print(arg);
        let cur_is_string = arg
            .as_ref()
            .is_some_and(|v| matches!(v, Value::String(_)) || go_string_bytes_len(v).is_some());
        if idx > 0 && !prev_is_string && !cur_is_string {
            out.push(' ');
        }
        out.push_str(&piece);
        prev_is_string = cur_is_string;
    }
    if with_newline {
        out.push('\n');
    }
    out
}

pub(super) fn builtin_urlquery(args: &[Option<Value>]) -> String {
    query_escape_bytes(&join_text_template_args_bytes(args))
}

pub(super) fn builtin_html(args: &[Option<Value>]) -> String {
    html_escape(&join_text_template_args(args))
}

pub(super) fn builtin_js(args: &[Option<Value>]) -> String {
    js_escape(&join_text_template_args(args))
}

pub(super) fn format_value_for_print(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => super::format_value_like_go(other),
    }
}

fn join_text_template_args(args: &[Option<Value>]) -> String {
    let mut joined = String::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let piece = match arg {
            None => "<no value>".to_string(),
            Some(v) => super::format_value_like_go(v),
        };
        let cur_is_string = arg
            .as_ref()
            .is_some_and(|v| matches!(v, Value::String(_)) || go_string_bytes_len(v).is_some());
        if idx > 0 && !prev_is_string && !cur_is_string {
            joined.push(' ');
        }
        joined.push_str(&piece);
        prev_is_string = cur_is_string;
    }
    joined
}

fn join_text_template_args_bytes(args: &[Option<Value>]) -> Vec<u8> {
    let mut joined = Vec::new();
    let mut prev_is_string = false;
    for (idx, arg) in args.iter().enumerate() {
        let (piece, cur_is_string) = match arg {
            None => (b"<no value>".as_slice().to_vec(), false),
            Some(Value::String(s)) => (s.as_bytes().to_vec(), true),
            Some(v) => {
                if let Some(bytes) = decode_go_string_bytes_value(v) {
                    (bytes, true)
                } else {
                    (super::format_value_like_go(v).into_bytes(), false)
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

        if ch.is_control() {
            let v = ch as u32;
            let code = format!("{v:04X}");
            out.push_str("\\u");
            out.push_str(&code);
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
    use super::{builtin_js, builtin_urlquery};
    use serde_json::Value;

    #[test]
    fn builtin_urlquery_keeps_go_no_value_and_byte_semantics() {
        let out = builtin_urlquery(&[None, Some(Value::String("a b".to_string()))]);
        assert_eq!(out, "%3Cno+value%3Ea+b");
    }

    #[test]
    fn builtin_js_escapes_special_ascii_as_go_style() {
        let out = builtin_js(&[Some(Value::String("<x&'\\\"=\\n>".to_string()))]);
        assert_eq!(out, "\\u003Cx\\u0026\\'\\\"\\u003D\\u000A\\u003E");
    }
}
