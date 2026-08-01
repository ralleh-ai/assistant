use serde::{Deserialize, Serialize};

/// The effect a matching rule has on a request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleEffect {
    Allow,
    Deny,
    RequireApproval,
}

/// A single declarative policy rule.
///
/// Matching semantics: a rule matches a request if every non-`None` field on
/// the rule equals (or, for `capability_prefix`, prefixes) the corresponding
/// field on the request. `None` fields are wildcards.
///
/// Evaluation order matters: rules are evaluated in the order provided to
/// the engine, and the **first matching rule wins**. This mirrors firewall/
/// ACL semantics that enterprise security teams already understand, and
/// keeps evaluation a simple, auditable, linear scan rather than a
/// "most specific wins" heuristic that's harder to reason about at audit
/// time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Stable identifier for this rule, surfaced in decisions/audit logs.
    pub id: String,
    /// Restrict this rule to a specific tenant; `None` = any tenant.
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Restrict this rule to a specific device; `None` = any device.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Restrict this rule to a specific actor; `None` = any actor.
    #[serde(default)]
    pub actor_id: Option<String>,
    /// Match requests whose `capability` starts with this prefix.
    /// e.g. "tool.github." matches "tool.github.create_issue".
    #[serde(default)]
    pub capability_prefix: Option<String>,
    /// Restrict this rule to a specific sensitivity label; `None` = any.
    #[serde(default)]
    pub sensitivity: Option<String>,
    /// What happens when this rule matches.
    pub effect: RuleEffect,
    /// Human-readable justification, required for every rule so that policy
    /// files stay self-documenting and reviewable (enterprise auditors will
    /// ask "why does this rule exist").
    pub reason: String,
}

impl PolicyRule {
    pub fn matches(&self, req: &crate::PolicyRequest) -> bool {
        if let Some(t) = &self.tenant_id {
            if t != &req.tenant_id {
                return false;
            }
        }
        if let Some(d) = &self.device_id {
            if d != &req.device_id {
                return false;
            }
        }
        if let Some(a) = &self.actor_id {
            if a != &req.actor_id {
                return false;
            }
        }
        if let Some(prefix) = &self.capability_prefix {
            if !req.capability.starts_with(prefix.as_str()) {
                return false;
            }
        }
        if let Some(s) = &self.sensitivity {
            if s != &req.sensitivity {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PolicyRequest;

    fn req() -> PolicyRequest {
        PolicyRequest::new(
            "tenant-1",
            "device-1",
            "user-1",
            "tool.github.create_issue",
            "internal",
        )
        .unwrap()
    }

    #[test]
    fn wildcard_rule_matches_everything() {
        let rule = PolicyRule {
            id: "r1".into(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: None,
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".into(),
        };
        assert!(rule.matches(&req()));
    }

    #[test]
    fn tenant_mismatch_does_not_match() {
        let rule = PolicyRule {
            id: "r1".into(),
            tenant_id: Some("other-tenant".into()),
            device_id: None,
            actor_id: None,
            capability_prefix: None,
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".into(),
        };
        assert!(!rule.matches(&req()));
    }

    #[test]
    fn capability_prefix_match_works() {
        let rule = PolicyRule {
            id: "r1".into(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.github.".into()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".into(),
        };
        assert!(rule.matches(&req()));
    }

    #[test]
    fn capability_prefix_mismatch_fails() {
        let rule = PolicyRule {
            id: "r1".into(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.slack.".into()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".into(),
        };
        assert!(!rule.matches(&req()));
    }

    #[test]
    fn sensitivity_mismatch_fails() {
        let rule = PolicyRule {
            id: "r1".into(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: None,
            sensitivity: Some("regulated".into()),
            effect: RuleEffect::Allow,
            reason: "test".into(),
        };
        assert!(!rule.matches(&req()));
    }
}
