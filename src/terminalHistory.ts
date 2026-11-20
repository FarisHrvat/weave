/**
 * Command history and autocomplete for terminals.
 *
 * Commands are learned from typed input only when the shell echoed them back
 * on screen, so text typed at password prompts (which is not echoed) is never
 * stored. When shell integration is installed, input is also only recorded
 * while the shell reports it is at a prompt.
 */

const storageKey = "weave-terminal-history";
export const maxHistory = 500;
const maxCommandLength = 500;

export function historyKey(profileId?: string | null) {
  return profileId ? `ssh:${profileId}` : "local";
}

type HistoryStore = Record<string, string[]>;

function readStore(storage: Pick<Storage, "getItem">): HistoryStore {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(storageKey) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
      return {};
    const store: HistoryStore = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (Array.isArray(value))
        store[key] = value
          .filter(
            (item): item is string =>
              typeof item === "string" && item.length <= maxCommandLength,
          )
          .slice(-maxHistory);
    }
    return store;
  } catch {
    return {};
  }
}

export function readHistory(
  storage: Pick<Storage, "getItem">,
  key: string,
): string[] {
  return readStore(storage)[key] ?? [];
}

export function addToHistory(
  storage: Pick<Storage, "getItem" | "setItem">,
  key: string,
  command: string,
): string[] {
  const trimmed = command.trim();
  const store = readStore(storage);
  const current = store[key] ?? [];
  if (
    !trimmed ||
    trimmed.length > maxCommandLength ||
    // Commands starting with a space are private by shell convention.
    command.startsWith(" ")
  )
    return current;
  const next = [...current.filter((item) => item !== trimmed), trimmed].slice(
    -maxHistory,
  );
  store[key] = next;
  storage.setItem(storageKey, JSON.stringify(store));
  return next;
}

export function clearHistory(
  storage: Pick<Storage, "getItem" | "setItem">,
  key: string,
) {
  const store = readStore(storage);
  delete store[key];
  storage.setItem(storageKey, JSON.stringify(store));
}

/**
 * Ranks completions: commands that start with the typed text first (most
 * recent first), then commands that contain every typed word.
 */
export function suggest(
  typed: string,
  candidates: string[],
  limit = 8,
): string[] {
  const query = typed.trim().toLocaleLowerCase();
  const recent = [...candidates].reverse();
  const unique = [...new Set(recent)];
  if (!query) return unique.slice(0, limit);
  const words = query.split(/\s+/);
  const prefix = unique.filter((item) =>
    item.toLocaleLowerCase().startsWith(query),
  );
  const fuzzy = unique.filter(
    (item) =>
      !prefix.includes(item) &&
      words.every((word) => item.toLocaleLowerCase().includes(word)),
  );
  return [...prefix, ...fuzzy]
    .filter((item) => item.trim() !== typed.trim())
    .slice(0, limit);
}

/**
 * Tracks the command line being typed from terminal input data. It gives up
 * (returns to an unknown state) on cursor movement or other editing keys it
 * cannot follow, so the tracked text is never wrong, only sometimes unknown.
 */
export class InputTracker {
  private buffer = "";
  private known = true;

  get current(): string | null {
    return this.known ? this.buffer : null;
  }

  reset() {
    this.buffer = "";
    this.known = true;
  }

  /** Feeds typed data; returns the command when Enter is pressed. */
  feed(data: string): string | null {
    // Bracketed paste and plain multi-character input are appended as text.
    const text = data.replace(/\x1b\[20[01]~/g, "");
    let submitted: string | null = null;
    for (let index = 0; index < text.length; index++) {
      const char = text[index];
      if (char === "\r" || char === "\n") {
        submitted = this.known ? this.buffer : null;
        this.reset();
      } else if (char === "\x7f" || char === "\b") {
        this.buffer = this.buffer.slice(0, -1);
      } else if (char === "\x15" || char === "\x03") {
        // Ctrl+U clears the line; Ctrl+C abandons it.
        this.reset();
      } else if (char === "\x1b" || char < " ") {
        // Arrow keys, history recall, tab completion and other editing.
        this.known = false;
        if (char === "\x1b") {
          const match = /^\x1b(\[[0-9;]*[A-Za-z~]|O[A-Za-z]|.)/.exec(
            text.slice(index),
          );
          index += (match?.[0].length ?? 1) - 1;
        }
      } else {
        this.buffer += char;
      }
    }
    return submitted;
  }
}

/** True when the visible prompt line ends with the typed command. */
export function wasEchoed(visibleLine: string, command: string): boolean {
  const trimmed = command.trim();
  return trimmed.length > 0 && visibleLine.trimEnd().endsWith(trimmed);
}
