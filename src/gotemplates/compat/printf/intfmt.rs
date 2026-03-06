#[derive(Debug, Clone, Copy)]
pub(super) struct GoInteger {
    pub(super) raw: u64,
    pub(super) signed: bool,
}

pub(super) fn format_integer_go(
    arg: GoInteger,
    verb: char,
    plus: bool,
    space: bool,
    sharp: bool,
    zero: bool,
    minus: bool,
    width: Option<usize>,
    precision: Option<usize>,
) -> String {
    let negative = arg.signed && (arg.raw as i64) < 0;
    let u = if negative {
        arg.raw.wrapping_neg()
    } else {
        arg.raw
    };
    let mut body = match verb {
        'd' => u.to_string(),
        'x' => format!("{u:x}"),
        'X' => format!("{u:X}"),
        'o' => format!("{u:o}"),
        'O' => format!("{u:o}"),
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
    if verb == 'O' {
        prefix.push_str("0o");
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

pub(super) fn format_unicode_verb_go(u: u64, sharp: bool, precision: Option<usize>) -> String {
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

pub(super) fn format_bytes_hex_go(
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

pub(super) fn format_byte_slice_list_go(
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
                let n = GoInteger {
                    raw: u64::from(*b),
                    signed: false,
                };
                format_integer_go(n, 'o', false, false, sharp, false, false, None, precision)
            }
            'O' => {
                let n = GoInteger {
                    raw: u64::from(*b),
                    signed: false,
                };
                format_integer_go(n, 'O', false, false, sharp, false, false, None, precision)
            }
            'b' => {
                let n = GoInteger {
                    raw: u64::from(*b),
                    signed: false,
                };
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

pub(super) fn apply_printf_sign_flags(
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

fn push_hex_byte(out: &mut String, b: u8, upper: bool) {
    let table = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    out.push(table[(b >> 4) as usize] as char);
    out.push(table[(b & 0x0F) as usize] as char);
}
