import { createSignal, For, onMount, Show } from "solid-js";
import { api, errorMessage } from "./api";
import type { SshPublicKey } from "./types";

type Props = {
  profileId: string;
  onChoose: (privatePath: string) => void;
  onInstall: (command: string) => void;
  onNotice: (message: string) => void;
};

export default function SSHKeysPanel(props: Props) {
  const [keys, setKeys] = createSignal<SshPublicKey[]>([]);
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function refresh() {
    setBusy(true);
    setError("");
    try {
      setKeys(await api.sshKeysList());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  onMount(() => void refresh());

  async function generate() {
    setBusy(true);
    setError("");
    try {
      const key = await api.sshKeysGenerate(name());
      setKeys(await api.sshKeysList());
      setName("");
      props.onChoose(key.publicPath.slice(0, -4));
      props.onNotice(`Created ${key.name}. Save this host to use the new key.`);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function install(key: SshPublicKey) {
    setBusy(true);
    setError("");
    try {
      props.onInstall(
        await api.sshKeysInstallCommand(props.profileId, key.publicPath),
      );
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <details class="tailnet-panel wide">
      <summary>SSH keys</summary>
      <p class="muted">
        Use a key from ~/.ssh or create a new Ed25519 key. New keys have no
        passphrase; protect your user account and add a passphrase with
        ssh-keygen if needed. Installing a key opens a local terminal so you can
        answer the host&apos;s password and trust prompts yourself.
      </p>
      <Show when={error()}>
        <p class="error-banner" role="alert">
          {error()}
        </p>
      </Show>
      <div class="known-hosts-list">
        <For
          each={keys()}
          fallback={<p class="muted">No public SSH keys found.</p>}
        >
          {(key) => (
            <div class="tailnet-peer">
              <div>
                <strong>{key.name}</strong>
                <small>
                  {key.keyType} · {key.fingerprint}
                </small>
                <Show when={!key.hasPrivate}>
                  <small>Matching private key not found</small>
                </Show>
              </div>
              <div class="external-online-toolbar">
                <button
                  type="button"
                  disabled={busy() || !key.hasPrivate}
                  onClick={() => props.onChoose(key.publicPath.slice(0, -4))}
                >
                  Use for host
                </button>
                <button
                  type="button"
                  disabled={busy() || !key.hasPrivate || !props.profileId}
                  title={
                    props.profileId
                      ? "Install on the saved SSH host"
                      : "Save this host first"
                  }
                  onClick={() => void install(key)}
                >
                  Install on host
                </button>
              </div>
            </div>
          )}
        </For>
      </div>
      <div class="external-online-toolbar">
        <input
          aria-label="New SSH key name"
          placeholder="Key name, such as nas"
          value={name()}
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <button type="button" disabled={busy()} onClick={() => void generate()}>
          Generate Ed25519 key
        </button>
        <button type="button" disabled={busy()} onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
    </details>
  );
}
