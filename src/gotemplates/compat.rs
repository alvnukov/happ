use serde_json::{Number, Value};
use super::typedvalue::decode_go_bytes_value;

const GO_PRINTF_NUM_LIMIT: usize = 1_000_000;

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
    let mut reordered = false;
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
                '+' | '-' | '#' | ' ' | '0' | '.' | '*' | '[' | ']' | '1'..='9'
            )
        {
            i += 1;
        }
        if i >= bytes.len() {
            let tail_spec = &fmt[spec_start + 1..i];
            let parsed_tail = parse_printf_spec_flags(tail_spec);
            if parsed_tail.arg_index.is_some()
                || parsed_tail.width_arg_index.is_some()
                || parsed_tail.precision_arg_index.is_some()
                || parsed_tail.bad_index
            {
                reordered = true;
            }
            if parsed_tail.width_from_arg && parsed_tail.width_arg_index.is_none() && argi < args.len()
            {
                argi += 1;
            }
            if parsed_tail.precision_from_arg
                && parsed_tail.precision_arg_index.is_none()
                && argi < args.len()
            {
                argi += 1;
            }
            if matches!(bytes.last(), Some(b']')) && parsed_tail.bad_index {
                out.push_str(&format_bad_index(']'));
                continue;
            }
            out.push_str("%!(NOVERB)");
            break;
        }
        let spec_flags = &fmt[spec_start + 1..i];
        let verb = bytes[i] as char;
        i += 1;
        let spec = parse_printf_spec_flags(spec_flags);
        if spec.no_verb {
            out.push_str("%!(NOVERB)");
            continue;
        }
        if spec.bad_index {
            if spec_flags.contains('[') {
                reordered = true;
            }
            out.push_str(&format_bad_index(verb));
            continue;
        }
        let mut good_arg_num = true;
        if let Some(idx) = spec.arg_index {
            reordered = true;
            if idx < args.len() {
                argi = idx;
            } else {
                good_arg_num = false;
            }
        }
        let mut width = spec.width;
        let mut precision = spec.precision;
        let mut left_align = spec.minus;
        let mut zero_pad = spec.zero;
        let plus_flag = spec.plus;
        let space_flag = !plus_flag && spec.space;
        let alt_flag = spec.sharp;

        if spec.width_from_arg {
            let width_idx = if let Some(idx) = spec.width_arg_index {
                reordered = true;
                if idx >= args.len() {
                    good_arg_num = false;
                }
                idx
            } else {
                argi
            };
            if good_arg_num {
                match args.get(width_idx) {
                    Some(v) => match value_to_int_for_width_prec(v) {
                        Some(n) => {
                            let abs = n.unsigned_abs();
                            let abs_usize = usize::try_from(abs).ok();
                            if abs_usize.map_or(true, |v| v > GO_PRINTF_NUM_LIMIT) {
                                out.push_str("%!(BADWIDTH)");
                            } else if n < 0 {
                                left_align = true;
                                zero_pad = false;
                                width = abs_usize;
                            } else {
                                width = abs_usize;
                            }
                        }
                        None => out.push_str("%!(BADWIDTH)"),
                    },
                    None => out.push_str("%!(BADWIDTH)"),
                }
            }
            if spec.width_arg_index.is_some() {
                argi = width_idx.saturating_add(1);
            } else if argi < args.len() {
                argi += 1;
            }
        }

        if spec.precision_from_arg {
            let prec_idx = if let Some(idx) = spec.precision_arg_index {
                reordered = true;
                if idx >= args.len() {
                    good_arg_num = false;
                }
                idx
            } else {
                argi
            };
            if good_arg_num {
                match args.get(prec_idx) {
                    Some(v) => match value_to_int_for_width_prec(v) {
                        Some(n) if n >= 0 => {
                            let p = usize::try_from(n).ok();
                            if p.map_or(true, |v| v > GO_PRINTF_NUM_LIMIT) {
                                out.push_str("%!(BADPREC)");
                            } else {
                                precision = p;
                            }
                        }
                        Some(_) => precision = None,
                        None => out.push_str("%!(BADPREC)"),
                    },
                    None => out.push_str("%!(BADPREC)"),
                }
            }
            if spec.precision_arg_index.is_some() {
                argi = prec_idx.saturating_add(1);
            } else if argi < args.len() {
                argi += 1;
            }
        }

        if verb == '%' {
            out.push('%');
            continue;
        }
        if !good_arg_num {
            out.push_str(&format_bad_index(verb));
            continue;
        }

        let Some(arg) = args.get(argi) else {
            out.push_str(&format_missing_arg(verb));
            continue;
        };
        argi += 1;

        match verb {
            'v' => {
                let rendered = format_value_for_printf(arg, verb, alt_flag);
                push_with_width(&mut out, &rendered, width, zero_pad, left_align);
            }
            's' => {
                let mut rendered = if let Some(Value::String(s)) = arg.as_ref() {
                    s.clone()
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                    continue;
                };
                if let Some(prec) = precision {
                    rendered = truncate_runes(&rendered, prec);
                }
                push_with_width(&mut out, &rendered, width, zero_pad, left_align);
            }
            'T' => {
                let rendered = format_type_for_printf(arg);
                push_with_width(&mut out, &rendered, width, zero_pad, left_align);
            }
            'q' => {
                let Some(rendered) = format_q_verb_go(arg, plus_flag, alt_flag) else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                    continue;
                };
                if alt_flag && matches!(arg, Some(Value::String(_))) {
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                    continue;
                }
                push_with_width(&mut out, &rendered, width, zero_pad, left_align);
            }
            'd' => {
                if let Some(n) = value_to_i64(arg) {
                    let rendered = format_integer_go(
                        n, verb, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    );
                    push_with_width(&mut out, &rendered, width, false, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = format_byte_slice_list_go(&bytes, 'd', alt_flag, precision);
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            'x' | 'X' | 'o' | 'b' => {
                if let Some(n) = value_to_i64(arg) {
                    let rendered = format_integer_go(
                        n, verb, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    );
                    push_with_width(&mut out, &rendered, width, false, left_align);
                } else if matches!(arg, Some(Value::String(_))) && matches!(verb, 'x' | 'X') {
                    let Some(Value::String(s)) = arg.as_ref() else {
                        unreachable!();
                    };
                    let rendered =
                        format_bytes_hex_go(s.as_bytes(), verb == 'X', alt_flag, space_flag, precision);
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = if matches!(verb, 'x' | 'X') {
                        format_bytes_hex_go(&bytes, verb == 'X', alt_flag, space_flag, precision)
                    } else {
                        format_byte_slice_list_go(&bytes, verb, alt_flag, precision)
                    };
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
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
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            't' => {
                if let Some(b) = value_to_bool(arg) {
                    let rendered = b.to_string();
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            'c' => {
                if let Some(r) = value_to_rune_go(arg) {
                    let rendered = r.to_string();
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = format_byte_slice_list_go(&bytes, 'c', alt_flag, precision);
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            'U' => {
                if let Some(u) = value_to_u64_for_unicode(arg) {
                    let rendered = format_unicode_verb_go(u, alt_flag, precision);
                    push_with_width(&mut out, &rendered, width, false, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = format_byte_slice_list_go(&bytes, 'U', alt_flag, precision);
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch(verb, arg));
                }
            }
            _ => out.push_str(&format_printf_mismatch(verb, arg)),
        }
    }

    if !reordered && argi < args.len() {
        out.push_str(&format_extra_args(&args[argi..]));
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

    let digits = raw;
    let exp10 = digits.len() as i32 - 1;
    (digits, exp10)
}

fn to_scientific_from_digits(digits: &str, exp10: i32, upper: bool) -> String {
    let mut out = String::with_capacity(digits.len() + 6);
    let mut chars = digits.chars();
    if let Some(first) = chars.next() {
        out.push(first);
        let rest_raw: String = chars.collect();
        let rest = rest_raw.trim_end_matches('0');
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
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

fn push_with_width(
    out: &mut String,
    rendered: &str,
    width: Option<usize>,
    zero_pad: bool,
    left_align: bool,
) {
    let Some(width) = width else {
        out.push_str(rendered);
        return;
    };
    let rendered_width = rendered.chars().count();
    if rendered_width >= width {
        out.push_str(rendered);
        return;
    }
    let pad_len = width - rendered_width;
    let pad_ch = if zero_pad && !left_align { '0' } else { ' ' };
    if left_align {
        out.push_str(rendered);
        for _ in 0..pad_len {
            out.push(' ');
        }
        return;
    }
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

fn truncate_runes(s: &str, max_runes: usize) -> String {
    if max_runes == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max_runes {
            break;
        }
        out.push(ch);
    }
    out
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

fn format_q_verb_go(arg: &Option<Value>, plus: bool, sharp: bool) -> Option<String> {
    let Some(value) = arg.as_ref() else {
        return None;
    };
    format_q_value_ref(value, plus, sharp)
}

fn format_q_value_ref(value: &Value, plus: bool, sharp: bool) -> Option<String> {
    if let Some(bytes) = value_as_byte_slice(value) {
        return Some(quote_bytes_go(&bytes, plus));
    }
    match value {
        Value::String(s) => {
            if sharp {
                if can_backquote_string(s) {
                    let mut raw = String::with_capacity(s.len() + 2);
                    raw.push('`');
                    raw.push_str(s);
                    raw.push('`');
                    return Some(raw);
                }
                return Some(format!("{s:?}"));
            }
            if plus {
                return Some(quote_string_ascii_go(s));
            }
            Some(format!("{s:?}"))
        }
        Value::Array(items) => Some(format_q_array_go(items, plus, sharp)),
        Value::Number(n) => value_number_to_rune_go(n).map(quote_rune_go),
        _ => None,
    }
}

fn format_q_array_go(items: &[Value], plus: bool, sharp: bool) -> String {
    let mut out = String::from("[");
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let rendered = format_q_value_ref(item, plus, sharp)
            .unwrap_or_else(|| format_printf_mismatch('q', &Some(item.clone())));
        out.push_str(&rendered);
    }
    out.push(']');
    out
}

fn quote_rune_go(ch: char) -> String {
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

fn quote_bytes_go(bytes: &[u8], _ascii_only: bool) -> String {
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

fn format_integer_go(
    n: i64,
    verb: char,
    plus: bool,
    space: bool,
    sharp: bool,
    zero: bool,
    minus: bool,
    width: Option<usize>,
    precision: Option<usize>,
) -> String {
    let negative = n < 0;
    let u = n.unsigned_abs();
    let mut body = match verb {
        'd' => u.to_string(),
        'x' => format!("{u:x}"),
        'X' => format!("{u:X}"),
        'o' => format!("{u:o}"),
        'b' => format!("{u:b}"),
        _ => u.to_string(),
    };

    if precision == Some(0) && u == 0 {
        body.clear();
    }

    if let Some(p) = precision {
        while body.len() < p {
            body.insert(0, '0');
        }
    }

    let mut prefix = String::new();
    if negative {
        prefix.push('-');
    } else if plus {
        prefix.push('+');
    } else if space {
        prefix.push(' ');
    }

    if sharp {
        match verb {
            'x' => prefix.push_str("0x"),
            'X' => prefix.push_str("0X"),
            'b' => prefix.push_str("0b"),
            'o' => {
                if !body.starts_with('0') {
                    prefix.push('0');
                }
            }
            _ => {}
        }
    }

    if precision.is_none() && zero && !minus {
        if let Some(w) = width {
            let cur = prefix.len() + body.len();
            if cur < w {
                let zeros = w - cur;
                let mut z = String::with_capacity(zeros + body.len());
                for _ in 0..zeros {
                    z.push('0');
                }
                z.push_str(&body);
                body = z;
            }
        }
    }

    let mut rendered = prefix;
    rendered.push_str(&body);
    rendered
}

fn format_unicode_verb_go(u: u64, sharp: bool, precision: Option<usize>) -> String {
    let mut body = format!("{u:X}");
    let mut min_digits = 4usize;
    if let Some(p) = precision {
        if p > min_digits {
            min_digits = p;
        }
    }
    while body.len() < min_digits {
        body.insert(0, '0');
    }
    let mut out = String::from("U+");
    out.push_str(&body);
    if sharp && u <= 0x10FFFF {
        if let Some(ch) = char::from_u32(u as u32) {
            if !ch.is_control() {
                out.push_str(" '");
                out.push(ch);
                out.push('\'');
            }
        }
    }
    out
}

fn format_bytes_hex_go(
    bytes: &[u8],
    upper: bool,
    alt: bool,
    spaced: bool,
    precision: Option<usize>,
) -> String {
    let length = precision.unwrap_or(bytes.len()).min(bytes.len());
    if length == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(length * if spaced { 5 } else { 2 } + 4);
    let prefix = if upper { "0X" } else { "0x" };

    if !spaced {
        if alt {
            out.push_str(prefix);
        }
        for &b in bytes.iter().take(length) {
            push_hex_byte(&mut out, b, upper);
        }
        return out;
    }

    for (idx, &b) in bytes.iter().take(length).enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        if alt {
            out.push_str(prefix);
        }
        push_hex_byte(&mut out, b, upper);
    }
    out
}

fn format_byte_slice_list_go(
    bytes: &[u8],
    verb: char,
    sharp: bool,
    precision: Option<usize>,
) -> String {
    let mut out = String::from("[");
    for (idx, b) in bytes.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let item = match verb {
            'd' | 'v' => b.to_string(),
            'o' => {
                let n = i64::from(*b);
                format_integer_go(n, 'o', false, false, sharp, false, false, None, precision)
            }
            'b' => {
                let n = i64::from(*b);
                format_integer_go(n, 'b', false, false, sharp, false, false, None, precision)
            }
            'c' => {
                let ch = char::from_u32(u32::from(*b)).unwrap_or('\u{FFFD}');
                ch.to_string()
            }
            'U' => format_unicode_verb_go(u64::from(*b), sharp, precision),
            _ => b.to_string(),
        };
        out.push_str(&item);
    }
    out.push(']');
    out
}

fn push_hex_byte(out: &mut String, b: u8, upper: bool) {
    let table = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    out.push(table[(b >> 4) as usize] as char);
    out.push(table[(b & 0x0F) as usize] as char);
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

fn value_to_rune_go(v: &Option<Value>) -> Option<char> {
    let Some(Value::Number(n)) = v.as_ref() else {
        return None;
    };
    value_number_to_rune_go(n)
}

fn value_number_to_rune_go(n: &Number) -> Option<char> {
    let raw = if let Some(i) = n.as_i64() {
        i as i128
    } else if let Some(u) = n.as_u64() {
        u as i128
    } else {
        return None;
    };
    let code = if (0..=0x10FFFF).contains(&raw) {
        raw as u32
    } else {
        0xFFFD
    };
    Some(char::from_u32(code).unwrap_or('\u{FFFD}'))
}

fn value_to_int_for_width_prec(v: &Option<Value>) -> Option<i64> {
    match v.as_ref() {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        _ => None,
    }
}

fn value_to_u64_for_unicode(v: &Option<Value>) -> Option<u64> {
    match v.as_ref() {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Some(i as u64)
            } else {
                n.as_u64()
            }
        }
        _ => None,
    }
}

fn value_to_byte_slice(v: &Option<Value>) -> Option<Vec<u8>> {
    let Some(value) = v.as_ref() else {
        return None;
    };
    decode_go_bytes_value(value)
}

fn value_as_byte_slice(v: &Value) -> Option<Vec<u8>> {
    decode_go_bytes_value(v)
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedPrintfSpec {
    arg_index: Option<usize>,
    bad_index: bool,
    no_verb: bool,
    sharp: bool,
    zero: bool,
    plus: bool,
    minus: bool,
    space: bool,
    width_from_arg: bool,
    width_arg_index: Option<usize>,
    width: Option<usize>,
    precision_from_arg: bool,
    precision_arg_index: Option<usize>,
    precision: Option<usize>,
}

fn parse_printf_spec_flags(spec: &str) -> ParsedPrintfSpec {
    let mut out = ParsedPrintfSpec::default();
    let bytes = spec.as_bytes();
    let mut i = 0usize;
    let mut after_index = false;

    if i < bytes.len() && bytes[i] as char == '[' {
        match parse_printf_arg_index(spec, i) {
            Some((idx, ni)) => {
                out.arg_index = Some(idx);
                i = ni;
                after_index = true;
            }
            None => out.bad_index = true,
        }
    }

    while i < bytes.len() {
        match bytes[i] as char {
            '#' => out.sharp = true,
            '0' => out.zero = true,
            '+' => out.plus = true,
            '-' => out.minus = true,
            ' ' => out.space = true,
            _ => break,
        }
        i += 1;
    }

    if i < bytes.len() && bytes[i] as char == '*' {
        out.width_from_arg = true;
        i += 1;
        after_index = false;
        if i < bytes.len() && bytes[i] as char == '[' {
            match parse_printf_arg_index(spec, i) {
                Some((idx, ni)) => {
                    out.width_arg_index = Some(idx);
                    i = ni;
                    after_index = true;
                }
                None => out.bad_index = true,
            }
        }
    } else {
        let start = i;
        while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
            i += 1;
        }
        if i > start {
            out.width = spec[start..i].parse::<usize>().ok();
            if out.width.map_or(true, |w| w > GO_PRINTF_NUM_LIMIT) {
                out.no_verb = true;
            }
            if after_index {
                out.bad_index = true;
            }
        }
    }

    if i < bytes.len() && bytes[i] as char == '.' {
        if after_index {
            out.bad_index = true;
        }
        i += 1;
        if i < bytes.len() && bytes[i] as char == '*' {
            out.precision_from_arg = true;
            i += 1;
            if i < bytes.len() && bytes[i] as char == '[' {
                match parse_printf_arg_index(spec, i) {
                    Some((idx, _)) => {
                        out.precision_arg_index = Some(idx);
                    }
                    None => out.bad_index = true,
                }
            }
        } else {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
            if i == start {
                out.precision = Some(0);
            } else {
                out.precision = spec[start..i].parse::<usize>().ok();
                if out.precision.map_or(true, |p| p > GO_PRINTF_NUM_LIMIT) {
                    out.no_verb = true;
                }
            }
        }
    }

    if i < bytes.len() {
        match bytes[i] as char {
            '[' | ']' => out.bad_index = true,
            _ => out.no_verb = true,
        }
    }

    out
}

fn parse_printf_arg_index(spec: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = spec.as_bytes();
    if start >= bytes.len() || bytes[start] as char != '[' {
        return None;
    }
    let mut i = start + 1;
    let digits_start = i;
    while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
        i += 1;
    }
    if i == digits_start || i >= bytes.len() || bytes[i] as char != ']' {
        return None;
    }
    let raw = spec[digits_start..i].parse::<usize>().ok()?;
    if raw == 0 {
        return None;
    }
    Some((raw - 1, i + 1))
}

fn format_missing_arg(verb: char) -> String {
    format!("%!{verb}(MISSING)")
}

fn format_bad_index(verb: char) -> String {
    format!("%!{verb}(BADINDEX)")
}

fn format_extra_args(extra: &[Option<Value>]) -> String {
    let mut out = String::from("%!(EXTRA ");
    for (idx, arg) in extra.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(&format_extra_arg(arg));
    }
    out.push(')');
    out
}

fn format_extra_arg(arg: &Option<Value>) -> String {
    match arg {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(v) => format!("{}={}", printf_type_name(v), format_value_like_go(v)),
    }
}

fn format_value_for_printf(v: &Option<Value>, verb: char, sharp: bool) -> String {
    match (verb, v) {
        (_, None) | (_, Some(Value::Null)) => "<nil>".to_string(),
        ('s', Some(Value::String(s))) => s.clone(),
        ('v', Some(value)) => {
            if sharp {
                format_value_go_syntax(value)
            } else {
                format_value_like_go(value)
            }
        }
        (_, Some(Value::String(s))) => s.clone(),
        (_, Some(value)) => format_value_like_go(value),
    }
}

fn format_value_go_syntax(v: &Value) -> String {
    if let Some(bytes) = value_as_byte_slice(v) {
        let mut out = String::from("[]byte{");
        for (idx, b) in bytes.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("0x{:x}", b));
        }
        out.push('}');
        return out;
    }
    match v {
        Value::Null => "interface {}(nil)".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number_sharp_v_go(n),
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => {
            let slice_ty = infer_slice_type_for_sharp_v(items);
            let mut out = format!("[]{slice_ty}{{");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_go_syntax_item(item, slice_ty));
            }
            out.push('}');
            out
        }
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            let val_ty = infer_map_value_type_for_sharp_v(map.values());
            let mut out = format!("map[string]{val_ty}{{");
            for (idx, k) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{k:?}:"));
                if let Some(v) = map.get(*k) {
                    out.push_str(&format_value_go_syntax_item(v, val_ty));
                } else {
                    out.push_str("interface {}(nil)");
                }
            }
            out.push('}');
            out
        }
    }
}

