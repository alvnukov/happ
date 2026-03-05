#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateFunctionClass {
    SprigV1Subset,
    HelmExtra,
    HelmLateBound,
    DisabledByHelm,
    Unknown,
}

// Curated subset for happ renderer v1.
const SPRIG_V1_FUNCTIONS: &[&str] = &[
    "add",
    "append",
    "cat",
    "coalesce",
    "compact",
    "concat",
    "contains",
    "default",
    "dict",
    "dig",
    "div",
    "empty",
    "first",
    "get",
    "hasKey",
    "indent",
    "join",
    "keys",
    "kindIs",
    "kindOf",
    "last",
    "list",
    "lower",
    "merge",
    "mergeOverwrite",
    "mul",
    "nindent",
    "pluck",
    "quote",
    "repeat",
    "replace",
    "rest",
    "set",
    "split",
    "splitList",
    "squote",
    "sub",
    "ternary",
    "title",
    "toString",
    "trim",
    "trimAll",
    "trimPrefix",
    "trimSuffix",
    "trunc",
    "uniq",
    "unset",
    "upper",
    "values",
];

// Added by Helm engine funcMap() in addition to TxtFuncMap().
const HELM_EXTRA_FUNCTIONS: &[&str] = &[
    "fromJson",
    "fromJsonArray",
    "fromToml",
    "fromYaml",
    "fromYamlArray",
    "lookup",
    "mustToJson",
    "mustToYaml",
    "required",
    "toJson",
    "toToml",
    "toYaml",
    "toYamlPretty",
];

// Late-bound by Helm during render init.
const HELM_LATE_BOUND_FUNCTIONS: &[&str] = &["include", "tpl"];

// Deleted by Helm from sprig map for security reasons.
const HELM_DISABLED_SPRIG_FUNCTIONS: &[&str] = &["env", "expandenv"];

pub fn sprig_v1_functions() -> &'static [&'static str] {
    SPRIG_V1_FUNCTIONS
}

pub fn helm_extra_functions() -> &'static [&'static str] {
    HELM_EXTRA_FUNCTIONS
}

pub fn helm_late_bound_functions() -> &'static [&'static str] {
    HELM_LATE_BOUND_FUNCTIONS
}

pub fn helm_disabled_sprig_functions() -> &'static [&'static str] {
    HELM_DISABLED_SPRIG_FUNCTIONS
}

pub fn classify_template_function(name: &str) -> TemplateFunctionClass {
    if contains(SPRIG_V1_FUNCTIONS, name) {
        return TemplateFunctionClass::SprigV1Subset;
    }
    if contains(HELM_EXTRA_FUNCTIONS, name) {
        return TemplateFunctionClass::HelmExtra;
    }
    if contains(HELM_LATE_BOUND_FUNCTIONS, name) {
        return TemplateFunctionClass::HelmLateBound;
    }
    if contains(HELM_DISABLED_SPRIG_FUNCTIONS, name) {
        return TemplateFunctionClass::DisabledByHelm;
    }
    TemplateFunctionClass::Unknown
}

pub fn is_known_renderer_function(name: &str) -> bool {
    !matches!(
        classify_template_function(name),
        TemplateFunctionClass::Unknown
    )
}

fn contains(haystack: &[&str], needle: &str) -> bool {
    haystack.iter().any(|item| item == &needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reports_helm_extra_and_late_bound() {
        assert_eq!(
            classify_template_function("toYaml"),
            TemplateFunctionClass::HelmExtra
        );
        assert_eq!(
            classify_template_function("include"),
            TemplateFunctionClass::HelmLateBound
        );
    }

    #[test]
    fn classify_reports_disabled_and_unknown() {
        assert_eq!(
            classify_template_function("env"),
            TemplateFunctionClass::DisabledByHelm
        );
        assert_eq!(
            classify_template_function("totallyUnknown"),
            TemplateFunctionClass::Unknown
        );
    }

    #[test]
    fn known_renderer_function_uses_union_of_sets() {
        assert!(is_known_renderer_function("default"));
        assert!(is_known_renderer_function("toYaml"));
        assert!(is_known_renderer_function("tpl"));
        assert!(is_known_renderer_function("expandenv"));
        assert!(!is_known_renderer_function("unknown_fn"));
    }
}
