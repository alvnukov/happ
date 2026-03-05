use happ::gotemplates::compat::go_printf;
use happ::gotemplates::encode_go_bytes_value;
use serde_json::{json, Number, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Clone)]
enum Arg {
    Nil,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(&'static str),
    Bytes(&'static [u8]),
    Strs(&'static [&'static str]),
    MapStrInt(&'static [(&'static str, i64)]),
    MapStrUint(&'static [(&'static str, u64)]),
}

impl Arg {
    fn to_rust_value(&self) -> Option<Value> {
        match self {
            Self::Nil => None,
            Self::Bool(v) => Some(Value::Bool(*v)),
            Self::Int(v) => Some(Value::Number(Number::from(*v))),
            Self::Uint(v) => Some(Value::Number(Number::from(*v))),
            Self::Float(v) => Number::from_f64(*v).map(Value::Number),
            Self::Str(v) => Some(Value::String((*v).to_string())),
            Self::Bytes(v) => Some(encode_go_bytes_value(v)),
            Self::Strs(v) => Some(Value::Array(
                v.iter()
                    .map(|s| Value::String((*s).to_string()))
                    .collect(),
            )),
            Self::MapStrInt(v) => {
                let mut map = serde_json::Map::new();
                for (k, i) in *v {
                    map.insert((*k).to_string(), Value::Number(Number::from(*i)));
                }
                Some(Value::Object(map))
            }
            Self::MapStrUint(v) => {
                let mut map = serde_json::Map::new();
                for (k, u) in *v {
                    map.insert((*k).to_string(), Value::Number(Number::from(*u)));
                }
                Some(Value::Object(map))
            }
        }
    }

    fn to_go_wire(&self) -> Value {
        match self {
            Self::Nil => json!({"k":"nil"}),
            Self::Bool(v) => json!({"k":"bool","b":v}),
            Self::Int(v) => json!({"k":"int","i":v}),
            Self::Uint(v) => json!({"k":"uint","u":v}),
            Self::Float(v) => json!({"k":"float","f":v}),
            Self::Str(v) => json!({"k":"string","s":v}),
            Self::Bytes(v) => json!({"k":"bytes","y":v}),
            Self::Strs(v) => json!({"k":"strings","ss":v}),
            Self::MapStrInt(v) => {
                let entries: Vec<Value> = v.iter().map(|(k, i)| json!({"k":k,"i":i})).collect();
                json!({"k":"map_str_int","msi":entries})
            }
            Self::MapStrUint(v) => {
                let entries: Vec<Value> = v.iter().map(|(k, u)| json!({"k":k,"u":u})).collect();
                json!({"k":"map_str_uint","msu":entries})
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Case {
    source_line: usize,
    fmt: &'static str,
    args: Vec<Arg>,
}

#[test]
fn compat_go_printf_matches_go_fmt_subset_from_source_tests() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoFmtRunner::new().expect("prepare go fmt runner");

    // Cases are taken from Go source:
    // $GOROOT/src/fmt/fmt_test.go (fmtTests subset supported by our runtime)
    let cases = vec![
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
            source_line: 255,
            fmt: "%q",
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
            source_line: 1208,
            fmt: "%[d",
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
            source_line: 1221,
            fmt: "%.[]",
            args: vec![],
        },
        Case {
            source_line: 0,
            fmt: "%v",
            args: vec![Arg::Nil],
        },
    ];

    for case in cases {
        let rust_args: Vec<Option<Value>> = case.args.iter().map(Arg::to_rust_value).collect();
        let go_args: Vec<Value> = case.args.iter().map(Arg::to_go_wire).collect();

        let rust_out = go_printf(case.fmt, &rust_args).expect("compat go_printf must succeed");
        let go_out = runner
            .sprintf(case.fmt, &go_args)
            .expect("go fmt.Sprintf must succeed");
        assert_eq!(
            rust_out, go_out,
            "mismatch for source_line={} fmt={} args={:?}",
            case.source_line, case.fmt, case.args
        );
    }
}

fn has_go_toolchain() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

struct GoFmtRunner {
    _tmp: TempDir,
    program: PathBuf,
}

impl GoFmtRunner {
    fn new() -> Result<Self, String> {
        let tmp = TempDir::new().map_err(|e| format!("tmpdir: {e}"))?;
        let program = tmp.path().join("fmtcheck.go");
        fs::write(&program, go_program_source())
            .map_err(|e| format!("write go source {}: {e}", program.display()))?;
        Ok(Self { _tmp: tmp, program })
    }

    fn sprintf(&self, fmt: &str, args: &[Value]) -> Result<String, String> {
        let encoded_fmt = base64_encode(fmt.as_bytes());
        let args_json = serde_json::to_string(args).map_err(|e| format!("serialize args: {e}"))?;
        let encoded_args = base64_encode(args_json.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_fmt)
            .arg(encoded_args)
            .output()
            .map_err(|e| format!("go run failed to start: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "go fmt failed: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0usize;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | input[i + 2] as u32;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }

    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }

    out
}

fn go_program_source() -> &'static str {
    r#"package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
)

type wireArg struct {
	K string `json:"k"`
	B bool   `json:"b"`
	I int64  `json:"i"`
	U uint64 `json:"u"`
	F float64 `json:"f"`
	S string `json:"s"`
	Y []uint8 `json:"y"`
	SS []string `json:"ss"`
	MSI []wireMapStrIntEntry `json:"msi"`
	MSU []wireMapStrUintEntry `json:"msu"`
}

type wireMapStrIntEntry struct {
	K string `json:"k"`
	I int64 `json:"i"`
}

type wireMapStrUintEntry struct {
	K string `json:"k"`
	U uint64 `json:"u"`
}

func decodeArg(a wireArg) (any, error) {
	switch a.K {
	case "nil":
		return nil, nil
	case "bool":
		return a.B, nil
	case "int":
		return int(a.I), nil
	case "uint":
		return a.U, nil
	case "float":
		return a.F, nil
	case "string":
		return a.S, nil
	case "bytes":
		return []byte(a.Y), nil
	case "strings":
		return []string(a.SS), nil
	case "map_str_int":
		out := make(map[string]int)
		for _, e := range a.MSI {
			out[e.K] = int(e.I)
		}
		return out, nil
	case "map_str_uint":
		out := make(map[string]uint)
		for _, e := range a.MSU {
			out[e.K] = uint(e.U)
		}
		return out, nil
	default:
		return nil, fmt.Errorf("unknown kind: %s", a.K)
	}
}

func main() {
	if len(os.Args) != 3 {
		fmt.Print("need format and args")
		os.Exit(3)
	}
	fmtBytes, err := base64.StdEncoding.DecodeString(os.Args[1])
	if err != nil {
		fmt.Print(err.Error())
		os.Exit(4)
	}
	argsBytes, err := base64.StdEncoding.DecodeString(os.Args[2])
	if err != nil {
		fmt.Print(err.Error())
		os.Exit(5)
	}

	var wire []wireArg
	if err := json.Unmarshal(argsBytes, &wire); err != nil {
		fmt.Print(err.Error())
		os.Exit(6)
	}
	args := make([]any, 0, len(wire))
	for _, item := range wire {
		v, err := decodeArg(item)
		if err != nil {
			fmt.Print(err.Error())
			os.Exit(7)
		}
		args = append(args, v)
	}
	fmt.Print(fmt.Sprintf(string(fmtBytes), args...))
}
"#
}
