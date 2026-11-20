import { describe, expect, it } from "vitest";
import {
  asOneLine,
  durationText,
  findLines,
  isPasswordPrompt,
  PasteHistory,
  pastedLines,
  zoomedFont,
} from "./terminalPaste";

describe("pastes", () => {
  it("counts lines, ignoring one trailing newline", () => {
    expect(pastedLines("ls")).toBe(1);
    expect(pastedLines("ls\n")).toBe(1);
    expect(pastedLines("ls\r\npwd\nwhoami")).toBe(3);
    expect(pastedLines("")).toBe(0);
  });

  it("joins lines for a safe single-line paste", () => {
    expect(asOneLine("cd /tmp &&\n  ls -la\r\n\n")).toBe("cd /tmp && ls -la");
  });

  it("keeps recent pastes newest first without duplicates", () => {
    const history = new PasteHistory(2);
    history.add("a");
    history.add("b");
    history.add("a");
    history.add("  ");
    expect(history.list()).toEqual(["a", "b"]);
    history.add("c");
    expect(history.list()).toEqual(["c", "a"]);
  });
});

describe("password prompts", () => {
  it("recognises common prompts", () => {
    for (const line of [
      "Password:",
      "[sudo] password for alice: ",
      "Enter passphrase for key '/Users/a/.ssh/id_ed25519': ",
      "alice@host's password: ",
      "PIN: ",
    ])
      expect(isPasswordPrompt(line)).toBe(true);
  });

  it("ignores ordinary output", () => {
    for (const line of ["password changed", "$ echo password", "Username:"])
      expect(isPasswordPrompt(line)).toBe(false);
  });
});

describe("helpers", () => {
  it("formats durations", () => {
    expect(durationText(12_400)).toBe("12s");
    expect(durationText(125_000)).toBe("2m 5s");
    expect(durationText(3_725_000)).toBe("1h 2m");
  });

  it("limits font zoom", () => {
    expect(zoomedFont(13, 1)).toBe(14);
    expect(zoomedFont(36, 1)).toBe(36);
    expect(zoomedFont(8, -1)).toBe(8);
  });

  it("finds matching lines case-insensitively", () => {
    expect(findLines(["Error: one", "ok", "error two"], "ERROR")).toEqual([
      { line: 0, text: "Error: one" },
      { line: 2, text: "error two" },
    ]);
    expect(findLines(["a", "a", "a"], "a", 2).map((m) => m.line)).toEqual([
      1, 2,
    ]);
    expect(findLines(["a"], "")).toEqual([]);
  });
});
