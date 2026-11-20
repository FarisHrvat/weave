import { profileAddress } from "./profileLabel";
import { os, thisComputer } from "./platform";
import { createSignal, For, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Trash2, X } from "lucide-solid";
import { errorMessage } from "./api";
import type { Profile } from "./types";
import {
  connectionKindLabels,
  defaultPorts,
  describeConnection,
  isTerminalConnection,
  readRemoteConnections,
  serialBaudRates,
  validateConnection,
  writeRemoteConnections,
  type RemoteConnection,
  type RemoteConnectionKind,
  type SavedRemoteConnection,
} from "./remoteConnections";

type ToolStatus = { mosh: boolean; xquartz: boolean; rdpApp: string | null };

type Props = {
  profiles: Profile[];
  onOpenTerminal: (connection: RemoteConnection) => void;
  onOpenVnc: (connection: RemoteConnection) => void;
  onNotify: (message: string, error?: boolean) => void;
  onClose: () => void;
};

const kinds: RemoteConnectionKind[] = [
  "telnet",
  "serial",
  "mosh",
  "x11",
  "vnc",
  "rdp",
];

function blank(kind: RemoteConnectionKind): RemoteConnection {
  return {
    kind,
    name: "",
    host: "",
    port: defaultPorts[kind],
    baud: 115200,
  };
}

