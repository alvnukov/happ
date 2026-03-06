use super::path::is_identifier_continue_char;
use super::tokenize::split_pipeline_commands;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PipelineDeclMode {
    Declare,
    Assign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PipelineDeclaration {
    pub(super) names: Vec<String>,
    pub(super) mode: PipelineDeclMode,
}

pub(super) fn extract_pipeline_declaration(expr: &str) -> (Option<PipelineDeclaration>, String) {
    let commands = split_pipeline_commands(expr);
    if commands.is_empty() {
        return (None, expr.trim().to_string());
    }
    let Some((decl, runtime_start)) = parse_pipeline_decl_prefix(&commands[0]) else {
        return (None, expr.trim().to_string());
    };

    let mut rebuilt = Vec::new();
    let first_runtime = commands[0][runtime_start..].trim();
    if !first_runtime.is_empty() {
        rebuilt.push(first_runtime.to_string());
    }
    for cmd in commands.iter().skip(1) {
        rebuilt.push(cmd.clone());
    }
    (Some(decl), rebuilt.join(" | "))
}

fn parse_pipeline_decl_prefix(command: &str) -> Option<(PipelineDeclaration, usize)> {
    let mut i = skip_ascii_ws(command, 0);
    let (first, next) = parse_variable_token_at(command, i)?;
    i = skip_ascii_ws(command, next);

    let mut names = vec![first.to_string()];
    if starts_with_at(command, i, ",") {
        i += 1;
        i = skip_ascii_ws(command, i);
        let (second, next_second) = parse_variable_token_at(command, i)?;
        names.push(second.to_string());
        i = skip_ascii_ws(command, next_second);
    }

    let (mode, op_len) = if starts_with_at(command, i, ":=") {
        (PipelineDeclMode::Declare, 2)
    } else if starts_with_at(command, i, "=") {
        (PipelineDeclMode::Assign, 1)
    } else {
        return None;
    };
    i += op_len;

    Some((PipelineDeclaration { names, mode }, i))
}

fn parse_variable_token_at(src: &str, start: usize) -> Option<(&str, usize)> {
    if !starts_with_at(src, start, "$") {
        return None;
    }

    let after_dollar = start + 1;
    let mut end = after_dollar;
    for (offset, ch) in src[after_dollar..].char_indices() {
        if !is_identifier_continue_char(ch) {
            break;
        }
        end = after_dollar + offset + ch.len_utf8();
    }
    if end == after_dollar {
        return None;
    }

    Some((&src[start..end], end))
}

fn starts_with_at(src: &str, start: usize, needle: &str) -> bool {
    src.get(start..).is_some_and(|tail| tail.starts_with(needle))
}

fn skip_ascii_ws(src: &str, mut i: usize) -> usize {
    while let Some(b) = src.as_bytes().get(i) {
        if !b.is_ascii_whitespace() {
            break;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::{extract_pipeline_declaration, PipelineDeclMode};

    #[test]
    fn extracts_single_var_declaration() {
        let (decl, runtime) = extract_pipeline_declaration("$x := printf \"%s\" .v | quote");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$x".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Declare);
        assert_eq!(runtime, "printf \"%s\" .v | quote");
    }

    #[test]
    fn extracts_range_style_two_var_assignment() {
        let (decl, runtime) = extract_pipeline_declaration("$i, $v = range .items");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$i".to_string(), "$v".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Assign);
        assert_eq!(runtime, "range .items");
    }

    #[test]
    fn extracts_declarations_without_spaces_around_operators() {
        let (decl, runtime) = extract_pipeline_declaration("$x:=printf \"%s\" .v|quote");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$x".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Declare);
        assert_eq!(runtime, "printf \"%s\" .v | quote");

        let (decl, runtime) = extract_pipeline_declaration("$i,$v=range .items");
        let decl = decl.expect("declaration must exist");
        assert_eq!(decl.names, vec!["$i".to_string(), "$v".to_string()]);
        assert_eq!(decl.mode, PipelineDeclMode::Assign);
        assert_eq!(runtime, "range .items");
    }

    #[test]
    fn keeps_expression_without_declaration() {
        let (decl, runtime) = extract_pipeline_declaration("printf \"%s\" .v");
        assert!(decl.is_none());
        assert_eq!(runtime, "printf \"%s\" .v");
    }
}
