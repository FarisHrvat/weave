import { isMac } from "./platform";

type KeyEventLike = Pick<
  KeyboardEvent,
  "code" | "metaKey" | "altKey" | "ctrlKey" | "shiftKey"
>;

const modifierCodes = new Set([
  "MetaLeft",
  "MetaRight",
  "AltLeft",
  "AltRight",
  "ControlLeft",
  "ControlRight",
  "ShiftLeft",
  "ShiftRight",
  "CapsLock",
  "Fn",
]);

/** Turns a key press into a shortcut such as "Super+Alt+KeyT", or null while only modifiers are held. */
export function shortcutFromEvent(event: KeyEventLike): string | null {
  if (!event.code || modifierCodes.has(event.code)) return null;
  const parts = [
    event.metaKey && "Super",
    event.ctrlKey && "Control",
    event.altKey && "Alt",
    event.shiftKey && "Shift",
  ].filter(Boolean) as string[];
  return [...parts, event.code].join("+");
}

const symbols: Record<string, string> = {
  Super: "⌘",
  Control: "⌃",
  Alt: "⌥",
  Shift: "⇧",
};

const names: Record<string, string> = {
  Super: "Win",
  Control: "Ctrl",
  Alt: "Alt",
  Shift: "Shift",
};

/** Fixed shortcuts that use ⌘ on a Mac and Ctrl+Shift elsewhere. */
export const fixedShortcuts = {
  find: isMac ? "Super+KeyF" : "Control+Shift+KeyF",
  promptUp: isMac ? "Super+ArrowUp" : "Control+Shift+ArrowUp",
  promptDown: isMac ? "Super+ArrowDown" : "Control+Shift+ArrowDown",
  complete: "Control+Space",
  preview: isMac ? "Super+KeyY" : "Control+KeyY",
  findFiles: isMac ? "Super+Shift+KeyF" : "Control+Shift+KeyF",
  zoomIn: isMac ? "Super+Equal" : "Control+Equal",
  zoomOut: isMac ? "Super+Minus" : "Control+Minus",
  zoomReset: isMac ? "Super+Digit0" : "Control+Digit0",
  copy: isMac ? "Super+KeyC" : "Control+Shift+KeyC",
  paste: isMac ? "Super+KeyV" : "Control+Shift+KeyV",
};

/** A readable label: "⌘⌥T" on a Mac, "Ctrl+Alt+T" elsewhere. */
export function shortcutLabel(shortcut: string, mac = isMac): string {
  if (!shortcut) return "";
  const parts = shortcut.split("+").map(
    (part) =>
      (mac ? symbols : names)[part] ??
      part
        .replace(/^Key/, "")
        .replace(/^Digit/, "")
        .replace("Backquote", "`")
        .replace("ArrowUp", "↑")
        .replace("ArrowDown", "↓")
        .replace("Equal", "+")
        .replace("Minus", "−"),
  );
  return parts.join(mac ? "" : "+");
}
