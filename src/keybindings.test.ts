import { describe, expect, it, vi } from "vitest";

vi.mock("./platform", () => ({ isMac: true }));
import {
  actionFor,
  conflictFor,
  defaultKeybindings,
  loadKeybindings,
  saveKeybindings,
} from "./keybindings";

function memory() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

const press = (
  code: string,
  mods: { meta?: boolean; shift?: boolean } = {},
) => ({
  code,
  metaKey: !!mods.meta,
  shiftKey: !!mods.shift,
  altKey: false,
  ctrlKey: false,
});

describe("keybindings", () => {
  it("matches key presses to actions", () => {
    const bindings = defaultKeybindings();
    expect(actionFor(bindings, press("KeyK", { meta: true }))).toBe("palette");
    expect(
      actionFor(bindings, press("KeyD", { meta: true, shift: true })),
    ).toBe("splitDown");
    expect(actionFor(bindings, press("KeyQ", { meta: true }))).toBeUndefined();
  });

  it("saves only changes and survives bad data", () => {
    const storage = memory();
    const bindings = { ...defaultKeybindings(), palette: "Super+KeyP" };
    saveKeybindings(storage, bindings);
    expect(storage.getItem("weave-keybindings")).toBe(
      '{"palette":"Super+KeyP"}',
    );
    expect(loadKeybindings(storage).palette).toBe("Super+KeyP");
    storage.setItem("weave-keybindings", "{broken");
    expect(loadKeybindings(storage).palette).toBe("Super+KeyK");
  });

  it("reports conflicts", () => {
    const bindings = defaultKeybindings();
    expect(conflictFor(bindings, "palette", "Super+KeyT")).toBe("newTerminal");
    expect(conflictFor(bindings, "palette", "Super+KeyK")).toBeUndefined();
    expect(conflictFor(bindings, "theme", "")).toBeUndefined();
  });

  it("keeps Ctrl+C and Ctrl+D for the shell off macOS", () => {
    const pc = Object.values(defaultKeybindings(false));
    expect(pc).not.toContain("Control+KeyC");
    expect(pc).not.toContain("Control+KeyD");
    expect(pc.filter((key) => key.startsWith("Super"))).toEqual([]);
    const used = pc.filter(Boolean);
    expect(new Set(used).size).toBe(used.length);
  });
});
