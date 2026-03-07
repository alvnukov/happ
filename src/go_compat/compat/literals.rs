use serde_json::{Number, Value};

pub fn parse_number_value(expr: &str) -> Option<Value> {
    if !has_valid_go_numeric_underscores(expr) {
        return None;
    }
    let compact = expr.replace('_', "");
    if compact.is_empty() {
        return None;
    }
    let (negative, body) = if let Some(rest) = compact.strip_prefix('+') {
        (false, rest)
    } else if let Some(rest) = compact.strip_prefix('-') {
        (true, rest)
    } else {
        (false, compact.as_str())
    };

    if let Some(intv) = parse_go_integer_literal(body) {
        if negative {
            let signed = if let Ok(v) = i128::try_from(intv) {
                -v
            } else {
                let fv = -(intv as f64);
                return Number::from_f64(fv).map(Value::Number);
            };
            if let Ok(v) = i64::try_from(signed) {
                return Some(Value::Number(Number::from(v)));
            }
            if let Some(v) = Number::from_f64(signed as f64) {
                return Some(Value::Number(v));
            }
            return None;
        }
        if let Ok(v) = i64::try_from(intv) {
            return Some(Value::Number(Number::from(v)));
        }
        if let Ok(v) = u64::try_from(intv) {
            return Some(Value::Number(Number::from(v)));
        }
        if let Some(v) = Number::from_f64(intv as f64) {
            return Some(Value::Number(v));
        }
        return None;
    }

    if let Some(fv) = parse_go_hex_float_literal(body) {
        let signed = if negative { -fv } else { fv };
        return Number::from_f64(signed).map(Value::Number);
    }

    if let Ok(v) = compact.parse::<i64>() {
        return Some(Value::Number(Number::from(v)));
    }
    if let Ok(v) = compact.parse::<u64>() {
        return Some(Value::Number(Number::from(v)));
    }
    if let Ok(v) = compact.parse::<f64>() {
        return Number::from_f64(v).map(Value::Number);
    }
    None
}

pub fn parse_char_constant(expr: &str) -> Option<i64> {
    if !(expr.starts_with('\'') && expr.ends_with('\'')) || expr.len() < 3 {
        return None;
    }
    let inner = &expr[1..expr.len() - 1];
    if let Some(rest) = inner.strip_prefix('\\') {
        let codepoint = parse_go_char_escape(rest)?;
        return Some(i64::from(codepoint));
    }
    let mut chars = inner.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(i64::from(first as u32))
}

pub fn decode_go_string_literal(expr: &str) -> Option<String> {
    if expr.len() < 2 {
        return None;
    }
    let bytes = expr.as_bytes();
    if bytes[0] == b'`' && bytes[expr.len() - 1] == b'`' {
        return Some(expr[1..expr.len() - 1].to_string());
    }
    if bytes[0] != b'"' || bytes[expr.len() - 1] != b'"' {
        return None;
    }
    decode_go_interpreted_string_body(&expr[1..expr.len() - 1])
}

pub fn parse_go_quoted_prefix(input: &str) -> Option<(String, &str)> {
    let mut iter = input.char_indices();
    let (start, quote) = iter.next()?;
    if start != 0 {
        return None;
    }

    match quote {
        '`' => {
            let end = input[1..].find('`')? + 1;
            let lit = &input[..=end];
            let decoded = decode_go_string_literal(lit)?;
            let tail = &input[end + 1..];
            Some((decoded, tail))
        }
        '"' => {
            let mut i = 1usize;
            let bytes = input.as_bytes();
            let mut escaped = false;
            while i < bytes.len() {
                let b = bytes[i];
                if escaped {
                    escaped = false;
                    i += 1;
                    continue;
                }
                if b == b'\\' {
                    escaped = true;
                    i += 1;
                    continue;
                }
                if b == b'"' {
                    let lit = &input[..=i];
                    let decoded = decode_go_string_literal(lit)?;
                    let tail = &input[i + 1..];
                    return Some((decoded, tail));
                }
                i += 1;
            }
            None
        }
        _ => None,
    }
}

fn has_valid_go_numeric_underscores(expr: &str) -> bool {
    if !expr.contains('_') {
        return true;
    }
    let body = expr
        .strip_prefix('+')
        .or_else(|| expr.strip_prefix('-'))
        .unwrap_or(expr);
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') {
        return false;
    }

    let base = if body.starts_with("0b") || body.starts_with("0B") {
        2u32
    } else if body.starts_with("0o") || body.starts_with("0O") {
        8u32
    } else if body.starts_with("0x") || body.starts_with("0X") {
        16u32
    } else {
        10u32
    };

    let bytes = body.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'_' {
            continue;
        }
        if i == 0 || i + 1 >= bytes.len() {
            return false;
        }
        let prev = bytes[i - 1] as char;
        let next = bytes[i + 1] as char;

        let is_after_prefix =
            i == 2 && matches!(&body[..2], "0b" | "0B" | "0o" | "0O" | "0x" | "0X");
        if is_after_prefix {
            if !is_digit_for_base(next, base) {
                return false;
            }
            continue;
        }

        if !(is_digit_for_base(prev, base) && is_digit_for_base(next, base)) {
            return false;
        }
    }
    true
}

fn is_digit_for_base(ch: char, base: u32) -> bool {
    match base {
        2 => matches!(ch, '0' | '1'),
        8 => matches!(ch, '0'..='7'),
        10 => ch.is_ascii_digit(),
        16 => ch.is_ascii_hexdigit(),
        _ => false,
    }
}

