use crate::go_compat::parse::{self, Mode, ParseError, Tree};
use crate::gotemplates::{
    render_template_native_with_resolver, NativeFunctionResolver, NativeRenderError,
    NativeRenderOptions,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::option::{TemplateOptionError, TemplateOptions};

#[derive(Debug, Clone)]
pub struct Template {
    name: String,
    left_delim: String,
    right_delim: String,
    mode: Mode,
    options: TemplateOptions,
    known_functions: BTreeSet<String>,
    trees: BTreeMap<String, Tree>,
}

impl Template {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            left_delim: "{{".to_string(),
            right_delim: "}}".to_string(),
            mode: Mode::default(),
            options: TemplateOptions::default(),
            known_functions: BTreeSet::new(),
            trees: BTreeMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn new_associated(&self, name: &str) -> Self {
        let mut next = self.clone();
        next.name = name.to_string();
        if !next.trees.contains_key(name) {
            next.trees.insert(
                name.to_string(),
                Tree {
                    name: name.to_string(),
                    parse_name: name.to_string(),
                    root: parse::ListNode::default(),
                    mode: next.mode,
                    text: String::new(),
                },
            );
        }
        next
    }

    pub fn set_mode(&mut self, mode: Mode) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn option(&mut self, spec: &str) -> Result<&mut Self, TemplateOptionError> {
        self.options.apply(spec)?;
        Ok(self)
    }

    pub fn delims(&mut self, left: &str, right: &str) -> &mut Self {
        self.left_delim = left.to_string();
        self.right_delim = right.to_string();
        self
    }

    pub fn funcs<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) -> &mut Self {
        for name in names {
            self.known_functions.insert(name.to_string());
        }
        self
    }

    pub fn parse(&mut self, text: &str) -> Result<&mut Self, ParseError> {
        let known: Vec<&str> = self.known_functions.iter().map(String::as_str).collect();
        let parsed = parse::parse(
            &self.name,
            text,
            &self.left_delim,
            &self.right_delim,
            self.mode,
            &known,
        )?;
        for (name, tree) in parsed {
            self.add_parse_tree(&name, tree);
        }
        Ok(self)
    }

    pub fn add_parse_tree(&mut self, name: &str, mut tree: Tree) -> &mut Self {
        tree.name = name.to_string();
        if tree.parse_name.is_empty() {
            tree.parse_name = self.name.clone();
        }
        self.trees.insert(name.to_string(), tree);
        self
    }

    pub fn lookup(&self, name: &str) -> Option<&Tree> {
        self.trees.get(name)
    }

    pub fn templates(&self) -> Vec<&Tree> {
        self.trees.values().collect()
    }

    pub fn defined_templates(&self) -> String {
        let mut names: Vec<&str> = self.trees.keys().map(String::as_str).collect();
        names.sort_unstable();
        if names.is_empty() {
            String::new()
        } else {
            let quoted: Vec<String> = names
                .iter()
                .map(|name| format!("\"{}\"", escape_quoted(name)))
                .collect();
            format!("; defined templates are: {}", quoted.join(", "))
        }
    }

    pub fn execute(&self, data: &Value) -> Result<String, NativeRenderError> {
        self.execute_template_with_resolver(
            &self.name,
            data,
            self.options.to_native_render_options(),
            None,
        )
    }

    pub fn execute_template(&self, name: &str, data: &Value) -> Result<String, NativeRenderError> {
        self.execute_template_with_resolver(
            name,
            data,
            self.options.to_native_render_options(),
            None,
        )
    }

    pub fn execute_with_resolver(
        &self,
        data: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        self.execute_template_with_resolver(&self.name, data, options, resolver)
    }

    pub fn execute_template_with_resolver(
        &self,
        name: &str,
        data: &Value,
        options: NativeRenderOptions,
        resolver: Option<&dyn NativeFunctionResolver>,
    ) -> Result<String, NativeRenderError> {
        if !self.trees.contains_key(name) {
            return Err(NativeRenderError::TemplateNotFound {
                name: name.to_string(),
            });
        }

        let mut src = self.defined_templates_source();
        src.push_str("{{template \"");
        src.push_str(&escape_quoted(name));
        src.push_str("\" .}}");

        render_template_native_with_resolver(&src, data, options, resolver)
    }

    fn defined_templates_source(&self) -> String {
        let mut out = String::new();
        for tree in self.trees.values() {
            out.push_str("{{define \"");
            out.push_str(&escape_quoted(&tree.name));
            out.push_str("\"}}");
            out.push_str(&tree.root.to_source());
            out.push_str("{{end}}");
        }
        out
    }
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Number, Value};

    #[test]
    fn template_executes_named_define() {
        let mut tpl = Template::new("main");
        tpl.parse(r#"A{{define "x"}}{{.v}}{{end}}"#)
            .expect("parse must succeed");

        let out = tpl
            .execute_template("x", &json!({"v":"ok"}))
            .expect("execute must succeed");
        assert_eq!(out, "ok");
    }

    #[test]
    fn template_executes_main_name() {
        let mut tpl = Template::new("main");
        tpl.parse(r#"{{define "main"}}hello {{.name}}{{end}}"#)
            .expect("parse must succeed");

        let out = tpl
            .execute(&json!({"name":"zol"}))
            .expect("execute must succeed");
        assert_eq!(out, "hello zol");
    }

    #[test]
    fn parse_accumulates_associated_templates_like_go() {
        let mut tpl = Template::new("main");
        tpl.parse(r#"{{define "a"}}A{{end}}"#)
            .expect("first parse must succeed");
        tpl.parse(r#"{{define "b"}}B{{end}}"#)
            .expect("second parse must succeed");

        let a = tpl
            .execute_template("a", &json!({}))
            .expect("a must be available");
        let b = tpl
            .execute_template("b", &json!({}))
            .expect("b must be available");
        assert_eq!(a, "A");
        assert_eq!(b, "B");
    }

    #[test]
    fn defined_templates_lists_non_empty_names() {
        let mut tpl = Template::new("main");
        tpl.parse(r#"{{define "a"}}A{{end}}{{define "b"}}B{{end}}"#)
            .expect("parse must succeed");
        let listed = tpl.defined_templates();
        assert!(listed.contains("a"));
        assert!(listed.contains("b"));
        assert!(listed.contains("main"));
    }

    #[test]
    fn add_parse_tree_registers_tree_by_name() {
        let mut tpl = Template::new("main");
        let tree = Tree {
            name: "x".to_string(),
            parse_name: "main".to_string(),
            root: parse::ListNode {
                nodes: vec![parse::Node::Text(parse::TextNode {
                    pos: 0,
                    text: "X".to_string(),
                })],
            },
            mode: Mode::default(),
            text: String::new(),
        };
        tpl.add_parse_tree("x", tree);
        let out = tpl
            .execute_template("x", &json!({}))
            .expect("execute template must succeed");
        assert_eq!(out, "X");
    }

    #[test]
    fn template_parses_and_executes_with_custom_delimiters() {
        let mut tpl = Template::new("main");
        tpl.delims("<<", ">>");
        tpl.parse(r#"<<define "main">>hello <<.name>><<end>>"#)
            .expect("parse must succeed");

        let out = tpl
            .execute(&json!({"name":"zol"}))
            .expect("execute must succeed");
        assert_eq!(out, "hello zol");
    }

    #[test]
    fn missingkey_error_option_uses_error_mode() {
        let mut tpl = Template::new("main");
        tpl.option("missingkey=error")
            .expect("option apply must succeed");
        tpl.parse(r#"{{define "main"}}{{.missing.value}}{{end}}"#)
            .expect("parse must succeed");
        let err = tpl.execute(&json!({})).expect_err("must fail");
        match err {
            NativeRenderError::MissingValue { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missingkey_zero_option_supports_typed_map_zero_values() {
        let mut tpl = Template::new("main");
        tpl.option("missingkey=zero")
            .expect("option apply must succeed");
        tpl.parse(
            r#"{{define "main"}}{{.m.missing.y}}|{{index .m "missing"}}|{{printf "%T" (index .m "missing")}}{{end}}"#,
        )
        .expect("parse must succeed");

        let mut inner = serde_json::Map::new();
        inner.insert("y".to_string(), Value::Number(Number::from(2)));
        let mut outer = serde_json::Map::new();
        outer.insert(
            "x".to_string(),
            crate::go_compat::typedvalue::encode_go_typed_map_value("int", Some(inner)),
        );
        let mut root = serde_json::Map::new();
        root.insert(
            "m".to_string(),
            crate::go_compat::typedvalue::encode_go_typed_map_value("map[string]int", Some(outer)),
        );

        let out = tpl
            .execute(&Value::Object(root))
            .expect("execute must succeed");
        assert_eq!(out, "0|map[]|map[string]int");
    }
}
