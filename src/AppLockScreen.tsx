import { createSignal, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { Lock } from "lucide-solid";
import { errorMessage } from "./api";

/**
 * Covers the whole window until it is confirmed that it is you: Touch ID or
 * the login password on a Mac, Weave's lock password elsewhere.
 */
export default function AppLockScreen(props: {
  usesPassword: boolean;
  onUnlocked: () => void;
}) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [password, setPassword] = createSignal("");
  let button!: HTMLButtonElement;
  let field: HTMLInputElement | undefined;

  async function unlock() {
    if (busy()) return;
    setBusy(true);
    setError("");
    try {
      await invoke("app_unlock", {
        password: props.usesPassword ? password() : null,
      });
      setPassword("");
      props.onUnlocked();
    } catch (reason) {
      setError(errorMessage(reason));
      (field ?? button).focus();
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    if (props.usesPassword) field?.focus();
    else {
      button.focus();
      void unlock();
    }
  });

  return (
    <div
      class="app-lock"
      role="dialog"
      aria-modal="true"
      aria-label="Weave is locked"
    >
      <form
        class="app-lock-card"
        onSubmit={(event) => {
          event.preventDefault();
          void unlock();
        }}
      >
        <span class="app-lock-icon">
          <Lock size={26} />
        </span>
        <h1>Weave is locked</h1>
        <p class="muted">
          {props.usesPassword
            ? "Enter your Weave lock password."
            : "Use Touch ID or your Mac password."}{" "}
          Transfers, sync jobs and tunnels keep running.
        </p>
        <Show when={props.usesPassword}>
          <input
            ref={field}
            type="password"
            aria-label="Lock password"
            autocomplete="current-password"
            value={password()}
            onInput={(event) => setPassword(event.currentTarget.value)}
          />
        </Show>
        <button
          ref={button}
          type="submit"
          class="primary-button"
          disabled={busy() || (props.usesPassword && !password())}
        >
          {busy() && !props.usesPassword ? "Waiting for macOS…" : "Unlock"}
        </button>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </form>
    </div>
  );
}
