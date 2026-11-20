export type Snippet = {
  id: string;
  name: string;
  body: string;
  profileId: string | null;
  pinned?: boolean;
};

const storageKey = "weave-snippets";

export function readSnippets(): Snippet[] {
  const saved = localStorage.getItem(storageKey);
  if (!saved) return [];
  try {
    const result: unknown = JSON.parse(saved);
    if (
      Array.isArray(result) &&
      result.every(
        (item) =>
          typeof item.id === "string" &&
          typeof item.name === "string" &&
          typeof item.body === "string",
      )
    ) {
      return result as Snippet[];
    }
  } catch {
    /* Invalid saved data is reported below. */
  }
  throw new Error(
    "Saved snippets could not be read. The stored data appears damaged.",
  );
}

export function writeSnippets(snippets: Snippet[]): void {
  localStorage.setItem(storageKey, JSON.stringify(snippets));
}

export function pinnedSnippets(
  snippets: Snippet[],
  profileId?: string,
): Snippet[] {
  return snippets.filter(
    (item) => item.pinned && (!item.profileId || item.profileId === profileId),
  );
}

export function snippetVariables(body: string): string[] {
  return [
    ...new Set(
      [...body.matchAll(/{{\s*([a-zA-Z][\w]*)\s*}}/g)].map((match) => match[1]),
    ),
  ];
}

export function fillSnippet(
  body: string,
  values: Record<string, string>,
): string {
  return body.replace(/{{\s*([a-zA-Z][\w]*)\s*}}/g, (_, name: string) => {
    if (!values[name]?.trim()) throw new Error(`Enter a value for ${name}.`);
    return values[name];
  });
}
