use crate::go_compat::tokenize::{
    split_command_tokens_exec, split_pipeline_commands_owned,
    strip_outer_parens as go_strip_outer_parens,
};

pub(super) fn strip_outer_parens(s: &str) -> Option<&str> {
    go_strip_outer_parens(s)
}

pub(super) fn split_pipeline_commands(inner: &str) -> Vec<String> {
    split_pipeline_commands_owned(inner)
}

pub(super) fn split_command_tokens(command: &str) -> Vec<String> {
    split_command_tokens_exec(command)
}

#[cfg(test)]
mod tests {
    use super::{split_command_tokens, split_pipeline_commands, strip_outer_parens};

    #[test]
    fn strip_outer_parens_only_unwraps_full_expression() {
        assert_eq!(strip_outer_parens("(1)"), Some("1"));
        assert_eq!(strip_outer_parens("((a.b))"), Some("(a.b)"));
        assert_eq!(strip_outer_parens("(a) + (b)"), None);
        assert_eq!(strip_outer_parens("(a"), None);
    }

    #[test]
    fn split_pipeline_commands_respects_quotes_and_treats_comments_as_text() {
        let cmds = split_pipeline_commands(r#"print "a|b" | printf "%s" /* x|y */ | quote"#);
        assert_eq!(
            cmds,
            vec!["print \"a|b\"", "printf \"%s\" /* x|y */", "quote"]
        );
    }

    #[test]
    fn split_command_tokens_preserves_unicode_and_commas() {
        let tokens = split_command_tokens(r#"sum (index .m "ключ") , 2 "#);
        assert_eq!(tokens, vec!["sum", "(index .m \"ключ\")", ",", "2"]);
    }
}
