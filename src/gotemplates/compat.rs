use serde_json::{Number, Value};

pub fn looks_like_numeric_literal(expr: &str) -> bool {
    let body = expr
        .strip_prefix('+')
        .or_else(|| expr.strip_prefix('-'))
        .unwrap_or(expr);
    body.as_bytes()
        .first()
        .is_some_and(|ch| ch.is_ascii_digit())
}

pub fn looks_like_char_literal(expr: &str) -> bool {
    expr.len() >= 2 && expr.starts_with('\'') && expr.ends_with('\'')
}

pub fn parse_width_zero_precision(flags: &str) -> (Option<usize>, bool, Option<usize>) {
    let mut zero = false;
    let mut width_digits = String::new();
    let mut precision_digits = String::new();
    let mut in_precision = false;
    let mut saw_width = false;
    let mut saw_dot = false;
    for ch in flags.chars() {
        match ch {
            '.' if !in_precision => {
                in_precision = true;
                saw_dot = true;
            }
            '0' if !in_precision && !saw_width && width_digits.is_empty() => {
                zero = true;
            }
            '0'..='9' if in_precision => {
                precision_digits.push(ch);
            }
            '0'..='9' => {
                saw_width = true;
                width_digits.push(ch);
            }
            _ => {}
        }
    }
    let width = if width_digits.is_empty() {
        None
    } else {
        width_digits.parse::<usize>().ok()
    };
    let precision = if saw_dot {
        if precision_digits.is_empty() {
            Some(0)
        } else {
            precision_digits.parse::<usize>().ok()
        }
    } else {
        None
    };
    (width, zero, precision)
}

pub fn format_signed_integer_radix(n: i64, verb: char) -> String {
    let abs = n.unsigned_abs();
    let body = match verb {
        'x' => format!("{:x}", abs),
        'X' => format!("{:X}", abs),
        'o' => format!("{:o}", abs),
        'b' => format!("{:b}", abs),
        _ => abs.to_string(),
    };
    if n < 0 {
        format!("-{body}")
    } else {
        body
    }
}

pub fn format_float_exp_go(n: f64, precision: usize, upper: bool) -> String {
    let raw = if upper {
        format!("{:.*E}", precision, n)
    } else {
        format!("{:.*e}", precision, n)
    };
    normalize_scientific_exponent(&raw, upper)
}

pub fn format_float_general_go(n: f64, precision: usize, upper: bool) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    let p = if precision == 0 { 1 } else { precision };
    let abs = n.abs();
    let exp10 = abs.log10().floor() as i32;
    let use_exp = exp10 < -4 || exp10 >= p as i32;
    if use_exp {
        let mut s = format_float_exp_go(n, p.saturating_sub(1), upper);
        trim_fraction_zeros(&mut s, upper);
        return s;
    }

    let frac_digits = (p as i32 - (exp10 + 1)).max(0) as usize;
    let mut s = format!("{:.*}", frac_digits, n);
    trim_trailing_zeros_fixed(&mut s);
    s
}

fn format_float_general_go_default(n: f64, upper: bool) -> String {
    if n == 0.0 {
        return if n.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }
    let sign = if n.is_sign_negative() { "-" } else { "" };
    let abs = n.abs();
    let (digits, exp10) = shortest_decimal_components(abs);
    if digits == "0" {
        return format!("{sign}0");
    }
    let use_exp = exp10 < -4 || exp10 >= 6;
    if use_exp {
        return format!("{sign}{}", to_scientific_from_digits(&digits, exp10, upper));
    }
    format!("{sign}{}", to_fixed_from_digits(&digits, exp10))
}

