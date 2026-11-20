import { confirmAction } from "./dialogs";
import { createEffect, createSignal, For, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage } from "./api";
import {
  readCustomCommands,
  splitArgs,
  writeCustomCommands,
  type CustomGitCommand,
} from "./gitArgs";
import type { BisectState, DeletedBranch, GitToolSettings } from "./types";

type Props = {
  repository: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
};

export default function GitAdvancedPanel(props: Props) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [tools, setTools] = createSignal<GitToolSettings>();
  const [diffTool, setDiffTool] = createSignal("");
  const [mergeTool, setMergeTool] = createSignal("");
  const [bisect, setBisect] = createSignal<BisectState>();
  const [goodCommit, setGoodCommit] = createSignal("");
  const [badCommit, setBadCommit] = createSignal("");
  const [patchRange, setPatchRange] = createSignal("HEAD");
  const [patchAsCommits, setPatchAsCommits] = createSignal(true);
  const [cleanIgnored, setCleanIgnored] = createSignal(false);
  const [cleanDirectories, setCleanDirectories] = createSignal(true);
  const [cleanPreview, setCleanPreview] = createSignal<string[]>();
  const [stashMessage, setStashMessage] = createSignal("");
  const [stashUntracked, setStashUntracked] = createSignal(false);
  const [stashKeepIndex, setStashKeepIndex] = createSignal(false);
  const [stashPaths, setStashPaths] = createSignal("");
  const [tagName, setTagName] = createSignal("");
  const [tagMessage, setTagMessage] = createSignal("");
  const [tagTarget, setTagTarget] = createSignal("");
  const [tagSign, setTagSign] = createSignal(false);
  const [noteCommit, setNoteCommit] = createSignal("HEAD");
  const [noteText, setNoteText] = createSignal("");
  const [subPrefix, setSubPrefix] = createSignal("");
  const [subRepo, setSubRepo] = createSignal("");
  const [subBranch, setSubBranch] = createSignal("main");
  const [subSquash, setSubSquash] = createSignal(true);
  const [commands, setCommands] = createSignal<CustomGitCommand[]>([]);
  const [commandName, setCommandName] = createSignal("");
  const [commandText, setCommandText] = createSignal("");
  const [commandOutput, setCommandOutput] = createSignal("");
  const [deleted, setDeleted] = createSignal<DeletedBranch[]>([]);

  async function run(job: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await job();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  createEffect(() => {
    const repository = props.repository;
    setCleanPreview(undefined);
    setCommandOutput("");
    setCommands(readCustomCommands(localStorage));
    void run(async () => {
      const settings = await api.gitToolSettings();
      setTools(settings);
      setDiffTool(settings.diffTool);
      setMergeTool(settings.mergeTool);
      setBisect(await api.gitBisect(repository, "status"));
      setDeleted(await api.gitDeletedBranches(repository));
    });
  });

  const bisectStep = (action: "start" | "good" | "bad" | "skip" | "reset") =>
    run(async () => {
      setBisect(
        await api.gitBisect(
          props.repository,
          action,
          goodCommit().trim() || undefined,
          badCommit().trim() || undefined,
        ),
      );
      props.onChanged();
    });

  const choosePatchFolder = async () => {
    const folder = await open({ directory: true, multiple: false });
    if (typeof folder !== "string") return;
    void run(async () => {
      const files = await api.gitPatchCreate(
        props.repository,
        patchRange(),
        folder,
      );
      props.onNotice(`Wrote ${files.length} patch file(s) to ${folder}.`);
    });
  };

  const applyPatch = async () => {
    const file = await open({
      multiple: false,
      filters: [{ name: "Patch", extensions: ["patch", "diff", "mbox"] }],
    });
    if (typeof file !== "string") return;
    void run(async () => {
      props.onNotice(
        await api.gitPatchApply(props.repository, file, patchAsCommits()),
      );
      props.onChanged();
    });
  };

  const saveCommand = () => {
    if (!commandName().trim() || !commandText().trim()) {
      setError("Name the command and enter its Git arguments.");
      return;
    }
    try {
      splitArgs(commandText());
    } catch (reason) {
      setError(errorMessage(reason));
      return;
    }
    const next = [
      ...commands().filter((item) => item.name !== commandName().trim()),
      {
        id: crypto.randomUUID(),
        name: commandName().trim(),
        command: commandText().trim(),
      },
    ];
    writeCustomCommands(localStorage, next);
    setCommands(next);
  };

  const runCommand = (text: string) =>
    run(async () => {
      setCommandOutput(
        (await api.gitCustomCommand(props.repository, splitArgs(text))) ||
          "Finished with no output.",
      );
      props.onChanged();
    });

  const check = (
    value: () => boolean,
    set: (value: boolean) => void,
    label: string,
  ) => (
    <label class="git-tools-checkbox">
      <input
        type="checkbox"
        checked={value()}
        onChange={(event) => set(event.currentTarget.checked)}
      />
      {label}
    </label>
  );

  return (
    <div class="git-tools-panel git-advanced-panel">
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>

      <section>
        <h3>External diff and merge tools</h3>
        <p class="muted">
          Saved in your Git settings, so they also work from the command line.
        </p>
        <div class="git-tools-form">
          <For
            each={
              [
                ["Diff tool", diffTool, setDiffTool],
                ["Merge tool", mergeTool, setMergeTool],
              ] as const
            }
          >
            {([label, value, set]) => (
              <label>
                {label}
                <select
                  value={value()}
                  onChange={(event) => set(event.currentTarget.value)}
                >
                  <option value="">None (use Weave)</option>
                  <For each={tools()?.tools ?? []}>
                    {(tool) => (
                      <option value={tool.id}>
                        {tool.name}
                        {tool.installed ? "" : " (not installed)"}
                      </option>
                    )}
                  </For>
                </select>
              </label>
            )}
          </For>
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() =>
              void run(async () => {
                setTools(
                  await api.gitToolSettingsSave(diffTool(), mergeTool()),
                );
                props.onNotice("External tools saved.");
              })
            }
          >
            Save tools
          </button>
        </div>
      </section>

      <section>
        <div class="git-tools-heading">
          <h3>Bisect</h3>
          <span class="muted">
            {bisect()?.active ? "In progress" : "Not running"}
          </span>
        </div>
        <p class="muted">
          Find the commit that introduced a problem. Mark each checked-out
          commit as good or bad until Git names the first bad one.
        </p>
        <Show
          when={bisect()?.active}
          fallback={
            <div class="git-tools-form">
              <input
                aria-label="Known good commit"
                placeholder="Known good commit or tag, such as v1.2"
                value={goodCommit()}
                onInput={(event) => setGoodCommit(event.currentTarget.value)}
              />
              <input
                aria-label="Known bad commit"
                placeholder="Known bad commit (default: HEAD)"
                value={badCommit()}
                onInput={(event) => setBadCommit(event.currentTarget.value)}
              />
              <button
                class="mini-button"
                disabled={busy() || !goodCommit().trim()}
                onClick={() => void bisectStep("start")}
              >
                Start bisect
              </button>
            </div>
          }
        >
          <p>
            Testing: <strong>{bisect()?.current}</strong>
          </p>
          <div class="git-tools-buttons">
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() => void bisectStep("good")}
            >
              Good
            </button>
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() => void bisectStep("bad")}
            >
              Bad
            </button>
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() => void bisectStep("skip")}
            >
              Skip
            </button>
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() => void bisectStep("reset")}
            >
              Finish
            </button>
          </div>
        </Show>
        <Show when={bisect()?.found}>
          <p class="git-advanced-found">{bisect()?.found}</p>
        </Show>
      </section>

      <section>
        <h3>Patches</h3>
        <div class="git-tools-form">
          <input
            aria-label="Commit or range for patches"
            placeholder="Commit or range, such as HEAD or main..feature"
            value={patchRange()}
            onInput={(event) => setPatchRange(event.currentTarget.value)}
          />
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() => void choosePatchFolder()}
          >
            Save patches to folder…
          </button>
          {check(
            patchAsCommits,
            setPatchAsCommits,
            "Apply as commits (git am) instead of file changes",
          )}
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() => void applyPatch()}
          >
            Apply patch file…
          </button>
        </div>
      </section>

      <section>
        <h3>Clean untracked files</h3>
        <div class="git-tools-form">
          {check(cleanDirectories, setCleanDirectories, "Include folders")}
          {check(cleanIgnored, setCleanIgnored, "Include ignored files")}
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() =>
              void run(async () => {
                setCleanPreview(
                  await api.gitCleanPreview(
                    props.repository,
                    cleanIgnored(),
                    cleanDirectories(),
                  ),
                );
              })
            }
          >
            Preview
          </button>
        </div>
        <Show when={cleanPreview()}>
          {(files) => (
            <>
              <ul class="git-advanced-list">
                <For
                  each={files()}
                  fallback={<li class="muted">Nothing to clean.</li>}
                >
                  {(file) => <li>{file}</li>}
                </For>
              </ul>
              <Show when={files().length}>
                <button
                  class="mini-button danger-text"
                  disabled={busy()}
                  onClick={async () => {
                    if (
                      !(await confirmAction(
                        `Permanently delete ${files().length} untracked item(s)? They are not in Git, so this cannot be undone.`,
                      ))
                    )
                      return;
                    void run(async () => {
                      const count = await api.gitClean(
                        props.repository,
                        cleanIgnored(),
                        cleanDirectories(),
                        files().length,
                      );
                      setCleanPreview(undefined);
                      props.onNotice(`Deleted ${count} untracked item(s).`);
                      props.onChanged();
                    });
                  }}
                >
                  Delete {files().length} item(s)
                </button>
              </Show>
            </>
          )}
        </Show>
      </section>

      <section>
        <h3>Stash with options</h3>
        <div class="git-tools-form">
          <input
            aria-label="Stash message"
            placeholder="Message (optional)"
            value={stashMessage()}
            onInput={(event) => setStashMessage(event.currentTarget.value)}
          />
          <textarea
            aria-label="Paths to stash"
            placeholder="Only these paths, one per line (optional)"
            value={stashPaths()}
            onInput={(event) => setStashPaths(event.currentTarget.value)}
          />
          {check(stashUntracked, setStashUntracked, "Include untracked files")}
          {check(stashKeepIndex, setStashKeepIndex, "Keep staged changes")}
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() =>
              void run(async () => {
                props.onNotice(
                  await api.gitStashPush(
                    props.repository,
                    stashMessage(),
                    stashUntracked(),
                    stashKeepIndex(),
                    stashPaths()
                      .split("\n")
                      .map((line) => line.trim())
                      .filter(Boolean),
                  ),
                );
                setStashMessage("");
                setStashPaths("");
                props.onChanged();
              })
            }
          >
            Stash
          </button>
        </div>
      </section>

      <section>
        <h3>Annotated tags</h3>
        <div class="git-tools-form">
          <input
            aria-label="Tag name"
            placeholder="v1.0.0"
            value={tagName()}
            onInput={(event) => setTagName(event.currentTarget.value)}
          />
          <textarea
            aria-label="Tag message"
            placeholder="Release notes or message"
            value={tagMessage()}
            onInput={(event) => setTagMessage(event.currentTarget.value)}
          />
          <input
            aria-label="Tag target"
            placeholder="Commit (default: HEAD)"
            value={tagTarget()}
            onInput={(event) => setTagTarget(event.currentTarget.value)}
          />
          {check(tagSign, setTagSign, "Sign the tag")}
          <div class="git-tools-buttons">
            <button
              class="mini-button"
              disabled={busy() || !tagName().trim()}
              onClick={() =>
                void run(async () => {
                  await api.gitTagAnnotated(
                    props.repository,
                    tagName(),
                    tagMessage(),
                    tagTarget(),
                    tagSign(),
                  );
                  props.onNotice(`Created tag ${tagName().trim()}.`);
                  props.onChanged();
                })
              }
            >
              Create tag
            </button>
            <button
              class="mini-button"
              disabled={busy() || !tagName().trim()}
              onClick={() =>
                void run(async () =>
                  props.onNotice(
                    await api.gitPushTags(props.repository, tagName(), false),
                  ),
                )
              }
            >
              Push this tag
            </button>
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() =>
                void run(async () =>
                  props.onNotice(
                    await api.gitPushTags(props.repository, null, false),
                  ),
                )
              }
            >
              Push all tags
            </button>
            <button
              class="mini-button danger-text"
              disabled={busy() || !tagName().trim()}
              onClick={async () => {
                if (
                  !(await confirmAction(
                    `Delete tag ${tagName().trim()} from origin? Others who fetched it keep their copy.`,
                  ))
                )
                  return;
                void run(async () =>
                  props.onNotice(
                    await api.gitPushTags(props.repository, tagName(), true),
                  ),
                );
              }}
            >
              Delete from origin
            </button>
          </div>
        </div>
      </section>

      <section>
        <h3>Commit notes</h3>
        <div class="git-tools-form">
          <div class="git-tools-buttons">
            <input
              aria-label="Commit for notes"
              value={noteCommit()}
              onInput={(event) => setNoteCommit(event.currentTarget.value)}
            />
            <button
              class="mini-button"
              disabled={busy()}
              onClick={() =>
                void run(async () => {
                  setNoteText(
                    await api.gitNotes(props.repository, noteCommit()),
                  );
                })
              }
            >
              Load
            </button>
          </div>
          <textarea
            aria-label="Note text"
            placeholder="Note attached to the commit (empty removes it)"
            value={noteText()}
            onInput={(event) => setNoteText(event.currentTarget.value)}
          />
          <button
            class="mini-button"
            disabled={busy()}
            onClick={() =>
              void run(async () => {
                await api.gitNotesSet(
                  props.repository,
                  noteCommit(),
                  noteText(),
                );
                props.onNotice("Note saved.");
              })
            }
          >
            Save note
          </button>
        </div>
      </section>

      <section>
        <h3>Subtrees</h3>
        <p class="muted">
          Include another repository's files in a folder, keeping its history
          mergeable.
        </p>
        <div class="git-tools-form">
          <input
            aria-label="Subtree folder"
            placeholder="Folder, such as vendor/library"
            value={subPrefix()}
            onInput={(event) => setSubPrefix(event.currentTarget.value)}
          />
          <input
            aria-label="Subtree repository"
            placeholder="Repository URL or remote name"
            value={subRepo()}
            onInput={(event) => setSubRepo(event.currentTarget.value)}
          />
          <input
            aria-label="Subtree branch"
            placeholder="Branch"
            value={subBranch()}
            onInput={(event) => setSubBranch(event.currentTarget.value)}
          />
          {check(subSquash, setSubSquash, "Squash its history into one commit")}
          <div class="git-tools-buttons">
            <For each={["add", "pull", "push"] as const}>
              {(action) => (
                <button
                  class="mini-button"
                  disabled={busy()}
                  onClick={() =>
                    void run(async () => {
                      props.onNotice(
                        await api.gitSubtree(
                          props.repository,
                          action,
                          subPrefix(),
                          subRepo(),
                          subBranch(),
                          subSquash(),
                        ),
                      );
                      props.onChanged();
                    })
                  }
                >
                  {action === "add"
                    ? "Add subtree"
                    : action === "pull"
                      ? "Pull updates"
                      : "Push changes"}
                </button>
              )}
            </For>
          </div>
        </div>
      </section>

      <section>
        <h3>Custom Git commands</h3>
        <p class="muted">
          Save commands you use often. Write them without “git”, for example log
          --oneline --graph -20.
        </p>
        <div class="git-tools-form">
          <input
            aria-label="Command name"
            placeholder="Name"
            value={commandName()}
            onInput={(event) => setCommandName(event.currentTarget.value)}
          />
          <input
            aria-label="Git arguments"
            placeholder="Arguments, such as fetch --prune"
            value={commandText()}
            onInput={(event) => setCommandText(event.currentTarget.value)}
          />
          <div class="git-tools-buttons">
            <button
              class="mini-button"
              disabled={busy() || !commandText().trim()}
              onClick={() => void runCommand(commandText())}
            >
              Run
            </button>
            <button class="mini-button" disabled={busy()} onClick={saveCommand}>
              Save
            </button>
          </div>
        </div>
        <For each={commands()}>
          {(command) => (
            <div class="git-reflog-item">
              <div>
                <strong>{command.name}</strong>
                <span class="muted">git {command.command}</span>
              </div>
              <button
                class="mini-button"
                disabled={busy()}
                onClick={() => void runCommand(command.command)}
              >
                Run
              </button>
              <button
                class="mini-button"
                onClick={() => {
                  const next = commands().filter(
                    (item) => item.id !== command.id,
                  );
                  writeCustomCommands(localStorage, next);
                  setCommands(next);
                }}
              >
                Remove
              </button>
            </div>
          )}
        </For>
        <Show when={commandOutput()}>
          <pre class="git-advanced-output">{commandOutput()}</pre>
        </Show>
      </section>

      <section>
        <h3>Deleted branches</h3>
        <For
          each={deleted()}
          fallback={
            <p class="muted">Branches you delete in Weave appear here.</p>
          }
        >
          {(branch) => (
            <div class="git-reflog-item">
              <div>
                <strong>{branch.name}</strong>
                <span class="muted">
                  {branch.commit.slice(0, 8)} · deleted{" "}
                  {new Date(branch.deletedAt * 1000).toLocaleString()}
                </span>
              </div>
              <button
                class="mini-button"
                disabled={busy()}
                onClick={() =>
                  void run(async () => {
                    props.onNotice(
                      await api.gitBranchRestore(props.repository, branch.name),
                    );
                    setDeleted(await api.gitDeletedBranches(props.repository));
                    props.onChanged();
                  })
                }
              >
                Restore
              </button>
            </div>
          )}
        </For>
      </section>
    </div>
  );
}
