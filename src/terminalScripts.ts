export type SavedTerminalScript = {
  id: string;
  name: string;
  profileId: string | null;
  cwd: string;
  body: string;
};

const storageKey = "weave-terminal-scripts";
const maxScripts = 30;
const maxScriptBytes = 64 * 1024;

function validateScript(value: unknown): value is SavedTerminalScript {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.id === "string" &&
    item.id.length > 0 &&
    typeof item.name === "string" &&
    item.name.trim().length > 0 &&
    item.name.length <= 80 &&
    (item.profileId === null || typeof item.profileId === "string") &&
    typeof item.cwd === "string" &&
    item.cwd.length <= 1024 &&
    typeof item.body === "string" &&
    item.body.trim().length > 0 &&
    new TextEncoder().encode(item.body).length <= maxScriptBytes
  );
}

export function readTerminalScripts(
  storage: Pick<Storage, "getItem">,
): SavedTerminalScript[] {
  const saved = storage.getItem(storageKey);
  if (!saved) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(saved);
  } catch {
    throw new Error(
      "Saved terminal scripts are damaged. Use Reset saved scripts in the Script panel to clear them.",
    );
  }
  if (
    !Array.isArray(parsed) ||
    parsed.length > maxScripts ||
    !parsed.every(validateScript) ||
    new Set(parsed.map((item) => item.id)).size !== parsed.length
  ) {
    throw new Error(
      "Saved terminal scripts have an invalid format. Use Reset saved scripts in the Script panel to clear them.",
    );
  }
  return parsed;
}

export function writeTerminalScripts(
  storage: Pick<Storage, "setItem">,
  scripts: SavedTerminalScript[],
): void {
  if (scripts.length > maxScripts) {
    throw new Error("You can save up to 30 terminal scripts.");
  }
  if (!scripts.every(validateScript)) {
    throw new Error(
      "Give each script a name of 1–80 characters, a working folder under 1,024 characters, and a body of 1 byte to 64 KB.",
    );
  }
  if (new Set(scripts.map((item) => item.id)).size !== scripts.length) {
    throw new Error(
      "Saved scripts contain duplicate IDs. Reload them and try again.",
    );
  }
  storage.setItem(storageKey, JSON.stringify(scripts));
}

export function scriptsForProfile(
  scripts: SavedTerminalScript[],
  profileId: string | null,
): SavedTerminalScript[] {
  return scripts.filter((item) => item.profileId === profileId);
}
