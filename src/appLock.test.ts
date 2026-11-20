import { describe, expect, it } from "vitest";
import { idleLockDue } from "./appLock";

describe("idleLockDue", () => {
  it("locks after the chosen idle time", () => {
    const settings = { enabled: true, idleMinutes: 5 };
    expect(idleLockDue(settings, 0, 4 * 60_000)).toBe(false);
    expect(idleLockDue(settings, 0, 5 * 60_000)).toBe(true);
  });

  it("never locks on idle when off or set to launch only", () => {
    expect(idleLockDue({ enabled: false, idleMinutes: 5 }, 0, 1e9)).toBe(false);
    expect(idleLockDue({ enabled: true, idleMinutes: 0 }, 0, 1e9)).toBe(false);
    expect(idleLockDue(undefined, 0, 1e9)).toBe(false);
  });
});
