pub fn push_utf8_char_from_bytes(bytes: &[u8], i: usize, out: &mut String) -> usize {
    if i >= bytes.len() {
        return i;
    }
    let b = bytes[i];
    if b.is_ascii() {
        out.push(b as char);
        return i + 1;
    }
    if let Ok(rest) = std::str::from_utf8(&bytes[i..]) {
        if let Some(ch) = rest.chars().next() {
            out.push(ch);
            return i + ch.len_utf8();
        }
    }
    out.push('\u{FFFD}');
    i + 1
}
