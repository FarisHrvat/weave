import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { keychain } from "./platform";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { errorMessage } from "./api";

type Prompt = {
  id: string;
  connection: string;
  prompt: string;
  secret: boolean;
  canRemember: boolean;
};

/** Questions ssh asks while Files, tunnels or copies connect: passwords, verification codes, new host keys. */
export default function SshPromptDialog(props: {
  onError: (message: string) => void;
}) {
  const [queue, setQueue] = createSignal<Prompt[]>([]);
  const [answer, setAnswer] = createSignal("");
  const [remember, setRemember] = createSignal(false);
  const current = () => queue()[0];

  onMount(() => {
    const stop = listen<Prompt>("ssh-prompt", (event) =>
      setQueue((items) => [...items, event.payload]),
    );
    onCleanup(() => void stop.then((unlisten) => unlisten()));
  });

  async function reply(value: string | null) {
    const prompt = current();
    if (!prompt) return;
    setQueue((items) => items.slice(1));
    const keep = remember();
    setAnswer("");
    setRemember(false);
    try {
      await invoke("ssh_prompt_answer", {
        id: prompt.id,
        answer: value,
        remember: keep && value !== null,
      });
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  return (
    <Show when={current()}>
      {(prompt) => (
        <div class="overlay ssh-prompt-overlay">
          <form
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Sign in"
            onSubmit={(event) => {
              event.preventDefault();
              void reply(answer());
            }}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">SIGN IN</span>
                <h2>{prompt().connection}</h2>
              </div>
            </div>
            <pre class="ssh-prompt-text">{prompt().prompt}</pre>
            <Show
              when={prompt().secret}
              fallback={
                <div class="ssh-prompt-choice">
                  <For each={["yes", "no"]}>
                    {(choice) => (
                      <button
                        type="button"
                        class={
                          choice === "yes" ? "primary-button" : "ghost-button"
                        }
                        onClick={() => void reply(choice)}
                      >
                        {choice === "yes" ? "Trust this host" : "Don't connect"}
                      </button>
                    )}
                  </For>
                </div>
              }
            >
              <input
                type="password"
                aria-label="Answer"
                autocomplete="off"
                ref={(element) => queueMicrotask(() => element.focus())}
                value={answer()}
                onInput={(event) => setAnswer(event.currentTarget.value)}
              />
              <Show when={prompt().canRemember}>
                <label class="check-label">
                  <input
                    type="checkbox"
                    checked={remember()}
                    onChange={(event) =>
                      setRemember(event.currentTarget.checked)
                    }
                  />
                  Remember this password in {keychain}
                </label>
              </Show>
              <div class="dialog-actions">
                <button
                  type="button"
                  class="ghost-button"
                  onClick={() => void reply(null)}
                >
                  Cancel
                </button>
                <button type="submit" class="primary-button">
                  Continue
                </button>
              </div>
            </Show>
          </form>
        </div>
      )}
    </Show>
  );
}
