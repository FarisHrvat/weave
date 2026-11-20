export function splitDiff(diff: string): { header: string; hunks: string[] } {
  const hunks: string[] = [];
  let header = "";
  for (const line of diff.match(/.*(?:\n|$)/g) ?? []) {
    if (!line) continue;
    if (line.startsWith("@@ ")) hunks.push("");
    if (hunks.length) hunks[hunks.length - 1] += line;
    else header += line;
  }
  return { header, hunks };
}

export type SideBySideRow = {
  left: string;
  right: string;
  leftNumber?: number;
  rightNumber?: number;
  changed: boolean;
};

export function sideBySideRows(hunk: string): SideBySideRow[] {
  const [heading, ...body] = hunk.split("\n");
  const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(heading);
  if (!match) return [];
  let leftNumber = Number(match[1]);
  let rightNumber = Number(match[2]);
  const rows: SideBySideRow[] = [];
  let removed: string[] = [];
  let added: string[] = [];

  function flushChanges() {
    for (
      let index = 0;
      index < Math.max(removed.length, added.length);
      index++
    ) {
      const left = removed[index];
      const right = added[index];
      rows.push({
        left: left ?? "",
        right: right ?? "",
        leftNumber: left === undefined ? undefined : leftNumber++,
        rightNumber: right === undefined ? undefined : rightNumber++,
        changed: true,
      });
    }
    removed = [];
    added = [];
  }

  for (const line of body) {
    if (line === "") continue;
    if (line.startsWith("-")) {
      removed.push(line.slice(1));
      continue;
    }
    if (line.startsWith("+")) {
      added.push(line.slice(1));
      continue;
    }
    flushChanges();
    if (line.startsWith(" ")) {
      rows.push({
        left: line.slice(1),
        right: line.slice(1),
        leftNumber: leftNumber++,
        rightNumber: rightNumber++,
        changed: false,
      });
    } else {
      rows.push({ left: line, right: line, changed: false });
    }
  }
  flushChanges();
  return rows;
}
