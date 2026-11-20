import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { systemName, thisComputer } from "./platform";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-solid";
import { errorMessage } from "./api";
import S3PropertiesForm from "./S3PropertiesForm";

export type FileTool =
  | { kind: "find"; profileId: string | null; root: string }
  | { kind: "preview"; profileId: string | null; path: string; name: string }
  | {
      kind: "symlink";
      profileId: string | null;
      folder: string;
      target: string;
    }
  | { kind: "owner"; profileId: string | null; paths: string[] }
  | { kind: "shares" }
  | { kind: "script"; localDir: string }
  | { kind: "servers"; folder: string }
  | { kind: "archive"; profileId: string | null; cwd: string; names: string[] }
  | { kind: "s3props"; profileId: string; path: string; name: string }
  | {
      kind: "command";
      profileId: string | null;
      cwd: string;
      paths: string[];
    };

type Found = {
  path: string;
  name: string;
  isDir: boolean;
  size: number;
};
type Preview = {
  kind: "image" | "pdf" | "text" | "binary";
  mime: string;
  data: string;
  size: number;
  truncated: boolean;
};
type SavedCommand = { id: string; name: string; template: string };

const commandsKey = "weave-file-commands";
const scriptsKey = "weave-file-scripts";

const scriptHelp = `open "My server"     connect to a saved connection
cd / lcd <folder>    change the remote / local folder
ls / lls [folder]    list files
get <file or *.mask> [local folder]
put <file or *.mask> [remote folder]
mkdir <folder>   mv <from> <to>   rm [-r] <path>
sync up|down|both <local> <remote> [--delete] [--exclude "*.tmp;cache"]
option conflict overwrite|skip|rename|newer
option errors stop|continue
echo <text>   exit   # comment   %DATE% %TIME%`;

function readCommands(key = commandsKey): SavedCommand[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(value)
      ? value.filter(
          (item): item is SavedCommand =>
            typeof item?.id === "string" &&
            typeof item?.name === "string" &&
            typeof item?.template === "string",
        )
      : [];
  } catch {
    return [];
  }
}

