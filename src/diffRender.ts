import hljs from "highlight.js/lib/common";

export type Segment = { text: string; changed: boolean };

const languages: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  py: "python",
  rb: "ruby",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  m: "objectivec",
  mm: "objectivec",
  cs: "csharp",
  php: "php",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  css: "css",
  scss: "scss",
  less: "less",
  html: "xml",
  htm: "xml",
  xml: "xml",
  svg: "xml",
  vue: "xml",
  md: "markdown",
  markdown: "markdown",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  sql: "sql",
  lua: "lua",
  pl: "perl",
  r: "r",
  diff: "diff",
  patch: "diff",
  graphql: "graphql",
  gql: "graphql",
};

const names: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  "cmakelists.txt": "cmake",
};

/** The highlight.js language for a file name, if it has one. */
export function languageFor(file: string): string | undefined {
  const name = file.split("/").pop()?.toLowerCase() ?? "";
  if (names[name]) return names[name];
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return undefined;
  const language = languages[name.slice(dot + 1)];
  return language && hljs.getLanguage(language) ? language : undefined;
}

const imageExtensions = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "ico",
  "svg",
  "avif",
]);

export function isImage(file: string): boolean {
  const name = file.toLowerCase();
  return imageExtensions.has(name.slice(name.lastIndexOf(".") + 1));
}

export function escapeHtml(text: string): string {
  return text.replace(
    /[&<>"']/g,
    (char) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[char]!,
  );
}

/** Highlighted HTML for one line of code; plain escaped text without a language. */
export function highlight(text: string, language?: string): string {
  if (!language || !text) return escapeHtml(text);
  try {
    return hljs.highlight(text, { language, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(text);
  }
}

/** Highlighted HTML for a line, with the changed words wrapped in <mark>. */
export function segmentsHtml(segments: Segment[], language?: string): string {
  return segments
    .map((segment) =>
      segment.changed
        ? `<mark>${highlight(segment.text, language)}</mark>`
        : highlight(segment.text, language),
    )
    .join("");
}

function tokens(line: string): string[] {
  return line.match(/\w+|\s+|[^\w\s]/g) ?? [];
}

function merge(parts: Segment[]): Segment[] {
  const merged: Segment[] = [];
  for (const part of parts) {
    const last = merged[merged.length - 1];
    if (last && last.changed === part.changed) last.text += part.text;
    else if (part.text) merged.push({ ...part });
  }
  return merged;
}

/**
 * Which words changed between an old and a new line. Long lines fall back to
 * comparing the shared start and end, which keeps this fast.
 */
export function wordDiff(
  before: string,
  after: string,
): { before: Segment[]; after: Segment[] } {
  const a = tokens(before);
  const b = tokens(after);
  if (a.length * b.length > 40_000) {
    let start = 0;
    while (start < before.length && before[start] === after[start]) start++;
    let end = 0;
    while (
      end < before.length - start &&
      end < after.length - start &&
      before[before.length - 1 - end] === after[after.length - 1 - end]
    )
      end++;
    const split = (line: string) =>
      merge([
        { text: line.slice(0, start), changed: false },
        { text: line.slice(start, line.length - end), changed: true },
        { text: line.slice(line.length - end), changed: false },
      ]);
    return { before: split(before), after: split(after) };
  }
  const lengths = Array.from({ length: a.length + 1 }, () =>
    new Array<number>(b.length + 1).fill(0),
  );
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--)
      lengths[i][j] =
        a[i] === b[j]
          ? lengths[i + 1][j + 1] + 1
          : Math.max(lengths[i + 1][j], lengths[i][j + 1]);
  const left: Segment[] = [];
  const right: Segment[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      left.push({ text: a[i++], changed: false });
      right.push({ text: b[j++], changed: false });
    } else if (
      j < b.length &&
      (i === a.length || lengths[i][j + 1] >= lengths[i + 1][j])
    ) {
      right.push({ text: b[j++], changed: true });
    } else {
      left.push({ text: a[i++], changed: true });
    }
  }
  return { before: merge(left), after: merge(right) };
}

/**
 * Word changes for the lines of one hunk (each line keeps its +, - or space
 * prefix). Removed lines are paired in order with the added lines that follow
 * them; lines without a partner get no entry.
 */
export function pairedWordDiffs(lines: string[]): Map<number, Segment[]> {
  const result = new Map<number, Segment[]>();
  let index = 0;
  while (index < lines.length) {
    if (!lines[index].startsWith("-")) {
      index++;
      continue;
    }
    const removedStart = index;
    while (index < lines.length && lines[index].startsWith("-")) index++;
    const addedStart = index;
    while (index < lines.length && lines[index].startsWith("+")) index++;
    const pairs = Math.min(addedStart - removedStart, index - addedStart);
    for (let pair = 0; pair < pairs; pair++) {
      const { before, after } = wordDiff(
        lines[removedStart + pair].slice(1),
        lines[addedStart + pair].slice(1),
      );
      // A line rewritten almost entirely reads better without marks.
      const changedShare = (segments: Segment[]) =>
        segments
          .filter((segment) => segment.changed)
          .reduce((sum, segment) => sum + segment.text.length, 0) /
        Math.max(
          1,
          segments.reduce((sum, s) => sum + s.text.length, 0),
        );
      if (changedShare(before) > 0.7 && changedShare(after) > 0.7) continue;
      result.set(removedStart + pair, before);
      result.set(addedStart + pair, after);
    }
  }
  return result;
}

export type FileDiff = { path: string; header: string; body: string };

/** Splits a multi-file unified diff into one entry per file. */
export function splitFiles(diff: string): FileDiff[] {
  const files: FileDiff[] = [];
  for (const chunk of diff.split(/^(?=diff --git )/m)) {
    if (!chunk.trim()) continue;
    const firstHunk = chunk.search(/^@@ /m);
    const header = firstHunk < 0 ? chunk : chunk.slice(0, firstHunk);
    const body = firstHunk < 0 ? "" : chunk.slice(firstHunk);
    const plus = /^\+\+\+ (?:b\/)?(.+)$/m.exec(header)?.[1];
    const minus = /^--- (?:a\/)?(.+)$/m.exec(header)?.[1];
    const named = /^diff --git a\/(.+?) b\/(.+)$/m.exec(header)?.[2];
    const path =
      (plus && plus !== "/dev/null" ? plus : undefined) ??
      (minus && minus !== "/dev/null" ? minus : undefined) ??
      named ??
      "";
    files.push({ path, header, body });
  }
  return files;
}
