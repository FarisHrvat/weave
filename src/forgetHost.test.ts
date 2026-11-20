import { describe, expect, it } from "vitest";
import { forgetHostLocally } from "./forgetHost";

function memory(entries: Record<string, unknown>) {
  const data = new Map(
    Object.entries(entries).map(([key, value]) => [key, JSON.stringify(value)]),
  );
  return {
    data,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
    removeItem: (key: string) => void data.delete(key),
  };
}

describe("forgetHostLocally", () => {
  it("removes the host's settings, history and saved items only", () => {
    const storage = memory({
      "weave-terminal-options:hpc%201": { fontSize: 14 },
      "weave-terminal-options:other": { fontSize: 12 },
      "weave-terminal-history": { "ssh:hpc 1": ["ls"], local: ["pwd"] },
      "weave-host-groups": {
        "hpc 1": { group: "Uni" },
        other: { group: "Work" },
      },
      "weave-snippets": [
        { id: "a", profileId: "hpc 1" },
        { id: "b", profileId: null },
      ],
      "weave-tunnel-presets": [{ id: "t", profileId: "hpc 1" }],
    });
    forgetHostLocally(storage, "hpc 1");
    expect(storage.data.has("weave-terminal-options:hpc%201")).toBe(false);
    expect(storage.data.has("weave-terminal-options:other")).toBe(true);
    expect(JSON.parse(storage.data.get("weave-terminal-history")!)).toEqual({
      local: ["pwd"],
    });
    expect(JSON.parse(storage.data.get("weave-host-groups")!)).toEqual({
      other: { group: "Work" },
    });
    expect(JSON.parse(storage.data.get("weave-snippets")!)).toEqual([
      { id: "b", profileId: null },
    ]);
    expect(JSON.parse(storage.data.get("weave-tunnel-presets")!)).toEqual([]);
  });
});
