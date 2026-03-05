use happ::gotemplates::compat::go_printf;
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
            source_line: 153,
            fmt: "%q",
            args: vec![Arg::Str("abc")],
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
}

func decodeArg(a wireArg) (any, error) {
	switch a.K {
	case "nil":
		return nil, nil
	case "bool":
		return a.B, nil
	case "int":
		return a.I, nil
	case "uint":
		return a.U, nil
	case "float":
		return a.F, nil
	case "string":
		return a.S, nil
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
