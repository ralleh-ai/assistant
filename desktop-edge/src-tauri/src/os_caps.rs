//! OS capability IPC helpers — policy gate then trait (T13).
//!
//! Phase 1 ships a bootstrap allow rule for `os.clipboard.*` so the smoke
//! path works offline. Real policy will come from the control plane later.

use ralleh_os_capabilities::Clipboard;
#[cfg(not(feature = "clipboard-os"))]
use ralleh_os_capabilities::MockClipboard;
use ralleh_policy_core::{
    PolicyDecision, PolicyEngine, PolicyOutcome, PolicyRequest, PolicyRule, RuleEffect,
};
use serde::Serialize;

use crate::settings::EdgeSettings;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSmokeResult {
    pub backend: String,
    pub written: String,
    pub read_back: String,
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    pub policy_outcome: String,
    pub policy_rule_id: Option<String>,
    pub policy_reason: String,
}

fn phase1_clipboard_engine() -> PolicyEngine {
    PolicyEngine::new(vec![PolicyRule {
        id: "phase1.os.clipboard.allow".into(),
        tenant_id: None,
        device_id: None,
        actor_id: None,
        capability_prefix: Some("os.clipboard.".into()),
        sensitivity: None,
        effect: RuleEffect::Allow,
        reason: "Phase 1 bootstrap: local clipboard smoke until control-plane policy lands".into(),
    }])
}

fn require_allowed(decision: &PolicyDecision) -> Result<(), String> {
    match decision.outcome {
        PolicyOutcome::Allowed => Ok(()),
        PolicyOutcome::Denied => Err(format!("policy denied: {}", decision.reason)),
        PolicyOutcome::ApprovalRequired => {
            Err(format!("policy requires approval: {}", decision.reason))
        }
    }
}

pub fn run_clipboard_smoke(settings: &EdgeSettings) -> Result<ClipboardSmokeResult, String> {
    let engine = phase1_clipboard_engine();
    let write_req = PolicyRequest::new(
        &settings.tenant_id,
        &settings.device_id,
        &settings.actor_id,
        "os.clipboard.write",
        "internal",
    )
    .map_err(|e| e.to_string())?;
    let write_decision = engine.evaluate(&write_req);
    require_allowed(&write_decision)?;

    let read_req = PolicyRequest::new(
        &settings.tenant_id,
        &settings.device_id,
        &settings.actor_id,
        "os.clipboard.read",
        "internal",
    )
    .map_err(|e| e.to_string())?;
    let read_decision = engine.evaluate(&read_req);
    require_allowed(&read_decision)?;

    let marker = format!(
        "ralleh-clipboard-smoke:{}:{}",
        settings.device_id, settings.actor_id
    );

    #[cfg(feature = "clipboard-os")]
    let clip: Box<dyn Clipboard> = Box::new(ralleh_os_capabilities::SystemClipboard::new());
    #[cfg(not(feature = "clipboard-os"))]
    let clip: Box<dyn Clipboard> = Box::new(MockClipboard::new());

    clip.write_text(&marker)
        .map_err(|e| format!("clipboard write: {e}"))?;
    let read_back = clip
        .read_text()
        .map_err(|e| format!("clipboard read: {e}"))?;

    Ok(ClipboardSmokeResult {
        backend: clip.backend_id().into(),
        written: marker,
        read_back,
        tenant_id: settings.tenant_id.clone(),
        device_id: settings.device_id.clone(),
        actor_id: settings.actor_id.clone(),
        policy_outcome: match read_decision.outcome {
            PolicyOutcome::Allowed => "allowed".into(),
            PolicyOutcome::Denied => "denied".into(),
            PolicyOutcome::ApprovalRequired => "approvalRequired".into(),
        },
        policy_rule_id: read_decision.matched_rule_id.clone(),
        policy_reason: read_decision.reason.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::EdgeSettings;

    #[test]
    fn mock_clipboard_smoke_passes_policy() {
        let settings = EdgeSettings::default();
        let result = run_clipboard_smoke(&settings).unwrap();
        assert_eq!(result.backend, "mock");
        assert_eq!(result.read_back, result.written);
        assert!(result.written.contains("desktop-1"));
    }
}
