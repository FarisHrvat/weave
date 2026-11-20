import { confirmAction, askText } from "./dialogs";
import { fileManager, keychain } from "./platform";
import VirtualRows from "./VirtualRows";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  CircleDot,
  GitBranch,
  GitCommitHorizontal,
  Minus,
  Plus,
  Redo2,
  RefreshCw,
  Search,
  Undo2,
  X,
} from "lucide-solid";
import { api, errorMessage } from "./api";
import ForgePanel from "./ForgePanel";
import ExternalForgePanel from "./ExternalForgePanel";
import { isExternalRemote } from "./externalRemote";
import JiraPanel from "./JiraPanel";
import FileHistoryPanel from "./FileHistoryPanel";
import LfsPanel from "./LfsPanel";
import GitToolsPanel from "./GitToolsPanel";
import RebasePanel from "./RebasePanel";
import GitAdvancedPanel from "./GitAdvancedPanel";
import { withFolders } from "./changeFolders";
import { splitDiff } from "./gitDiff";
import CommitDetail from "./CommitDetail";
import BranchCleanup from "./BranchCleanup";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import ComparePanel from "./ComparePanel";
import {
  CodeText,
  DiffPrefsBar,
  diffPrefs,
  ImageCompare,
  isImage,
  splitRows,
} from "./DiffCode";
import { languageFor, pairedWordDiffs } from "./diffRender";
import type {
  Commit,
  GitConflict,
  GitFile,
  GitIdentity,
  GitSnapshot,
  GitWorktree,
} from "./types";

type Props = {
  path: string;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
  onOpenTerminal: (cwd: string, startupCommand?: string) => void;
  onOpenRepository: (path: string) => void;
};
type GitAction = {
  operation: string;
  label: string;
  hint?: string;
  value?: string;
  destructive?: boolean;
};

const actions: GitAction[] = [
  {
    operation: "branch_rename",
    label: "Rename current branch",
    hint: "New branch name",
  },
  {
    operation: "branch_delete",
    label: "Delete a merged branch",
    hint: "Branch name",
  },
  { operation: "merge", label: "Merge branch", hint: "Branch to merge" },
  {
    operation: "merge_no_ff",
    label: "Merge branch with a merge commit",
    hint: "Branch to merge",
  },
  {
    operation: "merge_squash",
    label: "Squash branch into staged changes",
    hint: "Branch to squash",
  },
  {
    operation: "rebase",
    label: "Rebase onto branch",
    hint: "Branch to rebase onto",
  },
  { operation: "cherry_pick", label: "Cherry-pick commit", hint: "Commit ID" },
  { operation: "revert", label: "Revert commit", hint: "Commit ID" },
  { operation: "tag_create", label: "Create tag", hint: "Tag name" },
  { operation: "tag_delete", label: "Delete tag", hint: "Tag name" },
  { operation: "remote_add", label: "Add remote", hint: "Remote name and URL" },
  {
    operation: "remote_set_url",
    label: "Change remote URL",
    hint: "Remote name and new URL",
  },
  {
    operation: "remote_rename",
    label: "Rename remote",
    hint: "Current and new remote names",
  },
  { operation: "remote_remove", label: "Remove remote", hint: "Remote name" },
  { operation: "reset_soft", label: "Soft reset", hint: "Commit or ref" },
  { operation: "reset_mixed", label: "Mixed reset", hint: "Commit or ref" },
  {
    operation: "reset_hard",
    label: "Hard reset",
    hint: "Commit or ref",
    destructive: true,
  },
];

