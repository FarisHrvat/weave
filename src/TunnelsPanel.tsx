import { createSignal, For, onMount, Show } from "solid-js";
import { ThisComputer, thisComputer } from "./platform";
import { GitBranch, RefreshCw } from "lucide-solid";
import { api, errorMessage } from "./api";
import type { SshTunnel } from "./types";
import { loadTunnelPresets, saveTunnelPresets } from "./tunnelPresets";
import type { TunnelPreset } from "./tunnelPresets";

type Props = {
  profileId: string;
  onNotice: (message: string) => void;
};

export default function TunnelsPanel(props: Props) {
  const [tunnels, setTunnels] = createSignal<SshTunnel[]>([]);
  const [kind, setKind] = createSignal<SshTunnel["kind"]>("local");
  const [listenPort, setListenPort] = createSignal(8080);
  const [targetHost, setTargetHost] = createSignal("127.0.0.1");
  const [targetPort, setTargetPort] = createSignal(80);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [presets, setPresets] = createSignal<TunnelPreset[]>([]);
  const [presetName, setPresetName] = createSignal("");

  async function refresh() {
    try {
      setTunnels(await api.tunnels());
      setError("");
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  onMount(() => {
    void refresh();
    try {
      setPresets(loadTunnelPresets(localStorage));
    } catch (reason) {
      setError(errorMessage(reason));
    }
  });

  async function start(preset?: TunnelPreset) {
    setBusy(true);
    setError("");
    try {
      await api.startTunnel(
        props.profileId,
        preset?.kind ?? kind(),
        preset?.listenPort ?? listenPort(),
        (preset?.kind ?? kind()) === "dynamic"
          ? ""
          : (preset?.targetHost ?? targetHost()),
        (preset?.kind ?? kind()) === "dynamic"
          ? 0
          : (preset?.targetPort ?? targetPort()),
      );
      await refresh();
      props.onNotice(
        `${(preset?.kind ?? kind()) === "remote" ? "Remote host" : ThisComputer}: SSH tunnel is listening on 127.0.0.1:${preset?.listenPort ?? listenPort()}.`,
      );
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  function savePreset() {
    const name = presetName().trim();
    if (!name) {
      setError("Enter a name to save this tunnel.");
      return;
    }
    if (
      !Number.isInteger(listenPort()) ||
      listenPort() < 1 ||
      listenPort() > 65535 ||
      (kind() !== "dynamic" &&
        (!targetHost().trim() ||
          !Number.isInteger(targetPort()) ||
          targetPort() < 1 ||
          targetPort() > 65535))
    ) {
      setError(
        "Enter a valid listening port, destination host and destination port before saving.",
      );
      return;
    }
    const preset: TunnelPreset = {
      id: crypto.randomUUID(),
      profileId: props.profileId,
      name,
      kind: kind(),
      listenPort: listenPort(),
      targetHost: kind() === "dynamic" ? "" : targetHost().trim(),
      targetPort: kind() === "dynamic" ? 0 : targetPort(),
    };
    try {
      const next = [...presets(), preset];
      saveTunnelPresets(localStorage, next);
      setPresets(next);
      setPresetName("");
      setError("");
      props.onNotice("SSH tunnel preset saved.");
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  function toggleAutoStart(id: string) {
    try {
      const next = presets().map((item) =>
        item.id === id ? { ...item, autoStart: !item.autoStart } : item,
      );
      saveTunnelPresets(localStorage, next);
      setPresets(next);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  function deletePreset(id: string) {
    try {
      const next = presets().filter((item) => item.id !== id);
      saveTunnelPresets(localStorage, next);
      setPresets(next);
      setError("");
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  async function stop(id: string) {
    setBusy(true);
    setError("");
    try {
      await api.stopTunnel(id);
      await refresh();
      props.onNotice("SSH tunnel stopped.");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="tunnels-panel wide" aria-label="SSH tunnels">
      <div class="tailnet-head">
        <GitBranch size={16} />
        <div>
          <strong>SSH tunnels</strong>
          <small>Forward a local or remote port, or open a SOCKS proxy.</small>
        </div>
        <button
          class="mini-button"
          type="button"
          title="Refresh tunnels"
          aria-label="Refresh tunnels"
          onClick={() => void refresh()}
        >
          <RefreshCw size={14} />
        </button>
      </div>
      <div class="tunnels-fields">
        <label>
          Direction
          <select
            value={kind()}
            onChange={(event) =>
              setKind(event.currentTarget.value as SshTunnel["kind"])
            }
          >
            <option value="local">Local port</option>
            <option value="remote">Remote port</option>
            <option value="dynamic">SOCKS proxy</option>
          </select>
        </label>
        <label>
          Listen port
          <input
            type="number"
            min="1"
            max="65535"
            value={listenPort()}
            onInput={(event) =>
              setListenPort(Number(event.currentTarget.value))
            }
          />
        </label>
        <Show when={kind() !== "dynamic"}>
          <label>
            Destination host
            <input
              value={targetHost()}
              onInput={(event) => setTargetHost(event.currentTarget.value)}
              placeholder="127.0.0.1"
            />
          </label>
          <label>
            Destination port
            <input
              type="number"
              min="1"
              max="65535"
              value={targetPort()}
              onInput={(event) =>
                setTargetPort(Number(event.currentTarget.value))
              }
            />
          </label>
        </Show>
      </div>
      <p class="tailnet-guidance">
        The listening address is limited to 127.0.0.1 on {thisComputer} or the
        remote host. SSH key or agent authentication and an already trusted host
        key are required. Tunnels use the last saved host settings.
      </p>
      <button
        class="mini-button"
        type="button"
        disabled={busy()}
        onClick={() => void start()}
      >
        Start tunnel
      </button>
      <div class="tunnel-preset-entry">
        <input
          aria-label="Tunnel preset name"
          value={presetName()}
          onInput={(event) => setPresetName(event.currentTarget.value)}
          placeholder="Save as…"
        />
        <button class="mini-button" type="button" onClick={savePreset}>
          Save preset
        </button>
      </div>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <For
        each={tunnels().filter(
          (tunnel) => tunnel.profileId === props.profileId,
        )}
      >
        {(tunnel) => (
          <div class="tailnet-peer">
            <div>
              <strong>
                {tunnel.kind === "dynamic"
                  ? "SOCKS"
                  : tunnel.kind === "remote"
                    ? "Remote"
                    : "Local"}{" "}
                · {tunnel.listenPort}
              </strong>
              <small>
                {tunnel.targetHost
                  ? `→ ${tunnel.targetHost}:${tunnel.targetPort}`
                  : "SOCKS proxy"}
              </small>
            </div>
            <button
              class="mini-button"
              type="button"
              disabled={busy()}
              onClick={() => void stop(tunnel.id)}
            >
              Stop
            </button>
          </div>
        )}
      </For>
      <For
        each={presets().filter(
          (preset) => preset.profileId === props.profileId,
        )}
      >
        {(preset) => (
          <div class="tailnet-peer">
            <div>
              <strong>{preset.name}</strong>
              <small>
                {preset.kind} · {preset.listenPort}
                {preset.kind !== "dynamic"
                  ? ` → ${preset.targetHost}:${preset.targetPort}`
                  : ""}
              </small>
            </div>
            <button
              class="mini-button"
              type="button"
              disabled={busy()}
              onClick={() => void start(preset)}
            >
              Start
            </button>
            <label
              class="tunnel-autostart"
              title="Start this tunnel whenever Weave opens"
            >
              <input
                type="checkbox"
                checked={!!preset.autoStart}
                onChange={() => toggleAutoStart(preset.id)}
              />
              Auto-start
            </label>
            <button
              class="mini-button"
              type="button"
              title={`Delete ${preset.name} preset`}
              onClick={() => deletePreset(preset.id)}
            >
              Delete
            </button>
          </div>
        )}
      </For>
    </section>
  );
}
