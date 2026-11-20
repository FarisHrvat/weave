import { describe, expect, it } from "vitest";
import { sideBySideRows, splitDiff } from "./gitDiff";

describe("splitDiff", () => {
  it("keeps patch headers and separates selectable hunks", () => {
    const patch =
      "diff --git a/file b/file\n--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n@@ -10 +10 @@\n-before\n+after\n";
    const result = splitDiff(patch);
    expect(result.header).toBe(
      "diff --git a/file b/file\n--- a/file\n+++ b/file\n",
    );
    expect(result.hunks).toEqual([
      "@@ -1 +1 @@\n-old\n+new\n",
      "@@ -10 +10 @@\n-before\n+after\n",
    ]);
  });

  it("leaves binary or empty diffs as an unselectable header", () => {
    expect(splitDiff("Binary files differ\n")).toEqual({
      header: "Binary files differ\n",
      hunks: [],
    });
    expect(splitDiff("")).toEqual({ header: "", hunks: [] });
  });
});

describe("sideBySideRows", () => {
  it("aligns replacement and context lines with their original numbers", () => {
    const rows = sideBySideRows(
      "@@ -4,3 +4,4 @@\n context\n-old\n+new\n+extra\n after\n",
    );
    expect(rows).toEqual([
      {
        left: "context",
        right: "context",
        leftNumber: 4,
        rightNumber: 4,
        changed: false,
      },
      {
        left: "old",
        right: "new",
        leftNumber: 5,
        rightNumber: 5,
        changed: true,
      },
      {
        left: "",
        right: "extra",
        leftNumber: undefined,
        rightNumber: 6,
        changed: true,
      },
      {
        left: "after",
        right: "after",
        leftNumber: 6,
        rightNumber: 7,
        changed: false,
      },
    ]);
  });
});
