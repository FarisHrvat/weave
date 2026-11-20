import { createEffect, createSignal, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./api";

type HostOptions = {
  keepaliveSeconds: number;
  agentForwarding: boolean;
  compression: boolean;
  proxyCommand: string;
  environment: string;
  startupCommand: string;
  knockPorts: string;
  knockDelayMs: number;
};

const blank: HostOptions = {
  keepaliveSeconds: 0,
  agentForwarding: false,
  compression: false,
  proxyCommand: "",
  environment: "",
  startupCommand: "",
  knockPorts: "",
  knockDelayMs: 300,
};

type Props = { profileId: string; onNotice: (message: string) => void };

export default function HostOptionsPanel(props: Props) {
  const [options, setOptions] = createSignal<HostOptions>(blank);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  createEffect(() => {
    const profileId = props.profileId;
    setError("");
    void invoke<HostOptions>("host_options_get", { profileId })
      .then((value) =>
        setOptions({
          ...value,
          knockDelayMs: value.knockDelayMs || blank.knockDelayMs,
        }),
      )
      .catch((reason) => setError(errorMessage(reason)));
  });

  const update = (patch: Partial<HostOptions>) =>
    setOptions((current) => ({ ...current, ...patch }));

  async function save() {
    setBusy(true);
    setError("");
    try {
      await invoke("host_options_set", {
        profileId: props.profileId,
        options: options(),
      });
      props.onNotice(
        "Advanced SSH options saved. They apply to new connections.",
      );
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <details class="host-options wide">
      <summary>Advanced SSH options</summary>
      <div class="form-grid">
        <label>
          Keepalive every (seconds)
          <input
            type="number"
            min="0"
            max="3600"
            value={options().keepaliveSeconds}
            onInput={(event) =>
              update({ keepaliveSeconds: Number(event.currentTarget.value) })
            }
          />
        </label>
        <div class="host-options-checks">
          <label class="profile-favorite">
            <input
              type="checkbox"
              checked={options().agentForwarding}
              onChange={(event) =>
                update({ agentForwarding: event.currentTarget.checked })
              }
            />
            Forward SSH agent
          </label>
          <label class="profile-favorite">
            <input
              type="checkbox"
              checked={options().compression}
              onChange={(event) =>
                update({ compression: event.currentTarget.checked })
              }
            />
            Compression
          </label>
        </div>
        <label class="wide">
          Proxy command <span class="optional">optional</span>
          <input
            value={options().proxyCommand}
            spellcheck={false}
            placeholder="nc -X 5 -x proxy.example.com:1080 %h %p"
            onInput={(event) =>
              update({ proxyCommand: event.currentTarget.value })
            }
          />
        </label>
        <label class="wide">
          Environment variables{" "}
          <span class="optional">one NAME=value per line</span>
          <textarea
            rows="2"
            spellcheck={false}
            value={options().environment}
            placeholder={"LANG=en_US.UTF-8"}
            onInput={(event) =>
              update({ environment: event.currentTarget.value })
            }
          />
          <small class="muted">
            The server must allow them (AcceptEnv in sshd_config).
          </small>
        </label>
        <label class="wide">
          Startup command <span class="optional">runs before the shell</span>
          <input
            value={options().startupCommand}
            spellcheck={false}
            placeholder="tmux attach || tmux"
            onInput={(event) =>
              update({ startupCommand: event.currentTarget.value })
            }
          />
        </label>
        <label>
          Port knock sequence <span class="optional">optional</span>
          <input
            value={options().knockPorts}
            spellcheck={false}
            placeholder="7000, 8000, 9000/udp"
            onInput={(event) =>
              update({ knockPorts: event.currentTarget.value })
            }
          />
        </label>
        <label>
          Delay between knocks (ms)
          <input
            type="number"
            min="0"
            max="10000"
            value={options().knockDelayMs}
            onInput={(event) =>
              update({ knockDelayMs: Number(event.currentTarget.value) })
            }
          />
        </label>
        <div class="wide">
          <button
            type="button"
            class="server-action"
            disabled={busy()}
            onClick={() => void save()}
          >
            {busy() ? "Saving…" : "Save advanced options"}
          </button>
        </div>
      </div>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </details>
  );
}
