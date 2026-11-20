import { describe, expect, it } from "vitest";
import {
  clearTerminalActivity,
  markTerminalActivity,
} from "./terminalActivity";

describe("terminal activity", () => {
  it("marks hidden output once and clears visible split panes", () => {
    const empty = new Set<string>();
    expect(markTerminalActivity(empty, "active", true)).toBe(empty);
    const marked = markTerminalActivity(empty, "other", false);
    expect(marked.has("other")).toBe(true);
    expect(markTerminalActivity(marked, "other", false)).toBe(marked);
    expect(clearTerminalActivity(marked, ["active", "split"])).toBe(marked);
    expect(clearTerminalActivity(marked, ["other", undefined]).size).toBe(0);
  });
});
