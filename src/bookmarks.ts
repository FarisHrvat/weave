import type { FileLocation } from "./types";

export type FileBookmark = FileLocation & { id: string; name: string };

const storageKey = "weave-file-bookmarks";

export function loadBookmarks(
  storage: Pick<Storage, "getItem">,
): FileBookmark[] {
  const saved = storage.getItem(storageKey);
  if (!saved) return [];
  try {
    const value: unknown = JSON.parse(saved);
    if (!Array.isArray(value)) return [];
    return value.filter(
      (item): item is FileBookmark =>
        typeof item === "object" &&
        item !== null &&
        typeof item.id === "string" &&
        typeof item.name === "string" &&
        typeof item.path === "string" &&
        (item.profileId === null || typeof item.profileId === "string"),
    );
  } catch {
    return [];
  }
}

export function saveBookmarks(
  storage: Pick<Storage, "setItem">,
  bookmarks: FileBookmark[],
) {
  storage.setItem(storageKey, JSON.stringify(bookmarks));
}

export function bookmarkFor(bookmarks: FileBookmark[], location: FileLocation) {
  return bookmarks.find(
    (bookmark) =>
      bookmark.path === location.path &&
      bookmark.profileId === location.profileId,
  );
}
