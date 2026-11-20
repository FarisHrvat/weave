import { createSignal, onCleanup, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorMessage } from "./api";

type Start = {
  id: string;
  userCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
};

type Props = {
  provider: "GitHub" | "GitLab";
  onConnected: () => void;
};

const storageKey = (provider: string) => `weave-oauth-client:${provider}`;

export default function OAuthSignIn(props: Props) {
  const saved = () => {
    try {
      return localStorage.getItem(storageKey(props.provider)) ?? "";
    } catch {
      return "";
    }
  };
  const [clientId, setClientId] = createSignal(saved());
  const [start, setStart] = createSignal<Start>();
  const [error, setError] = createSignal("");
  let timer: number | undefined;

  onCleanup(() => window.clearTimeout(timer));

  function poll(current: Start, delay: number) {
    timer = window.setTimeout(async () => {
      try {
        if (await invoke<boolean>("forge_oauth_poll", { id: current.id })) {
          setStart(undefined);
          props.onConnected();
        } else poll(current, delay);
      } catch (reason) {
        setStart(undefined);
        setError(errorMessage(reason));
      }
    }, delay * 1000);
  }

  async function begin() {
    setError("");
    try {
      localStorage.setItem(storageKey(props.provider), clientId().trim());
    } catch {
      // Remembering the client ID is only a convenience.
    }
    try {
      const result = await invoke<Start>("forge_oauth_start", {
        provider: props.provider,
        clientId: clientId().trim(),
      });
      setStart(result);
      await openUrl(result.verificationUri);
      poll(result, result.interval);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  return (
    <div class="wide oauth-sign-in">
      <Show
        when={start()}
        fallback={
          <>
            <label>
              Sign in with the browser instead{" "}
              <span class="optional">
                needs your OAuth app's client ID with device flow enabled
              </span>
              <span class="inline-field">
                <input
                  value={clientId()}
                  placeholder={
                    props.provider === "GitHub"
                      ? "Iv1.0123456789abcdef"
                      : "GitLab application ID"
                  }
                  onInput={(event) => setClientId(event.currentTarget.value)}
                />
                <button
                  type="button"
                  class="server-action"
                  disabled={!clientId().trim()}
                  onClick={() => void begin()}
                >
                  Sign in
                </button>
              </span>
            </label>
          </>
        }
      >
        {(current) => (
          <p class="oauth-code">
            Enter <strong>{current().userCode}</strong> on the page that opened
            in your browser.{" "}
            <button
              type="button"
              class="text-button"
              onClick={() => void openUrl(current().verificationUri)}
            >
              Open it again
            </button>
            <span class="muted"> Waiting for approval…</span>
          </p>
        )}
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
