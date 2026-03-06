use super::GO_PRINTF_NUM_LIMIT;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ParsedPrintfSpec {
    pub(super) arg_index: Option<usize>,
    pub(super) bad_index: bool,
    pub(super) no_verb: bool,
    pub(super) sharp: bool,
    pub(super) zero: bool,
    pub(super) plus: bool,
    pub(super) minus: bool,
    pub(super) space: bool,
    pub(super) width_from_arg: bool,
    pub(super) width_arg_index: Option<usize>,
    pub(super) width: Option<usize>,
    pub(super) precision_from_arg: bool,
    pub(super) precision_arg_index: Option<usize>,
    pub(super) precision: Option<usize>,
}

pub(super) fn scan_printf_spec_end(format: &str, start: usize) -> usize {
    let bytes = format.as_bytes();
    let mut i = start;
    let mut after_index = false;

    while i < bytes.len() {
        match bytes[i] as char {
            '#' | '0' | '+' | '-' | ' ' => i += 1,
            _ => break,
        }
    }

    if i < bytes.len() && bytes[i] as char == '[' {
        let (_, ni, ok) = parse_printf_arg_index(format, i);
        i = ni;
        after_index = ok;
    }

    if i < bytes.len() && bytes[i] as char == '*' {
        i += 1;
        after_index = false;
    } else {
        while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
            i += 1;
        }
    }

    if i < bytes.len() && bytes[i] as char == '.' {
        i += 1;
        if i < bytes.len() && bytes[i] as char == '[' {
            let (_, ni, ok) = parse_printf_arg_index(format, i);
            i = ni;
            after_index = ok;
        }
        if i < bytes.len() && bytes[i] as char == '*' {
            i += 1;
            after_index = false;
        } else {
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
        }
    }

    if !after_index && i < bytes.len() && bytes[i] as char == '[' {
        let (_, ni, _) = parse_printf_arg_index(format, i);
        i = ni;
    }

    i
}

pub(super) fn parse_printf_spec_flags(spec: &str) -> ParsedPrintfSpec {
    let mut out = ParsedPrintfSpec::default();
    let bytes = spec.as_bytes();
    let mut i = 0usize;
    let mut after_index = false;
    let mut pending_index: Option<usize> = None;

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

    parse_printf_arg_number(
        spec,
        &mut i,
        &mut pending_index,
        &mut after_index,
        &mut out.bad_index,
    );

    if i < bytes.len() && bytes[i] as char == '*' {
        out.width_from_arg = true;
        out.width_arg_index = pending_index.take();
        i += 1;
        after_index = false;
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

        parse_printf_arg_number(
            spec,
            &mut i,
            &mut pending_index,
            &mut after_index,
            &mut out.bad_index,
        );

        if i < bytes.len() && bytes[i] as char == '*' {
            out.precision_from_arg = true;
            out.precision_arg_index = pending_index.take();
            i += 1;
            after_index = false;
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

    if !after_index {
        parse_printf_arg_number(
            spec,
            &mut i,
            &mut pending_index,
            &mut after_index,
            &mut out.bad_index,
        );
    }
    if after_index {
        out.arg_index = pending_index;
    }

    if i < bytes.len() {
        match bytes[i] as char {
            '[' | ']' => out.bad_index = true,
            _ => out.no_verb = true,
        }
    }

    out
}

fn parse_printf_arg_number(
    spec: &str,
    i: &mut usize,
    pending_index: &mut Option<usize>,
    after_index: &mut bool,
    bad_index: &mut bool,
) {
    if *i >= spec.len() || spec.as_bytes()[*i] as char != '[' {
        return;
    }
    let (idx, ni, ok) = parse_printf_arg_index(spec, *i);
    *i = ni;
    if ok {
        *pending_index = idx;
        *after_index = true;
    } else {
        *bad_index = true;
        *pending_index = None;
        *after_index = false;
    }
}

fn parse_printf_arg_index(spec: &str, start: usize) -> (Option<usize>, usize, bool) {
    let bytes = spec.as_bytes();
    if start >= bytes.len() || bytes[start] as char != '[' {
        return (None, start, false);
    }
    if bytes.len().saturating_sub(start) < 3 {
        return (None, start.saturating_add(1), false);
    }

    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] as char == ']' {
            let digits = &spec[start + 1..i];
            let raw = digits.parse::<usize>().ok();
            if digits.is_empty() || raw.is_none() || raw == Some(0) {
                return (None, i + 1, false);
            }
            return (Some(raw.unwrap_or_default() - 1), i + 1, true);
        }
        i += 1;
    }
    (None, start + 1, false)
}

#[cfg(test)]
mod tests {
    use super::{parse_printf_spec_flags, scan_printf_spec_end};

    #[test]
    fn parse_reordered_width_precision_and_value_indexes() {
        let parsed = parse_printf_spec_flags("[3]*.[2]*[1]");
        assert!(!parsed.bad_index);
        assert!(!parsed.no_verb);
        assert!(parsed.width_from_arg);
        assert_eq!(parsed.width_arg_index, Some(2));
        assert!(parsed.precision_from_arg);
        assert_eq!(parsed.precision_arg_index, Some(1));
        assert_eq!(parsed.arg_index, Some(0));
    }

    #[test]
    fn parse_precision_index_without_star_targets_value_arg() {
        let parsed = parse_printf_spec_flags(".[2]");
        assert!(!parsed.bad_index);
        assert_eq!(parsed.precision, Some(0));
        assert_eq!(parsed.arg_index, Some(1));
    }

    #[test]
    fn parse_reports_bad_index_for_width_after_explicit_index() {
        let parsed = parse_printf_spec_flags("[1]2");
        assert!(parsed.bad_index);
    }

    #[test]
    fn parse_sets_no_verb_for_too_large_width() {
        let parsed = parse_printf_spec_flags("2147483648");
        assert!(parsed.no_verb);
    }

    #[test]
    fn scan_stops_before_non_format_rune_like_go() {
        let fmt = "%.-3d";
        let end = scan_printf_spec_end(fmt, 1);
        assert_eq!(&fmt[1..end], ".");
        assert_eq!(fmt.as_bytes()[end] as char, '-');
    }
}