export default function ConnectDialog(props: Props) {
  const [draft, setDraft] = createSignal<RemoteConnection>(blank("telnet"));
  const [saved, setSaved] = createSignal<SavedRemoteConnection[]>([]);
  const [devices, setDevices] = createSignal<string[]>([]);
  const [tools, setTools] = createSignal<ToolStatus>();
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const sshProfiles = () =>
    props.profiles.filter((profile) => profile.protocol === "ssh");

  function update(patch: Partial<RemoteConnection>) {
    setDraft((current) => ({ ...current, ...patch }));
    setError("");
  }

  async function refreshDevices() {
    try {
      setDevices(await invoke<string[]>("serial_devices"));
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }

  onMount(() => {
    setSaved(readRemoteConnections(localStorage));
    void refreshDevices();
    invoke<ToolStatus>("remote_tools_status")
      .then(setTools)
      .catch(() => undefined);
  });

  function toolWarning() {
    const status = tools();
    if (!status) return "";
    switch (draft().kind) {
      case "mosh":
        return status.mosh
          ? ""
          : os === "windows"
            ? "Mosh has no Windows version. Use SSH, or run Mosh inside WSL."
            : os === "mac"
              ? `Mosh is not installed on ${thisComputer}. Install it with Homebrew (brew install mosh); the host also needs mosh-server.`
              : "Mosh is not installed. Install the mosh package; the host also needs mosh-server.";
      case "x11":
        return status.xquartz
          ? ""
          : os === "windows"
            ? "X11 forwarding needs an X server such as VcXsrv or Xming, started with access control off."
            : os === "mac"
              ? "X11 forwarding needs XQuartz from xquartz.org."
              : "X11 forwarding needs xauth (the xauth package).";
      case "rdp":
        return status.rdpApp
          ? ""
          : os === "mac"
            ? "RDP opens in Microsoft's Windows App, which is not installed. Get it from the Mac App Store."
            : os === "windows"
              ? "Remote Desktop Connection (mstsc) is missing from this PC."
              : "RDP needs FreeRDP or Remmina. Install the freerdp or remmina package.";
      case "vnc":
        return "";
      default:
        return "";
    }
  }

  async function connect(input: RemoteConnection, external = false) {
    const connection = {
      ...input,
      name:
        input.name.trim() ||
        `${connectionKindLabels[input.kind]} · ${describeConnection(input, hostFor(input.profileId))}`.slice(
          0,
          80,
        ),
    };
    const problem = validateConnection(connection);
    if (problem) {
      setError(problem);
      return;
    }
    if (isTerminalConnection(connection)) {
      props.onOpenTerminal(connection);
      props.onClose();
      return;
    }
    if (connection.kind === "vnc" && !external) {
      props.onOpenVnc(connection);
      props.onClose();
      return;
    }
    setBusy(true);
    try {
      const message = await invoke<string>("remote_desktop_open", {
        kind: connection.kind,
        host: connection.host,
        port: connection.port,
        username: connection.username || null,
      });
      props.onNotify(message);
      props.onClose();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  function save() {
    const connection = draft();
    const problem = validateConnection(connection);
    if (problem) {
      setError(problem);
      return;
    }
    const next = [
      ...saved().filter((item) => item.name !== connection.name.trim()),
      { ...connection, name: connection.name.trim(), id: crypto.randomUUID() },
    ];
    try {
      writeRemoteConnections(localStorage, next);
      setSaved(next);
      props.onNotify(`Saved “${connection.name.trim()}”.`);
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }

  function remove(id: string) {
    const next = saved().filter((item) => item.id !== id);
    writeRemoteConnections(localStorage, next);
    setSaved(next);
  }

  const hostFor = (profileId?: string) =>
    props.profiles.find((profile) => profile.id === profileId)?.host;

  return (
    <div class="overlay" onClick={() => props.onClose()}>
      <div
        class="dialog profile-dialog connect-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="New connection"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">REMOTE ACCESS</span>
            <h2>New connection</h2>
          </div>
          <button
            class="icon-button"
            aria-label="Close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>

        <Show when={saved().length}>
          <div class="connect-saved">
            <For each={saved()}>
              {(item) => (
                <div class="connect-saved-row">
                  <button
                    class="connect-saved-open"
                    disabled={busy()}
                    onClick={() => void connect(item)}
                    title={`Open ${item.name}`}
                  >
                    <strong>{item.name}</strong>
                    <span class="muted">
                      {connectionKindLabels[item.kind]} ·{" "}
                      {describeConnection(item, hostFor(item.profileId))}
                    </span>
                  </button>
                  <button
                    class="icon-button"
                    aria-label={`Delete ${item.name}`}
                    onClick={() => remove(item.id)}
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
              )}
            </For>
          </div>
        </Show>

        <div class="segmented connect-kinds" role="tablist">
          <For each={kinds}>
            {(kind) => (
              <button
                role="tab"
                aria-selected={draft().kind === kind}
                class={draft().kind === kind ? "active" : ""}
                onClick={() => {
                  setDraft({ ...blank(kind), name: draft().name });
                  setError("");
                }}
              >
                {connectionKindLabels[kind]}
              </button>
            )}
          </For>
        </div>

        <form
          onSubmit={(event) => {
            event.preventDefault();
            void connect(draft());
          }}
        >
          <div class="form-grid">
            <label class="wide">
              Name
              <input
                maxLength={80}
                value={draft().name}
                placeholder={
                  draft().kind === "serial" ? "Dev board" : "Office router"
                }
                onInput={(event) => update({ name: event.currentTarget.value })}
              />
            </label>

            <Show
              when={
                draft().kind === "telnet" ||
                draft().kind === "vnc" ||
                draft().kind === "rdp"
              }
            >
              <label>
                Host
                <input
                  required
                  value={draft().host ?? ""}
                  placeholder="192.168.1.1"
                  onInput={(event) =>
                    update({ host: event.currentTarget.value.trim() })
                  }
                />
              </label>
              <label>
                Port
                <input
                  type="number"
                  min="1"
                  max="65535"
                  value={draft().port ?? ""}
                  onInput={(event) =>
                    update({ port: Number(event.currentTarget.value) })
                  }
                />
              </label>
            </Show>
            <Show when={draft().kind === "vnc" || draft().kind === "rdp"}>
              <label class="wide">
                User name (optional)
                <input
                  value={draft().username ?? ""}
                  placeholder={
                    draft().kind === "rdp" ? "DOMAIN\\user" : "Asked on connect"
                  }
                  onInput={(event) =>
                    update({ username: event.currentTarget.value.trim() })
                  }
                />
              </label>
            </Show>

            <Show when={draft().kind === "serial"}>
              <label>
                Device
                <span class="inline-field">
                  <select
                    value={draft().device ?? ""}
                    onChange={(event) =>
                      update({ device: event.currentTarget.value })
                    }
                  >
                    <option value="">Choose a port…</option>
                    <For each={devices()}>
                      {(device) => <option value={device}>{device}</option>}
                    </For>
                  </select>
                  <button
                    type="button"
                    class="icon-button"
                    title="Refresh serial devices"
                    onClick={() => void refreshDevices()}
                  >
                    <RefreshCw size={15} />
                  </button>
                </span>
              </label>
              <label>
                Speed
                <select
                  value={String(draft().baud ?? 115200)}
                  onChange={(event) =>
                    update({ baud: Number(event.currentTarget.value) })
                  }
                >
                  <For each={serialBaudRates}>
                    {(rate) => <option value={rate}>{rate} baud</option>}
                  </For>
                </select>
              </label>
            </Show>

            <Show when={draft().kind === "mosh" || draft().kind === "x11"}>
              <label class="wide">
                SSH host
                <select
                  value={draft().profileId ?? ""}
                  onChange={(event) =>
                    update({ profileId: event.currentTarget.value })
                  }
                >
                  <option value="">Choose a saved SSH host…</option>
                  <For each={sshProfiles()}>
                    {(profile) => (
                      <option value={profile.id}>
                        {profile.name} · {profileAddress(profile)}
                      </option>
                    )}
                  </For>
                </select>
              </label>
            </Show>
            <Show when={draft().kind === "vnc"}>
              <label class="wide">
                Through SSH host (optional)
                <select
                  value={draft().profileId ?? ""}
                  onChange={(event) =>
                    update({
                      profileId: event.currentTarget.value || undefined,
                    })
                  }
                >
                  <option value="">Connect directly</option>
                  <For each={sshProfiles()}>
                    {(profile) => (
                      <option value={profile.id}>
                        {profile.name} · {profileAddress(profile)}
                      </option>
                    )}
                  </For>
                </select>
              </label>
            </Show>
            <Show when={draft().kind === "x11"}>
              <label class="wide">
                Remote command (optional)
                <input
                  maxLength={1024}
                  value={draft().command ?? ""}
                  placeholder="xclock — leave empty for a shell"
                  onInput={(event) =>
                    update({ command: event.currentTarget.value })
                  }
                />
              </label>
            </Show>
          </div>
          <p class="muted connect-hint">
            {draft().kind === "telnet"
              ? "Telnet sends everything, including passwords, unencrypted. Use it only on trusted networks."
              : draft().kind === "serial"
                ? "Uses 8 data bits, no parity, 1 stop bit, and no flow control."
                : draft().kind === "vnc"
                  ? "Opens in Weave. VNC itself is not encrypted, so choose an SSH host to tunnel through on untrusted networks; the host connects to the VNC address on your behalf (use localhost for a screen on the SSH host itself)."
                  : draft().kind === "rdp"
                    ? "Opens in Microsoft's Windows App, which asks for the password."
                    : draft().kind === "mosh"
                      ? "Mosh keeps the session alive across sleep and network changes."
                      : "Forwards X11 windows from the host to XQuartz."}
          </p>
          <Show when={toolWarning()}>
            <p class="form-error">{toolWarning()}</p>
          </Show>
          <Show when={error()}>
            <p class="form-error" role="alert">
              {error()}
            </p>
          </Show>
          <div class="dialog-actions">
            <button
              type="button"
              class="ghost-button"
              disabled={!draft().name.trim()}
              onClick={save}
            >
              Save
            </button>
            <span class="spacer" />
            <button
              type="button"
              class="ghost-button"
              onClick={() => props.onClose()}
            >
              Cancel
            </button>
            <Show when={draft().kind === "vnc"}>
              <button
                type="button"
                class="ghost-button"
                disabled={busy()}
                onClick={() => void connect(draft(), true)}
              >
                Open in Screen Sharing
              </button>
            </Show>
            <button type="submit" class="primary-button" disabled={busy()}>
              Connect
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