function hunkLines(hunk: string): string[] {
  const lines = hunk.split("\n").slice(1);
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

export default function GitView(props: Props) {
  const [snapshot, setSnapshot] = createSignal<GitSnapshot>();
  const [olderCommits, setOlderCommits] = createSignal<Commit[]>([]);
  const [olderBusy, setOlderBusy] = createSignal(false);
  const [olderComplete, setOlderComplete] = createSignal(false);
  const [selected, setSelected] = createSignal<{
    file: GitFile;
    staged: boolean;
  }>();
  const [diff, setDiff] = createSignal("");
  const [openCommit, setOpenCommit] = createSignal<string>();
  const [cleanupOpen, setCleanupOpen] = createSignal(false);
  const [graphOrder, setGraphOrder] = createSignal<"topo" | "date">("topo");
  const [message, setMessage] = createSignal("");
  const [writing, setWriting] = createSignal(false);
  const [askKey, setAskKey] = createSignal(false);
  const [aiKey, setAiKey] = createSignal("");

  async function writeMessage() {
    if (!(await api.aiKeyStatus())) {
      setAskKey(true);
      return;
    }
    setWriting(true);
    try {
      setMessage(await api.gitAiCommitMessage(props.path));
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setWriting(false);
    }
  }

  async function saveKey(event: Event) {
    event.preventDefault();
    try {
      await api.aiKeySet(aiKey());
      setAiKey("");
      setAskKey(false);
      await writeMessage();
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }
  const [busy, setBusy] = createSignal(false);
  const [undoInfo, setUndoInfo] = createSignal<{
    undo: string | null;
    redo: string | null;
  }>({ undo: null, redo: null });
  const [branchMenu, setBranchMenu] = createSignal(false);
  const [showCommand, setShowCommand] = createSignal(false);
  const [menuOpen, setMenuOpen] = createSignal(false);
  const [dialog, setDialog] = createSignal<GitAction>();
  const [dialogValue, setDialogValue] = createSignal("");
  const [conflict, setConflict] = createSignal<GitConflict>();
  const [resolution, setResolution] = createSignal("");
  const [identityOpen, setIdentityOpen] = createSignal(false);
  const [identity, setIdentity] = createSignal<GitIdentity>();
  const [identityScope, setIdentityScope] = createSignal<"local" | "global">(
    "local",
  );
  const [identityName, setIdentityName] = createSignal("");
  const [identityEmail, setIdentityEmail] = createSignal("");
  const [searchQuery, setSearchQuery] = createSignal("");
  const [searchMode, setSearchMode] = createSignal<
    "message" | "author" | "content"
  >("message");
  const [searchResults, setSearchResults] = createSignal<Commit[]>([]);
  const [searchActive, setSearchActive] = createSignal(false);
  const [searchBusy, setSearchBusy] = createSignal(false);
  const [centerTab, setCenterTab] = createSignal<
    | "history"
    | "online"
    | "jira"
    | "file"
    | "lfs"
    | "tools"
    | "advanced"
    | "rebase"
    | "compare"
  >("history");
  const [worktreeOpen, setWorktreeOpen] = createSignal(false);
  const [worktrees, setWorktrees] = createSignal<GitWorktree[]>([]);
  const [worktreeBusy, setWorktreeBusy] = createSignal(false);
  const [worktreeNewBranch, setWorktreeNewBranch] = createSignal(true);
  const [worktreeBranch, setWorktreeBranch] = createSignal("");
  const [worktreeParent, setWorktreeParent] = createSignal("");
  const [worktreeFolder, setWorktreeFolder] = createSignal("");
  const availableWorktreeBranches = () =>
    snapshot()?.branches.filter(
      (branch) => !worktrees().some((item) => item.branch === branch),
    ) ?? [];
  let searchRequest = 0;
  let refreshRequest = 0;
  let diffRequest = 0;
  let loadingPath: string | undefined;
  let historyVersion = 0;
  const [branchFilter, setBranchFilter] = createSignal("");
  const [folderView, setFolderView] = createSignal(false);
  const [filteredCommits, setFilteredCommits] = createSignal<Commit[]>([]);
  const historyCommits = createMemo(() => [
    ...(snapshot()?.commits ?? []),
    ...olderCommits(),
  ]);
  const visibleCommits = createMemo(() =>
    searchActive()
      ? searchResults()
      : branchFilter()
        ? filteredCommits()
        : historyCommits(),
  );

  async function filterHistory(branch: string) {
    setBranchFilter(branch);
    if (!branch) return;
    try {
      setFilteredCommits(await api.gitCommitsFor(props.path, [branch], 0));
    } catch (error) {
      setBranchFilter("");
      props.onError(errorMessage(error));
    }
  }

  async function loadOlderCommits() {
    if (olderBusy() || olderComplete() || !snapshot()) return;
    const path = props.path;
    const version = historyVersion;
    const skip = (snapshot()?.commits.length ?? 0) + olderCommits().length;
    setOlderBusy(true);
    try {
      const page = await api.gitCommitsPage(path, skip);
      if (version !== historyVersion || path !== props.path) return;
      setOlderCommits([...olderCommits(), ...page]);
      setOlderComplete(page.length < 300);
    } catch (error) {
      if (version === historyVersion && path === props.path)
        props.onError(errorMessage(error));
    } finally {
      if (version === historyVersion) setOlderBusy(false);
    }
  }

  async function searchCommits(event: Event) {
    event.preventDefault();
    const query = searchQuery().trim();
    if (!query) {
      clearSearch();
      return;
    }
    const request = ++searchRequest;
    setSearchBusy(true);
    try {
      const results = await api.searchCommits(props.path, query, searchMode());
      if (request !== searchRequest) return;
      setSearchResults(results);
      setSearchActive(true);
    } catch (error) {
      if (request === searchRequest) props.onError(errorMessage(error));
    } finally {
      if (request === searchRequest) setSearchBusy(false);
    }
  }

  function clearSearch() {
    searchRequest += 1;
    setSearchQuery("");
    setSearchResults([]);
    setSearchActive(false);
    setSearchBusy(false);
  }

  async function openIdentity() {
    try {
      const saved = await api.gitIdentity(props.path);
      setIdentity(saved);
      setIdentityScope("local");
      setIdentityName(saved.localName ?? saved.globalName ?? "");
      setIdentityEmail(saved.localEmail ?? saved.globalEmail ?? "");
      setIdentityOpen(true);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  function chooseIdentityScope(scope: "local" | "global") {
    const saved = identity();
    setIdentityScope(scope);
    setIdentityName(
      scope === "local"
        ? (saved?.localName ?? saved?.globalName ?? "")
        : (saved?.globalName ?? ""),
    );
    setIdentityEmail(
      scope === "local"
        ? (saved?.localEmail ?? saved?.globalEmail ?? "")
        : (saved?.globalEmail ?? ""),
    );
  }

  async function saveIdentity(event: Event) {
    event.preventDefault();
    setBusy(true);
    try {
      const saved = await api.saveGitIdentity(
        props.path,
        identityScope(),
        identityName(),
        identityEmail(),
      );
      setIdentity(saved);
      setIdentityOpen(false);
      props.onNotice(
        identityScope() === "local"
          ? "Commit identity saved for this repository."
          : "Global commit identity saved.",
      );
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function refresh() {
    const path = props.path;
    const request = ++refreshRequest;
    loadingPath = path;
    try {
      const result = await api.git(path);
      if (request === refreshRequest && props.path === path) {
        if (snapshot()?.commits[0]?.id !== result.commits[0]?.id) {
          historyVersion++;
          setOlderCommits([]);
          setOlderComplete(result.commits.length < 300);
          setOlderBusy(false);
        }
        setSnapshot(result);
        setUndoInfo(await api.gitUndoInfo(path));
      }
    } catch (error) {
      if (request === refreshRequest && props.path === path)
        props.onError(errorMessage(error));
    } finally {
      if (request === refreshRequest) loadingPath = undefined;
    }
  }

  async function openWorktrees() {
    setWorktreeOpen(true);
    setWorktreeBranch("");
    setWorktreeFolder("");
    setWorktreeParent(props.path.slice(0, props.path.lastIndexOf("/")));
    setWorktreeBusy(true);
    try {
      setWorktrees(await api.gitWorktrees(props.path));
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setWorktreeBusy(false);
    }
  }

  async function chooseWorktreeParent() {
    try {
      const parent = await open({
        directory: true,
        multiple: false,
        title: "Choose a folder for the new worktree",
      });
      if (typeof parent === "string") setWorktreeParent(parent);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function createWorktree(event: Event) {
    event.preventDefault();
    const folder = worktreeFolder().trim();
    if (!folder || folder === "." || folder === ".." || /[\\/]/.test(folder)) {
      props.onError("Enter a folder name without slashes.");
      return;
    }
    if (!worktreeParent()) {
      props.onError("Choose a parent folder for the worktree.");
      return;
    }
    setWorktreeBusy(true);
    try {
      const destination = `${worktreeParent().replace(/\/$/, "")}/${folder}`;
      const created = await api.gitWorktreeCreate(
        props.path,
        destination,
        worktreeBranch().trim(),
        worktreeNewBranch(),
      );
      setWorktreeOpen(false);
      props.onNotice(`Worktree created on ${worktreeBranch().trim()}.`);
      props.onOpenRepository(created);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setWorktreeBusy(false);
    }
  }

  async function removeWorktree(item: GitWorktree) {
    if (
      !(await confirmAction(
        `Remove the ${item.branch} worktree at ${item.path}?`,
      ))
    )
      return;
    setWorktreeBusy(true);
    try {
      await api.gitWorktreeRemove(props.path, item.path);
      setWorktrees(await api.gitWorktrees(props.path));
      props.onNotice("Worktree removed.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setWorktreeBusy(false);
    }
  }

  async function toggleWorktreeLock(item: GitWorktree) {
    setWorktreeBusy(true);
    try {
      setWorktrees(
        await api.gitWorktreeLock(props.path, item.path, !item.locked),
      );
      props.onNotice(item.locked ? "Worktree unlocked." : "Worktree locked.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setWorktreeBusy(false);
    }
  }

  createEffect(() => {
    if (!props.path) return;
    setOpenCommit(undefined);
    setGraphOrder("topo");
    void api
      .gitGraphOrder(props.path)
      .then(setGraphOrder)
      .catch(() => undefined);
    setSelected(undefined);
    setDiff("");
    setSnapshot(undefined);
    historyVersion++;
    setOlderCommits([]);
    setOlderComplete(false);
    setOlderBusy(false);
    clearSearch();
    void refresh();
  });
  onMount(() => {
    const interval = window.setInterval(() => {
      if (
        busy() ||
        loadingPath === props.path ||
        document.visibilityState !== "visible"
      )
        return;
      void refresh();
      void loadDiff();
    }, 5000);
    onCleanup(() => window.clearInterval(interval));
  });
  async function loadDiff() {
    const request = ++diffRequest;
    const path = props.path;
    const view = selected();
    if (!view) {
      setDiff("");
      return;
    }
    try {
      const result = await api.diff(path, view.file.path, view.staged);
      if (request === diffRequest && props.path === path) setDiff(result);
    } catch (error) {
      if (request === diffRequest && props.path === path)
        setDiff(errorMessage(error));
    }
  }

  createEffect(() => {
    selected();
    void loadDiff();
  });

  async function action(name: string, callback: () => Promise<unknown>) {
    setBusy(true);
    try {
      await callback();
      props.onNotice(`${name} completed.`);
      await refresh();
      await loadDiff();
      return true;
    } catch (error) {
      props.onError(errorMessage(error));
      await refresh();
      await loadDiff();
      return false;
    } finally {
      setBusy(false);
    }
  }

  function openAction(item: GitAction) {
    setMenuOpen(false);
    setDialogValue(item.value ?? "");
    setDialog(item);
  }

  async function submitAction(event: Event) {
    event.preventDefault();
    const item = dialog();
    if (!item) return;
    const value = dialogValue().trim();
    if (item.hint && !value) return;
    const completed = await action(item.label, () =>
      item.operation === "branch_create"
        ? api.branch(props.path, value)
        : api.gitOperation(
            props.path,
            item.operation,
            value,
            !!item.destructive,
          ),
    );
    if (completed) setDialog(undefined);
  }

  const staged = () => snapshot()?.files.filter((file) => file.staged) ?? [];
  const unstaged = () => snapshot()?.files.filter((file) => file.changed) ?? [];
  const diffParts = () => splitDiff(diff());
  const isConflict = (file: GitFile) =>
    ["DD", "AU", "UD", "UA", "DU", "AA", "UU"].includes(file.status);

  async function openConflict() {
    const file = selected()?.file;
    if (!file) return;
    try {
      const details = await api.conflict(props.path, file.path);
      setConflict(details);
      setResolution(details.ours ?? details.theirs ?? "");
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function openConflictExternal() {
    const file = selected()?.file;
    if (!file) return;
    try {
      await api.openConflictExternal(props.path, file.path);
      props.onNotice(
        `Opened ${file.path} in its default app. Save your edits, then stage the file to mark the conflict resolved.`,
      );
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function resolveConflict(event: Event) {
    event.preventDefault();
    const current = conflict();
    if (!current) return;
    const completed = await action("Resolve conflict", () =>
      api.resolveConflict(props.path, current.file, resolution()),
    );
    if (completed) {
      setConflict(undefined);
      setSelected(undefined);
    }
  }

  return (
    <div class="git-view">
      <div class="page-heading git-heading">
        <div>
          <span class="eyebrow">VERSION CONTROL</span>
          <h1>{snapshot()?.name ?? "Repository"}</h1>
          <p class="muted path-label">{props.path}</p>
        </div>
        <div class="toolbar-actions">
          <button
            class="ghost-button"
            onClick={() => props.onOpenTerminal(props.path)}
          >
            Open terminal
          </button>
          <button
            class="icon-button"
            title="Refresh repository"
            onClick={refresh}
          >
            <RefreshCw size={17} />
          </button>
          <button
            class="icon-button"
            disabled={busy() || !undoInfo().undo}
            title={
              undoInfo().undo ? `Undo ${undoInfo().undo}` : "Nothing to undo"
            }
            aria-label="Undo last Git action"
            onClick={() =>
              action(`Undo ${undoInfo().undo}`, () => api.gitUndo(props.path))
            }
          >
            <Undo2 size={17} />
          </button>
          <button
            class="icon-button"
            disabled={busy() || !undoInfo().redo}
            title={
              undoInfo().redo ? `Redo ${undoInfo().redo}` : "Nothing to redo"
            }
            aria-label="Redo Git action"
            onClick={() =>
              action(`Redo ${undoInfo().redo}`, () => api.gitRedo(props.path))
            }
          >
            <Redo2 size={17} />
          </button>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => action("Fetch", () => api.fetch(props.path))}
          >
            <ArrowDown size={16} /> Fetch
          </button>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => action("Pull", () => api.pull(props.path))}
          >
            <ArrowDown size={16} /> Pull
            <Show when={(snapshot()?.behind ?? 0) > 0}>
              <span class="count-badge">{snapshot()?.behind}</span>
            </Show>
          </button>
          <button
            class="primary-button"
            disabled={busy()}
            onClick={() => action("Push", () => api.push(props.path))}
          >
            <ArrowUp size={16} /> Push
            <Show when={(snapshot()?.ahead ?? 0) > 0}>
              <span class="count-badge">{snapshot()?.ahead}</span>
            </Show>
          </button>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() =>
              action("Publish branch", () => api.publish(props.path))
            }
          >
            Publish branch
          </button>
          <button class="ghost-button" onClick={() => setMenuOpen(!menuOpen())}>
            More actions <ChevronDown size={14} />
          </button>
          <Show when={menuOpen()}>
            <div class="git-actions-menu panel">
              <button
                onClick={() => {
                  setMenuOpen(false);
                  void openIdentity();
                }}
              >
                Commit identity…
              </button>
              <button
                onClick={() => {
                  setMenuOpen(false);
                  void openWorktrees();
                }}
              >
                Manage worktrees…
              </button>
              <button
                disabled={busy()}
                onClick={() => {
                  setMenuOpen(false);
                  void action("Pull with rebase", () =>
                    api.pull(props.path, "rebase"),
                  );
                }}
              >
                Pull with rebase
              </button>
              <button
                disabled={busy()}
                onClick={() => {
                  setMenuOpen(false);
                  void action("Pull with merge", () =>
                    api.pull(props.path, "merge"),
                  );
                }}
              >
                Pull with a merge commit
              </button>
              <button
                onClick={() => {
                  setMenuOpen(false);
                  setCleanupOpen(true);
                }}
              >
                Delete several branches…
              </button>
              <button
                onClick={() => {
                  setMenuOpen(false);
                  const order = graphOrder() === "date" ? "topo" : "date";
                  void api
                    .gitGraphOrderSet(props.path, order)
                    .then(() => {
                      setGraphOrder(order);
                      return refresh();
                    })
                    .catch((error) => props.onError(errorMessage(error)));
                }}
              >
                {graphOrder() === "date"
                  ? "Group history by branch"
                  : "Order history by date"}
              </button>
              <button
                onClick={() => {
                  setMenuOpen(false);
                  void revealItemInDir(props.path).catch((error) =>
                    props.onError(errorMessage(error)),
                  );
                }}
              >
                Show in {fileManager}
              </button>
              <For each={actions}>
                {(item) => (
                  <button onClick={() => openAction(item)}>{item.label}</button>
                )}
              </For>
            </div>
          </Show>
        </div>
      </div>
      <div class="git-content">
        <aside class="git-branches panel">
          <div class="panel-title">
            <span>BRANCHES</span>
            <button
              class="mini-button"
              aria-label="Create branch"
              onClick={() =>
                openAction({
                  operation: "branch_create",
                  label: "Create branch",
                  hint: "Branch name",
                })
              }
            >
              <Plus size={16} />
            </button>
          </div>
          <button
            class="branch-current"
            onClick={() => setBranchMenu(!branchMenu())}
          >
            <GitBranch size={16} />
            <span>{snapshot()?.branch ?? "Loading"}</span>
            <ChevronDown size={14} />
          </button>
          <Show when={branchMenu()}>
            <div class="branch-popover">
              <For each={snapshot()?.branches}>
                {(branch) => (
                  <button
                    class="branch-choice"
                    onClick={() => {
                      setBranchMenu(false);
                      if (branch !== snapshot()?.branch)
                        void action("Checkout", () =>
                          api.checkout(props.path, branch),
                        );
                    }}
                  >
                    {branch}
                  </button>
                )}
              </For>
              <button
                class="branch-choice accent"
                onClick={() => {
                  setBranchMenu(false);
                  openAction({
                    operation: "branch_create",
                    label: "Create branch",
                    hint: "Branch name",
                  });
                }}
              >
                <Plus size={14} /> New branch
              </button>
            </div>
          </Show>
          <div class="panel-title section-gap">
            <span>REMOTES</span>
            <button
              class="mini-button"
              title="Add remote"
              onClick={() =>
                openAction({
                  operation: "remote_add",
                  label: "Add remote",
                  hint: "Remote name and URL",
                })
              }
            >
              <Plus size={16} />
            </button>
          </div>
          <For
            each={snapshot()?.remoteUrls}
            fallback={
              <div class="remote-row">
                <CircleDot size={15} /> No remote configured
              </div>
            }
          >
            {(remote) => (
              <div class="remote-entry">
                <div class="remote-row">
                  <CircleDot size={15} /> {remote.name}
                </div>
                <p class="muted remote-url">{remote.url}</p>
              </div>
            )}
          </For>
          <Show when={(snapshot()?.remoteBranches.length ?? 0) > 0}>
            <div class="panel-title section-gap">
              <span>REMOTE BRANCHES</span>
            </div>
            <For each={snapshot()?.remoteBranches}>
              {(branch) => (
                <button
                  class="remote-branch"
                  title={`Track ${branch}`}
                  disabled={busy()}
                  onClick={() =>
                    void action("Track branch", () =>
                      api.checkoutRemote(props.path, branch),
                    )
                  }
                >
                  <GitBranch size={13} /> <span class="truncate">{branch}</span>
                </button>
              )}
            </For>
          </Show>
          <div class="branch-footer">
            <span>{snapshot()?.ahead ?? 0} ahead</span>
            <span>{snapshot()?.behind ?? 0} behind</span>
          </div>
          <div class="panel-title section-gap">
            <span>STASHES</span>
            <button
              class="mini-button"
              title="Stash changes"
              onClick={() =>
                openAction({ operation: "stash", label: "Stash changes" })
              }
            >
              <Plus size={16} />
            </button>
          </div>
          <For each={snapshot()?.stashes}>
            {(stash) => (
              <div class="git-compact-row">
                <span class="truncate">{stash.split("\x1f")[1] ?? stash}</span>
                <button
                  class="mini-button"
                  title="Apply stash"
                  onClick={() =>
                    void action("Apply stash", () =>
                      api.gitOperation(
                        props.path,
                        "stash_apply",
                        stash.split("\x1f")[0],
                      ),
                    )
                  }
                >
                  Apply
                </button>
                <button
                  class="mini-button"
                  title="Drop stash"
                  onClick={() =>
                    openAction({
                      operation: "stash_drop",
                      label: "Drop stash",
                      value: stash.split("\x1f")[0],
                      destructive: true,
                    })
                  }
                >
                  <X size={13} />
                </button>
              </div>
            )}
          </For>
          <div class="panel-title section-gap">
            <span>TAGS</span>
            <button
              class="mini-button"
              title="Create tag"
              onClick={() =>
                openAction({
                  operation: "tag_create",
                  label: "Create tag",
                  hint: "Tag name",
                })
              }
            >
              <Plus size={16} />
            </button>
          </div>
          <For each={snapshot()?.tags}>
            {(tag) => (
              <div class="git-compact-row">
                <span class="truncate">{tag}</span>
                <button
                  class="mini-button"
                  title="Delete tag"
                  onClick={() =>
                    openAction({
                      operation: "tag_delete",
                      label: "Delete tag",
                      value: tag,
                      destructive: true,
                    })
                  }
                >
                  <X size={13} />
                </button>
              </div>
            )}
          </For>
        </aside>

        <div class="git-center">
          <section class="panel changes-panel">
            <div class="panel-title">
              <span>WORKING CHANGES</span>
              <button
                class={
                  folderView() ? "mini-button active-toggle" : "mini-button"
                }
                title="Group changes by folder"
                aria-pressed={folderView()}
                onClick={() => setFolderView(!folderView())}
              >
                Folders
              </button>
              <button
                class="mini-button"
                title="Stage all changes"
                onClick={() =>
                  void action("Stage all", () =>
                    api.gitOperation(props.path, "stage_all"),
                  )
                }
              >
                Stage all
              </button>
              <span class="count-pill">{snapshot()?.files.length ?? 0}</span>
            </div>
            <Show
              when={(snapshot()?.files.length ?? 0) > 0}
              fallback={
                <div class="empty-small">
                  <Check size={20} /> Working tree clean
                </div>
              }
            >
              <div class="change-group">
                <div class="group-heading">
                  Unstaged <span>{unstaged().length}</span>
                </div>
                <For each={withFolders(unstaged(), folderView())}>
                  {(item) =>
                    item.kind === "folder" ? (
                      <div class="change-folder">
                        <span class="truncate">{item.folder}/</span>
                        <span class="muted">{item.count}</span>
                        <button
                          class="mini-button"
                          title="Stage folder"
                          onClick={() =>
                            void action("Stage folder", () =>
                              api.stage(props.path, item.folder),
                            )
                          }
                        >
                          <Plus size={15} />
                        </button>
                      </div>
                    ) : (
                      ((file: GitFile) => (
                        <div
                          class={`change-row ${selected()?.file.path === file.path && !selected()?.staged ? "selected" : ""}`}
                          onClick={() => {
                            setOpenCommit(undefined);
                            setSelected({ file, staged: false });
                          }}
                        >
                          <span class="file-state">
                            {file.status.trim() || "M"}
                          </span>
                          <span class="truncate">{file.path}</span>
                          <button
                            title={
                              file.status === "??"
                                ? file.path.endsWith("/")
                                  ? "Delete untracked folder"
                                  : "Delete untracked file"
                                : "Discard changes"
                            }
                            class="mini-button"
                            onClick={(event) => {
                              event.stopPropagation();
                              openAction({
                                operation: "discard",
                                label:
                                  file.status === "??"
                                    ? file.path.endsWith("/")
                                      ? "Delete untracked folder"
                                      : "Delete untracked file"
                                    : "Discard file changes",
                                value: file.path,
                                destructive: true,
                              });
                            }}
                          >
                            <X size={14} />
                          </button>
                          <button
                            title="Stage file"
                            class="mini-button"
                            onClick={(event) => {
                              event.stopPropagation();
                              void action("Stage", () =>
                                api.stage(props.path, file.path),
                              );
                            }}
                          >
                            <Plus size={15} />
                          </button>
                        </div>
                      ))(item.file)
                    )
                  }
                </For>
              </div>
              <div class="change-group">
                <div class="group-heading">
                  Staged <span>{staged().length}</span>
                </div>
                <For each={withFolders(staged(), folderView())}>
                  {(item) =>
                    item.kind === "folder" ? (
                      <div class="change-folder">
                        <span class="truncate">{item.folder}/</span>
                        <span class="muted">{item.count}</span>
                        <button
                          class="mini-button"
                          title="Unstage folder"
                          onClick={() =>
                            void action("Unstage folder", () =>
                              api.unstage(props.path, item.folder),
                            )
                          }
                        >
                          <Minus size={14} />
                        </button>
                      </div>
                    ) : (
                      ((file: GitFile) => (
                        <div
                          class={`change-row ${selected()?.file.path === file.path && selected()?.staged ? "selected" : ""}`}
                          onClick={() => {
                            setOpenCommit(undefined);
                            setSelected({ file, staged: true });
                          }}
                        >
                          <span class="file-state staged">
                            {file.status.trim() || "M"}
                          </span>
                          <span class="truncate">{file.path}</span>
                          <button
                            title="Unstage file"
                            class="mini-button"
                            onClick={(event) => {
                              event.stopPropagation();
                              void action("Unstage", () =>
                                api.unstage(props.path, file.path),
                              );
                            }}
                          >
                            −
                          </button>
                        </div>
                      ))(item.file)
                    )
                  }
                </For>
              </div>
            </Show>
            <div class="commit-box">
              <textarea
                value={message()}
                onInput={(event) => setMessage(event.currentTarget.value)}
                placeholder="Describe your changes…"
                aria-label="Commit message"
              />
              <Show when={askKey()}>
                <form class="ai-key-form" onSubmit={saveKey}>
                  <input
                    type="password"
                    aria-label="Anthropic API key"
                    placeholder={`Anthropic API key (sk-ant-…), saved in ${keychain}`}
                    value={aiKey()}
                    onInput={(event) => setAiKey(event.currentTarget.value)}
                  />
                  <button class="mini-button" type="submit">
                    Save
                  </button>
                  <button
                    class="mini-button"
                    type="button"
                    onClick={() => setAskKey(false)}
                  >
                    Cancel
                  </button>
                </form>
              </Show>
              <div class="commit-actions">
                <button
                  class="mini-button"
                  title="Write a message for the staged changes with Claude, using your own API key"
                  disabled={writing() || staged().length === 0}
                  onClick={() => void writeMessage()}
                >
                  {writing() ? "Writing…" : "Write message"}
                </button>
                <label>
                  <input
                    type="checkbox"
                    checked={showCommand()}
                    onChange={(event) =>
                      setShowCommand(event.currentTarget.checked)
                    }
                  />{" "}
                  Show command
                </label>
                <button
                  class="primary-button"
                  disabled={
                    !message().trim() || staged().length === 0 || busy()
                  }
                  onClick={() =>
                    action("Commit", () =>
                      api
                        .commit(props.path, message())
                        .then(() => setMessage("")),
                    )
                  }
                >
                  <GitCommitHorizontal size={16} /> Commit
                </button>
              </div>
              <Show when={showCommand()}>
                <code class="command-preview">
                  git commit -m {JSON.stringify(message())}
                </code>
              </Show>
            </div>
            <div class="git-subactions">
              <button
                onClick={() =>
                  void action("Unstage all", () =>
                    api.gitOperation(props.path, "unstage_all"),
                  )
                }
              >
                Unstage all
              </button>
              <button
                onClick={() =>
                  void action("Amend commit", () =>
                    api.gitOperation(props.path, "amend"),
                  )
                }
              >
                Amend last commit
              </button>
              <button
                onClick={() =>
                  void action("Abort merge", () =>
                    api.gitOperation(props.path, "merge_abort"),
                  )
                }
              >
                Abort merge
              </button>
              <button
                onClick={() =>
                  void action("Continue rebase", () =>
                    api.gitOperation(props.path, "rebase_continue"),
                  )
                }
              >
                Continue rebase
              </button>
              <button
                onClick={() =>
                  void action("Abort rebase", () =>
                    api.gitOperation(props.path, "rebase_abort"),
                  )
                }
              >
                Abort rebase
              </button>
            </div>
          </section>
          <section class="panel history-panel">
            <div class="panel-title">
              <button
                class={
                  centerTab() === "history"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("history")}
              >
                HISTORY
              </button>
              <button
                class={
                  centerTab() === "compare"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("compare")}
              >
                COMPARE
              </button>
              <button
                class={
                  centerTab() === "online"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("online")}
              >
                ONLINE
              </button>
              <button
                class={
                  centerTab() === "jira"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("jira")}
              >
                JIRA
              </button>
              <button
                class={
                  centerTab() === "file"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("file")}
              >
                FILE HISTORY
              </button>
              <button
                class={
                  centerTab() === "lfs"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("lfs")}
              >
                LFS
              </button>
              <button
                class={
                  centerTab() === "tools"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("tools")}
              >
                TOOLS
              </button>
              <button
                class={
                  centerTab() === "rebase"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("rebase")}
              >
                REBASE
              </button>
              <button
                class={
                  centerTab() === "advanced"
                    ? "git-panel-tab active"
                    : "git-panel-tab"
                }
                onClick={() => setCenterTab("advanced")}
              >
                ADVANCED
              </button>
              <span class="spacer" />
              <Show when={centerTab() === "history"}>
                <span class="muted">
                  {searchActive()
                    ? `${searchResults().length} result${searchResults().length === 1 ? "" : "s"}`
                    : "Recent commits"}
                </span>
              </Show>
            </div>
            <Show
              when={centerTab() === "history"}
              fallback={
                <Show
                  when={centerTab() === "online"}
                  fallback={
                    <Show
                      when={centerTab() === "jira"}
                      fallback={
                        <Show
                          when={centerTab() === "file"}
                          fallback={
                            <Show
                              when={centerTab() === "lfs"}
                              fallback={
                                <Show
                                  when={centerTab() === "tools"}
                                  fallback={
                                    <Show
                                      when={centerTab() === "advanced"}
                                      fallback={
                                        <Show
                                          when={centerTab() === "compare"}
                                          fallback={
                                            <RebasePanel
                                              repository={props.path}
                                              onNotice={props.onNotice}
                                              onChanged={() => void refresh()}
                                            />
                                          }
                                        >
                                          <ComparePanel
                                            path={props.path}
                                            snapshot={snapshot()}
                                            onNotice={props.onNotice}
                                            onError={props.onError}
                                          />
                                        </Show>
                                      }
                                    >
                                      <GitAdvancedPanel
                                        repository={props.path}
                                        onNotice={props.onNotice}
                                        onChanged={() => void refresh()}
                                      />
                                    </Show>
                                  }
                                >
                                  <GitToolsPanel
                                    repository={props.path}
                                    onNotice={props.onNotice}
                                    onChanged={() => void refresh()}
                                  />
                                </Show>
                              }
                            >
                              <LfsPanel
                                repository={props.path}
                                onNotice={props.onNotice}
                                onChanged={() => void refresh()}
                              />
                            </Show>
                          }
                        >
                          <FileHistoryPanel
                            repository={props.path}
                            file={selected()?.file.path}
                          />
                        </Show>
                      }
                    >
                      <JiraPanel onNotice={props.onNotice} />
                    </Show>
                  }
                >
                  <Show
                    when={isExternalRemote(snapshot()?.remote ?? "")}
                    fallback={
                      <ForgePanel
                        path={props.path}
                        branch={snapshot()?.branch ?? ""}
                        branches={snapshot()?.branches ?? []}
                        onError={props.onError}
                        onNotice={props.onNotice}
                      />
                    }
                  >
                    <ExternalForgePanel
                      path={props.path}
                      branch={snapshot()?.branch ?? ""}
                      branches={snapshot()?.branches ?? []}
                      onNotice={props.onNotice}
                    />
                  </Show>
                </Show>
              }
            >
              <div class="history-filters">
                <div class="commit-search history-branch-filter">
                  <select
                    aria-label="Show commits from"
                    value={branchFilter()}
                    onChange={(event) =>
                      void filterHistory(event.currentTarget.value)
                    }
                  >
                    <option value="">All branches</option>
                    <For each={snapshot()?.branches ?? []}>
                      {(branch) => <option value={branch}>{branch}</option>}
                    </For>
                    <For each={snapshot()?.remoteBranches ?? []}>
                      {(branch) => <option value={branch}>{branch}</option>}
                    </For>
                  </select>
                </div>
                <form class="commit-search" onSubmit={searchCommits}>
                  <select
                    aria-label="Search commits by"
                    value={searchMode()}
                    onChange={(event) =>
                      setSearchMode(
                        event.currentTarget.value as
                          "message" | "author" | "content",
                      )
                    }
                  >
                    <option value="message">Message</option>
                    <option value="author">Author</option>
                    <option value="content">Changed content</option>
                  </select>
                  <input
                    aria-label="Commit search term"
                    value={searchQuery()}
                    onInput={(event) =>
                      setSearchQuery(event.currentTarget.value)
                    }
                    placeholder="Find commits"
                  />
                  <button
                    type="submit"
                    class="mini-button"
                    title="Search commits"
                    disabled={searchBusy() || !searchQuery().trim()}
                  >
                    <Search size={15} />
                  </button>
                  <Show when={searchActive()}>
                    <button
                      type="button"
                      class="mini-button"
                      title="Show recent commits"
                      onClick={clearSearch}
                    >
                      <X size={15} />
                    </button>
                  </Show>
                </form>
              </div>
              <Show
                when={visibleCommits().length}
                fallback={
                  <div class="empty-small">
                    {searchActive() ? "No matching commits" : "No commits yet"}
                  </div>
                }
              >
                <VirtualRows
                  items={visibleCommits()}
                  rowHeight={49}
                  onNearEnd={() => {
                    if (
                      !searchActive() &&
                      !branchFilter() &&
                      snapshot()?.commits.length === 300
                    )
                      void loadOlderCommits();
                  }}
                >
                  {(commit: Commit) => (
                    <div
                      class={`commit-row virtual-commit-row clickable ${openCommit() === commit.id ? "selected" : ""}`}
                      role="button"
                      tabIndex={0}
                      title="Show this commit"
                      onClick={() => setOpenCommit(commit.id)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          setOpenCommit(commit.id);
                        }
                      }}
                    >
                      <pre class="commit-graph">{commit.graph}</pre>
                      <div class="commit-main">
                        <div class="commit-subject">{commit.subject}</div>
                        <div class="commit-meta">
                          {commit.author} · {commit.relativeDate}
                        </div>
                      </div>
                      <Show when={commit.refs}>
                        <span class="ref-badge">
                          {commit.refs.split(",")[0]}
                        </span>
                      </Show>
                      <code>{commit.shortId}</code>
                    </div>
                  )}
                </VirtualRows>
              </Show>
              <Show
                when={
                  !searchActive() &&
                  snapshot()?.commits.length === 300 &&
                  !olderComplete()
                }
              >
                <button
                  class="mini-button"
                  type="button"
                  disabled={olderBusy()}
                  onClick={() => void loadOlderCommits()}
                >
                  {olderBusy()
                    ? "Loading older commits…"
                    : "Load older commits"}
                </button>
              </Show>
            </Show>
          </section>
        </div>
        <aside class="git-inspector panel">
          <Show when={openCommit()}>
            {(commit) => (
              <CommitDetail
                path={props.path}
                commit={commit()}
                busy={busy()}
                onClose={() => setOpenCommit(undefined)}
                onRun={action}
                onNotice={props.onNotice}
                onError={props.onError}
              />
            )}
          </Show>
          <Show when={!openCommit()}>
            <div class="panel-title">
              <span>{selected()?.staged ? "STAGED DIFF" : "DIFF"}</span>
              <DiffPrefsBar />
            </div>
            <Show
              when={selected()}
              fallback={
                <div class="inspector-empty">
                  <GitCommitHorizontal size={24} />
                  <p>Select a changed file to inspect its diff.</p>
                </div>
              }
            >
              <div class="inspector-file">{selected()?.file.path}</div>
              <Show when={selected() && isConflict(selected()!.file)}>
                <div class="git-tools-buttons">
                  <button
                    class="conflict-open"
                    onClick={() => void openConflict()}
                  >
                    Resolve text conflict
                  </button>
                  <button
                    class="mini-button"
                    onClick={() => void openConflictExternal()}
                  >
                    Open externally
                  </button>
                  <button
                    class="mini-button"
                    title="Resolve in the merge tool chosen under Advanced"
                    onClick={() => {
                      const file = selected()?.file.path;
                      if (file)
                        void api
                          .gitOpenMergetool(props.path, file)
                          .catch((error) => props.onError(errorMessage(error)));
                    }}
                  >
                    Merge tool
                  </button>
                </div>
              </Show>
              <Show when={selected() && !isConflict(selected()!.file)}>
                <div class="git-tools-buttons">
                  <button
                    class="mini-button"
                    title="Compare in the diff tool chosen under Advanced"
                    onClick={() => {
                      const current = selected();
                      if (current)
                        void api
                          .gitOpenDifftool(
                            props.path,
                            current.file.path,
                            current.staged,
                          )
                          .catch((error) => props.onError(errorMessage(error)));
                    }}
                  >
                    Open in diff tool
                  </button>
                </div>
              </Show>
              <Show when={selected() && isImage(selected()!.file.path)}>
                <ImageCompare
                  path={props.path}
                  file={selected()!.file.path}
                  before={
                    selected()!.file.status === "??" ||
                    (selected()!.staged &&
                      selected()!.file.status.startsWith("A"))
                      ? null
                      : selected()!.staged
                        ? "HEAD"
                        : ":"
                  }
                  after={
                    selected()!.staged
                      ? ":"
                      : selected()!.file.status.includes("D")
                        ? null
                        : ""
                  }
                />
              </Show>
              <div class={`diff-view ${diffPrefs.wrap() ? "wrap" : ""}`}>
                <Show
                  when={diffParts().hunks.length}
                  fallback={
                    <pre>
                      {diff() || "No textual diff available for this file."}
                    </pre>
                  }
                >
                  <pre>{diffParts().header}</pre>
                  <For each={diffParts().hunks}>
                    {(hunk, index) => {
                      const pairs = pairedWordDiffs(hunkLines(hunk));
                      const language = () =>
                        languageFor(selected()?.file.path ?? "");
                      return (
                        <div class="diff-hunk">
                          <div class="diff-hunk-bar">
                            <span>Section {index() + 1}</span>
                            <button
                              disabled={
                                busy() || selected()?.file.status === "??"
                              }
                              title={
                                selected()?.file.status === "??"
                                  ? "Stage this new file before staging individual sections"
                                  : undefined
                              }
                              onClick={() =>
                                selected() &&
                                void action(
                                  selected()!.staged
                                    ? "Unstage section"
                                    : "Stage section",
                                  () =>
                                    api.stageHunk(
                                      props.path,
                                      selected()!.file.path,
                                      index(),
                                      selected()!.staged,
                                    ),
                                )
                              }
                            >
                              {selected()?.staged
                                ? "Unstage section"
                                : "Stage section"}
                            </button>
                          </div>
                          <pre class="diff-section-heading">
                            {hunk.split("\n", 1)[0]}
                          </pre>
                          <Show
                            when={diffPrefs.mode() === "inline"}
                            fallback={
                              <div class="side-by-side">
                                <For each={splitRows(hunk)}>
                                  {(row) => (
                                    <div class="side-by-side-row">
                                      <div class={row.changed ? "removed" : ""}>
                                        <span>{row.leftNumber ?? ""}</span>
                                        <pre>
                                          <CodeText
                                            text={row.left}
                                            language={language()}
                                            segments={row.leftSegments}
                                          />
                                        </pre>
                                      </div>
                                      <div class={row.changed ? "added" : ""}>
                                        <span>{row.rightNumber ?? ""}</span>
                                        <pre>
                                          <CodeText
                                            text={row.right}
                                            language={language()}
                                            segments={row.rightSegments}
                                          />
                                        </pre>
                                      </div>
                                    </div>
                                  )}
                                </For>
                              </div>
                            }
                          >
                            <For each={hunkLines(hunk)}>
                              {(line, lineIndex) => (
                                <div
                                  class={`diff-line ${line.startsWith("+") ? "added" : line.startsWith("-") ? "removed" : ""}`}
                                >
                                  <pre>
                                    {line[0]}
                                    <CodeText
                                      text={line.slice(1)}
                                      language={language()}
                                      segments={pairs.get(lineIndex())}
                                    />
                                  </pre>
                                  <Show
                                    when={
                                      (line.startsWith("+") ||
                                        line.startsWith("-")) &&
                                      selected()?.file.status !== "??"
                                    }
                                  >
                                    <button
                                      disabled={busy()}
                                      title={
                                        selected()?.staged
                                          ? "Unstage this line"
                                          : "Stage this line"
                                      }
                                      onClick={() =>
                                        selected() &&
                                        void action(
                                          selected()!.staged
                                            ? "Unstage line"
                                            : "Stage line",
                                          () =>
                                            api.stageLine(
                                              props.path,
                                              selected()!.file.path,
                                              index(),
                                              lineIndex(),
                                              selected()!.staged,
                                            ),
                                        )
                                      }
                                    >
                                      {selected()?.staged ? "Unstage" : "Stage"}
                                    </button>
                                  </Show>
                                </div>
                              )}
                            </For>
                          </Show>
                        </div>
                      );
                    }}
                  </For>
                </Show>
              </div>
            </Show>
          </Show>
        </aside>
      </div>
      <Show when={cleanupOpen()}>
        <BranchCleanup
          path={props.path}
          branches={snapshot()?.branches ?? []}
          current={snapshot()?.branch ?? ""}
          onClose={() => setCleanupOpen(false)}
          onDeleted={(report) => {
            if (report.deleted.length)
              props.onNotice(
                `Deleted ${report.deleted.length} branch${report.deleted.length === 1 ? "" : "es"}.`,
              );
            void refresh();
          }}
          onError={props.onError}
        />
      </Show>
      <Show when={worktreeOpen()}>
        <div class="overlay" onClick={() => setWorktreeOpen(false)}>
          <div
            class="dialog profile-dialog worktree-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Manage Git worktrees"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">GIT WORKTREES</span>
                <h2>Work on another branch</h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                onClick={() => setWorktreeOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <p class="muted">
              Each worktree has its own files and branch, while sharing this
              repository’s history.
            </p>
            <div class="worktree-list">
              <For each={worktrees()}>
                {(item) => (
                  <div class="worktree-row">
                    <div>
                      <strong>{item.branch}</strong>
                      <span class="muted">{item.path}</span>
                    </div>
                    <button
                      class="mini-button"
                      disabled={worktreeBusy()}
                      onClick={() => {
                        setWorktreeOpen(false);
                        props.onOpenRepository(item.path);
                      }}
                    >
                      Open
                    </button>
                    <button
                      class="mini-button"
                      disabled={worktreeBusy()}
                      title="Open this worktree and a terminal in its folder"
                      onClick={() => {
                        setWorktreeOpen(false);
                        props.onOpenRepository(item.path);
                        props.onOpenTerminal(item.path);
                      }}
                    >
                      Open with terminal
                    </button>
                    <button
                      class="mini-button"
                      disabled={worktreeBusy()}
                      title="Open a terminal in this worktree and start your coding agent"
                      onClick={async () => {
                        const saved = (() => {
                          try {
                            return localStorage.getItem("weave-agent-command");
                          } catch {
                            return null;
                          }
                        })();
                        const command = await askText(
                          "Agent command to run in this worktree (for example claude, codex or aider):",
                          saved ?? "claude",
                        );
                        if (!command?.trim()) return;
                        try {
                          localStorage.setItem(
                            "weave-agent-command",
                            command.trim(),
                          );
                        } catch {
                          // Remembering the command is only a convenience.
                        }
                        setWorktreeOpen(false);
                        props.onOpenTerminal(item.path, command.trim());
                      }}
                    >
                      Start agent
                    </button>
                    <Show when={!item.main}>
                      <button
                        class="mini-button"
                        disabled={worktreeBusy()}
                        title={
                          item.locked
                            ? "Allow this worktree to be pruned or removed"
                            : "Protect this worktree from pruning or removal"
                        }
                        onClick={() => void toggleWorktreeLock(item)}
                      >
                        {item.locked ? "Unlock" : "Lock"}
                      </button>
                      <button
                        class="mini-button danger-text"
                        disabled={worktreeBusy() || item.locked}
                        title={
                          item.locked
                            ? "This worktree is locked"
                            : "Remove worktree"
                        }
                        onClick={() => void removeWorktree(item)}
                      >
                        Remove
                      </button>
                    </Show>
                  </div>
                )}
              </For>
            </div>
            <form onSubmit={createWorktree}>
              <div class="form-grid">
                <label class="wide">
                  Branch
                  <select
                    value={worktreeNewBranch() ? "new" : "existing"}
                    onChange={(event) => {
                      setWorktreeNewBranch(event.currentTarget.value === "new");
                      setWorktreeBranch("");
                    }}
                  >
                    <option value="new">Create a new branch</option>
                    <option
                      value="existing"
                      disabled={!availableWorktreeBranches().length}
                    >
                      Use an existing branch
                    </option>
                  </select>
                </label>
                <label class="wide">
                  {worktreeNewBranch() ? "New branch name" : "Existing branch"}
                  <Show
                    when={worktreeNewBranch()}
                    fallback={
                      <select
                        required
                        value={worktreeBranch()}
                        onChange={(event) =>
                          setWorktreeBranch(event.currentTarget.value)
                        }
                      >
                        <option value="">Choose a branch</option>
                        <For each={availableWorktreeBranches()}>
                          {(branch) => <option value={branch}>{branch}</option>}
                        </For>
                      </select>
                    }
                  >
                    <input
                      required
                      value={worktreeBranch()}
                      onInput={(event) =>
                        setWorktreeBranch(event.currentTarget.value)
                      }
                      onBlur={() => {
                        if (!worktreeFolder())
                          setWorktreeFolder(
                            worktreeBranch().replaceAll("/", "-"),
                          );
                      }}
                      placeholder="feature/new-work"
                    />
                  </Show>
                </label>
                <label class="wide">
                  Parent folder
                  <div class="worktree-folder-picker">
                    <input value={worktreeParent()} readOnly />
                    <button
                      type="button"
                      class="ghost-button"
                      onClick={() => void chooseWorktreeParent()}
                    >
                      Choose…
                    </button>
                  </div>
                </label>
                <label class="wide">
                  New folder name
                  <input
                    required
                    value={worktreeFolder()}
                    onInput={(event) =>
                      setWorktreeFolder(event.currentTarget.value)
                    }
                    placeholder="new-work"
                  />
                </label>
              </div>
              <div class="dialog-actions">
                <span class="spacer" />
                <button
                  type="button"
                  class="ghost-button"
                  onClick={() => setWorktreeOpen(false)}
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  class="primary-button"
                  disabled={worktreeBusy()}
                >
                  Create worktree
                </button>
              </div>
            </form>
          </div>
        </div>
      </Show>
      <Show when={identityOpen()}>
        <div class="overlay" onClick={() => setIdentityOpen(false)}>
          <div
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Commit identity"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">GIT IDENTITY</span>
                <h2>Commit identity</h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                onClick={() => setIdentityOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <p class="muted">
              This name and email appear on commits. A repository identity takes
              precedence over your global identity.
            </p>
            <form onSubmit={saveIdentity}>
              <div class="form-grid">
                <label class="wide">
                  Save for
                  <select
                    value={identityScope()}
                    onChange={(event) =>
                      chooseIdentityScope(
                        event.currentTarget.value as "local" | "global",
                      )
                    }
                  >
                    <option value="local">This repository only</option>
                    <option value="global">All repositories by default</option>
                  </select>
                </label>
                <label class="wide">
                  Commit name
                  <input
                    autofocus
                    required
                    maxLength={100}
                    value={identityName()}
                    onInput={(event) =>
                      setIdentityName(event.currentTarget.value)
                    }
                    placeholder="Your name"
                  />
                </label>
                <label class="wide">
                  Commit email
                  <input
                    type="email"
                    required
                    maxLength={254}
                    value={identityEmail()}
                    onInput={(event) =>
                      setIdentityEmail(event.currentTarget.value)
                    }
                    placeholder="you@example.com"
                  />
                </label>
              </div>
              <div class="dialog-actions">
                <span class="spacer" />
                <button
                  type="button"
                  class="ghost-button"
                  onClick={() => setIdentityOpen(false)}
                >
                  Cancel
                </button>
                <button type="submit" class="primary-button" disabled={busy()}>
                  Save identity
                </button>
              </div>
            </form>
          </div>
        </div>
      </Show>
      <Show when={dialog()}>
        {(item) => (
          <div class="overlay" onClick={() => setDialog(undefined)}>
            <div
              class="dialog profile-dialog"
              role="dialog"
              aria-modal="true"
              aria-label={item().label}
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">GIT ACTION</span>
                  <h2>{item().label}</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setDialog(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <Show when={item().destructive}>
                <p class="muted">
                  {item().operation === "discard" &&
                  item().label.startsWith("Delete")
                    ? "This permanently deletes the untracked item and any files inside it. Git cannot recover them."
                    : "This action may remove changes or references. Check the target before continuing."}
                </p>
              </Show>
              <form onSubmit={submitAction}>
                <Show when={item().hint || item().value}>
                  <label class="git-action-field">
                    {item().hint ?? "Target"}
                    <input
                      autofocus
                      required={!!item().hint}
                      value={dialogValue()}
                      onInput={(event) =>
                        setDialogValue(event.currentTarget.value)
                      }
                      placeholder={item().hint}
                    />
                  </label>
                </Show>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => setDialog(undefined)}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    class={
                      item().destructive
                        ? "ghost-button danger-text"
                        : "primary-button"
                    }
                    disabled={busy()}
                  >
                    {item().label}
                  </button>
                </div>
              </form>
            </div>
          </div>
        )}
      </Show>
      <Show when={conflict()}>
        {(item) => (
          <div class="overlay" onClick={() => setConflict(undefined)}>
            <div
              class="dialog conflict-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Resolve merge conflict"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">MERGE CONFLICT</span>
                  <h2>{item().file}</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setConflict(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <p class="muted">
                Compare both versions, edit the result, then mark the file
                resolved.
              </p>
              <div class="conflict-versions">
                <div>
                  <strong>Base</strong>
                  <pre>{item().base ?? "No base version"}</pre>
                </div>
                <div>
                  <strong>Ours</strong>
                  <pre>{item().ours ?? "File absent on our side"}</pre>
                  <button
                    class="ghost-button"
                    disabled={item().ours === null}
                    onClick={() => setResolution(item().ours ?? "")}
                  >
                    Use ours
                  </button>
                </div>
                <div>
                  <strong>Theirs</strong>
                  <pre>{item().theirs ?? "File absent on their side"}</pre>
                  <button
                    class="ghost-button"
                    disabled={item().theirs === null}
                    onClick={() => setResolution(item().theirs ?? "")}
                  >
                    Use theirs
                  </button>
                </div>
              </div>
              <form onSubmit={resolveConflict}>
                <label class="conflict-result">
                  Result
                  <textarea
                    value={resolution()}
                    onInput={(event) =>
                      setResolution(event.currentTarget.value)
                    }
                  />
                </label>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => setConflict(undefined)}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    class="primary-button"
                    disabled={busy()}
                  >
                    Save and mark resolved
                  </button>
                </div>
              </form>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
}
