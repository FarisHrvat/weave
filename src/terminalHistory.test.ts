import { describe, expect, it } from "vitest";
import {
  addToHistory,
  InputTracker,
  readHistory,
  suggest,
  wasEchoed,
} from "./terminalHistory";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

describe("terminal history", () => {
  it("tracks typed commands and gives up on editing it cannot follow", () => {
    const tracker = new InputTracker();
    expect(tracker.feed("git stx")).toBeNull();
    expect(tracker.feed("\x7fatus\r")).toBe("git status");
    expect(tracker.feed("ls\x1b[D-l\r")).toBeNull();
    expect(tracker.feed("pwd\x03")).toBeNull();
    expect(tracker.current).toBe("");
    expect(tracker.feed("\x1b[200~echo hi\x1b[201~\r")).toBe("echo hi");
  });

  it("stores unique recent commands per host and skips private ones", () => {
    const storage = memoryStorage();
    addToHistory(storage, "local", "ls");
    addToHistory(storage, "local", "git status");
    addToHistory(storage, "local", "ls");
    addToHistory(storage, "local", " secret-token");
    addToHistory(storage, "ssh:1", "uptime");
    expect(readHistory(storage, "local")).toEqual(["git status", "ls"]);
    expect(readHistory(storage, "ssh:1")).toEqual(["uptime"]);
  });

  it("ranks prefix matches before word matches", () => {
    const history = ["git status", "docker ps", "git push origin main", "ls"];
    expect(suggest("git", history)).toEqual([
      "git push origin main",
      "git status",
    ]);
    expect(suggest("origin push", history)).toEqual(["git push origin main"]);
    expect(suggest("", history, 2)).toEqual(["ls", "git push origin main"]);
  });

  it("detects whether a command was echoed", () => {
    expect(wasEchoed("~/src % git status", "git status")).toBe(true);
    expect(wasEchoed("Password:", "hunter2")).toBe(false);
  });
});
