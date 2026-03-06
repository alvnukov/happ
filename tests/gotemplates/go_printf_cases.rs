use super::{Arg, Case};

// Cases are taken from Go source:
// $GOROOT/src/fmt/fmt_test.go (fmtTests subset supported by our runtime)
pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            source_line: 147,
            fmt: "%d",
            args: vec![Arg::Int(12345)],
        },
        Case {
            source_line: 148,
            fmt: "%v",
            args: vec![Arg::Int(12345)],
        },
        Case {
            source_line: 741,
            fmt: "%v",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 149,
            fmt: "%t",
            args: vec![Arg::Bool(true)],
        },
        Case {
            source_line: 152,
            fmt: "%s",
            args: vec![Arg::Str("abc")],
        },
        Case {
            source_line: 824,
            fmt: "%s",
            args: vec![Arg::Int(7)],
        },
        Case {
            source_line: 154,
            fmt: "%x",
            args: vec![Arg::Str("abc")],
        },
        Case {
            source_line: 174,
            fmt: "%q",
            args: vec![Arg::Bytes(b"abc")],
        },
        Case {
            source_line: 154,
            fmt: "%x",
            args: vec![Arg::Bytes(b"abc")],
        },
        Case {
            source_line: 0,
            fmt: "%+q",
            args: vec![Arg::Bytes("日本語".as_bytes())],
        },
        Case {
            source_line: 158,
            fmt: "%x",
            args: vec![Arg::Str("")],
        },
        Case {
            source_line: 159,
            fmt: "% x",
            args: vec![Arg::Str("")],
        },
        Case {
            source_line: 160,
            fmt: "%#x",
            args: vec![Arg::Str("")],
        },
        Case {
            source_line: 161,
            fmt: "%# x",
            args: vec![Arg::Str("")],
        },
        Case {
            source_line: 162,
            fmt: "%x",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 163,
            fmt: "%X",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 164,
            fmt: "% x",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 165,
            fmt: "% X",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 166,
            fmt: "%#x",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 167,
            fmt: "%#X",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 168,
            fmt: "%# x",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 169,
            fmt: "%# X",
            args: vec![Arg::Str("xyz")],
        },
        Case {
            source_line: 247,
            fmt: "%c",
            args: vec![Arg::Uint('x' as u64)],
        },
        Case {
            source_line: 659,
            fmt: "%c",
            args: vec![Arg::Bytes(b"ABC")],
        },
        Case {
            source_line: 248,
            fmt: "%c",
            args: vec![Arg::Int(0xe4)],
        },
        Case {
            source_line: 251,
            fmt: "%.0c",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 252,
            fmt: "%3c",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 221,
            fmt: "%03c",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 153,
            fmt: "%q",
            args: vec![Arg::Str("abc")],
        },
        Case {
            source_line: 0,
            fmt: "%T",
            args: vec![Arg::Int(7)],
        },
        Case {
            source_line: 0,
            fmt: "%10T",
            args: vec![Arg::Bool(true)],
        },
        Case {
            source_line: 0,
            fmt: "%T",
            args: vec![Arg::Nil],
        },
        Case {
            source_line: 255,
            fmt: "%q",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 0,
            fmt: "%+q",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 256,
            fmt: "%q",
            args: vec![Arg::Int('\n' as i64)],
        },
        Case {
            source_line: 291,
            fmt: "%q",
            args: vec![Arg::Int(0x0e00)],
        },
        Case {
            source_line: 292,
            fmt: "%q",
            args: vec![Arg::Int(0x10ffff)],
        },
        Case {
            source_line: 294,
            fmt: "%q",
            args: vec![Arg::Int(-1)],
        },
        Case {
            source_line: 763,
            fmt: "%q",
            args: vec![Arg::Strs(&["a", "b"])],
        },
        Case {
            source_line: 296,
            fmt: "%q",
            args: vec![Arg::Int(0x110000)],
        },
        Case {
            source_line: 218,
            fmt: "%10q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 220,
            fmt: "%-10q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 222,
            fmt: "%010q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 230,
            fmt: "%+10q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 232,
            fmt: "%+-10q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 234,
            fmt: "%+010q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 236,
            fmt: "%+-010q",
            args: vec![Arg::Str("⌘")],
        },
        Case {
            source_line: 225,
            fmt: "% q",
            args: vec![Arg::Str("☺")],
        },
        Case {
            source_line: 237,
            fmt: "%#8q",
            args: vec![Arg::Str("\n")],
        },
        Case {
            source_line: 238,
            fmt: "%#+8q",
            args: vec![Arg::Str("\r")],
        },
        Case {
            source_line: 239,
            fmt: "%#-8q",
            args: vec![Arg::Str("\t")],
        },
        Case {
            source_line: 240,
            fmt: "%#+-8q",
            args: vec![Arg::Str("\u{0008}")],
        },
        Case {
            source_line: 328,
            fmt: "%.5q",
            args: vec![Arg::Str("abcdefghijklmnopqrstuvwxyz")],
        },
        Case {
            source_line: 329,
            fmt: "%.5q",
            args: vec![Arg::Bytes(b"abcdefghijklmnopqrstuvwxyz")],
        },
        Case {
            source_line: 332,
            fmt: "%.3q",
            args: vec![Arg::Str("日本語日本語")],
        },
        Case {
            source_line: 333,
            fmt: "%.3q",
            args: vec![Arg::Bytes("日本語日本語".as_bytes())],
        },
        Case {
            source_line: 334,
            fmt: "%.1q",
            args: vec![Arg::Str("日本語")],
        },
        Case {
            source_line: 335,
            fmt: "%.1q",
            args: vec![Arg::Bytes("日本語".as_bytes())],
        },
        Case {
            source_line: 208,
            fmt: "%+q",
            args: vec![Arg::Str("日本語")],
        },
        Case {
            source_line: 193,
            fmt: "%#q",
            args: vec![Arg::Str("")],
        },
        Case {
            source_line: 195,
            fmt: "%#q",
            args: vec![Arg::Str("\"")],
        },
        Case {
            source_line: 197,
            fmt: "%#q",
            args: vec![Arg::Str("`")],
        },
        Case {
            source_line: 199,
            fmt: "%#q",
            args: vec![Arg::Str("\n")],
        },
        Case {
            source_line: 201,
            fmt: "%#q",
            args: vec![Arg::Str("\\n")],
        },
        Case {
            source_line: 203,
            fmt: "%#q",
            args: vec![Arg::Str("abc")],
        },
        Case {
            source_line: 206,
            fmt: "%#q",
            args: vec![Arg::Str("日本語")],
        },
        Case {
            source_line: 209,
            fmt: "%#+q",
            args: vec![Arg::Str("日本語")],
        },
        Case {
            source_line: 0,
            fmt: "%#+q",
            args: vec![Arg::Str("☺\n")],
        },
        Case {
            source_line: 241,
            fmt: "%#q",
            args: vec![Arg::Str("\u{FFFD}")],
        },
        Case {
            source_line: 671,
            fmt: "%#v",
            args: vec![Arg::Bytes(&[1, 11, 111])],
        },
        Case {
            source_line: 720,
            fmt: "%#v",
            args: vec![Arg::Int(1_000_000_000)],
        },
        Case {
            source_line: 719,
            fmt: "%#v",
            args: vec![Arg::Uint(u64::MAX)],
        },
        Case {
            source_line: 721,
            fmt: "%#v",
            args: vec![Arg::MapStrInt(&[("a", 1)])],
        },
        Case {
            source_line: 719,
            fmt: "%#v",
            args: vec![Arg::MapStrUint(&[("a", u64::MAX)])],
        },
        Case {
            source_line: 723,
            fmt: "%#v",
            args: vec![Arg::Strs(&["a", "b"])],
        },
        Case {
            source_line: 733,
            fmt: "%#v",
            args: vec![Arg::Str("foo")],
        },
        Case {
            source_line: 741,
            fmt: "%#v",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 742,
            fmt: "%#v",
            args: vec![Arg::Float(1_000_000.0)],
        },
        Case {
            source_line: 339,
            fmt: "%d",
            args: vec![Arg::Uint(12345)],
        },
        Case {
            source_line: 357,
            fmt: "%d",
            args: vec![Arg::Uint(u64::MAX)],
        },
        Case {
            source_line: 340,
            fmt: "%d",
            args: vec![Arg::Int(-12345)],
        },
        Case {
            source_line: 349,
            fmt: "%.d",
            args: vec![Arg::Int(0)],
        },
        Case {
            source_line: 351,
            fmt: "%6.0d",
            args: vec![Arg::Int(0)],
        },
        Case {
            source_line: 352,
            fmt: "%06.0d",
            args: vec![Arg::Int(0)],
        },
        Case {
            source_line: 366,
            fmt: "%o",
            args: vec![Arg::Int(668)],
        },
        Case {
            source_line: 657,
            fmt: "%o",
            args: vec![Arg::Bytes(b"ABC")],
        },
        Case {
            source_line: 367,
            fmt: "%o",
            args: vec![Arg::Int(-668)],
        },
        Case {
            source_line: 368,
            fmt: "%#o",
            args: vec![Arg::Int(668)],
        },
        Case {
            source_line: 369,
            fmt: "%#o",
            args: vec![Arg::Int(-668)],
        },
        Case {
            source_line: 367,
            fmt: "%O",
            args: vec![Arg::Int(668)],
        },
        Case {
            source_line: 368,
            fmt: "%O",
            args: vec![Arg::Int(-668)],
        },
        Case {
            source_line: 657,
            fmt: "%b",
            args: vec![Arg::Bytes(b"ABC")],
        },
        Case {
            source_line: 382,
            fmt: "%20.8d",
            args: vec![Arg::Int(1234)],
        },
        Case {
            source_line: 383,
            fmt: "%20.8d",
            args: vec![Arg::Int(-1234)],
        },
        Case {
            source_line: 384,
            fmt: "%020.8d",
            args: vec![Arg::Int(1234)],
        },
        Case {
            source_line: 385,
            fmt: "%020.8d",
            args: vec![Arg::Int(-1234)],
        },
        Case {
            source_line: 388,
            fmt: "%-#20.8x",
            args: vec![Arg::Int(0x1234abc)],
        },
        Case {
            source_line: 389,
            fmt: "%-#20.8X",
            args: vec![Arg::Int(0x1234abc)],
        },
        Case {
            source_line: 390,
            fmt: "%-#20.8o",
            args: vec![Arg::Int(668)],
        },
        Case {
            source_line: 404,
            fmt: "%U",
            args: vec![Arg::Int(0)],
        },
        Case {
            source_line: 657,
            fmt: "%U",
            args: vec![Arg::Bytes(b"ABC")],
        },
        Case {
            source_line: 405,
            fmt: "%U",
            args: vec![Arg::Int(-1)],
        },
        Case {
            source_line: 406,
            fmt: "%U",
            args: vec![Arg::Int('\n' as i64)],
        },
        Case {
            source_line: 407,
            fmt: "%#U",
            args: vec![Arg::Int('\n' as i64)],
        },
        Case {
            source_line: 411,
            fmt: "%#U",
            args: vec![Arg::Int('☺' as i64)],
        },
        Case {
            source_line: 410,
            fmt: "%#.2U",
            args: vec![Arg::Int('x' as i64)],
        },
        Case {
            source_line: 413,
            fmt: "%#14.6U",
            args: vec![Arg::Int('⌘' as i64)],
        },
        Case {
            source_line: 635,
            fmt: "%20.5s",
            args: vec![Arg::Str("qwertyuiop")],
        },
        Case {
            source_line: 0,
            fmt: "%s",
            args: vec![Arg::Bytes(b"abc")],
        },
        Case {
            source_line: 636,
            fmt: "%.5s",
            args: vec![Arg::Str("qwertyuiop")],
        },
        Case {
            source_line: 637,
            fmt: "%-20.5s",
            args: vec![Arg::Str("qwertyuiop")],
        },
        Case {
            source_line: 603,
            fmt: "%e",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 423,
            fmt: "%+.3x",
            args: vec![Arg::Float(0.0)],
        },
        Case {
            source_line: 424,
            fmt: "%+.3x",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 459,
            fmt: "%#.0x",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 464,
            fmt: "%#.4x",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 448,
            fmt: "%b",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 490,
            fmt: "%.4b",
            args: vec![Arg::Float(-1.0)],
        },
        Case {
            source_line: 518,
            fmt: "%+.3F",
            args: vec![Arg::Float(-1.0)],
        },
        Case {
            source_line: 604,
            fmt: "%g",
            args: vec![Arg::Float(1234.5678e3)],
        },
        Case {
            source_line: 608,
            fmt: "%g",
            args: vec![Arg::Float(-1e-9)],
        },
        Case {
            source_line: 610,
            fmt: "%E",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 615,
            fmt: "%G",
            args: vec![Arg::Float(1234.5678e3)],
        },
        Case {
            source_line: 619,
            fmt: "%G",
            args: vec![Arg::Float(-1e-9)],
        },
        Case {
            source_line: 451,
            fmt: "%#g",
            args: vec![Arg::Float(-1.0)],
        },
        Case {
            source_line: 453,
            fmt: "%#g",
            args: vec![Arg::Float(123_456.0)],
        },
        Case {
            source_line: 455,
            fmt: "%#g",
            args: vec![Arg::Float(1_230_000.0)],
        },
        Case {
            source_line: 457,
            fmt: "%#.0f",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 463,
            fmt: "%#.4e",
            args: vec![Arg::Float(1.0)],
        },
        Case {
            source_line: 478,
            fmt: "%#.0x",
            args: vec![Arg::Float(123.0)],
        },
        Case {
            source_line: 0,
            fmt: "%#x",
            args: vec![Arg::Float(1.25)],
        },
        Case {
            source_line: 0,
            fmt: "%#X",
            args: vec![Arg::Float(1.25)],
        },
        Case {
            source_line: 482,
            fmt: "%#.4x",
            args: vec![Arg::Float(123.0)],
        },
        Case {
            source_line: 470,
            fmt: "%#.4g",
            args: vec![Arg::Float(0.12)],
        },
        Case {
            source_line: 681,
            fmt: "% d",
            args: vec![Arg::Int(7)],
        },
        Case {
            source_line: 681,
            fmt: "%+d",
            args: vec![Arg::Int(7)],
        },
        // Type-mismatch marker shape (mirrors fmt behavior).
        Case {
            source_line: 824,
            fmt: "%d",
            args: vec![Arg::Str("7")],
        },
        Case {
            source_line: 0,
            fmt: "%",
            args: vec![],
        },
        Case {
            source_line: 0,
            fmt: "%d",
            args: vec![],
        },
        Case {
            source_line: 0,
            fmt: "%d",
            args: vec![Arg::Int(1), Arg::Int(2)],
        },
        Case {
            source_line: 0,
            fmt: "%*d",
            args: vec![Arg::Str("x"), Arg::Int(7)],
        },
        Case {
            source_line: 0,
            fmt: "%.*d",
            args: vec![Arg::Str("x"), Arg::Int(7)],
        },
        Case {
            source_line: 0,
            fmt: "%*.*f",
            args: vec![Arg::Int(8), Arg::Int(2), Arg::Float(1.2)],
        },
        Case {
            source_line: 1223,
            fmt: "%2147483648d",
            args: vec![Arg::Int(42)],
        },
        Case {
            source_line: 1224,
            fmt: "%-2147483648d",
            args: vec![Arg::Int(42)],
        },
        Case {
            source_line: 1225,
            fmt: "%.2147483648d",
            args: vec![Arg::Int(42)],
        },
        Case {
            source_line: 1673,
            fmt: "%*d",
            args: vec![Arg::Int(10_000_000), Arg::Int(42)],
        },
        Case {
            source_line: 1674,
            fmt: "%*d",
            args: vec![Arg::Int(-10_000_000), Arg::Int(42)],
        },
        Case {
            source_line: 1677,
            fmt: "%.*d",
            args: vec![Arg::Int(10_000_000), Arg::Int(42)],
        },
        Case {
            source_line: 1679,
            fmt: "%.*d",
            args: vec![Arg::Uint(1u64 << 63), Arg::Int(42)],
        },
        Case {
            source_line: 1680,
            fmt: "%.*d",
            args: vec![Arg::Uint(u64::MAX), Arg::Int(42)],
        },
        Case {
            source_line: 1683,
            fmt: "%*",
            args: vec![Arg::Int(4)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]d %[1]d",
            args: vec![Arg::Int(1), Arg::Int(2)],
        },
        Case {
            source_line: 1191,
            fmt: "%[1]d",
            args: vec![Arg::Int(1)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*d",
            args: vec![],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*d",
            args: vec![Arg::Int(-7)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*d",
            args: vec![Arg::Str("x")],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*d",
            args: vec![Arg::Int(-7), Arg::Int(3)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*.[1]*f",
            args: vec![],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*.[1]*f",
            args: vec![Arg::Int(-7)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]*.[1]*f",
            args: vec![Arg::Int(-7), Arg::Int(3)],
        },
        Case {
            source_line: 1193,
            fmt: "%[2]*[1]d",
            args: vec![Arg::Int(2), Arg::Int(5)],
        },
        Case {
            source_line: 1195,
            fmt: "%[3]*.[2]*[1]f",
            args: vec![Arg::Float(12.0), Arg::Int(2), Arg::Int(6)],
        },
        Case {
            source_line: 1196,
            fmt: "%[1]*.[2]*[3]f",
            args: vec![Arg::Int(6), Arg::Int(2), Arg::Float(12.0)],
        },
        Case {
            source_line: 1198,
            fmt: "%[1]*[3]f",
            args: vec![Arg::Int(10), Arg::Int(99), Arg::Float(12.0)],
        },
        Case {
            source_line: 1200,
            fmt: "%.[1]*[3]f",
            args: vec![Arg::Int(6), Arg::Int(99), Arg::Float(12.0)],
        },
        Case {
            source_line: 1202,
            fmt: "%[1]*.[3]f",
            args: vec![Arg::Int(6), Arg::Int(3), Arg::Float(12.0)],
        },
        Case {
            source_line: 1204,
            fmt: "%d %d %d %#[1]o %#o %#o",
            args: vec![Arg::Int(11), Arg::Int(12), Arg::Int(13)],
        },
        Case {
            source_line: 1208,
            fmt: "%[d",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1209,
            fmt: "%]d",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1210,
            fmt: "%[]d",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1211,
            fmt: "%[-3]d",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1212,
            fmt: "%[99]d",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1213,
            fmt: "%[3]",
            args: vec![Arg::Int(2), Arg::Int(1)],
        },
        Case {
            source_line: 1214,
            fmt: "%[1].2d",
            args: vec![Arg::Int(5), Arg::Int(6)],
        },
        Case {
            source_line: 1219,
            fmt: "%[5]d %[2]d %d",
            args: vec![Arg::Int(1), Arg::Int(2), Arg::Int(3)],
        },
        Case {
            source_line: 1220,
            fmt: "%d %[3]d %d",
            args: vec![Arg::Int(1), Arg::Int(2)],
        },
        Case {
            source_line: 0,
            fmt: "%[2]2d",
            args: vec![Arg::Int(1), Arg::Int(2)],
        },
        Case {
            source_line: 0,
            fmt: "%[2].2d",
            args: vec![Arg::Int(1), Arg::Int(2)],
        },
        Case {
            source_line: 1216,
            fmt: "%3.[2]d",
            args: vec![Arg::Int(7)],
        },
        Case {
            source_line: 1217,
            fmt: "%.[2]d",
            args: vec![Arg::Int(7)],
        },
        Case {
            source_line: 1218,
            fmt: "%d %d %d %#[1]o %#o %#o %#o",
            args: vec![Arg::Int(11), Arg::Int(12), Arg::Int(13)],
        },
        Case {
            source_line: 1221,
            fmt: "%.[]",
            args: vec![],
        },
        Case {
            source_line: 1234,
            fmt: "%.-3d",
            args: vec![Arg::Int(42)],
        },
        Case {
            source_line: 0,
            fmt: "%v",
            args: vec![Arg::Nil],
        },
        // Raw Go string bytes (invalid UTF-8) are produced by template string slicing.
        Case {
            source_line: 0,
            fmt: "%x",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%q",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%T",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%s",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%d",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%v",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%#v",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%+q",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%#q",
            args: vec![Arg::RawStrBytes(&[0x97])],
        },
        Case {
            source_line: 0,
            fmt: "%#v",
            args: vec![Arg::RawStrBytes(b"ab")],
        },
        Case {
            source_line: 0,
            fmt: "%#q",
            args: vec![Arg::RawStrBytes(b"ab")],
        },
        Case {
            source_line: 0,
            fmt: "%x",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "%X",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "% x",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "% X",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "%#x",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "%#X",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "%# x",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
        Case {
            source_line: 0,
            fmt: "%# X",
            args: vec![Arg::RawStrBytes(&[0x97, 0x61])],
        },
    ]
}
