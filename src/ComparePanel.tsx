import { batch, createSignal, For, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage } from "./api";
import AiAsk from "./AiAsk";
import { DiffPrefsBar, ImageCompare, isImage, ReadOnlyDiff } from "./DiffCode";
import type { CommitFile, Comparison, GitSnapshot } from "./types";

type Props = {
  path: string;
  snapshot?: GitSnapshot;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
};

/** Compares two branches, tags or commits, or any two files or folders. */
export default function ComparePanel(props: Props) {
  const [kind, setKind] = createSignal<"refs" | "paths">("refs");
  const [base, setBase] = createSignal("");
  const [target, setTarget] = createSignal("");
  const [sinceFork, setSinceFork] = createSignal(true);
  const [result, setResult] = createSignal<Comparison>();
  const [compared, setCompared] = createSignal<[string, string, boolean]>();
  const [file, setFile] = createSignal<CommitFile>();
  const [diff, setDiff] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [description, setDescription] = createSignal("");
  const [left, setLeft] = createSignal("");
  const [right, setRight] = createSignal("");
  const [pathDiff, setPathDiff] = createSignal<string>();

  const names = () => [
    ...(props.snapshot?.branches ?? []),
    ...(props.snapshot?.remoteBranches ?? []),
    ...(props.snapshot?.tags ?? []),
  ];

  async function compare(event: Event) {
    event.preventDefault();
    const from = base().trim() || props.snapshot?.branch || "HEAD";
    const to = target().trim() || "HEAD";
    setBusy(true);
    setFile(undefined);
    setDiff(undefined);
    setDescription("");
    try {
      const comparison = await api.gitCompareRefs(
        props.path,
        from,
        to,
        sinceFork(),
      );
      batch(() => {
        setCompared([from, to, sinceFork()]);
        setResult(comparison);
      });
    } catch (error) {
      setResult(undefined);
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function showFile(changed: CommitFile) {
    const range = compared();
    if (!range) return;
    setFile(changed);
    setDiff(undefined);
    if (isImage(changed.path)) return;
    try {
      setDiff(
        await api.gitCompareFileDiff(
          props.path,
          range[0],
          range[1],
          range[2],
          changed.path,
          changed.oldPath,
        ),
      );
    } catch (error) {
      setDiff(errorMessage(error));
    }
  }

  async function choose(set: (value: string) => void, directory: boolean) {
    const chosen = await open({
      directory,
      multiple: false,
      title: directory
        ? "Choose a folder to compare"
        : "Choose a file to compare",
    });
    if (typeof chosen === "string") set(chosen);
  }

  async function comparePaths(event: Event) {
    event.preventDefault();
    setBusy(true);
    try {
      setPathDiff(await api.gitComparePaths(left().trim(), right().trim()));
    } catch (error) {
      setPathDiff(undefined);
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="compare-panel">
      <div class="diff-mode compare-kind">
        <button
          type="button"
          class={kind() === "refs" ? "selected" : ""}
          onClick={() => setKind("refs")}
        >
          Branches and commits
        </button>
        <button
          type="button"
          class={kind() === "paths" ? "selected" : ""}
          onClick={() => setKind("paths")}
        >
          Files and folders
        </button>
      </div>
      <datalist id="compare-refs">
        <For each={names()}>{(name) => <option value={name} />}</For>
      </datalist>
      <Show
        when={kind() === "refs"}
        fallback={
          <>
            <form class="compare-form" onSubmit={comparePaths}>
              <label>
                Left
                <span class="compare-path">
                  <input
                    required
                    spellcheck={false}
                    placeholder="/path/to/old"
                    value={left()}
                    onInput={(event) => setLeft(event.currentTarget.value)}
                  />
                  <button
                    type="button"
                    class="mini-button"
                    onClick={() => void choose(setLeft, false)}
                  >
                    File…
                  </button>
                  <button
                    type="button"
                    class="mini-button"
                    onClick={() => void choose(setLeft, true)}
                  >
                    Folder…
                  </button>
                </span>
              </label>
              <label>
                Right
                <span class="compare-path">
                  <input
                    required
                    spellcheck={false}
                    placeholder="/path/to/new"
                    value={right()}
                    onInput={(event) => setRight(event.currentTarget.value)}
                  />
                  <button
                    type="button"
                    class="mini-button"
                    onClick={() => void choose(setRight, false)}
                  >
                    File…
                  </button>
                  <button
                    type="button"
                    class="mini-button"
                    onClick={() => void choose(setRight, true)}
                  >
                    Folder…
                  </button>
                </span>
              </label>
              <div class="compare-actions">
                <button class="primary-button" type="submit" disabled={busy()}>
                  {busy() ? "Comparing…" : "Compare"}
                </button>
                <DiffPrefsBar />
              </div>
            </form>
            <Show when={pathDiff() !== undefined}>
              <ReadOnlyDiff diff={pathDiff()!} empty="These are identical." />
            </Show>
          </>
        }
      >
        <form class="compare-form" onSubmit={compare}>
          <label>
            Base
            <input
              list="compare-refs"
              spellcheck={false}
              placeholder={props.snapshot?.branch || "main"}
              value={base()}
              onInput={(event) => setBase(event.currentTarget.value)}
            />
          </label>
          <label>
            Compare
            <input
              list="compare-refs"
              spellcheck={false}
              placeholder="HEAD"
              value={target()}
              onInput={(event) => setTarget(event.currentTarget.value)}
            />
          </label>
          <label class="check-label">
            <input
              type="checkbox"
              checked={sinceFork()}
              onChange={(event) => setSinceFork(event.currentTarget.checked)}
            />
            Only changes since they split, like a pull request
          </label>
          <div class="compare-actions">
            <button class="primary-button" type="submit" disabled={busy()}>
              {busy() ? "Comparing…" : "Compare"}
            </button>
            <Show when={compared()}>
              <AiAsk
                label="Write pull request description"
                busyLabel="Writing…"
                title="Describe these changes with Claude, using your own API key"
                run={() =>
                  api.gitAiPullRequest(
                    props.path,
                    compared()![0],
                    compared()![1],
                  )
                }
                onResult={setDescription}
                onError={props.onError}
              />
            </Show>
          </div>
        </form>
        <Show when={description()}>
          <div class="compare-description">
            <textarea
              aria-label="Pull request description"
              value={description()}
              onInput={(event) => setDescription(event.currentTarget.value)}
            />
            <button
              type="button"
              class="mini-button"
              onClick={() =>
                void navigator.clipboard
                  .writeText(description())
                  .then(() => props.onNotice("Description copied."))
                  .catch((error) => props.onError(errorMessage(error)))
              }
            >
              Copy
            </button>
          </div>
        </Show>
        <Show when={result()}>
          {(comparison) => (
            <>
              <p class="compare-summary">
                {compared()![1]} has {comparison().ahead} commit
                {comparison().ahead === 1 ? "" : "s"} not in {compared()![0]},
                and is {comparison().behind} behind it.{" "}
                {comparison().files.length} file
                {comparison().files.length === 1 ? " differs" : "s differ"}.
              </p>
              <div class="commit-detail-files" role="list">
                <For each={comparison().files}>
                  {(changed) => (
                    <button
                      type="button"
                      role="listitem"
                      class={`commit-detail-file ${file()?.path === changed.path ? "selected" : ""}`}
                      title={
                        changed.oldPath
                          ? `${changed.oldPath} → ${changed.path}`
                          : changed.path
                      }
                      onClick={() => void showFile(changed)}
                    >
                      <span class={`file-status status-${changed.status}`}>
                        {changed.status}
                      </span>
                      <span class="commit-detail-path">{changed.path}</span>
                    </button>
                  )}
                </For>
              </div>
            </>
          )}
        </Show>
        <Show when={file()}>
          {(chosen) => (
            <div class="commit-detail-diff">
              <div class="panel-title">
                <span class="commit-detail-path">{chosen().path}</span>
                <DiffPrefsBar />
              </div>
              <Show
                when={isImage(chosen().path)}
                fallback={
                  <Show
                    when={diff() !== undefined}
                    fallback={<p class="muted">Loading…</p>}
                  >
                    <ReadOnlyDiff diff={diff()!} empty="No textual changes." />
                  </Show>
                }
              >
                <ImageCompare
                  path={props.path}
                  file={chosen().path}
                  oldFile={chosen().oldPath}
                  before={chosen().status === "A" ? null : result()!.left}
                  after={chosen().status === "D" ? null : compared()![1]}
                />
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}
