/** Local settings that belong in a settings vault (never command history). */
export function collectVaultSettings(
  storage: Pick<Storage, "length" | "key" | "getItem">,
): Record<string, string> {
  const settings: Record<string, string> = {};
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (!key || !key.startsWith("weave-") || key === "weave-terminal-history")
      continue;
    const value = storage.getItem(key);
    if (value !== null) settings[key] = value;
  }
  return settings;
}

export function applyVaultSettings(
  storage: Pick<Storage, "setItem">,
  settings: Record<string, string>,
): number {
  let count = 0;
  for (const [key, value] of Object.entries(settings)) {
    if (!key.startsWith("weave-") || key === "weave-terminal-history") continue;
    storage.setItem(key, value);
    count++;
  }
  return count;
}
