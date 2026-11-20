const focusable =
  'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The dialog on top, if one is open. */
export function topDialog(root: ParentNode = document): HTMLElement | null {
  const dialogs = root.querySelectorAll<HTMLElement>(
    '[role="dialog"][aria-modal="true"]',
  );
  return dialogs.length ? dialogs[dialogs.length - 1] : null;
}

/** The control that closes a dialog: its Close button, or else Cancel. */
export function closeControl(dialog: HTMLElement): HTMLElement | null {
  return (
    dialog.querySelector<HTMLElement>(
      'button[aria-label="Close"]:not([disabled])',
    ) ??
    [...dialog.querySelectorAll<HTMLElement>("button:not([disabled])")].find(
      (button) => button.textContent?.trim() === "Cancel",
    ) ??
    null
  );
}

/**
 * Escape closes the dialog on top and Tab stays inside it, the way macOS
 * sheets behave. Returns a function that removes the handlers.
 */
export function installDialogKeys(): () => void {
  const onKey = (event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    const dialog = topDialog();
    if (!dialog) return;
    if (event.key === "Escape") {
      const close = closeControl(dialog);
      if (close) {
        event.preventDefault();
        close.click();
      }
      return;
    }
    if (event.key !== "Tab") return;
    const items = [...dialog.querySelectorAll<HTMLElement>(focusable)].filter(
      (item) => item.offsetParent !== null,
    );
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (!dialog.contains(active)) {
      event.preventDefault();
      first.focus();
    } else if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}
