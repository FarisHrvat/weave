import { describe, expect, it } from "vitest";
import {
  AutomationRun,
  parseAutomation,
  type AutomationStatus,
} from "./terminalAutomation";

function harness(source: string, secret: string | null = "s3cret") {
  const written: string[] = [];
  const statuses: AutomationStatus[] = [];
  const timers: (() => void)[] = [];
  const run = new AutomationRun(parseAutomation(source), {
    write: (data) => written.push(data),
    askSecret: async () => secret,
    onStatus: (status) => statuses.push(status),
    setTimer: (callback) => timers.push(callback),
    clearTimer: () => {},
  });
  return { run, written, statuses, timers };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("terminal automation", () => {
  it("parses steps and reports the line of a mistake", () => {
    expect(parseAutomation("expect login:\nsend admin\\n")).toHaveLength(2);
    expect(() => parseAutomation("expect /(/")).toThrow("Line 1");
    expect(() => parseAutomation("# only\n\nfoo bar")).toThrow(
      "Line 3: “foo” is not a step",
    );
    expect(() => parseAutomation("wait abc")).toThrow("Line 1");
    expect(() => parseAutomation("")).toThrow("at least one step");
  });

  it("waits for prompts across output chunks and types responses", async () => {
    const { run, written, statuses } = harness(
      "expect login:\nsend admin\\n\nexpect Password:\nsecret Router password\nexpect /[#>] ?$/\nsend show version\\n",
    );
    run.start();
    run.output("Welcome\r\nlog");
    expect(written).toEqual([]);
    run.output("in: ");
    await tick();
    expect(written).toEqual(["admin\r"]);
    run.output("\x1b[1mPassword:\x1b[0m ");
    await tick();
    expect(written).toEqual(["admin\r", "s3cret\r"]);
    run.output("router# ");
    await tick();
    expect(written.at(-1)).toBe("show version\r");
    expect(statuses.at(-1)).toEqual({ state: "done" });
  });

  it("stops with a clear message when a prompt never appears", () => {
    const { run, statuses, timers } = harness("timeout 5\nexpect ready");
    run.start();
    timers[0]();
    expect(statuses.at(-1)).toEqual({
      state: "failed",
      message: "Line 2: “ready” did not appear within 5 seconds.",
    });
    expect(run.active).toBe(false);
  });

  it("stops when a secret is cancelled", async () => {
    const { run, written, statuses } = harness("secret Token", null);
    run.start();
    await tick();
    expect(written).toEqual([]);
    expect(statuses.at(-1)).toEqual({ state: "failed", message: "Cancelled." });
  });
});
