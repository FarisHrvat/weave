import { onCleanup, onMount, type JSX } from "solid-js";
import { ChevronDown } from "lucide-solid";

type Props = { label: string; title?: string; children: JSX.Element };

/** A button that opens a short list of actions; choosing one closes it. */
export default function MenuButton(props: Props) {
  let menu!: HTMLDetailsElement;
  const closeOutside = (event: MouseEvent) => {
    if (menu.open && !menu.contains(event.target as Node)) menu.open = false;
  };
  const closeOnEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !menu.open) return;
    event.preventDefault();
    event.stopPropagation();
    menu.open = false;
    menu.querySelector("summary")?.focus();
  };
  onMount(() => {
    document.addEventListener("mousedown", closeOutside);
    document.addEventListener("keydown", closeOnEscape, true);
  });
  onCleanup(() => {
    document.removeEventListener("mousedown", closeOutside);
    document.removeEventListener("keydown", closeOnEscape, true);
  });
  return (
    <details ref={menu} class="menu-button">
      <summary class="ghost-button" title={props.title}>
        {props.label} <ChevronDown size={14} />
      </summary>
      <div
        class="menu-list"
        role="menu"
        onClick={(event) => {
          if ((event.target as HTMLElement).closest("button"))
            menu.open = false;
        }}
      >
        {props.children}
      </div>
    </details>
  );
}
