import { describe, expect, it } from "vitest";
import { broadcastTargets } from "./terminalBroadcast";

describe("terminal broadcast", () => {
  const terminals = new Map([
    ["source", "pty-a"],
    ["included", "pty-b"],
    ["excluded", "pty-c"],
    ["ended", "pty-d"],
  ]);

  it("sends only to connected included tabs other than the source", () => {
    expect(
      broadcastTargets(
        "source",
        terminals,
        new Set(["ended"]),
        new Set(["excluded"]),
      ),
    ).toEqual(["pty-b"]);
  });

  it("does not broadcast input typed in an excluded tab", () => {
    expect(
      broadcastTargets("excluded", terminals, new Set(), new Set(["excluded"])),
    ).toEqual([]);
  });
});