fn parse_go_integer_literal(body: &str) -> Option<u128> {
    if body.is_empty() {
        return None;
    }
    if let Some(rest) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
        if rest.is_empty() || !rest.bytes().all(|b| matches!(b, b'0' | b'1')) {
            return None;
        }
        return u128::from_str_radix(rest, 2).ok();
    }
    if let Some(rest) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) {
        if rest.is_empty() || !rest.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return None;
        }
        return u128::from_str_radix(rest, 8).ok();
    }
    if let Some(rest) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if rest.is_empty() || !rest.bytes().all(|b| (b as char).is_ascii_hexdigit()) {
            return None;
        }
        return u128::from_str_radix(rest, 16).ok();
    }

    if body.len() > 1 && body.starts_with('0') && body.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return u128::from_str_radix(body, 8).ok();
    }

    if body.bytes().all(|b| b.is_ascii_digit()) {
        return body.parse::<u128>().ok();
    }
    None
}

fn parse_go_hex_float_literal(body: &str) -> Option<f64> {
    let lower = body.to_ascii_lowercase();
    let rest = lower.strip_prefix("0x")?;
    let p_idx = rest.find('p')?;
    let mantissa = &rest[..p_idx];
    let exp_str = &rest[p_idx + 1..];
    if mantissa.is_empty() || exp_str.is_empty() {
        return None;
    }
    let exp = exp_str.parse::<i32>().ok()?;

    let (int_part, frac_part) = if let Some(dot_idx) = mantissa.find('.') {
        (&mantissa[..dot_idx], &mantissa[dot_idx + 1..])
    } else {
        (mantissa, "")
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }

    let mut value = 0f64;
    if !int_part.is_empty() {
        for ch in int_part.chars() {
            let d = ch.to_digit(16)? as f64;
            value = value * 16.0 + d;
        }
    }
    if !frac_part.is_empty() {
        let mut scale = 16.0;
        for ch in frac_part.chars() {
            let d = ch.to_digit(16)? as f64;
            value += d / scale;
            scale *= 16.0;
        }
    }
    Some(value * 2f64.powi(exp))
}

fn parse_go_char_escape(rest: &str) -> Option<u32> {
    if rest.is_empty() {
        return None;
    }
    match rest {
        "a" => return Some('\u{0007}' as u32),
        "b" => return Some('\u{0008}' as u32),
        "f" => return Some('\u{000C}' as u32),
        "n" => return Some('\n' as u32),
        "r" => return Some('\r' as u32),
        "t" => return Some('\t' as u32),
        "v" => return Some('\u{000B}' as u32),
        "\\" => return Some('\\' as u32),
        "'" => return Some('\'' as u32),
        "\"" => return Some('"' as u32),
        _ => {}
    }

    if let Some(hex) = rest.strip_prefix('x') {
        if hex.len() != 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(hex) = rest.strip_prefix('u') {
        if hex.len() != 4 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(v).map(|ch| ch as u32);
    }
    if let Some(hex) = rest.strip_prefix('U') {
        if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(v).map(|ch| ch as u32);
    }

    if rest.chars().all(|c| matches!(c, '0'..='7')) && rest.len() == 3 {
        let v = u32::from_str_radix(rest, 8).ok()?;
        if v > 0xFF {
            return None;
        }
        return Some(v);
    }

    None
}

fn decode_go_interpreted_string_body(body: &str) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            let (ch, consumed) = decode_go_escape(&bytes[i + 1..])?;
            out.push(ch);
            i += consumed + 1;
            continue;
        }
        if b == b'\n' || b == b'\r' {
            return None;
        }
        let rem = &body[i..];
        let ch = rem.chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }
    Some(out)
}

fn decode_go_escape(rest: &[u8]) -> Option<(char, usize)> {
    let first = *rest.first()?;
    match first {
        b'a' => return Some(('\u{0007}', 1)),
        b'b' => return Some(('\u{0008}', 1)),
        b'f' => return Some(('\u{000C}', 1)),
        b'n' => return Some(('\n', 1)),
        b'r' => return Some(('\r', 1)),
        b't' => return Some(('\t', 1)),
        b'v' => return Some(('\u{000B}', 1)),
        b'\\' => return Some(('\\', 1)),
        b'\'' => return Some(('\'', 1)),
        b'"' => return Some(('"', 1)),
        b'x' => {
            let value = parse_n_hex(rest.get(1..3)?)?;
            return Some((char::from_u32(value)?, 3));
        }
        b'u' => {
            let value = parse_n_hex(rest.get(1..5)?)?;
            return Some((char::from_u32(value)?, 5));
        }
        b'U' => {
            let value = parse_n_hex(rest.get(1..9)?)?;
            return Some((char::from_u32(value)?, 9));
        }
        b'0'..=b'7' => {
            if rest.len() < 3 {
                return None;
            }
            if !rest[..3].iter().all(|b| matches!(*b, b'0'..=b'7')) {
                return None;
            }
            let oct = std::str::from_utf8(&rest[..3]).ok()?;
            let value = u32::from_str_radix(oct, 8).ok()?;
            if value > 0xFF {
                return None;
            }
            return Some((char::from_u32(value)?, 3));
        }
        _ => return None,
    }
}

fn parse_n_hex(digits: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(digits).ok()?;
    if !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}
