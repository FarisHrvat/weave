import { askText } from "./dialogs";
import { ThisComputer, thisComputer } from "./platform";
import { createSignal, For, Show } from "solid-js";
import { X } from "lucide-solid";
import { api, errorMessage } from "./api";
import type { Profile, RemoteRepository } from "./types";
import { repositoryCloneUrl } from "./serverRepos";
import ServerHooks from "./ServerHooks";
import { GiteaAdmin, GitoliteAdmin } from "./ServerAdminPanels";

type Props = {
  profiles: Profile[];
  onClone: (url: string) => void;
  onNotify: (message: string, error?: boolean) => void;
  onClose: () => void;
};

export default function ServerReposDialog(props: Props) {
  const sshProfiles = () =>
    props.profiles.filter((profile) => profile.protocol === "ssh");
  const [profileId, setProfileId] = createSignal(sshProfiles()[0]?.id ?? "");
  const [folder, setFolder] = createSignal("~/git");
  const [repos, setRepos] = createSignal<RemoteRepository[]>();
  const [newName, setNewName] = createSignal("");
  const [newBranch, setNewBranch] = createSignal("main");
  const [newDescription, setNewDescription] = createSignal("");
  const [editing, setEditing] = createSignal<RemoteRepository>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [mode, setMode] = createSignal<"ssh" | "gitea" | "gitolite">("ssh");
  const profile = () =>
    props.profiles.find((item) => item.id === profileId()) ?? undefined;
  const target = () => profileId() || null;

  async function attempt(job: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await job();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  const load = () =>
    attempt(async () => {
      setRepos(await api.gitRemoteReposList(target(), folder().trim()));
    });

  function newPath() {
    const name = newName()
      .trim()
      .replace(/\.git$/, "");
    return `${folder().trim().replace(/\/+$/, "")}/${name}.git`;
  }

  const create = () =>
    attempt(async () => {
      if (
        !/^[A-Za-z0-9._+@-]+$/.test(
          newName()
            .trim()
            .replace(/\.git$/, ""),
        )
      )
        throw new Error(
          "Use letters, numbers, dots, dashes or underscores for the repository name.",
        );
      const url = await api.gitRemoteRepoCreate(
        target(),
        newPath(),
        newBranch().trim(),
        newDescription(),
      );
      props.onNotify(`Created ${url}`);
      setNewName("");
      setNewDescription("");
      setRepos(await api.gitRemoteReposList(target(), folder().trim()));
    });

  const saveSettings = (repo: RemoteRepository) =>
    attempt(async () => {
      await api.gitRemoteRepoConfigure(
        target(),
        repo.path,
        repo.defaultBranch,
        repo.description,
        repo.denyForcePush,
      );
      setEditing(undefined);
      props.onNotify("Repository settings saved.");
      setRepos(await api.gitRemoteReposList(target(), folder().trim()));
    });

  const remove = async (repo: RemoteRepository) => {
    const name = repo.path.split("/").pop() ?? repo.path;
    const typed = await askText(
      `Permanently delete ${repo.path} on ${profile()?.name ?? thisComputer}? Every branch and commit in it will be lost and cannot be recovered.\n\nType ${name} to confirm.`,
    );
    if (typed === null) return;
    void attempt(async () => {
      await api.gitRemoteRepoDelete(target(), repo.path, typed);
      props.onNotify(`Deleted ${repo.path}.`);
      setRepos(await api.gitRemoteReposList(target(), folder().trim()));
    });
  };

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class="dialog profile-dialog connect-dialog server-repos-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Server repositories"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">GIT OVER SSH</span>
            <h2>Server repositories</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>
        <div class="segmented connect-kinds" role="tablist">
          <button
            role="tab"
            aria-selected={mode() === "ssh"}
            class={mode() === "ssh" ? "active" : ""}
            onClick={() => setMode("ssh")}
          >
            Folders over SSH
          </button>
          <button
            role="tab"
            aria-selected={mode() === "gitea"}
            class={mode() === "gitea" ? "active" : ""}
            onClick={() => setMode("gitea")}
          >
            Gitea / Forgejo
          </button>
          <button
            role="tab"
            aria-selected={mode() === "gitolite"}
            class={mode() === "gitolite" ? "active" : ""}
            onClick={() => setMode("gitolite")}
          >
            Gitolite
          </button>
        </div>
        <Show when={mode() === "gitea"}>
          <GiteaAdmin
            onClone={(url) => {
              props.onClone(url);
              props.onClose();
            }}
            onNotify={(message) => props.onNotify(message)}
          />
        </Show>
        <Show when={mode() === "gitolite"}>
          <GitoliteAdmin
            profiles={props.profiles}
            onNotify={(message) => props.onNotify(message)}
          />
        </Show>
        <Show when={mode() === "ssh"}>
          <p class="muted">
            Create and manage bare repositories on a server you reach over SSH,
            such as a NAS or a VPS. The host needs Git installed and key or
            agent authentication.
          </p>
          <form
            class="form-grid"
            onSubmit={(event) => {
              event.preventDefault();
              void load();
            }}
          >
            <label>
              Server
              <select
                value={profileId()}
                onChange={(event) => {
                  setProfileId(event.currentTarget.value);
                  setRepos(undefined);
                }}
              >
                <For each={sshProfiles()}>
                  {(item) => (
                    <option value={item.id}>
                      {item.name} · {item.host}
                    </option>
                  )}
                </For>
                <option value="">{ThisComputer}</option>
              </select>
            </label>
            <label>
              Folder
              <span class="inline-field">
                <input
                  value={folder()}
                  onInput={(event) => setFolder(event.currentTarget.value)}
                  placeholder="~/git or /srv/git"
                />
                <button class="server-action" type="submit" disabled={busy()}>
                  List
                </button>
              </span>
            </label>
          </form>

          <Show when={repos()}>
            <div class="connect-saved">
              <For
                each={repos()}
                fallback={
                  <p class="muted">No bare repositories in this folder yet.</p>
                }
              >
                {(repo) => (
                  <div class="server-repo">
                    <div class="connect-saved-row">
                      <div class="server-repo-name">
                        <strong>{repo.path.split("/").pop()}</strong>
                        <span class="muted">
                          {repo.description || repo.path} ·{" "}
                          {repo.defaultBranch || "no default branch"}
                          {repo.denyForcePush ? " · force-push blocked" : ""}
                        </span>
                      </div>
                      <button
                        class="server-action"
                        onClick={() => {
                          props.onClone(
                            repositoryCloneUrl(profile(), repo.path),
                          );
                          props.onClose();
                        }}
                      >
                        Clone
                      </button>
                      <button
                        class="server-action"
                        onClick={() =>
                          setEditing(
                            editing()?.path === repo.path
                              ? undefined
                              : { ...repo },
                          )
                        }
                      >
                        Settings
                      </button>
                      <button
                        class="server-action danger-text"
                        disabled={busy()}
                        onClick={() => remove(repo)}
                      >
                        Delete
                      </button>
                    </div>
                    <Show when={editing()?.path === repo.path && editing()}>
                      {(draft) => (
                        <form
                          class="form-grid server-repo-settings"
                          onSubmit={(event) => {
                            event.preventDefault();
                            void saveSettings(draft());
                          }}
                        >
                          <label>
                            Default branch
                            <input
                              value={draft().defaultBranch}
                              onInput={(event) =>
                                setEditing({
                                  ...draft(),
                                  defaultBranch: event.currentTarget.value,
                                })
                              }
                            />
                          </label>
                          <label>
                            Description
                            <input
                              maxLength={300}
                              value={draft().description}
                              onInput={(event) =>
                                setEditing({
                                  ...draft(),
                                  description: event.currentTarget.value,
                                })
                              }
                            />
                          </label>
                          <label class="wide git-tools-checkbox">
                            <input
                              type="checkbox"
                              checked={draft().denyForcePush}
                              onChange={(event) =>
                                setEditing({
                                  ...draft(),
                                  denyForcePush: event.currentTarget.checked,
                                })
                              }
                            />
                            Block force pushes and branch deletion
                          </label>
                          <div class="wide">
                            <button
                              class="server-action"
                              type="submit"
                              disabled={busy()}
                            >
                              Save settings
                            </button>
                          </div>
                          <ServerHooks
                            profileId={target()}
                            path={repo.path}
                            onNotify={(message) => props.onNotify(message)}
                          />
                        </form>
                      )}
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>

          <h3 class="server-repos-heading">New repository</h3>
          <form
            class="form-grid"
            onSubmit={(event) => {
              event.preventDefault();
              void create();
            }}
          >
            <label>
              Name
              <input
                required
                value={newName()}
                onInput={(event) => setNewName(event.currentTarget.value)}
                placeholder="project"
              />
            </label>
            <label>
              Default branch
              <input
                required
                value={newBranch()}
                onInput={(event) => setNewBranch(event.currentTarget.value)}
              />
            </label>
            <label class="wide">
              Description (optional)
              <input
                maxLength={300}
                value={newDescription()}
                onInput={(event) =>
                  setNewDescription(event.currentTarget.value)
                }
              />
            </label>
            <p class="muted wide server-repo-preview">
              Creates{" "}
              {newName().trim()
                ? repositoryCloneUrl(profile(), newPath())
                : "a bare repository"}
            </p>
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
    </div>
  );
}
