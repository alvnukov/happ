use super::collect_action_spans;
use super::functions::collect_function_calls_in_action;
use crate::go_compat::parserbridge::{parse_action_compat, ControlAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityTier {
    NativeFastPath,
    HelmParityRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatibilityReason {
    ControlFlow,
    HelmTemplateInvocation,
    UnknownFunction(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub tier: CompatibilityTier,
    pub reasons: Vec<CompatibilityReason>,
}

pub fn plan_template_execution(src: &str) -> ExecutionPlan {
    let mut reasons = Vec::new();

    for span in collect_action_spans(src) {
        let action = &src[span.start..span.end];

        if let Ok(ctrl) = parse_action_compat(action, span.start) {
            if !matches!(ctrl, ControlAction::None) {
                reasons.push(CompatibilityReason::ControlFlow);
            }
        } else {
            reasons.push(CompatibilityReason::HelmTemplateInvocation);
        }

        for fn_name in collect_function_calls_in_action(action) {
            let lname = fn_name.to_ascii_lowercase();
            if matches!(
                lname.as_str(),
                "include" | "tpl" | "required" | "lookup" | "fail" | "template" | "block"
            ) {
                reasons.push(CompatibilityReason::HelmTemplateInvocation);
                continue;
            }
            if !crate::sprigset::is_known_renderer_function(&fn_name) {
                reasons.push(CompatibilityReason::UnknownFunction(fn_name));
            }
        }
    }

    let mut uniq = Vec::new();
    for reason in reasons {
        if !uniq.contains(&reason) {
            uniq.push(reason);
        }
    }

    let tier = if uniq.is_empty() {
        CompatibilityTier::NativeFastPath
    } else {
        CompatibilityTier::HelmParityRequired
    };

    ExecutionPlan {
        tier,
        reasons: uniq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_uses_native_for_simple_values_template() {
        let plan = plan_template_execution(r#"{{ default "x" .Values.name | quote }}"#);
        assert_eq!(plan.tier, CompatibilityTier::NativeFastPath);
        assert!(plan.reasons.is_empty());
    }

    #[test]
    fn planner_requires_helm_for_control_flow() {
        let plan = plan_template_execution(r#"{{ if .Values.enabled }}x{{ end }}"#);
        assert_eq!(plan.tier, CompatibilityTier::HelmParityRequired);
        assert!(plan.reasons.contains(&CompatibilityReason::ControlFlow));
    }

    #[test]
    fn planner_requires_helm_for_include_tpl() {
        let plan = plan_template_execution(r#"{{ include "x" . }} {{ tpl .Values.raw . }}"#);
        assert_eq!(plan.tier, CompatibilityTier::HelmParityRequired);
        assert!(plan
            .reasons
            .contains(&CompatibilityReason::HelmTemplateInvocation));
    }
}
