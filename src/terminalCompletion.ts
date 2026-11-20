const quote = (value: string) => `'${value.replace(/'/g, `'\\''`)}'`;

/** The word being completed, and whether it is the command itself. */
export function currentWord(line: string): { word: string; command: boolean } {
  const match = /(\S*)$/.exec(line);
  const word = match ? match[1] : "";
  const before = line.slice(0, line.length - word.length);
  const command = before.trim() === "" || /[|;&]\s*$/.test(before);
  return { word, command };
}

/**
 * A shell script that prints completions for `word` on the host: command
 * names, or files with a trailing slash for folders.
 */
export function completionScript(word: string, command: boolean): string {
  const mode = command ? "c" : "f";
  const body = [
    'w=$1; home=""',
    'case $w in "~/"*) home=$HOME; w="$HOME/${w#"~/"}";; esac',
    'if [ "$2" = c ]; then compgen -c -- "$w"; else',
    '  compgen -f -- "$w" | while IFS= read -r f; do',
    '    if [ -d "$f" ]; then f="$f/"; fi',
    '    if [ -n "$home" ]; then f="~/${f#"$home"/}"; fi',
    '    printf "%s\\n" "$f"',
    "  done",
    "fi",
  ].join("\n");
  return [
    "if command -v bash >/dev/null 2>&1; then",
    `  bash -c ${quote(body)} _ ${quote(word)} ${mode} 2>/dev/null | sort -u | head -n 200`,
    "fi",
  ].join("\n");
}

/** Turns host output into full command lines that extend what was typed. */
export function completionLines(
  typed: string,
  word: string,
  output: string,
): string[] {
  const prefix = typed.slice(0, typed.length - word.length);
  return output
    .split("\n")
    .map((item) => item.trim())
    .filter((item) => item && item !== word && item.startsWith(word))
    .map((item) => {
      const value = /[\s'"$`\\]/.test(item)
        ? item.replace(/([\s'"$`\\])/g, "\\$1")
        : item;
      return prefix + value;
    });
}
