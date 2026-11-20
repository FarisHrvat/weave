import { createSignal, For, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { errorMessage } from "./api";
import type { Profile } from "./types";
import { describeTeamSync, type TeamReport as Report } from "./teamVault";

type Status = { path: string | null; shared: string[]; hasPassphrase: boolean };

type Props = {
  profiles: Profile[];
  onChanged: () => void;
  onNotify: (message: string, error?: boolean) => void;
};

export default function TeamVaultPanel(props: Props) {
  const [status, setStatus] = createSignal<Status>();
  const [passphrase, setPassphrase] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const load = async () => setStatus(await invoke<Status>("team_vault_status"));
  onMount(() => void load().catch((reason) => setError(errorMessage(reason))));

  async function act(job: () => Promise<void>) {
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

  const syncNow = () =>
    act(async () => {
      const report = await invoke<Report | null>("team_vault_sync");
      await load();
      props.onChanged();
      if (report) props.onNotify(describeTeamSync(report));
    });

  const connect = (create: boolean) =>
    act(async () => {
      const path = create
        ? await save({
            defaultPath: "Team.weaveteam",
            filters: [{ name: "Weave team vault", extensions: ["weaveteam"] }],
          })
        : await open({
            multiple: false,
            filters: [{ name: "Weave team vault", extensions: ["weaveteam"] }],
          });
      if (typeof path !== "string") return;
      await invoke("team_vault_configure", {
        path,
        passphrase: passphrase() || null,
      });
      setPassphrase("");
      await load();
      await syncNow();
    });

  const shared = (id: string) => status()?.shared.includes(id) ?? false;

  return (
    <section class="team-vault">
      <h3 class="server-repos-heading">Team vault</h3>
      <p class="muted">
        Share hosts with your team through a folder you already sync (iCloud
        Drive, Dropbox, OneDrive or a network share). Everyone uses the same
        team passphrase; changes are merged host by host every two minutes.
        Passwords and keys are never shared.
      </p>
      <Show
        when={status()?.path}
        fallback={
          <div class="form-grid">
            <label class="wide">
              Team passphrase
              <input
                type="password"
                autocomplete="new-password"
                value={passphrase()}
                onInput={(event) => setPassphrase(event.currentTarget.value)}
              />
            </label>
            <div class="wide inline-field">
              <button
                class="ghost-button"
                disabled={busy() || !passphrase()}
                onClick={() => void connect(false)}
              >
                Join existing…
              </button>
              <button
                class="primary-button"
                disabled={busy() || passphrase().length < 10}
                onClick={() => void connect(true)}
              >
                Create team vault…
              </button>
            </div>
          </div>
        }
      >
        {(path) => (
          <>
            <p class="muted team-vault-path">
              <code>{path()}</code>
            </p>
            <div class="team-vault-hosts">
              <For
                each={props.profiles}
                fallback={<p class="muted">Add hosts first.</p>}
              >
                {(profile) => (
                  <label class="profile-favorite">
                    <input
                      type="checkbox"
                      checked={shared(profile.id)}
                      disabled={busy()}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked;
                        void act(async () => {
                          await invoke("team_vault_share", {
                            profileId: profile.id,
                            shared: checked,
                          });
                          await load();
                        });
                      }}
                    />
                    <span class="truncate">{profile.name}</span>
                  </label>
                )}
              </For>
            </div>
            <div class="inline-field">
              <button
                class="primary-button"
                disabled={busy()}
                onClick={() => void syncNow()}
              >
                {busy() ? "Syncing…" : "Sync now"}
              </button>
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() =>
                  void act(async () => {
                    await invoke("team_vault_disconnect");
                    await load();
                  })
                }
              >
                Leave team vault
              </button>
            </div>
          </>
        )}
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </section>
  );
}
