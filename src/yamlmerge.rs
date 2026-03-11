use serde::Deserialize;
use serde_yaml::{Mapping, Value};
use std::collections::HashSet;
use std::ffi::CStr;
use std::slice;
use unsafe_libyaml::{
    yaml_event_delete, yaml_event_t, yaml_parser_delete, yaml_parser_initialize, yaml_parser_parse,
    yaml_parser_set_input_string, yaml_parser_t, yaml_scalar_style_t, YAML_ALIAS_EVENT,
    YAML_DOCUMENT_END_EVENT, YAML_DOCUMENT_START_EVENT, YAML_MAPPING_END_EVENT,
    YAML_MAPPING_START_EVENT, YAML_PLAIN_SCALAR_STYLE, YAML_SCALAR_EVENT, YAML_SEQUENCE_END_EVENT,
    YAML_SEQUENCE_START_EVENT, YAML_STREAM_END_EVENT,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PathSegment {
    Key(String),
    Index(usize),
}

type Path = Vec<PathSegment>;

#[derive(Debug, Default, Clone)]
struct MergeStyleHints {
    plain_merge_paths: HashSet<Path>,
    nonplain_merge_paths: HashSet<Path>,
}

enum Frame {
    Mapping {
        path: Path,
        expecting_key: bool,
        current_key: Option<String>,
    },
    Sequence {
        path: Path,
        next_index: usize,
    },
}

#[derive(Default)]
struct HintCollector {
    docs: Vec<MergeStyleHints>,
    current_doc: Option<usize>,
    stack: Vec<Frame>,
}

impl HintCollector {
    fn into_docs(self) -> Vec<MergeStyleHints> {
        self.docs
    }

    fn on_document_start(&mut self) {
        self.docs.push(MergeStyleHints::default());
        self.current_doc = Some(self.docs.len() - 1);
        self.stack.clear();
    }

    fn on_document_end(&mut self) {
        self.stack.clear();
        self.current_doc = None;
    }

    fn on_mapping_start(&mut self) {
        let child_path = begin_container(&mut self.stack);
        self.stack.push(Frame::Mapping {
            path: child_path,
            expecting_key: true,
            current_key: None,
        });
    }

    fn on_mapping_end(&mut self) {
        self.stack.pop();
    }

    fn on_sequence_start(&mut self) {
        let child_path = begin_container(&mut self.stack);
        self.stack.push(Frame::Sequence {
            path: child_path,
            next_index: 0,
        });
    }

    fn on_sequence_end(&mut self) {
        self.stack.pop();
    }

    unsafe fn on_scalar_event(&mut self, event: &yaml_event_t) {
        let mut consumed_mapping_key = false;
        let mut merge_hint: Option<(Path, yaml_scalar_style_t)> = None;

        if let Some(Frame::Mapping {
            path,
            expecting_key,
            current_key,
        }) = self.stack.last_mut()
        {
            if *expecting_key {
                let key = scalar_string(event);
                if key == "<<" {
                    merge_hint = Some((path.clone(), event.data.scalar.style));
                }
                *current_key = Some(key);
                *expecting_key = false;
                consumed_mapping_key = true;
            }
        }

        if let Some((path, style)) = merge_hint {
            self.record_merge_style(path, style);
        }
        if consumed_mapping_key {
            return;
        }

        consume_scalar_or_alias_value(&mut self.stack);
    }

    fn on_alias_event(&mut self) {
        let mut consumed_mapping_key = false;
        if let Some(Frame::Mapping {
            expecting_key,
            current_key,
            ..
        }) = self.stack.last_mut()
        {
            if *expecting_key {
                *expecting_key = false;
                *current_key = None;
                consumed_mapping_key = true;
            }
        }
        if consumed_mapping_key {
            return;
        }
        consume_scalar_or_alias_value(&mut self.stack);
    }

    fn record_merge_style(&mut self, path: Path, style: yaml_scalar_style_t) {
        let Some(doc_idx) = self.current_doc else {
            return;
        };
        if style == YAML_PLAIN_SCALAR_STYLE {
            self.docs[doc_idx].plain_merge_paths.insert(path);
        } else {
            self.docs[doc_idx].nonplain_merge_paths.insert(path);
        }
    }
}

pub fn normalize_value(v: Value) -> Value {
    normalize_value_with_hints(v, None, &mut Vec::new())
}

#[allow(dead_code)]
pub fn normalize_value_from_source(input: &str, v: Value) -> Value {
    let hints = collect_merge_style_hints(input).ok();
    normalize_value_with_hints(v, hints.as_ref().and_then(|v| v.first()), &mut Vec::new())
}

pub fn normalize_documents(input: &str) -> Result<Vec<Value>, serde_yaml::Error> {
    let docs: Vec<Value> = serde_yaml::Deserializer::from_str(input)
        .map(Value::deserialize)
        .collect::<Result<Vec<_>, _>>()?;
    let hints = collect_merge_style_hints(input).ok();
    Ok(docs
        .into_iter()
        .enumerate()
        .map(|(i, doc)| {
            normalize_value_with_hints(doc, hints.as_ref().and_then(|v| v.get(i)), &mut Vec::new())
        })
        .filter(|v| !v.is_null())
        .collect())
}

fn normalize_value_with_hints(v: Value, hints: Option<&MergeStyleHints>, path: &mut Path) -> Value {
    match v {
        Value::Mapping(map) => normalize_mapping_merge(map, hints, path),
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for (i, item) in seq.into_iter().enumerate() {
                path.push(PathSegment::Index(i));
                out.push(normalize_value_with_hints(item, hints, path));
                path.pop();
            }
            Value::Sequence(out)
        }
        other => other,
    }
}

