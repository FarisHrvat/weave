import type { Profile } from "./types";

export type HostMeta = { group: string; tags: string[] };
export type HostMetaMap = Record<string, HostMeta>;

const storageKey = "weave-host-groups";

export function loadHostMeta(storage: Pick<Storage, "getItem">): HostMetaMap {
  try {
    const value: unknown = JSON.parse(storage.getItem(storageKey) ?? "{}");
    if (!value || typeof value !== "object") return {};
    const result: HostMetaMap = {};
    for (const [id, meta] of Object.entries(value)) {
      const group = typeof meta?.group === "string" ? meta.group : "";
      const tags = Array.isArray(meta?.tags)
        ? meta.tags.filter((tag: unknown) => typeof tag === "string")
        : [];
      result[id] = { group, tags };
    }
    return result;
  } catch {
    return {};
  }
}

export function saveHostMeta(
  storage: Pick<Storage, "setItem">,
  meta: HostMetaMap,
) {
  const kept = Object.fromEntries(
    Object.entries(meta).filter(([, item]) => item.group || item.tags.length),
  );
  storage.setItem(storageKey, JSON.stringify(kept));
}

/** Cleans up typed tags: trims, drops empties and duplicates, keeps order. */
export function parseTags(text: string): string[] {
  const seen = new Set<string>();
  return text
    .split(/[,\s]+/)
    .map((tag) => tag.replace(/^#/, "").trim().slice(0, 40))
    .filter((tag) => {
      const key = tag.toLowerCase();
      if (!tag || seen.has(key)) return false;
      seen.add(key);
      return true;
    });
}

/**
 * Hosts matching the search, grouped and sorted. "#tag" words must all
 * match a tag; other words match the name, address, user or group.
 */
export function groupHosts(
  profiles: Profile[],
  meta: HostMetaMap,
  query: string,
): { group: string; hosts: Profile[] }[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const tagWords = words
    .filter((word) => word.startsWith("#"))
    .map((word) => word.slice(1));
  const textWords = words.filter((word) => !word.startsWith("#"));
  const groups = new Map<string, Profile[]>();
  for (const profile of profiles) {
    const info = meta[profile.id] ?? { group: "", tags: [] };
    const tags = info.tags.map((tag) => tag.toLowerCase());
    const haystack = [
      profile.name,
      profile.host,
      profile.username,
      info.group,
      ...info.tags,
    ]
      .join(" ")
      .toLowerCase();
    if (!tagWords.every((tag) => tags.some((item) => item.startsWith(tag))))
      continue;
    if (!textWords.every((word) => haystack.includes(word))) continue;
    const list = groups.get(info.group) ?? [];
    list.push(profile);
    groups.set(info.group, list);
  }
  return [...groups.entries()]
    .sort(([a], [b]) =>
      a === ""
        ? 1
        : b === ""
          ? -1
          : a.localeCompare(b, undefined, { sensitivity: "base" }),
    )
    .map(([group, hosts]) => ({ group, hosts }));
}

export function knownGroups(meta: HostMetaMap): string[] {
  return [...new Set(Object.values(meta).map((item) => item.group))]
    .filter(Boolean)
    .sort((a, b) => a.localeCompare(b));
}
