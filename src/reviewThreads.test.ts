import { describe, expect, it } from "vitest";
import { mergeReviewThreads } from "./reviewThreads";
import type { ForgeReviewThread } from "./types";

describe("review thread pagination", () => {
  it("joins replies when a thread crosses page boundaries", () => {
    const first: ForgeReviewThread = {
      id: "11",
      providerId: null,
      path: "src/main.rs",
      line: 8,
      side: "RIGHT",
      resolved: false,
      comments: [
        { id: 11, author: "alice", body: "Change this", createdAt: "" },
      ],
    };
    const second: ForgeReviewThread = {
      ...first,
      comments: [
        { id: 11, author: "alice", body: "Change this", createdAt: "" },
        { id: 12, author: "bob", body: "Done", createdAt: "" },
      ],
    };
    const result = mergeReviewThreads([first], [second]);
    expect(result).toHaveLength(1);
    expect(result[0].comments.map((comment) => comment.id)).toEqual([11, 12]);
    expect(first.comments).toHaveLength(1);
  });
});
