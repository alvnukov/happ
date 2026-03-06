use crate::gotemplates::{MissingValueMode, NativeRenderOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingKeyOption {
    Default,
    Invalid,
    Zero,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateOptions {
    pub missing_key: MissingKeyOption,
}

impl Default for TemplateOptions {
    fn default() -> Self {
        Self {
            missing_key: MissingKeyOption::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateOptionError {
    pub message: String,
}

impl std::fmt::Display for TemplateOptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TemplateOptionError {}

impl TemplateOptions {
    pub fn apply(&mut self, spec: &str) -> Result<(), TemplateOptionError> {
        let trimmed = spec.trim();
        let Some(value) = trimmed.strip_prefix("missingkey=") else {
            return Err(TemplateOptionError {
                message: format!("unsupported option: {trimmed}"),
            });
        };
        self.missing_key = match value {
            "default" => MissingKeyOption::Default,
            "invalid" => MissingKeyOption::Invalid,
            "zero" => MissingKeyOption::Zero,
            "error" => MissingKeyOption::Error,
            other => {
                return Err(TemplateOptionError {
                    message: format!("unknown missingkey option: {other}"),
                });
            }
        };
        Ok(())
    }

    pub fn to_native_render_options(self) -> NativeRenderOptions {
        let missing_value_mode = match self.missing_key {
            MissingKeyOption::Error => MissingValueMode::Error,
            MissingKeyOption::Zero => MissingValueMode::GoZero,
            MissingKeyOption::Default | MissingKeyOption::Invalid => MissingValueMode::GoDefault,
        };
        NativeRenderOptions { missing_value_mode }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_missingkey_error_maps_to_error_mode() {
        let mut options = TemplateOptions::default();
        options
            .apply("missingkey=error")
            .expect("option apply must succeed");
        let native = options.to_native_render_options();
        assert_eq!(native.missing_value_mode, MissingValueMode::Error);
    }

    #[test]
    fn option_missingkey_default_like_values_map_to_go_default() {
        for raw in ["missingkey=default", "missingkey=invalid"] {
            let mut options = TemplateOptions::default();
            options.apply(raw).expect("option apply must succeed");
            let native = options.to_native_render_options();
            assert_eq!(native.missing_value_mode, MissingValueMode::GoDefault);
        }
    }

    #[test]
    fn option_missingkey_zero_maps_to_zero_mode() {
        let mut options = TemplateOptions::default();
        options
            .apply("missingkey=zero")
            .expect("option apply must succeed");
        let native = options.to_native_render_options();
        assert_eq!(native.missing_value_mode, MissingValueMode::GoZero);
    }
}
