import { describe, expect, it } from "vitest";
import { readSftpOnly, saysSftpOnly, writeSftpOnly } from "./sftpOnly";

describe("SFTP-only servers", () => {
  it("recognises the usual refusals", () => {
    for (const text of [
      "This service allows sftp connections only.\r\nShared connection to 203.0.113.10 closed.",
      "This account is restricted to SFTP.",
      "Only SFTP is allowed on this host",
    ])
      expect(saysSftpOnly(text)).toBe(true);
    expect(
      saysSftpOnly("Welcome to the cluster. Use sftp to copy files."),
    ).toBe(false);
  });

  it("remembers hosts", () => {
    const data = new Map<string, string>();
    const storage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
    };
    writeSftpOnly(storage, new Set(["hpc"]));
    expect([...readSftpOnly(storage)]).toEqual(["hpc"]);
    data.set("weave-sftp-only", "broken");
    expect(readSftpOnly(storage).size).toBe(0);
  });
});
