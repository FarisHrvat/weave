export function markTerminalActivity(
  current: Set<string>,
  key: string,
  visible: boolean,
): Set<string> {
  if (visible || current.has(key)) return current;
  return new Set(current).add(key);
}

export function clearTerminalActivity(
  current: Set<string>,
  keys: (string | undefined)[],
): Set<string> {
  if (!keys.some((key) => key && current.has(key))) return current;
  const next = new Set(current);
  for (const key of keys) if (key) next.delete(key);
  return next;
}
