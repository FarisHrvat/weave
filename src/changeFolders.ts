import type { GitFile } from "./types";

export type ChangeItem =
  | { kind: "folder"; folder: string; count: number }
  | { kind: "file"; file: GitFile };

function folderOf(path: string) {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  const index = trimmed.lastIndexOf("/");
  return index < 0 ? "" : trimmed.slice(0, index);
}

/**
 * Orders changed files by folder and inserts a heading before each folder's
 * files, so a whole folder can be staged, unstaged or discarded at once.
 */
export function withFolders(files: GitFile[], grouped: boolean): ChangeItem[] {
  if (!grouped) return files.map((file) => ({ kind: "file", file }));
  const sorted = [...files].sort(
    (a, b) =>
      folderOf(a.path).localeCompare(folderOf(b.path)) ||
      a.path.localeCompare(b.path),
  );
  const items: ChangeItem[] = [];
  let current: string | undefined;
  for (const file of sorted) {
    const folder = folderOf(file.path);
    if (folder !== current) {
      current = folder;
      if (folder)
        items.push({
          kind: "folder",
          folder,
          count: sorted.filter((item) => folderOf(item.path) === folder).length,
        });
    }
    items.push({ kind: "file", file });
  }
  return items;
}
