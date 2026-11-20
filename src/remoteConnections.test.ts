import { describe, expect, it } from "vitest";
import {
  describeConnection,
  mergeImportedConnections,
  readRemoteConnections,
  validateConnection,
  writeRemoteConnections,
} from "./remoteConnections";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

describe("remote connections", () => {
  it("validates each connection type", () => {
    expect(
      validateConnection({
        kind: "telnet",
        name: "Router",
        host: "10.0.0.1",
        port: 23,
      }),
    ).toBe("");
    expect(
      validateConnection({
        kind: "telnet",
        name: "Bad",
        host: "a b",
        port: 23,
      }),
    ).toMatch(/host/);
    expect(
      validateConnection({
        kind: "serial",
        name: "Board",
        device: "/dev/cu.usbserial-1",
        baud: 115200,
      }),
    ).toBe("");
    expect(
      validateConnection({
        kind: "serial",
        name: "Board",
        device: "/dev/cu.usbserial-1",
        baud: 1234,
      }),
    ).toMatch(/speed/);
    expect(validateConnection({ kind: "mosh", name: "Box" })).toMatch(/SSH/);
    expect(
      validateConnection({ kind: "rdp", name: "PC", host: "pc", port: 0 }),
    ).toMatch(/port/);
  });

  it("saves valid connections and drops damaged entries", () => {
    const storage = memoryStorage();
    writeRemoteConnections(storage, [
      { id: "1", kind: "vnc", name: "Mac", host: "mac.local", port: 5900 },
    ]);
    expect(readRemoteConnections(storage)).toHaveLength(1);
    storage.setItem(
      "weave-remote-connections",
      JSON.stringify([{ id: "x", kind: "telnet", name: "", host: "h" }]),
    );
    expect(readRemoteConnections(storage)).toEqual([]);
    storage.setItem("weave-remote-connections", "{not json");
    expect(readRemoteConnections(storage)).toEqual([]);
  });

  it("describes connections", () => {
    expect(
      describeConnection({
        kind: "serial",
        name: "B",
        device: "/dev/cu.x",
        baud: 9600,
      }),
    ).toBe("/dev/cu.x · 9600 baud");
    expect(
      describeConnection({ kind: "mosh", name: "M", profileId: "p" }, "box"),
    ).toBe("box");
  });
});

describe("mergeImportedConnections", () => {
  it("adds valid new sessions only once", () => {
    const saved = [
      { id: "a", kind: "rdp" as const, name: "Desk", host: "pc", port: 3389 },
    ];
    let next = 0;
    const result = mergeImportedConnections(
      saved,
      [
        { kind: "rdp", name: "Desk again", host: "PC", port: 3389 },
        { kind: "vnc", name: "Mac", host: "mac.lab", port: 5901 },
        { kind: "telnet", name: "Bad", host: "has space", port: 23 },
      ],
      () => `new-${next++}`,
    );
    expect(result.added).toBe(1);
    expect(result.connections.map((item) => item.id)).toEqual(["a", "new-0"]);
  });
});
