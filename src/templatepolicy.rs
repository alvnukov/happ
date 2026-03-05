pub fn is_supported_library_include(name: &str) -> bool {
    name.starts_with("fl.")
        || name.starts_with("_fl.")
        || name.starts_with("apps-")
        || name.starts_with("apps.")
}

pub fn is_user_allowed_include(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|raw| {
        let pattern = raw.trim();
        if pattern.is_empty() {
            return false;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            name == pattern
        }
    })
}

pub fn is_supported_include(name: &str, allow_patterns: &[String]) -> bool {
    is_supported_library_include(name) || is_user_allowed_include(name, allow_patterns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_library_include_matches_known_prefixes() {
        for ok in [
            "fl.a",
            "_fl.b",
            "apps-thing.render",
            "apps.utils",
            "apps-service",
        ] {
            assert!(is_supported_library_include(ok), "{ok}");
        }
        assert!(!is_supported_library_include("custom.helper"));
    }

    #[test]
    fn user_allowed_include_supports_exact_and_prefix_patterns() {
        let patterns = vec![
            " opensearch-cluster.* ".to_string(),
            "custom.helper".to_string(),
            "".to_string(),
        ];
        assert!(is_user_allowed_include(
            "opensearch-cluster.cluster-name",
            &patterns
        ));
        assert!(is_user_allowed_include("custom.helper", &patterns));
        assert!(!is_user_allowed_include("custom.helper.v2", &patterns));
    }

    #[test]
    fn supported_include_combines_library_and_user_allowlist() {
        let patterns = vec!["custom.*".to_string()];
        assert!(is_supported_include("apps-utils.init-library", &patterns));
        assert!(is_supported_include("custom.one", &patterns));
        assert!(!is_supported_include("unknown.one", &patterns));
    }
}
