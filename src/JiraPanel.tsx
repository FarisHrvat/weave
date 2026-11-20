import { confirmAction } from "./dialogs";
import { keychain } from "./platform";
import { createSignal, For, onMount, Show } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { api, errorMessage } from "./api";
import type { JiraInfo, JiraIssue, JiraTransition } from "./types";

type Props = { onNotice: (message: string) => void };

export default function JiraPanel(props: Props) {
  const [info, setInfo] = createSignal<JiraInfo | null>(null);
  const [site, setSite] = createSignal("");
  const [project, setProject] = createSignal("");
  const [email, setEmail] = createSignal("");
  const isCloud = () =>
    site()
      .trim()
      .replace(/^https:\/\//, "")
      .split("/")[0]
      .endsWith(".atlassian.net");
  const [token, setToken] = createSignal("");
  const [clientId, setClientId] = createSignal("");
  const [clientSecret, setClientSecret] = createSignal("");
  const [issues, setIssues] = createSignal<JiraIssue[]>([]);
  const [nextPageToken, setNextPageToken] = createSignal<string | null>(null);
  const [summary, setSummary] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [issueType, setIssueType] = createSignal("Task");
  const [creating, setCreating] = createSignal(false);
  const [transitions, setTransitions] = createSignal<{
    key: string;
    values: JiraTransition[];
  }>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function loadInfo() {
    setError("");
    try {
      const saved = await api.jiraInfo();
      setInfo(saved);
      if (saved) {
        setSite(saved.site);
        setProject(saved.project);
        setEmail(saved.email);
        if (saved.connected) await loadIssues();
      }
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  onMount(() => void loadInfo());

  async function loadIssues(continuation: string | null = null) {
    setBusy(true);
    setError("");
    try {
      const page = await api.jiraList(continuation);
      setIssues(continuation ? [...issues(), ...page.issues] : page.issues);
      setNextPageToken(page.nextPageToken);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function signIn(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await invoke("jira_oauth_connect", {
        site: site(),
        project: project(),
        clientId: clientId(),
        clientSecret: clientSecret(),
      });
      setClientSecret("");
      await loadInfo();
      props.onNotice("Jira connected.");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function connect(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.jiraConnect(site(), project(), email(), token());
      setToken("");
      await loadInfo();
      props.onNotice("Jira connected.");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    if (!(await confirmAction(`Disconnect Jira project ${info()?.project}?`)))
      return;
    setBusy(true);
    try {
      await api.jiraDisconnect();
      setInfo(null);
      setIssues([]);
      setNextPageToken(null);
      props.onNotice("Jira disconnected.");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function createIssue(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const issue = await api.jiraCreate(summary(), description(), issueType());
      setCreating(false);
      setSummary("");
      setDescription("");
      await loadIssues();
      props.onNotice(`${issue.key} created.`);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function showTransitions(key: string) {
    setError("");
    try {
      setTransitions({ key, values: await api.jiraTransitions(key) });
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function transition(key: string, id: string) {
    setBusy(true);
    setError("");
    try {
      await api.jiraTransition(key, id);
      setTransitions(undefined);
      await loadIssues();
      props.onNotice(`${key} updated.`);
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
          <span class="eyebrow">ISSUES</span>
          <h2>Jira</h2>
          <Show when={info()?.connected}>
            <p class="muted">
              {info()?.project} · {info()?.site}
            </p>
          </Show>
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
      <Show when={!info()?.connected}>
        <form class="external-online-form" onSubmit={connect}>
          <p class="muted">
            Connect a Jira Cloud or Jira Data Center project. Your token is
            stored in {keychain}.
          </p>
          <label>
            Jira address
            <input
              required
              placeholder="team.atlassian.net or jira.example.com"
              value={site()}
              onInput={(event) => setSite(event.currentTarget.value)}
            />
          </label>
          <label>
            Project key
            <input
              required
              placeholder="OPS"
              value={project()}
              onInput={(event) => setProject(event.currentTarget.value)}
            />
          </label>
          <Show when={isCloud()}>
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
          <label>
            {isCloud() ? "API token" : "Personal access token"}
            <input
              type="password"
              required
              value={token()}
              onInput={(event) => setToken(event.currentTarget.value)}
            />
          </label>
          <button class="primary-button" type="submit" disabled={busy()}>
            Connect Jira
          </button>
        </form>
        <Show when={isCloud()}>
          <details class="oauth-sign-in">
            <summary>Or sign in with your browser</summary>
            <p class="muted">
              In the Atlassian developer console, create an OAuth 2.0
              integration, add the Jira API scopes read:jira-work,
              write:jira-work and read:jira-user, set the callback URL to
              http://localhost:53682/, then copy its client ID and secret. The
              address and project key above are used too.
            </p>
            <form class="external-online-form" onSubmit={signIn}>
              <label>
                Client ID
                <input
                  required
                  spellcheck={false}
                  value={clientId()}
                  onInput={(event) => setClientId(event.currentTarget.value)}
                />
              </label>
              <label>
                Secret
                <input
                  type="password"
                  required
                  value={clientSecret()}
                  onInput={(event) =>
                    setClientSecret(event.currentTarget.value)
                  }
                />
              </label>
              <button
                class="primary-button"
                type="submit"
                disabled={busy() || !project().trim()}
              >
                {busy() ? "Waiting for the browser…" : "Sign in"}
              </button>
            </form>
          </details>
        </Show>
      </Show>
      <Show when={info()?.connected}>
        <div class="external-online-actions">
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => void loadIssues()}
          >
            Refresh
          </button>
          <button class="ghost-button" onClick={() => setCreating(!creating())}>
            New issue
          </button>
        </div>
        <Show when={creating()}>
          <form class="external-online-form" onSubmit={createIssue}>
            <label>
              Summary
              <input
                required
                value={summary()}
                onInput={(event) => setSummary(event.currentTarget.value)}
              />
            </label>
            <label>
              Type
              <select
                value={issueType()}
                onChange={(event) => setIssueType(event.currentTarget.value)}
              >
                <option>Task</option>
                <option>Bug</option>
                <option>Story</option>
              </select>
            </label>
            <label>
              Description
              <textarea
                value={description()}
                onInput={(event) => setDescription(event.currentTarget.value)}
              />
            </label>
            <button class="primary-button" type="submit" disabled={busy()}>
              Create issue
            </button>
          </form>
        </Show>
        <div class="external-online-list">
          <For
            each={issues()}
            fallback={
              <p class="muted">{busy() ? "Loading…" : "No issues found."}</p>
            }
          >
            {(issue) => (
              <div class="external-online-item">
                <div>
                  <strong>
                    {issue.key} · {issue.summary}
                  </strong>
                  <p class="muted">
                    {issue.issueType} · {issue.status}
                  </p>
                </div>
                <div class="external-online-actions">
                  <button
                    class="ghost-button"
                    onClick={() => void openUrl(issue.url)}
                  >
                    Open
                  </button>
                  <button
                    class="ghost-button"
                    onClick={() => void showTransitions(issue.key)}
                  >
                    Change status
                  </button>
                </div>
                <Show when={transitions()?.key === issue.key}>
                  <div class="external-online-transitions">
                    <For each={transitions()?.values}>
                      {(item) => (
                        <button
                          class="ghost-button"
                          disabled={busy()}
                          onClick={() => void transition(issue.key, item.id)}
                        >
                          {item.name}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
              </div>
            )}
          </For>
        </div>
        <Show when={nextPageToken()}>
          <button
            class="ghost-button"
            disabled={busy()}
            onClick={() => void loadIssues(nextPageToken())}
          >
            Load more
          </button>
        </Show>
      </Show>
    </section>
  );
}
