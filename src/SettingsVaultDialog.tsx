import { createSignal, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { X } from "lucide-solid";
import { errorMessage } from "./api";
import { applyVaultSettings, collectVaultSettings } from "./settingsVault";
import TeamVaultPanel from "./TeamVaultPanel";
import type { Profile } from "./types";

type ImportReport = {
  profiles: number;
  workspaces: number;
  skipped: string[];
  local: Record<string, string>;
};

type Props = {
  profiles: Profile[];
  onImported: () => void;
  onNotify: (message: string, error?: boolean) => void;
  onClose: () => void;
};

export default function SettingsVaultDialog(props: Props) {
  const [passphrase, setPassphrase] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [skipped, setSkipped] = createSignal<string[]>([]);

  async function exportVault() {
    setError("");
    if (passphrase().length < 10) {
      setError("Use a passphrase of at least 10 characters.");
      return;
    }
    if (passphrase() !== confirm()) {
      setError("The passphrases do not match.");
      return;
    }
    const path = await save({
      defaultPath: "Weave settings.weavevault",
      filters: [{ name: "Weave settings vault", extensions: ["weavevault"] }],
    });
    if (!path) return;
    setBusy(true);
    try {
      const message = await invoke<string>("settings_vault_export", {
        path,
        passphrase: passphrase(),
        local: collectVaultSettings(localStorage),
      });
      setPassphrase("");
      setConfirm("");
      props.onNotify(message);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  async function importVault() {
    setError("");
    setSkipped([]);
    if (!passphrase()) {
      setError("Enter the vault passphrase.");
      return;
    }
    const path = await open({
      multiple: false,
      filters: [{ name: "Weave settings vault", extensions: ["weavevault"] }],
    });
    if (!path) return;
    setBusy(true);
    try {
      const report = await invoke<ImportReport>("settings_vault_import", {
        path,
        passphrase: passphrase(),
      });
      const settings = applyVaultSettings(localStorage, report.local);
      setPassphrase("");
      setConfirm("");
      setSkipped(report.skipped);
      props.onImported();
      props.onNotify(
        `Imported ${report.profiles} hosts, ${report.workspaces} workspaces and ${settings} settings groups. Reopen terminal tabs to use imported terminal settings. Enter passwords where needed.`,
      );
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class="dialog profile-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Settings vault"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">MOVE TO ANOTHER MAC</span>
            <h2>Settings vault</h2>
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
          Save hosts, workspaces, snippets, scripts, triggers, saved
          connections, bookmarks and terminal appearance to one encrypted file
          (AES-256-GCM, Argon2id key). Keep it in iCloud Drive or another synced
          folder and import it on your other computer. Passwords, tokens and
          command history are never included, and the passphrase cannot be
          recovered.
        </p>
        <div class="form-grid">
          <label class="wide">
            Passphrase
            <input
              type="password"
              autocomplete="new-password"
              value={passphrase()}
              onInput={(event) => setPassphrase(event.currentTarget.value)}
            />
          </label>
          <label class="wide">
            Confirm passphrase (export only)
            <input
              type="password"
              autocomplete="new-password"
              value={confirm()}
              onInput={(event) => setConfirm(event.currentTarget.value)}
            />
          </label>
        </div>
        <Show when={error()}>
          <p class="form-error" role="alert">
            {error()}
          </p>
        </Show>
        <Show when={skipped().length}>
          <p class="form-error">Not imported: {skipped().join("; ")}</p>
        </Show>
        <div class="dialog-actions">
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => void importVault()}
          >
            Import vault…
          </button>
          <span class="spacer" />
          <button
            class="primary-button"
            disabled={busy()}
            onClick={() => void exportVault()}
          >
            Export vault…
          </button>
        </div>
        <TeamVaultPanel
          profiles={props.profiles}
          onChanged={props.onImported}
          onNotify={props.onNotify}
        />
      </div>
    </div>
  );
}
