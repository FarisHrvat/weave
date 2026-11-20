import { describe, expect, it } from "vitest";
import {
  readTerminalScripts,
  scriptsForProfile,
  writeTerminalScripts,
  type SavedTerminalScript,
} from "./terminalScripts";

function storage() {
  let saved: string | null = null;
  return {
    getItem: () => saved,
    setItem: (_key: string, value: string) => {
      saved = value;
    },
  };
}

const example: SavedTerminalScript = {
  id: "one",
  name: "Check disk",
  profileId: "nas",
  cwd: "/home/alice",
  body: "df -h\n",
};

describe("saved terminal scripts", () => {
  it("round trips host-specific scripts and filters other hosts", () => {
    const local = storage();
    const scripts = [example, { ...example, id: "two", profileId: null }];
    writeTerminalScripts(local, scripts);
    expect(readTerminalScripts(local)).toEqual(scripts);
    expect(scriptsForProfile(readTerminalScripts(local), "nas")).toEqual([
      example,
    ]);
    expect(scriptsForProfile(readTerminalScripts(local), null)).toEqual([
      scripts[1],
    ]);
  });

  it("rejects damaged, duplicate, oversized and empty scripts", () => {
    const local = storage();
    local.setItem("weave-terminal-scripts", "not json");
    expect(() => readTerminalScripts(local)).toThrow("damaged");
    expect(() => writeTerminalScripts(local, [example, example])).toThrow(
      "duplicate IDs",
    );
    expect(() =>
      writeTerminalScripts(local, [{ ...example, body: " " }]),
    ).toThrow("1 byte to 64 KB");
    expect(() =>
      writeTerminalScripts(local, [{ ...example, body: "ü".repeat(33_000) }]),
    ).toThrow("64 KB");
    expect(() =>
      writeTerminalScripts(
        local,
        Array.from({ length: 31 }, (_, index) => ({
          ...example,
          id: String(index),
        })),
      ),
    ).toThrow("30 terminal scripts");
  });
});
