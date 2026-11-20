import { describe, expect, it } from "vitest";
import { TerminalRecording } from "./terminalRecording";

describe("terminal recording", () => {
  it("writes asciicast v2 output, Unicode, and resize events", () => {
    const recording = new TerminalRecording(80, 24, 1000, 1_700_000_000_000);
    const bytes = new TextEncoder().encode("café");
    recording.output(bytes.slice(0, 4), 1100);
    recording.output(bytes.slice(4), 1200);
    recording.resize(100, 30, 1300);
    const lines = recording
      .finish(1400)
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    expect(lines[0]).toMatchObject({ version: 2, width: 80, height: 24 });
    expect(lines.slice(1)).toEqual([
      [0.1, "o", "caf"],
      [0.2, "o", "é"],
      [0.3, "r", "100x30"],
    ]);
  });

  it("stops before the recording grows beyond its in-memory limit", () => {
    const recording = new TerminalRecording(80, 24, 0, 0);
    expect(recording.output(new Uint8Array(4 * 1024 * 1024 + 1), 1)).toBe(
      false,
    );
    const lines = recording.finish(2).trim().split("\n");
    expect(lines).toHaveLength(1);
  });

  it("exports readable output without terminal control sequences or input", () => {
    const recording = new TerminalRecording(80, 24, 0, 0);
    recording.output(
      new TextEncoder().encode("\x1b(B\x1b[31mred\x1b[0m\r\n"),
      1,
    );
    recording.output(
      new TextEncoder().encode(
        "\x1b]8;;https://example.com\x07link\x1b]8;;\x1b\\\n",
      ),
      2,
    );
    recording.output(new TextEncoder().encode("abc\bD\rprogress"), 3);
    recording.resize(90, 25, 4);
    recording.finish(5);
    expect(recording.plainText()).toBe("red\nlink\nabD\nprogress");
  });
});
