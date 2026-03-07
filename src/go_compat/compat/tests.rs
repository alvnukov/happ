use super::*;
use serde_json::{Number, Value};
fn typed_bytes(bytes: &[u8]) -> Value {
    crate::go_compat::typedvalue::encode_go_bytes_value(bytes)
}

fn typed_nil_bytes() -> Value {
    crate::go_compat::typedvalue::encode_go_nil_bytes_value()
}

fn typed_string_bytes(bytes: &[u8]) -> Value {
    crate::go_compat::typedvalue::encode_go_string_bytes_value(bytes)
}

fn typed_map(elem_type: &str, entries: Option<serde_json::Map<String, Value>>) -> Value {
    crate::go_compat::typedvalue::encode_go_typed_map_value(elem_type, entries)
}

fn typed_slice(elem_type: &str, items: Option<Vec<Value>>) -> Value {
    crate::go_compat::typedvalue::encode_go_typed_slice_value(elem_type, items)
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
    assert_eq!(parse_char_constant("'\\007'"), Some(7));
    assert_eq!(parse_char_constant("'\\07'"), None);
    assert_eq!(parse_char_constant("'\\400'"), None);
}

#[test]
fn decode_go_string_literal_supports_go_escapes() {
    assert_eq!(decode_go_string_literal(r#""abc""#).as_deref(), Some("abc"));
    assert_eq!(
        decode_go_string_literal(r#""\x61\u263A""#).as_deref(),
        Some("a☺")
    );
    assert_eq!(
        decode_go_string_literal(r#"`raw\nstring`"#).as_deref(),
        Some("raw\\nstring")
    );
    assert_eq!(
        decode_go_string_literal(r#""\007""#).as_deref(),
        Some("\u{0007}")
    );
    assert_eq!(decode_go_string_literal(r#""\07""#), None);
    assert_eq!(decode_go_string_literal(r#""\q""#), None);
}

#[test]
fn parse_go_quoted_prefix_extracts_decoded_name_and_tail() {
    let (name, tail) = parse_go_quoted_prefix(r#""x\"y" ."#).expect("must parse");
    assert_eq!(name, r#"x"y"#);
    assert_eq!(tail, " .");

    let (raw_name, raw_tail) = parse_go_quoted_prefix("`raw name`\t.").expect("must parse");
    assert_eq!(raw_name, "raw name");
    assert_eq!(raw_tail, "\t.");
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
fn go_printf_preserves_non_ascii_literal_text() {
    let args = vec![Some(Value::Number(Number::from(7)))];
    assert_eq!(
        go_printf("привет %d", &args).expect("must render"),
        "привет 7"
    );
    assert_eq!(go_printf("😊%d", &args).expect("must render"), "😊7");
    assert_eq!(
        go_printf("до%%после %d", &args).expect("must render"),
        "до%после 7"
    );
}

#[test]
fn go_printf_applies_precision_for_q_on_strings_and_bytes() {
    let s = vec![Some(Value::String(
        "abcdefghijklmnopqrstuvwxyz".to_string(),
    ))];
    assert_eq!(go_printf("%.5q", &s).expect("must render"), "\"abcde\"");

    let b = vec![Some(typed_bytes("日本語日本語".as_bytes()))];
    assert_eq!(go_printf("%.3q", &b).expect("must render"), "\"日本語\"");
    assert_eq!(go_printf("%.1q", &b).expect("must render"), "\"日\"");
}

#[test]
fn go_printf_formats_q_for_integer_as_rune_literal() {
    let args = vec![Some(Value::Number(Number::from('⌘' as i64)))];
    assert_eq!(go_printf("%q", &args).expect("must render"), "'⌘'");
    assert_eq!(go_printf("%+q", &args).expect("must render"), "'\\u2318'");
    assert_eq!(go_printf("%.0q", &args).expect("must render"), "'⌘'");

    let args = vec![Some(Value::Number(Number::from('\n' as i64)))];
    assert_eq!(go_printf("%q", &args).expect("must render"), "'\\n'");

    let args = vec![Some(Value::Number(Number::from(0x0e00i64)))];
    assert_eq!(go_printf("%q", &args).expect("must render"), "'\\u0e00'");

    let args = vec![Some(Value::Number(Number::from(0x10ffffi64)))];
    assert_eq!(
        go_printf("%q", &args).expect("must render"),
        "'\\U0010ffff'"
    );

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

    let s = vec![Some(typed_string_bytes(&[0x97]))];
    assert_eq!(go_printf("%T", &s).expect("must render"), "string");

    let map = vec![Some(serde_json::json!({"a":1}))];
    assert_eq!(
        go_printf("%d", &map).expect("must render"),
        "%!d(map[string]int=map[a:1])"
    );

    let n = vec![Some(Value::Number(Number::from(7)))];
    assert_eq!(go_printf("%s", &n).expect("must render"), "%!s(int=7)");
}

#[test]
fn go_printf_supports_typed_map_metadata() {
    let mut int_entries = serde_json::Map::new();
    int_entries.insert("a".to_string(), Value::Number(Number::from(1)));
    let args = vec![Some(typed_map("int", Some(int_entries)))];
    assert_eq!(
        go_printf("%T", &args).expect("must render"),
        "map[string]int"
    );
    assert_eq!(go_printf("%v", &args).expect("must render"), "map[a:1]");
    assert_eq!(
        go_printf("%#v", &args).expect("must render"),
        "map[string]int{\"a\":1}"
    );

    let nil_map = vec![Some(typed_map("int", None))];
    assert_eq!(go_printf("%v", &nil_map).expect("must render"), "map[]");
    assert_eq!(
        go_printf("%#v", &nil_map).expect("must render"),
        "map[string]int(nil)"
    );
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

    let raw = vec![Some(typed_string_bytes(&[0x97]))];
    assert_eq!(go_printf("%x", &raw).expect("must render"), "97");
    assert_eq!(go_printf("%q", &raw).expect("must render"), "\"\\x97\"");
    assert_eq!(go_printf("%d", &raw).expect("must render"), "%!d(string=�)");
    assert_eq!(go_printf("%v", &raw).expect("must render"), "�");
    assert_eq!(go_printf("%#v", &raw).expect("must render"), "\"\\x97\"");
    assert_eq!(go_printf("%+q", &raw).expect("must render"), "\"\\x97\"");
    assert_eq!(go_printf("%#q", &raw).expect("must render"), "\"\\x97\"");

    let raw_ascii = vec![Some(typed_string_bytes(b"ab"))];
    assert_eq!(go_printf("%#v", &raw_ascii).expect("must render"), "\"ab\"");
    assert_eq!(go_printf("%#q", &raw_ascii).expect("must render"), "`ab`");

    let raw_two = vec![Some(typed_string_bytes(&[0x97, 0x61]))];
    assert_eq!(go_printf("%x", &raw_two).expect("must render"), "9761");
    assert_eq!(go_printf("%X", &raw_two).expect("must render"), "9761");
    assert_eq!(go_printf("% x", &raw_two).expect("must render"), "97 61");
    assert_eq!(go_printf("% X", &raw_two).expect("must render"), "97 61");
    assert_eq!(go_printf("%#x", &raw_two).expect("must render"), "0x9761");
    assert_eq!(go_printf("%#X", &raw_two).expect("must render"), "0X9761");
    assert_eq!(
        go_printf("%# x", &raw_two).expect("must render"),
        "0x97 0x61"
    );
    assert_eq!(
        go_printf("%# X", &raw_two).expect("must render"),
        "0X97 0X61"
    );
}

#[test]
fn go_printf_formats_byte_arrays_for_s_q_x() {
    let args = vec![Some(typed_bytes(&[97, 98]))];
    assert_eq!(go_printf("%s", &args).expect("must render"), "ab");
    assert_eq!(go_printf("%q", &args).expect("must render"), "\"ab\"");
    assert_eq!(go_printf("%x", &args).expect("must render"), "6162");
    assert_eq!(
        go_printf("%b", &args).expect("must render"),
        "[1100001 1100010]"
    );
    assert_eq!(go_printf("%o", &args).expect("must render"), "[141 142]");
    assert_eq!(
        go_printf("%O", &args).expect("must render"),
        "[0o141 0o142]"
    );
    assert_eq!(go_printf("%c", &args).expect("must render"), "[a b]");
    assert_eq!(
        go_printf("%U", &args).expect("must render"),
        "[U+0061 U+0062]"
    );

    let args = vec![Some(typed_bytes(&[255]))];
    assert_eq!(go_printf("%q", &args).expect("must render"), "\"\\xff\"");

    let utf8 = vec![Some(typed_bytes("日本語".as_bytes()))];
    assert_eq!(
        go_printf("%+q", &utf8).expect("must render"),
        "\"\\u65e5\\u672c\\u8a9e\""
    );
    assert_eq!(go_printf("%#q", &utf8).expect("must render"), "`日本語`");

    let bad = vec![Some(typed_bytes(&[0xff]))];
    assert_eq!(go_printf("%+q", &bad).expect("must render"), "\"\\xff\"");
}

#[test]
fn go_printf_q_combines_sharp_and_plus_like_go() {
    let args = vec![Some(Value::String("☺\n".to_string()))];
    assert_eq!(go_printf("%#q", &args).expect("must render"), "\"☺\\n\"");
    assert_eq!(
        go_printf("%#+q", &args).expect("must render"),
        "\"\\u263a\\n\""
    );
}

#[test]
fn go_printf_formats_q_for_non_byte_arrays() {
    let args = vec![Some(Value::Array(vec![
        Value::String("a".to_string()),
        Value::String("b".to_string()),
    ]))];
    assert_eq!(
        go_printf("%q", &args).expect("must render"),
        "[\"a\" \"b\"]"
    );
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
    let nil_bytes = vec![Some(typed_nil_bytes())];
    assert_eq!(go_printf("%v", &nil_bytes).expect("must render"), "[]");
    assert_eq!(
        go_printf("%#v", &nil_bytes).expect("must render"),
        "[]byte(nil)"
    );
    assert_eq!(go_printf("%T", &nil_bytes).expect("must render"), "[]uint8");
    let typed_slice_int = vec![Some(typed_slice(
        "int",
        Some(vec![
            Value::Number(Number::from(1)),
            Value::Number(Number::from(2)),
        ]),
    ))];
    assert_eq!(
        go_printf("%#v", &typed_slice_int).expect("must render"),
        "[]int{1, 2}"
    );
    assert_eq!(
        go_printf("%T", &typed_slice_int).expect("must render"),
        "[]int"
    );
    let nil_typed_slice_int = vec![Some(typed_slice("int", None))];
    assert_eq!(
        go_printf("%v", &nil_typed_slice_int).expect("must render"),
        "[]"
    );
    assert_eq!(
        go_printf("%#v", &nil_typed_slice_int).expect("must render"),
        "[]int(nil)"
    );
    assert_eq!(
        go_printf("%T", &nil_typed_slice_int).expect("must render"),
        "[]int"
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
    map_u.insert("a".to_string(), Value::Number(Number::from(u64::MAX)));
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
fn go_printf_formats_sharp_float_like_go_subset() {
    let one = vec![Number::from_f64(1.0).map(Value::Number)];
    assert_eq!(go_printf("%#.0f", &one).expect("must render"), "1.");
    assert_eq!(go_printf("%#.4e", &one).expect("must render"), "1.0000e+00");

    let neg_one = vec![Number::from_f64(-1.0).map(Value::Number)];
    assert_eq!(go_printf("%#g", &neg_one).expect("must render"), "-1.00000");

    let large_plain = vec![Number::from_f64(123456.0).map(Value::Number)];
    assert_eq!(
        go_printf("%#g", &large_plain).expect("must render"),
        "123456."
    );

    let large_exp = vec![Number::from_f64(1_230_000.0).map(Value::Number)];
    assert_eq!(
        go_printf("%#g", &large_exp).expect("must render"),
        "1.23000e+06"
    );

    let small = vec![Number::from_f64(0.12).map(Value::Number)];
    assert_eq!(go_printf("%#.4g", &small).expect("must render"), "0.1200");
}

#[test]
fn go_printf_formats_float_binary_like_go() {
    let one = vec![Number::from_f64(1.0).map(Value::Number)];
    assert_eq!(
        go_printf("%b", &one).expect("must render"),
        "4503599627370496p-52"
    );
    assert_eq!(
        go_printf("%.4b", &one).expect("must render"),
        "4503599627370496p-52"
    );

    let half = vec![Number::from_f64(0.5).map(Value::Number)];
    assert_eq!(
        go_printf("%b", &half).expect("must render"),
        "4503599627370496p-53"
    );

    let minus = vec![Number::from_f64(-1.0).map(Value::Number)];
    assert_eq!(
        go_printf("%.4b", &minus).expect("must render"),
        "-4503599627370496p-52"
    );
}

#[test]
fn go_printf_formats_float_hex_like_go_subset() {
    let one = vec![Number::from_f64(1.0).map(Value::Number)];
    assert_eq!(go_printf("%x", &one).expect("must render"), "0x1p+00");
    assert_eq!(go_printf("%X", &one).expect("must render"), "0X1P+00");
    assert_eq!(go_printf("%#.0x", &one).expect("must render"), "0x1.p+00");
    assert_eq!(
        go_printf("%#.4x", &one).expect("must render"),
        "0x1.0000p+00"
    );
    assert_eq!(
        go_printf("%+.3x", &one).expect("must render"),
        "+0x1.000p+00"
    );

    let half = vec![Number::from_f64(0.5).map(Value::Number)];
    assert_eq!(
        go_printf("%.3x", &half).expect("must render"),
        "0x1.000p-01"
    );

    let three_halves = vec![Number::from_f64(1.5).map(Value::Number)];
    assert_eq!(
        go_printf("%#.0x", &three_halves).expect("must render"),
        "0x1.p+01"
    );
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
    assert_eq!(
        go_printf("%*", &no_verb).expect("must render"),
        "%!(NOVERB)"
    );
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
    assert_eq!(go_printf("%[3]", &args).expect("must render"), "%!(NOVERB)");

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
    assert_eq!(go_printf("%3.[2]d", &args).expect("must render"), "  2");
    assert_eq!(go_printf("%.[2]d", &args).expect("must render"), "2");

    let one_arg = vec![Some(Value::Number(Number::from(7)))];
    assert_eq!(
        go_printf("%3.[2]d", &one_arg).expect("must render"),
        "%!d(BADINDEX)"
    );
    assert_eq!(
        go_printf("%.[2]d", &one_arg).expect("must render"),
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

    let u = vec![Some(Value::Number(Number::from(u64::MAX)))];
    assert_eq!(
        go_printf("%d", &u).expect("must render"),
        "18446744073709551615"
    );
    assert_eq!(
        go_printf("%+d", &u).expect("must render"),
        "+18446744073709551615"
    );
    assert_eq!(
        go_printf("%O", &u).expect("must render"),
        "0o1777777777777777777777"
    );
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
