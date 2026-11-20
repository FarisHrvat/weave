import { createEffect, createSignal, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./api";

export const cloudProtocols = ["dropbox", "gdrive", "onedrive"] as const;
export type CloudProtocol = (typeof cloudProtocols)[number];

export const isCloud = (protocol: string): protocol is CloudProtocol =>
  (cloudProtocols as readonly string[]).includes(protocol);

const providers: Record<
  CloudProtocol,
  { name: string; console: string; steps: string; secret: boolean }
> = {
  dropbox: {
    name: "Dropbox",
    console: "https://www.dropbox.com/developers/apps",
    steps:
      "Create an app with Full Dropbox access, add the redirect URI below, enable the files.content.read, files.content.write and account_info.read permissions, then copy its App key.",
    secret: false,
  },
  gdrive: {
    name: "Google Drive",
    console: "https://console.cloud.google.com/apis/credentials",
    steps:
      "Enable the Google Drive API, create an OAuth client of type Desktop app, then copy its client ID and client secret. Google treats a desktop app's secret as non-confidential, but asks for it.",
    secret: true,
  },
  onedrive: {
    name: "OneDrive",
    console:
      "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
    steps:
      "Register an app for personal and work accounts, add the redirect URI below under Mobile and desktop applications, allow public client flows, then copy its Application (client) ID.",
    secret: false,
  },
};

type CloudApp = { clientId: string; hasSecret: boolean; redirectUri: string };

type Props = {
  protocol: CloudProtocol;
  profileId: string;
  account: string;
  onSignedIn: (email: string) => void;
  onNotice: (message: string) => void;
};

export default function CloudDrivePanel(props: Props) {
  const [app, setApp] = createSignal<CloudApp>();
  const [clientId, setClientId] = createSignal("");
  const [secret, setSecret] = createSignal("");
  const [signedIn, setSignedIn] = createSignal(false);
  const [busy, setBusy] = createSignal("");
  const [error, setError] = createSignal("");
  const provider = () => providers[props.protocol];

  createEffect(() => {
    const protocol = props.protocol;
    const id = props.profileId;
    setError("");
    setSecret("");
    void invoke<CloudApp>("cloud_app_get", { provider: protocol })
      .then((value) => {
        setApp(value);
        setClientId(value.clientId);
      })
      .catch((reason) => setError(errorMessage(reason)));
    if (id)
      void invoke<boolean>("cloud_signed_in", { profileId: id })
        .then(setSignedIn)
        .catch(() => setSignedIn(false));
    else setSignedIn(false);
  });

  async function act(label: string, job: () => Promise<void>) {
    setBusy(label);
    setError("");
    try {
      await job();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy("");
    }
  }

  const saveApp = () =>
    act("save", async () => {
      await invoke("cloud_app_set", {
        provider: props.protocol,
        clientId: clientId(),
        clientSecret: provider().secret ? secret() || null : null,
      });
      setApp(
        await invoke<CloudApp>("cloud_app_get", { provider: props.protocol }),
      );
      setSecret("");
      props.onNotice(`${provider().name} app saved.`);
    });

  return (
    <div class="cloud-drive-panel wide">
      <p class="muted">
        Weave connects to {provider().name} through an app you register with{" "}
        {provider().name}, so your files never pass through anyone else.{" "}
        {provider().steps}
      </p>
      <p class="muted">
        Developer console: <code>{provider().console}</code>
        <br />
        Redirect URI:{" "}
        <code>{app()?.redirectUri ?? "http://localhost:53682/"}</code>
      </p>
      <label>
        {props.protocol === "dropbox" ? "App key" : "Client ID"}
        <input
          value={clientId()}
          spellcheck={false}
          onInput={(event) => setClientId(event.currentTarget.value)}
        />
      </label>
      <Show when={provider().secret}>
        <label>
          Client secret{" "}
          <Show when={app()?.hasSecret}>
            <span class="optional">saved; leave blank to keep</span>
          </Show>
          <input
            type="password"
            value={secret()}
            autocomplete="new-password"
            onInput={(event) => setSecret(event.currentTarget.value)}
          />
        </label>
      </Show>
      <div class="inline-field">
        <button
          type="button"
          class="server-action"
          disabled={!!busy() || !clientId().trim()}
          onClick={() => void saveApp()}
        >
          {busy() === "save" ? "Saving…" : "Save app"}
        </button>
        <Show
          when={props.profileId}
          fallback={
            <span class="muted">Save this connection, then sign in.</span>
          }
        >
          <button
            type="button"
            class="primary-button"
            disabled={!!busy() || !app()?.clientId}
            onClick={() =>
              void act("sign-in", async () => {
                const email = await invoke<string>("cloud_sign_in", {
                  profileId: props.profileId,
                });
                setSignedIn(true);
                props.onSignedIn(email);
                props.onNotice(`Signed in to ${provider().name} as ${email}.`);
              })
            }
          >
            {busy() === "sign-in"
              ? "Waiting for the browser…"
              : signedIn()
                ? "Sign in again"
                : `Sign in to ${provider().name}`}
          </button>
          <Show when={signedIn()}>
            <button
              type="button"
              class="ghost-button"
              disabled={!!busy()}
              onClick={() =>
                void act("sign-out", async () => {
                  await invoke("cloud_sign_out", {
                    profileId: props.profileId,
                  });
                  setSignedIn(false);
                  props.onNotice(`Signed out of ${provider().name}.`);
                })
              }
            >
              Sign out
            </button>
          </Show>
        </Show>
      </div>
      <Show when={signedIn() && props.account !== "not-signed-in"}>
        <p class="muted">Signed in as {props.account}.</p>
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
