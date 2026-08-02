export type VoiceStyleId = "calm" | "direct" | "warm";

export type EdgeSettings = {
  tenantId: string;
  deviceId: string;
  actorId: string;
  mcpBaseUrl: string;
  micAcknowledged: boolean;
  voiceStyle: string;
};

/** IPC payload from load/save — settings fields plus derived gate. */
export type EdgeSettingsResponse = EdgeSettings & {
  setupComplete: boolean;
};

export function settingsFromResponse(r: EdgeSettingsResponse): EdgeSettings {
  return {
    tenantId: r.tenantId,
    deviceId: r.deviceId,
    actorId: r.actorId,
    mcpBaseUrl: r.mcpBaseUrl,
    micAcknowledged: r.micAcknowledged,
    voiceStyle: r.voiceStyle ?? "",
  };
}

export const VOICE_STYLES: {
  id: VoiceStyleId;
  label: string;
  description: string;
}[] = [
  {
    id: "calm",
    label: "Calm",
    description: "Measured pace, soft edges — good for long sessions.",
  },
  {
    id: "direct",
    label: "Direct",
    description: "Short answers, less ornament — good for tasking.",
  },
  {
    id: "warm",
    label: "Warm",
    description: "Friendly tone without losing clarity.",
  },
];

export const DEFAULT_SETTINGS: EdgeSettings = {
  tenantId: "local",
  deviceId: "desktop-1",
  actorId: "operator",
  mcpBaseUrl: "http://127.0.0.1:8787",
  micAcknowledged: false,
  voiceStyle: "",
};

export function isSettingsComplete(s: EdgeSettings): boolean {
  const styleOk = VOICE_STYLES.some((v) => v.id === s.voiceStyle);
  return (
    s.tenantId.trim().length > 0 &&
    s.deviceId.trim().length > 0 &&
    s.actorId.trim().length > 0 &&
    (s.mcpBaseUrl.startsWith("http://") || s.mcpBaseUrl.startsWith("https://")) &&
    s.micAcknowledged &&
    styleOk
  );
}
