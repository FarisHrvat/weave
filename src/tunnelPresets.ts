import type { SshTunnel } from "./types";

export type TunnelPreset = {
  id: string;
  profileId: string;
  name: string;
  kind: SshTunnel["kind"];
  listenPort: number;
  targetHost: string;
  targetPort: number;
  /** Start this tunnel whenever Weave opens. */
  autoStart?: boolean;
};

const storageKey = "weave-tunnel-presets";

export function loadTunnelPresets(
  storage: Pick<Storage, "getItem">,
): TunnelPreset[] {
  const saved = storage.getItem(storageKey);
  if (!saved) return [];
  let value: unknown;
  try {
    value = JSON.parse(saved);
  } catch {
    throw new Error(
      "Saved SSH tunnels could not be read. The stored data appears damaged.",
    );
  }
  if (!Array.isArray(value)) {
    throw new Error(
      "Saved SSH tunnels could not be read. The stored data has an unexpected format.",
    );
  }
  return value.filter(
    (item): item is TunnelPreset =>
      typeof item === "object" &&
      item !== null &&
      typeof item.id === "string" &&
      typeof item.profileId === "string" &&
      typeof item.name === "string" &&
      ["local", "remote", "dynamic"].includes(item.kind) &&
      Number.isInteger(item.listenPort) &&
      item.listenPort > 0 &&
      item.listenPort <= 65535 &&
      typeof item.targetHost === "string" &&
      Number.isInteger(item.targetPort) &&
      item.targetPort >= 0 &&
      item.targetPort <= 65535 &&
      (item.autoStart === undefined || typeof item.autoStart === "boolean"),
  );
}

export function saveTunnelPresets(
  storage: Pick<Storage, "setItem">,
  presets: TunnelPreset[],
): void {
  storage.setItem(storageKey, JSON.stringify(presets));
}

/** Presets marked to start with Weave whose listener is not already open. */
export function presetsToAutoStart(
  presets: TunnelPreset[],
  running: Pick<SshTunnel, "profileId" | "kind" | "listenPort">[],
): TunnelPreset[] {
  return presets.filter(
    (preset) =>
      preset.autoStart &&
      !running.some(
        (tunnel) =>
          tunnel.profileId === preset.profileId &&
          tunnel.kind === preset.kind &&
          tunnel.listenPort === preset.listenPort,
      ),
  );
}
