//! Live microphone IPC — policy + station-log clearance, then audio-core.

use ralleh_audio_core::{run_live_mic_smoke, LiveMicSmokeResult};
use ralleh_policy_core::{PolicyEngine, PolicyOutcome, PolicyRequest, PolicyRule, RuleEffect};
use serde::Serialize;

use crate::settings::EdgeSettings;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicSmokeResult {
    pub sample_rate_hz: u32,
    pub duration_ms: u32,
    pub frames: u32,
    pub samples: usize,
    pub peak_rms: f32,
    pub max_abs: f32,
    pub mic_feature: bool,
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    pub policy_outcome: String,
    pub policy_rule_id: Option<String>,
}

fn phase1_mic_engine() -> PolicyEngine {
    PolicyEngine::new(vec![PolicyRule {
        id: "phase1.os.mic.allow".into(),
        tenant_id: None,
        device_id: None,
        actor_id: None,
        capability_prefix: Some("os.mic.".into()),
        sensitivity: None,
        effect: RuleEffect::Allow,
        reason: "Phase 1 bootstrap: local mic smoke until control-plane policy lands".into(),
    }])
}

pub fn mic_feature_enabled() -> bool {
    cfg!(feature = "mic")
}

pub fn run_mic_smoke(settings: &EdgeSettings, seconds: f32) -> Result<MicSmokeResult, String> {
    if !settings.mic_acknowledged {
        return Err(
            "mic clearance not stamped — open the station log (Voice) and acknowledge OS mic guidance first"
                .into(),
        );
    }

    let engine = phase1_mic_engine();
    let req = PolicyRequest::new(
        &settings.tenant_id,
        &settings.device_id,
        &settings.actor_id,
        "os.mic.capture",
        "internal",
    )
    .map_err(|e| e.to_string())?;
    let decision = engine.evaluate(&req);
    match decision.outcome {
        PolicyOutcome::Allowed => {}
        PolicyOutcome::Denied => return Err(format!("policy denied: {}", decision.reason)),
        PolicyOutcome::ApprovalRequired => {
            return Err(format!("policy requires approval: {}", decision.reason))
        }
    }

    let capture: LiveMicSmokeResult = run_live_mic_smoke(seconds)?;

    Ok(MicSmokeResult {
        sample_rate_hz: capture.sample_rate_hz,
        duration_ms: capture.duration_ms,
        frames: capture.frames,
        samples: capture.samples,
        peak_rms: capture.peak_rms,
        max_abs: capture.max_abs,
        mic_feature: mic_feature_enabled(),
        tenant_id: settings.tenant_id.clone(),
        device_id: settings.device_id.clone(),
        actor_id: settings.actor_id.clone(),
        policy_outcome: "allowed".into(),
        policy_rule_id: decision.matched_rule_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_without_mic_clearance() {
        let settings = EdgeSettings::default();
        let err = run_mic_smoke(&settings, 0.5).unwrap_err();
        assert!(err.contains("clearance"), "{err}");
    }

    #[test]
    fn with_clearance_fails_cleanly_without_mic_feature() {
        let mut settings = EdgeSettings::default();
        settings.mic_acknowledged = true;
        let result = run_mic_smoke(&settings, 0.5);
        // Default edge build: no mic feature → clear rebuild message.
        // With mic feature under CI skip: also Err, never panic.
        assert!(result.is_err());
    }
}
