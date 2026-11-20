import { describe, expect, it } from "vitest";
import { isArchive } from "./archives";

describe("isArchive", () => {
  it("recognises the formats Weave can extract", () => {
    for (const name of [
      "a.zip",
      "B.TAR.GZ",
      "c.tgz",
      "d.tar.bz2",
      "e.tar.xz",
      "f.tar",
    ])
      expect(isArchive(name)).toBe(true);
  });

  it("rejects other files and bare extensions", () => {
    for (const name of ["notes.txt", "photo.gz.png", ".zip", "rar.rar"])
      expect(isArchive(name)).toBe(false);
  });
});
