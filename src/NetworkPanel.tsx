import { confirmAction } from "./dialogs";
import { os, systemName, thisComputer } from "./platform";
import { createSignal, For, onMount, Show } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { open } from "@tauri-apps/plugin-dialog";
import { Network, RefreshCw } from "lucide-solid";
import { api, errorMessage } from "./api";
import type {
  TailnetPeer,
  TailnetStatus,
  VpnCommandProfile,
  VpnCommandStatus,
  VpnService,
} from "./types";

const newCommand = (): VpnCommandProfile => ({
  id: "",
  name: "",
  executable: "",
  connectArgs: [],
  disconnectArgs: [],
  statusArgs: [],
  connectedText: "",
});

const lines = (value: string) =>
  value.split("\n").filter((part) => part.length > 0);

type Props = {
  host: string;
  port: number;
  onSelect: (peer: TailnetPeer) => void;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
};

export default function NetworkPanel(props: Props) {
  const [status, setStatus] = createSignal<TailnetStatus>();
  const [error, setError] = createSignal("");
  const [loading, setLoading] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [checking, setChecking] = createSignal("");
  const [vpns, setVpns] = createSignal<VpnService[]>([]);
  const [vpnError, setVpnError] = createSignal("");
  const [vpnLoading, setVpnLoading] = createSignal(true);
  const [vpnBusy, setVpnBusy] = createSignal("");
  const [commands, setCommands] = createSignal<VpnCommandStatus[]>([]);
  const [commandError, setCommandError] = createSignal("");
  const [commandBusy, setCommandBusy] = createSignal("");
  const [editing, setEditing] = createSignal<VpnCommandProfile | null>(null);

  async function refreshCommands() {
    setCommandError("");
    try {
      setCommands(await api.vpnCommandsList());
    } catch (error) {
      setCommandError(errorMessage(error));
    }
  }

  async function refreshVpns() {
    setVpnLoading(true);
    setVpnError("");
    try {
      setVpns(await api.vpnList());
    } catch (error) {
      setVpnError(errorMessage(error));
    } finally {
      setVpnLoading(false);
    }
  }

  async function refresh() {
    setLoading(true);
    setError("");
    try {
      setStatus(await api.tailnetStatus());
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setLoading(false);
    }
  }

  onMount(() => {
    void refresh();
    void refreshVpns();
    void refreshCommands();
  });

  async function saveCommand() {
    const profile = editing();
    if (!profile) return;
    setCommandBusy("save");
    setCommandError("");
    try {
      await api.vpnCommandsSave(profile);
      setEditing(null);
      await refreshCommands();
      props.onNotice("VPN command saved.");
    } catch (error) {
      setCommandError(errorMessage(error));
    } finally {
      setCommandBusy("");
    }
  }

  async function chooseExecutable() {
    try {
      const selected = await open({
        title: "Choose VPN command",
        directory: false,
        multiple: false,
      });
      if (typeof selected === "string") {
        setEditing((current) =>
          current ? { ...current, executable: selected } : null,
        );
      }
    } catch (error) {
      setCommandError(errorMessage(error));
    }
  }

  async function deleteCommand(item: VpnCommandStatus) {
    if (!(await confirmAction(`Remove ${item.profile.name} from Weave?`)))
      return;
    setCommandBusy(item.profile.id);
    setCommandError("");
    try {
      await api.vpnCommandsDelete(item.profile.id);
      await refreshCommands();
    } catch (error) {
      setCommandError(errorMessage(error));
    } finally {
      setCommandBusy("");
    }
  }

  async function runCommand(item: VpnCommandStatus, connect: boolean) {
    setCommandBusy(item.profile.id);
    setCommandError("");
    try {
      const result = await api.vpnCommandsRun(item.profile.id, connect);
      setCommands((current) =>
        current.map((entry) =>
          entry.profile.id === result.profile.id ? result : entry,
        ),
      );
      props.onNotice(
        `${item.profile.name}: ${connect ? "connect" : "disconnect"} command completed. ` +
          (result.state === "Unknown"
            ? "Check the NAS address to confirm the network is usable."
            : `Status: ${result.state}.`),
      );
    } catch (error) {
      setCommandError(errorMessage(error));
    } finally {
      setCommandBusy("");
    }
  }

  async function changeVpn(service: VpnService, connect: boolean) {
    if (
      !connect &&
      !(await confirmAction(
        `Disconnect ${service.name} for ${thisComputer}? Other apps using this VPN will lose access too.`,
      ))
    )
      return;
    setVpnBusy(service.id);
    setVpnError("");
    try {
      const next = connect
        ? await api.vpnConnect(service.id)
        : await api.vpnDisconnect(service.id);
      setVpns(next);
      const current = next.find((item) => item.id === service.id);
      props.onNotice(
        `${service.name}: ${current?.state ?? "state unavailable"}.` +
          (connect && current?.state !== "Connected"
            ? ` If prompted, finish signing in through ${systemName} or your VPN provider, then refresh.`
            : ""),
      );
    } catch (error) {
      setVpnError(errorMessage(error));
    } finally {
      setVpnBusy("");
    }
  }

  async function checkEnteredHost() {
    setChecking("host");
    try {
      props.onNotice(await api.vpnCheckSsh(props.host, props.port));
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setChecking("");
    }
  }

  async function changeConnection(connect: boolean) {
    if (
      !connect &&
      !(await confirmAction(
        `Disconnect Tailscale for ${thisComputer}? Other apps using the tailnet will lose access too.`,
      ))
    )
      return;
    setBusy(true);
    setError("");
    try {
      const next = connect
        ? await api.tailnetConnect()
        : await api.tailnetDisconnect();
      setStatus(next);
      props.onNotice(
        connect
          ? next.state === "Running"
            ? "Tailscale connected."
            : "Finish sign-in in Tailscale, then refresh."
          : "Tailscale disconnected.",
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function check(peer: TailnetPeer) {
    setChecking(peer.ip);
    try {
      props.onNotice(await api.tailnetCheckSsh(peer.ip, props.port));
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setChecking("");
    }
  }

  return (
    <section
      class="tailnet-panel wide"
      aria-label="Private network connections"
    >
      <div class="tailnet-head">
        <Network size={16} />
        <div>
          <strong>VPN connections</strong>
          <small>
            Use a VPN configured on {thisComputer}, then connect to the NAS
            address above.
          </small>
        </div>
        <button
          type="button"
          class="mini-button"
          title="Refresh VPN services"
          aria-label="Refresh VPN services"
          disabled={vpnLoading() || Boolean(vpnBusy())}
          onClick={() => void refreshVpns()}
        >
          <RefreshCw size={14} />
        </button>
      </div>
      <Show when={vpnError()}>
        <p class="tailnet-error" role="alert">
          {vpnError()}
        </p>
      </Show>
      <Show when={!vpnLoading()}>
        <For
          each={vpns().filter(
            (service) => !service.kind.toLowerCase().includes("tailscale"),
          )}
          fallback={
            <p class="tailnet-guidance">
              {os === "mac"
                ? "No macOS VPN service is configured. Add one in System Settings → VPN, use Tailscale below, or configure another provider’s command in the advanced section."
                : os === "windows"
                  ? "No VPN is set up in Windows. Add one in Settings → Network & internet → VPN, use Tailscale below, or configure another provider’s command in the advanced section."
                  : "No NetworkManager VPN or WireGuard connection is set up. Add one in your network settings, use Tailscale below, or configure another provider’s command in the advanced section."}
            </p>
          }
        >
          {(service) => (
            <div class="tailnet-peer">
              <span
                class={
                  service.state === "Connected"
                    ? "tailnet-dot online"
                    : "tailnet-dot"
                }
              />
              <div>
                <strong>{service.name}</strong>
                <small>
                  {service.state} · {service.kind}
                </small>
              </div>
              <button
                type="button"
                class="tailnet-link"
                disabled={
                  Boolean(vpnBusy()) ||
                  !["Connected", "Disconnected"].includes(service.state) ||
                  (service.state !== "Connected" && !service.enabled)
                }
                title={
                  !service.enabled
                    ? `Enable this VPN in ${systemName} network settings`
                    : undefined
                }
                onClick={() =>
                  void changeVpn(service, service.state !== "Connected")
                }
              >
                {vpnBusy() === service.id
                  ? "Working…"
                  : service.state === "Connected"
                    ? "Disconnect"
                    : service.state === "Disconnected"
                      ? "Connect"
                      : service.state}
              </button>
            </div>
          )}
        </For>
      </Show>
      <details class="vpn-advanced">
        <summary>Other VPN providers</summary>
        <p class="tailnet-guidance">
          If your VPN is not listed above, you can control its installed
          command-line tool here. It must have commands that return after
          connecting or disconnecting. Weave runs the executable directly
          without a shell. Do not put passwords in arguments.
        </p>
        <Show when={commandError()}>
          <p class="tailnet-error" role="alert">
            {commandError()}
          </p>
        </Show>
        <For each={commands()}>
          {(item) => (
            <div class="tailnet-peer">
              <span
                class={
                  item.state === "Connected"
                    ? "tailnet-dot online"
                    : "tailnet-dot"
                }
              />
              <div>
                <strong>{item.profile.name}</strong>
                <small>{item.state}</small>
              </div>
              <Show when={item.state !== "Connected"}>
                <button
                  type="button"
                  class="tailnet-link"
                  disabled={Boolean(commandBusy())}
                  onClick={() => void runCommand(item, true)}
                >
                  Connect
                </button>
              </Show>
              <Show when={item.state !== "Disconnected"}>
                <button
                  type="button"
                  class="tailnet-link"
                  disabled={Boolean(commandBusy())}
                  onClick={() => void runCommand(item, false)}
                >
                  Disconnect
                </button>
              </Show>
              <button
                type="button"
                class="tailnet-link"
                disabled={Boolean(commandBusy())}
                onClick={() => setEditing({ ...item.profile })}
              >
                Edit
              </button>
              <button
                type="button"
                class="tailnet-link"
                disabled={Boolean(commandBusy())}
                onClick={() => void deleteCommand(item)}
              >
                Remove
              </button>
            </div>
          )}
        </For>
        <Show when={!editing()}>
          <button
            type="button"
            class="ghost-button"
            onClick={() => setEditing(newCommand())}
          >
            Add VPN command
          </button>
        </Show>
        <Show when={editing()}>
          {(profile) => (
            <div class="vpn-command-form">
              <label>
                Name
                <input
                  value={profile().name}
                  onInput={(event) =>
                    setEditing({
                      ...profile(),
                      name: event.currentTarget.value,
                    })
                  }
                  placeholder="My VPN"
                />
              </label>
              <label>
                Executable path
                <span class="vpn-path-field">
                  <input
                    value={profile().executable}
                    onInput={(event) =>
                      setEditing({
                        ...profile(),
                        executable: event.currentTarget.value,
                      })
                    }
                    placeholder="/opt/homebrew/bin/my-vpn"
                  />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => void chooseExecutable()}
                  >
                    Browse
                  </button>
                </span>
              </label>
              <label>
                Connect arguments <small>one argument per line</small>
                <textarea
                  rows="2"
                  value={profile().connectArgs.join("\n")}
                  onInput={(event) =>
                    setEditing({
                      ...profile(),
                      connectArgs: lines(event.currentTarget.value),
                    })
                  }
                  placeholder="connect"
                />
              </label>
              <label>
                Disconnect arguments <small>one argument per line</small>
                <textarea
                  rows="2"
                  value={profile().disconnectArgs.join("\n")}
                  onInput={(event) =>
                    setEditing({
                      ...profile(),
                      disconnectArgs: lines(event.currentTarget.value),
                    })
                  }
                  placeholder="disconnect"
                />
              </label>
              <label>
                Status arguments{" "}
                <small>optional; exit code 0 means connected</small>
                <textarea
                  rows="2"
                  value={profile().statusArgs.join("\n")}
                  onInput={(event) =>
                    setEditing({
                      ...profile(),
                      statusArgs: lines(event.currentTarget.value),
                    })
                  }
                  placeholder="status"
                />
              </label>
              <label>
                Connected text{" "}
                <small>optional; must appear in status output</small>
                <input
                  value={profile().connectedText}
                  onInput={(event) =>
                    setEditing({
                      ...profile(),
                      connectedText: event.currentTarget.value,
                    })
                  }
                  placeholder="Connected"
                />
              </label>
              <div class="tailnet-actions">
                <button
                  type="button"
                  class="primary-button"
                  disabled={Boolean(commandBusy())}
                  onClick={() => void saveCommand()}
                >
                  {commandBusy() === "save" ? "Saving…" : "Save VPN command"}
                </button>
                <button
                  type="button"
                  class="ghost-button"
                  disabled={Boolean(commandBusy())}
                  onClick={() => setEditing(null)}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
        </Show>
        <button
          type="button"
          class="tailnet-link"
          disabled={Boolean(commandBusy())}
          onClick={() => void refreshCommands()}
        >
          Refresh command status
        </button>
      </details>
      <div class="tailnet-actions">
        <button
          type="button"
          class="ghost-button"
          disabled={Boolean(checking()) || !props.host.trim()}
          onClick={() => void checkEnteredHost()}
        >
          {checking() === "host" ? "Checking…" : "Check NAS SSH"}
        </button>
      </div>
      <p class="tailnet-note">
        The check confirms only that the SSH port is reachable. Your NAS account
        is verified when you connect.
      </p>
      <div class="tailnet-divider" />
      <div class="tailnet-head">
        <Network size={16} />
        <div>
          <strong>Tailscale</strong>
          <small>
            {loading()
              ? `Checking ${thisComputer}…`
              : status()?.installed
                ? status()?.state === "Running"
                  ? `Connected${status()?.deviceName ? ` as ${status()?.deviceName}` : ""}`
                  : `Status: ${status()?.state}`
                : `Not installed on ${thisComputer}`}
          </small>
        </div>
        <button
          type="button"
          class="mini-button"
          title="Refresh Tailscale status"
          aria-label="Refresh Tailscale status"
          disabled={loading() || busy()}
          onClick={() => void refresh()}
        >
          <RefreshCw size={14} />
        </button>
      </div>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={!loading()}>
        <Show
          when={status()?.installed}
          fallback={
            <div class="tailnet-guidance">
              <p>
                Install Tailscale on {thisComputer} and your NAS, sign both into
                the same tailnet, and enable SSH on the NAS.
              </p>
              <button
                type="button"
                class="tailnet-link"
                onClick={() =>
                  void openUrl(
                    `https://tailscale.com/download/${os === "mac" ? "mac" : os}`,
                  ).catch((error) => props.onError(errorMessage(error)))
                }
              >
                Get Tailscale for {systemName}
              </button>
            </div>
          }
        >
          <div class="tailnet-actions">
            <Show
              when={status()?.state === "Running"}
              fallback={
                <button
                  type="button"
                  class="primary-button"
                  disabled={busy()}
                  onClick={() => void changeConnection(true)}
                >
                  {busy() ? "Connecting…" : "Connect Tailscale"}
                </button>
              }
            >
              <button
                type="button"
                class="ghost-button"
                disabled={busy()}
                onClick={() => void changeConnection(false)}
              >
                Disconnect Tailscale
              </button>
            </Show>
          </div>
          <Show when={status()?.state === "Running"}>
            <div class="tailnet-peers">
              <For
                each={status()?.peers}
                fallback={
                  <p class="tailnet-guidance">
                    No other devices are visible. Check that the NAS joined your
                    tailnet, then refresh.
                  </p>
                }
              >
                {(peer) => (
                  <div class="tailnet-peer">
                    <span
                      class={peer.online ? "tailnet-dot online" : "tailnet-dot"}
                      title={peer.online ? "Online" : "Offline"}
                    />
                    <div>
                      <strong>{peer.name}</strong>
                      <small>
                        {peer.ip}
                        {peer.dnsName ? ` · ${peer.dnsName}` : ""}
                      </small>
                    </div>
                    <button
                      type="button"
                      class="tailnet-link"
                      onClick={() => props.onSelect(peer)}
                    >
                      Use
                    </button>
                    <button
                      type="button"
                      class="tailnet-link"
                      disabled={!peer.online || Boolean(checking())}
                      onClick={() => void check(peer)}
                    >
                      {checking() === peer.ip ? "Checking…" : "Check SSH"}
                    </button>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </Show>
      <p class="tailnet-note">
        Tailscale is optional. Any working VPN route can reach the NAS if its
        SSH service and firewall allow your connection.
      </p>
    </section>
  );
}
