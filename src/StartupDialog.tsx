import { createSignal, onMount, Show } from "solid-js";
import { systemName } from "./platform";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-solid";
import { errorMessage } from "./api";
import { shortcutFromEvent, shortcutLabel } from "./shortcuts";
import AppLockSettingsPanel from "./AppLockSettings";
import type { AppLockSettings } from "./appLock";

type StartupSettings = { startAtLogin: boolean; syncWhileQuit: boolean };

type Props = {
  menuBarMode: boolean;
  onMenuBarMode: (enabled: boolean) => void;
  onLockChange: (settings: AppLockSettings) => void;
  onClose: () => void;
};

export default function StartupDialog(props: Props) {
  const [settings, setSettings] = createSignal<StartupSettings>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [hotkey, setHotkey] = createSignal<string | null>(null);
  const [recording, setRecording] = createSignal(false);

  onMount(() => {
    void invoke<string | null>("hotkey_get")
      .then(setHotkey)
      .catch(() => undefined);
    void invoke<StartupSettings>("startup_settings_get")
      .then(setSettings)
      .catch((reason) => setError(errorMessage(reason)));
  });

  async function change(job: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await job();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  const save = (patch: Partial<StartupSettings>) =>
    change(async () => {
      setSettings(await invoke<StartupSettings>("startup_settings_set", patch));
    });

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class="dialog profile-dialog startup-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Background and startup"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">WEAVE</span>
            <h2>Background and startup</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>
        <label class="profile-favorite">
          <input
            type="checkbox"
            checked={props.menuBarMode}
            disabled={busy()}
            onChange={(event) => {
              const enabled = event.currentTarget.checked;
              void change(async () => {
                props.onMenuBarMode(
                  await invoke<boolean>("background_mode_set", { enabled }),
                );
              });
            }}
          />
          <span>
            Keep running in the menu bar when the window closes
            <small class="muted">
              Sync jobs, queued transfers and SSH tunnels keep going. Quit from
              the menu bar icon.
            </small>
          </span>
        </label>
        <label class="profile-favorite">
          <input
            type="checkbox"
            checked={settings()?.startAtLogin ?? false}
            disabled={busy() || !settings()}
            onChange={(event) =>
              void save({ startAtLogin: event.currentTarget.checked })
            }
          />
          <span>
            Start at login in the menu bar
            <small class="muted">
              Weave opens without a window; click the menu bar icon to show it.
            </small>
          </span>
        </label>
        <label class="profile-favorite">
          <input
            type="checkbox"
            checked={settings()?.syncWhileQuit ?? false}
            disabled={busy() || !settings()}
            onChange={(event) =>
              void save({ syncWhileQuit: event.currentTarget.checked })
            }
          />
          <span>
            Run sync jobs while Weave is quit
            <small class="muted">
              Every 15 minutes {systemName} starts Weave without a window, runs
              the jobs that are due and any queued transfers, then quits again.
              Connections need a saved password, key or sign-in.
            </small>
          </span>
        </label>
        <div class="startup-hotkey">
          <span>
            Show or hide Weave from any app
            <small class="muted">
              A system-wide shortcut, like iTerm2's hotkey window.
            </small>
          </span>
          <button
            type="button"
            class={recording() ? "primary-button" : "server-action"}
            disabled={busy()}
            onClick={() => setRecording(!recording())}
            onKeyDown={(event) => {
              if (!recording()) return;
              event.preventDefault();
              if (event.key === "Escape") {
                setRecording(false);
                return;
              }
              const shortcut = shortcutFromEvent(event);
              if (!shortcut) return;
              setRecording(false);
              void change(async () => {
                setHotkey(
                  await invoke<string | null>("hotkey_set", { shortcut }),
                );
              });
            }}
          >
            {recording()
              ? "Press the shortcut…"
              : hotkey()
                ? shortcutLabel(hotkey()!)
                : "Set shortcut"}
          </button>
          <Show when={hotkey() && !recording()}>
            <button
              type="button"
              class="ghost-button"
              disabled={busy()}
              onClick={() =>
                void change(async () => {
                  setHotkey(
                    await invoke<string | null>("hotkey_set", {
                      shortcut: null,
                    }),
                  );
                })
              }
            >
              Remove
            </button>
          </Show>
        </div>
        <AppLockSettingsPanel onChange={props.onLockChange} />
        <Show when={error()}>
          <p class="form-error" role="alert">
            {error()}
          </p>
        </Show>
      </div>
    </div>
  );
}
