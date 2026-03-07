pub(super) fn push_with_width(
    out: &mut String,
    rendered: &str,
    width: Option<usize>,
    zero_pad: bool,
    left_align: bool,
) {
    let Some(w) = width else {
        out.push_str(rendered);
        return;
    };

    let char_count = rendered.chars().count();
    if char_count >= w {
        out.push_str(rendered);
        return;
    }

    let pad_len = w - char_count;
    let pad_ch = if zero_pad { '0' } else { ' ' };

    if left_align {
        out.push_str(rendered);
        for _ in 0..pad_len {
            out.push(' ');
        }
        return;
    }

    if zero_pad && rendered.starts_with(['+', '-', ' ']) {
        let mut chars = rendered.chars();
        if let Some(sign) = chars.next() {
            out.push(sign);
        }
        for _ in 0..pad_len {
            out.push('0');
        }
        out.push_str(chars.as_str());
        return;
    }
    for _ in 0..pad_len {
        out.push(pad_ch);
    }
    out.push_str(rendered);
}

pub(super) fn truncate_runes(s: &str, max_runes: usize) -> String {
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
