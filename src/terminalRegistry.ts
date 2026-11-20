import type { LineMatch } from "./terminalPaste";

export type SearchableTerminal = {
  find: (query: string) => LineMatch[];
  reveal: (line: number) => void;
};

const terminals = new Map<string, SearchableTerminal>();

/** Lets "Find in all terminals" reach every open terminal tab. */
export function registerTerminal(key: string, terminal: SearchableTerminal) {
  terminals.set(key, terminal);
  return () => {
    if (terminals.get(key) === terminal) terminals.delete(key);
  };
}

export function searchTerminals(query: string) {
  return [...terminals].map(([key, terminal]) => ({
    key,
    matches: terminal.find(query),
  }));
}

export function revealInTerminal(key: string, line: number) {
  terminals.get(key)?.reveal(line);
}
