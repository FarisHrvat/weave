import { createEffect, createSignal, For, Show } from "solid-js";
import { keychain } from "./platform";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, ExternalLink, Plus, RefreshCw, X } from "lucide-solid";
import { api, errorMessage } from "./api";
import { mergeReviewThreads } from "./reviewThreads";
import { reviewDiffLines } from "./reviewDiff";
import type {
  ForgeComment,
  ForgeInfo,
  ForgeItem,
  ForgeReviewThread,
  ForgeReviewFile,
} from "./types";

type Kind = "reviews" | "issues" | "ci";
type Props = {
  path: string;
  branch: string;
  branches: string[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
};

export default function ForgePanel(props: Props) {
  const [info, setInfo] = createSignal<ForgeInfo>();
  const [unavailable, setUnavailable] = createSignal("");
  const [kind, setKind] = createSignal<Kind>("reviews");
  const [state, setState] = createSignal<"open" | "all">("open");
  const [items, setItems] = createSignal<ForgeItem[]>([]);
  const [listError, setListError] = createSignal("");
  const [page, setPage] = createSignal(0);
  const [hasMore, setHasMore] = createSignal(false);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [tokenOpen, setTokenOpen] = createSignal(false);
  const [token, setToken] = createSignal("");
  const [customProvider, setCustomProvider] = createSignal<"GitHub" | "GitLab">(
    "GitLab",
  );
  const [createOpen, setCreateOpen] = createSignal(false);
  const [title, setTitle] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [source, setSource] = createSignal("");
  const [target, setTarget] = createSignal("");
  const [mergeItem, setMergeItem] = createSignal<ForgeItem>();
  const [commentItem, setCommentItem] = createSignal<ForgeItem>();
  const [commentKind, setCommentKind] = createSignal<"reviews" | "issues">(
    "reviews",
  );
  const [comments, setComments] = createSignal<ForgeComment[]>([]);
  const [commentPage, setCommentPage] = createSignal(0);
  const [commentHasMore, setCommentHasMore] = createSignal(false);
  const [commentBody, setCommentBody] = createSignal("");
  const [commentBusy, setCommentBusy] = createSignal(false);
  const [commentError, setCommentError] = createSignal("");
  const [discussionTab, setDiscussionTab] = createSignal<"timeline" | "code">(
    "timeline",
  );
  const [reviewThreads, setReviewThreads] = createSignal<ForgeReviewThread[]>(
    [],
  );
  const [reviewPage, setReviewPage] = createSignal(0);
  const [reviewHasMore, setReviewHasMore] = createSignal(false);
  const [reviewBusy, setReviewBusy] = createSignal(false);
  const [reviewError, setReviewError] = createSignal("");
  const [reviewFiles, setReviewFiles] = createSignal<ForgeReviewFile[]>([]);
  const [reviewFilesPage, setReviewFilesPage] = createSignal(0);
  const [reviewFilesMore, setReviewFilesMore] = createSignal(false);
  const [reviewFilesBusy, setReviewFilesBusy] = createSignal(false);
  const [newReviewPath, setNewReviewPath] = createSignal("");
  const [newReviewLine, setNewReviewLine] = createSignal(1);
  const [newReviewOtherLine, setNewReviewOtherLine] = createSignal<
    number | null
  >(null);
  const [newReviewSide, setNewReviewSide] = createSignal<"LEFT" | "RIGHT">(
    "RIGHT",
  );
  const [newReviewBody, setNewReviewBody] = createSignal("");
  const [replyThread, setReplyThread] = createSignal("");
  const [replyBody, setReplyBody] = createSignal("");
  let requestId = 0;
  let discussionRequestId = 0;
  let reviewFilesRequestId = 0;

  async function loadInfo() {
    const request = ++requestId;
    setInfo(undefined);
    setItems([]);
    setListError("");
    setPage(0);
    setHasMore(false);
    setBusy(false);
    setLoadingMore(false);
    setUnavailable("");
    try {
      const account = await api.forgeInfo(props.path);
      if (request !== requestId) return;
      setInfo(account);
      if (account.connected) await loadItems(kind());
    } catch (error) {
      if (request === requestId) setUnavailable(errorMessage(error));
    }
  }

  async function loadItems(
    nextKind: Kind,
    nextPage = 1,
    nextState: "open" | "all" = state(),
  ) {
    const request = ++requestId;
    setKind(nextKind);
    setState(nextState);
    setListError("");
    if (nextPage === 1) {
      setItems([]);
      setPage(0);
      setHasMore(false);
      setBusy(true);
    } else {
      setLoadingMore(true);
    }
    try {
      const result = await api.forgeList(
        props.path,
        nextKind,
        nextPage,
        nextState,
      );
      if (request === requestId) {
        setItems(nextPage === 1 ? result.items : [...items(), ...result.items]);
        setPage(nextPage);
        setHasMore(result.hasMore);
      }
    } catch (error) {
      if (request === requestId) setListError(errorMessage(error));
    } finally {
      if (request === requestId) {
        setBusy(false);
        setLoadingMore(false);
      }
    }
  }

  createEffect(() => {
    props.path;
    discussionRequestId += 1;
    setCommentItem(undefined);
    void loadInfo();
  });

  function closeToken() {
    setToken("");
    setTokenOpen(false);
  }

  async function connect(event: Event) {
    event.preventDefault();
    setBusy(true);
    try {
      if (info()) {
        await api.forgeConnect(props.path, token());
      } else {
        await api.forgeConnectSelfHosted(props.path, customProvider(), token());
      }
      closeToken();
      props.onNotice(
        `${info()?.provider ?? customProvider()} account connected.`,
      );
      await loadInfo();
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    setBusy(true);
    try {
      await api.forgeDisconnect(props.path);
      setItems([]);
      setInfo((current) => current && { ...current, connected: false });
      props.onNotice(`${info()?.provider} account disconnected.`);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function create(event: Event) {
    event.preventDefault();
    const currentKind = kind();
    if (currentKind === "ci") return;
    setBusy(true);
    try {
      const created = await api.forgeCreate(
        props.path,
        currentKind,
        title(),
        description(),
        currentKind === "reviews" ? source() : undefined,
        currentKind === "reviews" ? target() : undefined,
      );
      setCreateOpen(false);
      setTitle("");
      setDescription("");
      props.onNotice(
        `${currentKind === "reviews" ? "Review request" : "Issue"} #${created.number} created.`,
      );
      await loadItems(currentKind);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function approve(item: ForgeItem) {
    setBusy(true);
    try {
      await api.forgeReview(props.path, item.number);
      props.onNotice(`Request #${item.number} approved.`);
      await loadItems("reviews");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function changeIssueState(item: ForgeItem) {
    const close = item.state !== "closed";
    setBusy(true);
    try {
      await api.forgeIssueState(props.path, item.number, close);
      props.onNotice(`Issue #${item.number} ${close ? "closed" : "reopened"}.`);
      await loadItems("issues");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function merge() {
    const selected = mergeItem();
    if (!selected) return;
    setBusy(true);
    try {
      await api.forgeMerge(props.path, selected.number);
      setMergeItem(undefined);
      props.onNotice(`Request #${selected.number} merged.`);
      await loadItems("reviews");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  function openItem(url: string) {
    const expectedHost = info()?.host;
    try {
      const target = new URL(url);
      if (target.protocol !== "https:" || target.hostname !== expectedHost) {
        props.onError(
          "The provider returned an unexpected link. Open the repository on its website instead.",
        );
        return;
      }
    } catch {
      props.onError("The provider returned an invalid link.");
      return;
    }
    void openUrl(url).catch((error) => props.onError(errorMessage(error)));
  }

  async function loadComments(
    item: ForgeItem,
    itemKind: "reviews" | "issues",
    nextPage = 1,
  ) {
    const repository = props.path;
    const request = ++discussionRequestId;
    setCommentBusy(true);
    setCommentError("");
    try {
      const result = await api.forgeComments(
        repository,
        itemKind,
        item.number,
        nextPage,
      );
      if (request !== discussionRequestId || repository !== props.path) return;
      setComments(
        nextPage === 1 ? result.items : [...comments(), ...result.items],
      );
      setCommentPage(nextPage);
      setCommentHasMore(result.hasMore);
    } catch (error) {
      if (request === discussionRequestId) setCommentError(errorMessage(error));
    } finally {
      if (request === discussionRequestId) setCommentBusy(false);
    }
  }

  async function loadThreads(item: ForgeItem, nextPage = 1) {
    const repository = props.path;
    const request = ++discussionRequestId;
    setReviewBusy(true);
    setReviewError("");
    try {
      const result = await api.forgeReviewThreads(
        repository,
        item.number,
        nextPage,
      );
      if (request !== discussionRequestId || repository !== props.path) return;
      setReviewThreads(
        nextPage === 1
          ? result.items
          : mergeReviewThreads(reviewThreads(), result.items),
      );
      setReviewPage(nextPage);
      setReviewHasMore(result.hasMore);
    } catch (error) {
      if (request === discussionRequestId) setReviewError(errorMessage(error));
    } finally {
      if (request === discussionRequestId) setReviewBusy(false);
    }
  }

  async function loadReviewFiles(item: ForgeItem, nextPage = 1) {
    const repository = props.path;
    const request = ++reviewFilesRequestId;
    setReviewFilesBusy(true);
    try {
      const result = await api.forgeReviewFiles(
        repository,
        item.number,
        nextPage,
      );
      if (
        request !== reviewFilesRequestId ||
        repository !== props.path ||
        commentItem()?.number !== item.number ||
        discussionTab() !== "code"
      )
        return;
      setReviewFiles(
        nextPage === 1 ? result.items : [...reviewFiles(), ...result.items],
      );
      setReviewFilesPage(nextPage);
      setReviewFilesMore(result.hasMore);
      if (nextPage === 1) setNewReviewPath(result.items[0]?.newPath ?? "");
    } catch (error) {
      if (request === reviewFilesRequestId && repository === props.path)
        setReviewError(errorMessage(error));
    } finally {
      if (request === reviewFilesRequestId) setReviewFilesBusy(false);
    }
  }

  async function createReviewThread(event: Event) {
    event.preventDefault();
    const item = commentItem();
    const file = reviewFiles().find(
      (candidate) => candidate.newPath === newReviewPath(),
    );
    if (!item || !file) return;
    const repository = props.path;
    setReviewBusy(true);
    setReviewError("");
    try {
      await api.forgeReviewCreate(
        repository,
        item.number,
        file.oldPath,
        file.newPath,
        newReviewLine(),
        newReviewSide(),
        newReviewOtherLine(),
        newReviewBody(),
      );
      if (
        repository !== props.path ||
        commentItem()?.number !== item.number ||
        discussionTab() !== "code"
      )
        return;
      setNewReviewBody("");
      props.onNotice(`Review thread created on #${item.number}.`);
      await loadThreads(item);
    } catch (error) {
      if (repository === props.path) setReviewError(errorMessage(error));
    } finally {
      if (repository === props.path) setReviewBusy(false);
    }
  }

  function closeDiscussion() {
    discussionRequestId += 1;
    reviewFilesRequestId += 1;
    setCommentItem(undefined);
    setCommentBusy(false);
    setReviewBusy(false);
    setReviewFilesBusy(false);
    setReplyThread("");
    setReplyBody("");
  }

  function openComments(item: ForgeItem) {
    discussionRequestId += 1;
    reviewFilesRequestId += 1;
    const itemKind = kind() === "issues" ? "issues" : "reviews";
    setCommentItem(item);
    setCommentKind(itemKind);
    setCommentBody("");
    setComments([]);
    setCommentPage(0);
    setCommentBusy(false);
    setReviewBusy(false);
    setReviewThreads([]);
    setReviewPage(0);
    setReviewHasMore(false);
    setReviewFiles([]);
    setReviewFilesPage(0);
    setReviewFilesMore(false);
    setNewReviewPath("");
    setNewReviewOtherLine(null);
    setNewReviewBody("");
    setReplyThread("");
    setDiscussionTab("timeline");
    void loadComments(item, itemKind);
  }

  function showDiscussionTab(tab: "timeline" | "code") {
    const item = commentItem();
    if (!item) return;
    discussionRequestId += 1;
    reviewFilesRequestId += 1;
    setCommentBusy(false);
    setReviewBusy(false);
    setDiscussionTab(tab);
    if (tab === "timeline") void loadComments(item, commentKind());
    else {
      void loadThreads(item);
      void loadReviewFiles(item);
    }
  }

  async function postComment(event: Event) {
    event.preventDefault();
    const item = commentItem();
    if (!item) return;
    const repository = props.path;
    setCommentBusy(true);
    setCommentError("");
    try {
      await api.forgeCommentCreate(
        repository,
        commentKind(),
        item.number,
        commentBody(),
      );
      if (
        repository !== props.path ||
        commentItem()?.number !== item.number ||
        discussionTab() !== "timeline"
      )
        return;
      setCommentBody("");
      props.onNotice(`Comment posted on #${item.number}.`);
      await loadComments(item, commentKind());
    } catch (error) {
      setCommentError(errorMessage(error));
    } finally {
      setCommentBusy(false);
    }
  }

  async function postReply(event: Event) {
    event.preventDefault();
    const item = commentItem();
    const thread = replyThread();
    if (!item || !thread) return;
    const repository = props.path;
    setReviewBusy(true);
    setReviewError("");
    try {
      await api.forgeReviewReply(repository, item.number, thread, replyBody());
      if (
        repository !== props.path ||
        commentItem()?.number !== item.number ||
        discussionTab() !== "code"
      )
        return;
      setReplyThread("");
      setReplyBody("");
      props.onNotice(`Reply posted on review #${item.number}.`);
      await loadThreads(item);
    } catch (error) {
      if (repository === props.path) setReviewError(errorMessage(error));
    } finally {
      if (repository === props.path) setReviewBusy(false);
    }
  }

  async function resolveThread(thread: ForgeReviewThread) {
    const item = commentItem();
    if (!item) return;
    const repository = props.path;
    setReviewBusy(true);
    setReviewError("");
    try {
      await api.forgeReviewResolve(
        repository,
        item.number,
        thread.providerId ?? thread.id,
        !thread.resolved,
      );
      if (repository !== props.path || commentItem()?.number !== item.number)
        return;
      props.onNotice(thread.resolved ? "Thread reopened." : "Thread resolved.");
      await loadThreads(item);
    } catch (error) {
      if (repository === props.path) setReviewError(errorMessage(error));
    } finally {
      if (repository === props.path) setReviewBusy(false);
    }
  }

  return (
    <>
      <Show when={unavailable()}>
        <div class="forge-message">
          {unavailable()}
          <Show
            when={
              unavailable().includes("self-hosted") ||
              unavailable().includes("not connected")
            }
          >
            <button
              class="mini-button forge-text-button"
              onClick={() => setTokenOpen(true)}
            >
              Connect self-hosted server
            </button>
          </Show>
        </div>
      </Show>
      <Show when={info()}>
        {(account) => (
          <>
            <div class="forge-account">
              <div>
                <strong>{account().provider}</strong>
                <span>
                  {account().host} · {account().project}
                </span>
              </div>
              <Show
                when={account().connected}
                fallback={
                  <button
                    class="primary-button"
                    onClick={() => setTokenOpen(true)}
                  >
                    Connect account
                  </button>
                }
              >
                <button
                  class="mini-button forge-text-button"
                  disabled={busy()}
                  onClick={() => setTokenOpen(true)}
                >
                  Update token
                </button>
                <button
                  class="mini-button forge-text-button"
                  disabled={busy()}
                  onClick={disconnect}
                >
                  Disconnect
                </button>
              </Show>
            </div>
            <Show
              when={account().connected}
              fallback={
                <p class="forge-message">
                  Connect with a personal access token to manage reviews,
                  issues, and CI for this repository. HTTPS Git operations use
                  the saved token; SSH operations use your SSH key or agent.
                </p>
              }
            >
              <div class="forge-tabs">
                <button
                  class={kind() === "reviews" ? "active" : ""}
                  onClick={() => void loadItems("reviews")}
                >
                  {account().provider === "GitHub"
                    ? "Pull requests"
                    : "Merge requests"}
                </button>
                <button
                  class={kind() === "issues" ? "active" : ""}
                  onClick={() => void loadItems("issues")}
                >
                  Issues
                </button>
                <button
                  class={kind() === "ci" ? "active" : ""}
                  onClick={() => void loadItems("ci")}
                >
                  CI
                </button>
                <span class="spacer" />
                <Show when={kind() !== "ci"}>
                  <select
                    class="forge-state-filter"
                    aria-label="Filter online items"
                    value={state()}
                    onChange={(event) =>
                      void loadItems(
                        kind(),
                        1,
                        event.currentTarget.value as "open" | "all",
                      )
                    }
                  >
                    <option value="open">Open</option>
                    <option value="all">All</option>
                  </select>
                </Show>
                <button
                  class="mini-button"
                  title="Refresh online items"
                  onClick={() => void loadItems(kind())}
                >
                  <RefreshCw size={14} />
                </button>
                <Show when={kind() !== "ci"}>
                  <button
                    class="mini-button"
                    title="Create new item"
                    onClick={() => {
                      setSource(props.branch);
                      setTarget(
                        props.branches.find(
                          (branch) =>
                            branch !== props.branch &&
                            /^(main|master|develop)$/.test(branch),
                        ) ??
                          props.branches.find(
                            (branch) => branch !== props.branch,
                          ) ??
                          "",
                      );
                      setCreateOpen(true);
                    }}
                  >
                    <Plus size={15} />
                  </button>
                </Show>
              </div>
              <div class="forge-list">
                <Show when={listError()}>
                  <div class="forge-message" role="alert">
                    {listError()}
                    <button
                      class="mini-button forge-text-button"
                      onClick={() => void loadItems(kind())}
                    >
                      Retry
                    </button>
                  </div>
                </Show>
                <Show
                  when={!busy()}
                  fallback={<div class="empty-small">Loading {kind()}…</div>}
                >
                  <For
                    each={items()}
                    fallback={
                      <div class="empty-small">
                        No {state() === "all" ? "" : "open "}
                        {kind() === "reviews" ? "requests" : kind()} found
                      </div>
                    }
                  >
                    {(item) => (
                      <div class="forge-item">
                        <button
                          class="forge-item-main"
                          onClick={() => openItem(item.url)}
                          title="Open in browser"
                        >
                          <span class="forge-item-title">
                            {item.title || `#${item.number}`}{" "}
                            <ExternalLink size={12} />
                          </span>
                          <span>
                            #{item.number} · {item.detail || item.state}
                            <Show when={state() === "all" && kind() !== "ci"}>
                              {" · "}
                              {item.state}
                            </Show>
                          </span>
                        </button>
                        <Show when={kind() !== "ci"}>
                          <button
                            class="mini-button forge-text-button"
                            title={`Read and post comments on #${item.number}`}
                            onClick={() => openComments(item)}
                          >
                            Discussion
                          </button>
                        </Show>
                        <Show
                          when={
                            kind() === "reviews" &&
                            (item.state === "open" || item.state === "opened")
                          }
                        >
                          <button
                            class="mini-button"
                            title={`Approve #${item.number}`}
                            disabled={busy()}
                            onClick={() => void approve(item)}
                          >
                            <Check size={14} />
                          </button>
                          <button
                            class="mini-button forge-text-button"
                            title={`Merge #${item.number}`}
                            disabled={busy()}
                            onClick={() => setMergeItem(item)}
                          >
                            Merge
                          </button>
                        </Show>
                        <Show when={kind() === "issues"}>
                          <button
                            class="mini-button forge-text-button"
                            disabled={busy()}
                            onClick={() => void changeIssueState(item)}
                          >
                            {item.state === "closed" ? "Reopen" : "Close"}
                          </button>
                        </Show>
                        <Show when={kind() === "ci"}>
                          <span class="forge-state">
                            {item.state || item.detail}
                          </span>
                        </Show>
                      </div>
                    )}
                  </For>
                </Show>
                <Show when={hasMore() && !busy()}>
                  <button
                    class="forge-load-more"
                    disabled={loadingMore()}
                    onClick={() => void loadItems(kind(), page() + 1)}
                  >
                    {loadingMore() ? "Loading…" : "Load more"}
                  </button>
                </Show>
              </div>
            </Show>
          </>
        )}
      </Show>
      <Show when={commentItem()}>
        {(item) => (
          <div class="overlay" onClick={closeDiscussion}>
            <div
              class="dialog profile-dialog forge-comments-dialog"
              role="dialog"
              aria-modal="true"
              aria-label={`Discussion for ${item().title}`}
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">DISCUSSION #{item().number}</span>
                  <h2>{item().title}</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close discussion"
                  onClick={closeDiscussion}
                >
                  <X size={19} />
                </button>
              </div>
              <Show when={commentKind() === "reviews"}>
                <div class="forge-tabs">
                  <button
                    class={discussionTab() === "timeline" ? "active" : ""}
                    onClick={() => showDiscussionTab("timeline")}
                  >
                    Timeline
                  </button>
                  <button
                    class={discussionTab() === "code" ? "active" : ""}
                    onClick={() => showDiscussionTab("code")}
                  >
                    Code review
                  </button>
                </div>
              </Show>
              <Show when={discussionTab() === "timeline"}>
                <div class="forge-comments-list">
                  <For
                    each={comments()}
                    fallback={
                      <p class="empty-small">
                        {commentBusy()
                          ? "Loading discussion…"
                          : "No comments yet."}
                      </p>
                    }
                  >
                    {(comment) => (
                      <div class="forge-comment">
                        <div>
                          <strong>{comment.author}</strong>
                          <span class="muted">{comment.createdAt}</span>
                        </div>
                        <p>{comment.body}</p>
                      </div>
                    )}
                  </For>
                  <Show when={commentHasMore()}>
                    <button
                      class="mini-button"
                      disabled={commentBusy()}
                      onClick={() =>
                        void loadComments(
                          item(),
                          commentKind(),
                          commentPage() + 1,
                        )
                      }
                    >
                      Load more comments
                    </button>
                  </Show>
                </div>
                <Show when={commentError()}>
                  <p class="tailnet-error" role="alert">
                    {commentError()}
                  </p>
                </Show>
                <form class="forge-comment-form" onSubmit={postComment}>
                  <textarea
                    aria-label="New discussion comment"
                    value={commentBody()}
                    onInput={(event) =>
                      setCommentBody(event.currentTarget.value)
                    }
                    placeholder="Write a comment…"
                    rows={4}
                    required
                  />
                  <div class="dialog-actions">
                    <span class="spacer" />
                    <button
                      class="primary-button"
                      type="submit"
                      disabled={commentBusy()}
                    >
                      Post comment
                    </button>
                  </div>
                </form>
              </Show>
              <Show when={discussionTab() === "code"}>
                <div class="forge-comments-list">
                  <For
                    each={reviewThreads()}
                    fallback={
                      <p class="empty-small">
                        {reviewBusy()
                          ? "Loading code review threads…"
                          : "No line-attached review threads on this page."}
                      </p>
                    }
                  >
                    {(thread) => (
                      <div class="forge-review-thread">
                        <div class="git-tools-heading">
                          <strong>
                            {thread.path || "Changed file"}
                            {thread.line ? `:${thread.line}` : ""}
                          </strong>
                          <span class="muted">
                            {thread.side === "LEFT" ? "Before" : "After"}
                            {thread.resolved ? " · Resolved" : ""}
                          </span>
                        </div>
                        <For each={thread.comments}>
                          {(comment) => (
                            <div class="forge-comment">
                              <div>
                                <strong>{comment.author}</strong>
                                <span class="muted">{comment.createdAt}</span>
                              </div>
                              <p>{comment.body}</p>
                            </div>
                          )}
                        </For>
                        <Show
                          when={
                            info()?.provider === "GitLab" || !!thread.providerId
                          }
                        >
                          <button
                            class="mini-button"
                            disabled={reviewBusy()}
                            onClick={() => void resolveThread(thread)}
                          >
                            {thread.resolved
                              ? "Reopen thread"
                              : "Resolve thread"}
                          </button>
                        </Show>
                        <Show
                          when={replyThread() === thread.id}
                          fallback={
                            <button
                              class="mini-button"
                              disabled={reviewBusy()}
                              onClick={() => {
                                setReplyThread(thread.id);
                                setReplyBody("");
                              }}
                            >
                              Reply
                            </button>
                          }
                        >
                          <form class="forge-comment-form" onSubmit={postReply}>
                            <textarea
                              aria-label={`Reply to review thread on ${thread.path}`}
                              value={replyBody()}
                              onInput={(event) =>
                                setReplyBody(event.currentTarget.value)
                              }
                              rows={3}
                              required
                            />
                            <div class="dialog-actions">
                              <button
                                type="button"
                                class="ghost-button"
                                onClick={() => setReplyThread("")}
                              >
                                Cancel
                              </button>
                              <button
                                class="primary-button"
                                type="submit"
                                disabled={reviewBusy()}
                              >
                                Post reply
                              </button>
                            </div>
                          </form>
                        </Show>
                      </div>
                    )}
                  </For>
                  <Show when={reviewHasMore()}>
                    <button
                      class="mini-button"
                      disabled={reviewBusy()}
                      onClick={() => void loadThreads(item(), reviewPage() + 1)}
                    >
                      Load more threads
                    </button>
                  </Show>
                </div>
                <Show when={reviewError()}>
                  <p class="tailnet-error" role="alert">
                    {reviewError()}
                  </p>
                </Show>
                <form class="forge-comment-form" onSubmit={createReviewThread}>
                  <strong>New line review thread</strong>
                  <p class="muted">
                    Choose a changed file and a line shown in this request’s
                    diff.
                  </p>
                  <div class="form-grid">
                    <label class="wide">
                      Changed file
                      <select
                        value={newReviewPath()}
                        onChange={(event) => {
                          setNewReviewPath(event.currentTarget.value);
                          setNewReviewOtherLine(null);
                        }}
                        required
                      >
                        <For each={reviewFiles()}>
                          {(file) => (
                            <option value={file.newPath}>{file.newPath}</option>
                          )}
                        </For>
                      </select>
                    </label>
                    <label>
                      Diff side
                      <select
                        value={newReviewSide()}
                        onChange={(event) => {
                          const next = event.currentTarget.value as
                            "LEFT" | "RIGHT";
                          const other = newReviewOtherLine();
                          if (other !== null && next !== newReviewSide()) {
                            const previous = newReviewLine();
                            setNewReviewLine(other);
                            setNewReviewOtherLine(previous);
                          }
                          setNewReviewSide(next);
                        }}
                      >
                        <option value="RIGHT">After</option>
                        <option value="LEFT">Before</option>
                      </select>
                    </label>
                    <label>
                      Line number
                      <input
                        type="number"
                        min="1"
                        max="1000000"
                        value={newReviewLine()}
                        onInput={(event) => {
                          setNewReviewLine(Number(event.currentTarget.value));
                          setNewReviewOtherLine(null);
                        }}
                        required
                      />
                    </label>
                  </div>
                  <Show
                    when={
                      reviewFiles().find(
                        (file) => file.newPath === newReviewPath(),
                      )?.patch
                    }
                  >
                    {(patch) => (
                      <div class="forge-review-diff" aria-label="Changed lines">
                        <For each={reviewDiffLines(patch())}>
                          {(line) => (
                            <button
                              type="button"
                              class={`forge-review-diff-line ${line.kind}`}
                              classList={{
                                selected:
                                  line.line === newReviewLine() &&
                                  line.side === newReviewSide(),
                              }}
                              disabled={line.line === null}
                              onClick={() => {
                                if (line.line === null || line.side === null)
                                  return;
                                setNewReviewLine(line.line);
                                setNewReviewSide(line.side);
                                setNewReviewOtherLine(
                                  line.kind === "context" ? line.before : null,
                                );
                              }}
                              title={
                                line.line
                                  ? `Comment on ${line.side === "LEFT" ? "before" : "after"} line ${line.line}`
                                  : undefined
                              }
                            >
                              <span>{line.before ?? ""}</span>
                              <span>{line.after ?? ""}</span>
                              <code>{line.text}</code>
                            </button>
                          )}
                        </For>
                      </div>
                    )}
                  </Show>
                  <Show
                    when={
                      reviewFiles().length > 0 &&
                      !reviewFiles().find(
                        (file) => file.newPath === newReviewPath(),
                      )?.patch
                    }
                  >
                    <p class="muted">
                      This file has no text diff preview. Enter a changed line
                      number manually.
                    </p>
                  </Show>
                  <Show when={reviewFilesMore()}>
                    <button
                      type="button"
                      class="mini-button"
                      disabled={reviewFilesBusy()}
                      onClick={() =>
                        void loadReviewFiles(item(), reviewFilesPage() + 1)
                      }
                    >
                      Load more changed files
                    </button>
                  </Show>
                  <textarea
                    aria-label="New line review comment"
                    value={newReviewBody()}
                    onInput={(event) =>
                      setNewReviewBody(event.currentTarget.value)
                    }
                    placeholder="Write a review comment…"
                    rows={3}
                    required
                  />
                  <div class="dialog-actions">
                    <span class="spacer" />
                    <button
                      class="primary-button"
                      type="submit"
                      disabled={
                        reviewBusy() || reviewFilesBusy() || !newReviewPath()
                      }
                    >
                      Create review thread
                    </button>
                  </div>
                </form>
              </Show>
            </div>
          </div>
        )}
      </Show>
      <Show when={tokenOpen()}>
        <div class="overlay" onClick={closeToken}>
          <div
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Connect provider account"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">ACCOUNT</span>
                <h2>Connect {info()?.provider ?? "self-hosted server"}</h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                onClick={closeToken}
              >
                <X size={19} />
              </button>
            </div>
            <p class="muted">
              Use a personal access token with access to this repository. Weave
              sends it only to the origin server for verification, then stores
              it in {keychain}.
            </p>
            <form onSubmit={connect}>
              <Show when={!info()}>
                <label class="git-action-field">
                  Server type
                  <select
                    value={customProvider()}
                    onChange={(event) =>
                      setCustomProvider(
                        event.currentTarget.value as "GitHub" | "GitLab",
                      )
                    }
                  >
                    <option value="GitHub">GitHub Enterprise Server</option>
                    <option value="GitLab">GitLab Self-Managed</option>
                  </select>
                </label>
              </Show>
              <label class="git-action-field">
                Personal access token
                <input
                  type="password"
                  autofocus
                  required
                  autocomplete="off"
                  value={token()}
                  onInput={(event) => setToken(event.currentTarget.value)}
                />
              </label>
              <div class="dialog-actions">
                <span class="spacer" />
                <button type="button" class="ghost-button" onClick={closeToken}>
                  Cancel
                </button>
                <button class="primary-button" type="submit" disabled={busy()}>
                  Connect
                </button>
              </div>
            </form>
          </div>
        </div>
      </Show>
      <Show when={createOpen()}>
        <div class="overlay" onClick={() => setCreateOpen(false)}>
          <div
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Create online item"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">{info()?.provider}</span>
                <h2>
                  Create{" "}
                  {kind() === "reviews"
                    ? info()?.provider === "GitHub"
                      ? "pull request"
                      : "merge request"
                    : "issue"}
                </h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                onClick={() => setCreateOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <form onSubmit={create} class="forge-form">
              <label>
                Title
                <input
                  autofocus
                  required
                  maxLength={255}
                  value={title()}
                  onInput={(event) => setTitle(event.currentTarget.value)}
                />
              </label>
              <Show when={kind() === "reviews"}>
                <div class="forge-branches">
                  <label>
                    From branch
                    <input
                      required
                      value={source()}
                      onInput={(event) => setSource(event.currentTarget.value)}
                    />
                  </label>
                  <label>
                    Into branch
                    <input
                      required
                      value={target()}
                      onInput={(event) => setTarget(event.currentTarget.value)}
                    />
                  </label>
                </div>
                <p class="muted">
                  Push the source branch before creating the request.
                </p>
              </Show>
              <label>
                Description
                <textarea
                  rows={5}
                  value={description()}
                  onInput={(event) => setDescription(event.currentTarget.value)}
                />
              </label>
              <div class="dialog-actions">
                <span class="spacer" />
                <button
                  type="button"
                  class="ghost-button"
                  onClick={() => setCreateOpen(false)}
                >
                  Cancel
                </button>
                <button class="primary-button" type="submit" disabled={busy()}>
                  Create
                </button>
              </div>
            </form>
          </div>
        </div>
      </Show>
      <Show when={mergeItem()}>
        {(item) => (
          <div class="overlay" onClick={() => setMergeItem(undefined)}>
            <div
              class="dialog profile-dialog"
              role="alertdialog"
              aria-modal="true"
              aria-label="Confirm merge"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">MERGE REQUEST</span>
                  <h2>Merge #{item().number}?</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setMergeItem(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <p class="muted">{item().title}</p>
              <div class="dialog-actions">
                <span class="spacer" />
                <button
                  class="ghost-button"
                  onClick={() => setMergeItem(undefined)}
                >
                  Cancel
                </button>
                <button
                  class="primary-button"
                  disabled={busy()}
                  onClick={() => void merge()}
                >
                  Merge request
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
