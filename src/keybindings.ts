import { isMac } from "./platform";
import { shortcutFromEvent } from "./shortcuts";

export const keyActions = [
  {
    id: "palette",
    label: "Command palette",
    key: "Super+KeyK",
    pc: "Control+Shift+KeyP",
  },
  {
    id: "home",
    label: "Go to Home",
    key: "Super+Digit1",
    pc: "Control+Shift+Digit1",
  },
  {
    id: "terminal",
    label: "Go to Terminal",
    key: "Super+Digit2",
    pc: "Control+Shift+Digit2",
  },
  {
    id: "files",
    label: "Go to Files",
    key: "Super+Digit3",
    pc: "Control+Shift+Digit3",
  },
  {
    id: "git",
    label: "Go to Git",
    key: "Super+Digit4",
    pc: "Control+Shift+Digit4",
  },
  {
    id: "newTerminal",
    label: "New local terminal tab",
    key: "Super+KeyT",
    pc: "Control+Shift+KeyT",
  },
  {
    id: "closeTab",
    label: "Close terminal tab",
    key: "Super+Shift+KeyW",
    pc: "Control+Shift+KeyW",
  },
  {
    id: "nextTab",
    label: "Next terminal tab",
    key: "Control+Tab",
    pc: "Control+Tab",
  },
  {
    id: "previousTab",
    label: "Previous terminal tab",
    key: "Control+Shift+Tab",
    pc: "Control+Shift+Tab",
  },
  {
    id: "findTerminals",
    label: "Find in all terminal tabs",
    key: "Super+Alt+KeyF",
    pc: "Control+Alt+Shift+KeyF",
  },
  {
    id: "splitRight",
    label: "Split terminal right",
    key: "Super+KeyD",
    pc: "Control+Shift+KeyD",
  },
  {
    id: "splitDown",
    label: "Split terminal down",
    key: "Super+Shift+KeyD",
    pc: "Control+Alt+Shift+KeyD",
  },
  {
    id: "broadcast",
    label: "Broadcast input on or off",
    key: "Super+Alt+KeyI",
    pc: "Control+Shift+KeyI",
  },
  {
    id: "newConnection",
    label: "New connection…",
    key: "Super+Shift+KeyN",
    pc: "Control+Shift+KeyN",
  },
  {
    id: "addHost",
    label: "Add a host",
    key: "Super+Alt+KeyN",
    pc: "Control+Alt+KeyN",
  },
  { id: "theme", label: "Light or dark theme", key: "", pc: "" },
  {
    id: "lock",
    label: "Lock Weave",
    key: "Super+Control+KeyL",
    pc: "Control+Alt+KeyL",
  },
] as const;

export type KeyAction = (typeof keyActions)[number]["id"];
export type Keybindings = Record<KeyAction, string>;

const storageKey = "weave-keybindings";

/**
 * Defaults use ⌘ on a Mac. Elsewhere they use Ctrl+Shift, as other terminal
 * apps do, so Ctrl+C, Ctrl+D and the rest still reach the shell.
 */
export function defaultKeybindings(mac = isMac): Keybindings {
  return Object.fromEntries(
    keyActions.map((action) => [action.id, mac ? action.key : action.pc]),
  ) as Keybindings;
}

/** Saved changes on top of the defaults; unknown entries are ignored. */
export function loadKeybindings(
  storage: Pick<Storage, "getItem">,
): Keybindings {
  const bindings = defaultKeybindings();
  try {
    const saved: unknown = JSON.parse(storage.getItem(storageKey) ?? "{}");
    if (saved && typeof saved === "object")
      for (const [id, key] of Object.entries(saved))
        if (id in bindings && typeof key === "string")
          bindings[id as KeyAction] = key;
  } catch {
    // Keep the defaults when the saved value is unreadable.
  }
  return bindings;
}

export function saveKeybindings(
  storage: Pick<Storage, "setItem">,
  bindings: Keybindings,
) {
  const defaults = defaultKeybindings();
  const changed = Object.fromEntries(
    Object.entries(bindings).filter(
      ([id, key]) => defaults[id as KeyAction] !== key,
    ),
  );
  storage.setItem(storageKey, JSON.stringify(changed));
}

export function actionFor(
  bindings: Keybindings,
  event: Parameters<typeof shortcutFromEvent>[0],
): KeyAction | undefined {
  const pressed = shortcutFromEvent(event);
  if (!pressed) return undefined;
  return (Object.keys(bindings) as KeyAction[]).find(
    (id) => bindings[id] === pressed,
  );
}

/** The action already using this shortcut, if any. */
export function conflictFor(
  bindings: Keybindings,
  action: KeyAction,
  shortcut: string,
): KeyAction | undefined {
  return (Object.keys(bindings) as KeyAction[]).find(
    (id) => id !== action && shortcut && bindings[id] === shortcut,
  );
}
