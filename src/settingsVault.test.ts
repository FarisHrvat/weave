import { describe, expect, it } from "vitest";
import { applyVaultSettings, collectVaultSettings } from "./settingsVault";

function storage(entries: Record<string, string>) {
  const values = new Map(Object.entries(entries));
  return {
    get length() {
      return values.size;
    },
    key: (index: number) => [...values.keys()][index] ?? null,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
    values,
  };
}

describe("settings vault", () => {
  it("collects Weave settings but never command history", () => {
    const local = storage({
      "weave-snippets": "[]",
      "weave-terminal-history": "{}",
      "other-app": "x",
    });
    expect(collectVaultSettings(local)).toEqual({ "weave-snippets": "[]" });
  });

  it("applies only Weave settings", () => {
    const local = storage({});
    expect(
      applyVaultSettings(local, {
        "weave-theme": "light",
        "weave-terminal-history": "{}",
        evil: "1",
      }),
    ).toBe(1);
    expect([...local.values.keys()]).toEqual(["weave-theme"]);
  });
});
