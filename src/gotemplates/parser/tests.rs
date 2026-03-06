use super::*;

#[test]
fn parser_accepts_basic_include_action() {
    let action = parse_action_compat("{{ include \"x\" . }}", 0).expect("must parse");
    assert_eq!(action, ControlAction::None);
}

#[test]
fn parser_reports_nested_left_delim_in_operand() {
    let err = parse_action_compat("{{ include \"a\" {{ .Values.x }} }}", 0).expect_err("must fail");
    assert_eq!(err.code, "unexpected_left_delim_in_operand");
}

#[test]
fn parser_reports_unexpected_dot_in_operand() {
    let err = parse_action_compat("{{ .Values.bad..path }}", 0).expect_err("must fail");
    assert_eq!(err.code, "unexpected_dot_in_operand");
}

#[test]
fn parser_accepts_control_define_end_actions() {
    assert_eq!(
        parse_action_compat("{{ define \"a\" }}", 0).expect("must parse"),
        ControlAction::Open(ControlKind::Define)
    );
    assert_eq!(
        parse_action_compat("{{ end }}", 0).expect("must parse"),
        ControlAction::End
    );
}

#[test]
fn parser_decodes_define_name_like_go_string_literal() {
    let report = parse_action_report_with_options(
        "{{ define \"\\x61\" }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect("must parse");
    assert_eq!(
        report.control,
        ControlAction::Open(ControlKind::Define),
        "define must be recognized as control open"
    );
    assert_eq!(report.define_name.as_deref(), Some("a"));
}

#[test]
fn parser_can_check_function_existence() {
    let err = parse_action_compat_with_options(
        "{{ totallyUnknown . }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "undefined_function");
    assert!(err.message.contains("function \"totallyUnknown\" not defined"));

    let err = parse_action_compat_with_options(
        "{{ 1 | totallyUnknown }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "undefined_function");
    assert!(err.message.contains("function \"totallyUnknown\" not defined"));

    let err = parse_action_compat_with_options(
        "{{ call totallyUnknown }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "undefined_function");
    assert!(err.message.contains("function \"totallyUnknown\" not defined"));

    let err = parse_action_compat_with_options(
        "{{ 1 | call totallyUnknown }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "undefined_function");
    assert!(err.message.contains("function \"totallyUnknown\" not defined"));
}

#[test]
fn parser_allows_declared_function_with_func_check_enabled() {
    let action = parse_action_compat_with_options(
        "{{ customFn . }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &["customFn"],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect("must parse");
    assert_eq!(action, ControlAction::None);
}

#[test]
fn parser_treats_break_as_function_when_declared() {
    let action = parse_action_compat_with_options(
        "{{ break 20 }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &["break"],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect("must parse");
    assert_eq!(action, ControlAction::None);
}

#[test]
fn parser_accepts_unicode_function_and_variable_names() {
    let action = parse_action_compat_with_options(
        "{{ привет . }}",
        0,
        ParseCompatOptions {
            skip_func_check: false,
            known_functions: &["привет"],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect("must parse");
    assert_eq!(action, ControlAction::None);

    let action = parse_action_compat_with_options(
        "{{ $имя }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: true,
            visible_variables: &["$имя"],
        },
    )
    .expect("must parse");
    assert_eq!(action, ControlAction::None);
}

#[test]
fn parser_rejects_multi_decl_outside_range() {
    let err = parse_action_compat_with_options(
        "{{ with $v, $u := 3 }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "too_many_declarations");
}

#[test]
fn parser_reports_undefined_variable_name_like_go() {
    let err = parse_action_compat_with_options(
        "{{ $x }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: true,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "undefined_variable");
    assert!(err.message.contains("undefined variable \"$x\""));
}

#[test]
fn parser_reports_non_executable_pipeline_stage_number_like_go() {
    let err = parse_action_compat_with_options(
        "{{ 1 | nil }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "non_executable_command_in_pipeline");
    assert!(err.message.contains("non executable command in pipeline stage 2"));
    assert_eq!(err.offset, 7);

    let err = parse_action_compat_with_options(
        "{{ 1 | print | nil }}",
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .expect_err("must fail");
    assert_eq!(err.code, "non_executable_command_in_pipeline");
    assert!(err.message.contains("non executable command in pipeline stage 3"));
    assert_eq!(err.offset, 15);
}
