//! Bridge from `presence-ipc` wire types to this crate's internal state.
//!
//! Gated on the `ipc` Cargo feature — see this crate's `Cargo.toml` and
//! Phase 2 §2 of `../../../docs/PRESENCE_INTEGRATION_PLAN.md`. Enabled by
//! default; a build without it drops the `presence-ipc` dependency
//! entirely and the [`SceneDirector::apply_command`] entry point.
//!
//! # Why not implement `From` in `presence-ipc`?
//!
//! Because that would force `presence-ipc` to depend on `presence-core`,
//! and `presence-core` needs a GPU/window — which means the ipc crate
//! could no longer build in headless CI. The `presence-ipc` crate is the
//! *canonical* wire format; this module is the reverse map for a single
//! consumer of it.

use presence_ipc::{
    Command, IpcAnchor, IpcDisposition, IpcPlacement, PaletteId as IpcPalette,
    PresenceMode as IpcMode, QualityTier as IpcTier, SceneParamsWire, Signals, MAX_SCENE_ID_LEN,
};

use crate::palette::PaletteId;
use crate::scene::disposition::Disposition;
use crate::scene::mode::PresenceMode;
use crate::scene::params::{SceneParams, PARAM_DENSITY, PARAM_WIND};
use crate::scene::placement::{Anchor, Placement};
use crate::scene::provenance::{Provenance, ProvenanceSource};
use crate::scene::{QualityTier, SceneDirector};
use crate::sim::PresenceSignals;

impl From<IpcMode> for PresenceMode {
    fn from(mode: IpcMode) -> Self {
        match mode {
            IpcMode::Thinking => PresenceMode::Thinking,
            IpcMode::Speaking => PresenceMode::Speaking,
            IpcMode::ToolUse => PresenceMode::ToolUse,
            IpcMode::Listening => PresenceMode::Listening,
            IpcMode::Attention => PresenceMode::Attention,
            IpcMode::Error => PresenceMode::Error,
        }
    }
}

impl From<PresenceMode> for IpcMode {
    fn from(mode: PresenceMode) -> Self {
        match mode {
            PresenceMode::Thinking => IpcMode::Thinking,
            PresenceMode::Speaking => IpcMode::Speaking,
            PresenceMode::ToolUse => IpcMode::ToolUse,
            PresenceMode::Listening => IpcMode::Listening,
            PresenceMode::Attention => IpcMode::Attention,
            PresenceMode::Error => IpcMode::Error,
        }
    }
}

impl From<IpcTier> for QualityTier {
    fn from(tier: IpcTier) -> Self {
        match tier {
            IpcTier::Balanced => QualityTier::Balanced,
            IpcTier::Low => QualityTier::Low,
        }
    }
}

impl From<QualityTier> for IpcTier {
    fn from(tier: QualityTier) -> Self {
        match tier {
            QualityTier::Balanced => IpcTier::Balanced,
            QualityTier::Low => IpcTier::Low,
        }
    }
}

impl From<IpcPalette> for PaletteId {
    fn from(id: IpcPalette) -> Self {
        match id {
            IpcPalette::Teal => PaletteId::Teal,
            IpcPalette::Lime => PaletteId::Lime,
            IpcPalette::Ice => PaletteId::Ice,
            IpcPalette::Ember => PaletteId::Ember,
        }
    }
}

impl From<PaletteId> for IpcPalette {
    fn from(id: PaletteId) -> Self {
        match id {
            PaletteId::Teal => IpcPalette::Teal,
            PaletteId::Lime => IpcPalette::Lime,
            PaletteId::Ice => IpcPalette::Ice,
            PaletteId::Ember => IpcPalette::Ember,
        }
    }
}

impl From<IpcDisposition> for Disposition {
    fn from(d: IpcDisposition) -> Self {
        match d {
            IpcDisposition::Overlay => Disposition::Overlay,
            IpcDisposition::Replace => Disposition::Replace,
        }
    }
}

impl From<IpcAnchor> for Anchor {
    fn from(a: IpcAnchor) -> Self {
        match a {
            IpcAnchor::Center => Anchor::Center,
            IpcAnchor::TopLeft => Anchor::TopLeft,
            IpcAnchor::TopRight => Anchor::TopRight,
            IpcAnchor::BottomLeft => Anchor::BottomLeft,
            IpcAnchor::BottomRight => Anchor::BottomRight,
            IpcAnchor::CloudRelative => Anchor::CloudRelative,
        }
    }
}

