use serde_json::Value;
// Go parity reference:
// - stdlib fmt package (print.go, format.go)
// This module ports formatting behavior used by template `printf`.
mod argconv;
mod diag;
mod floatfmt;
mod intfmt;
mod layout;
mod q;
mod spec;
mod valuefmt;
use argconv::{
    value_as_byte_slice, value_as_string_bytes, value_number_to_rune_go, value_to_bool,
    value_to_byte_slice, value_to_f64, value_to_int_for_width_prec, value_to_integer_go,
    value_to_rune_go, value_to_string_bytes, value_to_u64_for_unicode,
};
use diag::{format_bad_index, format_extra_args, format_missing_arg, format_printf_mismatch};
use floatfmt::{format_float_binary_go, format_float_hex_go, format_float_with_verb_go};
use intfmt::{
    apply_printf_sign_flags, format_byte_slice_list_go, format_bytes_hex_go, format_integer_go,
    format_unicode_verb_go,
};
use layout::{push_with_width, truncate_runes};
use q::format_q_verb_go;
use spec::{parse_printf_spec_flags, scan_printf_spec_end};
use valuefmt::{format_type_for_printf, format_value_for_printf, format_value_like_go};

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
    floatfmt::format_float_exp_go(n, precision, upper)
}

pub fn format_float_general_go(n: f64, precision: usize, upper: bool) -> String {
    floatfmt::format_float_general_go(n, precision, upper)
}

pub fn go_printf(fmt: &str, args: &[Option<Value>]) -> Result<String, String> {
    let mut out = String::with_capacity(fmt.len() + 8);
    let mut i = 0usize;
    let mut argi = 0usize;
    let mut reordered = false;
    let bytes = fmt.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'%' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'%' {
                i += 1;
            }
            out.push_str(&fmt[start..i]);
            continue;
        }
        if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
            out.push('%');
            i += 2;
            continue;
        }

        let spec_start = i;
        i += 1;
        i = scan_printf_spec_end(fmt, i);
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
            if parsed_tail.width_from_arg
                && parsed_tail.width_arg_index.is_none()
                && argi < args.len()
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
        // Go parity (fmt): arg reordering, star width/precision consumption, and
        // BADWIDTH/BADPREC/BADINDEX markers must follow the same state machine.
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
        let mut width = spec.width;
        let mut precision = spec.precision;
        let mut left_align = spec.minus;
        let mut zero_pad = spec.zero;
        let plus_flag = spec.plus;
        let space_flag = !plus_flag && spec.space;
        let alt_flag = spec.sharp;

        if spec.width_from_arg {
            let mut width_idx = argi;
            if let Some(idx) = spec.width_arg_index {
                reordered = true;
                if idx < args.len() {
                    width_idx = idx;
                } else {
                    good_arg_num = false;
                }
            }
            match args.get(width_idx) {
                Some(v) => match value_to_int_for_width_prec(v) {
                    Some(n) => {
                        argi = width_idx.saturating_add(1);
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
                    None => {
                        argi = width_idx.saturating_add(1);
                        out.push_str("%!(BADWIDTH)");
                    }
                },
                None => {
                    out.push_str("%!(BADWIDTH)");
                }
            }
        }

        if spec.precision_from_arg {
            let mut prec_idx = argi;
            if let Some(idx) = spec.precision_arg_index {
                reordered = true;
                if idx < args.len() {
                    prec_idx = idx;
                } else {
                    good_arg_num = false;
                }
            }
            match args.get(prec_idx) {
                Some(v) => match value_to_int_for_width_prec(v) {
                    Some(n) if n >= 0 => {
                        argi = prec_idx.saturating_add(1);
                        let p = usize::try_from(n).ok();
                        if p.map_or(true, |v| v > GO_PRINTF_NUM_LIMIT) {
                            out.push_str("%!(BADPREC)");
                        } else {
                            precision = p;
                        }
                    }
                    Some(_) => {
                        argi = prec_idx.saturating_add(1);
                        precision = None;
                        out.push_str("%!(BADPREC)");
                    }
                    None => {
                        argi = prec_idx.saturating_add(1);
                        out.push_str("%!(BADPREC)");
                    }
                },
                None => {
                    out.push_str("%!(BADPREC)");
                }
            }
        }

        if let Some(idx) = spec.arg_index {
            reordered = true;
            if idx < args.len() {
                argi = idx;
            } else {
                good_arg_num = false;
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
                } else if let Some(bytes) = value_to_string_bytes(arg) {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
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
                let Some(rendered) = format_q_verb_go(arg, plus_flag, alt_flag, precision) else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
                    continue;
                };
                if alt_flag && matches!(arg, Some(Value::String(_))) {
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                    continue;
                }
                push_with_width(&mut out, &rendered, width, zero_pad, left_align);
            }
            'd' => {
                if let Some(n) = value_to_integer_go(arg) {
                    let rendered = format_integer_go(
                        n, verb, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    );
                    push_with_width(&mut out, &rendered, width, false, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = format_byte_slice_list_go(&bytes, 'd', alt_flag, precision);
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
                }
            }
            'x' | 'X' | 'o' | 'O' | 'b' => {
                if let Some(n) = value_to_integer_go(arg) {
                    let rendered = format_integer_go(
                        n, verb, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    );
                    push_with_width(&mut out, &rendered, width, false, left_align);
                } else if matches!(arg, Some(Value::String(_))) && matches!(verb, 'x' | 'X') {
                    let Some(Value::String(s)) = arg.as_ref() else {
                        unreachable!();
                    };
                    let rendered = format_bytes_hex_go(
                        s.as_bytes(),
                        verb == 'X',
                        alt_flag,
                        space_flag,
                        precision,
                    );
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else if let Some(bytes) = value_to_byte_slice(arg) {
                    let rendered = if matches!(verb, 'x' | 'X') {
                        format_bytes_hex_go(&bytes, verb == 'X', alt_flag, space_flag, precision)
                    } else {
                        format_byte_slice_list_go(&bytes, verb, alt_flag, precision)
                    };
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else if matches!(verb, 'x' | 'X') {
                    if let Some(bytes) = value_to_string_bytes(arg) {
                        let rendered = format_bytes_hex_go(
                            &bytes,
                            verb == 'X',
                            alt_flag,
                            space_flag,
                            precision,
                        );
                        push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                    } else if let Some(n) = value_to_f64(arg) {
                        let special_float = n.is_infinite() || n.is_nan();
                        let rendered = if n.is_nan() {
                            "NaN".to_string()
                        } else if n.is_infinite() {
                            if n.is_sign_negative() {
                                "-Inf".to_string()
                            } else {
                                "Inf".to_string()
                            }
                        } else {
                            format_float_hex_go(n, verb == 'X', precision, alt_flag)
                        };
                        let rendered = apply_printf_sign_flags(
                            rendered,
                            !n.is_sign_negative(),
                            plus_flag,
                            space_flag,
                        );
                        let pad_zeros = if special_float { false } else { zero_pad };
                        push_with_width(&mut out, &rendered, width, pad_zeros, left_align);
                    } else {
                        out.push_str(&format_printf_mismatch_with_state(
                            verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align,
                            width, precision,
                        ));
                    }
                } else if verb == 'b' {
                    if let Some(n) = value_to_f64(arg) {
                        let special_float = n.is_infinite() || n.is_nan();
                        let rendered = if n.is_nan() {
                            "NaN".to_string()
                        } else if n.is_infinite() {
                            if n.is_sign_negative() {
                                "-Inf".to_string()
                            } else {
                                "Inf".to_string()
                            }
                        } else {
                            format_float_binary_go(n)
                        };
                        let rendered = apply_printf_sign_flags(
                            rendered,
                            !n.is_sign_negative(),
                            plus_flag,
                            space_flag,
                        );
                        let pad_zeros = if special_float { false } else { zero_pad };
                        push_with_width(&mut out, &rendered, width, pad_zeros, left_align);
                    } else {
                        out.push_str(&format_printf_mismatch_with_state(
                            verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align,
                            width, precision,
                        ));
                    }
                } else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
                }
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                if let Some(n) = value_to_f64(arg) {
                    let (rendered, special_float) =
                        format_float_with_verb_go(n, verb, precision, alt_flag);
                    let rendered = apply_printf_sign_flags(
                        rendered,
                        !n.is_sign_negative(),
                        plus_flag,
                        space_flag,
                    );
                    let pad_zeros = if special_float { false } else { zero_pad };
                    push_with_width(&mut out, &rendered, width, pad_zeros, left_align);
                } else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
                }
            }
            't' => {
                if let Some(b) = value_to_bool(arg) {
                    let rendered = b.to_string();
                    push_with_width(&mut out, &rendered, width, zero_pad, left_align);
                } else {
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
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
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
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
                    out.push_str(&format_printf_mismatch_with_state(
                        verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width,
                        precision,
                    ));
                }
            }
            _ => out.push_str(&format_printf_mismatch_with_state(
                verb, arg, plus_flag, space_flag, alt_flag, zero_pad, left_align, width, precision,
            )),
        }
    }

    if !reordered && argi < args.len() {
        out.push_str(&format_extra_args(&args[argi..]));
    }
    Ok(out)
}

