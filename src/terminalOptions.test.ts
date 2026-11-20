import { describe, expect, it } from "vitest";
import {
  defaultTerminalOptions,
  readTerminalOptions,
  writeTerminalOptions,
} from "./terminalOptions";

describe("per-host terminal settings", () => {
  it("keeps local and host settings separate and rejects damaged values", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    };
    const custom = {
      ...defaultTerminalOptions,
      fontSize: 18,
      theme: "light" as const,
    };
    writeTerminalOptions(storage, "host-a", custom);
    expect(readTerminalOptions(storage, "host-a")).toEqual(custom);
    expect(readTerminalOptions(storage, "host-b")).toEqual(
      defaultTerminalOptions,
    );
    expect(readTerminalOptions(storage)).toEqual(defaultTerminalOptions);
    values.set("weave-terminal-options:host-a", "broken");
    expect(() => readTerminalOptions(storage, "host-a")).toThrow("damaged");
    values.set(
      "weave-terminal-options:host-a",
      JSON.stringify({ ...custom, fontSize: 999 }),
    );
    expect(() => readTerminalOptions(storage, "host-a")).toThrow(
      "invalid format",
    );
  });
});