fn placement_from_wire(w: IpcPlacement) -> Placement {
    Placement {
        anchor: w.anchor.into(),
        offset: glam::Vec2::new(w.offset[0], w.offset[1]),
        scale: w.scale,
    }
}

fn scene_params_from_wire(w: SceneParamsWire) -> SceneParams {
    let mut params = SceneParams::default();
    params.set(PARAM_DENSITY, w.density);
    params.set(PARAM_WIND, w.wind);
    params
}

/// Copies the scalars out of a wire [`Signals`] into a
/// [`PresenceSignals`]. The `active_modes` field on `Signals` is applied
/// separately (via `SceneDirector::apply_command` and
/// [`Command::SetSignals`]) because the director's mode ownership is on
/// `ModeLayer`, not on the signal struct.
///
/// Scalars are clamped to conservative ranges: the debug UI clamps
/// `intensity` to `[0.0, 1.5]` and the other two to `[0.0, 1.0]`. Values
/// outside those ranges are accepted (nothing panics) but pulled back in
/// so a misbehaving sender cannot drive the shell into a nonsensical
/// state.
fn signals_from_wire(w: &Signals) -> PresenceSignals {
    // NaN passes through `f32::clamp` unchanged, so a naive
    // `w.intensity.clamp(0.0, 1.5)` would let a NaN reach the simulation
    // and quietly corrupt every derived value there. Fold NaN to zero
    // before clamping — a misbehaving sender should not be able to break
    // the presence, only fail to command it.
    let sanitize = |v: f32, lo: f32, hi: f32| {
        if v.is_nan() {
            lo
        } else {
            v.clamp(lo, hi)
        }
    };
    PresenceSignals {
        intensity: sanitize(w.intensity, 0.0, 1.5),
        audio_level: sanitize(w.audio_level, 0.0, 1.0),
        progress: sanitize(w.progress, 0.0, 1.0),
    }
}

impl SceneDirector {
    /// Applies a single [`Command`] from the shell. Every command is a
    /// small, idempotent mutation — a stream of duplicates has the same
    /// end state as one, and re-ordering only matters between
    /// [`Command::SetSignals`] messages that carry different `active_modes`.
    ///
    /// # Semantics of `SetSignals`
    ///
    /// The `active_modes` field is treated as the shell's *authoritative*
    /// desired set: every mode in the list is engaged, every mode *not*
    /// in the list is released. This is deliberately different from the
    /// per-mode [`Command::SetMode`] path, because the shell often sends
    /// signal packets from a snapshot of its own state and expects the
    /// presence to converge on that snapshot rather than accumulate
    /// engaged modes over time.
    pub fn apply_command(&mut self, command: Command) {
        match command {
            Command::SetSignals(w) => {
                self.signals = signals_from_wire(&w);
                // Snapshot semantics — see the docs on this method.
                for mode in PresenceMode::ALL {
                    let wanted = w
                        .active_modes
                        .iter()
                        .any(|m| PresenceMode::from(*m) == mode);
                    self.modes.set(mode, wanted);
                }
            }
            Command::SetSignalsScalars {
                intensity,
                audio_level,
                progress,
            } => {
                // Scalars-only path. Same NaN-fold + clamp as SetSignals,
                // but ModeLayer is untouched — this is the message the
                // mic pump fires at ~30 Hz, and letting it churn mode
                // engagement would be exactly the collision the two
                // commands exist to keep separate.
                let sanitize = |v: f32, lo: f32, hi: f32| {
                    if v.is_nan() {
                        lo
                    } else {
                        v.clamp(lo, hi)
                    }
                };
                self.signals = PresenceSignals {
                    intensity: sanitize(intensity, 0.0, 1.5),
                    audio_level: sanitize(audio_level, 0.0, 1.0),
                    progress: sanitize(progress, 0.0, 1.0),
                };
            }
            Command::SetMode { mode, engaged } => {
                self.modes.set(mode.into(), engaged);
            }
            Command::SetRingWanted { wanted } => {
                self.set_ring_wanted(wanted);
            }
            Command::SetReducedMotion { enabled } => {
                self.reduced_motion = enabled;
            }
            Command::SetQualityTier { tier } => {
                self.set_quality_tier(tier.into());
            }
            Command::SetPosition { x, y } => {
                // Same one-shot pattern as hittest — the runtime
                // owns the window handle. Overwriting a queued
                // position is intentional: if the shell sends two
                // moves back-to-back only the latest matters.
                self.pending_position = Some((x, y));
            }
            Command::SetInteractive { interactive } => {
                // Same pattern as `SetPalette` — the runtime is the
                // side that owns the `winit::Window`, so we just
                // record the desired state and let the runtime
                // apply it between frames.
                self.pending_hittest = Some(interactive);
            }
            Command::SetPalette { palette } => {
                // The palette is a *render* setting, not a director one —
                // it lives on `Renderer::palette`. The director records
                // the wanted id so the runtime can read it back and apply
                // it to the renderer on the next frame. Storing it on the
                // director keeps `apply_command` self-contained.
                self.pending_palette = Some(palette.into());
            }
            Command::PresentScene {
                id,
                params,
                disposition,
                placement,
                transition_secs: _,
                ttl_ms,
            } => {
                if id.len() > MAX_SCENE_ID_LEN {
                    log::warn!(
                        "present_scene: id length {} exceeds cap {MAX_SCENE_ID_LEN}",
                        id.len()
                    );
                    return;
                }
                let mut scene_params = scene_params_from_wire(params);
                if let Some(template) = self.registry.get(&id) {
                    scene_params.clamp_to(&template.param_schema);
                }
                let ttl = ttl_ms.map(|ms| ms as f32 / 1000.0);
                let provenance = Provenance {
                    source: ProvenanceSource::Ipc,
                };
                if !self.present_scene(
                    &id,
                    scene_params,
                    disposition.into(),
                    placement_from_wire(placement),
                    ttl,
                    provenance,
                ) {
                    log::warn!("present_scene: director rejected id {id}");
                }
            }
            Command::DismissScene { id } => {
                if id.len() > MAX_SCENE_ID_LEN {
                    return;
                }
                if !self.dismiss_scene(&id) {
                    log::warn!("dismiss_scene: no live scene with id {id}");
                }
            }
            other => {
                log::warn!(
                    "presence-core: ignoring unknown ipc command {:?} \
                     (wire is newer than this build)",
                    other
                );
            }
        }
    }