fn format_printf_mismatch_with_state(
    verb: char,
    arg: &Option<Value>,
    plus_flag: bool,
    space_flag: bool,
    alt_flag: bool,
    zero_pad: bool,
    left_align: bool,
    width: Option<usize>,
    precision: Option<usize>,
) -> String {
    if matches!(arg, None | Some(Value::Null)) {
        return format!("%!{verb}(<nil>)");
    }
    let type_name = format_type_for_printf(arg);
    let mut rendered = format_mismatch_value_as_v(arg, plus_flag, space_flag, alt_flag, precision);
    if width.is_some() {
        let mut padded = String::new();
        push_with_width(&mut padded, &rendered, width, zero_pad, left_align);
        rendered = padded;
    }
    format!("%!{verb}({type_name}={rendered})")
}

fn format_mismatch_value_as_v(
    arg: &Option<Value>,
    plus_flag: bool,
    space_flag: bool,
    alt_flag: bool,
    precision: Option<usize>,
) -> String {
    if let Some(n) = value_to_integer_go(arg) {
        return format_integer_go(
            n, 'd', plus_flag, space_flag, alt_flag, false, false, None, precision,
        );
    }
    if let Some(n) = value_to_f64(arg) {
        let (rendered, _special_float) = format_float_with_verb_go(n, 'g', precision, alt_flag);
        return apply_printf_sign_flags(rendered, !n.is_sign_negative(), plus_flag, space_flag);
    }
    if let Some(Value::String(s)) = arg.as_ref() {
        if let Some(p) = precision {
            return truncate_runes(s, p);
        }
        return s.clone();
    }
    if let Some(bytes) = value_to_string_bytes(arg) {
        let mut rendered = String::from_utf8_lossy(&bytes).into_owned();
        if let Some(p) = precision {
            rendered = truncate_runes(&rendered, p);
        }
        return rendered;
    }
    if let Some(bytes) = value_to_byte_slice(arg) {
        let value = Value::Array(
            bytes
                .iter()
                .map(|b| Value::Number(serde_json::Number::from(*b)))
                .collect(),
        );
        return format_value_like_go(&value);
    }
    match arg.as_ref() {
        Some(v) => format_value_like_go(v),
        None => "<nil>".to_string(),
    }
}
