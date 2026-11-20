export type ReviewDiffLine = {
  text: string;
  before: number | null;
  after: number | null;
  side: "LEFT" | "RIGHT" | null;
  line: number | null;
  kind: "header" | "add" | "remove" | "context" | "meta";
};

export function reviewDiffLines(patch: string): ReviewDiffLine[] {
  let before = 0;
  let after = 0;
  let inHunk = false;
  const lines: ReviewDiffLine[] = [];
  for (const text of patch.split("\n").slice(0, 2000)) {
    const header = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    if (header) {
      before = Number(header[1]);
      after = Number(header[2]);
      inHunk = true;
      lines.push({
        text,
        before: null,
        after: null,
        side: null,
        line: null,
        kind: "header",
      });
      continue;
    }
    if (!inHunk || text.startsWith("\\") || text === "") {
      lines.push({
        text,
        before: null,
        after: null,
        side: null,
        line: null,
        kind: "meta",
      });
      continue;
    }
    if (text.startsWith("+")) {
      lines.push({
        text,
        before: null,
        after,
        side: "RIGHT",
        line: after,
        kind: "add",
      });
      after += 1;
    } else if (text.startsWith("-")) {
      lines.push({
        text,
        before,
        after: null,
        side: "LEFT",
        line: before,
        kind: "remove",
      });
      before += 1;
    } else if (text.startsWith(" ")) {
      lines.push({
        text,
        before,
        after,
        side: "RIGHT",
        line: after,
        kind: "context",
      });
      before += 1;
      after += 1;
    } else {
      lines.push({
        text,
        before: null,
        after: null,
        side: null,
        line: null,
        kind: "meta",
      });
    }
  }
  return lines;
}
