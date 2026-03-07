use crate::go_compat::textfmt::format_value_for_print;
use crate::go_compat::tokenize::strip_outer_parens;
use serde_json::Value;

pub fn call_target_display(token: Option<&str>, value: &Value) -> String {
    if let Some(raw) = token {
        let trimmed = raw.trim();
        if let Some(inner) = strip_outer_parens(trimmed) {
            return inner.trim().to_string();
        }
        return trimmed.to_string();
    }
    format_value_for_print(Some(value))
}

#[cfg(test)]
mod tests {
    use super::call_target_display;
    use serde_json::json;

    #[test]
    fn keeps_trimmed_token_or_unwrapped_parens() {
        assert_eq!(
            call_target_display(Some(" ( .Values.fn ) "), &json!(null)),
            ".Values.fn"
        );
        assert_eq!(call_target_display(Some("  $fn  "), &json!(null)), "$fn");
    }

    #[test]
    fn falls_back_to_value_print_for_missing_token() {
        assert_eq!(call_target_display(None, &json!(1)), "1");
    }
}