function sizeText(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

type Mounted = { kind: string; source: string; path: string };
type FileServer = {
  id: string;
  kind: "tftp" | "http";
  folder: string;
  port: number;
  everyone: boolean;
  uploads: boolean;
  addresses: string[];
};

type Props = {
  tool: FileTool;
  onReveal: (path: string) => void;
  onOpenFolder: (path: string) => void;
  onDone: (message: string) => void;
  onClose: () => void;
};

export default function FileToolDialogs(props: Props) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [mask, setMask] = createSignal("");
  const [found, setFound] = createSignal<{
    results: Found[];
    truncated: boolean;
  }>();
  const [preview, setPreview] = createSignal<Preview>();
  const [target, setTarget] = createSignal("");
  const [linkName, setLinkName] = createSignal("");
  const [owner, setOwner] = createSignal("");
  const [archiveName, setArchiveName] = createSignal("");
  const [archiveFormat, setArchiveFormat] = createSignal("zip");
  const [group, setGroup] = createSignal("");
  const [recursive, setRecursive] = createSignal(false);
  const [template, setTemplate] = createSignal("");
  const [commandName, setCommandName] = createSignal("");
  const [commands, setCommands] = createSignal<SavedCommand[]>(readCommands());
  const [output, setOutput] = createSignal("");
  const [shareKind, setShareKind] = createSignal<"smb" | "nfs">("smb");
  const [server, setServer] = createSignal("");
  const [share, setShare] = createSignal("");
  const [shareUser, setShareUser] = createSignal("");
  const [mounted, setMounted] = createSignal<Mounted[]>([]);
  const [scripts, setScripts] = createSignal<SavedCommand[]>(
    readCommands(scriptsKey),
  );
  const [scriptText, setScriptText] = createSignal("");
  const [scriptName, setScriptName] = createSignal("");
  const [scriptLog, setScriptLog] = createSignal<string[]>([]);
  const [fileServers, setFileServers] = createSignal<FileServer[]>([]);
  const [serverKind, setServerKind] = createSignal<"tftp" | "http">("tftp");
  const [serverPort, setServerPort] = createSignal(69);
  const [serverEveryone, setServerEveryone] = createSignal(true);
  const [serverUploads, setServerUploads] = createSignal(false);
  const [serverLog, setServerLog] = createSignal<string[]>([]);
  const loadServers = () =>
    invoke<FileServer[]>("file_servers_list").then(setFileServers);
  let scriptRun = "";
  let stopServerLog: (() => void) | undefined;
  let closed = false;
  onCleanup(() => {
    closed = true;
    stopServerLog?.();
  });
  let logBox: HTMLPreElement | undefined;
  onCleanup(() => {
    if (scriptRun) void invoke("files_script_stop", { runId: scriptRun });
  });
  const loadMounted = () =>
    run(async () => {
      setMounted(await invoke<Mounted[]>("shares_mounted"));
    });

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

  onMount(() => {
    const tool = props.tool;
    if (tool.kind === "preview")
      void run(async () => {
        setPreview(
          await invoke<Preview>("files_preview", {
            profileId: tool.profileId,
            path: tool.path,
          }),
        );
      });
    if (tool.kind === "shares") void loadMounted();
    if (tool.kind === "servers") {
      void loadServers().catch((reason) => setError(errorMessage(reason)));
      void listen<{ id: string; line: string }>("file-server-log", (event) =>
        setServerLog((lines) => [
          ...lines.slice(-199),
          `${new Date().toLocaleTimeString()}  ${event.payload.line}`,
        ]),
      ).then((stop) => {
        if (closed) stop();
        else stopServerLog = stop;
      });
    }
    if (tool.kind === "archive") {
      setArchiveName(tool.names.length === 1 ? tool.names[0] : "Archive");
    }
    if (tool.kind === "symlink") {
      setTarget(tool.target);
      setLinkName(
        tool.target ? `${tool.target.split("/").pop()} link` : "link",
      );
    }
  });

  const title = () =>
    ({
      find: "Find files",
      preview: props.tool.kind === "preview" ? props.tool.name : "Preview",
      symlink: "Create symbolic link",
      owner: "Change owner and group",
      command: "Run a command",
      shares: "Network shares",
      script: "File script",
      servers: "TFTP and HTTP servers",
      archive: "Compress",
      s3props:
        props.tool.kind === "s3props"
          ? `S3 properties: ${props.tool.name}`
          : "S3 properties",
    })[props.tool.kind];

  const saveCommands = (next: SavedCommand[]) => {
    try {
      localStorage.setItem(commandsKey, JSON.stringify(next.slice(0, 50)));
    } catch {
      setError(`Could not save the command on ${thisComputer}.`);
    }
    setCommands(next);
  };

  const saveScripts = (next: SavedCommand[]) => {
    try {
      localStorage.setItem(scriptsKey, JSON.stringify(next.slice(0, 50)));
    } catch {
      setError(`Could not save the script on ${thisComputer}.`);
    }
    setScripts(next);
  };

  async function runScript(text: string, localDir: string) {
    const runId = crypto.randomUUID();
    scriptRun = runId;
    setScriptLog([]);
    const stopListening = await listen<{ runId: string; line: string }>(
      "file-script-output",
      (event) => {
        if (event.payload.runId !== runId) return;
        setScriptLog((lines) => [...lines.slice(-1999), event.payload.line]);
        queueMicrotask(() => logBox?.scrollTo({ top: logBox.scrollHeight }));
      },
    );
    await run(async () => {
      try {
        const outcome = await invoke<{
          failures: number;
          error: string | null;
        }>("files_script_run", { runId, localDir, script: text });
        if (outcome.error) setError(outcome.error);
        else
          setScriptLog((lines) => [
            ...lines,
            outcome.failures
              ? `Finished with ${outcome.failures} failed command(s).`
              : "Finished.",
          ]);
      } finally {
        stopListening();
        scriptRun = "";
      }
    });
  }

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class={`dialog profile-dialog file-tool-dialog file-tool-${props.tool.kind}`}
        role="dialog"
        aria-modal="true"
        aria-label={title()}
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">FILES</span>
            <h2 class="truncate">{title()}</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>

        <Show when={props.tool.kind === "find" && props.tool}>
          {(tool) => (
            <>
              <form
                class="connect-saved-row"
                onSubmit={(event) => {
                  event.preventDefault();
                  const current = tool() as Extract<FileTool, { kind: "find" }>;
                  void run(async () => {
                    setFound(
                      await invoke<{ results: Found[]; truncated: boolean }>(
                        "files_find",
                        {
                          profileId: current.profileId,
                          root: current.root,
                          mask: mask(),
                        },
                      ),
                    );
                  });
                }}
              >
                <input
                  class="file-tool-input"
                  autofocus
                  aria-label="File name or pattern"
                  placeholder="Name or pattern, such as *.log or report-??.pdf"
                  value={mask()}
                  onInput={(event) => setMask(event.currentTarget.value)}
                />
                <button class="primary-button" type="submit" disabled={busy()}>
                  {busy() ? "Searching…" : "Find"}
                </button>
              </form>
              <p class="muted">
                Searches inside{" "}
                {(tool() as Extract<FileTool, { kind: "find" }>).root} and its
                subfolders.
              </p>
              <Show when={found()}>
                {(result) => (
                  <div class="cryptomator-list file-find-results">
                    <For
                      each={result().results}
                      fallback={<p class="muted">Nothing matched.</p>}
                    >
                      {(item) => (
                        <button
                          onClick={() => {
                            props.onReveal(item.path);
                            props.onClose();
                          }}
                          title="Show in the pane"
                        >
                          <span class="truncate">{item.path}</span>
                          <span class="muted">
                            {item.isDir ? "Folder" : sizeText(item.size)}
                          </span>
                        </button>
                      )}
                    </For>
                    <Show when={result().truncated}>
                      <p class="muted">
                        Showing the first 500 matches. Narrow the pattern to see
                        more.
                      </p>
                    </Show>
                  </div>
                )}
              </Show>
            </>
          )}
        </Show>

        <Show when={props.tool.kind === "preview"}>
          <div class="file-preview">
            <Show
              when={preview()}
              fallback={<p class="muted">{busy() ? "Loading…" : ""}</p>}
            >
              {(current) => (
                <Show
                  when={!current().truncated || current().kind === "text"}
                  fallback={
                    <p class="muted">
                      This file is {sizeText(current().size)}, too large to
                      preview. Open it in its app instead.
                    </p>
                  }
                >
                  <Show when={current().kind === "image"}>
                    <img
                      alt={title()}
                      src={`data:${current().mime};base64,${current().data}`}
                    />
                  </Show>
                  <Show when={current().kind === "pdf"}>
                    <iframe
                      title={title()}
                      src={`data:application/pdf;base64,${current().data}`}
                    />
                  </Show>
                  <Show when={current().kind === "text"}>
                    <pre>{current().data}</pre>
                    <Show when={current().truncated}>
                      <p class="muted">
                        Showing the first 512 KB of {sizeText(current().size)}.
                      </p>
                    </Show>
                  </Show>
                  <Show when={current().kind === "binary"}>
                    <p class="muted">
                      This is a binary file ({sizeText(current().size)}) with no
                      preview. Open it in its app instead.
                    </p>
                  </Show>
                </Show>
              )}
            </Show>
          </div>
        </Show>

        <Show when={props.tool.kind === "symlink" && props.tool}>
          {(tool) => (
            <form
              class="form-grid"
              onSubmit={(event) => {
                event.preventDefault();
                const current = tool() as Extract<
                  FileTool,
                  { kind: "symlink" }
                >;
                void run(async () => {
                  const separator = current.folder.endsWith("/") ? "" : "/";
                  await invoke("files_symlink", {
                    profileId: current.profileId,
                    target: target(),
                    link: `${current.folder}${separator}${linkName()}`,
                  });
                  props.onDone(`Created link ${linkName()}.`);
                  props.onClose();
                });
              }}
            >
              <label class="wide">
                Points to
                <input
                  required
                  value={target()}
                  onInput={(event) => setTarget(event.currentTarget.value)}
                />
              </label>
              <label class="wide">
                Link name (in the current folder)
                <input
                  required
                  value={linkName()}
                  onInput={(event) => setLinkName(event.currentTarget.value)}
                />
              </label>
              <div class="wide">
                <button class="primary-button" type="submit" disabled={busy()}>
                  Create link
                </button>
              </div>
            </form>
          )}
        </Show>

        <Show when={props.tool.kind === "owner" && props.tool}>
          {(tool) => (
            <form
              class="form-grid"
              onSubmit={(event) => {
                event.preventDefault();
                const current = tool() as Extract<FileTool, { kind: "owner" }>;
                void run(async () => {
                  await invoke("files_chown", {
                    profileId: current.profileId,
                    paths: current.paths,
                    owner: owner(),
                    group: group(),
                    recursive: recursive(),
                  });
                  props.onDone("Owner and group updated.");
                  props.onClose();
                });
              }}
            >
              <p class="muted wide">
                {(tool() as Extract<FileTool, { kind: "owner" }>).paths.length}{" "}
                item(s). Leave a field empty to keep it. Changing the owner
                usually needs administrator rights on the server.
              </p>
              <label>
                Owner
                <input
                  value={owner()}
                  onInput={(event) => setOwner(event.currentTarget.value)}
                />
              </label>
              <label>
                Group
                <input
                  value={group()}
                  onInput={(event) => setGroup(event.currentTarget.value)}
                />
              </label>
              <label class="wide git-tools-checkbox">
                <input
                  type="checkbox"
                  checked={recursive()}
                  onChange={(event) =>
                    setRecursive(event.currentTarget.checked)
                  }
                />
                Include everything inside folders
              </label>
              <div class="wide">
                <button class="primary-button" type="submit" disabled={busy()}>
                  Apply
                </button>
              </div>
            </form>
          )}
        </Show>

        <Show when={props.tool.kind === "s3props" && props.tool}>
          {(tool) => {
            const current = tool() as Extract<FileTool, { kind: "s3props" }>;
            return (
              <S3PropertiesForm
                profileId={current.profileId}
                path={current.path}
                onDone={props.onDone}
                onClose={props.onClose}
              />
            );
          }}
        </Show>

        <Show when={props.tool.kind === "archive" && props.tool}>
          {(tool) => {
            const current = () =>
              tool() as Extract<FileTool, { kind: "archive" }>;
            const output = () => `${archiveName().trim()}.${archiveFormat()}`;
            return (
              <form
                class="form-grid"
                onSubmit={(event) => {
                  event.preventDefault();
                  void run(async () => {
                    await invoke("files_archive", {
                      profileId: current().profileId,
                      cwd: current().cwd,
                      names: current().names,
                      format: archiveFormat(),
                      output: output(),
                    });
                    props.onDone(`Created ${output()}.`);
                    props.onClose();
                  });
                }}
              >
                <p class="muted wide">
                  {current().names.length === 1
                    ? current().names[0]
                    : `${current().names.length} items`}{" "}
                  from {current().cwd}
                </p>
                <label>
                  Name
                  <input
                    required
                    spellcheck={false}
                    value={archiveName()}
                    onInput={(event) =>
                      setArchiveName(event.currentTarget.value)
                    }
                  />
                </label>
                <label>
                  Format
                  <select
                    value={archiveFormat()}
                    onChange={(event) =>
                      setArchiveFormat(event.currentTarget.value)
                    }
                  >
                    <option value="zip">zip</option>
                    <option value="tar.gz">tar.gz</option>
                    <option value="tar.bz2">tar.bz2</option>
                    <option value="tar.xz">tar.xz</option>
                    <option value="tar">tar</option>
                  </select>
                </label>
                <div class="wide">
                  <button
                    class="primary-button"
                    type="submit"
                    disabled={busy() || !archiveName().trim()}
                  >
                    {busy() ? "Compressing…" : `Create ${output()}`}
                  </button>
                </div>
              </form>
            );
          }}
        </Show>

        <Show when={props.tool.kind === "command" && props.tool}>
          {(tool) => {
            const current = () =>
              tool() as Extract<FileTool, { kind: "command" }>;
            const execute = (text: string) =>
              run(async () => {
                setOutput(
                  (await invoke<string>("files_run_command", {
                    profileId: current().profileId,
                    cwd: current().cwd,
                    template: text,
                    paths: current().paths,
                  })) || "Finished with no output.",
                );
              });
            return (
              <>
                <p class="muted">
                  Runs in {current().cwd} on{" "}
                  {current().profileId ? "the SSH host" : thisComputer}. Use{" "}
                  <code>{"{path}"}</code>, <code>{"{paths}"}</code>,{" "}
                  <code>{"{name}"}</code> and <code>{"{dir}"}</code> for the{" "}
                  {current().paths.length} selected item(s); Weave quotes them
                  for you.
                </p>
                <For each={commands()}>
                  {(command) => (
                    <div class="connect-saved-row">
                      <button
                        class="connect-saved-open"
                        disabled={busy()}
                        onClick={() => void execute(command.template)}
                      >
                        <strong>{command.name}</strong>
                        <span class="muted">{command.template}</span>
                      </button>
                      <button
                        class="server-action"
                        onClick={() =>
                          saveCommands(
                            commands().filter((item) => item.id !== command.id),
                          )
                        }
                      >
                        Remove
                      </button>
                    </div>
                  )}
                </For>
                <textarea
                  class="file-tool-input"
                  aria-label="Command"
                  placeholder="tar -czf {name}.tar.gz {paths}"
                  value={template()}
                  onInput={(event) => setTemplate(event.currentTarget.value)}
                />
                <div class="connect-saved-row">
                  <input
                    class="file-tool-input"
                    aria-label="Name to save the command as"
                    placeholder="Name to save it as (optional)"
                    value={commandName()}
                    onInput={(event) =>
                      setCommandName(event.currentTarget.value)
                    }
                  />
                  <button
                    class="server-action"
                    disabled={!template().trim() || !commandName().trim()}
                    onClick={() =>
                      saveCommands([
                        ...commands(),
                        {
                          id: crypto.randomUUID(),
                          name: commandName().trim(),
                          template: template().trim(),
                        },
                      ])
                    }
                  >
                    Save
                  </button>
                  <button
                    class="primary-button"
                    disabled={busy() || !template().trim()}
                    onClick={() => void execute(template())}
                  >
                    {busy() ? "Running…" : "Run"}
                  </button>
                </div>
                <Show when={output()}>
                  <pre class="git-advanced-output">{output()}</pre>
                </Show>
              </>
            );
          }}
        </Show>

        <Show when={props.tool.kind === "shares"}>
          <p class="muted">
            Connect to Windows, NAS (SMB) or NFS shares. {systemName} asks for
            the password and can keep it for you; the share then opens as a
            folder.
          </p>
          <form
            class="form-grid"
            onSubmit={(event) => {
              event.preventDefault();
              void run(async () => {
                const path = await invoke<string>("shares_connect", {
                  kind: shareKind(),
                  server: server(),
                  share: share(),
                  user: shareUser(),
                });
                props.onOpenFolder(path);
                props.onClose();
              });
            }}
          >
            <label>
              Type
              <select
                value={shareKind()}
                onChange={(event) =>
                  setShareKind(event.currentTarget.value as "smb" | "nfs")
                }
              >
                <option value="smb">SMB (Windows, most NAS)</option>
                <option value="nfs">NFS</option>
              </select>
            </label>
            <label>
              Server
              <input
                required
                placeholder="nas.local"
                value={server()}
                onInput={(event) => setServer(event.currentTarget.value)}
              />
            </label>
            <label>
              {shareKind() === "smb" ? "Share" : "Exported folder"}
              <input
                required
                placeholder={shareKind() === "smb" ? "Photos" : "/volume1/data"}
                value={share()}
                onInput={(event) => setShare(event.currentTarget.value)}
              />
            </label>
            <Show when={shareKind() === "smb"}>
              <label>
                User (optional)
                <input
                  placeholder="DOMAIN;name or name"
                  value={shareUser()}
                  onInput={(event) => setShareUser(event.currentTarget.value)}
                />
              </label>
            </Show>
            <div class="wide">
              <button class="primary-button" type="submit" disabled={busy()}>
                {busy() ? "Waiting for the share…" : "Connect"}
              </button>
            </div>
          </form>
          <h3 class="server-repos-heading">Connected shares</h3>
          <For
            each={mounted()}
            fallback={<p class="muted">No network shares are connected.</p>}
          >
            {(item) => (
              <div class="connect-saved-row">
                <div class="server-repo-name">
                  <strong>{item.path}</strong>
                  <span class="muted">
                    {item.kind.toUpperCase()} · {item.source}
                  </span>
                </div>
                <button
                  class="server-action"
                  onClick={() => {
                    props.onOpenFolder(item.path);
                    props.onClose();
                  }}
                >
                  Open
                </button>
                <button
                  class="server-action"
                  disabled={busy()}
                  onClick={() =>
                    void run(async () => {
                      await invoke("shares_eject", { path: item.path });
                      await loadMounted();
                    })
                  }
                >
                  Eject
                </button>
              </div>
            )}
          </For>
        </Show>

        <Show when={props.tool.kind === "script" && props.tool}>
          {(tool) => {
            const localDir = () =>
              (tool() as Extract<FileTool, { kind: "script" }>).localDir;
            return (
              <>
                <p class="muted">
                  Runs one command per line, starting in {localDir()} on{" "}
                  {thisComputer}. Copies go through the transfer queue and stop
                  the script if they fail, unless it sets{" "}
                  <code>option errors continue</code>.
                </p>
                <details class="file-script-help">
                  <summary>Commands</summary>
                  <pre>{scriptHelp}</pre>
                </details>
                <For each={scripts()}>
                  {(saved) => (
                    <div class="connect-saved-row">
                      <button
                        class="connect-saved-open"
                        disabled={busy()}
                        onClick={() => {
                          setScriptText(saved.template);
                          setScriptName(saved.name);
                        }}
                      >
                        <strong>{saved.name}</strong>
                        <span class="muted">
                          {saved.template.split("\n").length} lines
                        </span>
                      </button>
                      <button
                        class="server-action"
                        disabled={busy()}
                        onClick={() =>
                          void runScript(saved.template, localDir())
                        }
                      >
                        Run
                      </button>
                      <button
                        class="server-action"
                        onClick={() =>
                          saveScripts(
                            scripts().filter((item) => item.id !== saved.id),
                          )
                        }
                      >
                        Remove
                      </button>
                    </div>
                  )}
                </For>
                <textarea
                  class="file-tool-input file-script-input"
                  aria-label="Script"
                  spellcheck={false}
                  placeholder={
                    'open "Web server"\ncd /var/www\nput *.html\nsync up site /var/www/site --exclude "*.tmp"'
                  }
                  value={scriptText()}
                  onInput={(event) => setScriptText(event.currentTarget.value)}
                />
                <div class="connect-saved-row">
                  <input
                    class="file-tool-input"
                    aria-label="Name to save the script as"
                    placeholder="Name to save it as (optional)"
                    value={scriptName()}
                    onInput={(event) =>
                      setScriptName(event.currentTarget.value)
                    }
                  />
                  <button
                    class="server-action"
                    disabled={!scriptText().trim() || !scriptName().trim()}
                    onClick={() =>
                      saveScripts([
                        ...scripts().filter(
                          (item) => item.name !== scriptName().trim(),
                        ),
                        {
                          id: crypto.randomUUID(),
                          name: scriptName().trim(),
                          template: scriptText(),
                        },
                      ])
                    }
                  >
                    Save
                  </button>
                  <Show
                    when={busy()}
                    fallback={
                      <button
                        class="primary-button"
                        disabled={!scriptText().trim()}
                        onClick={() => void runScript(scriptText(), localDir())}
                      >
                        Run
                      </button>
                    }
                  >
                    <button
                      class="ghost-button"
                      onClick={() =>
                        void invoke("files_script_stop", { runId: scriptRun })
                      }
                    >
                      Stop
                    </button>
                  </Show>
                </div>
                <Show when={scriptLog().length}>
                  <pre class="git-advanced-output" ref={logBox}>
                    {scriptLog().join("\n")}
                  </pre>
                </Show>
              </>
            );
          }}
        </Show>

        <Show when={props.tool.kind === "servers" && props.tool}>
          {(tool) => {
            const folder = () =>
              (tool() as Extract<FileTool, { kind: "servers" }>).folder;
            return (
              <>
                <p class="muted">
                  Share {folder()} with network devices: TFTP for routers,
                  switches and phones loading firmware or saving configs, HTTP
                  for anything with a browser. HTTP is read-only. Servers stop
                  when Weave quits.
                </p>
                <form
                  class="form-grid"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void run(async () => {
                      await invoke("file_server_start", {
                        kind: serverKind(),
                        folder: folder(),
                        port: serverPort(),
                        everyone: serverEveryone(),
                        uploads: serverUploads(),
                      });
                      await loadServers();
                    });
                  }}
                >
                  <label>
                    Server
                    <select
                      value={serverKind()}
                      onChange={(event) => {
                        const kind = event.currentTarget.value as
                          "tftp" | "http";
                        setServerKind(kind);
                        setServerPort(kind === "tftp" ? 69 : 8080);
                      }}
                    >
                      <option value="tftp">TFTP</option>
                      <option value="http">HTTP</option>
                    </select>
                  </label>
                  <label>
                    Port
                    <input
                      type="number"
                      min="1"
                      max="65535"
                      value={serverPort()}
                      onInput={(event) =>
                        setServerPort(Number(event.currentTarget.value))
                      }
                    />
                  </label>
                  <label class="profile-favorite wide">
                    <input
                      type="checkbox"
                      checked={serverEveryone()}
                      onChange={(event) =>
                        setServerEveryone(event.currentTarget.checked)
                      }
                    />
                    Reachable from other devices on the network (otherwise only
                    {thisComputer})
                  </label>
                  <Show when={serverKind() === "tftp"}>
                    <label class="profile-favorite wide">
                      <input
                        type="checkbox"
                        checked={serverUploads()}
                        onChange={(event) =>
                          setServerUploads(event.currentTarget.checked)
                        }
                      />
                      Allow devices to upload new files (existing files are
                      never replaced)
                    </label>
                  </Show>
                  <div class="wide">
                    <button
                      class="primary-button"
                      type="submit"
                      disabled={busy()}
                    >
                      Start server
                    </button>
                  </div>
                </form>
                <h3 class="server-repos-heading">Running</h3>
                <For
                  each={fileServers()}
                  fallback={<p class="muted">No servers are running.</p>}
                >
                  {(server) => (
                    <div class="connect-saved-row">
                      <div class="server-repo-name">
                        <strong>
                          {server.kind.toUpperCase()} on port {server.port}
                          {server.uploads ? " · uploads allowed" : ""}
                        </strong>
                        <span class="muted">
                          {server.addresses
                            .map((address) =>
                              server.kind === "http"
                                ? `http://${address}:${server.port}/`
                                : `${address}:${server.port}`,
                            )
                            .join("  ·  ") || "No network address found"}
                        </span>
                        <span class="muted truncate">{server.folder}</span>
                      </div>
                      <button
                        class="server-action"
                        onClick={() =>
                          void run(async () => {
                            await invoke("file_server_stop", { id: server.id });
                            await loadServers();
                          })
                        }
                      >
                        Stop
                      </button>
                    </div>
                  )}
                </For>
                <Show when={serverLog().length}>
                  <pre class="git-advanced-output">
                    {serverLog().join("\n")}
                  </pre>
                </Show>
              </>
            );
          }}
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
