import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  Show,
} from "solid-js";
import { api, errorMessage } from "./api";
import type { BlameLine, Commit, CommitFile, LineHistoryEntry } from "./types";

const statusLabels: Record<CommitFile["status"], string> = {
  A: "Added",
  M: "Changed",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  T: "Type changed",
};

type Props = {
  repository: string;
  file?: string;
};

export default function FileHistoryPanel(props: Props) {
  const [file, setFile] = createSignal(props.file ?? "");
  const [mode, setMode] = createSignal<"history" | "blame" | "range">(
    "history",
  );
  const [commits, setCommits] = createSignal<Commit[]>([]);
  const [blame, setBlame] = createSignal<BlameLine[]>([]);
  const [blameStartLine, setBlameStartLine] = createSignal(1);
  const [blameHasMore, setBlameHasMore] = createSignal(false);
  const [blameLoadingMore, setBlameLoadingMore] = createSignal(false);
  const [blameMoreError, setBlameMoreError] = createSignal("");
  const [blameFile, setBlameFile] = createSignal("");
  const [blameQuery, setBlameQuery] = createSignal("");
  const visibleBlame = createMemo(() => {
    const query = blameQuery().trim().toLowerCase();
    if (!query) return blame();
    return blame().filter(
      (line) =>
        line.content.toLowerCase().includes(query) ||
        line.author.toLowerCase().includes(query) ||
        line.summary.toLowerCase().includes(query) ||
        line.commit.startsWith(query),
    );
  });
  const [lineHistory, setLineHistory] = createSignal<LineHistoryEntry[]>([]);
  const [startLine, setStartLine] = createSignal(1);
  const [endLine, setEndLine] = createSignal(1);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [openCommit, setOpenCommit] = createSignal<string>();
  const [commitFiles, setCommitFiles] = createSignal<CommitFile[]>([]);
  const [filesError, setFilesError] = createSignal("");
  const [backStack, setBackStack] = createSignal<string[]>([]);
  let request = 0;

  async function showCommitFiles(commit: string) {
    if (openCommit() === commit) {
      setOpenCommit(undefined);
      return;
    }
    setOpenCommit(commit);
    setCommitFiles([]);
    setFilesError("");
    try {
      const files = await api.gitCommitFiles(props.repository, commit);
      if (openCommit() === commit) setCommitFiles(files);
    } catch (reason) {
      if (openCommit() === commit) setFilesError(errorMessage(reason));
    }
  }

  function openFile(path: string, remember = true) {
    const current = file().trim();
    if (remember && current && current !== path)
      setBackStack((stack) => [...stack.slice(-19), current]);
    setFile(path);
    setOpenCommit(undefined);
    setMode("history");
    void load("history", path, props.repository, 1);
  }

  function goBack() {
    const previous = backStack().at(-1);
    if (!previous) return;
    setBackStack((stack) => stack.slice(0, -1));
    openFile(previous, false);
  }

  const commitFilesView = (commit: string) => (
    <Show when={openCommit() === commit}>
      <div class="commit-files">
        <Show when={filesError()}>
          <p class="tailnet-error" role="alert">
            {filesError()}
          </p>
        </Show>
        <For
          each={commitFiles()}
          fallback={
            <Show when={!filesError()}>
              <p class="empty-small">Loading files…</p>
            </Show>
          }
        >
          {(item) => (
            <button
              type="button"
              class={
                item.path === file().trim()
                  ? "commit-file current"
                  : "commit-file"
              }
              disabled={item.status === "D"}
              title={
                item.status === "D"
                  ? `${item.path} was deleted in this commit`
                  : `Show the history of ${item.path}`
              }
              onClick={() => openFile(item.path)}
            >
              <span class={`commit-file-status status-${item.status}`}>
                {item.status}
              </span>
              <span class="truncate">
                {item.oldPath ? `${item.oldPath} → ${item.path}` : item.path}
              </span>
              <span class="muted">{statusLabels[item.status]}</span>
            </button>
          )}
        </For>
      </div>
    </Show>
  );

  async function load(
    nextMode = mode(),
    name = file().trim(),
    repository = props.repository,
    blameFrom = blameStartLine(),
  ) {
    const current = ++request;
    if (!name) {
      setError("Enter a file path relative to the repository.");
      return;
    }
    setBusy(true);
    setError("");
    setBlameLoadingMore(false);
    setBlameMoreError("");
    try {
      if (nextMode === "history") {
        const result = await api.gitFileHistory(repository, name);
        if (current === request && repository === props.repository)
          setCommits(result);
      } else if (nextMode === "blame") {
        const result = await api.gitBlamePage(repository, name, blameFrom);
        if (current === request && repository === props.repository) {
          setBlame(result.lines);
          setBlameHasMore(result.hasMore);
          setBlameFile(name);
          setBlameMoreError("");
        }
      } else {
        const result = await api.gitLineHistory(
          repository,
          name,
          startLine(),
          endLine(),
        );
        if (current === request && repository === props.repository)
          setLineHistory(result);
      }
    } catch (reason) {
      if (current === request && repository === props.repository)
        setError(errorMessage(reason));
    } finally {
      if (current === request && repository === props.repository)
        setBusy(false);
    }
  }

  async function loadMoreBlame() {
    const last = blame().at(-1);
    if (!last || !blameHasMore() || blameLoadingMore() || busy()) return;
    const current = ++request;
    const repository = props.repository;
    const name = blameFile();
    setBlameLoadingMore(true);
    setBlameMoreError("");
    try {
      const result = await api.gitBlamePage(repository, name, last.number + 1);
      if (current === request && repository === props.repository) {
        setBlame((previous) => [...previous, ...result.lines]);
        setBlameHasMore(result.hasMore);
      }
    } catch (reason) {
      if (current === request && repository === props.repository)
        setBlameMoreError(errorMessage(reason));
    } finally {
      if (current === request && repository === props.repository)
        setBlameLoadingMore(false);
    }
  }

  createEffect(
    on(
      () => [props.repository, props.file] as const,
      ([repository, nextFile]) => {
        request += 1;
        setCommits([]);
        setBlame([]);
        setBlameStartLine(1);
        setBlameHasMore(false);
        setBlameLoadingMore(false);
        setBlameMoreError("");
        setBlameFile("");
        setLineHistory([]);
        setStartLine(1);
        setEndLine(1);
        setFile(nextFile ?? "");
        setOpenCommit(undefined);
        setBackStack([]);
        setMode("history");
        setError("");
        setBusy(false);
        if (nextFile) void load("history", nextFile, repository);
      },
    ),
  );

  function switchMode(next: "history" | "blame" | "range") {
    setMode(next);
    if (next === "blame") setBlameStartLine(1);
    void load(next, file().trim(), props.repository, 1);
  }

  return (
    <div class="file-history-panel">
      <form
        class="commit-search"
        onSubmit={(event) => {
          event.preventDefault();
          if (mode() === "blame") setBlameStartLine(1);
          void load(mode(), file().trim(), props.repository, 1);
        }}
      >
        <input
          aria-label="Repository relative file path"
          value={file()}
          onInput={(event) => setFile(event.currentTarget.value)}
          placeholder="src/example.ts"
        />
        <button
          class="mini-button"
          type="submit"
          disabled={busy() || !file().trim()}
        >
          Open
        </button>
        <Show when={backStack().length}>
          <button
            class="mini-button"
            type="button"
            title={`Back to ${backStack().at(-1)}`}
            onClick={goBack}
          >
            Back
          </button>
        </Show>
      </form>
      <div class="file-history-tabs">
        <button
          type="button"
          class={mode() === "history" ? "active" : ""}
          onClick={() => switchMode("history")}
        >
          History
        </button>
        <button
          type="button"
          class={mode() === "blame" ? "active" : ""}
          onClick={() => switchMode("blame")}
        >
          Blame
        </button>
        <button
          type="button"
          class={mode() === "range" ? "active" : ""}
          onClick={() => switchMode("range")}
        >
          Line history
        </button>
      </div>
      <Show when={mode() === "range"}>
        <form
          class="line-history-controls"
          onSubmit={(event) => {
            event.preventDefault();
            void load("range");
          }}
        >
          <label>
            From{" "}
            <input
              aria-label="First line"
              type="number"
              min="1"
              value={startLine()}
              onInput={(event) =>
                setStartLine(Number(event.currentTarget.value))
              }
            />
          </label>
          <label>
            To{" "}
            <input
              aria-label="Last line"
              type="number"
              min="1"
              value={endLine()}
              onInput={(event) => setEndLine(Number(event.currentTarget.value))}
            />
          </label>
          <button
            class="mini-button"
            type="submit"
            disabled={busy() || !file().trim()}
          >
            Trace lines
          </button>
        </form>
      </Show>
      <Show when={mode() === "blame"}>
        <form
          class="line-history-controls"
          onSubmit={(event) => {
            event.preventDefault();
            void load("blame");
          }}
        >
          <label>
            Start at line{" "}
            <input
              aria-label="Blame starting line"
              type="number"
              min="1"
              max="10000000"
              value={blameStartLine()}
              onInput={(event) =>
                setBlameStartLine(Number(event.currentTarget.value))
              }
            />
          </label>
          <button
            class="mini-button"
            type="submit"
            disabled={
              busy() ||
              !file().trim() ||
              !Number.isInteger(blameStartLine()) ||
              blameStartLine() < 1 ||
              blameStartLine() > 10_000_000
            }
          >
            Go to line
          </button>
          <input
            class="blame-search"
            type="search"
            aria-label="Find in blame"
            placeholder="Find text, author or commit"
            value={blameQuery()}
            onInput={(event) => setBlameQuery(event.currentTarget.value)}
          />
          <Show when={blameQuery().trim()}>
            <span class="muted">
              {visibleBlame().length} of {blame().length} lines
              {blameHasMore() ? " loaded" : ""}
            </span>
          </Show>
        </form>
      </Show>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={busy()}>
        <p class="empty-small">Loading file details…</p>
      </Show>
      <Show when={!busy() && !error() && mode() === "history"}>
        <For
          each={commits()}
          fallback={
            <p class="empty-small">
              No history for this file on the current branch.
            </p>
          }
        >
          {(commit) => (
            <>
              <button
                type="button"
                class="commit-row commit-row-button"
                aria-expanded={openCommit() === commit.id}
                title="Show the other files this commit changed"
                onClick={() => void showCommitFiles(commit.id)}
              >
                <div class="commit-main">
                  <div class="commit-subject">{commit.subject}</div>
                  <div class="commit-meta">
                    {commit.author} · {commit.relativeDate}
                  </div>
                </div>
                <code>{commit.shortId}</code>
              </button>
              {commitFilesView(commit.id)}
            </>
          )}
        </For>
      </Show>
      <Show when={!busy() && !error() && mode() === "blame"}>
        <div class="blame-lines">
          <For
            each={visibleBlame()}
            fallback={
              <p class="empty-small">
                {blameQuery().trim()
                  ? "No loaded lines match."
                  : "No blame information for this file."}
              </p>
            }
          >
            {(line, index) => (
              <>
                <button
                  type="button"
                  class="blame-line"
                  title={`${line.summary} · ${line.commit} — click for the files this commit changed`}
                  onClick={() => void showCommitFiles(line.commit)}
                >
                  <span>{line.number}</span>
                  <span>{line.author}</span>
                  <code>{line.content || " "}</code>
                </button>
                <Show
                  when={
                    openCommit() === line.commit &&
                    visibleBlame().findIndex(
                      (item) => item.commit === line.commit,
                    ) === index()
                  }
                >
                  {commitFilesView(line.commit)}
                </Show>
              </>
            )}
          </For>
          <Show when={blameHasMore()}>
            <button
              class="mini-button"
              type="button"
              disabled={blameLoadingMore()}
              onClick={() => void loadMoreBlame()}
            >
              {blameLoadingMore() ? "Loading lines…" : "Load next 250 lines"}
            </button>
          </Show>
          <Show when={blameMoreError()}>
            <p class="tailnet-error" role="alert">
              {blameMoreError()}
            </p>
          </Show>
        </div>
      </Show>
      <Show when={!busy() && !error() && mode() === "range"}>
        <For
          each={lineHistory()}
          fallback={
            <p class="empty-small">No changes found for this line range.</p>
          }
        >
          {(entry) => (
            <div class="line-history-entry">
              <strong>{entry.commit.subject}</strong>
              <span class="muted">
                {entry.commit.author} · {entry.commit.relativeDate} ·{" "}
                {entry.commit.shortId}
              </span>
              <pre class="lfs-patterns">{entry.patch}</pre>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
