import { describe, expect, it } from "vitest";
import {
  decodeTriggerResponse,
  readTerminalTriggers,
  validateTrigger,
  TerminalTriggerMatcher,
  writeTerminalTriggers,
} from "./terminalTriggers";

describe("terminal output triggers", () => {
  it("matches lines across byte chunks and ignores terminal colors", () => {
    const matcher = new TerminalTriggerMatcher();
    const rules = [{ id: "1", phrase: "build failed" }];
    const bytes = new TextEncoder().encode("\x1b[31mBuild failed\x1b[0m\r\n");
    expect(matcher.feed(bytes.slice(0, 12), rules)).toEqual([]);
    expect(matcher.feed(bytes.slice(12), rules)).toEqual(["build failed"]);
    expect(
      matcher.feed(new TextEncoder().encode("Another line\n"), rules),
    ).toEqual([]);
  });

  it("handles Unicode split across chunks and keeps partial lines bounded", () => {
    const matcher = new TerminalTriggerMatcher();
    const rules = [{ id: "1", phrase: "café" }];
    const bytes = new TextEncoder().encode("café\n");
    expect(matcher.feed(bytes.slice(0, 4), rules)).toEqual([]);
    expect(matcher.feed(bytes.slice(4), rules)).toEqual(["café"]);
    matcher.feed(new TextEncoder().encode("x".repeat(5000)), rules);
    expect(matcher.feed(new TextEncoder().encode("\n"), rules)).toEqual([]);
  });

  it("persists validated rules and reports damaged storage", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    };
    const rules = [{ id: "1", phrase: "done" }];
    writeTerminalTriggers(storage, rules);
    expect(readTerminalTriggers(storage)).toEqual(rules);
    values.set("weave-terminal-triggers", "broken");
    expect(() => readTerminalTriggers(storage)).toThrow("damaged");
  });
});

describe("terminal trigger actions", () => {
  const encode = (text: string) => new TextEncoder().encode(text);

  it("matches regular expressions and returns the matched line", () => {
    const matcher = new TerminalTriggerMatcher();
    const rule = { id: "r", phrase: "error \\d+", regex: true };
    expect(matcher.feedMatches(encode("ok\nError 42 here\n"), [rule])).toEqual([
      { trigger: rule, line: "Error 42 here" },
    ]);
  });

  it("answers a waiting prompt once and rate-limits repeats", () => {
    const matcher = new TerminalTriggerMatcher();
    const rule = {
      id: "y",
      phrase: "continue? [y/n]",
      action: "respond" as const,
      response: "y\\n",
    };
    expect(
      matcher.feedMatches(encode("Continue? [y/N] "), [rule], 0),
    ).toHaveLength(1);
    expect(
      matcher.feedMatches(encode("Continue? [y/N] "), [rule], 500),
    ).toHaveLength(0);
    expect(
      matcher.feedMatches(encode("Continue? [y/N] "), [rule], 3000),
    ).toHaveLength(1);
    // Alert rules do not fire on unfinished lines.
    expect(
      matcher.feedMatches(encode("Continue? [y/N] "), [
        { id: "a", phrase: "continue" },
      ]),
    ).toEqual([]);
  });

  it("validates rules and decodes responses", () => {
    expect(validateTrigger({ phrase: "(", regex: true })).toMatch(/regular/);
    expect(validateTrigger({ phrase: ".*", regex: true })).toMatch(/empty/);
    expect(validateTrigger({ phrase: "ok", action: "respond" })).toMatch(
      /response/,
    );
    expect(validateTrigger({ phrase: "ok", action: "notify" })).toBe("");
    expect(decodeTriggerResponse("yes\\n")).toBe("yes\r");
    expect(decodeTriggerResponse("a\\tb\\\\")).toBe("a\tb\\");
  });
});