fn infer_slice_type_for_sharp_v(items: &[Value]) -> &'static str {
    if items.is_empty() {
        return "interface {}";
    }
    if items.iter().all(|item| matches!(item, Value::String(_))) {
        return "string";
    }
    if items.iter().all(|item| matches!(item, Value::Bool(_))) {
        return "bool";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_i64().is_some()))
    {
        return "int";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_u64().is_some()))
    {
        return "uint";
    }
    if items
        .iter()
        .all(|item| matches!(item, Value::Number(n) if n.as_f64().is_some()))
    {
        return "float64";
    }
    "interface {}"
}

fn infer_map_value_type_for_sharp_v<'a>(values: impl Iterator<Item = &'a Value>) -> &'static str {
    let vals: Vec<&Value> = values.collect();
    if vals.is_empty() {
        return "interface {}";
    }
    if vals.iter().all(|v| matches!(v, Value::String(_))) {
        return "string";
    }
    if vals.iter().all(|v| matches!(v, Value::Bool(_))) {
        return "bool";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_i64().is_some()))
    {
        return "int";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_u64().is_some()))
    {
        return "uint";
    }
    if vals
        .iter()
        .all(|v| matches!(v, Value::Number(n) if n.as_f64().is_some()))
    {
        return "float64";
    }
    "interface {}"
}

