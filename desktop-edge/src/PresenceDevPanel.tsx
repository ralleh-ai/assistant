// Small dev panel that lets a developer toggle presence modes without
// a real assistant flow driving them. Renders only when the Tauri side
// reports `presence_status.enabled === true`, so a machine that hasn't
// set `RALLEH_PRESENCE_BIN` never sees it. Not user-facing UI — the
// eventual mapping is signal → mode automatically, driven by real
// assistant state.
//
// Kept in its own component (rather than inlined into `Core.tsx`)
// because it is scaffolding: when signals are wired up in a later
// commit this file will either disappear or move behind an explicit
// `?debug=1` query flag. Isolating it now keeps that cleanup a
// single-file diff.

import { useCallback, useEffect, useRef, useState } from "react";
import {
    PRESENCE_MODES,
    presenceMicStart,
    presenceMicStatus,
    presenceMicStop,
    presenceSetMode,
    presenceSetPalette,
    presenceSetReducedMotion,
    presenceSetRingWanted,
    presenceSetSignals,
    PaletteId,
    PALETTES,
    PresenceMode,
} from "./presence";

// Send at most one signals packet per this many milliseconds. The
// runtime takes SetSignals at whatever rate it arrives, but every
// packet is a Tauri round-trip + a JSON encode + a pipe write, and
// firing on every slider `input` event would coalesce badly on slow
// hardware. 30 Hz matches the cadence the eventual mic pump will use
// and is fine-grained enough that a dragging slider feels continuous.
const SIGNALS_MIN_INTERVAL_MS = 33;

