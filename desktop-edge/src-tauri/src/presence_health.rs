//! Presence-runtime liveness monitor.
//!
//! Polls the [`crate::presence::Liveness`] handle stamped by the
//! reader thread and, on healthy → stalled and stalled → healthy
//! edges, records an audit event so operators can see runtime
//! wedges after the fact. Keeps the monitor as a plain state
//! machine so the transition logic can be unit-tested without a
//! child process — see the tests at the bottom of this module.
//!
//! The alternative would have been to bolt this into the reader
//! thread, but the reader is *event-driven* — it wakes up when
//! bytes arrive on stdout, which is the very signal a stall
//! suppresses. A separate polling loop is what actually gives us
//! the "no bytes for N seconds" edge.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::audit::{AuditKind, AuditLog};
use crate::audit_event_with_identity;
use crate::presence::{Liveness, LivenessSnapshot, Presence, PresenceHealth, SPAWN_GRACE_MS};
use crate::presence_log::PresenceLog;

/// How often the monitor polls the liveness snapshot. Small
/// enough to catch a stall within one heartbeat interval past
/// the threshold, large enough that the poll costs nothing —
/// one mutex lock and a couple of `Duration` compares.
const POLL_INTERVAL: Duration = Duration::from_millis(1_000);

/// Threshold in `Duration` form. Re-exported here rather than
/// referenced inline so the tests can swap the constant without
/// touching `presence_ipc`.
fn stall_threshold() -> Duration {
    Duration::from_millis(presence_ipc::STALL_THRESHOLD_MS)
}

/// Classify a snapshot at `now`. Pulled out of the monitor loop
/// so the state machine can be unit-tested against a fabricated
/// timeline without spinning a thread.
pub fn classify(snap: &LivenessSnapshot, now: Instant) -> PresenceHealth {
    let Some(spawned_at) = snap.spawned_at else {
        // A snapshot without a spawn timestamp is a disabled
        // presence — the monitor exits immediately in that case,
        // but we still return a well-defined variant for the
        // caller's edge detection.
        return PresenceHealth::Disabled;
    };
    match snap.last_event_at {
        None => {
            // Still waiting on the first event. Anything under
            // the grace window counts as "starting"; past it we
            // upgrade to Stalled so a runtime that never emits
            // Ready is caught.
            if now.duration_since(spawned_at) >= Duration::from_millis(SPAWN_GRACE_MS) {
                PresenceHealth::Stalled
            } else {
                PresenceHealth::Starting
            }
        }
        Some(last) => {
            if now.duration_since(last) >= stall_threshold() {
                PresenceHealth::Stalled
            } else {
                PresenceHealth::Healthy
            }
        }
    }
}

/// State the monitor loop carries between polls. Only exposed
/// for tests; production code interacts through [`spawn`].
#[derive(Debug, Clone, Copy)]
pub struct MonitorState {
    pub health: PresenceHealth,
    /// Wall-clock the current stall started, so a `Recovered`
    /// event can attach `elapsed_ms`. `None` outside a stall.
    pub stall_started_at: Option<Instant>,
}

impl MonitorState {
    pub const fn initial() -> Self {
        Self {
            health: PresenceHealth::Starting,
            stall_started_at: None,
        }
    }
}

/// Edge produced by [`step`]: either nothing changed, a stall
/// began (with the snapshot at the moment of detection), or a
/// stall ended (with the duration it lasted).
#[derive(Debug, Clone)]
pub enum MonitorEdge {
    None,
    Stalled {
        elapsed_ms: u64,
        snapshot: LivenessSnapshot,
    },
    Recovered {
        recovery_ms: u64,
        snapshot: LivenessSnapshot,
    },
}

/// Fold one poll tick into the monitor state. Returns the edge
/// that fired, if any — the caller decides what to do with it
/// (production: write an audit event; tests: assert against).
pub fn step(state: &mut MonitorState, snap: &LivenessSnapshot, now: Instant) -> MonitorEdge {
    let health = classify(snap, now);
    let previous = state.health;
    state.health = health;
    match (previous, health) {
        (PresenceHealth::Stalled, PresenceHealth::Stalled)
        | (PresenceHealth::Starting, PresenceHealth::Starting)
        | (PresenceHealth::Healthy, PresenceHealth::Healthy) => MonitorEdge::None,
        // Any → Stalled: enter the stall window.
        (_, PresenceHealth::Stalled) => {
            state.stall_started_at = Some(now);
            let elapsed = snap
                .last_event_at
                .map(|t| now.duration_since(t).as_millis() as u64)
                .unwrap_or_else(|| {
                    snap.spawned_at
                        .map(|t| now.duration_since(t).as_millis() as u64)
                        .unwrap_or_default()
                });
            MonitorEdge::Stalled {
                elapsed_ms: elapsed,
                snapshot: snap.clone(),
            }
        }
        // Stalled → Healthy: recovery.
        (PresenceHealth::Stalled, PresenceHealth::Healthy) => {
            let started = state.stall_started_at.take();
            let recovery_ms = started
                .map(|t| now.duration_since(t).as_millis() as u64)
                .unwrap_or_default();
            MonitorEdge::Recovered {
                recovery_ms,
                snapshot: snap.clone(),
            }
        }
        // All other transitions are progress but not edge-worthy
        // (Starting → Healthy is the boring happy path, Disabled
        // shouldn't get here because `spawn` exits before the
        // loop).
        _ => MonitorEdge::None,
    }
}

