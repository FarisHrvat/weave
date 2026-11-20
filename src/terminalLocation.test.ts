import { describe, expect, it } from "vitest";
import { parseOsc7 } from "./terminalLocation";

describe("terminal OSC 7 directory", () => {
  it("reads local and remote shell paths", () => {
    expect(parseOsc7("file://localhost/Users/a/My%20Project")).toBe(
      "/Users/a/My Project",
    );
    expect(parseOsc7("file://server.example/home/a/project")).toBe(
      "/home/a/project",
    );
  });

  it("ignores unsafe or unrelated messages", () => {
    expect(parseOsc7("https://example.com/folder")).toBeNull();
    expect(parseOsc7("file://user:pass@host/home/a")).toBeNull();
    expect(parseOsc7("file://host/home/a?x=1")).toBeNull();
    expect(parseOsc7("file://host/home/%00hidden")).toBeNull();
    expect(parseOsc7("not a URL")).toBeNull();
  });
});
