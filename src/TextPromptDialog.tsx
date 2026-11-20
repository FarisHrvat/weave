import { createSignal, createEffect, Show } from "solid-js";
import { finishText, textRequest } from "./dialogs";

/** The dialog behind askText(). */
export default function TextPromptDialog() {
  const [value, setValue] = createSignal("");
  createEffect(() => setValue(textRequest()?.value ?? ""));
  return (
    <Show when={textRequest()}>
      {(request) => (
        <div class="overlay text-prompt-overlay">
          <form
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label={request().message}
            onSubmit={(event) => {
              event.preventDefault();
              finishText(value());
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                finishText(null);
              }
            }}
          >
            <p class="text-prompt-message">{request().message}</p>
            <input
              aria-label={request().message}
              spellcheck={false}
              value={value()}
              ref={(element) =>
                queueMicrotask(() => {
                  element.focus();
                  element.select();
                })
              }
              onInput={(event) => setValue(event.currentTarget.value)}
            />
            <div class="dialog-actions">
              <button
                type="button"
                class="ghost-button"
                onClick={() => finishText(null)}
              >
                Cancel
              </button>
              <button type="submit" class="primary-button">
                OK
              </button>
            </div>
          </form>
        </div>
      )}
    </Show>
  );
}
