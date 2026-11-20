import { describe, expect, it } from "vitest";
import {
  loadTunnelPresets,
  presetsToAutoStart,
  saveTunnelPresets,
} from "./tunnelPresets";

describe("SSH tunnel presets", () => {
  it("preserves a saved preset and reports damaged storage", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    };
    const preset = {
      id: "one",
      profileId: "host",
      name: "NAS UI",
      kind: "local" as const,
      listenPort: 8080,
      targetHost: "127.0.0.1",
      targetPort: 80,
    };
    saveTunnelPresets(storage, [preset]);
    expect(loadTunnelPresets(storage)).toEqual([preset]);
    values.set("weave-tunnel-presets", "not-json");
    expect(() => loadTunnelPresets(storage)).toThrow("damaged");
  });

  it("auto-starts only marked presets that are not running", () => {
    const base = {
      profileId: "host",
      kind: "local" as const,
      targetHost: "127.0.0.1",
      targetPort: 80,
    };
    const presets = [
      { ...base, id: "a", name: "A", listenPort: 8080, autoStart: true },
      { ...base, id: "b", name: "B", listenPort: 8081, autoStart: true },
      { ...base, id: "c", name: "C", listenPort: 8082 },
    ];
    expect(
      presetsToAutoStart(presets, [
        { profileId: "host", kind: "local", listenPort: 8080 },
      ]).map((preset) => preset.id),
    ).toEqual(["b"]);
  });
});
