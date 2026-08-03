import { useEffect, useState } from "react";
import type { EdgeSettings } from "./settings";
import { presenceStatus } from "./presence";
import { BackendSettings } from "./BackendSettings";
import { Conversation } from "./Conversation";
import { PresenceDevPanel } from "./PresenceDevPanel";
import { PresenceStatusLine } from "./PresenceStatusLine";

type Props = {
  settings: EdgeSettings;
  onOpenSettings: () => void;
};

function GearIcon() {
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z" />
      <path d="M19.4 13.5a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V19a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H5a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H10a1.7 1.7 0 0 0 1-1.5V5a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V10c.2.6.8 1 1.5 1H19a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z" />
    </svg>
  );
}

export function Core({ settings, onOpenSettings }: Props) {
  const styleLabel =
    settings.voiceStyle.charAt(0).toUpperCase() + settings.voiceStyle.slice(1);

  // Presence dev panel: only rendered when the Tauri side reports the
  // runtime is spawned. On a machine without `RALLEH_PRESENCE_BIN` this
  // stays `false` and the panel never mounts, so the shipping Core view
  // is unchanged. The check is a single Tauri round-trip on mount —
  // cheap and inert once resolved.
  const [presenceEnabled, setPresenceEnabled] = useState(false);
  useEffect(() => {
    let cancelled = false;
    presenceStatus().then((s) => {
      if (!cancelled) setPresenceEnabled(s.enabled);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section className="core" aria-label="Ralleh edge">
      <button
        type="button"
        className="core-gear"
        onClick={onOpenSettings}
        aria-label="Settings"
        title="Settings"
      >
        <GearIcon />
      </button>

      <div className="core-body">
        <p className="brand">Ralleh</p>
        <p className="core-identity">
          <span>{settings.tenantId}</span>
          <span className="core-sep" aria-hidden="true">
            ·
          </span>
          <span>{settings.deviceId}</span>
          <span className="core-sep" aria-hidden="true">
            ·
          </span>
          <span>{settings.actorId}</span>
          <span className="core-sep" aria-hidden="true">
            ·
          </span>
          <span>{styleLabel}</span>
        </p>

        {presenceEnabled && <PresenceStatusLine />}
        <BackendSettings />
        <Conversation />
        {presenceEnabled && <PresenceDevPanel />}
      </div>
    </section>
  );
}
