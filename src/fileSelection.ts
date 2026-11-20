import type { FileEntry } from "./types";

export type FileSelection = {
  side: "left" | "right";
  entries: FileEntry[];
};

export function selectFileEntry(
  current: FileSelection | undefined,
  anchor: string | undefined,
  side: FileSelection["side"],
  visible: FileEntry[],
  entry: FileEntry,
  modifiers: { shift: boolean; additive: boolean },
): { selection: FileSelection; anchor: string } {
  if (modifiers.shift && current?.side === side) {
    const from = visible.findIndex((item) => item.path === anchor);
    const to = visible.findIndex((item) => item.path === entry.path);
    if (from >= 0 && to >= 0) {
      return {
        selection: {
          side,
          entries: visible.slice(Math.min(from, to), Math.max(from, to) + 1),
        },
        anchor: visible[from].path,
      };
    }
  }

  if (modifiers.additive && current?.side === side) {
    const paths = new Set(current.entries.map((item) => item.path));
    if (paths.has(entry.path)) paths.delete(entry.path);
    else paths.add(entry.path);
    return {
      selection: {
        side,
        entries: visible.filter((item) => paths.has(item.path)),
      },
      anchor: entry.path,
    };
  }

  return { selection: { side, entries: [entry] }, anchor: entry.path };
}