pub fn go_printf(fmt: &str, args: &[Option<Value>]) -> Result<String, String> {
    let mut out = String::with_capacity(fmt.len() + 8);
    let mut i = 0usize;
    let mut argi = 0usize;
    let bytes = fmt.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
            out.push('%');
            i += 2;
            continue;
        }

        let spec_start = i;
        i += 1;
        while i < bytes.len()
            && matches!(
                bytes[i] as char,
                '+' | '-' | '#' | ' ' | '0' | '.' | '1'..='9'
            )
        {
            i += 1;
        }
        if i >= bytes.len() {
            return Err("printf has incomplete format specifier".to_string());
        }
        let spec_flags = &fmt[spec_start + 1..i];
        let verb = bytes[i] as char;
        i += 1;
        let (width, zero_pad, precision) = parse_width_zero_precision(spec_flags);
        let plus_flag = spec_flags.contains('+');
        let space_flag = !plus_flag && spec_flags.contains(' ');
        let alt_flag = spec_flags.contains('#');

        let Some(arg) = args.get(argi) else {
            return Err("printf argument count mismatch".to_string());
        };
        argi += 1;

        match verb {
            'v' | 's' => out.push_str(&format_value_for_printf(arg, verb)),
            'T' => out.push_str(&format_type_for_printf(arg)),
            'q' => {
                if alt_flag {
                    if let Some(Value::String(s)) = arg.as_ref() {
                        if can_backquote_string(s) {
                            out.push('`');
                            out.push_str(s);
                            out.push('`');
                        } else {
                            out.push_str(&format!("{s:?}"));
                        }
                        continue;
                    }
                }
                let rendered = format_value_for_printf(arg, 's');
                out.push_str(&format!("{rendered:?}"));
            }
            'd' => {
                if let Some(n) = value_to_i64(arg) {
                    let rendered =
                        apply_printf_sign_flags(n.to_string(), n >= 0, plus_flag, space_flag);
                    push_with_width(&mut out, &rendered, width, zero_pad);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            'x' | 'X' | 'o' | 'b' => {
                if let Some(n) = value_to_i64(arg) {
                    let rendered = apply_printf_sign_flags(
                        format_signed_integer_radix(n, verb),
                        n >= 0,
                        plus_flag,
                        space_flag,
                    );
                    push_with_width(&mut out, &rendered, width, zero_pad);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            'f' | 'e' | 'E' | 'g' | 'G' => {
                if let Some(n) = value_to_f64(arg) {
                    let rendered = match verb {
                        'f' => format!("{:.*}", precision.unwrap_or(6), n),
                        'e' => format_float_exp_go(n, precision.unwrap_or(6), false),
                        'E' => format_float_exp_go(n, precision.unwrap_or(6), true),
                        'g' => {
                            if let Some(prec) = precision {
                                format_float_general_go(n, prec, false)
                            } else {
                                format_float_general_go_default(n, false)
                            }
                        }
                        'G' => {
                            if let Some(prec) = precision {
                                format_float_general_go(n, prec, true)
                            } else {
                                format_float_general_go_default(n, true)
                            }
                        }
                        _ => String::new(),
                    };
                    let rendered = apply_printf_sign_flags(
                        rendered,
                        !n.is_sign_negative(),
                        plus_flag,
                        space_flag,
                    );
                    push_with_width(&mut out, &rendered, width, zero_pad);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            't' => {
                if let Some(b) = value_to_bool(arg) {
                    out.push_str(&b.to_string());
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            _ => return Err(format!("printf verb %{verb} is not supported")),
        }
    }

    if argi != args.len() {
        return Err("printf argument count mismatch".to_string());
    }
    Ok(out)
}

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

fn normalize_scientific_exponent(raw: &str, upper: bool) -> String {
    let sep = if upper { 'E' } else { 'e' };
    let Some((mantissa, exp_raw)) = raw.split_once(sep) else {
        return raw.to_string();
    };
    let exp = exp_raw.parse::<i32>().ok();
    let Some(exp) = exp else {
        return raw.to_string();
    };
    let sign = if exp >= 0 { '+' } else { '-' };
    let abs = exp.unsigned_abs();
    format!("{mantissa}{sep}{sign}{abs:02}")
}

fn trim_trailing_zeros_fixed(s: &mut String) {
    if !s.contains('.') {
        return;
    }
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
}

fn trim_fraction_zeros(s: &mut String, upper: bool) {
    let sep = if upper { 'E' } else { 'e' };
    let Some(idx) = s.find(sep) else {
        trim_trailing_zeros_fixed(s);
        return;
    };
    let mut mantissa = s[..idx].to_string();
    trim_trailing_zeros_fixed(&mut mantissa);
    let exp = &s[idx..];
    *s = format!("{mantissa}{exp}");
}

fn shortest_decimal_components(v: f64) -> (String, i32) {
    let raw = v.to_string();
    if let Some((mantissa, exp_raw)) = raw.split_once(['e', 'E']) {
        let exp = exp_raw.parse::<i32>().unwrap_or(0);
        let mut digits = String::with_capacity(mantissa.len());
        let mut int_len = 0usize;
        for ch in mantissa.chars() {
            if ch == '.' {
                continue;
            }
            if ch == '-' || ch == '+' {
                continue;
            }
            if ch.is_ascii_digit() {
                digits.push(ch);
                if int_len == 0 && !mantissa.contains('.') {
                    int_len += 1;
                }
            }
        }
        if let Some(dot) = mantissa.find('.') {
            int_len = dot.saturating_sub(usize::from(mantissa.starts_with(['-', '+'])));
        } else if int_len == 0 {
            int_len = digits.len();
        }
        while digits.starts_with('0') && digits.len() > 1 {
            digits.remove(0);
        }
        while digits.ends_with('0') && digits.len() > 1 {
            digits.pop();
        }
        if digits.is_empty() {
            return ("0".to_string(), 0);
        }
        let exp10 = exp + (int_len as i32) - 1;
        return (digits, exp10);
    }

    if let Some((int_part, frac_part)) = raw.split_once('.') {
        let mut digits = String::new();
        if int_part != "0" {
            digits.push_str(int_part);
            digits.push_str(frac_part);
            while digits.ends_with('0') && digits.len() > 1 {
                digits.pop();
            }
            return (digits, int_part.len() as i32 - 1);
        }

        let first = frac_part
            .chars()
            .position(|ch| ch != '0')
            .unwrap_or(frac_part.len());
        if first == frac_part.len() {
            return ("0".to_string(), 0);
        }
        digits.push_str(&frac_part[first..]);
        while digits.ends_with('0') && digits.len() > 1 {
            digits.pop();
        }
        return (digits, -(first as i32) - 1);
    }

    let mut digits = raw;
    while digits.ends_with('0') && digits.len() > 1 {
        digits.pop();
    }
    let exp10 = digits.len() as i32 - 1;
    (digits, exp10)
}

fn to_scientific_from_digits(digits: &str, exp10: i32, upper: bool) -> String {
    let mut out = String::with_capacity(digits.len() + 6);
    let mut chars = digits.chars();
    if let Some(first) = chars.next() {
        out.push(first);
        let rest: String = chars.collect();
        if !rest.is_empty() {
            out.push('.');
            out.push_str(&rest);
        }
    } else {
        out.push('0');
    }
    out.push(if upper { 'E' } else { 'e' });
    out.push(if exp10 >= 0 { '+' } else { '-' });
    let abs = exp10.unsigned_abs();
    if abs < 10 {
        out.push('0');
        out.push((b'0' + abs as u8) as char);
    } else {
        out.push_str(&abs.to_string());
    }
    out
}

fn to_fixed_from_digits(digits: &str, exp10: i32) -> String {
    let len = digits.len() as i32;
    if exp10 >= len - 1 {
        let mut out = String::with_capacity(exp10 as usize + 1);
        out.push_str(digits);
        for _ in 0..(exp10 - (len - 1)) {
            out.push('0');
        }
        return out;
    }
    if exp10 >= 0 {
        let split = (exp10 + 1) as usize;
        let (left, right) = digits.split_at(split);
        return format!("{left}.{right}");
    }
    let mut out = String::with_capacity(digits.len() + (-exp10) as usize + 2);
    out.push_str("0.");
    for _ in 0..(-exp10 - 1) {
        out.push('0');
    }
    out.push_str(digits);
    out
}

fn can_backquote_string(s: &str) -> bool {
    if s.contains('`') || s.contains('\r') {
        return false;
    }
    s.chars().all(|ch| ch == '\t' || !ch.is_control())
}

fn push_with_width(out: &mut String, rendered: &str, width: Option<usize>, zero_pad: bool) {
    let Some(width) = width else {
        out.push_str(rendered);
        return;
    };
    if rendered.len() >= width {
        out.push_str(rendered);
        return;
    }
    let pad_len = width - rendered.len();
    let pad_ch = if zero_pad { '0' } else { ' ' };
    if zero_pad && rendered.starts_with(['-', '+', ' ']) {
        let mut chars = rendered.chars();
        let sign = chars.next().unwrap_or_default();
        out.push(sign);
        for _ in 0..pad_len {
            out.push(pad_ch);
        }
        out.push_str(chars.as_str());
        return;
    }
    for _ in 0..pad_len {
        out.push(pad_ch);
    }
    out.push_str(rendered);
}

fn apply_printf_sign_flags(
    rendered: String,
    non_negative: bool,
    plus_flag: bool,
    space_flag: bool,
) -> String {
    if !non_negative || rendered.starts_with(['-', '+', ' ']) {
        return rendered;
    }
    if plus_flag {
        return format!("+{rendered}");
    }
    if space_flag {
        return format!(" {rendered}");
    }
    rendered
}

fn value_to_bool(v: &Option<Value>) -> Option<bool> {
    v.as_ref().and_then(Value::as_bool)
}

fn value_to_i64(v: &Option<Value>) -> Option<i64> {
    match v.as_ref() {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        _ => None,
    }
}

fn value_to_f64(v: &Option<Value>) -> Option<f64> {
    match v.as_ref() {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

fn format_value_for_printf(v: &Option<Value>, verb: char) -> String {
    match (verb, v) {
        (_, None) | (_, Some(Value::Null)) => "<nil>".to_string(),
        ('s', Some(Value::String(s))) => s.clone(),
        ('v', Some(value)) => format_value_like_go(value),
        (_, Some(Value::String(s))) => s.clone(),
        (_, Some(value)) => format_value_like_go(value),
    }
}

fn format_type_for_printf(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(other) => printf_type_name(other),
    }
}

fn format_printf_mismatch(verb: char, arg: &Option<Value>) -> String {
    match arg {
        None | Some(Value::Null) => format!("%!{verb}(<nil>)"),
        Some(v) => {
            let type_name = printf_type_name(v);
            let value = format_value_like_go(v);
            format!("%!{verb}({type_name}={value})")
        }
    }
}

fn printf_type_name(v: &Value) -> String {
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(_) => "[]interface {}".to_string(),
        Value::Object(_) => "map[string]interface {}".to_string(),
        Value::Number(n) => {
            if n.as_i64().is_some() {
                "int".to_string()
            } else if n.as_u64().is_some() {
                "uint".to_string()
            } else {
                "float64".to_string()
            }
        }
    }
}

fn format_value_like_go(v: &Value) -> String {
    match v {
        Value::Null => "<no value>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = String::from("[");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format_value_like_go(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            let mut out = String::from("map[");
            for (idx, k) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(k);
                out.push(':');
                if let Some(v) = map.get(*k) {
                    out.push_str(&format_value_like_go(v));
                }
            }
            out.push(']');
            out
        }
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

    if rest.chars().all(|c| matches!(c, '0'..='7')) && rest.len() <= 3 {
        let v = u32::from_str_radix(rest, 8).ok()?;
        if v > 0xFF {
            return None;
        }
        return Some(v);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_zero_precision_parser_matches_go_shape() {
        assert_eq!(parse_width_zero_precision("04"), (Some(4), true, None));
        assert_eq!(parse_width_zero_precision(".2"), (None, false, Some(2)));
        assert_eq!(parse_width_zero_precision("08.3"), (Some(8), true, Some(3)));
    }

    #[test]
    fn scientific_format_has_go_exponent_sign() {
        assert_eq!(format_float_exp_go(1.2, 6, false), "1.200000e+00");
        assert_eq!(format_float_exp_go(1.2, 6, true), "1.200000E+00");
    }

    #[test]
    fn general_float_format_matches_basic_go_shapes() {
        assert_eq!(format_float_general_go(3.5, 6, false), "3.5");
        assert_eq!(format_float_general_go(1234567.0, 6, true), "1.23457E+06");
    }

    #[test]
    fn number_parser_supports_go_literals_and_rejects_invalid_underscore() {
        assert_eq!(
            parse_number_value("0b_101"),
            Some(Value::Number(Number::from(5)))
        );
        assert_eq!(
            parse_number_value("+0x_1.e_0p+0_2"),
            Number::from_f64(7.5).map(Value::Number)
        );
        assert_eq!(parse_number_value("1__2"), None);
    }

    #[test]
    fn char_parser_supports_go_escapes() {
        assert_eq!(parse_char_constant("'\\n'"), Some(10));
        assert_eq!(parse_char_constant("'\\x41'"), Some(65));
        assert_eq!(parse_char_constant("'\\u263A'"), Some(9786));
        assert_eq!(parse_char_constant("'\\400'"), None);
    }
}
