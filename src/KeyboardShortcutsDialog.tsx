import { createSignal, For, Show } from "solid-js";
import { X } from "lucide-solid";
import {
  conflictFor,
  defaultKeybindings,
  keyActions,
  type KeyAction,
  type Keybindings,
} from "./keybindings";
import { fixedShortcuts, shortcutFromEvent, shortcutLabel } from "./shortcuts";
import { isMac } from "./platform";

type Props = {
  bindings: Keybindings;
  onChange: (bindings: Keybindings) => void;
  onClose: () => void;
};

const key = (id: keyof typeof fixedShortcuts) =>
  shortcutLabel(fixedShortcuts[id]);
const fixed = [
  [key("find"), "Find in terminal output"],
  [`${key("promptUp")} / ${key("promptDown")}`, "Jump between prompts"],
  [key("complete"), "Command completion"],
  [`${key("copy")} / ${key("paste")}`, "Copy and paste in a terminal"],
  [key("preview"), "Quick preview in Files"],
  [key("findFiles"), "Find files"],
];

export default function KeyboardShortcutsDialog(props: Props) {
  const [recording, setRecording] = createSignal<KeyAction>();
  const [message, setMessage] = createSignal("");
  const labelOf = (id: KeyAction) =>
    keyActions.find((action) => action.id === id)?.label ?? id;

  function record(id: KeyAction, event: KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setRecording(undefined);
      return;
    }
    const shortcut = shortcutFromEvent(event);
    if (!shortcut) return;
    if (!/Super|Control|Alt/.test(shortcut) && !/^F\d+$/.test(shortcut)) {
      setMessage(
        isMac
          ? "Use ⌘, ⌥ or ⌃ in the shortcut, so it does not take over normal typing."
          : "Use Ctrl or Alt in the shortcut, so it does not take over normal typing.",
      );
      return;
    }
    const other = conflictFor(props.bindings, id, shortcut);
    const next = { ...props.bindings, [id]: shortcut };
    if (other) next[other] = "";
    props.onChange(next);
    setRecording(undefined);
    setMessage(
      other
        ? `${shortcutLabel(shortcut)} was used by “${labelOf(other)}”, which now has no shortcut.`
        : "",
    );
  }

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class="dialog profile-dialog shortcuts-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Keyboard shortcuts"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">WEAVE</span>
            <h2>Keyboard shortcuts</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>
        <p class="muted">
          Click a shortcut, then press the new keys. Esc cancels. These work
          everywhere in Weave, including while a terminal has focus.
        </p>
        <div class="shortcut-list">
          <For each={keyActions}>
            {(action) => (
              <div class="shortcut-row">
                <span class="truncate">{action.label}</span>
                <button
                  class={
                    recording() === action.id
                      ? "primary-button shortcut-key"
                      : "server-action shortcut-key"
                  }
                  onClick={() =>
                    setRecording(
                      recording() === action.id ? undefined : action.id,
                    )
                  }
                  onKeyDown={(event) =>
                    recording() === action.id && record(action.id, event)
                  }
                  onBlur={() =>
                    recording() === action.id && setRecording(undefined)
                  }
                >
                  {recording() === action.id
                    ? "Press keys…"
                    : props.bindings[action.id]
                      ? shortcutLabel(props.bindings[action.id])
                      : "None"}
                </button>
                <button
                  class="ghost-button"
                  title="Remove this shortcut"
                  disabled={!props.bindings[action.id]}
                  onClick={() =>
                    props.onChange({ ...props.bindings, [action.id]: "" })
                  }
                >
                  Clear
                </button>
              </div>
            )}
          </For>
        </div>
        <Show when={message()}>
          <p class="muted" role="status">
            {message()}
          </p>
        </Show>
        <h3 class="server-repos-heading">Built-in</h3>
        <div class="shortcut-list">
          <For each={fixed}>
            {([keys, label]) => (
              <div class="shortcut-row shortcut-fixed">
                <span>{label}</span>
                <kbd>{keys}</kbd>
              </div>
            )}
          </For>
        </div>
        <div class="dialog-actions">
          <button
            class="ghost-button"
            onClick={() => {
              props.onChange(defaultKeybindings());
              setMessage("Shortcuts reset to the defaults.");
            }}
          >
            Reset to defaults
          </button>
          <span class="spacer" />
          <button class="primary-button" onClick={() => props.onClose()}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
