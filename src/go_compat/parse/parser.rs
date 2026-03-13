use crate::go_compat::parse::{
    parse_action_report_with_options, ControlAction, ControlKind, ParseCompatOptions,
};
use crate::go_compat::scan::{
    parse_template_tokens_strict_with_options_and_delims, GoTemplateToken,
};
use std::collections::BTreeMap;

use self::tree_set::split_template_set;
use super::node::{
    ActionNode, BlockNode, BreakNode, CommentNode, ContinueNode, DefineNode, ElseNode, EndNode,
    IfNode, ListNode, Node, RangeNode, TemplateNode, TextNode, WithNode,
};
use super::tree::{Mode, ParseError, Tree};

const LEFT_DELIM: &str = "{{";
const RIGHT_DELIM: &str = "}}";

mod tree_set;

pub fn parse(
    name: &str,
    text: &str,
    left_delim: &str,
    right_delim: &str,
    mode: Mode,
    known_functions: &[&str],
) -> Result<BTreeMap<String, Tree>, ParseError> {
    let left_delim = normalize_delimiter(left_delim, LEFT_DELIM);
    let right_delim = normalize_delimiter(right_delim, RIGHT_DELIM);

    let tokens = parse_template_tokens_strict_with_options_and_delims(
        text,
        left_delim,
        right_delim,
        ParseCompatOptions {
            skip_func_check: mode.contains(Mode::SKIP_FUNC_CHECK),
            known_functions,
            check_variables: true,
            visible_variables: &[],
        },
    )
    .map_err(ParseError::from_scan)?;

    let (main_tokens, defs) = split_template_set(&tokens)?;
    let mut trees = BTreeMap::new();
    trees.insert(
        name.to_string(),
        tree_from_tokens(name, name, mode, text, &main_tokens),
    );

    for (tpl_name, body) in defs {
        trees.insert(
            tpl_name.clone(),
            tree_from_tokens(&tpl_name, name, mode, text, &body),
        );
    }

    Ok(trees)
}

fn normalize_delimiter<'a>(raw: &'a str, default: &'a str) -> &'a str {
    if raw.is_empty() {
        default
    } else {
        raw
    }
}

fn tree_from_tokens(
    name: &str,
    parse_name: &str,
    mode: Mode,
    source_text: &str,
    tokens: &[GoTemplateToken],
) -> Tree {
    let offsets = token_offsets(tokens);
    let (root, _, _) = parse_list(tokens, &offsets, mode, 0, StopMode::TopLevel);
    Tree {
        name: name.to_string(),
        parse_name: parse_name.to_string(),
        root,
        mode,
        text: source_text.to_string(),
    }
}

