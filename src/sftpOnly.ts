const storageKey = "weave-sftp-only";

export function readSftpOnly(storage: Pick<Storage, "getItem">): Set<string> {
  try {
    const value: unknown = JSON.parse(storage.getItem(storageKey) ?? "[]");
    return new Set(
      Array.isArray(value)
        ? value.filter((item): item is string => typeof item === "string")
        : [],
    );
  } catch {
    return new Set();
  }
}

export function writeSftpOnly(
  storage: Pick<Storage, "setItem">,
  hosts: Set<string>,
) {
  storage.setItem(storageKey, JSON.stringify([...hosts]));
}

/** Whether a server's greeting says it only offers file transfer. */
export function saysSftpOnly(output: string): boolean {
  return /\b(only\s+(allows\s+)?sftp|sftp\s+(connections\s+)?only|allows\s+sftp\s+connections\s+only|this\s+account\s+is\s+(restricted|limited)\s+to\s+sftp)\b/i.test(
    output,
  );
}
