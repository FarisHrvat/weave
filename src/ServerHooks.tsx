import { confirmAction } from "./dialogs";
import { createSignal, For, onMount, Show } from "solid-js";
import { api, errorMessage } from "./api";

const hookNames = [
  "pre-receive",
  "update",
  "post-receive",
  "post-update",
  "push-to-checkout",
  "reference-transaction",
];

type Props = {
  profileId: string | null;
  path: string;
  onNotify: (message: string) => void;
};

export default function ServerHooks(props: Props) {
  const [installed, setInstalled] = createSignal<string[]>([]);
  const [hook, setHook] = createSignal("post-receive");
  const [content, setContent] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  async function attempt(job: () => Promise<void>) {
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

  const load = (name: string) =>
    attempt(async () => {
      setHook(name);
      const text = await api.gitRemoteHookRead(
        props.profileId,
        props.path,
        name,
      );
      setContent(text || "#!/bin/sh\n");
    });

  onMount(
    () =>
      void attempt(async () => {
        setInstalled(await api.gitRemoteHooks(props.profileId, props.path));
        await load(hook());
      }),
  );

  const save = () =>
    attempt(async () => {
      await api.gitRemoteHookSave(
        props.profileId,
        props.path,
        hook(),
        content(),
      );
      setInstalled(await api.gitRemoteHooks(props.profileId, props.path));
      props.onNotify(`Saved the ${hook()} hook.`);
    });

  const remove = async () => {
    if (
      !(await confirmAction(`Remove the ${hook()} hook from this repository?`))
    )
      return;
    void attempt(async () => {
      await api.gitRemoteHookDelete(props.profileId, props.path, hook());
      setInstalled(await api.gitRemoteHooks(props.profileId, props.path));
      setContent("#!/bin/sh\n");
      props.onNotify(`Removed the ${hook()} hook.`);
    });
  };

  return (
    <div class="wide server-hooks">
      <label>
        Server hook
        <select
          value={hook()}
          disabled={busy()}
          onChange={(event) => void load(event.currentTarget.value)}
        >
          <For each={hookNames}>
            {(name) => (
              <option value={name}>
                {name}
                {installed().includes(name) ? " (installed)" : ""}
              </option>
            )}
          </For>
        </select>
      </label>
      <textarea
        aria-label={`${hook()} hook script`}
        spellcheck={false}
        value={content()}
        onInput={(event) => setContent(event.currentTarget.value)}
      />
      <div class="server-hooks-actions">
        <button
          type="button"
          class="server-action"
          disabled={busy()}
          onClick={() => void save()}
        >
          Save hook
        </button>
        <Show when={installed().includes(hook())}>
          <button
            type="button"
            class="server-action danger-text"
            disabled={busy()}
            onClick={remove}
          >
            Remove hook
          </button>
        </Show>
      </div>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
