use crate::gotemplates::go_compat::truth::{
    builtin_and as go_builtin_and, builtin_or as go_builtin_or, is_truthy as go_is_truthy,
};
use serde_json::Value;

pub(super) fn is_truthy(v: &Option<Value>) -> bool {
    go_is_truthy(v.as_ref())
}

pub(super) fn builtin_and(args: &[Option<Value>]) -> Option<Value> {
    go_builtin_and(args)
}

pub(super) fn builtin_or(args: &[Option<Value>]) -> Option<Value> {
    go_builtin_or(args)
}

#[cfg(test)]
mod tests {
    use super::{builtin_and, builtin_or, is_truthy};
    use serde_json::{json, Value};

    #[test]
    fn truthiness_for_plain_json_values() {
        assert!(!is_truthy(&None));
        assert!(!is_truthy(&Some(Value::Null)));
        assert!(!is_truthy(&Some(json!(0))));
        assert!(!is_truthy(&Some(json!(""))));
        assert!(!is_truthy(&Some(json!([]))));
        assert!(!is_truthy(&Some(json!({}))));
        assert!(is_truthy(&Some(json!(1))));
        assert!(is_truthy(&Some(json!("x"))));
        assert!(is_truthy(&Some(json!([1]))));
        assert!(is_truthy(&Some(json!({"x":1}))));
    }

    #[test]
    fn and_or_return_short_circuit_argument() {
        let args = vec![Some(json!(1)), Some(json!(0)), Some(json!(2))];
        assert_eq!(builtin_and(&args), Some(json!(0)));
        assert_eq!(builtin_or(&args), Some(json!(1)));

        let all_truthy = vec![Some(json!("x")), Some(json!(1))];
        assert_eq!(builtin_and(&all_truthy), Some(json!(1)));
        assert_eq!(builtin_or(&all_truthy), Some(json!("x")));
    }

    #[test]
    fn truthiness_for_typed_nil_and_non_empty_collections() {
        let nil_map = crate::gotemplates::encode_go_typed_map_value("string", None);
        let mut entries = serde_json::Map::new();
        entries.insert(String::from("k"), json!("v"));
        let map = crate::gotemplates::encode_go_typed_map_value("string", Some(entries));
        let nil_slice = crate::gotemplates::encode_go_typed_slice_value("int", None);
        let slice =
            crate::gotemplates::encode_go_typed_slice_value("int", Some(vec![json!(1)]));

        assert!(!is_truthy(&Some(nil_map)));
        assert!(is_truthy(&Some(map)));
        assert!(!is_truthy(&Some(nil_slice)));
        assert!(is_truthy(&Some(slice)));
    }
}
