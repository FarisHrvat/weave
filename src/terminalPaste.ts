/** How many lines a paste would send; more than one deserves a confirmation. */
export function pastedLines(text: string): number {
  const trimmed = text.replace(/(\r\n|\r|\n)$/, "");
  return trimmed ? trimmed.split(/\r\n|\r|\n/).length : 0;
}

/** Joins a multi-line paste into one line so nothing runs on its own. */
export function asOneLine(text: string): string {
  return text
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .join(" ");
}

/** Recent pastes, kept only in memory so pasted secrets are never written to disk. */
export class PasteHistory {
  private items: string[] = [];

  constructor(private readonly limit = 20) {}

  add(text: string) {
    if (!text.trim()) return;
    this.items = [text, ...this.items.filter((item) => item !== text)].slice(
      0,
      this.limit,
    );
  }

  list(): readonly string[] {
    return this.items;
  }

  clear() {
    this.items = [];
  }
}

export const pasteHistory = new PasteHistory();

/** Whether the line the cursor is on is asking for a password. */
export function isPasswordPrompt(line: string): boolean {
  return /(password|passphrase|passcode|pin)( for [^:]{1,80})?\s*:$/i.test(
    line.trimEnd(),
  );
}

export function durationText(milliseconds: number): string {
  const seconds = Math.round(milliseconds / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

export const minFontSize = 8;
export const maxFontSize = 36;

export function zoomedFont(size: number, step: number): number {
  return Math.min(maxFontSize, Math.max(minFontSize, size + step));
}

export type LineMatch = { line: number; text: string };

/** Lines containing the query, newest last, at most `limit` of them. */
export function findLines(
  lines: Iterable<string>,
  query: string,
  limit = 200,
): LineMatch[] {
  const needle = query.toLocaleLowerCase();
  if (!needle) return [];
  const found: LineMatch[] = [];
  let index = 0;
  for (const text of lines) {
    if (text.toLocaleLowerCase().includes(needle)) {
      found.push({ line: index, text });
      if (found.length > limit) found.shift();
    }
    index++;
  }
  return found;
}
