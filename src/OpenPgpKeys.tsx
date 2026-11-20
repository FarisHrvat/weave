import { createSignal, For, onMount, Show } from "solid-js";
import { os } from "./platform";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage } from "./api";
import type { GitSigningInfo, OpenPgpKey } from "./types";

type Props = {
  repository: string;
  onSigningChanged: (info: GitSigningInfo) => void;
  onNotice: (message: string) => void;
};

export default function OpenPgpKeys(props: Props) {
  const [keys, setKeys] = createSignal<OpenPgpKey[]>([]);
  const [name, setName] = createSignal("");
  const [email, setEmail] = createSignal("");
  const [passphrase, setPassphrase] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function attempt(job: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await job();
      setKeys(await api.openPgpKeys());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  onMount(() => void attempt(async () => {}));

  const create = (event: Event) => {
    event.preventDefault();
    void attempt(async () => {
      const info = await api.openPgpGenerateKey(
        props.repository,
        name(),
        email(),
        passphrase(),
      );
      setPassphrase("");
      props.onSigningChanged(info);
      props.onNotice(
        "Created an OpenPGP key and enabled signing. Add its public key to GitHub or GitLab so they show commits as verified.",
      );
    });
  };

  const importKey = async () => {
    const file = await open({
      multiple: false,
      filters: [
        {
          name: "OpenPGP public key",
          extensions: ["asc", "gpg", "pgp", "key"],
        },
      ],
    });
    if (!file) return;
    void attempt(async () => {
      const who = await api.openPgpImport(file);
      props.onNotice(`Imported and trusted ${who}.`);
    });
  };

  return (
    <div class="openpgp-keys">
      <h4>OpenPGP keys</h4>
      <For
        each={keys()}
        fallback={<p class="muted">No OpenPGP keys in your keyring yet.</p>}
      >
        {(key) => (
          <div class="git-reflog-item">
            <div>
              <strong>{key.userId || key.fingerprint}</strong>
              <span class="muted">
                {key.fingerprint.slice(-16)} · {key.validity}
              </span>
            </div>
            <Show when={!key.secret && key.validity !== "Trusted"}>
              <button
                class="mini-button"
                disabled={busy()}
                title="Certify this key with yours so Git trusts its signatures"
                onClick={() =>
                  void attempt(() => api.openPgpTrust(key.fingerprint))
                }
              >
                Trust
              </button>
            </Show>
          </div>
        )}
      </For>
      <button
        class="mini-button"
        type="button"
        disabled={busy()}
        onClick={() => void importKey()}
      >
        Import and trust a public key…
      </button>
      <form class="git-tools-form" onSubmit={create}>
        <h4>Create a signing key</h4>
        <input
          aria-label="Name for the new key"
          placeholder="Your name"
          required
          value={name()}
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <input
          aria-label="Email for the new key"
          type="email"
          placeholder="Email used for commits"
          required
          value={email()}
          onInput={(event) => setEmail(event.currentTarget.value)}
        />
        <input
          aria-label="Passphrase for the new key"
          type="password"
          placeholder="Passphrase (optional)"
          value={passphrase()}
          onInput={(event) => setPassphrase(event.currentTarget.value)}
        />
        <p class="muted">
          {os === "mac"
            ? "With a passphrase, macOS asks for it when you commit; that needs pinentry-mac (brew install pinentry-mac)."
            : "With a passphrase, GnuPG asks for it when you commit, using its own pinentry window."}
        </p>
        <button class="mini-button" type="submit" disabled={busy()}>
          {busy() ? "Working…" : "Create key and sign with it"}
        </button>
      </form>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
