import { createEffect, createSignal, Show } from "solid-js";
import { api, errorMessage } from "./api";

type Props = { host: string };
type Operation = "ping" | "traceroute" | "port" | "wake";

export default function NetworkToolsPanel(props: Props) {
  const [host, setHost] = createSignal(props.host);
  const [port, setPort] = createSignal("22");
  const [mac, setMac] = createSignal("");
  const [broadcast, setBroadcast] = createSignal("255.255.255.255");
  const [wakePort, setWakePort] = createSignal("9");
  const [busy, setBusy] = createSignal<Operation | null>(null);
  const [output, setOutput] = createSignal("");
  const [error, setError] = createSignal("");

  createEffect(() => setHost(props.host));

  function parsePort(value: string): number {
    const number = Number(value);
    if (!Number.isInteger(number) || number < 1 || number > 65535) {
      throw new Error("Enter a port between 1 and 65535.");
    }
    return number;
  }

  async function run(operation: Operation) {
    if (busy()) return;
    setError("");
    setOutput("");
    setBusy(operation);
    try {
      const result =
        operation === "ping"
          ? await api.networkPing(host())
          : operation === "traceroute"
            ? await api.networkTraceroute(host())
            : operation === "port"
              ? await api.networkCheckPort(host(), parsePort(port()))
              : await api.networkWake(
                  mac(),
                  broadcast(),
                  parsePort(wakePort()),
                );
      setOutput(result);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <details class="tailnet-panel wide">
      <summary>Network tools</summary>
      <p class="muted">
        Diagnose a host or wake a device on a reachable local or VPN network.
      </p>
      <div class="network-tools-fields">
        <label>
          Host or IP address
          <input
            value={host()}
            placeholder="nas.example.com"
            onInput={(event) => setHost(event.currentTarget.value)}
          />
        </label>
        <label>
          TCP port
          <input
            type="number"
            min="1"
            max="65535"
            value={port()}
            onInput={(event) => setPort(event.currentTarget.value)}
          />
        </label>
      </div>
      <div class="external-online-toolbar">
        <button
          type="button"
          disabled={!!busy()}
          onClick={() => void run("ping")}
        >
          Ping
        </button>
        <button
          type="button"
          disabled={!!busy()}
          onClick={() => void run("traceroute")}
        >
          Traceroute
        </button>
        <button
          type="button"
          disabled={!!busy()}
          onClick={() => void run("port")}
        >
          Check TCP port
        </button>
      </div>
      <div class="network-tools-fields">
        <label>
          Device MAC address
          <input
            value={mac()}
            placeholder="00:11:22:33:44:55"
            onInput={(event) => setMac(event.currentTarget.value)}
          />
        </label>
        <label>
          Broadcast address
          <input
            value={broadcast()}
            onInput={(event) => setBroadcast(event.currentTarget.value)}
          />
        </label>
        <label>
          UDP port
          <input
            type="number"
            min="1"
            max="65535"
            value={wakePort()}
            onInput={(event) => setWakePort(event.currentTarget.value)}
          />
        </label>
      </div>
      <button
        type="button"
        disabled={!!busy()}
        onClick={() => void run("wake")}
      >
        Send Wake-on-LAN
      </button>
      <Show when={busy()}>
        <p class="muted" role="status">
          Running {busy()}…
        </p>
      </Show>
      <Show when={error()}>
        <p class="error-banner" role="alert">
          {error()}
        </p>
      </Show>
      <Show when={output()}>
        <pre class="network-tools-output" role="status">
          {output()}
        </pre>
      </Show>
    </details>
  );
}
