/** Which system Weave runs on, and the words and keys that differ. */

export type Os = "mac" | "windows" | "linux";

export function detectOs(userAgent: string): Os {
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "mac";
  return "linux";
}

export const os: Os = detectOs(
  typeof navigator === "undefined" ? "Macintosh" : navigator.userAgent,
);
export const isMac = os === "mac";

/** "this Mac", "this PC" or "this computer". */
export const thisComputer =
  os === "mac" ? "this Mac" : os === "windows" ? "this PC" : "this computer";

/** Finder, File Explorer or the Linux file manager. */
export const fileManager =
  os === "mac" ? "Finder" : os === "windows" ? "File Explorer" : "Files";

/** Where passwords and tokens are kept. */
export const keychain =
  os === "mac"
    ? "the macOS Keychain"
    : os === "windows"
      ? "Windows Credential Manager"
      : "the system keyring";

/** Whether the main shortcut modifier (⌘ on a Mac, Ctrl elsewhere) is held. */
export function primaryKey(event: { metaKey: boolean; ctrlKey: boolean }) {
  return isMac ? event.metaKey : event.ctrlKey;
}

/** "This Mac", "This PC" or "This computer", to start a label. */
export const ThisComputer =
  thisComputer.charAt(0).toUpperCase() + thisComputer.slice(1);

/** The system's name for messages: macOS, Windows or Linux. */
export const systemName =
  os === "mac" ? "macOS" : os === "windows" ? "Windows" : "Linux";

/** Where client certificates live, or null on Linux, which has no shared store. */
export const certificateStore =
  os === "mac"
    ? "Keychain"
    : os === "windows"
      ? "Windows certificate store"
      : null;
