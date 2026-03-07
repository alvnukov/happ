use super::{GoTemplateScanError, Tok, TokKind};
use crate::gotemplates::go_compat::ident::{
    is_identifier_continue_char, is_identifier_start_char,
};
// Go parity reference: stdlib text/template/parse/lex.go.

pub(super) fn lex_action_inner(src: &str, abs_base: usize) -> Result<Vec<Tok>, GoTemplateScanError> {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(src.len() / 2 + 2);
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        if is_space(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_space(bytes[i]) {
                i += 1;
            }
            out.push(Tok {
                kind: TokKind::Space,
                start,
                end: i,
            });
            continue;
        }

        match b {
            b'=' => {
                out.push(tok(TokKind::Assign, i, i + 1));
                i += 1;
            }
            b':' => {
                if i + 1 >= bytes.len() || bytes[i + 1] != b'=' {
                    return Err(GoTemplateScanError {
                        code: "expected_declare_assign",
                        message: "expected :=".to_string(),
                        offset: abs_base + i,
                    });
                }
                out.push(tok(TokKind::Declare, i, i + 2));
                i += 2;
            }
            b'|' => {
                out.push(tok(TokKind::Pipe, i, i + 1));
                i += 1;
            }
            b'(' => {
                out.push(tok(TokKind::LeftParen, i, i + 1));
                i += 1;
            }
            b')' => {
                out.push(tok(TokKind::RightParen, i, i + 1));
                i += 1;
            }
            b',' => {
                out.push(tok(TokKind::Comma, i, i + 1));
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                loop {
                    if i >= bytes.len() {
                        return Err(GoTemplateScanError {
                            code: "unterminated_quoted_string",
                            message: "unterminated quoted string".to_string(),
                            offset: abs_base + start,
                        });
                    }
                    match bytes[i] {
                        b'\\' => {
                            i += 1;
                            if i < bytes.len() {
                                i += 1;
                            }
                        }
                        b'\n' => {
                            return Err(GoTemplateScanError {
                                code: "unterminated_quoted_string",
                                message: "unterminated quoted string".to_string(),
                                offset: abs_base + start,
                            });
                        }
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                out.push(tok(TokKind::String, start, i));
            }
            b'`' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(GoTemplateScanError {
                        code: "unterminated_raw_quoted_string",
                        message: "unterminated raw quoted string".to_string(),
                        offset: abs_base + start,
                    });
                }
                i += 1;
                out.push(tok(TokKind::RawString, start, i));
            }
            b'$' => {
                let (kind, end) = lex_field_or_variable(bytes, i, true, abs_base)?;
                out.push(tok(kind, i, end));
                i = end;
            }
            b'\'' => {
                let start = i;
                i += 1;
                loop {
                    if i >= bytes.len() {
                        return Err(GoTemplateScanError {
                            code: "unterminated_character_constant",
                            message: "unterminated character constant".to_string(),
                            offset: abs_base + start,
                        });
                    }
                    match bytes[i] {
                        b'\\' => {
                            i += 1;
                            if i < bytes.len() {
                                i += 1;
                            }
                        }
                        b'\n' => {
                            return Err(GoTemplateScanError {
                                code: "unterminated_character_constant",
                                message: "unterminated character constant".to_string(),
                                offset: abs_base + start,
                            });
                        }
                        b'\'' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                out.push(tok(TokKind::CharConst, start, i));
            }
            b'.' => {
                if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                    let end = lex_number(bytes, i, abs_base)?;
                    out.push(tok(TokKind::Number, i, end));
                    i = end;
                } else {
                    let (kind, end) = lex_field_or_variable(bytes, i, false, abs_base)?;
                    out.push(tok(kind, i, end));
                    i = end;
                }
            }
            b'+' | b'-' | b'0'..=b'9' => {
                let end = lex_number(bytes, i, abs_base)?;
                out.push(tok(TokKind::Number, i, end));
                i = end;
            }
            _ if is_identifier_start_at(bytes, i) => {
                let start = i;
                i = consume_identifier(bytes, i, false).ok_or_else(|| GoTemplateScanError {
                    code: "bad_character",
                    message: "bad character in action".to_string(),
                    offset: abs_base + start,
                })?;
                if !at_terminator(bytes, i) {
                    return Err(GoTemplateScanError {
                        code: "bad_character",
                        message: "bad character in action".to_string(),
                        offset: abs_base + i,
                    });
                }
                let word = &src[start..i];
                let kind = classify_word(word);
                out.push(tok(kind, start, i));
            }
            _ if b.is_ascii_graphic() || b == b' ' => {
                out.push(tok(TokKind::Char, i, i + 1));
                i += 1;
            }
            _ => {
                return Err(GoTemplateScanError {
                    code: "unrecognized_character_in_action",
                    message: "unrecognized character in action".to_string(),
                    offset: abs_base + i,
                });
            }
        }
    }

    out.push(Tok {
        kind: TokKind::Eof,
        start: src.len(),
        end: src.len(),
    });
    Ok(out)
}

