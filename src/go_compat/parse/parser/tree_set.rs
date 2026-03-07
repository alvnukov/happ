use std::collections::BTreeMap;

use crate::go_compat::compat::parse_go_quoted_prefix;
use crate::go_compat::parse::{
    parse_action_report_with_options, ControlAction, ControlKind, ParseCompatOptions,
};
use crate::go_compat::scan::{
    GoTemplateToken,
};

use super::super::tree::ParseError;
use super::action_inner_trimmed;

pub(super) fn split_template_set(
    tokens: &[GoTemplateToken],
) -> Result<(Vec<GoTemplateToken>, BTreeMap<String, Vec<GoTemplateToken>>), ParseError> {
    extract_tree_set_tokens(tokens)
}

fn extract_tree_set_tokens(
    tokens: &[GoTemplateToken],
) -> Result<(Vec<GoTemplateToken>, BTreeMap<String, Vec<GoTemplateToken>>), ParseError> {
    let mut main = Vec::with_capacity(tokens.len());
    let mut defs: BTreeMap<String, Vec<GoTemplateToken>> = BTreeMap::new();
    let mut idx = 0usize;

    while idx < tokens.len() {
        match &tokens[idx] {
            GoTemplateToken::Literal(_) => {
                main.push(tokens[idx].clone());
                idx += 1;
            }
            GoTemplateToken::Action(action) => {
                let report = parse_action_report_with_options(
                    action,
                    0,
                    ParseCompatOptions {
                        skip_func_check: true,
                        known_functions: &[],
                        check_variables: false,
                        visible_variables: &[],
                    },
                )
                .map_err(ParseError::from_scan)?;
                match (report.control, report.define_name) {
                    (ControlAction::Open(ControlKind::Define), Some(name)) => {
                        let end_idx = find_matching_end(tokens, idx + 1)?;
                        let raw_body = &tokens[idx + 1..end_idx.saturating_sub(1)];
                        let (body, nested_defs) = extract_tree_set_tokens(raw_body)?;
                        defs.extend(nested_defs);
                        defs.insert(name, body);
                        idx = end_idx;
                    }
                    (ControlAction::Open(ControlKind::Block), _) => {
                        let end_idx = find_matching_end(tokens, idx + 1)?;
                        let raw_body = &tokens[idx + 1..end_idx.saturating_sub(1)];
                        let (body, nested_defs) = extract_tree_set_tokens(raw_body)?;
                        defs.extend(nested_defs);

                        if let Some((block_name, pipeline_tail)) = parse_block_name_and_tail(action)
                        {
                            defs.insert(block_name.clone(), body);
                            main.push(GoTemplateToken::Action(render_template_action(
                                &block_name,
                                &pipeline_tail,
                            )));
                        } else {
                            main.push(tokens[idx].clone());
                        }
                        idx = end_idx;
                    }
                    _ => {
                        main.push(tokens[idx].clone());
                        idx += 1;
                    }
                }
            }
        }
    }

    Ok((main, defs))
}

fn parse_block_name_and_tail(action: &str) -> Option<(String, String)> {
    let inner = action_inner_trimmed(action)?;
    let rest = inner.strip_prefix("block")?.trim_start();
    let (name, tail) = parse_go_quoted_prefix(rest)?;
    Some((name, tail.to_string()))
}

fn render_template_action(name: &str, pipeline_tail: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{{{{template \"{}\"{}}}}}", escaped, pipeline_tail)
}

fn find_matching_end(tokens: &[GoTemplateToken], mut idx: usize) -> Result<usize, ParseError> {
    let mut depth = 1usize;
    while idx < tokens.len() {
        if let GoTemplateToken::Action(action) = &tokens[idx] {
            let report = parse_action_report_with_options(
                action,
                0,
                ParseCompatOptions {
                    skip_func_check: true,
                    known_functions: &[],
                    check_variables: false,
                    visible_variables: &[],
                },
            )
            .map_err(ParseError::from_scan)?;
            match report.control {
                ControlAction::Open(_) => depth += 1,
                ControlAction::End => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(idx + 1);
                    }
                }
                ControlAction::None
                | ControlAction::Else(_)
                | ControlAction::Break
                | ControlAction::Continue => {}
            }
        }
        idx += 1;
    }

    Err(ParseError::unexpected_eof_for_define())
}
