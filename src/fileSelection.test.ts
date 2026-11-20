import { describe, expect, it } from "vitest";
import { selectFileEntry } from "./fileSelection";
import type { FileEntry } from "./types";

const files: FileEntry[] = ["a", "b", "c", "d"].map((name) => ({
  name,
  path: `/folder/${name}`,
  isDir: false,
  isSymlink: false,
  size: 0,
  modified: null,
  readonly: false,
}));

describe("file selection", () => {
  it("adds and removes items with Command-click", () => {
    const first = selectFileEntry(
      undefined,
      undefined,
      "left",
      files,
      files[0],
      {
        shift: false,
        additive: false,
      },
    );
    const added = selectFileEntry(
      first.selection,
      first.anchor,
      "left",
      files,
      files[2],
      { shift: false, additive: true },
    );
    expect(added.selection.entries.map((entry) => entry.name)).toEqual([
      "a",
      "c",
    ]);
    const removed = selectFileEntry(
      added.selection,
      added.anchor,
      "left",
      files,
      files[0],
      { shift: false, additive: true },
    );
    expect(removed.selection.entries.map((entry) => entry.name)).toEqual(["c"]);
  });

  it("selects a range and resets when switching panes", () => {
    const first = selectFileEntry(
      undefined,
      undefined,
      "left",
      files,
      files[1],
      {
        shift: false,
        additive: false,
      },
    );
    const range = selectFileEntry(
      first.selection,
      first.anchor,
      "left",
      files,
      files[3],
      { shift: true, additive: false },
    );
    expect(range.selection.entries.map((entry) => entry.name)).toEqual([
      "b",
      "c",
      "d",
    ]);
    const switched = selectFileEntry(
      range.selection,
      range.anchor,
      "right",
      files,
      files[0],
      { shift: true, additive: false },
    );
    expect(switched.selection).toEqual({ side: "right", entries: [files[0]] });
  });
});
