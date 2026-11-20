/** Splits command text into arguments, honouring single and double quotes. */
export function splitArgs(text: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let started = false;
  for (const char of text.trim()) {
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
    } else if (char === '"' || char === "'") {
      quote = char;
      started = true;
    } else if (/\s/.test(char)) {
      if (started) args.push(current);
      current = "";
      started = false;
    } else {
      current += char;
      started = true;
    }
  }
  if (quote) throw new Error("A quote is not closed.");
  if (started) args.push(current);
  return args;
}

export type CustomGitCommand = { id: string; name: string; command: string };
const storageKey = "weave-git-custom-commands";

export function readCustomCommands(
  storage: Pick<Storage, "getItem">,
): CustomGitCommand[] {
  try {
    const value: unknown = JSON.parse(storage.getItem(storageKey) ?? "[]");
    return Array.isArray(value)
      ? value.filter(
          (item): item is CustomGitCommand =>
            typeof item?.id === "string" &&
            typeof item?.name === "string" &&
            typeof item?.command === "string",
        )
      : [];
  } catch {
    return [];
  }
}

export function writeCustomCommands(
  storage: Pick<Storage, "setItem">,
  commands: CustomGitCommand[],
) {
  storage.setItem(storageKey, JSON.stringify(commands.slice(0, 50)));
}
