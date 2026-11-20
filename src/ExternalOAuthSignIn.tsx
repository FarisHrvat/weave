import { createSignal, onCleanup, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorMessage } from "./api";

type DeviceStart = {
  id: string;
  userCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
};

type Props = {
  provider: "Bitbucket Cloud" | "Azure DevOps";
  path: string;
  onConnected: () => void;
};

const setup = {
  "Bitbucket Cloud":
    "In Bitbucket, open Workspace settings → OAuth consumers, add a consumer with the callback URL http://localhost:53682/ and repository, pull request and pipeline permissions, then copy its key and secret.",
  "Azure DevOps":
    "In the Microsoft Entra admin center, register an app for organizational accounts, allow public client flows, add the Azure DevOps user_impersonation permission, then copy its Application (client) ID.",
};

export default function ExternalOAuthSignIn(props: Props) {
  const [clientId, setClientId] = createSignal("");
  const [secret, setSecret] = createSignal("");
  const [device, setDevice] = createSignal<DeviceStart>();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let timer: number | undefined;
  onCleanup(() => window.clearTimeout(timer));

  function poll(start: DeviceStart) {
    timer = window.setTimeout(async () => {
      try {
        const done = await invoke<boolean>("external_oauth_azure_poll", {
          path: props.path,
          id: start.id,
          clientId: clientId(),
        });
        if (done) {
          setDevice(undefined);
          setBusy(false);
          props.onConnected();
        } else poll(start);
      } catch (reason) {
        setDevice(undefined);
        setBusy(false);
        setError(errorMessage(reason));
      }
    }, start.interval * 1000);
  }

  async function signIn(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      if (props.provider === "Bitbucket Cloud") {
        await invoke("external_oauth_bitbucket", {
          path: props.path,
          clientId: clientId(),
          clientSecret: secret(),
        });
        setSecret("");
        setBusy(false);
        props.onConnected();
        return;
      }
      const start = await invoke<DeviceStart>("external_oauth_azure_start", {
        clientId: clientId(),
      });
      setDevice(start);
      void openUrl(start.verificationUri).catch(() => undefined);
      poll(start);
    } catch (reason) {
      setBusy(false);
      setError(errorMessage(reason));
    }
  }

  return (
    <details class="oauth-sign-in">
      <summary>Or sign in with your browser</summary>
      <p class="muted">{setup[props.provider]}</p>
      <form class="external-online-form" onSubmit={signIn}>
        <label>
          {props.provider === "Bitbucket Cloud" ? "Key" : "Client ID"}
          <input
            required
            spellcheck={false}
            value={clientId()}
            onInput={(event) => setClientId(event.currentTarget.value)}
          />
        </label>
        <Show when={props.provider === "Bitbucket Cloud"}>
          <label>
            Secret
            <input
              type="password"
              required
              value={secret()}
              onInput={(event) => setSecret(event.currentTarget.value)}
            />
          </label>
        </Show>
        <button class="primary-button" type="submit" disabled={busy()}>
          {busy() ? "Waiting for the browser…" : "Sign in"}
        </button>
      </form>
      <Show when={device()}>
        {(start) => (
          <p class="oauth-device-code">
            Enter <strong>{start().userCode}</strong> on{" "}
            <button
              type="button"
              class="link-button"
              onClick={() => void openUrl(start().verificationUri)}
            >
              the Microsoft sign-in page
            </button>
            .
          </p>
        )}
      </Show>
      <Show when={error()}>
        <p class="form-error" role="alert">
          {error()}
        </p>
      </Show>
    </details>
  );
}