/// Spawn a background thread that monitors the given liveness
/// handle. Exits silently when the presence is disabled (no
/// `spawned_at` set). Emits audit events on stall / recovery
/// edges via the `AuditLog` owned by Tauri state — resolved on
/// each edge rather than captured up-front, so a hot-swap of the
/// managed state (however unlikely) is picked up automatically.
pub fn spawn(app: AppHandle, liveness: Liveness) {
    // Cheap early bail — avoid spawning a thread that will
    // immediately exit for a disabled presence. Non-critical
    // (the loop would exit fine on its own), but keeps thread
    // dumps clean during development.
    if liveness.lock().ok().and_then(|s| s.spawned_at).is_none() {
        log::info!("presence-health: presence disabled (no spawn timestamp) — monitor not started");
        return;
    }
    std::thread::Builder::new()
        .name("presence-health".into())
        .spawn(move || run(app, liveness))
        .ok();
}

fn run(app: AppHandle, liveness: Liveness) {
    let mut state = MonitorState::initial();
    log::info!(
        "presence-health: monitor started (poll {}ms, stall threshold {}ms)",
        POLL_INTERVAL.as_millis(),
        presence_ipc::STALL_THRESHOLD_MS
    );
    loop {
        std::thread::sleep(POLL_INTERVAL);
        // If the app is tearing down, the Tauri state may be
        // gone. `try_state` returns `None` in that case; we
        // exit rather than spin.
        let audit_state = app.try_state::<AuditLog>();
        let snap = match liveness.lock() {
            Ok(g) => g.clone(),
            Err(_) => continue,
        };
        // A `Presence` handle disappearing (managed state
        // dropped during shutdown) is the natural exit signal.
        if app.try_state::<Presence>().is_none() {
            log::info!("presence-health: presence state removed — monitor exiting");
            return;
        }
        let edge = step(&mut state, &snap, Instant::now());
        match edge {
            MonitorEdge::None => continue,
            MonitorEdge::Stalled {
                elapsed_ms,
                snapshot,
            } => {
                log::warn!(
                    "presence-health: STALLED — no events for {}ms (last heartbeat #{:?} at uptime {:?}ms)",
                    elapsed_ms,
                    snapshot.last_heartbeat_sequence,
                    snapshot.last_heartbeat_uptime_ms
                );
                if let Some(audit) = audit_state.as_ref() {
                    // Attach the presence.log path (when
                    // enabled) so an operator reading the audit
                    // trail has a direct pointer to the
                    // correlated stderr. Empty when the log
                    // sink failed to open.
                    let log_path = app
                        .try_state::<std::sync::Arc<PresenceLog>>()
                        .and_then(|s| s.active_path())
                        .map(|p| p.display().to_string());
                    let event = audit_event_with_identity(&app, AuditKind::PresenceStalled)
                        .with_subject("presence-runtime")
                        .with_detail(serde_json::json!({
                            "elapsed_ms": elapsed_ms,
                            "last_heartbeat_sequence": snapshot.last_heartbeat_sequence,
                            "last_heartbeat_uptime_ms": snapshot.last_heartbeat_uptime_ms,
                            "log_path": log_path,
                        }));
                    let _ = audit.write(&event);
                }
            }
            MonitorEdge::Recovered {
                recovery_ms,
                snapshot,
            } => {
                log::info!(
                    "presence-health: RECOVERED — events resumed after {}ms (heartbeat #{:?})",
                    recovery_ms,
                    snapshot.last_heartbeat_sequence
                );
                if let Some(audit) = audit_state.as_ref() {
                    let event = audit_event_with_identity(&app, AuditKind::PresenceRecovered)
                        .with_subject("presence-runtime")
                        .with_detail(serde_json::json!({
                            "recovery_ms": recovery_ms,
                            "last_heartbeat_sequence": snapshot.last_heartbeat_sequence,
                            "last_heartbeat_uptime_ms": snapshot.last_heartbeat_uptime_ms,
                        }));
                    let _ = audit.write(&event);
                }
            }
        }
    }
}

