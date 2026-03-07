use super::{compat, GoTemplateScanError};
mod lex;
use self::lex::{is_space, lex_action_inner};

// Go parity references:
// - stdlib text/template/parse (pipeline/command parsing and semantic checks)
// - stdlib text/template/funcs (builtin function namespace)
const LEFT_DELIM: &str = "{{";
const RIGHT_DELIM: &str = "}}";
const GO_BUILTIN_FUNCTIONS: &[&str] = &[
    "and", "call", "html", "index", "slice", "js", "len", "not", "or", "print", "printf",
    "println", "urlquery", "eq", "ne", "lt", "le", "gt", "ge",
];

#[derive(Debug, Clone, Copy)]
pub struct ParseCompatOptions<'a> {
    pub skip_func_check: bool,
    pub known_functions: &'a [&'a str],
    pub check_variables: bool,
    pub visible_variables: &'a [&'a str],
}

impl<'a> Default for ParseCompatOptions<'a> {
    fn default() -> Self {
        Self {
            skip_func_check: true,
            known_functions: &[],
            check_variables: true,
            visible_variables: &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VariableRef {
    pub name: String,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionParseReport {
    pub control: ControlAction,
    pub define_name: Option<String>,
    pub declared_vars: Vec<VariableRef>,
    pub assigned_vars: Vec<VariableRef>,
    pub referenced_vars: Vec<VariableRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Space,
    Pipe,
    LeftParen,
    RightParen,
    Bool,
    CharConst,
    Number,
    Assign,
    Declare,
    Field,
    Identifier,
    Comma,
    String,
    RawString,
    Variable,
    Dot,
    Nil,
    KwBlock,
    KwBreak,
    KwContinue,
    KwDefine,
    KwElse,
    KwEnd,
    KwIf,
    KwRange,
    KwTemplate,
    KwWith,
    Char,
    Eof,
}

#[derive(Debug, Clone, Copy)]
struct Tok {
    kind: TokKind,
    start: usize,
    end: usize,
}

impl Tok {
    fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermKind {
    Identifier,
    Dot,
    Nil,
    Variable,
    Field,
    Bool,
    Number,
    String,
    OtherExec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlKind {
    If,
    Range,
    With,
    Define,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlAction {
    None,
    Open(ControlKind),
    Else(Option<ControlKind>),
    Break,
    Continue,
    End,
}

pub(crate) fn parse_action_compat(
    action: &str,
    action_start: usize,
) -> Result<ControlAction, GoTemplateScanError> {
    parse_action_compat_with_options(action, action_start, ParseCompatOptions::default())
}

pub(crate) fn parse_action_compat_with_options(
    action: &str,
    action_start: usize,
    options: ParseCompatOptions<'_>,
) -> Result<ControlAction, GoTemplateScanError> {
    parse_action_report_with_options(action, action_start, options).map(|r| r.control)
}

pub(crate) fn parse_action_report_with_options(
    action: &str,
    action_start: usize,
    options: ParseCompatOptions<'_>,
) -> Result<ActionParseReport, GoTemplateScanError> {
    let Some((inner, inner_rel_start)) = action_inner_with_offset(action) else {
        return Ok(ActionParseReport {
            control: ControlAction::None,
            define_name: None,
            declared_vars: Vec::new(),
            assigned_vars: Vec::new(),
            referenced_vars: Vec::new(),
        });
    };
    if inner.starts_with("/*") {
        return Ok(ActionParseReport {
            control: ControlAction::None,
            define_name: None,
            declared_vars: Vec::new(),
            assigned_vars: Vec::new(),
            referenced_vars: Vec::new(),
        });
    }
    if inner.is_empty() {
        return Err(GoTemplateScanError {
            code: "missing_value_for_context",
            message: "missing value for command".to_string(),
            offset: action_start + inner_rel_start,
        });
    }
    let abs_base = action_start + inner_rel_start;
    let tokens = lex_action_inner(inner, abs_base)?;

    let mut p = Parser::new(inner, tokens, abs_base, options);
    let control = p.parse_action()?;
    Ok(ActionParseReport {
        control,
        define_name: p.opened_define_name,
        declared_vars: p.declared_vars,
        assigned_vars: p.assigned_vars,
        referenced_vars: p.referenced_vars,
    })
}

fn action_inner_with_offset(action: &str) -> Option<(&str, usize)> {
    if !(action.starts_with(LEFT_DELIM) && action.ends_with(RIGHT_DELIM)) || action.len() < 4 {
        return None;
    }
    let inner = &action[LEFT_DELIM.len()..action.len() - RIGHT_DELIM.len()];
    let bytes = inner.as_bytes();

    let mut start = 0usize;
    let mut end = inner.len();

    if bytes.len() >= 2 && bytes[0] == b'-' && is_space(bytes[1]) {
        start = 1;
    }

    while start < end && is_space(bytes[start]) {
        start += 1;
    }
    while start < end && is_space(bytes[end - 1]) {
        end -= 1;
    }
    if end > start && bytes[end - 1] == b'-' {
        end -= 1;
        while start < end && is_space(bytes[end - 1]) {
            end -= 1;
        }
    }

    Some((&inner[start..end], LEFT_DELIM.len() + start))
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Tok>,
    base: usize,
    idx: usize,
    options: ParseCompatOptions<'a>,
    opened_define_name: Option<String>,
    declared_vars: Vec<VariableRef>,
    assigned_vars: Vec<VariableRef>,
    referenced_vars: Vec<VariableRef>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, tokens: Vec<Tok>, base: usize, options: ParseCompatOptions<'a>) -> Self {
        Self {
            src,
            tokens,
            base,
            idx: 0,
            options,
            opened_define_name: None,
            declared_vars: Vec::new(),
            assigned_vars: Vec::new(),
            referenced_vars: Vec::new(),
        }
    }

    fn parse_action(&mut self) -> Result<ControlAction, GoTemplateScanError> {
        let tok = self.peek_non_space();
        match tok.kind {
            TokKind::KwEnd => {
                let _ = self.next_non_space();
                let tail = self.next_non_space();
                if tail.kind != TokKind::Eof {
                    return Err(self.unexpected_token(&tail, "command"));
                }
                Ok(ControlAction::End)
            }
            TokKind::KwBreak | TokKind::KwContinue => {
                if self.keyword_token_is_function(tok) {
                    self.parse_pipeline("command", TokKind::Eof, false)?;
                    return Ok(ControlAction::None);
                }
                let kw = self.next_non_space().kind;
                let tail = self.next_non_space();
                if tail.kind != TokKind::Eof {
                    return Err(self.unexpected_token(&tail, "command"));
                }
                Ok(match kw {
                    TokKind::KwBreak => ControlAction::Break,
                    TokKind::KwContinue => ControlAction::Continue,
                    _ => unreachable!(),
                })
            }
            TokKind::KwElse => Ok(ControlAction::Else(self.parse_else_clause()?)),
            TokKind::KwDefine => {
                let name = self.parse_define_clause()?;
                self.opened_define_name = Some(name);
                Ok(ControlAction::Open(ControlKind::Define))
            }
            TokKind::KwTemplate => {
                self.parse_template_clause()?;
                Ok(ControlAction::None)
            }
            TokKind::KwBlock => {
                self.parse_block_clause()?;
                Ok(ControlAction::Open(ControlKind::Block))
            }
            TokKind::KwIf | TokKind::KwWith => {
                let _ = self.next_non_space();
                self.parse_pipeline("control", TokKind::Eof, false)?;
                let kind = match tok.kind {
                    TokKind::KwIf => ControlKind::If,
                    TokKind::KwWith => ControlKind::With,
                    _ => unreachable!(),
                };
                Ok(ControlAction::Open(kind))
            }
            TokKind::KwRange => {
                let _ = self.next_non_space();
                self.parse_pipeline("range", TokKind::Eof, true)?;
                Ok(ControlAction::Open(ControlKind::Range))
            }
            _ => {
                self.parse_pipeline("command", TokKind::Eof, false)?;
                Ok(ControlAction::None)
            }
        }
    }

    fn parse_else_clause(&mut self) -> Result<Option<ControlKind>, GoTemplateScanError> {
        let _ = self.next_non_space();
        match self.peek_non_space().kind {
            TokKind::Eof => Ok(None),
            TokKind::KwIf | TokKind::KwWith => {
                let kw = self.next_non_space().kind;
                self.parse_pipeline("control", TokKind::Eof, false)?;
                Ok(Some(match kw {
                    TokKind::KwIf => ControlKind::If,
                    TokKind::KwWith => ControlKind::With,
                    _ => unreachable!(),
                }))
            }
            _ => Err(self.unexpected_token(&self.peek_non_space(), "else")),
        }
    }

    fn parse_define_clause(&mut self) -> Result<String, GoTemplateScanError> {
        let _ = self.next_non_space();
        let name = self.next_non_space();
        match name.kind {
            TokKind::String | TokKind::RawString => {
                let decoded = compat::decode_go_string_literal(name.text(self.src))
                    .ok_or_else(|| self.unexpected_token(&name, "define clause"))?;
                let tail = self.next_non_space();
                if tail.kind == TokKind::Eof {
                    Ok(decoded)
                } else {
                    Err(self.unexpected_token(&tail, "define clause"))
                }
            }
            _ => Err(self.unexpected_token(&name, "define clause")),
        }
    }

    fn parse_template_clause(&mut self) -> Result<(), GoTemplateScanError> {
        let _ = self.next_non_space();
        let name = self.next_non_space();
        match name.kind {
            TokKind::String | TokKind::RawString => {
                compat::decode_go_string_literal(name.text(self.src))
                    .ok_or_else(|| self.unexpected_token(&name, "template clause"))?;
                if self.peek_non_space().kind == TokKind::Eof {
                    return Ok(());
                }
                self.parse_pipeline("template clause", TokKind::Eof, false)
            }
            _ => Err(self.unexpected_token(&name, "template clause")),
        }
    }

    fn parse_block_clause(&mut self) -> Result<(), GoTemplateScanError> {
        let _ = self.next_non_space();
        let name = self.next_non_space();
        match name.kind {
            TokKind::String | TokKind::RawString => {
                compat::decode_go_string_literal(name.text(self.src))
                    .ok_or_else(|| self.unexpected_token(&name, "block clause"))?;
                if self.peek_non_space().kind == TokKind::Eof {
                    return Err(GoTemplateScanError {
                        code: "missing_value_for_context",
                        message: "missing value for block clause".to_string(),
                        offset: self.abs(name.end),
                    });
                }
                self.parse_pipeline("block clause", TokKind::Eof, false)
            }
            _ => Err(self.unexpected_token(&name, "block clause")),
        }
    }

    fn parse_pipeline(
        &mut self,
        context: &'static str,
        end_kind: TokKind,
        allow_multi_decl: bool,
    ) -> Result<(), GoTemplateScanError> {
        self.parse_declarations(context, allow_multi_decl)?;
        let mut stage = 0usize;
        let mut saw_command = false;
        loop {
            let tok = self.next_non_space();
            if tok.kind == end_kind {
                break;
            }
            if tok.kind == TokKind::Eof {
                if end_kind == TokKind::Eof {
                    break;
                }
                return Err(self.unexpected_token(&tok, context));
            }
            if self.is_term_start(tok.kind) {
                self.backup();
                let first = self.parse_command()?;
                saw_command = true;
                stage += 1;
                // Go parity (text/template/parse): validate the command head function
                // and the explicit call target in `call <ident>`.
                if let Some(func) = first.first_identifier {
                    self.check_function_is_defined(func.name, func.start)?;
                }
                if let Some(func) = first.call_target_identifier {
                    self.check_function_is_defined(func.name, func.start)?;
                }
                if stage > 1
                    && matches!(
                        first.first_term,
                        TermKind::Bool
                            | TermKind::Dot
                            | TermKind::Nil
                            | TermKind::Number
                            | TermKind::String
                    )
                {
                    return Err(GoTemplateScanError {
                        code: "non_executable_command_in_pipeline",
                        message: format!(
                            "non executable command in pipeline stage {}",
                            stage
                        ),
                        offset: self.abs(first.first_start),
                    });
                }
                continue;
            }
            return Err(self.unexpected_token(&tok, context));
        }

        if !saw_command {
            return Err(GoTemplateScanError {
                code: "missing_value_for_context",
                message: context_message(context).to_string(),
                offset: self.current_offset(),
            });
        }

        Ok(())
    }

    fn parse_declarations(
        &mut self,
        context: &'static str,
        allow_multi_decl: bool,
    ) -> Result<(), GoTemplateScanError> {
        let checkpoint = self.idx;
        let v = self.peek_non_space();
        if v.kind != TokKind::Variable {
            return Ok(());
        }
        let _ = self.next_non_space();
        let next = self.peek_non_space();
        match next.kind {
            TokKind::Assign | TokKind::Declare => {
                let op = self.next_non_space();
                if op.kind == TokKind::Declare {
                    self.record_declared(v.text(self.src), v.start);
                } else {
                    self.record_assigned(v.text(self.src), v.start);
                }
                Ok(())
            }
            TokKind::Comma => {
                if !allow_multi_decl {
                    return Err(GoTemplateScanError {
                        code: "too_many_declarations",
                        message: "too many declarations".to_string(),
                        offset: self.abs(v.start),
                    });
                }
                let _ = self.next_non_space();
                let second = self.next_non_space();
                if second.kind != TokKind::Variable {
                    if second.kind == TokKind::Eof {
                        return Err(GoTemplateScanError {
                            code: "missing_value_for_context",
                            message: context_message(context).to_string(),
                            offset: self.current_offset(),
                        });
                    }
                    return Err(self.unexpected_token(&second, context));
                }
                if self.peek_non_space().kind == TokKind::Comma {
                    return Err(GoTemplateScanError {
                        code: "too_many_declarations",
                        message: "too many declarations".to_string(),
                        offset: self.abs(second.start),
                    });
                }
                let assign = self.next_non_space();
                if assign.kind != TokKind::Assign && assign.kind != TokKind::Declare {
                    if assign.kind == TokKind::Eof {
                        return Err(GoTemplateScanError {
                            code: "missing_value_for_context",
                            message: context_message(context).to_string(),
                            offset: self.current_offset(),
                        });
                    }
                    return Err(self.unexpected_token(&assign, context));
                }
                if assign.kind == TokKind::Declare {
                    self.record_declared(v.text(self.src), v.start);
                    self.record_declared(second.text(self.src), second.start);
                } else {
                    self.record_assigned(v.text(self.src), v.start);
                    self.record_assigned(second.text(self.src), second.start);
                }
                Ok(())
            }
            _ => {
                self.idx = checkpoint;
                Ok(())
            }
        }
    }

    fn parse_command(&mut self) -> Result<CommandInfo<'a>, GoTemplateScanError> {
        let mut first_term = None;
        let mut first_start = None;
        let mut first_identifier: Option<IdentifierRef<'a>> = None;
        let mut first_variable: Option<IdentifierRef<'a>> = None;
        let mut call_target_identifier: Option<IdentifierRef<'a>> = None;
        let mut args = 0usize;
        loop {
            let first_tok = self.peek_non_space();
            let operand = self.parse_operand()?;
            if let Some(term) = operand {
                if args == 1
                    && matches!(first_identifier, Some(id) if id.name == "call")
                    && term == TermKind::Identifier
                    && first_tok.kind == TokKind::Identifier
                {
                    call_target_identifier = Some(IdentifierRef {
                        name: first_tok.text(self.src),
                        start: first_tok.start,
                    });
                }
                if first_term.is_none() {
                    first_term = Some(term);
                    first_start = Some(first_tok.start);
                    if term == TermKind::Identifier && first_tok.kind == TokKind::Identifier {
                        first_identifier = Some(IdentifierRef {
                            name: first_tok.text(self.src),
                            start: first_tok.start,
                        });
                    } else if term == TermKind::Variable && first_tok.kind == TokKind::Variable {
                        first_variable = Some(IdentifierRef {
                            name: first_tok.text(self.src),
                            start: first_tok.start,
                        });
                    }
                }
                args += 1;
            }

            let tok = self.next();
            match tok.kind {
                TokKind::Space => continue,
                TokKind::Eof | TokKind::RightParen => {
                    self.backup();
                    break;
                }
                TokKind::Pipe => break,
                TokKind::Declare | TokKind::Assign => {
                    if let Some(v) = first_variable {
                        if self.options.check_variables && !self.variable_is_visible(v.name) {
                            return Err(GoTemplateScanError {
                                code: "undefined_variable",
                                message: format!("undefined variable \"{}\"", v.name),
                                offset: self.abs(v.start),
                            });
                        }
                    }
                    return Err(self.unexpected_in_operand(&tok));
                }
                _ => return Err(self.unexpected_in_operand(&tok)),
            }
        }

        if args == 0 {
            return Err(GoTemplateScanError {
                code: "empty_command",
                message: "empty command".to_string(),
                offset: self.current_offset(),
            });
        }
        // Go parity (text/template/parse): check variable visibility only after full
        // command parse, so declarations/assignments are not misclassified as reads.
        if let Some(v) = first_variable {
            if self.options.check_variables && !self.variable_is_visible(v.name) {
                return Err(GoTemplateScanError {
                    code: "undefined_variable",
                    message: format!("undefined variable \"{}\"", v.name),
                    offset: self.abs(v.start),
                });
            }
        }

        Ok(CommandInfo {
            first_term: first_term.unwrap_or(TermKind::OtherExec),
            first_start: first_start.unwrap_or(self.src.len()),
            first_identifier,
            call_target_identifier,
        })
    }

    fn parse_operand(&mut self) -> Result<Option<TermKind>, GoTemplateScanError> {
        let Some(term) = self.parse_term()? else {
            return Ok(None);
        };

        if self.peek().kind == TokKind::Field {
            while self.peek().kind == TokKind::Field {
                let _ = self.next();
            }
            if matches!(
                term,
                TermKind::Bool
                    | TermKind::String
                    | TermKind::Number
                    | TermKind::Nil
                    | TermKind::Dot
            ) {
                return Err(GoTemplateScanError {
                    code: "unexpected_dot_after_term",
                    message: "unexpected <.> after term".to_string(),
                    offset: self.abs(self.peek().start),
                });
            }
        }

        Ok(Some(term))
    }

    fn parse_term(&mut self) -> Result<Option<TermKind>, GoTemplateScanError> {
        let tok = self.next_non_space();
        match tok.kind {
            TokKind::Identifier => Ok(Some(TermKind::Identifier)),
            TokKind::KwBreak | TokKind::KwContinue if self.keyword_token_is_function(tok) => {
                Ok(Some(TermKind::Identifier))
            }
            TokKind::Dot => Ok(Some(TermKind::Dot)),
            TokKind::Nil => Ok(Some(TermKind::Nil)),
            TokKind::Variable => {
                self.record_reference(tok.text(self.src), tok.start);
                Ok(Some(TermKind::Variable))
            }
            TokKind::Field => Ok(Some(TermKind::Field)),
            TokKind::Bool => Ok(Some(TermKind::Bool)),
            TokKind::CharConst | TokKind::Number => Ok(Some(TermKind::Number)),
            TokKind::String | TokKind::RawString => Ok(Some(TermKind::String)),
            TokKind::LeftParen => {
                self.parse_pipeline("parenthesized pipeline", TokKind::RightParen, false)?;
                Ok(Some(TermKind::OtherExec))
            }
            _ => {
                self.backup();
                Ok(None)
            }
        }
    }

    fn unexpected_in_operand(&self, tok: &Tok) -> GoTemplateScanError {
        match tok.kind {
            TokKind::Char if tok.text(self.src) == "{" => GoTemplateScanError {
                code: "unexpected_left_delim_in_operand",
                message: "unexpected \"{\" in operand".to_string(),
                offset: self.abs(tok.start),
            },
            TokKind::Dot => GoTemplateScanError {
                code: "unexpected_dot_in_operand",
                message: "unexpected <.> in operand".to_string(),
                offset: self.abs(tok.start),
            },
            _ => self.unexpected_token(tok, "operand"),
        }
    }

    fn unexpected_token(&self, tok: &Tok, context: &'static str) -> GoTemplateScanError {
        if tok.kind == TokKind::Eof {
            return GoTemplateScanError {
                code: "unexpected_eof",
                message: "unexpected EOF".to_string(),
                offset: self.abs(tok.start),
            };
        }
        GoTemplateScanError {
            code: "unexpected_token",
            message: context_message(context).to_string(),
            offset: self.abs(tok.start),
        }
    }

    fn current_offset(&self) -> usize {
        self.tokens
            .get(self.idx)
            .or_else(|| self.tokens.last())
            .map_or(self.base, |t| self.abs(t.start))
    }

    fn keyword_token_is_function(&self, tok: Tok) -> bool {
        let Some(name) = keyword_function_candidate(tok.kind) else {
            return false;
        };
        self.function_is_defined(name)
    }

    fn check_function_is_defined(
        &self,
        name: &'a str,
        start: usize,
    ) -> Result<(), GoTemplateScanError> {
        if self.options.skip_func_check || self.function_is_defined(name) {
            return Ok(());
        }
        Err(GoTemplateScanError {
            code: "undefined_function",
            message: format!("function \"{name}\" not defined"),
            offset: self.abs(start),
        })
    }

    fn function_is_defined(&self, name: &str) -> bool {
        GO_BUILTIN_FUNCTIONS.iter().any(|builtin| builtin == &name)
            || self
                .options
                .known_functions
                .iter()
                .any(|known| known == &name)
    }

    fn is_term_start(&self, kind: TokKind) -> bool {
        is_term_start(kind)
            || keyword_function_candidate(kind).is_some_and(|name| self.function_is_defined(name))
    }

    fn variable_is_visible(&self, name: &str) -> bool {
        if name == "$" {
            return true;
        }
        self.options.visible_variables.iter().any(|v| v == &name)
    }

    fn record_declared(&mut self, name: &str, start: usize) {
        self.declared_vars.push(VariableRef {
            name: name.to_string(),
            offset: self.abs(start),
        });
    }

    fn record_assigned(&mut self, name: &str, start: usize) {
        self.assigned_vars.push(VariableRef {
            name: name.to_string(),
            offset: self.abs(start),
        });
    }

    fn record_reference(&mut self, name: &str, start: usize) {
        self.referenced_vars.push(VariableRef {
            name: name.to_string(),
            offset: self.abs(start),
        });
    }

    fn abs(&self, local: usize) -> usize {
        self.base + local
    }

    fn peek(&self) -> Tok {
        self.tokens.get(self.idx).copied().unwrap_or(Tok {
            kind: TokKind::Eof,
            start: self.src.len(),
            end: self.src.len(),
        })
    }

    fn next(&mut self) -> Tok {
        let tok = self.peek();
        if self.idx < self.tokens.len() {
            self.idx += 1;
        }
        tok
    }

    fn backup(&mut self) {
        if self.idx > 0 {
            self.idx -= 1;
        }
    }

    fn peek_non_space(&self) -> Tok {
        let mut i = self.idx;
        while let Some(tok) = self.tokens.get(i).copied() {
            if tok.kind != TokKind::Space {
                return tok;
            }
            i += 1;
        }
        Tok {
            kind: TokKind::Eof,
            start: self.src.len(),
            end: self.src.len(),
        }
    }

    fn next_non_space(&mut self) -> Tok {
        loop {
            let tok = self.next();
            if tok.kind != TokKind::Space {
                return tok;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CommandInfo<'a> {
    first_term: TermKind,
    first_start: usize,
    first_identifier: Option<IdentifierRef<'a>>,
    call_target_identifier: Option<IdentifierRef<'a>>,
}

#[derive(Debug, Clone, Copy)]
struct IdentifierRef<'a> {
    name: &'a str,
    start: usize,
}

fn keyword_function_candidate(kind: TokKind) -> Option<&'static str> {
    match kind {
        TokKind::KwBreak => Some("break"),
        TokKind::KwContinue => Some("continue"),
        _ => None,
    }
}

fn context_message(context: &'static str) -> &'static str {
    match context {
        "command" => "unexpected token in command",
        "operand" => "unexpected token in operand",
        "define clause" => "unexpected token in define clause",
        "template clause" => "unexpected token in template clause",
        "block clause" => "unexpected token in block clause",
        "else" => "unexpected token in else clause",
        "control" => "unexpected token in control",
        "range" => "missing value for range",
        "parenthesized pipeline" => "unexpected token in parenthesized pipeline",
        _ => "unexpected token",
    }
}

fn is_term_start(kind: TokKind) -> bool {
    matches!(
        kind,
        TokKind::Bool
            | TokKind::CharConst
            | TokKind::Dot
            | TokKind::Field
            | TokKind::Identifier
            | TokKind::Number
            | TokKind::Nil
            | TokKind::RawString
            | TokKind::String
            | TokKind::Variable
            | TokKind::LeftParen
    )
}

#[cfg(test)]
mod tests;