fn tok(kind: TokKind, start: usize, end: usize) -> Tok {
    Tok { kind, start, end }
}

fn classify_word(word: &str) -> TokKind {
    match word {
        "true" | "false" => TokKind::Bool,
        "nil" => TokKind::Nil,
        "block" => TokKind::KwBlock,
        "break" => TokKind::KwBreak,
        "continue" => TokKind::KwContinue,
        "define" => TokKind::KwDefine,
        "else" => TokKind::KwElse,
        "end" => TokKind::KwEnd,
        "if" => TokKind::KwIf,
        "range" => TokKind::KwRange,
        "template" => TokKind::KwTemplate,
        "with" => TokKind::KwWith,
        _ => TokKind::Identifier,
    }
}

fn lex_field_or_variable(
    bytes: &[u8],
    start: usize,
    variable: bool,
    abs_base: usize,
) -> Result<(TokKind, usize), GoTemplateScanError> {
    let mut i = start + 1;
    if at_terminator(bytes, i) {
        if variable {
            return Ok((TokKind::Variable, i));
        }
        return Ok((TokKind::Dot, i));
    }
    i = consume_identifier(bytes, i, true).ok_or_else(|| GoTemplateScanError {
        code: "bad_character",
        message: "bad character in action".to_string(),
        offset: abs_base + i,
    })?;
    if !at_terminator(bytes, i) {
        return Err(GoTemplateScanError {
            code: "bad_character",
            message: "bad character in action".to_string(),
            offset: abs_base + i,
        });
    }
    if variable {
        Ok((TokKind::Variable, i))
    } else {
        Ok((TokKind::Field, i))
    }
}

fn lex_number(bytes: &[u8], start: usize, abs_base: usize) -> Result<usize, GoTemplateScanError> {
    let mut i = start;
    let end1 = scan_number(bytes, &mut i, abs_base, start)?;
    if end1 == start {
        return Err(GoTemplateScanError {
            code: "bad_number_syntax",
            message: "bad number syntax".to_string(),
            offset: abs_base + start,
        });
    }
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        let mut k = i;
        let end2 = scan_number(bytes, &mut k, abs_base, start)?;
        if end2 == i || bytes.get(k.wrapping_sub(1)).copied() != Some(b'i') {
            return Err(GoTemplateScanError {
                code: "bad_number_syntax",
                message: "bad number syntax".to_string(),
                offset: abs_base + start,
            });
        }
        i = k;
    }
    if !at_terminator(bytes, i) {
        return Err(GoTemplateScanError {
            code: "bad_number_syntax",
            message: "bad number syntax".to_string(),
            offset: abs_base + start,
        });
    }
    Ok(i)
}

