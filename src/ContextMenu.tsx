import {
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";

export type MenuItem =
  | {
      label: string;
      icon?: JSX.Element;
      danger?: boolean;
      disabled?: boolean;
      run: () => void;
    }
  | "separator";

const [menu, setMenu] = createSignal<{
  x: number;
  y: number;
  items: MenuItem[];
}>();

/** Opens a right-click menu at the pointer. */
export function openContextMenu(event: MouseEvent, items: MenuItem[]) {
  event.preventDefault();
  event.stopPropagation();
  setMenu({ x: event.clientX, y: event.clientY, items });
}

/** Place once in the app; shows whichever menu is open. */
export default function ContextMenu() {
  let element: HTMLDivElement | undefined;
  const close = () => setMenu(undefined);
  onMount(() => {
    const outside = (event: MouseEvent) => {
      if (element && !element.contains(event.target as Node)) close();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape" && menu()) {
        event.preventDefault();
        close();
      }
    };
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("keydown", key, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", close, true);
    onCleanup(() => {
      document.removeEventListener("mousedown", outside, true);
      document.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", close, true);
    });
  });
  return (
    <Show when={menu()}>
      {(open) => (
        <div
          ref={(node) => {
            element = node;
            queueMicrotask(() => {
              // Keep the menu inside the window.
              const box = node.getBoundingClientRect();
              node.style.left = `${Math.max(8, Math.min(open().x, innerWidth - box.width - 8))}px`;
              node.style.top = `${Math.max(8, Math.min(open().y, innerHeight - box.height - 8))}px`;
              node
                .querySelector<HTMLButtonElement>("button:not(:disabled)")
                ?.focus();
            });
          }}
          class="context-menu"
          role="menu"
          style={{ left: `${open().x}px`, top: `${open().y}px` }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            event.preventDefault();
            const buttons = [
              ...event.currentTarget.querySelectorAll<HTMLButtonElement>(
                "button:not(:disabled)",
              ),
            ];
            const index = buttons.indexOf(
              document.activeElement as HTMLButtonElement,
            );
            const next = event.key === "ArrowDown" ? index + 1 : index - 1;
            buttons[(next + buttons.length) % buttons.length]?.focus();
          }}
        >
          <For each={open().items}>
            {(item) =>
              item === "separator" ? (
                <div class="context-menu-separator" role="separator" />
              ) : (
                <button
                  type="button"
                  role="menuitem"
                  class={item.danger ? "danger-text" : ""}
                  disabled={item.disabled}
                  onClick={() => {
                    close();
                    item.run();
                  }}
                >
                  {item.icon}
                  <span>{item.label}</span>
                </button>
              )
            }
          </For>
        </div>
      )}
    </Show>
  );
}