    /// Consumes and returns any palette change the shell has requested via
    /// [`Command::SetPalette`]. The runtime calls this between ticks and
    /// applies the returned id to `Renderer::palette`.
    pub fn take_pending_palette(&mut self) -> Option<PaletteId> {
        self.pending_palette.take()
    }

    /// Consumes and returns any interactivity change requested via
    /// [`Command::SetInteractive`]. `true` means "grab clicks",
    /// `false` means "pass them through". The runtime calls this
    /// between ticks and applies the value via
    /// `winit::Window::set_cursor_hittest`.
    pub fn take_pending_hittest(&mut self) -> Option<bool> {
        self.pending_hittest.take()
    }

    /// Consumes and returns any window-position change requested via
    /// [`Command::SetPosition`]. Physical screen pixels, top-left
    /// corner — the same units `WindowEvent::Moved` reports.
    pub fn take_pending_position(&mut self) -> Option<(i32, i32)> {
        self.pending_position.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use presence_ipc::Signals as WireSignals;

    #[test]
    fn set_signals_engages_the_listed_modes_and_releases_the_rest() {
        let mut director = SceneDirector::new();
        director.set_mode(PresenceMode::Thinking, true);
        assert!(director.modes.is_engaged(PresenceMode::Thinking));

        // The wire packet mentions only Speaking — Thinking should drop.
        director.apply_command(Command::SetSignals(WireSignals {
            intensity: 0.6,
            audio_level: 0.4,
            progress: 0.0,
            active_modes: vec![IpcMode::Speaking],
        }));

        assert!(!director.modes.is_engaged(PresenceMode::Thinking));
        assert!(director.modes.is_engaged(PresenceMode::Speaking));
        assert!((director.signals.intensity - 0.6).abs() < 1e-6);
        assert!((director.signals.audio_level - 0.4).abs() < 1e-6);
    }

    #[test]
    fn out_of_range_scalars_get_clamped_rather_than_panic() {
        let mut director = SceneDirector::new();
        director.apply_command(Command::SetSignals(WireSignals {
            intensity: 10.0,
            audio_level: -1.0,
            progress: f32::NAN,
            active_modes: vec![],
        }));
        assert!(director.signals.intensity <= 1.5);
        assert!(director.signals.audio_level >= 0.0);
        // NaN is folded to zero before clamping — `f32::clamp` on its own
        // would happily pass NaN through, which would corrupt every
        // derived value downstream. `signals_from_wire` guards against it.
        assert!(!director.signals.progress.is_nan());
        assert_eq!(director.signals.progress, 0.0);
    }

    #[test]
    fn set_mode_toggles_a_single_mode_without_touching_others() {
        let mut director = SceneDirector::new();
        director.set_mode(PresenceMode::Speaking, true);
        director.apply_command(Command::SetMode {
            mode: IpcMode::Thinking,
            engaged: true,
        });
        assert!(director.modes.is_engaged(PresenceMode::Speaking));
        assert!(director.modes.is_engaged(PresenceMode::Thinking));
    }

    #[test]
    fn set_quality_tier_regenerates_via_the_director() {
        let mut director = SceneDirector::new();
        let before = director.assistant_cloud.particles.len();
        director.apply_command(Command::SetQualityTier { tier: IpcTier::Low });
        assert_eq!(director.tier(), QualityTier::Low);
        let after = director.assistant_cloud.particles.len();
        assert_ne!(before, after, "point set should have been regenerated");
    }

    #[test]
    fn set_signals_scalars_updates_scalars_and_leaves_modes_alone() {
        let mut director = SceneDirector::new();
        director.set_mode(PresenceMode::Thinking, true);
        director.apply_command(Command::SetSignalsScalars {
            intensity: 0.6,
            audio_level: 0.4,
            progress: 0.0,
        });
        assert!((director.signals.audio_level - 0.4).abs() < 1e-6);
        // The whole point of this variant: no mode churn.
        assert!(director.modes.is_engaged(PresenceMode::Thinking));
    }

    #[test]
    fn set_signals_scalars_sanitises_nan_and_out_of_range_like_set_signals() {
        let mut director = SceneDirector::new();
        director.apply_command(Command::SetSignalsScalars {
            intensity: 999.0,
            audio_level: f32::NAN,
            progress: -1.0,
        });
        assert!(director.signals.intensity <= 1.5);
        assert!(!director.signals.audio_level.is_nan());
        assert!(director.signals.progress >= 0.0);
    }

    #[test]
    fn set_interactive_shows_up_in_take_pending_hittest_once() {
        let mut director = SceneDirector::new();
        assert_eq!(director.take_pending_hittest(), None);
        director.apply_command(Command::SetInteractive { interactive: true });
        assert_eq!(director.take_pending_hittest(), Some(true));
        // Idempotent take: a runtime that reads twice in the same
        // frame must not apply the change twice or misinterpret a
        // stale value as a fresh request.
        assert_eq!(director.take_pending_hittest(), None);
    }

    #[test]
    fn set_position_shows_up_in_take_pending_position_once() {
        let mut director = SceneDirector::new();
        director.apply_command(Command::SetPosition { x: 250, y: 400 });
        // A second, later position wins — the runtime only needs the
        // freshest one, and the wire may deliver two moves in the same
        // frame during a fast drag.
        director.apply_command(Command::SetPosition { x: 260, y: 410 });
        assert_eq!(director.take_pending_position(), Some((260, 410)));
        assert_eq!(director.take_pending_position(), None);
    }

    #[test]
    fn set_palette_shows_up_in_take_pending_palette() {
        let mut director = SceneDirector::new();
        assert!(director.take_pending_palette().is_none());
        director.apply_command(Command::SetPalette {
            palette: IpcPalette::Ember,
        });
        assert_eq!(director.take_pending_palette(), Some(PaletteId::Ember));
        assert!(director.take_pending_palette().is_none());
    }

    #[test]
    fn present_and_dismiss_scene_round_trip_via_apply_command() {
        use crate::scene::registry::TEST_SCENE_ID;
        use presence_ipc::{Command, IpcAnchor, IpcDisposition, IpcPlacement, SceneParamsWire};

        let mut director = SceneDirector::new();
        director.apply_command(Command::PresentScene {
            id: TEST_SCENE_ID.to_string(),
            params: SceneParamsWire {
                density: 0.9,
                wind: 0.2,
            },
            disposition: IpcDisposition::Overlay,
            placement: IpcPlacement {
                anchor: IpcAnchor::BottomRight,
                offset: [0.0, 0.0],
                scale: 0.4,
            },
            transition_secs: None,
            ttl_ms: None,
        });
        assert_eq!(director.live_scenes.len(), 1);
        director.apply_command(Command::DismissScene {
            id: TEST_SCENE_ID.to_string(),
        });
        assert!(director.live_scenes[0].dismiss_pending);
    }
}
