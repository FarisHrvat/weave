import { createSignal } from "solid-js";
import { confirm } from "@tauri-apps/plugin-dialog";

/**
 * Asks yes or no with a native macOS dialog. The web view's own confirm()
 * cannot be relied on inside the app, so every confirmation comes here.
 */
export async function confirmAction(
  message: string,
  okLabel = "Continue",
): Promise<boolean> {
  if (!("__TAURI_INTERNALS__" in window)) return window.confirm(message);
  try {
    return await confirm(message, {
      title: "Weave",
      kind: "warning",
      okLabel,
      cancelLabel: "Cancel",
    });
  } catch {
    return false;
  }
}

export type TextRequest = {
  message: string;
  value: string;
  resolve: (value: string | null) => void;
};

const [textRequest, setTextRequest] = createSignal<TextRequest>();
export { textRequest };

/** Asks for a line of text in an in-app dialog; null when cancelled. */
export function askText(message: string, value = ""): Promise<string | null> {
  return new Promise((resolve) => {
    textRequest()?.resolve(null);
    setTextRequest({ message, value, resolve });
  });
}

export function finishText(value: string | null) {
  const request = textRequest();
  setTextRequest(undefined);
  request?.resolve(value);
}
