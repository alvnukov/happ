use super::valuefmt::{format_value_like_go, printf_type_name};
use serde_json::Value;

pub(super) fn format_missing_arg(verb: char) -> String {
    format!("%!{verb}(MISSING)")
}

pub(super) fn format_bad_index(verb: char) -> String {
    format!("%!{verb}(BADINDEX)")
}

pub(super) fn format_extra_args(extra: &[Option<Value>]) -> String {
    let mut out = String::from("%!(EXTRA ");
    for (idx, arg) in extra.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(&format_extra_arg(arg));
    }
    out.push(')');
    out
}

fn format_extra_arg(arg: &Option<Value>) -> String {
    match arg {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(v) => format!("{}={}", printf_type_name(v), format_value_like_go(v)),
    }
}

pub(super) fn format_printf_mismatch(verb: char, arg: &Option<Value>) -> String {
    match arg {
        None | Some(Value::Null) => format!("%!{verb}(<nil>)"),
        Some(v) => {
            let type_name = printf_type_name(v);
            let value = format_value_like_go(v);
            format!("%!{verb}({type_name}={value})")
        }
    }
}
