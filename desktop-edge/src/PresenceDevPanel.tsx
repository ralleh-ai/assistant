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

import { useCallback, useState } from "react";
import {
  PRESENCE_MODES,
  presenceSetMode,
  presenceSetPalette,
  presenceSetReducedMotion,
  presenceSetRingWanted,
  PaletteId,
  PALETTES,
  PresenceMode,
} from "./presence";

export function PresenceDevPanel() {
  // Presence engagement state is authoritative in the runtime — we do
  // not read it back, we just track what *this* UI has asked for so the
  // toggle button reflects the last request. If the runtime is behaving
  // and no other process is sending commands, the two match.
  const [engaged, setEngaged] = useState<Set<PresenceMode>>(new Set());
  const [ring, setRing] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const [palette, setPalette] = useState<PaletteId>("teal");

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
      </div>

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
    </section>
  );
}