export function PresenceDevPanel() {
  // Presence engagement state is authoritative in the runtime — we do
  // not read it back, we just track what *this* UI has asked for so the
  // toggle button reflects the last request. If the runtime is behaving
  // and no other process is sending commands, the two match.
  const [engaged, setEngaged] = useState<Set<PresenceMode>>(new Set());
  const [ring, setRing] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const [palette, setPalette] = useState<PaletteId>("teal");

  // Continuous signals: mirrors `PresenceSignals::default()` on the
  // runtime side. `0.15` intensity is the idle baseline the shell was
  // tuned against; a fresh session starts there rather than at zero so
  // the entity has visible life before the user touches anything.
  const [intensity, setIntensity] = useState(0.15);
  const [audioLevel, setAudioLevel] = useState(0.0);
  const [progress, setProgress] = useState(0.0);

    // Live-mic pump. Off until the operator clicks the toggle — mic
    // capture without an explicit gesture would violate the same
    // clearance policy the smoke button enforces. `micError` surfaces
    // start failures (no clearance, no device, presence not spawned)
    // beside the toggle rather than silently.
    const [micRunning, setMicRunning] = useState(false);
    const [micAvailable, setMicAvailable] = useState(false);
    const [micError, setMicError] = useState<string | null>(null);
    useEffect(() => {
        let cancelled = false;
        presenceMicStatus().then((s) => {
            if (cancelled) return;
            setMicRunning(s.running);
            setMicAvailable(s.micFeature);
        });
        return () => {
            cancelled = true;
        };
    }, []);

    const onMicToggle = useCallback(async () => {
        setMicError(null);
        try {
            const next = micRunning
                ? await presenceMicStop()
                : await presenceMicStart();
            setMicRunning(next.running);
        } catch (err) {
            setMicError(String(err));
            setMicRunning(false);
        }
    }, [micRunning]);

    // Last emitted timestamp — the send throttle looks at this so a
  // slider drag can update local state at 60+ Hz for a smooth thumb
  // without flooding the pipe. `useRef` because a re-render every 33ms
  // just to bump a throttle timer would defeat the point.
  const lastSentAtRef = useRef(0);
  const engagedRef = useRef(engaged);
  useEffect(() => {
    engagedRef.current = engaged;
  }, [engaged]);

  const sendSignals = useCallback(
    (next: { intensity: number; audioLevel: number; progress: number }) => {
      const now = performance.now();
      if (now - lastSentAtRef.current < SIGNALS_MIN_INTERVAL_MS) return;
      lastSentAtRef.current = now;
      // `active_modes` is authoritative on the wire — omitting it would
      // release every engaged mode on the next tick. Pass the current
      // set so the sliders touch scalars only.
      void presenceSetSignals({
        intensity: next.intensity,
        audioLevel: next.audioLevel,
        progress: next.progress,
        activeModes: Array.from(engagedRef.current),
      });
    },
    [],
  );

  const toggleMode = useCallback(
    async (mode: PresenceMode) => {
      const next = new Set(engaged);
      const willEngage = !next.has(mode);
      if (willEngage) next.add(mode);
      else next.delete(mode);
      setEngaged(next);
      await presenceSetMode(mode, willEngage);
    },
    [engaged],
  );

  const onRingToggle = useCallback(async () => {
    const next = !ring;
    setRing(next);
    await presenceSetRingWanted(next);
  }, [ring]);

  const onReducedMotionToggle = useCallback(async () => {
    const next = !reducedMotion;
    setReducedMotion(next);
    await presenceSetReducedMotion(next);
  }, [reducedMotion]);

  const onPaletteChange = useCallback(async (id: PaletteId) => {
    setPalette(id);
    await presenceSetPalette(id);
  }, []);

  const onIntensityChange = useCallback(
    (v: number) => {
      setIntensity(v);
      sendSignals({ intensity: v, audioLevel, progress });
    },
    [audioLevel, progress, sendSignals],
  );

  const onAudioLevelChange = useCallback(
    (v: number) => {
      setAudioLevel(v);
      sendSignals({ intensity, audioLevel: v, progress });
    },
    [intensity, progress, sendSignals],
  );

  const onProgressChange = useCallback(
    (v: number) => {
      setProgress(v);
      sendSignals({ intensity, audioLevel, progress: v });
    },
    [intensity, audioLevel, sendSignals],
  );

  return (
    <section className="presence-dev" aria-label="Presence dev controls">
      <p className="presence-dev-heading">Presence</p>

      <div className="presence-dev-row">
        {PRESENCE_MODES.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            className={
              engaged.has(id)
                ? "presence-dev-chip is-engaged"
                : "presence-dev-chip"
            }
            onClick={() => toggleMode(id)}
          >
            {label}
          </button>
        ))}
      </div>

            <div className="presence-dev-row presence-dev-secondary">
                <button
                    type="button"
                    className={
                        ring ? "presence-dev-chip is-engaged" : "presence-dev-chip"
                    }
                    onClick={onRingToggle}
                >
                    Loading ring
                </button>
                <button
                    type="button"
                    className={
                        reducedMotion
                            ? "presence-dev-chip is-engaged"
                            : "presence-dev-chip"
                    }
                    onClick={onReducedMotionToggle}
                >
                    Reduced motion
                </button>
                <button
                    type="button"
                    className={
                        micRunning
                            ? "presence-dev-chip is-engaged"
                            : "presence-dev-chip"
                    }
                    onClick={onMicToggle}
                    disabled={!micAvailable}
                    title={
                        micAvailable
                            ? "Live mic → presence audio_level"
                            : "Shell built without the `mic` feature"
                    }
                >
                    {micRunning ? "Mic pump ●" : "Mic pump"}
                </button>
            </div>
            {micError && (
                <p className="presence-dev-error" role="alert">
                    {micError}
                </p>
            )}

      <div className="presence-dev-row presence-dev-palette">
        <span className="presence-dev-label">Palette</span>
        {PALETTES.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            className={
              palette === id
                ? "presence-dev-chip is-engaged"
                : "presence-dev-chip"
            }
            onClick={() => onPaletteChange(id)}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="presence-dev-signals">
        <SignalSlider
          label="Intensity"
          value={intensity}
          min={0}
          max={1.5}
          onChange={onIntensityChange}
        />
        <SignalSlider
          label="Audio level"
          value={audioLevel}
          min={0}
          max={1}
          onChange={onAudioLevelChange}
        />
        <SignalSlider
          label="Progress"
          value={progress}
          min={0}
          max={1}
          onChange={onProgressChange}
        />
      </div>
    </section>
  );
}

function SignalSlider(props: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  const { label, value, min, max, onChange } = props;
  return (
    <label className="presence-dev-slider">
      <span className="presence-dev-slider-label">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={0.01}
        value={value}
        onChange={(e) => onChange(parseFloat(e.currentTarget.value))}
      />
      <span className="presence-dev-slider-value">{value.toFixed(2)}</span>
    </label>
  );
}
