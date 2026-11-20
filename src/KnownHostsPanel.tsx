import { confirmAction } from "./dialogs";
import { thisComputer } from "./platform";
import { createSignal, For, onMount, Show } from "solid-js";
import { api, errorMessage } from "./api";
import type { KnownHost } from "./types";

type Props = {
  host: string;
  onNotice: (message: string) => void;
};

export default function KnownHostsPanel(props: Props) {
  const [entries, setEntries] = createSignal<KnownHost[]>([]);
  const [host, setHost] = createSignal(props.host);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function refresh() {
    setBusy(true);
    setError("");
    try {
      setEntries(await api.knownHostsList());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  onMount(() => void refresh());

  async function forget() {
    const name = host().trim();
    if (!name) {
      setError("Enter the SSH host name to forget.");
      return;
    }
    if (
      !(await confirmAction(
        `Forget saved SSH keys for ${name}? On your next connection, compare the new fingerprint with one supplied by the server administrator before trusting it.`,
      ))
    )
      return;
    setBusy(true);
    setError("");
    try {
      await api.knownHostsRemove(name);
      setEntries(await api.knownHostsList());
      props.onNotice(`Forgot saved SSH keys for ${name}.`);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <details class="tailnet-panel wide">
      <summary>Known SSH hosts</summary>
      <p class="muted">
        Review fingerprints saved by OpenSSH on {thisComputer}. Hashed names are
        hidden; enter a host name to forget its key.
      </p>
      <Show when={error()}>
        <p class="error-banner" role="alert">
          {error()}
        </p>
      </Show>
      <div class="known-hosts-list">
        <For
          each={entries()}
          fallback={<p class="muted">No saved SSH host keys were found.</p>}
        >
          {(entry) => (
            <div class="tailnet-peer">
              <div>
                <strong>
                  {entry.hashed ? "Hashed hostname" : entry.hosts}
                </strong>
                <small>
                  {entry.keyType} · {entry.fingerprint}
                  {entry.marker ? ` · ${entry.marker}` : ""}
                </small>
              </div>
            </div>
          )}
        </For>
      </div>
      <div class="external-online-toolbar">
        <input
          aria-label="SSH host to forget"
          placeholder="nas.example.com or [nas.example.com]:2222"
          value={host()}
          onInput={(event) => setHost(event.currentTarget.value)}
        />
        <button type="button" disabled={busy()} onClick={() => void forget()}>
          Forget host key
        </button>
        <button type="button" disabled={busy()} onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
    </details>
  );
}
