export function broadcastTargets(
  source: string,
  terminals: ReadonlyMap<string, string>,
  ended: ReadonlySet<string>,
  excluded: ReadonlySet<string>,
): string[] {
  if (excluded.has(source)) return [];
  return [...terminals]
    .filter(([key]) => key !== source && !ended.has(key) && !excluded.has(key))
    .map(([, id]) => id);
}