fn scan_number(
    bytes: &[u8],
    i: &mut usize,
    abs_base: usize,
    start: usize,
) -> Result<usize, GoTemplateScanError> {
    let len = bytes.len();
    if *i < len && (bytes[*i] == b'+' || bytes[*i] == b'-') {
        *i += 1;
    }

    let mut base = 10u8;
    let mut require_digit_after_prefix = false;
    let mut leading_decimal_zero = false;
    if *i < len && bytes[*i] == b'0' {
        *i += 1;
        leading_decimal_zero = true;
        if *i < len && (bytes[*i] == b'x' || bytes[*i] == b'X') {
            *i += 1;
            base = 16;
            leading_decimal_zero = false;
            require_digit_after_prefix = true;
        } else if *i < len && (bytes[*i] == b'o' || bytes[*i] == b'O') {
            *i += 1;
            base = 8;
            leading_decimal_zero = false;
            require_digit_after_prefix = true;
        } else if *i < len && (bytes[*i] == b'b' || bytes[*i] == b'B') {
            *i += 1;
            base = 2;
            leading_decimal_zero = false;
            require_digit_after_prefix = true;
        }
    }

    let mut saw_digit = leading_decimal_zero;
    saw_digit |= consume_number_digits(bytes, i, base);

    if *i < len && bytes[*i] == b'.' {
        *i += 1;
        let saw_fraction_digit = consume_number_digits(bytes, i, base);
        saw_digit |= saw_fraction_digit;
        if !saw_fraction_digit {
            return Err(GoTemplateScanError {
                code: "bad_number_syntax",
                message: "bad number syntax".to_string(),
                offset: abs_base + start,
            });
        }
    }

    if base == 10 && *i < len && (bytes[*i] == b'e' || bytes[*i] == b'E') {
        *i += 1;
        if *i < len && (bytes[*i] == b'+' || bytes[*i] == b'-') {
            *i += 1;
        }
        let mut exp_digits = false;
        while *i < len && (bytes[*i].is_ascii_digit() || bytes[*i] == b'_') {
            if bytes[*i].is_ascii_digit() {
                exp_digits = true;
            }
            *i += 1;
        }
        if !exp_digits {
            return Err(GoTemplateScanError {
                code: "bad_number_syntax",
                message: "bad number syntax".to_string(),
                offset: abs_base + start,
            });
        }
    }

    if base == 16 && *i < len && (bytes[*i] == b'p' || bytes[*i] == b'P') {
        *i += 1;
        if *i < len && (bytes[*i] == b'+' || bytes[*i] == b'-') {
            *i += 1;
        }
        let mut exp_digits = false;
        while *i < len && (bytes[*i].is_ascii_digit() || bytes[*i] == b'_') {
            if bytes[*i].is_ascii_digit() {
                exp_digits = true;
            }
            *i += 1;
        }
        if !exp_digits {
            return Err(GoTemplateScanError {
                code: "bad_number_syntax",
                message: "bad number syntax".to_string(),
                offset: abs_base + start,
            });
        }
    }

    if require_digit_after_prefix && !saw_digit {
        return Err(GoTemplateScanError {
            code: "bad_number_syntax",
            message: "bad number syntax".to_string(),
            offset: abs_base + start,
        });
    }
    if !require_digit_after_prefix && !saw_digit {
        return Err(GoTemplateScanError {
            code: "bad_number_syntax",
            message: "bad number syntax".to_string(),
            offset: abs_base + start,
        });
    }

    if *i < len && bytes[*i] == b'i' {
        *i += 1;
    }

    Ok(*i)
}

fn consume_number_digits(bytes: &[u8], i: &mut usize, base: u8) -> bool {
    let len = bytes.len();
    let mut saw_digit = false;
    while *i < len && is_number_digit(bytes[*i], base) {
        if bytes[*i] != b'_' {
            saw_digit = true;
        }
        *i += 1;
    }
    saw_digit
}

fn at_terminator(bytes: &[u8], i: usize) -> bool {
    if i >= bytes.len() {
        return true;
    }
    let b = bytes[i];
    is_space(b)
        || matches!(
            b,
            b'.' | b',' | b'|' | b':' | b'=' | b')' | b'(' | b'"' | b'\'' | b'`'
        )
}

pub(super) fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn consume_identifier(bytes: &[u8], start: usize, allow_digit_first: bool) -> Option<usize> {
    let (first, mut i) = decode_utf8_char(bytes, start)?;
    if allow_digit_first {
        if !is_identifier_continue_char(first) {
            return None;
        }
    } else if !is_identifier_start_char(first) {
        return None;
    }
    while i < bytes.len() {
        let Some((ch, next)) = decode_utf8_char(bytes, i) else {
            return None;
        };
        if !is_identifier_continue_char(ch) {
            break;
        }
        i = next;
    }
    Some(i)
}

fn is_identifier_start_at(bytes: &[u8], start: usize) -> bool {
    decode_utf8_char(bytes, start).is_some_and(|(ch, _)| is_identifier_start_char(ch))
}

fn decode_utf8_char(bytes: &[u8], start: usize) -> Option<(char, usize)> {
    let tail = std::str::from_utf8(bytes.get(start..)?).ok()?;
    let ch = tail.chars().next()?;
    Some((ch, start + ch.len_utf8()))
}

fn is_number_digit(b: u8, base: u8) -> bool {
    if b == b'_' {
        return true;
    }
    match base {
        2 => matches!(b, b'0' | b'1'),
        8 => (b'0'..=b'7').contains(&b),
        10 => b.is_ascii_digit(),
        16 => b.is_ascii_hexdigit(),
        _ => false,
    }
}
