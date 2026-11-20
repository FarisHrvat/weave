import { askText } from "./dialogs";
import { createSignal, For, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorMessage } from "./api";
import type { Profile } from "./types";

type GiteaRepository = {
  fullName: string;
  description: string;
  private: boolean;
  cloneUrl: string;
  webUrl: string;
  defaultBranch: string;
};
type Collaborator = { login: string; permission: string };

function useBusy() {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const run = async (job: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await job();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

export function GiteaAdmin(props: {
  onClone: (url: string) => void;
  onNotify: (message: string) => void;
}) {
  const remembered = (() => {
    try {
      return localStorage.getItem("weave-gitea-server") ?? "";
    } catch {
      return "";
    }
  })();
  const [server, setServer] = createSignal(remembered);
  const [token, setToken] = createSignal("");
  const [login, setLogin] = createSignal("");
  const [repos, setRepos] = createSignal<GiteaRepository[]>();
  const [selected, setSelected] = createSignal<string>();
  const [collaborators, setCollaborators] = createSignal<Collaborator[]>([]);
  const [newUser, setNewUser] = createSignal("");
  const [newPermission, setNewPermission] = createSignal("write");
  const [newName, setNewName] = createSignal("");
  const [newOrg, setNewOrg] = createSignal("");
  const [newPrivate, setNewPrivate] = createSignal(true);
  const { busy, error, run } = useBusy();

  const load = () =>
    run(async () => {
      setRepos(
        await invoke<GiteaRepository[]>("gitea_repositories", {
          server: server(),
        }),
      );
      try {
        localStorage.setItem("weave-gitea-server", server().trim());
      } catch {
        // Remembering the server is only a convenience.
      }
    });

  const loadCollaborators = (fullName: string) =>
    run(async () => {
      setSelected(fullName);
      setCollaborators(
        await invoke<Collaborator[]>("gitea_collaborators", {
          server: server(),
          fullName,
        }),
      );
    });

  return (
    <div class="server-admin">
      <p class="muted">
        Manage repositories and access on a Gitea or Forgejo server with an
        access token (scopes: repository and organization read/write).
      </p>
      <form
        class="form-grid"
        onSubmit={(event) => {
          event.preventDefault();
          void run(async () => {
            if (token().trim())
              setLogin(
                await invoke<string>("gitea_connect", {
                  server: server(),
                  token: token(),
                }),
              );
            setToken("");
            await load();
          });
        }}
      >
        <label>
          Server
          <input
            required
            placeholder="https://git.example.com"
            value={server()}
            onInput={(event) => setServer(event.currentTarget.value)}
          />
        </label>
        <label>
          Access token
          <input
            type="password"
            placeholder={
              login() ? `Signed in as ${login()}` : "Leave empty if saved"
            }
            value={token()}
            onInput={(event) => setToken(event.currentTarget.value)}
          />
        </label>
        <div class="wide server-hooks-actions">
          <button class="server-action" type="submit" disabled={busy()}>
            Connect and list
          </button>
          <Show when={repos()}>
            <button
              type="button"
              class="server-action"
              onClick={() =>
                void run(async () => {
                  await invoke("gitea_disconnect", { server: server() });
                  setRepos(undefined);
                  setLogin("");
                })
              }
            >
              Forget token
            </button>
          </Show>
        </div>
      </form>
      <Show when={repos()}>
        <div class="connect-saved">
          <For
            each={repos()}
            fallback={<p class="muted">No repositories yet.</p>}
          >
            {(repo) => (
              <div class="server-repo">
                <div class="connect-saved-row">
                  <div class="server-repo-name">
                    <strong>{repo.fullName}</strong>
                    <span class="muted">
                      {repo.private ? "Private" : "Public"}
                      {repo.description ? ` · ${repo.description}` : ""}
                    </span>
                  </div>
                  <button
                    class="server-action"
                    disabled={!repo.cloneUrl}
                    onClick={() => props.onClone(repo.cloneUrl)}
                  >
                    Clone
                  </button>
                  <button
                    class="server-action"
                    onClick={() =>
                      selected() === repo.fullName
                        ? setSelected(undefined)
                        : void loadCollaborators(repo.fullName)
                    }
                  >
                    Access
                  </button>
                  <Show when={repo.webUrl}>
                    <button
                      class="server-action"
                      onClick={() => void openUrl(repo.webUrl)}
                    >
                      Open
                    </button>
                  </Show>
                  <button
                    class="server-action danger-text"
                    disabled={busy()}
                    onClick={async () => {
                      const typed = await askText(
                        `Permanently delete ${repo.fullName} and everything in it? Type ${repo.fullName} to confirm.`,
                      );
                      if (typed === null) return;
                      void run(async () => {
                        await invoke("gitea_delete", {
                          server: server(),
                          fullName: repo.fullName,
                          confirmation: typed,
                        });
                        props.onNotify(`Deleted ${repo.fullName}.`);
                        await load();
                      });
                    }}
                  >
                    Delete
                  </button>
                </div>
                <Show when={selected() === repo.fullName}>
                  <div class="server-repo-settings">
                    <For
                      each={collaborators()}
                      fallback={<p class="muted">Only the owner has access.</p>}
                    >
                      {(person) => (
                        <div class="connect-saved-row">
                          <span class="server-repo-name">
                            <strong>{person.login}</strong>
                          </span>
                          <select
                            aria-label={`Access for ${person.login}`}
                            value={person.permission}
                            onChange={(event) => {
                              const permission = event.currentTarget.value;
                              void run(async () => {
                                await invoke("gitea_collaborator_set", {
                                  server: server(),
                                  fullName: repo.fullName,
                                  user: person.login,
                                  permission,
                                });
                                await loadCollaborators(repo.fullName);
                              });
                            }}
                          >
                            <option value="read">Read</option>
                            <option value="write">Write</option>
                            <option value="admin">Admin</option>
                            <option value="remove">Remove access</option>
                          </select>
                        </div>
                      )}
                    </For>
                    <form
                      class="connect-saved-row"
                      onSubmit={(event) => {
                        event.preventDefault();
                        void run(async () => {
                          await invoke("gitea_collaborator_set", {
                            server: server(),
                            fullName: repo.fullName,
                            user: newUser(),
                            permission: newPermission(),
                          });
                          setNewUser("");
                          await loadCollaborators(repo.fullName);
                        });
                      }}
                    >
                      <input
                        aria-label="User to add"
                        placeholder="User name"
                        required
                        value={newUser()}
                        onInput={(event) =>
                          setNewUser(event.currentTarget.value)
                        }
                      />
                      <select
                        aria-label="Permission for the new user"
                        value={newPermission()}
                        onChange={(event) =>
                          setNewPermission(event.currentTarget.value)
                        }
                      >
                        <option value="read">Read</option>
                        <option value="write">Write</option>
                        <option value="admin">Admin</option>
                      </select>
                      <button class="server-action" type="submit">
                        Add
                      </button>
                    </form>
                  </div>
                </Show>
              </div>
            )}
          </For>
        </div>
        <h3 class="server-repos-heading">New repository</h3>
        <form
          class="form-grid"
          onSubmit={(event) => {
            event.preventDefault();
            void run(async () => {
              const created = await invoke<GiteaRepository>("gitea_create", {
                server: server(),
                organization: newOrg(),
                name: newName(),
                description: "",
                private: newPrivate(),
              });
              props.onNotify(`Created ${created.fullName}.`);
              setNewName("");
              await load();
            });
          }}
        >
          <label>
            Name
            <input
              required
              value={newName()}
              onInput={(event) => setNewName(event.currentTarget.value)}
            />
          </label>
          <label>
            Organization (optional)
            <input
              value={newOrg()}
              onInput={(event) => setNewOrg(event.currentTarget.value)}
            />
          </label>
          <label class="wide git-tools-checkbox">
            <input
              type="checkbox"
              checked={newPrivate()}
              onChange={(event) => setNewPrivate(event.currentTarget.checked)}
            />
            Private
          </label>
          <div class="wide">
            <button class="primary-button" type="submit" disabled={busy()}>
              Create repository
            </button>
          </div>
        </form>
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}

export function GitoliteAdmin(props: {
  profiles: Profile[];
  onNotify: (message: string) => void;
}) {
  const sshProfiles = () =>
    props.profiles.filter((profile) => profile.protocol === "ssh");
  const [profileId, setProfileId] = createSignal(sshProfiles()[0]?.id ?? "");
  const [user, setUser] = createSignal("git");
  const [config, setConfig] = createSignal<string>();
  const [message, setMessage] = createSignal("");
  const { busy, error, run } = useBusy();
  const profile = () => props.profiles.find((item) => item.id === profileId());
  const adminUrl = () => {
    const host = profile();
    return host
      ? `ssh://${user().trim()}@${host.host}:${host.port}/gitolite-admin`
      : "";
  };

  return (
    <div class="server-admin">
      <p class="muted">
        Edit Gitolite's access rules (conf/gitolite.conf). Weave clones
        gitolite-admin, saves your change as a commit and pushes it; Gitolite
        applies it immediately. Your key must have admin rights.
      </p>
      <div class="form-grid">
        <label>
          Server
          <select
            value={profileId()}
            onChange={(event) => {
              setProfileId(event.currentTarget.value);
              setConfig(undefined);
            }}
          >
            <For each={sshProfiles()}>
              {(item) => <option value={item.id}>{item.name}</option>}
            </For>
          </select>
        </label>
        <label>
          Gitolite user
          <input
            value={user()}
            onInput={(event) => setUser(event.currentTarget.value)}
          />
        </label>
        <div class="wide">
          <button
            class="server-action"
            disabled={busy() || !adminUrl()}
            onClick={() =>
              void run(async () => {
                setConfig(
                  await invoke<string>("gitolite_config", {
                    adminUrl: adminUrl(),
                    keyPath: profile()?.keyPath ?? null,
                  }),
                );
              })
            }
          >
            Load access rules
          </button>
        </div>
      </div>
      <Show when={config() !== undefined}>
        <textarea
          class="gitolite-config"
          aria-label="gitolite.conf"
          spellcheck={false}
          value={config()}
          onInput={(event) => setConfig(event.currentTarget.value)}
        />
        <div class="connect-saved-row">
          <input
            aria-label="Change description"
            placeholder="What changed (optional)"
            value={message()}
            onInput={(event) => setMessage(event.currentTarget.value)}
          />
          <button
            class="primary-button"
            disabled={busy()}
            onClick={() =>
              void run(async () => {
                props.onNotify(
                  await invoke<string>("gitolite_config_save", {
                    adminUrl: adminUrl(),
                    keyPath: profile()?.keyPath ?? null,
                    config: config(),
                    message: message(),
                  }),
                );
                setMessage("");
              })
            }
          >
            Save and push
          </button>
        </div>
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
