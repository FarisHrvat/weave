import { confirmAction, askText } from "./dialogs";
import { thisComputer } from "./platform";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { File, Folder, Lock, X } from "lucide-solid";
import { errorMessage } from "./api";

type Entry = { name: string; isDir: boolean; size: number };

type Props = {
  onNotice: (message: string) => void;
  onClose: () => void;
};

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export default function CryptomatorDialog(props: Props) {
  const [folder, setFolder] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [vaultId, setVaultId] = createSignal("");
  const [path, setPath] = createSignal("/");
  const [entries, setEntries] = createSignal<Entry[]>([]);
  const [selected, setSelected] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const childPath = (name: string) => `${path() === "/" ? "" : path()}/${name}`;

  async function attempt(job: () => Promise<void>) {
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

  async function listFolder(next: string) {
    setEntries(
      await invoke<Entry[]>("cryptomator_list", { id: vaultId(), path: next }),
    );
    setPath(next);
    setSelected("");
  }

  const load = (next = path()) => attempt(() => listFolder(next));

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") setFolder(chosen);
  }

  const unlock = (event: Event) => {
    event.preventDefault();
    void attempt(async () => {
      const id = await invoke<string>(
        creating() ? "cryptomator_create" : "cryptomator_unlock",
        { folder: folder(), password: password() },
      );
      setPassword("");
      setVaultId(id);
      await listFolder("/");
      if (creating())
        props.onNotice(
          "Vault created. Keep the password safe; it cannot be recovered.",
        );
    });
  };

  function lock() {
    if (vaultId()) void invoke("cryptomator_lock", { id: vaultId() });
    setVaultId("");
    setEntries([]);
    setPath("/");
  }

  onCleanup(lock);

  const addItems = async (directory: boolean) => {
    const chosen = await open({ directory, multiple: true });
    const sources = Array.isArray(chosen) ? chosen : chosen ? [chosen] : [];
    if (!sources.length) return;
    void attempt(async () => {
      await invoke("cryptomator_add", {
        id: vaultId(),
        sources,
        destination: path(),
      });
      await listFolder(path());
      props.onNotice(`Encrypted ${sources.length} item(s) into the vault.`);
    });
  };

  const extract = async () => {
    const destination = await open({ directory: true, multiple: false });
    if (typeof destination !== "string") return;
    void attempt(async () => {
      await invoke("cryptomator_extract", {
        id: vaultId(),
        paths: [childPath(selected())],
        destination,
      });
      props.onNotice(`Decrypted “${selected()}” to ${destination}.`);
    });
  };

  const newFolder = async () => {
    const name = await askText("Name of the new folder:");
    if (!name?.trim()) return;
    void attempt(async () => {
      await invoke("cryptomator_mkdir", {
        id: vaultId(),
        path: childPath(name.trim()),
      });
      await listFolder(path());
    });
  };

  const rename = async () => {
    const name = await askText("New name:", selected());
    if (!name?.trim() || name === selected()) return;
    void attempt(async () => {
      await invoke("cryptomator_rename", {
        id: vaultId(),
        path: childPath(selected()),
        name: name.trim(),
      });
      await listFolder(path());
    });
  };

  const remove = async () => {
    if (
      !(await confirmAction(
        `Permanently delete “${selected()}” from the vault? This cannot be undone.`,
      ))
    )
      return;
    void attempt(async () => {
      await invoke("cryptomator_delete", {
        id: vaultId(),
        path: childPath(selected()),
      });
      await listFolder(path());
    });
  };

  const breadcrumbs = () => {
    const parts = path().split("/").filter(Boolean);
    return [
      { name: "Vault", path: "/" },
      ...parts.map((name, index) => ({
        name,
        path: "/" + parts.slice(0, index + 1).join("/"),
      })),
    ];
  };

  return (
    <div class="overlay">
      <div
        class="dialog profile-dialog connect-dialog cryptomator-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Cryptomator vault"
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">ENCRYPTED VAULT</span>
            <h2>Cryptomator</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>
        <Show
          when={vaultId()}
          fallback={
            <form onSubmit={unlock}>
              <p class="muted">
                Open a Cryptomator vault (format 7 or 8) stored on{" "}
                {thisComputer}, including in iCloud Drive, Dropbox or OneDrive
                folders, or create a new one. Files are encrypted before they
                are written.
              </p>
              <div class="segmented connect-kinds">
                <button
                  type="button"
                  class={creating() ? "" : "active"}
                  onClick={() => setCreating(false)}
                >
                  Open vault
                </button>
                <button
                  type="button"
                  class={creating() ? "active" : ""}
                  onClick={() => setCreating(true)}
                >
                  Create vault
                </button>
              </div>
              <div class="form-grid">
                <label class="wide">
                  {creating() ? "Empty folder for the vault" : "Vault folder"}
                  <span class="inline-field">
                    <input
                      required
                      value={folder()}
                      placeholder="Folder containing vault.cryptomator or masterkey.cryptomator"
                      onInput={(event) => setFolder(event.currentTarget.value)}
                    />
                    <button
                      type="button"
                      class="server-action"
                      onClick={() => void chooseFolder()}
                    >
                      Choose…
                    </button>
                  </span>
                </label>
                <label class="wide">
                  Vault password
                  <input
                    type="password"
                    required
                    autocomplete={
                      creating() ? "new-password" : "current-password"
                    }
                    value={password()}
                    onInput={(event) => setPassword(event.currentTarget.value)}
                  />
                </label>
              </div>
              <Show when={error()}>
                <p class="form-error" role="alert">
                  {error()}
                </p>
              </Show>
              <div class="dialog-actions">
                <span class="spacer" />
                <button type="submit" class="primary-button" disabled={busy()}>
                  {busy() ? "Working…" : creating() ? "Create vault" : "Unlock"}
                </button>
              </div>
            </form>
          }
        >
          <nav class="cryptomator-path" aria-label="Vault location">
            <For each={breadcrumbs()}>
              {(crumb, index) => (
                <>
                  <Show when={index() > 0}>
                    <span class="muted">/</span>
                  </Show>
                  <button
                    class="text-button"
                    onClick={() => void load(crumb.path)}
                  >
                    {crumb.name}
                  </button>
                </>
              )}
            </For>
          </nav>
          <div class="cryptomator-list" role="listbox" aria-label="Vault items">
            <For
              each={entries()}
              fallback={<p class="muted">This folder is empty.</p>}
            >
              {(entry) => (
                <button
                  role="option"
                  aria-selected={selected() === entry.name}
                  class={selected() === entry.name ? "active" : ""}
                  onClick={() => setSelected(entry.name)}
                  onDblClick={() =>
                    entry.isDir && void load(childPath(entry.name))
                  }
                >
                  {entry.isDir ? <Folder size={15} /> : <File size={15} />}
                  <span class="truncate">{entry.name}</span>
                  <span class="muted">
                    {entry.isDir ? "Folder" : formatSize(entry.size)}
                  </span>
                </button>
              )}
            </For>
          </div>
          <div class="cryptomator-actions">
            <button
              class="server-action"
              disabled={busy()}
              onClick={() => void addItems(false)}
            >
              Add files…
            </button>
            <button
              class="server-action"
              disabled={busy()}
              onClick={() => void addItems(true)}
            >
              Add folder…
            </button>
            <button class="server-action" disabled={busy()} onClick={newFolder}>
              New folder
            </button>
            <button
              class="server-action"
              disabled={busy() || !selected()}
              onClick={() => void extract()}
            >
              Decrypt to…
            </button>
            <button
              class="server-action"
              disabled={busy() || !selected()}
              onClick={rename}
            >
              Rename
            </button>
            <button
              class="server-action danger-text"
              disabled={busy() || !selected()}
              onClick={remove}
            >
              Delete
            </button>
          </div>
          <Show when={error()}>
            <p class="form-error" role="alert">
              {error()}
            </p>
          </Show>
          <div class="dialog-actions">
            <Show when={busy()}>
              <span class="muted">Working…</span>
            </Show>
            <span class="spacer" />
            <button class="ghost-button" onClick={lock}>
              <Lock size={15} /> Lock vault
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}