fn normalize_mapping_merge(
    map: Mapping,
    hints: Option<&MergeStyleHints>,
    path: &mut Path,
) -> Value {
    let merge_key = Value::String("<<".to_string());
    let has_merge_key = map.contains_key(&merge_key);
    let should_merge = has_merge_key && should_apply_merge(hints, path);

    let mut out = Mapping::new();
    if should_merge {
        if let Some(merge_source) = map.get(&merge_key).cloned() {
            apply_merge_source(&mut out, merge_source);
        }
    }

    for (k, v) in map {
        if matches!(&k, Value::String(s) if s == "<<") && should_merge {
            continue;
        }
        let key_seg = mapping_key_segment(&k);
        path.push(PathSegment::Key(key_seg));
        let nv = normalize_value_with_hints(v, hints, path);
        path.pop();
        out.insert(k, nv);
    }
    Value::Mapping(out)
}

fn should_apply_merge(hints: Option<&MergeStyleHints>, path: &Path) -> bool {
    let Some(hints) = hints else {
        return true;
    };
    if hints.plain_merge_paths.contains(path) {
        return true;
    }
    if hints.nonplain_merge_paths.contains(path) {
        return false;
    }
    true
}

fn mapping_key_segment(k: &Value) -> String {
    if let Some(s) = k.as_str() {
        return s.to_string();
    }
    serde_yaml::to_string(k)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "<non-string-key>".to_string())
}

fn apply_merge_source(target: &mut Mapping, source: Value) {
    match normalize_value(source) {
        Value::Mapping(m) => merge_mapping_into(target, m),
        Value::Sequence(seq) => {
            for item in seq {
                if let Value::Mapping(m) = normalize_value(item) {
                    merge_mapping_into(target, m);
                }
            }
        }
        _ => {}
    }
}

fn merge_mapping_into(target: &mut Mapping, source: Mapping) {
    for (k, v) in source {
        target.entry(k).or_insert(v);
    }
}

fn collect_merge_style_hints(input: &str) -> Result<Vec<MergeStyleHints>, String> {
    unsafe {
        with_yaml_parser(input, |parser| {
            collect_merge_style_hints_from_parser(parser)
        })
    }
}

unsafe fn with_yaml_parser<T, F>(input: &str, parse: F) -> Result<T, String>
where
    F: FnOnce(&mut yaml_parser_t) -> Result<T, String>,
{
    let mut parser = std::mem::MaybeUninit::<yaml_parser_t>::uninit();
    if !yaml_parser_initialize(parser.as_mut_ptr()).ok {
        return Err("yaml parser init failed".to_string());
    }
    let mut parser = parser.assume_init();
    yaml_parser_set_input_string(&mut parser, input.as_ptr(), input.len() as u64);
    let result = parse(&mut parser);
    yaml_parser_delete(&mut parser);
    result
}

