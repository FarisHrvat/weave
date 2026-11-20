const listsWithHosts = [
  "weave-snippets",
  "weave-terminal-scripts",
  "weave-file-bookmarks",
  "weave-remote-connections",
  "weave-tunnel-presets",
];

function readJson(storage: Pick<Storage, "getItem">, key: string): unknown {
  try {
    return JSON.parse(storage.getItem(key) ?? "null");
  } catch {
    return null;
  }
}

/** Removes everything this Mac's local storage keeps for a deleted host. */
export function forgetHostLocally(
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem">,
  profileId: string,
) {
  storage.removeItem(`weave-terminal-options:${encodeURIComponent(profileId)}`);
  for (const key of ["weave-terminal-history", "weave-host-groups"]) {
    const value = readJson(storage, key);
    if (value && typeof value === "object" && !Array.isArray(value)) {
      const record = value as Record<string, unknown>;
      delete record[`ssh:${profileId}`];
      delete record[profileId];
      storage.setItem(key, JSON.stringify(record));
    }
  }
  for (const key of listsWithHosts) {
    const value = readJson(storage, key);
    if (Array.isArray(value))
      storage.setItem(
        key,
        JSON.stringify(
          value.filter(
            (item) =>
              !(
                item &&
                typeof item === "object" &&
                item.profileId === profileId
              ),
          ),
        ),
      );
  }
}
