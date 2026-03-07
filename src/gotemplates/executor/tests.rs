use super::*;
use serde_json::json;

#[test]
fn native_renderer_renders_literals_and_simple_paths() {
    let data = json!({"a":{"b":"ok"}});
    let out = render_template_native("A{{.a.b}}C", &data).expect("must render");
    assert_eq!(out, "AokC");
}

#[test]
fn native_renderer_uses_go_missing_value_default() {
    let data = json!({"a":{"b":"ok"}});
    let out = render_template_native("{{.a.c}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
}

#[test]
fn native_renderer_go_zero_mode_keeps_leaf_missing_as_no_value() {
    let data = json!({"m":{"a":1}});
    let out = render_template_native_with_options(
        "{{.m.missing}}",
        &data,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoZero,
        },
    )
    .expect("must render");
    assert_eq!(out, "<no value>");
}

#[test]
fn native_renderer_go_zero_mode_errors_on_nested_missing_after_nil_interface() {
    let data = json!({"m":{"a":1}});
    let err = render_template_native_with_options(
        "{{.m.missing.y}}",
        &data,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoZero,
        },
    )
    .expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("nil pointer evaluating interface {}.y"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_applies_trim_markers() {
    let data = json!({"a":{"b":"ok"}});
    let out = render_template_native("x {{- .a.b -}} y", &data).expect("must render");
    assert_eq!(out, "xoky");
}

#[test]
fn native_renderer_supports_if_with_else() {
    let data = json!({"flag": false});
    let out =
        render_template_native("{{if .flag}}yes{{else}}no{{end}}", &data).expect("must render");
    assert_eq!(out, "no");
}

#[test]
fn native_renderer_supports_with() {
    let data = json!({"user": {"name":"alice"}});
    let out = render_template_native("{{with .user}}{{.name}}{{else}}none{{end}}", &data)
        .expect("must render");
    assert_eq!(out, "alice");
}

#[test]
fn native_renderer_supports_range_with_else() {
    let data = json!({"items": ["a", "b"]});
    let out = render_template_native("{{range .items}}{{.}}{{else}}empty{{end}}", &data)
        .expect("must render");
    assert_eq!(out, "ab");

    let empty = json!({"items": []});
    let out = render_template_native("{{range .items}}{{.}}{{else}}empty{{end}}", &empty)
        .expect("must render");
    assert_eq!(out, "empty");
}

#[test]
fn native_renderer_supports_template_invocation() {
    let data = json!({"v":"x"});
    let tpl = "{{define \"t\"}}<{{.v}}>{{end}}{{template \"t\" .}}";
    let out = render_template_native(tpl, &data).expect("must render");
    assert_eq!(out, "<x>");
}

#[test]
fn native_renderer_supports_template_invocation_with_arg() {
    let data = json!({"v":"x","user":{"name":"alice"}});
    let tpl = "{{define \"name\"}}{{.name}}{{end}}{{template \"name\" .user}}";
    let out = render_template_native(tpl, &data).expect("must render");
    assert_eq!(out, "alice");
}

#[test]
fn native_renderer_supports_pipeline_and_builtins_subset() {
    let data = json!({
        "items": ["x", "y"],
        "m": {"k":"v"},
        "n": 7,
        "s": "ok"
    });
    let out = render_template_native("{{print 1 2}}", &data).expect("must render");
    assert_eq!(out, "1 2");
    let out = render_template_native("{{printf \"%s-%d\" .s 7}}", &data).expect("must render");
    assert_eq!(out, "ok-7");
    let out = render_template_native("{{printf \"%f\" 1.2}}", &data).expect("must render");
    assert_eq!(out, "1.200000");
    let out = render_template_native("{{printf \"%.2f\" 1.2}}", &data).expect("must render");
    assert_eq!(out, "1.20");
    let out = render_template_native("{{printf \"%e\" 1.2}}", &data).expect("must render");
    assert_eq!(out, "1.200000e+00");
    let out = render_template_native("{{printf \"%E\" 1.2}}", &data).expect("must render");
    assert_eq!(out, "1.200000E+00");
    let out = render_template_native("{{printf \"%o\" 9}}", &data).expect("must render");
    assert_eq!(out, "11");
    let out = render_template_native("{{printf \"%b\" 9}}", &data).expect("must render");
    assert_eq!(out, "1001");
    let out = render_template_native("{{printf \"%g\" 3.5}}", &data).expect("must render");
    assert_eq!(out, "3.5");
    let out = render_template_native("{{printf \"%G\" 1234567.0}}", &data).expect("must render");
    assert_eq!(out, "1.234567E+06");
    let out = render_template_native("{{printf \"%T\" 0xef}}", &data).expect("must render");
    assert_eq!(out, "int");
    let out = render_template_native("{{printf \"%04x\" -1}}", &data).expect("must render");
    assert_eq!(out, "-001");
    let out = render_template_native("{{3 | printf \"%d\"}}", &data).expect("must render");
    assert_eq!(out, "3");
    let out = render_template_native("{{len .items}}", &data).expect("must render");
    assert_eq!(out, "2");
    let out = render_template_native("{{index .items 1}}", &data).expect("must render");
    assert_eq!(out, "y");
    let out = render_template_native("{{index .m \"k\"}}", &data).expect("must render");
    assert_eq!(out, "v");
    let out = render_template_native("{{or .missing \"x\"}}", &data).expect("must render");
    assert_eq!(out, "x");
    let out = render_template_native("{{and .missing \"x\"}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
    let out = render_template_native("{{slice .items 1}}", &data).expect("must render");
    assert_eq!(out, "[y]");
    let out = render_template_native("{{slice \"abcd\" 1 3}}", &data).expect("must render");
    assert_eq!(out, "bc");
    let out = render_template_native("{{urlquery \"a b\" \"+\"}}", &data).expect("must render");
    assert_eq!(out, "a+b%2B");
    let out =
        render_template_native("{{urlquery (slice \"日本\" 1 2)}}", &data).expect("must render");
    assert_eq!(out, "%97");
    let out = render_template_native("{{urlquery .missing}}", &data).expect("must render");
    assert_eq!(out, "%3Cno+value%3E");
    let out = render_template_native("{{html \"<x&'\\\"\\u0000>\"}}", &data).expect("must render");
    assert_eq!(out, "&lt;x&amp;&#39;&#34;\u{FFFD}&gt;");
    let out = render_template_native("{{js \"<x&'\\\"=\\n>\"}}", &data).expect("must render");
    assert_eq!(out, "\\u003Cx\\u0026\\'\\\"\\u003D\\u000A\\u003E");
}

#[test]
fn native_renderer_keeps_go_type_strictness_for_numeric_ops() {
    let data = json!({"items":["x","y"], "m":{"1":"v"}});
    let out = render_template_native("{{printf \"%d\" \"7\"}}", &data).expect("must render");
    assert_eq!(out, "%!d(string=7)");
    let err = render_template_native("{{index .items \"1\"}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("cannot index slice/array with type string"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    let err = render_template_native("{{index .m 1}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("value has type int; should be string"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_builtins_support_typed_go_bytes() {
    let mut data = serde_json::Map::new();
    data.insert(
        "b".to_string(),
        crate::gotemplates::encode_go_bytes_value(b"abc"),
    );
    let root = Value::Object(data);

    let out = render_template_native("{{len .b}}", &root).expect("must render");
    assert_eq!(out, "3");

    let out = render_template_native("{{index .b 1}}", &root).expect("must render");
    assert_eq!(out, "98");

    let out =
        render_template_native("{{printf \"%s\" (slice .b 1 3)}}", &root).expect("must render");
    assert_eq!(out, "bc");
}

#[test]
fn native_renderer_treats_typed_go_bytes_as_slice_in_if_and_range() {
    let mut non_empty = serde_json::Map::new();
    non_empty.insert(
        "b".to_string(),
        crate::gotemplates::encode_go_bytes_value(b"ab"),
    );
    let non_empty = Value::Object(non_empty);
    let out =
        render_template_native("{{if .b}}yes{{else}}no{{end}}", &non_empty).expect("must render");
    assert_eq!(out, "yes");
    let out = render_template_native("{{range $i, $v := .b}}{{$i}}:{{$v}};{{end}}", &non_empty)
        .expect("must render");
    assert_eq!(out, "0:97;1:98;");

    let mut empty = serde_json::Map::new();
    empty.insert(
        "b".to_string(),
        crate::gotemplates::encode_go_bytes_value(b""),
    );
    let empty = Value::Object(empty);
    let out = render_template_native("{{if .b}}yes{{else}}no{{end}}", &empty).expect("must render");
    assert_eq!(out, "no");
}

#[test]
fn native_renderer_go_zero_mode_returns_typed_map_zero_values() {
    let mut int_entries = serde_json::Map::new();
    int_entries.insert("a".to_string(), Value::Number(Number::from(1)));

    let mut root = serde_json::Map::new();
    root.insert(
        "m".to_string(),
        crate::gotemplates::encode_go_typed_map_value("int", Some(int_entries)),
    );
    let root = Value::Object(root);

    let out = render_template_native_with_options(
        "{{.m.missing}}|{{printf \"%T\" .m.missing}}",
        &root,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoZero,
        },
    )
    .expect("must render");
    assert_eq!(out, "0|int");

    let out = render_template_native(
        "{{index .m \"missing\"}}|{{printf \"%T\" (index .m \"missing\")}}",
        &root,
    )
    .expect("must render");
    assert_eq!(out, "0|int");
}

#[test]
fn native_renderer_handles_nested_typed_map_missing_like_go() {
    let mut inner = serde_json::Map::new();
    inner.insert("y".to_string(), Value::Number(Number::from(2)));
    let mut outer = serde_json::Map::new();
    outer.insert(
        "x".to_string(),
        crate::gotemplates::encode_go_typed_map_value("int", Some(inner)),
    );
    let mut root = serde_json::Map::new();
    root.insert(
        "m".to_string(),
        crate::gotemplates::encode_go_typed_map_value("map[string]int", Some(outer)),
    );
    root.insert(
        "nilMap".to_string(),
        crate::gotemplates::encode_go_typed_map_value("int", None),
    );
    let root = Value::Object(root);

    let out = render_template_native_with_options(
        "{{.m.missing.y}}|{{index .m \"missing\"}}|{{printf \"%T\" (index .m \"missing\")}}",
        &root,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoZero,
        },
    )
    .expect("must render");
    assert_eq!(out, "0|map[]|map[string]int");

    let out = render_template_native_with_options(
        "{{.m.missing.y}}",
        &root,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoDefault,
        },
    )
    .expect("must render");
    assert_eq!(out, "<no value>");

    let out = render_template_native("{{len .nilMap}}", &root).expect("must render");
    assert_eq!(out, "0");
    let out = render_template_native("{{range .nilMap}}x{{else}}empty{{end}}", &root)
        .expect("must render");
    assert_eq!(out, "empty");
}

#[test]
fn native_renderer_typed_map_interface_zero_mode_matches_go() {
    let mut nested = serde_json::Map::new();
    nested.insert("y".to_string(), Value::Number(Number::from(2)));
    let mut entries = serde_json::Map::new();
    entries.insert("x".to_string(), Value::Object(nested));
    let mut root = serde_json::Map::new();
    root.insert(
        "m".to_string(),
        crate::gotemplates::encode_go_typed_map_value("interface {}", Some(entries)),
    );
    let root = Value::Object(root);

    let err = render_template_native_with_options(
        "{{.m.missing.y}}",
        &root,
        NativeRenderOptions {
            missing_value_mode: MissingValueMode::GoZero,
        },
    )
    .expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("nil pointer evaluating interface {}.y"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let out = render_template_native(
        "{{index .m \"missing\"}}|{{printf \"%T\" (index .m \"missing\")}}",
        &root,
    )
    .expect("must render");
    assert_eq!(out, "<no value>|<nil>");
}

#[test]
fn native_renderer_builtin_errors_follow_go_text_template_style() {
    let data = json!({"items":["x"], "s":"abc"});

    let err = render_template_native("{{len 3}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling len: len of type int"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{index 1 0}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling index: can't index item of type int"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let data = json!({"m": {"k": "v"}});
    let err = render_template_native("{{index .m nil}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling index: value is nil; should be string"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let data = json!({"items":["x"], "u": u64::MAX});
    let err = render_template_native("{{index .items .u}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling index: index out of range: -1"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{slice 1 0}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling slice: can't slice item of type int"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{range true}}x{{end}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("range can't iterate over true"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{range 1.5}}x{{end}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("range can't iterate over 1.5"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{lt true false}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling lt: invalid type for comparison"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{lt true 1}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling lt: incompatible types for comparison"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_reports_cannot_evaluate_field_on_slice_like_go() {
    let data = json!({"arr":[1,2]});
    let err = render_template_native("{{.arr.x}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("can't evaluate field x in type []interface {}"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_slice_preserves_nil_for_typed_go_bytes() {
    let mut root = serde_json::Map::new();
    root.insert(
        "b".to_string(),
        crate::gotemplates::encode_go_nil_bytes_value(),
    );
    let root = Value::Object(root);

    let out = render_template_native(r#"{{printf "%#v" (slice .b)}}"#, &root).expect("must render");
    assert_eq!(out, "[]byte(nil)");
}

#[test]
fn native_renderer_field_on_nil_interface_returns_no_value_like_go() {
    let data = Value::Null;
    let out = render_template_native("{{.foo}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
    let out = render_template_native("{{(.).foo}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
}

#[test]
fn native_renderer_rejects_nil_as_command_like_go() {
    let data = json!({});
    for src in [
        "{{nil}}",
        "{{if nil}}T{{end}}",
        "{{with nil}}T{{end}}",
        "{{range nil}}T{{end}}",
        "{{(nil)}}",
        "{{print (nil)}}",
    ] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(
                    reason.contains("nil is not a command"),
                    "src={src} reason={reason}"
                );
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_reports_non_function_commands_like_go() {
    let data = json!({});
    for (src, want) in [
        ("{{nil 1}}", "can't give argument to non-function nil"),
        ("{{\"x\" 1}}", "can't give argument to non-function \"x\""),
        ("{{(1) 2}}", "can't give argument to non-function 1"),
        (
            "{{(printf) 2}}",
            "can't give argument to non-function printf",
        ),
        (
            "{{$x := 1}}{{1 | $x}}",
            "can't give argument to non-function $x",
        ),
        ("{{1 | (nil)}}", "can't give argument to non-function nil"),
        (
            "{{1 | (\"x\")}}",
            "can't give argument to non-function \"x\"",
        ),
        (
            "{{1 | (printf)}}",
            "can't give argument to non-function printf",
        ),
    ] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains(want), "src={src} reason={reason}");
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_reports_unknown_identifier_in_pipeline_as_undefined_function() {
    let data = json!({});
    let err = render_template_native("{{1 | unknownFn}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("\"unknownFn\" is not a defined function"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_preserves_non_executable_pipeline_stage_errors_like_go_parse() {
    let data = json!({});
    for src in ["{{1 | nil}}", "{{1 | \"x\"}}", "{{1 | .}}", "{{1 | true}}"] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::Parse(parse) => {
                assert!(
                    parse.message.contains("non executable command in pipeline stage 2"),
                    "src={src} parse={parse:?}"
                );
            }
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(
                    reason.contains("non executable command in pipeline stage 2"),
                    "src={src} reason={reason}"
                );
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_reports_field_invocation_argument_errors_like_go() {
    let data = json!({"x": 7, "m": {"a": 1}, "a": {}});
    for (src, want) in [
        ("{{.x 2}}", "x is not a method but has arguments"),
        (
            "{{$m := .m}}{{$m.a 2}}",
            "a is not a method but has arguments",
        ),
        ("{{1 | .x}}", "x is not a method but has arguments"),
        (
            "{{.a.b 2}}",
            "b is not a method but has arguments",
        ),
        (
            "{{$m := .m}}{{1 | $m.a}}",
            "a is not a method but has arguments",
        ),
    ] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains(want), "src={src} reason={reason}");
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_allows_field_with_args_when_dot_is_nil_like_go() {
    let data = Value::Null;
    let out = render_template_native("{{.x 2}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
    let out = render_template_native("{{1 | .x}}", &data).expect("must render");
    assert_eq!(out, "<no value>");

    let data = json!({});
    let out = render_template_native("{{.x 2}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
    let out = render_template_native("{{.a.b 2}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
}

#[test]
fn native_renderer_field_invocation_on_slices_keeps_field_type_errors() {
    let data = json!({"arr": [1, 2, 3]});
    let err = render_template_native("{{.arr.x 2}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("can't evaluate field x in type []interface {}"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let mut root = serde_json::Map::new();
    root.insert(
        "arr".to_string(),
        crate::gotemplates::encode_go_typed_slice_value(
            "int",
            Some(vec![
                Value::Number(Number::from(1)),
                Value::Number(Number::from(2)),
            ]),
        ),
    );
    let data = Value::Object(root);
    let err = render_template_native("{{.arr.x 2}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("can't evaluate field x in type []int"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let data = json!({});
    let err = render_template_native("{{$x := 1}}{{$x.y 2}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("can't evaluate field y in type int"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_eq_reports_non_comparable_like_go_text_template() {
    let mut m = serde_json::Map::new();
    m.insert(
        "arr".to_string(),
        Value::Array(vec![
            Value::Number(Number::from(1)),
            Value::Number(Number::from(2)),
        ]),
    );
    let mut obj = serde_json::Map::new();
    obj.insert("a".to_string(), Value::Number(Number::from(1)));
    m.insert("mapv".to_string(), Value::Object(obj));
    m.insert(
        "bytes".to_string(),
        crate::gotemplates::encode_go_bytes_value(b"ab"),
    );
    let data = Value::Object(m);

    let err = render_template_native("{{eq .arr .arr}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling eq: non-comparable type"));
            assert!(reason.contains("[]interface {}"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{eq .arr .mapv}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling eq: non-comparable types"));
            assert!(reason.contains("[]interface {}"));
            assert!(reason.contains("map[string]interface {}"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{eq .bytes .arr}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling eq: non-comparable type"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_compares_string_bytes_like_go_strings() {
    let data = json!({"m":{"a":"ok"}});

    let out = render_template_native("{{eq (slice \"日本\" 1 2) (slice \"日本\" 1 2)}}", &data)
        .expect("must render");
    assert_eq!(out, "true");

    let out = render_template_native("{{ne (slice \"日本\" 1 2) (slice \"日本\" 1 2)}}", &data)
        .expect("must render");
    assert_eq!(out, "false");

    let out = render_template_native("{{lt (slice \"ab\" 0 1) (slice \"ab\" 1 2)}}", &data)
        .expect("must render");
    assert_eq!(out, "true");

    let err =
        render_template_native("{{eq (slice \"日本\" 1 2) .m}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling eq: incompatible types for comparison"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_allows_string_bytes_as_map_index_key() {
    let data = json!({"m":{"a":"ok"}, "m2":{"�":"hit"}});

    let out =
        render_template_native("{{index .m (slice \"ab\" 0 1)}}", &data).expect("must render");
    assert_eq!(out, "ok");

    let out =
        render_template_native("{{index .m (slice \"日本\" 1 2)}}", &data).expect("must render");
    assert_eq!(out, "<no value>");

    let out =
        render_template_native("{{index .m2 (slice \"日本\" 1 2)}}", &data).expect("must render");
    assert_eq!(out, "<no value>");
}

#[test]
fn native_renderer_matches_builtin_arity_and_index_identity() {
    let data = json!({"x": 1});
    let out = render_template_native("{{index 1}}", &data).expect("must render");
    assert_eq!(out, "1");

    assert!(render_template_native("{{and}}", &data).is_err());
    assert!(render_template_native("{{or}}", &data).is_err());
    assert!(render_template_native("{{not}}", &data).is_err());
    assert!(render_template_native("{{not 1 2}}", &data).is_err());
    assert!(render_template_native("{{eq}}", &data).is_err());
    assert!(render_template_native("{{eq 1}}", &data).is_err());
    assert!(render_template_native("{{ne 1 2 3}}", &data).is_err());
    assert!(render_template_native("{{lt 1}}", &data).is_err());
    assert!(render_template_native("{{len}}", &data).is_err());
    assert!(render_template_native("{{slice}}", &data).is_err());
    assert!(render_template_native("{{printf}}", &data).is_err());
}

#[test]
fn native_renderer_supports_variable_declare_and_assign() {
    let data = json!({"v":"rootv"});
    let out =
        render_template_native("{{$x := .v}}{{$x = \"b\"}}{{$x}}", &data).expect("must render");
    assert_eq!(out, "b");
}

#[test]
fn native_renderer_supports_variable_declarations_without_spaces() {
    let data = json!({"v":"rootv"});
    let out = render_template_native("{{$x:=.v}}{{$x=\"b\"}}{{$x}}", &data).expect("must render");
    assert_eq!(out, "b");
}

#[test]
fn native_renderer_supports_digit_started_variable_names_like_go() {
    let data = json!({});
    let out = render_template_native("{{ $1 := 7 }}{{ $1 }}", &data).expect("must render");
    assert_eq!(out, "7");
}

#[test]
fn native_renderer_supports_range_variable_declarations() {
    let data = json!({"items":["a","b"]});
    let out = render_template_native("{{range $i, $v := .items}}{{$i}}={{$v}};{{end}}", &data)
        .expect("must render");
    assert_eq!(out, "0=a;1=b;");
}

#[test]
fn native_renderer_supports_range_variable_declarations_without_spaces() {
    let data = json!({"items":["a","b"]});
    let out =
        render_template_native("{{range $i,$v:=.items}}{{$i}}={{$v}};{{end}}", &data)
            .expect("must render");
    assert_eq!(out, "0=a;1=b;");
}

#[test]
fn native_renderer_rejects_range_over_integer_like_go() {
    let data = json!({});
    let err = render_template_native("{{range 3}}{{.}}{{end}}", &data).expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("range can't iterate over 3"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let err = render_template_native("{{range $i, $v := 3}}{{$i}}={{$v}};{{end}}", &data)
        .expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("range can't iterate over 3"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_supports_range_break_and_continue() {
    let data = json!({"items":[0,1,2,3]});
    let out = render_template_native(
        "{{range .items}}{{if eq . 2}}{{break}}{{end}}{{.}}{{end}}",
        &data,
    )
    .expect("must render");
    assert_eq!(out, "01");
    let out = render_template_native(
        "{{range .items}}{{if eq . 2}}{{continue}}{{end}}{{.}}{{end}}",
        &data,
    )
    .expect("must render");
    assert_eq!(out, "013");
    assert!(render_template_native("{{break}}", &data).is_err());
    assert!(render_template_native("{{continue}}", &data).is_err());
}

#[test]
fn native_renderer_slice_string_keeps_byte_semantics_for_printf() {
    let data = json!({});
    let out = render_template_native("{{printf \"%x\" (slice \"日本\" 1 2)}}", &data)
        .expect("must render");
    assert_eq!(out, "97");

    let out = render_template_native("{{printf \"%q\" (slice \"日本\" 1 2)}}", &data)
        .expect("must render");
    assert_eq!(out, "\"\\x97\"");
}

#[test]
fn native_renderer_preserves_unicode_literals_in_function_args() {
    let data = json!({});
    let out = render_template_native("{{printf \"%s\" \"日本語\"}}", &data).expect("must render");
    assert_eq!(out, "日本語");
}

#[test]
fn native_renderer_range_else_exposes_declared_variable() {
    let data = json!({"empty":[]});
    let out = render_template_native("{{range $v := .empty}}x{{else}}{{$v}}{{end}}", &data)
        .expect("must render");
    assert_eq!(out, "[]");
}

#[test]
fn native_renderer_template_call_resets_root_context() {
    let data = json!({"v":"rootv","user":{"v":"userv"}});
    let out = render_template_native(
        "{{define \"t\"}}{{$.v}}{{end}}{{template \"t\" .user}}",
        &data,
    )
    .expect("must render");
    assert_eq!(out, "userv");
}

#[test]
fn native_renderer_supports_block_action() {
    let data = json!({"user":{"name":"alice"}});
    let out = render_template_native("{{block \"b\" .user}}{{.name}}{{end}}", &data)
        .expect("must render");
    assert_eq!(out, "alice");
}

#[test]
fn native_renderer_and_or_short_circuit_matches_go() {
    let data = json!({});
    let out = render_template_native("{{or 0 1 (index nil 0)}}", &data).expect("must render");
    assert_eq!(out, "1");

    let out = render_template_native("{{and 1 0 (index nil 0)}}", &data).expect("must render");
    assert_eq!(out, "0");

    assert!(render_template_native("{{or 0 0 (index nil 0)}}", &data).is_err());
    assert!(render_template_native("{{and 1 1 (index nil 0)}}", &data).is_err());
}

#[test]
fn native_renderer_and_or_pipeline_argument_order_matches_go() {
    let data = json!({});
    for src in [
        "{{0 | and (index nil 0)}}",
        "{{0 | or (index nil 0)}}",
        "{{1 | or (index nil 0)}}",
    ] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(
                    reason.contains("error calling index: index of untyped nil"),
                    "src={src} reason={reason}"
                );
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_supports_external_function_resolver_with_args() {
    let data = json!({});
    let out = render_template_native_with_resolver(
        "{{ext \"a\" 2}}",
        &data,
        NativeRenderOptions::default(),
        Some(&|name: &str, args: &[Option<Value>]| {
            if name != "ext" {
                return Err(NativeFunctionResolverError::UnknownFunction);
            }
            assert_eq!(args.len(), 2);
            Ok(Some(Value::String(format!(
                "{}:{}",
                format_value_for_print(&args[0]),
                format_value_for_print(&args[1])
            ))))
        }),
    )
    .expect("must render");
    assert_eq!(out, "a:2");
}

#[test]
fn native_renderer_supports_call_builtin_via_resolver() {
    let data = json!({"fn":"ext"});
    let resolver = |name: &str, args: &[Option<Value>]| {
        if name != "ext" {
            return Err(NativeFunctionResolverError::UnknownFunction);
        }
        Ok(Some(Value::String(format!(
            "called:{}",
            format_value_for_print(&args[0])
        ))))
    };
    let out = render_template_native_with_resolver(
        "{{call ext \"x\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "called:x");
    let out = render_template_native_with_resolver(
        "{{call .fn \"y\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "called:y");
    let out = render_template_native_with_resolver(
        "{{.fn \"z\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "called:z");

    let err = render_template_native_with_resolver(
        "{{call nope \"x\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("\"nope\" is not a defined function"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_call_builtin_reports_go_like_non_function_errors() {
    let data = json!({"fn":"ext"});
    for (src, want) in [
        ("{{call}}", "wrong number of args for call: want at least 1 got 0"),
        ("{{call ext}}", "\"ext\" is not a defined function"),
        ("{{call nil}}", "error calling call: call of nil"),
        ("{{call (nil)}}", "nil is not a command"),
        ("{{call (printf)}}", "wrong number of args for printf"),
        (
            "{{call .fn}}",
            "error calling call: non-function .fn of type string",
        ),
        (
            "{{1 | call .fn}}",
            "error calling call: non-function .fn of type string",
        ),
        (
            "{{call (.fn)}}",
            "error calling call: non-function .fn of type string",
        ),
        (
            "{{call ((.fn))}}",
            "error calling call: non-function (.fn) of type string",
        ),
        ("{{call .missing}}", "error calling call: call of nil"),
        ("{{1 | call .missing}}", "error calling call: call of nil"),
        ("{{call (.missing)}}", "error calling call: call of nil"),
        (
            "{{call \"x\"}}",
            "error calling call: non-function \"x\" of type string",
        ),
        (
            "{{call (\"x\")}}",
            "error calling call: non-function \"x\" of type string",
        ),
        ("{{call 1}}", "error calling call: non-function 1 of type int"),
        ("{{call (1)}}", "error calling call: non-function 1 of type int"),
    ] {
        let err = render_template_native(src, &data).expect_err("must fail");
        match err {
            NativeRenderError::UnsupportedAction { reason, .. } => {
                assert!(reason.contains(want), "src={src} reason={reason}");
            }
            other => panic!("unexpected error for {src}: {other:?}"),
        }
    }
}

#[test]
fn native_renderer_supports_unicode_identifiers_in_resolver_and_paths() {
    let data = json!({"fn":"привет","данные":{"ключ":"значение"}});
    let resolver = |name: &str, args: &[Option<Value>]| {
        if name != "привет" {
            return Err(NativeFunctionResolverError::UnknownFunction);
        }
        Ok(Some(Value::String(format!(
            "ok:{}",
            format_value_for_print(&args[0])
        ))))
    };

    let out = render_template_native_with_resolver(
        "{{привет \"мир\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "ok:мир");

    let out = render_template_native_with_resolver(
        "{{call .fn \"x\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "ok:x");

    let out = render_template_native_with_resolver(
        "{{.fn \"y\"}}",
        &data,
        NativeRenderOptions::default(),
        Some(&resolver),
    )
    .expect("must render");
    assert_eq!(out, "ok:y");

    let out =
        render_template_native("{{$имя := .данные.ключ}}{{$имя}}", &data).expect("must render");
    assert_eq!(out, "значение");
}

#[test]
fn native_renderer_supports_external_niladic_function() {
    let data = json!({"ext":"value-from-data"});
    let out = render_template_native_with_resolver(
        "{{ext}}",
        &data,
        NativeRenderOptions::default(),
        Some(&|name: &str, _args: &[Option<Value>]| {
            if name == "ext" {
                Ok(Some(Value::String("value-from-resolver".to_string())))
            } else {
                Err(NativeFunctionResolverError::UnknownFunction)
            }
        }),
    )
    .expect("must render");
    assert_eq!(out, "value-from-resolver");
}

#[test]
fn native_renderer_external_resolver_can_return_typed_go_bytes() {
    let data = json!({});
    let out = render_template_native_with_resolver(
        "{{printf \"%s\" (ext)}}",
        &data,
        NativeRenderOptions::default(),
        Some(&|name: &str, _args: &[Option<Value>]| {
            if name == "ext" {
                Ok(Some(crate::gotemplates::encode_go_bytes_value(b"ab")))
            } else {
                Err(NativeFunctionResolverError::UnknownFunction)
            }
        }),
    )
    .expect("must render");
    assert_eq!(out, "ab");
}

#[test]
fn native_renderer_reports_external_function_error() {
    let data = json!({});
    let err = render_template_native_with_resolver(
        "{{ext 1}}",
        &data,
        NativeRenderOptions::default(),
        Some(&|name: &str, _args: &[Option<Value>]| {
            if name == "ext" {
                Err(NativeFunctionResolverError::Failed {
                    reason: "boom".to_string(),
                })
            } else {
                Err(NativeFunctionResolverError::UnknownFunction)
            }
        }),
    )
    .expect_err("must fail");
    match err {
        NativeRenderError::UnsupportedAction { reason, .. } => {
            assert!(reason.contains("error calling ext: boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn native_renderer_parses_go_char_literal_escapes() {
    let data = json!({});
    let out = render_template_native("{{print '\\n'}}", &data).expect("must render");
    assert_eq!(out, "10");
    let out = render_template_native("{{print '\\x41'}}", &data).expect("must render");
    assert_eq!(out, "65");
    let out = render_template_native("{{print '\\u263A'}}", &data).expect("must render");
    assert_eq!(out, "9786");
    let out = render_template_native("{{print '\\U0001F600'}}", &data).expect("must render");
    assert_eq!(out, "128512");
    assert!(render_template_native("{{print '\\400'}}", &data).is_err());
}

#[test]
fn native_renderer_validates_go_number_underscore_syntax() {
    let data = json!({});
    let out = render_template_native("{{print 0x_10}}", &data).expect("must render");
    assert_eq!(out, "16");
    assert!(render_template_native("{{print 1__2}}", &data).is_err());
    assert!(render_template_native("{{print 12_}}", &data).is_err());
}

#[test]
fn native_renderer_reports_undefined_variable_from_outer_scope_in_define() {
    let data = json!({"v":"rootv"});
    let err = render_template_native(
        "{{$x := \"outer\"}}{{define \"t\"}}{{$x}}{{end}}{{template \"t\" .}}",
        &data,
    )
    .expect_err("must fail");
    match err {
        NativeRenderError::Parse(parse) => {
            assert_eq!(parse.code, "undefined_variable");
            assert!(parse.message.contains("undefined variable \"$x\""));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
