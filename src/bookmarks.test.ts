import { describe, expect, it } from "vitest";
import { bookmarkFor, loadBookmarks, saveBookmarks } from "./bookmarks";

describe("folder bookmarks", () => {
  it("persists and matches a folder on its exact connection", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    const bookmarks = [
      { id: "one", name: "project", path: "/project", profileId: "host-a" },
    ];
    saveBookmarks(storage, bookmarks);

    expect(loadBookmarks(storage)).toEqual(bookmarks);
    expect(bookmarkFor(loadBookmarks(storage), bookmarks[0])).toEqual(
      bookmarks[0],
    );
    expect(
      bookmarkFor(loadBookmarks(storage), {
        path: "/project",
        profileId: "host-b",
      }),
    ).toBeUndefined();
  });

  it("ignores damaged or unrelated stored data", () => {
    expect(loadBookmarks({ getItem: () => "not json" })).toEqual([]);
    expect(
      loadBookmarks({ getItem: () => JSON.stringify([{ path: 12 }]) }),
    ).toEqual([]);
  });
});
