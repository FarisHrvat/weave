export type TerminalOptions = {
  fontSize: number;
  scrollback: number;
  theme: "midnight" | "light" | "black";
  cursorStyle: "block" | "underline" | "bar";
  cursorBlink: boolean;
};

export const defaultTerminalOptions: TerminalOptions = {
  fontSize: 13,
  scrollback: 100_000,
  theme: "midnight",
  cursorStyle: "block",
  cursorBlink: true,
};

const keyFor = (profileId?: string) =>
  `weave-terminal-options:${encodeURIComponent(profileId ?? "local")}`;

function valid(value: unknown): value is TerminalOptions {
  if (!value || typeof value !== "object") return false;
  const options = value as Partial<TerminalOptions>;
  return (
    typeof options.fontSize === "number" &&
    Number.isInteger(options.fontSize) &&
    options.fontSize! >= 10 &&
    options.fontSize! <= 24 &&
    [5_000, 20_000, 100_000].includes(options.scrollback!) &&
    ["midnight", "light", "black"].includes(options.theme!) &&
    ["block", "underline", "bar"].includes(options.cursorStyle!) &&
    typeof options.cursorBlink === "boolean"
  );
}

export function readTerminalOptions(
  storage: Pick<Storage, "getItem">,
  profileId?: string,
): TerminalOptions {
  const saved = storage.getItem(keyFor(profileId));
  if (!saved) return { ...defaultTerminalOptions };
  let value: unknown;
  try {
    value = JSON.parse(saved);
  } catch {
    throw new Error(
      "Saved terminal settings are damaged. Open Appearance and choose new settings to replace them.",
    );
  }
  if (!valid(value)) {
    throw new Error(
      "Saved terminal settings have an invalid format. Open Appearance and choose new settings to replace them.",
    );
  }
  return value;
}

export function writeTerminalOptions(
  storage: Pick<Storage, "setItem">,
  profileId: string | undefined,
  options: TerminalOptions,
): void {
  if (!valid(options))
    throw new Error("Choose valid terminal settings before saving.");
  storage.setItem(keyFor(profileId), JSON.stringify(options));
}
