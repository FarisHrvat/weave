import {
  createEffect,
  createResource,
  createSignal,
  For,
  Show,
} from "solid-js";
import { X } from "lucide-solid";
import { api, errorMessage } from "./api";
import AiAsk from "./AiAsk";
import { DiffPrefsBar, ImageCompare, isImage, ReadOnlyDiff } from "./DiffCode";
import type { CommitFile } from "./types";

type Props = {
  path: string;
  commit: string;
  busy: boolean;
  onClose: () => void;
  onRun: (name: string, callback: () => Promise<unknown>) => Promise<boolean>;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
};

const statusNames: Record<CommitFile["status"], string> = {
  A: "Added",
  M: "Modified",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  T: "Type changed",
};

/** Everything about one commit from the history: details, files, diffs and actions. */
export default function CommitDetail(props: Props) {
  const [file, setFile] = createSignal<CommitFile>();
  const [explanation, setExplanation] = createSignal("");
  const [branchName, setBranchName] = createSignal("");
  const [naming, setNaming] = createSignal(false);
  const [details] = createResource(
    () => [props.path, props.commit] as const,
    async ([path, commit]) => {
      const [info, files] = await Promise.all([
        api.gitCommitInfo(path, commit),
        api.gitCommitFiles(path, commit),
      ]);
      return { info, files };
    },
  );
  const [diff] = createResource(
    () => {
      const chosen = file();
      return chosen && !isImage(chosen.path)
        ? ([props.path, props.commit, chosen.path] as const)
        : undefined;
    },
    ([path, commit, chosen]) =>
      api.gitCommitFileDiff(path, commit, chosen).catch(errorMessage),
  );

  createEffect(() => {
    props.commit;
    setFile(undefined);
    setExplanation("");
    setNaming(false);
  });

  const info = () => details()?.info;
  const short = () => props.commit.slice(0, 8);

  async function copyId() {
    try {
      await navigator.clipboard.writeText(info()?.id ?? props.commit);
      props.onNotice("Commit ID copied.");
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function createBranch(event: Event) {
    event.preventDefault();
    const name = branchName().trim();
    if (!name) return;
    if (
      await props.onRun(`Create branch ${name}`, () =>
        api.gitOperation(
          props.path,
          "branch_at",
          `${name} ${props.commit}`,
          false,
        ),
      )
    ) {
      setNaming(false);
      setBranchName("");
    }
  }

  return (
    <div class="commit-detail">
      <div class="commit-detail-head">
        <code>{short()}</code>
        <button
          type="button"
          class="icon-button"
          title="Back to changed files"
          aria-label="Close commit"
          onClick={props.onClose}
        >
          <X size={15} />
        </button>
      </div>
      <Show when={details.error}>
        <p class="form-error">{errorMessage(details.error)}</p>
      </Show>
      <Show when={info()}>
        {(commit) => (
          <>
            <div class="commit-detail-info">
              <h3>{commit().subject}</h3>
              <Show when={commit().body}>
                <p class="commit-detail-body">{commit().body}</p>
              </Show>
              <dl>
                <dt>Author</dt>
                <dd>
                  {commit().author} &lt;{commit().authorEmail}&gt; ·{" "}
                  {commit().authorDate}
                </dd>
                <Show
                  when={
                    commit().committer !== commit().author ||
                    commit().committerDate !== commit().authorDate
                  }
                >
                  <dt>Committed</dt>
                  <dd>
                    {commit().committer} · {commit().committerDate}
                  </dd>
                </Show>
                <dt>Parents</dt>
                <dd>
                  {commit()
                    .parents.map((parent) => parent.slice(0, 8))
                    .join(", ") || "None (first commit)"}
                </dd>
              </dl>
            </div>
            <div class="git-tools-buttons commit-detail-actions">
              <button
                type="button"
                class="mini-button"
                onClick={() => void copyId()}
              >
                Copy ID
              </button>
              <button
                type="button"
                class="mini-button"
                disabled={props.busy}
                onClick={() => setNaming(!naming())}
              >
                Branch here…
              </button>
              <button
                type="button"
                class="mini-button"
                disabled={props.busy}
                onClick={() =>
                  void props.onRun("Cherry-pick", () =>
                    api.gitOperation(
                      props.path,
                      "cherry_pick",
                      props.commit,
                      false,
                    ),
                  )
                }
              >
                Cherry-pick
              </button>
              <button
                type="button"
                class="mini-button"
                disabled={props.busy}
                onClick={() =>
                  void props.onRun("Revert", () =>
                    api.gitOperation(props.path, "revert", props.commit, false),
                  )
                }
              >
                Revert
              </button>
              <button
                type="button"
                class="mini-button"
                disabled={props.busy}
                title="Check out this commit without a branch"
                onClick={() =>
                  void props.onRun("Check out commit", () =>
                    api.gitOperation(
                      props.path,
                      "checkout_detached",
                      props.commit,
                      false,
                    ),
                  )
                }
              >
                Check out
              </button>
              <AiAsk
                label="Explain"
                busyLabel="Explaining…"
                title="Explain this commit with Claude, using your own API key"
                run={() => api.gitAiExplainCommit(props.path, props.commit)}
                onResult={setExplanation}
                onError={props.onError}
              />
            </div>
            <Show when={naming()}>
              <form class="ai-key-form" onSubmit={createBranch}>
                <input
                  aria-label="New branch name"
                  placeholder="New branch name"
                  spellcheck={false}
                  value={branchName()}
                  onInput={(event) => setBranchName(event.currentTarget.value)}
                />
                <button class="mini-button" type="submit" disabled={props.busy}>
                  Create
                </button>
              </form>
            </Show>
            <Show when={explanation()}>
              <p class="commit-explanation">{explanation()}</p>
            </Show>
          </>
        )}
      </Show>
      <div class="commit-detail-files" role="list">
        <For each={details()?.files ?? []}>
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
              onClick={() => setFile(changed)}
            >
              <span
                class={`file-status status-${changed.status}`}
                title={statusNames[changed.status]}
              >
                {changed.status}
              </span>
              <span class="commit-detail-path">{changed.path}</span>
            </button>
          )}
        </For>
        <Show when={details() && details()!.files.length === 0}>
          <p class="muted">This commit changes no files.</p>
        </Show>
      </div>
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
                before={
                  info()?.parents[0] && chosen().status !== "A"
                    ? info()!.parents[0]
                    : null
                }
                after={chosen().status === "D" ? null : props.commit}
              />
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