fn token_offsets(tokens: &[GoTemplateToken]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(tokens.len());
    let mut pos = 0usize;
    for token in tokens {
        offsets.push(pos);
        pos += match token {
            GoTemplateToken::Literal(text) | GoTemplateToken::Action(text) => text.len(),
        };
    }
    offsets
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopMode {
    TopLevel,
    EndOrElse,
    EndOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StopSignal {
    Eof,
    End {
        action: String,
    },
    Else {
        action: String,
        nested: Option<ControlKind>,
        pos: usize,
    },
}

fn parse_list(
    tokens: &[GoTemplateToken],
    offsets: &[usize],
    mode: Mode,
    mut idx: usize,
    stop_mode: StopMode,
) -> (ListNode, usize, StopSignal) {
    let mut nodes = Vec::new();

    while idx < tokens.len() {
        let pos = offsets.get(idx).copied().unwrap_or_default();
        match &tokens[idx] {
            GoTemplateToken::Literal(text) => {
                if text.is_empty() {
                    idx += 1;
                    continue;
                }
                nodes.push(Node::Text(TextNode {
                    pos,
                    text: text.clone(),
                }));
                idx += 1;
            }
            GoTemplateToken::Action(action) => {
                let canonical = canonical_action(action);
                if is_comment_action(action) {
                    if mode.contains(Mode::PARSE_COMMENTS) {
                        nodes.push(Node::Comment(CommentNode {
                            pos,
                            text: canonical.clone(),
                        }));
                    }
                    idx += 1;
                    continue;
                }

                let control = parse_control(action);
                if let Some(control_action) = control {
                    match (stop_mode, control_action) {
                        (StopMode::EndOrElse, ControlAction::End)
                        | (StopMode::EndOnly, ControlAction::End) => {
                            return (
                                ListNode { nodes },
                                idx + 1,
                                StopSignal::End {
                                    action: canonical.clone(),
                                },
                            );
                        }
                        (StopMode::EndOrElse, ControlAction::Else(nested)) => {
                            return (
                                ListNode { nodes },
                                idx + 1,
                                StopSignal::Else {
                                    action: canonical.clone(),
                                    nested,
                                    pos,
                                },
                            );
                        }
                        _ => {}
                    }

                    match control_action {
                        ControlAction::Open(
                            kind @ (ControlKind::If | ControlKind::Range | ControlKind::With),
                        ) => {
                            let (node, next_idx) = parse_branch_node(
                                kind,
                                &canonical,
                                pos,
                                tokens,
                                offsets,
                                mode,
                                idx + 1,
                            );
                            nodes.push(node);
                            idx = next_idx;
                            continue;
                        }
                        ControlAction::Open(ControlKind::Define) => {
                            let (list, next_idx, stop) =
                                parse_list(tokens, offsets, mode, idx + 1, StopMode::EndOnly);
                            let end_action = match stop {
                                StopSignal::End { action } => Some(action),
                                StopSignal::Eof | StopSignal::Else { .. } => None,
                            };
                            nodes.push(Node::Define(DefineNode {
                                pos,
                                text: canonical.clone(),
                                list,
                                end_action,
                            }));
                            idx = next_idx;
                            continue;
                        }
                        ControlAction::Open(ControlKind::Block) => {
                            let (list, next_idx, stop) =
                                parse_list(tokens, offsets, mode, idx + 1, StopMode::EndOnly);
                            let end_action = match stop {
                                StopSignal::End { action } => Some(action),
                                StopSignal::Eof | StopSignal::Else { .. } => None,
                            };
                            nodes.push(Node::Block(BlockNode {
                                pos,
                                text: canonical.clone(),
                                list,
                                end_action,
                            }));
                            idx = next_idx;
                            continue;
                        }
                        ControlAction::Else(nested) => {
                            nodes.push(Node::Else(ElseNode {
                                pos,
                                text: canonical.clone(),
                                chained: nested.is_some(),
                            }));
                            idx += 1;
                            continue;
                        }
                        ControlAction::End => {
                            nodes.push(Node::End(EndNode {
                                pos,
                                text: canonical.clone(),
                            }));
                            idx += 1;
                            continue;
                        }
                        ControlAction::Break => {
                            nodes.push(Node::Break(BreakNode {
                                pos,
                                text: canonical.clone(),
                            }));
                            idx += 1;
                            continue;
                        }
                        ControlAction::Continue => {
                            nodes.push(Node::Continue(ContinueNode {
                                pos,
                                text: canonical.clone(),
                            }));
                            idx += 1;
                            continue;
                        }
                        ControlAction::None => {}
                    }
                }

                if matches!(action_head_keyword(action), Some("template")) {
                    nodes.push(Node::Template(TemplateNode {
                        pos,
                        text: canonical,
                    }));
                } else {
                    nodes.push(Node::Action(ActionNode {
                        pos,
                        text: canonical,
                    }));
                }
                idx += 1;
            }
        }
    }

    (ListNode { nodes }, idx, StopSignal::Eof)
}

fn parse_branch_node(
    kind: ControlKind,
    open_action: &str,
    pos: usize,
    tokens: &[GoTemplateToken],
    offsets: &[usize],
    mode: Mode,
    start_idx: usize,
) -> (Node, usize) {
    let (list, mut next_idx, stop) =
        parse_list(tokens, offsets, mode, start_idx, StopMode::EndOrElse);
    let mut else_list: Option<ListNode> = None;
    let mut else_action: Option<String> = None;
    let mut end_action: Option<String> = None;

    if let StopSignal::Else {
        action,
        nested,
        pos: else_pos,
    } = stop
    {
        if let Some(nested_kind) = nested {
            let nested_open = normalize_else_nested_open_action(&action, nested_kind);
            let (nested_node, idx_after) = parse_branch_node(
                nested_kind,
                &nested_open,
                else_pos,
                tokens,
                offsets,
                mode,
                next_idx,
            );
            else_list = Some(ListNode {
                nodes: vec![nested_node],
            });
            else_action = Some(normalize_else_action(&action));
            next_idx = idx_after;
        } else {
            let (parsed_else, idx_after_else, stop_after_else) =
                parse_list(tokens, offsets, mode, next_idx, StopMode::EndOrElse);
            else_list = Some(parsed_else);
            else_action = Some(normalize_else_action(&action));
            if let StopSignal::End { action } = stop_after_else {
                end_action = Some(action);
            }
            next_idx = idx_after_else;
        }
    } else if let StopSignal::End { action } = stop {
        end_action = Some(action);
    }

    let node = match kind {
        ControlKind::If => Node::If(IfNode {
            pos,
            text: open_action.to_string(),
            list,
            else_list,
            else_action,
            end_action,
        }),
        ControlKind::Range => Node::Range(RangeNode {
            pos,
            text: open_action.to_string(),
            list,
            else_list,
            else_action,
            end_action,
        }),
        ControlKind::With => Node::With(WithNode {
            pos,
            text: open_action.to_string(),
            list,
            else_list,
            else_action,
            end_action,
        }),
        ControlKind::Define => Node::Define(DefineNode {
            pos,
            text: open_action.to_string(),
            list,
            end_action,
        }),
        ControlKind::Block => Node::Block(BlockNode {
            pos,
            text: open_action.to_string(),
            list,
            end_action,
        }),
    };

    (node, next_idx)
}

fn parse_control(action: &str) -> Option<ControlAction> {
    parse_action_report_with_options(
        action,
        0,
        ParseCompatOptions {
            skip_func_check: true,
            known_functions: &[],
            check_variables: false,
            visible_variables: &[],
        },
    )
    .ok()
    .map(|r| r.control)
}

fn is_comment_action(action: &str) -> bool {
    action_inner_trimmed(action).is_some_and(|trimmed| trimmed.starts_with("/*"))
}

fn action_head_keyword(action: &str) -> Option<&str> {
    action_inner_trimmed(action)?.split_whitespace().next()
}

pub(super) fn action_inner_trimmed(action: &str) -> Option<&str> {
    let mut inner = action;
    if let Some(rest) = inner.strip_prefix("{{") {
        inner = rest;
    } else {
        return None;
    }
    if let Some(rest) = inner.strip_suffix("}}") {
        inner = rest;
    } else {
        return None;
    }

    let mut trimmed = inner.trim();
    if let Some(rest) = trimmed.strip_prefix('-') {
        trimmed = rest.trim_start();
    }
    if let Some(rest) = trimmed.strip_suffix('-') {
        trimmed = rest.trim_end();
    }
    Some(trimmed)
}

fn normalize_else_action(action: &str) -> String {
    let Some(inner) = action_inner_trimmed(action) else {
        return "{{else}}".to_string();
    };
    if inner.starts_with("else") {
        "{{else}}".to_string()
    } else {
        canonical_action(action)
    }
}

fn normalize_else_nested_open_action(action: &str, kind: ControlKind) -> String {
    let Some(inner) = action_inner_trimmed(action) else {
        return default_open_for_kind(kind);
    };
    let Some(rest) = inner.strip_prefix("else") else {
        return action.to_string();
    };
    let rest = rest.trim_start();
    let (kw, rem) = match kind {
        ControlKind::If => ("if", rest.strip_prefix("if")),
        ControlKind::With => ("with", rest.strip_prefix("with")),
        ControlKind::Range => ("range", rest.strip_prefix("range")),
        ControlKind::Define | ControlKind::Block => return action.to_string(),
    };
    let Some(rem) = rem else {
        return default_open_for_kind(kind);
    };
    format!("{{{{{}{}}}}}", kw, rem)
}

fn default_open_for_kind(kind: ControlKind) -> String {
    match kind {
        ControlKind::If => "{{if}}".to_string(),
        ControlKind::With => "{{with}}".to_string(),
        ControlKind::Range => "{{range}}".to_string(),
        ControlKind::Define => "{{define}}".to_string(),
        ControlKind::Block => "{{block}}".to_string(),
    }
}

fn canonical_action(action: &str) -> String {
    let Some(inner) = action_inner_trimmed(action) else {
        return action.to_string();
    };
    format!("{{{{{inner}}}}}")
}

#[cfg(test)]
mod parser_tests;