unsafe fn collect_merge_style_hints_from_parser(
    parser: &mut yaml_parser_t,
) -> Result<Vec<MergeStyleHints>, String> {
    let mut collector = HintCollector::default();

    loop {
        let mut event = std::mem::MaybeUninit::<yaml_event_t>::zeroed().assume_init();
        if !yaml_parser_parse(parser, &mut event).ok {
            return Err(parser_error(parser));
        }
        let event_type = event.type_;

        match event_type {
            YAML_DOCUMENT_START_EVENT => collector.on_document_start(),
            YAML_DOCUMENT_END_EVENT => collector.on_document_end(),
            YAML_MAPPING_START_EVENT => collector.on_mapping_start(),
            YAML_MAPPING_END_EVENT => collector.on_mapping_end(),
            YAML_SEQUENCE_START_EVENT => collector.on_sequence_start(),
            YAML_SEQUENCE_END_EVENT => collector.on_sequence_end(),
            YAML_SCALAR_EVENT => collector.on_scalar_event(&event),
            YAML_ALIAS_EVENT => collector.on_alias_event(),
            _ => {}
        }

        yaml_event_delete(&mut event);
        if event_type == YAML_STREAM_END_EVENT {
            break;
        }
    }

    Ok(collector.into_docs())
}

fn begin_container(stack: &mut [Frame]) -> Path {
    let Some(parent) = stack.last_mut() else {
        return Vec::new();
    };
    match parent {
        Frame::Sequence { path, next_index } => {
            let idx = *next_index;
            *next_index += 1;
            let mut out = path.clone();
            out.push(PathSegment::Index(idx));
            out
        }
        Frame::Mapping {
            path,
            expecting_key,
            current_key,
        } => {
            if *expecting_key {
                *expecting_key = false;
                *current_key = None;
                let mut out = path.clone();
                out.push(PathSegment::Key("<complex-key>".to_string()));
                return out;
            }
            let key = current_key
                .take()
                .unwrap_or_else(|| "<complex-key>".to_string());
            *expecting_key = true;
            let mut out = path.clone();
            out.push(PathSegment::Key(key));
            out
        }
    }
}

fn consume_scalar_or_alias_value(stack: &mut [Frame]) {
    let Some(parent) = stack.last_mut() else {
        return;
    };
    match parent {
        Frame::Sequence { next_index, .. } => {
            *next_index += 1;
        }
        Frame::Mapping {
            expecting_key,
            current_key,
            ..
        } => {
            if !*expecting_key {
                *expecting_key = true;
                *current_key = None;
            }
        }
    }
}

unsafe fn scalar_string(event: &yaml_event_t) -> String {
    let ptr = event.data.scalar.value.cast::<u8>();
    let len = event.data.scalar.length as usize;
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    let bytes = slice::from_raw_parts(ptr, len);
    String::from_utf8_lossy(bytes).into_owned()
}

