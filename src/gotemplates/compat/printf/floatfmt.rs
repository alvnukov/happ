pub(super) fn format_float_exp_go(n: f64, precision: usize, upper: bool) -> String {
    let raw = if upper {
        format!("{:.*E}", precision, n)
    } else {
        format!("{:.*e}", precision, n)
    };
    normalize_scientific_exponent(&raw, upper)
}

pub(super) fn format_float_general_go(n: f64, precision: usize, upper: bool) -> String {
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

pub(super) fn format_float_general_go_default(n: f64, upper: bool) -> String {
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

pub(super) fn format_float_with_verb_go(
    n: f64,
    verb: char,
    precision: Option<usize>,
    sharp: bool,
) -> (String, bool) {
    if n.is_nan() {
        return ("NaN".to_string(), true);
    }
    if n.is_infinite() {
        return (
            if n.is_sign_negative() {
                "-Inf".to_string()
            } else {
                "Inf".to_string()
            },
            true,
        );
    }

    let mut rendered = match verb {
        'f' | 'F' => format!("{:.*}", precision.unwrap_or(6), n),
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

    if sharp {
        rendered = apply_float_sharp_flag_go(&rendered, verb, precision);
    }

    (rendered, false)
}

fn apply_float_sharp_flag_go(rendered: &str, verb: char, precision: Option<usize>) -> String {
    if rendered.is_empty() {
        return String::new();
    }
    if matches!(verb, 'x' | 'X') && precision.is_some_and(|p| p > 0) {
        // %#[N]x already has exact fractional width from format_float_hex_go.
        return rendered.to_string();
    }

    let mut digits: isize = match verb {
        'v' | 'g' | 'G' | 'x' => precision.unwrap_or(6) as isize,
        _ => 0,
    };

    let mut body = rendered.to_string();
    let mut tail = String::new();
    let tail_start = body.find(['e', 'E']).or_else(|| body.find(['p', 'P']));
    if let Some(idx) = tail_start {
        tail.push_str(&body[idx..]);
        body.truncate(idx);
    }

    if !body.contains('.') {
        if body == "0" || body == "-0" || body == "+0" {
            digits -= 1;
        }
        body.push('.');
    }

    if digits > 0 {
        let mut saw_nonzero = false;
        for ch in body.chars() {
            if matches!(ch, '.' | '-' | '+') {
                continue;
            }
            if ch != '0' {
                saw_nonzero = true;
            }
            if saw_nonzero {
                digits -= 1;
            }
        }
    }

    while digits > 0 {
        body.push('0');
        digits -= 1;
    }

    body.push_str(&tail);
    body
}

pub(super) fn format_float_binary_go(n: f64) -> String {
    if n == 0.0 {
        return "0p-1074".to_string();
    }

    let bits = n.to_bits();
    let negative = (bits >> 63) != 0;
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);

    let (mantissa, exp) = if exp_bits == 0 {
        (frac, -1074)
    } else {
        ((1u64 << 52) | frac, exp_bits - 1023 - 52)
    };

    if negative {
        format!("-{mantissa}p{exp:+}")
    } else {
        format!("{mantissa}p{exp:+}")
    }
}

pub(super) fn format_float_hex_go(
    n: f64,
    upper: bool,
    precision: Option<usize>,
    sharp: bool,
) -> String {
    if n == 0.0 {
        let mut frac = match precision {
            Some(p) => "0".repeat(p),
            None => String::new(),
        };
        if frac.is_empty() {
            let mut out = if upper {
                "0X0P+00".to_string()
            } else {
                "0x0p+00".to_string()
            };
            if sharp {
                out = apply_float_sharp_flag_go(&out, if upper { 'X' } else { 'x' }, precision);
            }
            return if n.is_sign_negative() {
                format!("-{out}")
            } else {
                out
            };
        }
        if upper {
            frac.make_ascii_uppercase();
        }
        let mut out = if upper {
            format!("0X0.{frac}P+00")
        } else {
            format!("0x0.{frac}p+00")
        };
        if sharp {
            out = apply_float_sharp_flag_go(&out, if upper { 'X' } else { 'x' }, precision);
        }
        return if n.is_sign_negative() {
            format!("-{out}")
        } else {
            out
        };
    }

    let bits = n.abs().to_bits();
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);

    let (mut sig, mut exp2) = if exp_bits == 0 {
        (frac_bits, -1022)
    } else {
        ((1u64 << 52) | frac_bits, exp_bits - 1023)
    };
    while sig != 0 && sig < (1u64 << 52) {
        sig <<= 1;
        exp2 -= 1;
    }

    let mut frac_digits = String::new();
    if let Some(p) = precision {
        if p == 0 {
            let drop = 52u32;
            let mut retained = sig >> drop;
            let rem = sig & ((1u64 << drop) - 1);
            let half = 1u64 << (drop - 1);
            if rem > half || (rem == half && (retained & 1) == 1) {
                retained = retained.saturating_add(1);
            }
            if retained >= 2 {
                exp2 += 1;
            }
        } else if p >= 13 {
            frac_digits = format!("{frac_bits:013x}");
            frac_digits.push_str(&"0".repeat(p - 13));
        } else {
            let keep_bits = (p * 4) as u32;
            let drop = 52u32 - keep_bits;
            let mut retained = sig >> drop;
            let rem = sig & ((1u64 << drop) - 1);
            let half = 1u64 << (drop - 1);
            if rem > half || (rem == half && (retained & 1) == 1) {
                retained = retained.saturating_add(1);
            }
            if retained >= (1u64 << (keep_bits + 1)) {
                retained >>= 1;
                exp2 += 1;
            }
            let mask = (1u64 << keep_bits) - 1;
            let kept = retained & mask;
            frac_digits = format!("{kept:0width$x}", width = p);
        }
    } else {
        frac_digits = format!("{:013x}", sig - (1u64 << 52));
        while frac_digits.ends_with('0') {
            frac_digits.pop();
        }
    }

    if upper {
        frac_digits.make_ascii_uppercase();
    }
    let exp_sign = if exp2 >= 0 { '+' } else { '-' };
    let exp_abs = exp2.unsigned_abs();
    let exp_text = if exp_abs < 10 {
        format!("{exp_sign}0{exp_abs}")
    } else {
        format!("{exp_sign}{exp_abs}")
    };

    let mut out = if frac_digits.is_empty() {
        if upper {
            format!("0X1P{exp_text}")
        } else {
            format!("0x1p{exp_text}")
        }
    } else if upper {
        format!("0X1.{frac_digits}P{exp_text}")
    } else {
        format!("0x1.{frac_digits}p{exp_text}")
    };
    if sharp {
        out = apply_float_sharp_flag_go(&out, if upper { 'X' } else { 'x' }, precision);
    }
    if n.is_sign_negative() {
        format!("-{out}")
    } else {
        out
    }
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
