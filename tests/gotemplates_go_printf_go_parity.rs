use happ::go_compat::compat::go_printf;
use happ::gotemplates::encode_go_bytes_value;
use serde_json::{json, Number, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[path = "gotemplates/go_printf_cases.rs"]
mod gotemplates_go_printf_cases;

#[derive(Debug, Clone)]
enum Arg {
    Nil,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(&'static str),
    RawStrBytes(&'static [u8]),
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
            Self::RawStrBytes(v) => Some(happ::gotemplates::encode_go_string_bytes_value(v)),
            Self::Bytes(v) => Some(encode_go_bytes_value(v)),
            Self::Strs(v) => Some(Value::Array(
                v.iter().map(|s| Value::String((*s).to_string())).collect(),
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
            Self::RawStrBytes(v) => json!({"k":"raw_string_bytes","y":v}),
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

    let cases = gotemplates_go_printf_cases::cases();
    let mut go_batch = Vec::with_capacity(cases.len());
    for case in &cases {
        let go_args: Vec<Value> = case.args.iter().map(Arg::to_go_wire).collect();
        go_batch.push(json!({"fmt": case.fmt, "args": go_args}));
    }
    let go_outputs = runner
        .sprintf_batch(&go_batch)
        .expect("go fmt.Sprintf must succeed");
    assert_eq!(
        go_outputs.len(),
        cases.len(),
        "go batch size mismatch: got={} want={}",
        go_outputs.len(),
        cases.len()
    );

    for (idx, case) in cases.into_iter().enumerate() {
        let rust_args: Vec<Option<Value>> = case.args.iter().map(Arg::to_rust_value).collect();
        let rust_out = go_printf(case.fmt, &rust_args).expect("compat go_printf must succeed");
        let go_out = &go_outputs[idx];
        assert_eq!(
            rust_out, *go_out,
            "mismatch for source_line={} fmt={} args={:?}",
            case.source_line, case.fmt, case.args
        );
    }
}

#[test]
fn compat_go_printf_matches_go_fmt_generated_matrix() {
    if !has_go_toolchain() {
        eprintln!("skip: go toolchain is unavailable");
        return;
    }

    let runner = GoFmtRunner::new().expect("prepare go fmt runner");

    let pool = vec![
        Arg::Int(-7),
        Arg::Uint(u64::MAX),
        Arg::Float(1.25),
        Arg::Str("abc"),
        Arg::Bytes(b"ab"),
        Arg::Bool(true),
        Arg::Nil,
    ];
    let formats = vec![
        "%d",
        "%+d",
        "% d",
        "%08d",
        "%-8d",
        "%.3d",
        "%8.3d",
        "%x",
        "%#x",
        "% X",
        "%# X",
        "%o",
        "%O",
        "%b",
        "%c",
        "%U",
        "%s",
        "%.2s",
        "%8.2s",
        "%q",
        "%+q",
        "%#q",
        "%v",
        "%+v",
        "%#v",
        "%T",
        "%t",
        "%f",
        "%.2f",
        "%8.2f",
        "%e",
        "%E",
        "%g",
        "%G",
        "%[2]d",
        "%[3]q",
        "%[1]x %#[1]x",
        "%[8]d",
        "%[2]*d",
        "%[2]*.[1]*f",
        "%[1].2d",
        "%[1]2d",
        "%3.[2]d",
        "%.[2]d",
        "%[5]d %[2]d %d",
        "%d %[3]d %d",
        "%[2]2d",
        "%[2].2d",
        "%[d",
        "%[]d",
        "%[-3]d",
        "%[99]d",
        "%[3]",
        "%]d",
        "%.[]",
        "%*d",
        "%.*f",
        "%*.*f",
        "%d %d %d",
        "%s %q %x",
        "%v %T %#[1]v",
        "%q %q %q",
    ];
    let arg_sizes = [0usize, 1, 2, 3, 4, 7];
    let mut arg_sets = vec![
        vec![],
        vec![Arg::Int(-7)],
        vec![Arg::Str("x")],
        vec![Arg::Float(1.25)],
        vec![Arg::Bool(true)],
        vec![Arg::Nil],
        vec![Arg::RawStrBytes(&[0x97])],
        vec![Arg::RawStrBytes(&[0x97, 0x61])],
        vec![Arg::Int(-7), Arg::Int(3)],
        vec![Arg::Str("x"), Arg::Int(3)],
        vec![Arg::Int(-7), Arg::Str("x")],
        vec![Arg::Int(-7), Arg::Float(1.25)],
        vec![Arg::Int(-7), Arg::Int(3), Arg::Float(1.25)],
        vec![Arg::Int(-7), Arg::Bool(true)],
        vec![Arg::Uint(u64::MAX), Arg::Int(3)],
    ];
    for size in arg_sizes {
        arg_sets.push(pool.iter().take(size).cloned().collect());
    }

    let mut matrix: Vec<(&str, Vec<Arg>)> = Vec::new();
    for fmt in &formats {
        for args in &arg_sets {
            matrix.push((*fmt, args.clone()));
        }
    }

    let mut go_batch = Vec::with_capacity(matrix.len());
    for (fmt, args) in &matrix {
        let go_args: Vec<Value> = args.iter().map(Arg::to_go_wire).collect();
        go_batch.push(json!({"fmt": fmt, "args": go_args}));
    }
    let go_outputs = runner
        .sprintf_batch(&go_batch)
        .expect("go fmt.Sprintf must succeed");
    assert_eq!(
        go_outputs.len(),
        matrix.len(),
        "go batch size mismatch: got={} want={}",
        go_outputs.len(),
        matrix.len()
    );

    for (idx, (fmt, args)) in matrix.into_iter().enumerate() {
        let rust_args: Vec<Option<Value>> = args.iter().map(Arg::to_rust_value).collect();
        let rust_out = go_printf(fmt, &rust_args).expect("compat go_printf must succeed");
        let go_out = &go_outputs[idx];
        assert_eq!(
            rust_out, *go_out,
            "generated mismatch idx={} fmt={} args={:?}",
            idx, fmt, args
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

    fn sprintf_batch(&self, cases: &[Value]) -> Result<Vec<String>, String> {
        let payload = serde_json::to_string(cases).map_err(|e| format!("serialize cases: {e}"))?;
        let encoded_payload = base64_encode(payload.as_bytes());

        let output = Command::new("go")
            .arg("run")
            .arg(&self.program)
            .arg(encoded_payload)
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
        serde_json::from_slice::<Vec<String>>(&output.stdout)
            .map_err(|e| format!("decode go results: {e}"))
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

type wireCase struct {
	Fmt string `json:"fmt"`
	Args []wireArg `json:"args"`
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
		return uint(a.U), nil
	case "float":
		return a.F, nil
	case "string":
		return a.S, nil
	case "raw_string_bytes":
		return string([]byte(a.Y)), nil
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
	if len(os.Args) != 2 {
		fmt.Print("need cases payload")
		os.Exit(3)
	}
	payloadBytes, err := base64.StdEncoding.DecodeString(os.Args[1])
	if err != nil {
		fmt.Print(err.Error())
		os.Exit(4)
	}
	var cases []wireCase
	if err := json.Unmarshal(payloadBytes, &cases); err != nil {
		fmt.Print(err.Error())
		os.Exit(6)
	}

	results := make([]string, 0, len(cases))
	for _, c := range cases {
		args := make([]any, 0, len(c.Args))
		for _, item := range c.Args {
			v, err := decodeArg(item)
			if err != nil {
				fmt.Print(err.Error())
				os.Exit(7)
			}
			args = append(args, v)
		}
		results = append(results, fmt.Sprintf(c.Fmt, args...))
	}

	encoded, err := json.Marshal(results)
	if err != nil {
		fmt.Print(err.Error())
		os.Exit(8)
	}
	fmt.Print(string(encoded))
}
"#
}
