import { createSignal, Show } from "solid-js";
import { keychain } from "./platform";
import { api, errorMessage } from "./api";

type Props = {
  label: string;
  busyLabel: string;
  title?: string;
  disabled?: boolean;
  run: () => Promise<string>;
  onResult: (text: string) => void;
  onError: (message: string) => void;
};

/** A button that asks Claude for something, first asking for the API key if needed. */
export default function AiAsk(props: Props) {
  const [busy, setBusy] = createSignal(false);
  const [askKey, setAskKey] = createSignal(false);
  const [key, setKey] = createSignal("");

  async function start() {
    if (!(await api.aiKeyStatus())) {
      setAskKey(true);
      return;
    }
    setBusy(true);
    try {
      props.onResult(await props.run());
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function saveKey(event: Event) {
    event.preventDefault();
    try {
      await api.aiKeySet(key());
      setKey("");
      setAskKey(false);
      await start();
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  return (
    <>
      <button
        type="button"
        class="mini-button"
        title={props.title}
        disabled={busy() || props.disabled}
        onClick={() => void start()}
      >
        {busy() ? props.busyLabel : props.label}
      </button>
      <Show when={askKey()}>
        <form class="ai-key-form" onSubmit={saveKey}>
          <input
            type="password"
            aria-label="Anthropic API key"
            placeholder={`Anthropic API key (sk-ant-…), saved in ${keychain}`}
            value={key()}
            onInput={(event) => setKey(event.currentTarget.value)}
          />
          <button class="mini-button" type="submit">
            Save
          </button>
          <button
            class="mini-button"
            type="button"
            onClick={() => setAskKey(false)}
          >
            Cancel
          </button>
        </form>
      </Show>
    </>
  );
}
