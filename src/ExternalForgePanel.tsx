import { confirmAction } from "./dialogs";
import { keychain } from "./platform";
import ExternalOAuthSignIn from "./ExternalOAuthSignIn";
import { createEffect, createSignal, For, Show } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage } from "./api";
import type {
  ExternalComment,
  ExternalInfo,
  ExternalItem,
  ExternalRepository,
} from "./types";

type Props = {
  path: string;
  branch: string;
  branches: string[];
  onNotice: (message: string) => void;
};

export default function ExternalForgePanel(props: Props) {
  const [info, setInfo] = createSignal<ExternalInfo>();
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [kind, setKind] = createSignal<"reviews" | "ci" | "repos">("reviews");
  const [reviewState, setReviewState] = createSignal("open");
  const [repositories, setRepositories] = createSignal<ExternalRepository[]>(
    [],
  );
  const [newRepoName, setNewRepoName] = createSignal("");
  const [newRepoPrivate, setNewRepoPrivate] = createSignal(true);
  const [openComments, setOpenComments] = createSignal<number>();
  const [comments, setComments] = createSignal<ExternalComment[]>([]);
  const [commentText, setCommentText] = createSignal("");
  const [replyTo, setReplyTo] = createSignal<ExternalComment>();
  const [items, setItems] = createSignal<ExternalItem[]>([]);
  const [page, setPage] = createSignal(0);
  const [more, setMore] = createSignal(false);
  const [email, setEmail] = createSignal("");
  const [token, setToken] = createSignal("");
  const [title, setTitle] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [source, setSource] = createSignal(props.branch);
  const [target, setTarget] = createSignal("main");
  const [creating, setCreating] = createSignal(false);
  let generation = 0;
  let listRequest = 0;

  async function refreshInfo() {
    const current = ++generation;
    ++listRequest;
    setInfo(undefined);
    setBusy(false);
    setError("");
    setItems([]);
    setPage(0);
    try {
      const next = await api.externalInfo(props.path);
      if (current !== generation) return;
      setInfo(next);
      if (next.connected) await refreshList("reviews", 1);
    } catch (cause) {
      if (current === generation) setError(errorMessage(cause));
    }
  }

  async function run(job: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await job();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const showRepositories = () =>
    run(async () => {
      setKind("repos");
      setRepositories(await api.externalRepositories(props.path));
    });

  const createRepository = (event: SubmitEvent) => {
    event.preventDefault();
    void run(async () => {
      const created = await api.externalCreateRepository(
        props.path,
        newRepoName(),
        newRepoPrivate(),
      );
      setNewRepoName("");
      setRepositories(await api.externalRepositories(props.path));
      props.onNotice(`Created ${created.name}.`);
    });
  };

  const approve = (item: ExternalItem) =>
    run(async () => {
      await api.externalApprove(props.path, item.number);
      props.onNotice(`Approved pull request #${item.number}.`);
    });

  const toggleComments = (item: ExternalItem) => {
    if (openComments() === item.number) {
      setOpenComments(undefined);
      return;
    }
    void run(async () => {
      setComments(await api.externalComments(props.path, item.number));
      setOpenComments(item.number);
      setCommentText("");
    });
  };

  const postComment = (event: SubmitEvent, item: ExternalItem) => {
    event.preventDefault();
    void run(async () => {
      const target = replyTo();
      await api.externalComment(
        props.path,
        item.number,
        commentText(),
        target ? { id: target.id, thread: target.thread } : null,
      );
      setCommentText("");
      setReplyTo(undefined);
      setComments(await api.externalComments(props.path, item.number));
    });
  };

  async function refreshList(
    nextKind: "reviews" | "ci" = kind() === "ci" ? "ci" : "reviews",
    nextPage = 1,
  ) {
    const currentGeneration = generation;
    const currentRequest = ++listRequest;
    setBusy(true);
    setError("");
    setKind(nextKind);
    setOpenComments(undefined);
    try {
      const result = await api.externalList(
        props.path,
        nextKind,
        nextPage,
        reviewState(),
      );
      if (currentGeneration !== generation || currentRequest !== listRequest)
        return;
      setItems(nextPage === 1 ? result.items : [...items(), ...result.items]);
      setPage(nextPage);
      setMore(result.hasMore);
    } catch (cause) {
      if (currentGeneration === generation && currentRequest === listRequest)
        setError(errorMessage(cause));
    } finally {
      if (currentGeneration === generation && currentRequest === listRequest)
        setBusy(false);
    }
  }

  createEffect(() => {
    props.path;
    void refreshInfo();
  });

  async function connect(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.externalConnect(props.path, email(), token());
      setToken("");
      await refreshInfo();
      props.onNotice(`${info()?.provider ?? "Provider"} connected.`);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    if (
      !(await confirmAction(
        `Disconnect ${info()?.provider} for ${info()?.organization}?`,
      ))
    )
      return;
    setBusy(true);
    try {
      await api.externalDisconnect(props.path);
      await refreshInfo();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function createReview(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.externalCreateReview(
        props.path,
        title(),
        description(),
        source(),
        target(),
      );
      setCreating(false);
      setTitle("");
      setDescription("");
      await refreshList("reviews", 1);
      props.onNotice("Pull request created.");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function merge(item: ExternalItem) {
    if (
      !(await confirmAction(
        `Merge pull request #${item.number}: ${item.title}?`,
      ))
    )
      return;
    setBusy(true);
    setError("");
    try {
      await api.externalMerge(props.path, item.number);
      await refreshList("reviews", 1);
      props.onNotice(`Pull request #${item.number} merged.`);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="external-online-panel">
      <div class="external-online-header">
        <div>
          <span class="eyebrow">ONLINE</span>
          <h2>{info()?.provider ?? "Online repository"}</h2>
          <p class="muted">
            {info()?.organization}/{info()?.repository}
          </p>
        </div>
        <Show when={info()?.connected}>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => void disconnect()}
          >
            Disconnect
          </button>
        </Show>
      </div>
      <Show when={error()}>
        <p class="error-banner" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={info() && !info()?.connected}>
        <form class="external-online-form" onSubmit={connect}>
          <p class="muted">
            Connect a {info()?.provider} token with repository and pull request
            access. The token is saved in {keychain}.
          </p>
          <Show when={info()?.provider === "Bitbucket Cloud"}>
            <label>
              Atlassian account email
              <input
                type="email"
                required
                value={email()}
                onInput={(event) => setEmail(event.currentTarget.value)}
              />
            </label>
          </Show>
          <Show when={info()?.provider === "Bitbucket Data Center"}>
            <label>
              Bitbucket username
              <input
                required
                autocomplete="username"
                value={email()}
                onInput={(event) => setEmail(event.currentTarget.value)}
              />
            </label>
          </Show>
          <label>
            {info()?.provider === "Azure DevOps"
              ? "Personal access token"
              : info()?.provider === "Bitbucket Data Center"
                ? "HTTP access token"
                : "API token"}
            <input
              type="password"
              required
              value={token()}
              onInput={(event) => setToken(event.currentTarget.value)}
            />
          </label>
          <button class="primary-button" type="submit" disabled={busy()}>
            Connect
          </button>
        </form>
        <Show
          when={
            info()?.provider === "Bitbucket Cloud" ||
            (info()?.provider === "Azure DevOps" &&
              info()?.url.startsWith("https://dev.azure.com/"))
          }
        >
          <ExternalOAuthSignIn
            provider={info()!.provider as "Bitbucket Cloud" | "Azure DevOps"}
            path={props.path}
            onConnected={() => {
              void refreshInfo();
              props.onNotice(`${info()?.provider} connected.`);
            }}
          />
        </Show>
      </Show>
      <Show when={info()?.connected}>
        <div
          class="external-online-actions"
          role="tablist"
          aria-label="Online repository view"
        >
          <button
            class={kind() === "reviews" ? "active" : ""}
            onClick={() => void refreshList("reviews", 1)}
          >
            Pull requests
          </button>
          <button
            class={kind() === "ci" ? "active" : ""}
            onClick={() => void refreshList("ci", 1)}
          >
            Builds
          </button>
          <button
            class={kind() === "repos" ? "active" : ""}
            onClick={() => void showRepositories()}
          >
            Repositories
          </button>
          <button
            onClick={() =>
              kind() === "repos"
                ? void showRepositories()
                : void refreshList(kind() === "ci" ? "ci" : "reviews", 1)
            }
            disabled={busy()}
          >
            Refresh
          </button>
        </div>
        <Show when={kind() === "reviews"}>
          <div class="external-online-toolbar">
            <select
              aria-label="Pull request state"
              value={reviewState()}
              onChange={(event) => {
                setReviewState(event.currentTarget.value);
                void refreshList("reviews", 1);
              }}
            >
              <option value="open">Open</option>
              <option value="merged">Merged</option>
              <option value="declined">Declined or abandoned</option>
              <option value="all">All</option>
            </select>
            <button
              class="ghost-button"
              onClick={() => setCreating(!creating())}
            >
              New pull request
            </button>
          </div>
        </Show>
        <Show when={kind() === "repos"}>
          <form class="external-online-toolbar" onSubmit={createRepository}>
            <input
              aria-label="New repository name"
              placeholder="new-repository"
              required
              value={newRepoName()}
              onInput={(event) => setNewRepoName(event.currentTarget.value)}
            />
            <label class="git-tools-checkbox">
              <input
                type="checkbox"
                checked={newRepoPrivate()}
                onChange={(event) =>
                  setNewRepoPrivate(event.currentTarget.checked)
                }
              />
              Private
            </label>
            <button class="ghost-button" type="submit" disabled={busy()}>
              Create repository
            </button>
          </form>
          <div class="external-online-list">
            <For
              each={repositories()}
              fallback={
                <p class="muted">
                  {busy() ? "Loading…" : "No repositories found."}
                </p>
              }
            >
              {(repo) => (
                <div class="external-online-item">
                  <div>
                    <strong>{repo.name}</strong>
                    <p class="muted">
                      {repo.private ? "Private" : "Public"} · {repo.cloneUrl}
                    </p>
                  </div>
                  <div class="external-online-actions">
                    <Show when={repo.cloneUrl}>
                      <button
                        class="ghost-button"
                        onClick={() =>
                          void api
                            .copyLink(repo.cloneUrl)
                            .then(() =>
                              props.onNotice(
                                "Clone URL copied. Paste it in Clone a repository.",
                              ),
                            )
                            .catch((cause) => setError(errorMessage(cause)))
                        }
                      >
                        Copy clone URL
                      </button>
                    </Show>
                    <Show when={repo.webUrl}>
                      <button
                        class="ghost-button"
                        onClick={() => void openUrl(repo.webUrl)}
                      >
                        Open
                      </button>
                    </Show>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Show>
        <Show when={creating() && kind() === "reviews"}>
          <form class="external-online-form" onSubmit={createReview}>
            <label>
              Title
              <input
                required
                value={title()}
                onInput={(event) => setTitle(event.currentTarget.value)}
              />
            </label>
            <label>
              Description
              <textarea
                value={description()}
                onInput={(event) => setDescription(event.currentTarget.value)}
              />
            </label>
            <label>
              Source branch
              <input
                list="external-branches"
                required
                value={source()}
                onInput={(event) => setSource(event.currentTarget.value)}
              />
            </label>
            <label>
              Target branch
              <input
                list="external-branches"
                required
                value={target()}
                onInput={(event) => setTarget(event.currentTarget.value)}
              />
            </label>
            <datalist id="external-branches">
              <For each={props.branches}>
                {(branch) => <option value={branch} />}
              </For>
            </datalist>
            <button class="primary-button" type="submit" disabled={busy()}>
              Create pull request
            </button>
          </form>
        </Show>
        <div
          class="external-online-list"
          style={{ display: kind() === "repos" ? "none" : undefined }}
        >
          <For
            each={items()}
            fallback={
              <p class="muted">{busy() ? "Loading…" : "No items found."}</p>
            }
          >
            {(item) => (
              <div class="external-online-item">
                <div>
                  <strong>
                    {item.number > 0 ? `#${item.number} ` : ""}
                    {item.title}
                  </strong>
                  <p class="muted">
                    {item.state}
                    {item.detail ? ` · ${item.detail}` : ""}
                  </p>
                </div>
                <div class="external-online-actions">
                  <Show when={item.url}>
                    <button
                      class="ghost-button"
                      onClick={() => void openUrl(item.url)}
                    >
                      Open
                    </button>
                  </Show>
                  <Show
                    when={
                      (kind() === "reviews" &&
                        item.state.toLowerCase() === "open") ||
                      (kind() === "reviews" &&
                        item.state.toLowerCase() === "active")
                    }
                  >
                    <button
                      class="ghost-button"
                      disabled={busy()}
                      onClick={() => void approve(item)}
                    >
                      Approve
                    </button>
                    <button
                      class="ghost-button"
                      disabled={busy()}
                      onClick={() => void merge(item)}
                    >
                      Merge
                    </button>
                  </Show>
                  <Show when={kind() === "reviews"}>
                    <button
                      class="ghost-button"
                      disabled={busy()}
                      onClick={() => toggleComments(item)}
                    >
                      {openComments() === item.number
                        ? "Hide comments"
                        : "Comments"}
                    </button>
                  </Show>
                </div>
                <Show when={openComments() === item.number}>
                  <div class="external-comments">
                    <For
                      each={comments()}
                      fallback={<p class="muted">No comments yet.</p>}
                    >
                      {(comment) => (
                        <div class="external-comment">
                          <strong>{comment.author || "Unknown"}</strong>
                          <span class="muted">
                            {comment.location ? `${comment.location} · ` : ""}
                            {comment.created.slice(0, 10)}
                          </span>
                          <p>{comment.body}</p>
                          <Show when={comment.id > 0}>
                            <button
                              class="text-button external-reply"
                              onClick={() => setReplyTo(comment)}
                            >
                              Reply
                            </button>
                          </Show>
                        </div>
                      )}
                    </For>
                    <form
                      class="external-online-form"
                      onSubmit={(event) => postComment(event, item)}
                    >
                      <Show when={replyTo()}>
                        <p class="muted">
                          Replying to {replyTo()?.author || "comment"}{" "}
                          <button
                            type="button"
                            class="text-button"
                            onClick={() => setReplyTo(undefined)}
                          >
                            Cancel reply
                          </button>
                        </p>
                      </Show>
                      <textarea
                        aria-label="New comment"
                        required
                        placeholder={
                          replyTo() ? "Write a reply" : "Write a comment"
                        }
                        value={commentText()}
                        onInput={(event) =>
                          setCommentText(event.currentTarget.value)
                        }
                      />
                      <button
                        class="primary-button"
                        type="submit"
                        disabled={busy()}
                      >
                        Comment
                      </button>
                    </form>
                  </div>
                </Show>
              </div>
            )}
          </For>
        </div>
        <Show when={more() && kind() !== "repos"}>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() =>
              void refreshList(kind() === "ci" ? "ci" : "reviews", page() + 1)
            }
          >
            Load more
          </button>
        </Show>
      </Show>
    </section>
  );
}
