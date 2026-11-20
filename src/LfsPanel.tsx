import { confirmAction } from "./dialogs";
import { createEffect, createSignal, For, Show } from "solid-js";
import { api, errorMessage } from "./api";
import type { GitLfsInfo, LfsLock } from "./types";

type Props = {
  repository: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
};

export default function LfsPanel(props: Props) {
  const [info, setInfo] = createSignal<GitLfsInfo>();
  const [pattern, setPattern] = createSignal("");
  const [lockFile, setLockFile] = createSignal("");
  const [locks, setLocks] = createSignal<LfsLock[]>([]);
  const [locksLoaded, setLocksLoaded] = createSignal(false);
  const [migrationPattern, setMigrationPattern] = createSignal("");
  const [previewPattern, setPreviewPattern] = createSignal("");
  const [migrationPreview, setMigrationPreview] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function refresh() {
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const result = await api.gitLfsInfo(repository);
      if (repository === props.repository) setInfo(result);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function changeTracking(track: boolean) {
    if (!pattern().trim()) {
      setError("Enter a file pattern, such as *.psd.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      setInfo(await api.gitLfsTrack(props.repository, pattern(), track));
      props.onChanged();
      props.onNotice(
        `Git LFS ${track ? "tracks" : "no longer tracks"} ${pattern().trim()}. Review and commit .gitattributes.`,
      );
      setPattern("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function pull() {
    setBusy(true);
    setError("");
    try {
      setInfo(await api.gitLfsPull(props.repository));
      props.onChanged();
      props.onNotice("Git LFS objects downloaded.");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function loadLocks(remote: boolean) {
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const result = await api.gitLfsLocks(repository, remote);
      if (repository !== props.repository) return;
      setLocks(result);
      setLocksLoaded(true);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function changeLock(file: string, lock: boolean) {
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      await api.gitLfsChangeLock(repository, file, lock);
      if (repository !== props.repository) return;
      setLockFile("");
      props.onNotice(
        `${file} ${lock ? "locked" : "unlocked"} on the Git LFS server.`,
      );
      setLocks(await api.gitLfsLocks(repository, false));
      setLocksLoaded(true);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function previewMigration() {
    const repository = props.repository;
    const requestedPattern = migrationPattern().trim();
    setBusy(true);
    setError("");
    setPreviewPattern("");
    setMigrationPreview("");
    try {
      const result = await api.gitLfsMigratePreview(
        repository,
        requestedPattern,
      );
      if (repository !== props.repository) return;
      setMigrationPreview(result || "No matching objects found.");
      setPreviewPattern(requestedPattern);
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  async function migrate() {
    const selectedPattern = previewPattern();
    if (!selectedPattern || selectedPattern !== migrationPattern().trim()) {
      setError("Preview the current pattern before starting migration.");
      return;
    }
    if (
      !(await confirmAction(
        `Rewrite the current branch history to migrate ${selectedPattern} to Git LFS? Commit IDs will change and a force push may be required. Weave will create a recovery branch first.`,
      ))
    )
      return;
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const result = await api.gitLfsMigrateImport(repository, selectedPattern);
      if (repository !== props.repository) return;
      setInfo(result.info);
      setPreviewPattern("");
      setMigrationPreview("");
      props.onNotice(
        `Git LFS migration complete. Previous history saved as ${result.recoveryBranch}. Review rewritten commits before pushing.`,
      );
      props.onChanged();
    } catch (reason) {
      if (repository === props.repository) setError(errorMessage(reason));
    } finally {
      if (repository === props.repository) setBusy(false);
    }
  }

  createEffect(() => {
    props.repository;
    setInfo(undefined);
    setLocks([]);
    setLocksLoaded(false);
    setPreviewPattern("");
    setMigrationPreview("");
    void refresh();
  });

  return (
    <div class="lfs-panel">
      <div class="lfs-toolbar">
        <input
          aria-label="Git LFS file pattern"
          value={pattern()}
          onInput={(event) => setPattern(event.currentTarget.value)}
          placeholder="*.psd"
        />
        <button
          class="mini-button"
          type="button"
          disabled={busy()}
          onClick={() => void changeTracking(true)}
        >
          Track
        </button>
        <button
          class="mini-button"
          type="button"
          disabled={busy()}
          onClick={() => void changeTracking(false)}
        >
          Untrack
        </button>
      </div>
      <p class="tailnet-guidance">
        Tracking updates .gitattributes. Stage and commit that file to share the
        rule.
      </p>
      <div class="lfs-toolbar">
        <button
          class="mini-button"
          type="button"
          disabled={busy()}
          onClick={() => void pull()}
        >
          Download LFS files
        </button>
        <button
          class="mini-button"
          type="button"
          disabled={busy()}
          onClick={() => void refresh()}
        >
          Refresh
        </button>
      </div>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={busy()}>
        <p class="empty-small">Checking Git LFS…</p>
      </Show>
      <Show when={info() && !busy()}>
        <p class="lfs-version">{info()?.version}</p>
        <strong class="lfs-heading">Tracked patterns</strong>
        <pre class="lfs-patterns">
          {info()?.trackedPatterns || "No patterns yet."}
        </pre>
        <strong class="lfs-heading">LFS files</strong>
        <For
          each={info()?.trackedFiles}
          fallback={<p class="empty-small">No LFS files in this checkout.</p>}
        >
          {(file) => <div class="lfs-file">{file}</div>}
        </For>
      </Show>
      <section>
        <strong class="lfs-heading">File locks</strong>
        <p class="muted">
          Lock an existing file to coordinate edits through the Git LFS server.
          Unlock only your own files.
        </p>
        <div class="lfs-toolbar">
          <input
            aria-label="LFS file to lock"
            value={lockFile()}
            onInput={(event) => setLockFile(event.currentTarget.value)}
            placeholder="artwork/design.psd"
          />
          <button
            class="mini-button"
            type="button"
            disabled={busy() || !lockFile().trim()}
            onClick={() => void changeLock(lockFile().trim(), true)}
          >
            Lock
          </button>
        </div>
        <div class="lfs-toolbar">
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            onClick={() => void loadLocks(false)}
          >
            My locks
          </button>
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            onClick={() => void loadLocks(true)}
          >
            Refresh server locks
          </button>
        </div>
        <Show when={locksLoaded()}>
          <For
            each={locks()}
            fallback={<p class="empty-small">No locks found.</p>}
          >
            {(item) => (
              <div class="lfs-lock">
                <span>
                  <strong>{item.path}</strong> {item.owner && `· ${item.owner}`}
                </span>
                <Show when={item.ours}>
                  <button
                    class="mini-button"
                    type="button"
                    disabled={busy()}
                    onClick={() => void changeLock(item.path, false)}
                  >
                    Unlock
                  </button>
                </Show>
              </div>
            )}
          </For>
        </Show>
      </section>
      <section>
        <strong class="lfs-heading">Migrate existing history</strong>
        <p class="muted">
          Convert matching files on the current branch to LFS. This rewrites
          commit IDs. A recovery branch preserves the original tip.
        </p>
        <div class="lfs-toolbar">
          <input
            aria-label="LFS migration pattern"
            value={migrationPattern()}
            onInput={(event) => {
              setMigrationPattern(event.currentTarget.value);
              setPreviewPattern("");
            }}
            placeholder="*.psd"
          />
          <button
            class="mini-button"
            type="button"
            disabled={busy() || !migrationPattern().trim()}
            onClick={() => void previewMigration()}
          >
            Preview
          </button>
        </div>
        <Show when={previewPattern()}>
          <pre class="lfs-patterns">{migrationPreview()}</pre>
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            onClick={() => void migrate()}
          >
            Migrate current branch
          </button>
        </Show>
      </section>
    </div>
  );
}
