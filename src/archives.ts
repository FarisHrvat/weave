const suffixes = [
  ".zip",
  ".tar",
  ".tar.gz",
  ".tgz",
  ".tar.bz2",
  ".tbz2",
  ".tar.xz",
  ".txz",
];

/** Whether Weave can extract this file. */
export function isArchive(name: string): boolean {
  const lower = name.toLowerCase();
  return suffixes.some(
    (suffix) => lower.endsWith(suffix) && lower.length > suffix.length,
  );
}