unsafe fn parser_error(parser: &yaml_parser_t) -> String {
    let problem = if parser.problem.is_null() {
        "yaml parse error".to_string()
    } else {
        CStr::from_ptr(parser.problem.cast::<std::ffi::c_char>())
            .to_string_lossy()
            .into_owned()
    };
    format!(
        "{} at line {} column {}",
        problem,
        parser.problem_mark.line + 1,
        parser.problem_mark.column + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_merge_style_hints_tracks_plain_and_quoted_merge_keys() {
        let src = r#"
base: &base
  x: 1
plain:
  <<: *base
quoted:
  "<<": *base
single_quoted:
  '<<': *base
"#;
        let hints = collect_merge_style_hints(src).expect("collect hints");
        assert_eq!(hints.len(), 1);
        let doc = &hints[0];

        let plain_path = vec![PathSegment::Key("plain".to_string())];
        let quoted_path = vec![PathSegment::Key("quoted".to_string())];
        let single_quoted_path = vec![PathSegment::Key("single_quoted".to_string())];

        assert!(doc.plain_merge_paths.contains(&plain_path));
        assert!(doc.nonplain_merge_paths.contains(&quoted_path));
        assert!(doc.nonplain_merge_paths.contains(&single_quoted_path));
    }

    #[test]
    fn collect_merge_style_hints_reports_line_and_column_for_invalid_yaml() {
        let src = "name: {{ $.Values.global.env }}-integration\n";
        let err = collect_merge_style_hints(src).expect_err("must fail");
        assert!(err.contains("line"), "err: {err}");
        assert!(err.contains("column"), "err: {err}");
    }

    #[test]
    fn normalize_value_from_source_falls_back_to_default_merge_when_hints_fail() {
        let invalid_source = "name: {{ $.Values.global.env }}-integration\n";
        let value: Value = serde_yaml::from_str(
            r#"
obj:
  <<:
    common: true
  name: svc
"#,
        )
        .expect("parse");
        let normalized = normalize_value_from_source(invalid_source, value);
        let json = serde_json::to_value(normalized).expect("json");
        assert_eq!(json["obj"]["common"], true);
        assert_eq!(json["obj"]["name"], "svc");
        assert!(json["obj"].get("<<").is_none());
    }

    #[test]
    fn begin_container_for_sequence_uses_next_index_and_increments_cursor() {
        let mut stack = vec![Frame::Sequence {
            path: vec![PathSegment::Key("root".to_string())],
            next_index: 3,
        }];
        let child_path = begin_container(&mut stack);
        assert_eq!(
            child_path,
            vec![PathSegment::Key("root".to_string()), PathSegment::Index(3)]
        );
        let Frame::Sequence { next_index, .. } = &stack[0] else {
            panic!("expected sequence frame");
        };
        assert_eq!(*next_index, 4);
    }

    #[test]
    fn begin_container_for_mapping_value_uses_current_key_and_resets_state() {
        let mut stack = vec![Frame::Mapping {
            path: vec![PathSegment::Key("root".to_string())],
            expecting_key: false,
            current_key: Some("spec".to_string()),
        }];
        let child_path = begin_container(&mut stack);
        assert_eq!(
            child_path,
            vec![
                PathSegment::Key("root".to_string()),
                PathSegment::Key("spec".to_string())
            ]
        );
        let Frame::Mapping {
            expecting_key,
            current_key,
            ..
        } = &stack[0]
        else {
            panic!("expected mapping frame");
        };
        assert!(*expecting_key);
        assert!(current_key.is_none());
    }

    #[test]
    fn resolves_inline_merge_map() {
        let src = r#"
obj:
  <<: { foo: 123, bar: 456 }
  baz: 999
"#;
        let v: Value = serde_yaml::from_str(src).expect("parse");
        let n = normalize_value_from_source(src, v);
        let j = serde_json::to_value(n).expect("json");
        assert_eq!(j["obj"]["foo"], 123);
        assert_eq!(j["obj"]["bar"], 456);
        assert_eq!(j["obj"]["baz"], 999);
        assert!(j["obj"].get("<<").is_none());
        let line = serde_json::to_string(&j["obj"]).expect("json");
        assert_eq!(line, r#"{"foo":123,"bar":456,"baz":999}"#);
    }

    #[test]
    fn merge_sequence_earlier_source_overrides_later_source() {
        let src = r#"
base1: &base1
  x: first
base2: &base2
  x: second
obj:
  <<: [*base1, *base2]
"#;
        let v: Value = serde_yaml::from_str(src).expect("parse");
        let n = normalize_value_from_source(src, v);
        let j = serde_json::to_value(n).expect("json");
        assert_eq!(j["obj"]["x"], "first");
    }

    #[test]
    fn explicit_key_overrides_merged_value() {
        let src = r#"
base: &base
  image: nginx
  replicas: 2
obj:
  <<: *base
  replicas: 3
"#;
        let v: Value = serde_yaml::from_str(src).expect("parse");
        let n = normalize_value_from_source(src, v);
        let j = serde_json::to_value(n).expect("json");
        assert_eq!(j["obj"]["image"], "nginx");
        assert_eq!(j["obj"]["replicas"], 3);
    }

    #[test]
    fn quoted_merge_key_is_treated_as_regular_key() {
        let src = r#"
obj:
  "<<": { foo: 1 }
  baz: 2
"#;
        let v: Value = serde_yaml::from_str(src).expect("parse");
        let n = normalize_value_from_source(src, v);
        let j = serde_json::to_value(n).expect("json");
        assert_eq!(j["obj"]["<<"]["foo"], 1);
        assert_eq!(j["obj"]["baz"], 2);
    }

    #[test]
    fn merge_precedence_property_like_regression() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..200 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let a = (seed % 1000) as i64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let b = (seed % 1000) as i64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let c = (seed % 1000) as i64;

            let src = format!(
                r#"
base1: &base1
  x: {a}
  y: {b}
base2: &base2
  y: {c}
  z: {a}
obj:
  <<: [*base1, *base2]
  z: {b}
"#
            );
            let v: Value = serde_yaml::from_str(&src).expect("parse");
            let n = normalize_value_from_source(&src, v);
            let j = serde_json::to_value(n).expect("json");
            assert_eq!(j["obj"]["x"], a, "x mismatch for source:\n{src}");
            assert_eq!(j["obj"]["y"], b, "y precedence mismatch for source:\n{src}");
            assert_eq!(
                j["obj"]["z"], b,
                "local override mismatch for source:\n{src}"
            );
        }
    }
}
