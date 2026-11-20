import { createSignal, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./api";
import type { AppLockSettings as Settings } from "./appLock";
import { shortcutLabel } from "./shortcuts";
import { isMac } from "./platform";

/**
 * Turns the app lock on or off. On a Mac it uses Touch ID or the login
 * password; elsewhere Weave's own lock password. Every change asks for the
 * same proof as unlocking.
 */
export default function AppLockSettings(props: {
  onChange: (settings: Settings) => void;
}) {
  const [settings, setSettings] = createSignal<Settings>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [current, setCurrent] = createSignal("");
  const [fresh, setFresh] = createSignal("");
  const [repeat, setRepeat] = createSignal("");

  onMount(() => {
    void invoke<Settings>("app_lock_get")
      .then(setSettings)
      .catch((reason) => setError(errorMessage(reason)));
  });

  async function save(next: Settings, newPassword?: string) {
    if (newPassword !== undefined && newPassword !== repeat()) {
      setError("The two passwords are not the same.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const saved = await invoke<Settings>("app_lock_set", {
        lock: next,
        password: current() || null,
        newPassword: newPassword ?? null,
      });
      setSettings(saved);
      props.onChange(saved);
      setCurrent("");
      setFresh("");
      setRepeat("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  const lockKey = () =>
    shortcutLabel(isMac ? "Super+Control+KeyL" : "Control+Alt+KeyL");

  const idleChoice = (lock: Settings) => (
    <label class="lock-idle">
      Also lock after
      <select
        value={String(lock.idleMinutes)}
        disabled={busy() || !lock.enabled}
        onChange={(event) =>
          void save({ ...lock, idleMinutes: Number(event.currentTarget.value) })
        }
      >
        <option value="0">never</option>
        <option value="1">1 minute idle</option>
        <option value="5">5 minutes idle</option>
        <option value="15">15 minutes idle</option>
        <option value="60">1 hour idle</option>
      </select>
    </label>
  );

  return (
    <section class="startup-section">
      <Show when={settings()}>
        {(lock) => (
          <Show
            when={lock().usesPassword}
            fallback={
              <>
                <label class="profile-favorite">
                  <input
                    type="checkbox"
                    checked={lock().enabled}
                    disabled={busy()}
                    onChange={(event) =>
                      void save({
                        ...lock(),
                        enabled: event.currentTarget.checked,
                      })
                    }
                  />
                  <span>
                    Lock Weave with Touch ID or my Mac password
                    <small class="muted">
                      Asked when Weave opens. Changing this asks macOS to
                      confirm it is you. Lock right away with {lockKey()} or
                      Lock Weave in the command palette.
                    </small>
                  </span>
                </label>
                {idleChoice(lock())}
              </>
            }
          >
            <div class="lock-password">
              <p class="muted">
                Weave asks for its lock password when it opens. Lock right away
                with {lockKey()} or Lock Weave in the command palette. The
                password is kept only as a protected hash in the system's
                credential store; if you forget it, remove that entry to reset
                the lock.
              </p>
              <Show when={lock().enabled}>
                <label>
                  Current lock password
                  <input
                    type="password"
                    autocomplete="current-password"
                    value={current()}
                    onInput={(event) => setCurrent(event.currentTarget.value)}
                  />
                </label>
                {idleChoice(lock())}
              </Show>
              <label>
                {lock().enabled ? "New lock password" : "Lock password"}
                <input
                  type="password"
                  autocomplete="new-password"
                  value={fresh()}
                  onInput={(event) => setFresh(event.currentTarget.value)}
                />
              </label>
              <label>
                Repeat it
                <input
                  type="password"
                  autocomplete="new-password"
                  value={repeat()}
                  onInput={(event) => setRepeat(event.currentTarget.value)}
                />
              </label>
              <div class="lock-actions">
                <Show
                  when={lock().enabled}
                  fallback={
                    <button
                      type="button"
                      class="primary-button"
                      disabled={busy() || !fresh()}
                      onClick={() =>
                        void save({ ...lock(), enabled: true }, fresh())
                      }
                    >
                      Turn on lock
                    </button>
                  }
                >
                  <button
                    type="button"
                    disabled={busy() || !fresh() || !current()}
                    onClick={() => void save(lock(), fresh())}
                  >
                    Change password
                  </button>
                  <button
                    type="button"
                    disabled={busy() || !current()}
                    onClick={() => void save({ ...lock(), enabled: false })}
                  >
                    Turn off lock
                  </button>
                </Show>
              </div>
            </div>
          </Show>
        )}
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </section>
  );
}
