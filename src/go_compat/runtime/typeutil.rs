use crate::go_compat::typeutil::value_type_name_for_template as go_value_type_name_for_template;
use serde_json::Value;

pub(super) fn value_type_name_for_template(v: &Value) -> String {
    go_value_type_name_for_template(v)
}

#[cfg(test)]
mod tests {
    use super::value_type_name_for_template;
    use crate::go_compat::typeutil::{
        parse_slice_like_index, ParseSliceLikeIndexError, SliceLikeIndexMode,
    };
    use serde_json::json;

    #[test]
    fn parse_slice_like_index_validates_bounds_like_go() {
        let idx = parse_slice_like_index(Some(&json!(1)), 3, SliceLikeIndexMode::Index)
            .expect("must parse");
        assert_eq!(idx, 1);

        let err = parse_slice_like_index(Some(&json!(3)), 3, SliceLikeIndexMode::Index)
            .expect_err("must fail");
        assert_eq!(err, ParseSliceLikeIndexError::OutOfRange { raw: 3 });

        let idx = parse_slice_like_index(Some(&json!(3)), 3, SliceLikeIndexMode::Slice)
            .expect("must parse");
        assert_eq!(idx, 3);
    }

    #[test]
    fn value_type_name_reports_go_typed_shapes() {
        let b = crate::gotemplates::encode_go_bytes_value(&[1, 2]);
        assert_eq!(value_type_name_for_template(&b), "[]uint8");

        let t = crate::gotemplates::encode_go_typed_slice_value("int", Some(vec![json!(1)]));
        assert_eq!(value_type_name_for_template(&t), "[]int");
    }
}
