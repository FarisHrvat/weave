import { describe, expect, it } from "vitest";
import { withFolders } from "./changeFolders";

const file = (path: string) => ({ path, status: " M" }) as never;

describe("grouping changes by folder", () => {
  it("adds a heading for each folder and keeps root files first", () => {
    const items = withFolders(
      [
        file("src/b.ts"),
        file("README.md"),
        file("src/a.ts"),
        file("docs/x.md"),
      ],
      true,
    );
    expect(
      items.map((item) =>
        item.kind === "folder"
          ? `[${item.folder} ${item.count}]`
          : item.file.path,
      ),
    ).toEqual([
      "README.md",
      "[docs 1]",
      "docs/x.md",
      "[src 2]",
      "src/a.ts",
      "src/b.ts",
    ]);
    expect(withFolders([file("a")], false)).toEqual([
      { kind: "file", file: file("a") },
    ]);
  });
});
