import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-solid";
import type RFB from "@novnc/novnc";
import { errorMessage } from "./api";

type Props = {
  name: string;
  host: string;
  port: number;
  username?: string;
  profileId?: string;
  onClose: () => void;
};

type Phase = "connecting" | "credentials" | "connected" | "closed";

export default function VncViewer(props: Props) {
  let screen: HTMLDivElement | undefined;
  let rfb: RFB | undefined;
  const [phase, setPhase] = createSignal<Phase>("connecting");
  const [needsUser, setNeedsUser] = createSignal(false);
  const [username, setUsername] = createSignal(props.username ?? "");
  const [password, setPassword] = createSignal("");
  const [message, setMessage] = createSignal("");
  const [scaled, setScaled] = createSignal(true);
  const [viewOnly, setViewOnly] = createSignal(false);
  const [title, setTitle] = createSignal(props.name);

  onMount(async () => {
    try {
      const [{ default: Rfb }, bridge] = await Promise.all([
        import("@novnc/novnc"),
        invoke<{ url: string }>("vnc_bridge_start", {
          host: props.host,
          port: props.port,
          profileId: props.profileId ?? null,
        }),
      ]);
      if (!screen) return;
      rfb = new Rfb(screen, bridge.url);
      rfb.scaleViewport = true;
      rfb.background = "#0d0f14";
      rfb.addEventListener("connect", () => {
        setPhase("connected");
        setMessage("");
        rfb?.focus();
      });
      rfb.addEventListener("disconnect", (event) => {
        const clean = (event as CustomEvent<{ clean: boolean }>).detail.clean;
        setPhase("closed");
        setMessage(
          (current) =>
            current ||
            (clean
              ? "The session ended."
              : "The connection was lost. Check that screen sharing is on, and that the host and port are right."),
        );
      });
      rfb.addEventListener("credentialsrequired", (event) => {
        const types =
          (event as CustomEvent<{ types: string[] }>).detail.types ?? [];
        setNeedsUser(types.includes("username"));
        setPhase("credentials");
      });
      rfb.addEventListener("securityfailure", (event) => {
        const reason = (event as CustomEvent<{ reason?: string }>).detail
          .reason;
        setMessage(
          reason
            ? `The server refused the sign-in: ${reason}`
            : "The server refused the sign-in. Check the user name and password.",
        );
      });
      rfb.addEventListener("desktopname", (event) => {
        const name = (event as CustomEvent<{ name: string }>).detail.name;
        if (name) setTitle(name);
      });
      rfb.addEventListener("clipboard", (event) => {
        const text = (event as CustomEvent<{ text: string }>).detail.text;
        void navigator.clipboard?.writeText(text).catch(() => undefined);
      });
    } catch (error) {
      setPhase("closed");
      setMessage(errorMessage(error));
    }
  });

  onCleanup(() => rfb?.disconnect());

  function signIn(event: Event) {
    event.preventDefault();
    rfb?.sendCredentials({
      username: needsUser() ? username() : undefined,
      password: password(),
    });
    setPassword("");
    setPhase("connecting");
  }

  async function paste() {
    try {
      const text = await navigator.clipboard.readText();
      if (text) rfb?.clipboardPasteFrom(text);
      rfb?.focus();
    } catch {
      setMessage("The system did not allow reading the clipboard.");
    }
  }

  return (
    <div class="overlay vnc-overlay">
      <div
        class="vnc-window"
        role="dialog"
        aria-modal="true"
        aria-label={`Screen of ${title()}`}
      >
        <div class="vnc-toolbar">
          <strong class="truncate">{title()}</strong>
          <span class="muted vnc-status">
            {phase() === "connected"
              ? props.profileId
                ? "Connected through SSH"
                : "Connected"
              : phase() === "closed"
                ? "Disconnected"
                : "Connecting…"}
          </span>
          <span class="spacer" />
          <label class="profile-favorite">
            <input
              type="checkbox"
              checked={scaled()}
              onChange={(event) => {
                setScaled(event.currentTarget.checked);
                if (rfb) rfb.scaleViewport = event.currentTarget.checked;
              }}
            />
            Fit to window
          </label>
          <label class="profile-favorite">
            <input
              type="checkbox"
              checked={viewOnly()}
              onChange={(event) => {
                setViewOnly(event.currentTarget.checked);
                if (rfb) rfb.viewOnly = event.currentTarget.checked;
              }}
            />
            View only
          </label>
          <button
            class="server-action"
            disabled={phase() !== "connected"}
            onClick={() => void paste()}
          >
            Paste clipboard
          </button>
          <button
            class="server-action"
            disabled={phase() !== "connected"}
            onClick={() => rfb?.sendCtrlAltDel()}
          >
            Ctrl-Alt-Del
          </button>
          <button
            class="icon-button"
            aria-label="Disconnect and close"
            onClick={() => props.onClose()}
          >
            <X size={19} />
          </button>
        </div>
        <div class="vnc-screen" ref={screen} />
        <Show when={phase() === "credentials"}>
          <form class="vnc-credentials" onSubmit={signIn}>
            <p>{title()} asks for a password.</p>
            <Show when={needsUser()}>
              <label>
                User name
                <input
                  value={username()}
                  autocomplete="username"
                  onInput={(event) => setUsername(event.currentTarget.value)}
                />
              </label>
            </Show>
            <label>
              Password
              <input
                type="password"
                autofocus
                value={password()}
                autocomplete="current-password"
                onInput={(event) => setPassword(event.currentTarget.value)}
              />
            </label>
            <div class="dialog-actions">
              <button
                type="button"
                class="ghost-button"
                onClick={() => props.onClose()}
              >
                Cancel
              </button>
              <button type="submit" class="primary-button">
                Sign in
              </button>
            </div>
          </form>
        </Show>
        <Show when={message()}>
          <p
            class={
              phase() === "closed" ? "form-error vnc-message" : "vnc-message"
            }
            role="alert"
          >
            {message()}
          </p>
        </Show>
      </div>
    </div>
  );
}