// Force the unused-in-non-test path to be seen as used from the
// monitor spawn — silences a dead_code warning if someone later
// refactors the loop.
#[allow(dead_code)]
fn _keep_arc_reachable(_: &Arc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn snap_from(now: Instant, ago_ms: Option<u64>) -> LivenessSnapshot {
        LivenessSnapshot {
            last_event_at: ago_ms.map(|ms| now - Duration::from_millis(ms)),
            last_heartbeat_sequence: ago_ms.map(|_| 42),
            last_heartbeat_uptime_ms: ago_ms,
            spawned_at: Some(now - Duration::from_millis(60_000)),
        }
    }

    #[test]
    fn disabled_presence_classifies_as_disabled() {
        let snap = LivenessSnapshot::new();
        assert_eq!(classify(&snap, Instant::now()), PresenceHealth::Disabled);
    }

    #[test]
    fn spawned_but_no_events_yet_stays_starting_inside_grace() {
        let now = Instant::now();
        let snap = LivenessSnapshot {
            last_event_at: None,
            last_heartbeat_sequence: None,
            last_heartbeat_uptime_ms: None,
            spawned_at: Some(now - Duration::from_millis(1_000)),
        };
        assert_eq!(classify(&snap, now), PresenceHealth::Starting);
    }

    #[test]
    fn spawned_but_no_events_past_grace_is_stalled() {
        let now = Instant::now();
        let snap = LivenessSnapshot {
            last_event_at: None,
            last_heartbeat_sequence: None,
            last_heartbeat_uptime_ms: None,
            // Well past SPAWN_GRACE_MS.
            spawned_at: Some(now - Duration::from_millis(SPAWN_GRACE_MS + 5_000)),
        };
        assert_eq!(classify(&snap, now), PresenceHealth::Stalled);
    }

    #[test]
    fn fresh_event_is_healthy() {
        let now = Instant::now();
        // Well inside the stall threshold.
        let snap = snap_from(now, Some(500));
        assert_eq!(classify(&snap, now), PresenceHealth::Healthy);
    }

    #[test]
    fn stale_event_is_stalled() {
        let now = Instant::now();
        let snap = snap_from(now, Some(presence_ipc::STALL_THRESHOLD_MS + 1_000));
        assert_eq!(classify(&snap, now), PresenceHealth::Stalled);
    }

    #[test]
    fn step_fires_stalled_edge_once_and_then_stays_quiet() {
        let now = Instant::now();
        let mut state = MonitorState::initial();
        // Warm up healthy first.
        let healthy = snap_from(now, Some(100));
        assert!(matches!(step(&mut state, &healthy, now), MonitorEdge::None));

        // Now go stale — one Stalled edge, then None while we
        // continue to be stalled.
        let stale = snap_from(now, Some(presence_ipc::STALL_THRESHOLD_MS + 500));
        match step(&mut state, &stale, now) {
            MonitorEdge::Stalled { elapsed_ms, .. } => {
                assert!(elapsed_ms >= presence_ipc::STALL_THRESHOLD_MS);
            }
            other => panic!("expected Stalled edge, got {other:?}"),
        }
        match step(&mut state, &stale, now + Duration::from_millis(500)) {
            MonitorEdge::None => {}
            other => panic!("expected no repeat edge, got {other:?}"),
        }
    }

    #[test]
    fn step_fires_recovered_edge_after_stall() {
        let now = Instant::now();
        let mut state = MonitorState::initial();
        // Prime healthy → stalled → healthy.
        let _ = step(&mut state, &snap_from(now, Some(100)), now);
        let _ = step(
            &mut state,
            &snap_from(now, Some(presence_ipc::STALL_THRESHOLD_MS + 500)),
            now,
        );
        let recovered_at = now + Duration::from_millis(2_000);
        match step(&mut state, &snap_from(recovered_at, Some(50)), recovered_at) {
            MonitorEdge::Recovered { recovery_ms, .. } => {
                // Recovery time is measured from the stall's
                // detection (which was `now`), not from the
                // stall's original event boundary.
                assert!(recovery_ms >= 1_500, "recovery_ms too small: {recovery_ms}");
            }
            other => panic!("expected Recovered edge, got {other:?}"),
        }
        // Subsequent healthy poll must be quiet.
        assert!(matches!(
            step(
                &mut state,
                &snap_from(recovered_at, Some(60)),
                recovered_at + Duration::from_millis(1_000)
            ),
            MonitorEdge::None
        ));
    }

    #[test]
    fn flapping_stall_recover_stall_produces_two_stalled_edges() {
        // Belt-and-braces on the edge detection: a runtime that
        // wedges briefly, recovers, then wedges again must
        // produce two distinct Stalled events so the audit log
        // captures each incident.
        let now = Instant::now();
        let mut state = MonitorState::initial();
        let _ = step(&mut state, &snap_from(now, Some(100)), now);
        let stall_a = step(
            &mut state,
            &snap_from(now, Some(presence_ipc::STALL_THRESHOLD_MS + 100)),
            now,
        );
        assert!(matches!(stall_a, MonitorEdge::Stalled { .. }));
        let recover = step(
            &mut state,
            &snap_from(now + Duration::from_millis(1_000), Some(100)),
            now + Duration::from_millis(1_000),
        );
        assert!(matches!(recover, MonitorEdge::Recovered { .. }));
        let stall_b = step(
            &mut state,
            &snap_from(
                now + Duration::from_millis(2_000),
                Some(presence_ipc::STALL_THRESHOLD_MS + 100),
            ),
            now + Duration::from_millis(2_000),
        );
        assert!(matches!(stall_b, MonitorEdge::Stalled { .. }));
    }
}
