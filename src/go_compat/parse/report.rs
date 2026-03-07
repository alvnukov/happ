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
pub struct VariableRef {
    pub name: String,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionParseReport {
    pub control: ControlAction,
    pub define_name: Option<String>,
    pub declared_vars: Vec<VariableRef>,
    pub assigned_vars: Vec<VariableRef>,
    pub referenced_vars: Vec<VariableRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    If,
    Range,
    With,
    Define,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    None,
    Open(ControlKind),
    Else(Option<ControlKind>),
    Break,
    Continue,
    End,
}
