import { describe, expect, it, vi } from "vitest";

vi.mock("./platform", () => ({ isMac: true }));
import { shortcutFromEvent, shortcutLabel } from "./shortcuts";

const press = (code: string, mods: Partial<Record<string, boolean>> = {}) => ({
  code,
  metaKey: !!mods.meta,
  altKey: !!mods.alt,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
});

describe("shortcuts", () => {
  it("records key combinations", () => {
    expect(shortcutFromEvent(press("KeyT", { meta: true, alt: true }))).toBe(
      "Super+Alt+KeyT",
    );
    expect(shortcutFromEvent(press("Space", { ctrl: true }))).toBe(
      "Control+Space",
    );
    expect(shortcutFromEvent(press("AltLeft", { alt: true }))).toBeNull();
  });

  it("shows readable labels", () => {
    expect(shortcutLabel("Super+Alt+KeyT")).toBe("⌘⌥T");
    expect(shortcutLabel("Control+Backquote")).toBe("⌃`");
    expect(shortcutLabel("Shift+Digit1")).toBe("⇧1");
    expect(shortcutLabel("Control+Shift+KeyT", false)).toBe("Ctrl+Shift+T");
    expect(shortcutLabel("", false)).toBe("");
  });
});
