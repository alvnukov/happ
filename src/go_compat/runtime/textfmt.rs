#[cfg(test)]
use crate::go_compat::textfmt::format_value_for_print as go_format_value_for_print;
use crate::go_compat::textfmt::{
    builtin_html as go_builtin_html, builtin_js as go_builtin_js,
    builtin_print as go_builtin_print, builtin_urlquery as go_builtin_urlquery,
};
use serde_json::Value;

pub(super) fn builtin_print(args: &[Option<Value>], with_newline: bool) -> String {
    go_builtin_print(args, with_newline)
}

pub(super) fn builtin_urlquery(args: &[Option<Value>]) -> String {
    go_builtin_urlquery(args)
}

pub(super) fn builtin_html(args: &[Option<Value>]) -> String {
    go_builtin_html(args)
}

pub(super) fn builtin_js(args: &[Option<Value>]) -> String {
    go_builtin_js(args)
}

#[cfg(test)]
pub(super) fn format_value_for_print(v: &Option<Value>) -> String {
    go_format_value_for_print(v.as_ref())
}

#[cfg(test)]
mod tests {
    use super::{builtin_html, builtin_js, builtin_print, builtin_urlquery};
    use serde_json::Value;

    #[test]
    fn builtin_urlquery_keeps_go_no_value_and_byte_semantics() {
        let out = builtin_urlquery(&[None, Some(Value::String("a b".to_string()))]);
        assert_eq!(out, "%3Cno+value%3Ea+b");
    }

    #[test]
    fn builtin_js_escapes_special_ascii_as_go_style() {
        let out = builtin_js(&[Some(Value::String("<x&'\\\"=\\n>".to_string()))]);
        assert_eq!(out, "\\u003Cx\\u0026\\'\\\\\\\"\\u003D\\\\n\\u003E");
    }

    #[test]
    fn builtin_js_escapes_go_non_print_unicode_runes() {
        let out = builtin_js(&[Some(Value::String(
            "\u{00A0}\u{200B}\u{2028}\u{2029}\u{FFFE}".to_string(),
        ))]);
        assert_eq!(out, "\\u00A0\\u200B\\u2028\\u2029\\uFFFE");

        let out = builtin_js(&[Some(Value::String("ĀЖ🙂".to_string()))]);
        assert_eq!(out, "ĀЖ🙂");
    }

    #[test]
    fn builtin_print_uses_nil_placeholder_like_go_fmt_sprint() {
        let out = builtin_print(&[None], false);
        assert_eq!(out, "<nil>");

        let out = builtin_print(&[Some(Value::Null)], false);
        assert_eq!(out, "<nil>");
    }

    #[test]
    fn html_uses_no_value_placeholder_from_eval_args_path() {
        let out = builtin_html(&[None]);
        assert_eq!(out, "&lt;no value&gt;");
    }

    #[test]
    fn builtin_print_matches_go_spacing_rules_for_non_strings() {
        let out = builtin_print(
            &[
                Some(Value::Number(1.into())),
                Some(Value::Number(2.into())),
                Some(Value::String("x".to_string())),
                Some(Value::Number(3.into())),
            ],
            false,
        );
        assert_eq!(out, "1 2x3");
    }
}
