import { describe, expect, it } from "vitest";
import { reviewDiffLines } from "./reviewDiff";

describe("reviewDiffLines", () => {
  it("maps added and removed diff lines to the correct side and file line", () => {
    const lines = reviewDiffLines(
      "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -8,2 +8,3 @@\n context\n-old\n+new\n+extra\n",
    );
    expect(
      lines
        .filter((line) => line.line !== null)
        .map((line) => [line.kind, line.side, line.line]),
    ).toEqual([
      ["context", "RIGHT", 8],
      ["remove", "LEFT", 9],
      ["add", "RIGHT", 9],
      ["add", "RIGHT", 10],
    ]);
  });

  it("resets line numbers at each hunk", () => {
    const lines = reviewDiffLines("@@ -1 +1 @@\n-a\n+b\n@@ -30 +40 @@\n-c\n+d");
    expect(
      lines.filter((line) => line.line !== null).map((line) => line.line),
    ).toEqual([1, 1, 30, 40]);
  });
});
