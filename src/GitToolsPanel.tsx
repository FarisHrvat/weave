import { confirmAction, askText } from "./dialogs";
import { thisComputer } from "./platform";
import { createEffect, createSignal, For, Show, untrack } from "solid-js";
import { api, errorMessage } from "./api";
import type {
  GitSignatureEntry,
  GitSigningInfo,
  GitToolsInfo,
  ReflogEntry,
} from "./types";
import GitFlowPanel from "./GitFlowPanel";
import OpenPgpKeys from "./OpenPgpKeys";

type Props = {
  repository: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
};

export default function GitToolsPanel(props: Props) {
  const [info, setInfo] = createSignal<GitToolsInfo>();
  const [signing, setSigning] = createSignal<GitSigningInfo>();
  const [signatureAudit, setSignatureAudit] = createSignal<GitSignatureEntry[]>(
    [],
  );
  const [auditBusy, setAuditBusy] = createSignal(false);
  const [auditComplete, setAuditComplete] = createSignal(false);
  const [reflog, setReflog] = createSignal<ReflogEntry[]>([]);
  const [signingEnabled, setSigningEnabled] = createSignal(false);
  const [signingFormat, setSigningFormat] = createSignal<"openpgp" | "ssh">(
    "openpgp",
  );
  const [signingKey, setSigningKey] = createSignal("");
  const [trustPrincipal, setTrustPrincipal] = createSignal("");
  const [trustPublicKeyPath, setTrustPublicKeyPath] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [directory, setDirectory] = createSignal("");
  const [sparseDirectories, setSparseDirectories] = createSignal("");
  const [sparseMode, setSparseMode] = createSignal<"folders" | "patterns">(
    "folders",
  );
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let auditGeneration = 0;

  async function run(change: () => Promise<GitToolsInfo>, notice?: string) {
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const result = await change();
      if (repository !== props.repository) return;
      setInfo(result);
      setSparseDirectories(result.sparseDirectories.join("\n"));
      setSparseMode(
        result.sparseEnabled && !result.sparseCone ? "patterns" : "folders",
      );
      if (notice) {
        props.onNotice(notice);
        props.onChanged();
      }
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function loadSigning() {
    const repository = props.repository;
    try {
      const result = await api.gitSigningInfo(repository);
      if (repository !== props.repository) return;
      setSigning(result);
      setSigningEnabled(result.enabled);
      setSigningFormat(result.format === "ssh" ? "ssh" : "openpgp");
      setSigningKey(result.key);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    }
  }

  async function generateSigningKey() {
    const principal = await askText(
      "Email address to identify the new SSH signing key (usually your Git email):",
    );
    if (!principal?.trim()) return;
    const repository = props.repository;
    setBusy(true);
    try {
      const result = await api.gitSigningGenerateKey(
        repository,
        principal.trim(),
      );
      if (repository !== props.repository) return;
      setSigning(result);
      setSigningEnabled(result.enabled);
      setSigningFormat("ssh");
      setSigningKey(result.key);
      props.onNotice(
        `Created ${result.key} and enabled SSH signing. Add this public key to GitHub or GitLab as a signing key so they show commits as verified.`,
      );
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function saveSigning(event: Event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const result = await api.gitSigningSave(
        props.repository,
        signingEnabled(),
        signingFormat(),
        signingKey(),
      );
      setSigning(result);
      props.onNotice("Commit signing settings saved for this repository.");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function loadSignatureAudit(reset = false) {
    if (auditBusy() && !reset) return;
    const generation = ++auditGeneration;
    const repository = props.repository;
    const skip = reset ? 0 : signatureAudit().length;
    setAuditBusy(true);
    setError("");
    try {
      const page = await api.gitSigningAudit(repository, skip);
      if (generation !== auditGeneration || repository !== props.repository)
        return;
      setSignatureAudit(reset ? page : [...signatureAudit(), ...page]);
      setAuditComplete(page.length < 50);
    } catch (reason) {
      if (generation === auditGeneration && repository === props.repository)
        setError(errorMessage(reason));
    } finally {
      if (generation === auditGeneration) setAuditBusy(false);
    }
  }

  async function addTrustedSigner(event: Event) {
    event.preventDefault();
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const result = await api.gitSigningTrustAdd(
        repository,
        trustPrincipal(),
        trustPublicKeyPath(),
      );
      if (repository !== props.repository) return;
      setSigning(result);
      setTrustPrincipal("");
      setTrustPublicKeyPath("");
      props.onNotice("SSH signer added to this repository's trusted keys.");
      void loadSignatureAudit(true);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function loadReflog() {
    const repository = props.repository;
    try {
      const result = await api.gitReflog(repository);
      if (repository === props.repository) setReflog(result);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    }
  }

  async function restoreHistory(entry: ReflogEntry) {
    if (
      !(await confirmAction(
        `Move the current branch to ${entry.commit.slice(0, 8)}? Weave will preserve its current tip as a recovery branch. Commit or stash local changes first.`,
      ))
    )
      return;
    setBusy(true);
    setError("");
    try {
      const recovery = await api.gitReflogRestore(
        props.repository,
        entry.commit,
      );
      props.onNotice(
        `Branch restored. The previous tip is saved as ${recovery}.`,
      );
      props.onChanged();
      await loadReflog();
      await loadSigning();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  createEffect(() => {
    const repository = props.repository;
    setInfo(undefined);
    setSigning(undefined);
    setSignatureAudit([]);
    setAuditComplete(false);
    setAuditBusy(false);
    auditGeneration++;
    setReflog([]);
    setBusy(false);
    void run(() => api.gitToolsInfo(repository));
    void loadSigning();
    untrack(() => void loadSignatureAudit(true));
    void loadReflog();
  });

  function addSubmodule(event: Event) {
    event.preventDefault();
    void run(
      () => api.gitSubmoduleAdd(props.repository, url(), directory()),
      "Submodule added. Review and commit .gitmodules and the new submodule entry.",
    ).then(() => {
      if (!error()) {
        setUrl("");
        setDirectory("");
      }
    });
  }

  function setSparse(event: Event) {
    event.preventDefault();
    const directories = sparseDirectories()
      .split("\n")
      .map((item) => item.trim())
      .filter(Boolean);
    void run(
      () =>
        sparseMode() === "folders"
          ? api.gitSparseSet(props.repository, directories)
          : api.gitSparseSetPatterns(props.repository, directories),
      `Sparse checkout ${sparseMode()} updated.`,
    );
  }

  async function removeSubmodule(directory: string) {
    if (
      !(await confirmAction(
        `Remove submodule ${directory}? Git will stage the removal and .gitmodules change. Commit or stash its local changes first.`,
      ))
    )
      return;
    void run(
      () => api.gitSubmoduleRemove(props.repository, directory),
      `Removed ${directory}. Review and commit the staged changes.`,
    );
  }

  return (
    <div class="git-tools-panel">
      <GitFlowPanel
        repository={props.repository}
        onNotice={props.onNotice}
        onChanged={props.onChanged}
      />
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={busy() && !info()}>
        <p class="empty-small">Loading repository tools…</p>
      </Show>
      <section>
        <div class="git-tools-heading">
          <h3>Submodules</h3>
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            onClick={() =>
              void run(
                () => api.gitSubmodulesUpdate(props.repository),
                "Submodules synchronized and updated.",
              )
            }
          >
            Sync and update
          </button>
        </div>
        <For
          each={info()?.submodules}
          fallback={
            <p class="empty-small">No submodules in this repository.</p>
          }
        >
          {(item) => (
            <div class="git-tools-item">
              <strong>{item.path}</strong>
              <span class="muted">{item.url}</span>
              <button
                class="mini-button"
                type="button"
                disabled={busy()}
                aria-label={`Remove submodule ${item.path}`}
                onClick={() => removeSubmodule(item.path)}
              >
                Remove
              </button>
            </div>
          )}
        </For>
        <form class="git-tools-form" onSubmit={addSubmodule}>
          <input
            aria-label="Submodule repository URL"
            value={url()}
            onInput={(event) => setUrl(event.currentTarget.value)}
            placeholder="SSH or HTTPS repository URL"
            required
          />
          <input
            aria-label="Submodule directory"
            value={directory()}
            onInput={(event) => setDirectory(event.currentTarget.value)}
            placeholder="vendor/library"
            required
          />
          <button class="mini-button" type="submit" disabled={busy()}>
            Add submodule
          </button>
        </form>
      </section>
      <section>
        <div class="git-tools-heading">
          <h3>Sparse checkout</h3>
          <span class="muted">{info()?.sparseEnabled ? "On" : "Off"}</span>
        </div>
        <p class="muted">
          Keep selected folders or Git ignore-style patterns in this worktree.
          Commit or stash changes before applying a new selection.
        </p>
        <form class="git-tools-form" onSubmit={setSparse}>
          <select
            aria-label="Sparse checkout mode"
            value={sparseMode()}
            onChange={(event) =>
              setSparseMode(event.currentTarget.value as "folders" | "patterns")
            }
          >
            <option value="folders">Folders</option>
            <option value="patterns">Advanced patterns</option>
          </select>
          <textarea
            aria-label={
              sparseMode() === "folders"
                ? "Sparse checkout directories"
                : "Sparse checkout patterns"
            }
            value={sparseDirectories()}
            onInput={(event) => setSparseDirectories(event.currentTarget.value)}
            placeholder={
              sparseMode() === "folders"
                ? "src\ndocs"
                : "/src/\n!/src/generated/"
            }
            rows={4}
          />
          <div class="git-tools-buttons">
            <button class="mini-button" type="submit" disabled={busy()}>
              Apply {sparseMode()}
            </button>
            <button
              class="mini-button"
              type="button"
              disabled={busy() || !info()?.sparseEnabled}
              onClick={() =>
                void run(
                  () => api.gitSparseDisable(props.repository),
                  "Sparse checkout disabled; the full worktree is restored.",
                )
              }
            >
              Restore all folders
            </button>
          </div>
        </form>
      </section>
      <section>
        <div class="git-tools-heading">
          <h3>Commit signing</h3>
          <span class="muted">{signing()?.headSignature}</span>
        </div>
        <Show when={signing()?.headSigner}>
          <p class="muted">Latest commit: {signing()?.headSigner}</p>
        </Show>
        <p class="muted">
          Git uses the signing key and tool installed on {thisComputer}.
          Verification may also require a trusted keys file.
        </p>
        <form class="git-tools-form" onSubmit={saveSigning}>
          <label class="git-tools-checkbox">
            <input
              type="checkbox"
              checked={signingEnabled()}
              onChange={(event) =>
                setSigningEnabled(event.currentTarget.checked)
              }
            />
            Sign new commits in this repository
          </label>
          <select
            aria-label="Commit signing format"
            value={signingFormat()}
            onChange={(event) =>
              setSigningFormat(event.currentTarget.value as "openpgp" | "ssh")
            }
          >
            <option value="openpgp">OpenPGP</option>
            <option value="ssh">SSH</option>
          </select>
          <input
            aria-label="Commit signing key"
            value={signingKey()}
            onInput={(event) => setSigningKey(event.currentTarget.value)}
            placeholder={
              signingFormat() === "ssh"
                ? "SSH public key or .pub file path"
                : "OpenPGP key ID"
            }
          />
          <button class="mini-button" type="submit" disabled={busy()}>
            Save signing settings
          </button>
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            title="Create an Ed25519 key in ~/.ssh without a passphrase, sign commits with it, and trust it here"
            onClick={() => void generateSigningKey()}
          >
            Generate SSH signing key
          </button>
        </form>
        <Show when={signingFormat() === "openpgp"}>
          <OpenPgpKeys
            repository={props.repository}
            onNotice={props.onNotice}
            onSigningChanged={(info) => {
              setSigning(info);
              setSigningEnabled(info.enabled);
              setSigningKey(info.key);
            }}
          />
        </Show>
        <Show when={signingFormat() === "ssh"}>
          <h4>Trust an SSH signing key</h4>
          <p class="muted">
            Add a public key that Git should trust when verifying commits in
            this repository. Review the key and its owner before adding it.
          </p>
          <Show when={signing()?.allowedSignersFile}>
            <p class="muted">Trust file: {signing()?.allowedSignersFile}</p>
          </Show>
          <form class="git-tools-form" onSubmit={addTrustedSigner}>
            <input
              aria-label="Trusted signer email or account name"
              value={trustPrincipal()}
              onInput={(event) => setTrustPrincipal(event.currentTarget.value)}
              placeholder="person@example.com"
              required
            />
            <input
              aria-label="Trusted SSH public key file path"
              value={trustPublicKeyPath()}
              onInput={(event) =>
                setTrustPublicKeyPath(event.currentTarget.value)
              }
              placeholder="/Users/name/.ssh/id_ed25519.pub"
              required
            />
            <button class="mini-button" type="submit" disabled={busy()}>
              Trust signing key
            </button>
          </form>
        </Show>
        <div class="git-tools-heading">
          <h4>Signature history</h4>
          <button
            class="mini-button"
            type="button"
            disabled={auditBusy()}
            onClick={() => void loadSignatureAudit(true)}
          >
            Refresh
          </button>
        </div>
        <p class="muted">
          Checks commits reachable from all local refs, 50 at a time.
        </p>
        <For
          each={signatureAudit()}
          fallback={<p class="empty-small">No commits to audit.</p>}
        >
          {(entry) => (
            <div class="git-reflog-item">
              <div>
                <strong>{entry.subject}</strong>
                <span class="muted">
                  {entry.commit.slice(0, 8)} · {entry.status}
                  {entry.signer ? ` · ${entry.signer}` : ""}
                </span>
              </div>
            </div>
          )}
        </For>
        <Show when={!auditComplete() && signatureAudit().length > 0}>
          <button
            class="mini-button"
            type="button"
            disabled={auditBusy()}
            onClick={() => void loadSignatureAudit()}
          >
            {auditBusy() ? "Loading…" : "Load more commits"}
          </button>
        </Show>
      </section>
      <section>
        <div class="git-tools-heading">
          <h3>Restore branch history</h3>
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() => void loadReflog()}
          >
            Refresh
          </button>
        </div>
        <p class="muted">
          Choose a recent Git state. Weave saves the current branch tip under a
          recovery branch before moving it.
        </p>
        <For
          each={reflog()}
          fallback={<p class="empty-small">No recent branch history.</p>}
        >
          {(entry) => (
            <div class="git-reflog-item">
              <div>
                <strong>{entry.action}</strong>
                <span class="muted">
                  {entry.selector} · {entry.commit.slice(0, 8)}
                </span>
              </div>
              <button
                class="mini-button"
                disabled={busy()}
                onClick={() => void restoreHistory(entry)}
              >
                Restore
              </button>
            </div>
          )}
        </For>
      </section>
    </div>
  );
}