fn format_value_go_syntax_item(v: &Value, ty: &str) -> String {
    if ty == "interface {}" {
        return format_value_go_syntax(v);
    }
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if ty == "float64" {
                format_number_sharp_v_go(n)
            } else if ty == "uint" {
                if let Some(u) = n.as_u64() {
                    format!("0x{u:x}")
                } else {
                    n.to_string()
                }
            } else {
                n.to_string()
            }
        }
        Value::Null => "nil".to_string(),
        _ => format_value_go_syntax(v),
    }
}

fn format_number_sharp_v_go(n: &Number) -> String {
    if n.as_i64().is_some() {
        return n.to_string();
    }
    if let Some(u) = n.as_u64() {
        return format!("0x{u:x}");
    }
    if let Some(f) = n.as_f64() {
        return format_float_general_go_default(f, false);
    }
    n.to_string()
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
    if value_as_byte_slice(v).is_some() {
        return "[]uint8".to_string();
    }
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => format!("[]{}", infer_slice_type_for_sharp_v(items)),
        Value::Object(map) => {
            let ty = infer_map_value_type_for_sharp_v(map.values());
            format!("map[string]{ty}")
        }
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
    if let Some(bytes) = value_as_byte_slice(v) {
        let mut out = String::from("[");
        for (idx, b) in bytes.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            out.push_str(&b.to_string());
        }
        out.push(']');
        return out;
    }
    match v {
        Value::Null => "<no value>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number_like_go(n),
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

fn format_number_like_go(n: &Number) -> String {
    if n.as_i64().is_some() || n.as_u64().is_some() {
        return n.to_string();
    }
    if let Some(f) = n.as_f64() {
        return format_float_general_go_default(f, false);
    }
    n.to_string()
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
    fn typed_bytes(bytes: &[u8]) -> Value {
        crate::gotemplates::encode_go_bytes_value(bytes)
    }

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

    #[test]
    fn go_printf_plus_q_uses_ascii_escapes() {
        let args = vec![Some(Value::String("日本語".to_string()))];
        let out = go_printf("%+q", &args).expect("must render");
        assert_eq!(out, "\"\\u65e5\\u672c\\u8a9e\"");
    }

    #[test]
    fn go_printf_supports_width_and_left_align_for_q() {
        let args = vec![Some(Value::String("⌘".to_string()))];
        let out = go_printf("%10q", &args).expect("must render");
        assert_eq!(out, "       \"⌘\"");
        let out = go_printf("%-10q", &args).expect("must render");
        assert_eq!(out, "\"⌘\"       ");
        let out = go_printf("%010q", &args).expect("must render");
        assert_eq!(out, "0000000\"⌘\"");
    }

    #[test]
    fn go_printf_formats_q_for_integer_as_rune_literal() {
        let args = vec![Some(Value::Number(Number::from('⌘' as i64)))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "'⌘'");

        let args = vec![Some(Value::Number(Number::from('\n' as i64)))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "'\\n'");

        let args = vec![Some(Value::Number(Number::from(0x0e00i64)))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "'\\u0e00'");

        let args = vec![Some(Value::Number(Number::from(0x10ffffi64)))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "'\\U0010ffff'");

        let args = vec![Some(Value::Number(Number::from(0x11_0000i64)))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "'�'");
    }

    #[test]
    fn go_printf_reports_mismatch_for_q_on_unsupported_type() {
        let args = vec![Some(Value::Bool(true))];
        assert_eq!(
            go_printf("%q", &args).expect("must render"),
            "%!q(bool=true)"
        );
    }

    #[test]
    fn go_printf_reports_typed_names_for_slices_and_maps() {
        let bytes = vec![Some(typed_bytes(&[1, 2]))];
        assert_eq!(go_printf("%T", &bytes).expect("must render"), "[]uint8");

        let map = vec![Some(serde_json::json!({"a":1}))];
        assert_eq!(
            go_printf("%d", &map).expect("must render"),
            "%!d(map[string]int=map[a:1])"
        );

        let n = vec![Some(Value::Number(Number::from(7)))];
        assert_eq!(go_printf("%s", &n).expect("must render"), "%!s(int=7)");
    }

    #[test]
    fn go_printf_applies_rune_precision_for_s() {
        let args = vec![Some(Value::String("абв".to_string()))];
        let out = go_printf("%.2s", &args).expect("must render");
        assert_eq!(out, "аб");
        let out = go_printf("%5.2s", &args).expect("must render");
        assert_eq!(out, "   аб");
        let out = go_printf("%-5.2s", &args).expect("must render");
        assert_eq!(out, "аб   ");
    }

    #[test]
    fn go_printf_formats_strings_for_x_and_x_flags() {
        let args = vec![Some(Value::String("xyz".to_string()))];
        assert_eq!(go_printf("%x", &args).expect("must render"), "78797a");
        assert_eq!(go_printf("%X", &args).expect("must render"), "78797A");
        assert_eq!(go_printf("% x", &args).expect("must render"), "78 79 7a");
        assert_eq!(go_printf("% X", &args).expect("must render"), "78 79 7A");
        assert_eq!(go_printf("%#x", &args).expect("must render"), "0x78797a");
        assert_eq!(go_printf("%#X", &args).expect("must render"), "0X78797A");
        assert_eq!(
            go_printf("%# x", &args).expect("must render"),
            "0x78 0x79 0x7a"
        );
        assert_eq!(
            go_printf("%# X", &args).expect("must render"),
            "0X78 0X79 0X7A"
        );
    }

    #[test]
    fn go_printf_formats_byte_arrays_for_s_q_x() {
        let args = vec![Some(typed_bytes(&[97, 98]))];
        assert_eq!(go_printf("%s", &args).expect("must render"), "ab");
        assert_eq!(go_printf("%q", &args).expect("must render"), "\"ab\"");
        assert_eq!(go_printf("%x", &args).expect("must render"), "6162");
        assert_eq!(go_printf("%b", &args).expect("must render"), "[1100001 1100010]");
        assert_eq!(go_printf("%o", &args).expect("must render"), "[141 142]");
        assert_eq!(go_printf("%c", &args).expect("must render"), "[a b]");
        assert_eq!(go_printf("%U", &args).expect("must render"), "[U+0061 U+0062]");

        let args = vec![Some(typed_bytes(&[255]))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "\"\\xff\"");
    }

    #[test]
    fn go_printf_formats_q_for_non_byte_arrays() {
        let args = vec![Some(Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]))];
        assert_eq!(go_printf("%q", &args).expect("must render"), "[\"a\" \"b\"]");
    }

    #[test]
    fn go_printf_formats_sharp_v_go_syntax_subset() {
        let s = vec![Some(Value::String("foo".to_string()))];
        assert_eq!(go_printf("%#v", &s).expect("must render"), "\"foo\"");

        let n = vec![Some(Value::Number(Number::from(1_000_000)))];
        assert_eq!(go_printf("%#v", &n).expect("must render"), "1000000");

        let f1 = vec![Number::from_f64(1.0).map(Value::Number)];
        assert_eq!(go_printf("%#v", &f1).expect("must render"), "1");

        let f2 = vec![Number::from_f64(1_000_000.0).map(Value::Number)];
        assert_eq!(go_printf("%#v", &f2).expect("must render"), "1e+06");

        let u = vec![Some(Value::Number(Number::from(u64::MAX)))];
        assert_eq!(
            go_printf("%#v", &u).expect("must render"),
            "0xffffffffffffffff"
        );

        let bytes = vec![Some(typed_bytes(&[1, 11, 111]))];
        assert_eq!(
            go_printf("%#v", &bytes).expect("must render"),
            "[]byte{0x1, 0xb, 0x6f}"
        );

        let strs = vec![Some(Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]))];
        assert_eq!(
            go_printf("%#v", &strs).expect("must render"),
            "[]string{\"a\", \"b\"}"
        );

        let map = serde_json::json!({"a":1});
        let obj = vec![Some(map)];
        assert_eq!(
            go_printf("%#v", &obj).expect("must render"),
            "map[string]int{\"a\":1}"
        );

        let ints = vec![Some(Value::Array(vec![
            Value::Number(Number::from(1)),
            Value::Number(Number::from(2)),
        ]))];
        assert_eq!(go_printf("%#v", &ints).expect("must render"), "[]int{1, 2}");

        let map_s = serde_json::json!({"a":"x","b":"y"});
        let obj_s = vec![Some(map_s)];
        assert_eq!(
            go_printf("%#v", &obj_s).expect("must render"),
            "map[string]string{\"a\":\"x\", \"b\":\"y\"}"
        );

        let empty_arr = vec![Some(Value::Array(vec![]))];
        assert_eq!(
            go_printf("%#v", &empty_arr).expect("must render"),
            "[]interface {}{}"
        );

        let empty_map = vec![Some(Value::Object(serde_json::Map::new()))];
        assert_eq!(
            go_printf("%#v", &empty_map).expect("must render"),
            "map[string]interface {}{}"
        );

        let mut map_u = serde_json::Map::new();
        map_u.insert(
            "a".to_string(),
            Value::Number(Number::from(u64::MAX)),
        );
        let obj_u = vec![Some(Value::Object(map_u))];
        assert_eq!(
            go_printf("%#v", &obj_u).expect("must render"),
            "map[string]uint{\"a\":0xffffffffffffffff}"
        );
    }

    #[test]
    fn go_printf_formats_v_float_like_go_g() {
        let v = vec![Number::from_f64(1.0).map(Value::Number)];
        assert_eq!(go_printf("%v", &v).expect("must render"), "1");
    }

    #[test]
    fn go_printf_formats_c_like_go() {
        let args = vec![Some(Value::Number(Number::from('⌘' as i64)))];
        assert_eq!(go_printf("%.0c", &args).expect("must render"), "⌘");
        assert_eq!(go_printf("%3c", &args).expect("must render"), "  ⌘");
        assert_eq!(go_printf("%03c", &args).expect("must render"), "00⌘");
    }

    #[test]
    fn go_printf_handles_missing_extra_and_noverb_markers() {
        assert_eq!(go_printf("%", &[]).expect("must render"), "%!(NOVERB)");
        assert_eq!(go_printf("%d", &[]).expect("must render"), "%!d(MISSING)");
        let args = vec![
            Some(Value::Number(Number::from(1))),
            Some(Value::Number(Number::from(2))),
        ];
        assert_eq!(
            go_printf("%d", &args).expect("must render"),
            "1%!(EXTRA int=2)"
        );
    }

    #[test]
    fn go_printf_handles_star_width_and_precision() {
        let bad_width = vec![
            Some(Value::String("x".to_string())),
            Some(Value::Number(Number::from(7))),
        ];
        assert_eq!(
            go_printf("%*d", &bad_width).expect("must render"),
            "%!(BADWIDTH)7"
        );

        let bad_prec = vec![
            Some(Value::String("x".to_string())),
            Some(Value::Number(Number::from(7))),
        ];
        assert_eq!(
            go_printf("%.*d", &bad_prec).expect("must render"),
            "%!(BADPREC)7"
        );

        let ok = vec![
            Some(Value::Number(Number::from(8))),
            Some(Value::Number(Number::from(2))),
            Number::from_f64(1.2).map(Value::Number),
        ];
        assert_eq!(go_printf("%*.*f", &ok).expect("must render"), "    1.20");
    }

    #[test]
    fn go_printf_handles_too_large_width_precision_like_go() {
        let args = vec![Some(Value::Number(Number::from(42)))];
        assert_eq!(
            go_printf("%2147483648d", &args).expect("must render"),
            "%!(NOVERB)%!(EXTRA int=42)"
        );
        assert_eq!(
            go_printf("%-2147483648d", &args).expect("must render"),
            "%!(NOVERB)%!(EXTRA int=42)"
        );
        assert_eq!(
            go_printf("%.2147483648d", &args).expect("must render"),
            "%!(NOVERB)%!(EXTRA int=42)"
        );

        let bad_width = vec![
            Some(Value::Number(Number::from(10_000_000))),
            Some(Value::Number(Number::from(42))),
        ];
        assert_eq!(
            go_printf("%*d", &bad_width).expect("must render"),
            "%!(BADWIDTH)42"
        );

        let bad_prec = vec![
            Some(Value::Number(Number::from(10_000_000))),
            Some(Value::Number(Number::from(42))),
        ];
        assert_eq!(
            go_printf("%.*d", &bad_prec).expect("must render"),
            "%!(BADPREC)42"
        );

        let huge_prec = vec![
            Some(Value::Number(Number::from(1u64 << 63))),
            Some(Value::Number(Number::from(42))),
        ];
        assert_eq!(
            go_printf("%.*d", &huge_prec).expect("must render"),
            "%!(BADPREC)42"
        );

        let no_verb = vec![Some(Value::Number(Number::from(4)))];
        assert_eq!(go_printf("%*", &no_verb).expect("must render"), "%!(NOVERB)");
    }

    #[test]
    fn go_printf_handles_argument_indexes_like_go() {
        let args = vec![
            Some(Value::Number(Number::from(1))),
            Some(Value::Number(Number::from(2))),
        ];
        assert_eq!(
            go_printf("%[d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%[]d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%[-3]d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%[99]d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%[3]", &args).expect("must render"),
            "%!(NOVERB)"
        );

        assert_eq!(go_printf("%[2]d %[1]d", &args).expect("must render"), "2 1");

        let args = vec![
            Some(Value::Number(Number::from(1))),
            Some(Value::Number(Number::from(2))),
            Some(Value::Number(Number::from(3))),
        ];
        assert_eq!(
            go_printf("%[5]d %[2]d %d", &args).expect("must render"),
            "%!d(BADINDEX) 2 3"
        );

        let args = vec![
            Some(Value::Number(Number::from(1))),
            Some(Value::Number(Number::from(2))),
        ];
        assert_eq!(
            go_printf("%d %[3]d %d", &args).expect("must render"),
            "1 %!d(BADINDEX) 2"
        );

        let args = vec![
            Some(Value::Number(Number::from(1))),
            Some(Value::Number(Number::from(2))),
        ];
        assert_eq!(
            go_printf("%[2]2d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%[2].2d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%3.[2]d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%.[2]d", &args).expect("must render"),
            "%!d(BADINDEX)"
        );
        assert_eq!(
            go_printf("%.[]", &[]).expect("must render"),
            "%!](BADINDEX)"
        );
    }

    #[test]
    fn go_printf_formats_integers_with_precision_and_sharp() {
        let zero = vec![Some(Value::Number(Number::from(0)))];
        assert_eq!(go_printf("%.d", &zero).expect("must render"), "");
        assert_eq!(go_printf("%6.0d", &zero).expect("must render"), "      ");
        assert_eq!(go_printf("%06.0d", &zero).expect("must render"), "      ");

        let n = vec![Some(Value::Number(Number::from(-1234)))];
        assert_eq!(
            go_printf("%020.8d", &n).expect("must render"),
            "           -00001234"
        );

        let x = vec![Some(Value::Number(Number::from(0x1234abc)))];
        assert_eq!(
            go_printf("%-#20.8x", &x).expect("must render"),
            "0x01234abc          "
        );

        let o = vec![Some(Value::Number(Number::from(-668)))];
        assert_eq!(go_printf("%#o", &o).expect("must render"), "-01234");
    }

    #[test]
    fn go_printf_formats_unicode_verb_u_like_go() {
        let zero = vec![Some(Value::Number(Number::from(0)))];
        assert_eq!(go_printf("%U", &zero).expect("must render"), "U+0000");

        let minus_one = vec![Some(Value::Number(Number::from(-1)))];
        assert_eq!(
            go_printf("%U", &minus_one).expect("must render"),
            "U+FFFFFFFFFFFFFFFF"
        );

        let smile = vec![Some(Value::Number(Number::from('☺' as i64)))];
        assert_eq!(go_printf("%#U", &smile).expect("must render"), "U+263A '☺'");

        let cmd = vec![Some(Value::Number(Number::from('⌘' as i64)))];
        assert_eq!(
            go_printf("%#14.6U", &cmd).expect("must render"),
            "  U+002318 '⌘'"
        );
    }
}
