import { confirmAction } from "./dialogs";
import { fixedShortcuts, shortcutLabel } from "./shortcuts";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ThisComputer, isMac, primaryKey, thisComputer } from "./platform";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { ImageAddon } from "@xterm/addon-image";
import {
  asOneLine,
  durationText,
  findLines,
  isPasswordPrompt,
  pastedLines,
  pasteHistory,
  zoomedFont,
} from "./terminalPaste";
import { registerTerminal } from "./terminalRegistry";
import MenuButton from "./MenuButton";
import { ZmodemBridge } from "./zmodemBridge";
import { saysSftpOnly } from "./sftpOnly";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { open, save } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronUp, Search, X } from "lucide-solid";
import { api, errorMessage } from "./api";
import { parseOsc7 } from "./terminalLocation";
import { findPathLinks, parentFolder, resolvePath } from "./terminalPaths";
import {
  completionLines,
  completionScript,
  currentWord,
} from "./terminalCompletion";
import type { RemoteConnection } from "./remoteConnections";
import {
  addToHistory,
  clearHistory,
  historyKey,
  InputTracker,
  readHistory,
  suggest,
  wasEchoed,
} from "./terminalHistory";
import {
  AutomationRun,
  parseAutomation,
  readAutomations,
  writeAutomations,
  type AutomationStatus,
  type SavedAutomation,
} from "./terminalAutomation";
import {
  installerScript,
  parseOsc133,
  uninstallerScript,
} from "./shellIntegration";
import { TerminalRecording } from "./terminalRecording";
import {
  readTerminalScripts,
  scriptsForProfile,
  writeTerminalScripts,
  type SavedTerminalScript,
} from "./terminalScripts";
import {
  decodeTriggerResponse,
  maxTriggers,
  readTerminalTriggers,
  TerminalTriggerMatcher,
  triggerActionLabels,
  validateTrigger,
  writeTerminalTriggers,
  type TerminalTrigger,
  type TerminalTriggerAction,
} from "./terminalTriggers";
import {
  defaultTerminalOptions,
  readTerminalOptions,
  writeTerminalOptions,
  type TerminalOptions,
} from "./terminalOptions";

const terminalThemes = {
  midnight: {
    background: "#171a23",
    foreground: "#e4e7f0",
    cursor: "#ad9cf8",
    selectionBackground: "#53527188",
  },
  light: {
    background: "#f7f8fc",
    foreground: "#202431",
    cursor: "#5f52ac",
    selectionBackground: "#aaa0e088",
  },
  black: {
    background: "#0a0b0e",
    foreground: "#f1f1f1",
    cursor: "#77d6bc",
    selectionBackground: "#467b7088",
  },
};

type Props = {
  profileId?: string;
  connection?: RemoteConnection;
  /** Typed into the terminal once it is ready, followed by Enter. */
  startupCommand?: string;
  cwd?: string;
  onError: (message: string) => void;
  onReady?: (id: string) => void;
  onExit?: () => void;
  onClosed?: () => void;
  onOpenFiles?: (path: string) => void;
  onInput?: (data: string) => void;
  onActivity?: () => void;
  onRecordingState?: (unsaved: boolean) => void;
  /** The server said this sign-in may only transfer files: open Files, and
   *  optionally always open this host there. */
  onSftpOnly?: (always: boolean) => void;
  /** Lets "Find in all terminal tabs" search this one. */
  searchKey?: string;
  label?: string;
};

function readFlag(key: string, fallback: boolean) {
  try {
    const value = localStorage.getItem(key);
    return value === null ? fallback : value === "1";
  } catch {
    return fallback;
  }
}

function writeFlag(key: string, value: boolean) {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // The choice still applies until Weave restarts.
  }
}

export default function TerminalView(props: Props) {
  const passwordProfile = () =>
    props.profileId ?? props.connection?.profileId ?? undefined;
  let container!: HTMLDivElement;
  let terminalId: string | undefined;
  let disposed = false;
  let unlistenOutput: (() => void) | undefined;
  let unlistenExit: (() => void) | undefined;
  let searchInput: HTMLInputElement | undefined;
  const [searchOpen, setSearchOpen] = createSignal(false);
  const [narrow, setNarrow] = createSignal(false);
  const [pasteRequest, setPasteRequest] = createSignal<string>();
  const [sftpOnlyNotice, setSftpOnlyNotice] = createSignal(false);
  const [skipPasteCheck, setSkipPasteCheck] = createSignal(false);
  const [pasteSkip, setPasteSkip] = createSignal(
    readFlag("weave-terminal-paste-skip", false),
  );
  const [pastesOpen, setPastesOpen] = createSignal(false);
  const [passwordPrompt, setPasswordPrompt] = createSignal(false);
  const [notifyLong, setNotifyLong] = createSignal(
    readFlag("weave-terminal-notify-long", true),
  );
  const [searchTerm, setSearchTerm] = createSignal("");
  const [searchResult, setSearchResult] = createSignal({ index: 0, count: 0 });
  const [currentDirectory, setCurrentDirectory] = createSignal(props.cwd ?? "");
  const [recording, setRecording] = createSignal(false);
  const [recordingReady, setRecordingReady] = createSignal(false);
  const [triggerOpen, setTriggerOpen] = createSignal(false);
  const [triggerInput, setTriggerInput] = createSignal("");
  const [triggerRegex, setTriggerRegex] = createSignal(false);
  const [triggerAction, setTriggerAction] =
    createSignal<TerminalTriggerAction>("alert");
  const [triggerResponse, setTriggerResponse] = createSignal("");
  const [triggers, setTriggers] = createSignal<TerminalTrigger[]>([]);
  const [triggerAlert, setTriggerAlert] = createSignal("");
  const [optionsOpen, setOptionsOpen] = createSignal(false);
  const [scriptOpen, setScriptOpen] = createSignal(false);
  const [scriptInput, setScriptInput] = createSignal("");
  const [scriptCwd, setScriptCwd] = createSignal(props.cwd ?? "");
  const [scriptName, setScriptName] = createSignal("");
  const [selectedScriptId, setSelectedScriptId] = createSignal("");
  const [savedScripts, setSavedScripts] = createSignal<SavedTerminalScript[]>(
    [],
  );
  const [scriptBusy, setScriptBusy] = createSignal(false);
  const [scriptError, setScriptError] = createSignal("");
  const [scriptStorageError, setScriptStorageError] = createSignal(false);
  const [scriptResult, setScriptResult] = createSignal<{
    exitCode: number | null;
    stdout: string;
    stderr: string;
  }>();
  const [terminalOptions, setTerminalOptions] = createSignal<TerminalOptions>({
    ...defaultTerminalOptions,
  });
  const [completions, setCompletions] = createSignal<string[]>([]);
  const [completionIndex, setCompletionIndex] = createSignal(0);
  const [integrationOpen, setIntegrationOpen] = createSignal(false);
  const [integrationBusy, setIntegrationBusy] = createSignal(false);
  const [integrationOutput, setIntegrationOutput] = createSignal("");
  const [integrationActive, setIntegrationActive] = createSignal(false);
  const [lastExit, setLastExit] = createSignal<number | null>(null);
  const [automationOpen, setAutomationOpen] = createSignal(false);
  const [automationBody, setAutomationBody] = createSignal(
    "expect login:\nsend admin\\n\nexpect Password:\nsecret Password\n",
  );
  const [automationName, setAutomationName] = createSignal("");
  const [automations, setAutomations] = createSignal<SavedAutomation[]>([]);
  const [automationStatus, setAutomationStatus] =
    createSignal<AutomationStatus>();
  const [secretRequest, setSecretRequest] = createSignal<{
    prompt: string;
    resolve: (value: string | null) => void;
  }>();
  let secretInput: HTMLInputElement | undefined;
  let automation: AutomationRun | undefined;
  const automationDecoder = new TextDecoder();
  const inputTracker = new InputTracker();
  const promptMarkers: import("@xterm/xterm").IMarker[] = [];
  let promptState: "unknown" | "prompt" | "running" = "unknown";
  const historyStorageKey = () =>
    props.connection &&
    props.connection.kind !== "mosh" &&
    props.connection.kind !== "x11"
      ? `${props.connection.kind}:${props.connection.host ?? props.connection.device}`
      : historyKey(props.profileId);
  // Scripts run on this Mac or on the SSH host; Telnet and serial sessions
  // have no shell Weave can reach.
  const scriptTarget = () =>
    props.connection &&
    props.connection.kind !== "mosh" &&
    props.connection.kind !== "x11"
      ? undefined
      : (props.profileId ?? null);
  const triggerMatcher = new TerminalTriggerMatcher();
  let triggerAlertTimer: number | undefined;
  let recorder: TerminalRecording | undefined;
  let finishedRecording = "";
  let finishedTextRecording = "";
  const earlyOutput = new Map<string, Uint8Array[]>();
  const earlyExit = new Set<string>();
  let exited = false;
  const terminal = new Terminal({
    fontFamily: '"SFMono-Regular", "JetBrains Mono", Menlo, monospace',
    fontSize: 13,
    lineHeight: 1.5,
    cursorBlink: true,
    scrollback: 100_000,
    theme: terminalThemes.midnight,
    allowProposedApi: true,
    // Links a program prints with OSC 8 open only if they are web addresses.
    linkHandler: {
      allowNonHttpProtocols: false,
      activate: (_event, uri) => {
        if (/^https?:\/\//i.test(uri))
          void openUrl(uri).catch((error) =>
            props.onError(`Could not open link: ${errorMessage(error)}`),
          );
      },
    },
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  terminal.loadAddon(fit);
  terminal.loadAddon(search);
  // Pictures sent with the iTerm2 image protocol (imgcat) or as Sixel.
  terminal.loadAddon(new ImageAddon({ sixelSizeLimit: 25_000_000 }));
  let commandStarted = 0;
  let zoom = 0;
  const locationHandler = terminal.parser.registerOscHandler(7, (data) => {
    const path = parseOsc7(data);
    if (path) setCurrentDirectory(path);
    return true;
  });
  const promptHandler = terminal.parser.registerOscHandler(133, (data) => {
    const event = parseOsc133(data);
    if (!event) return true;
    setIntegrationActive(true);
    if (event.kind === "prompt") {
      promptState = "prompt";
      inputTracker.reset();
      const marker = terminal.registerMarker(0);
      if (marker) {
        promptMarkers.push(marker);
        marker.onDispose(() => {
          const index = promptMarkers.indexOf(marker);
          if (index >= 0) promptMarkers.splice(index, 1);
        });
      }
    } else if (event.kind === "running") {
      promptState = "running";
      commandStarted = Date.now();
    } else if (event.kind === "finished") {
      setLastExit(event.exitCode);
      notifyIfLong(event.exitCode);
      const marker = promptMarkers.at(-1);
      if (marker && event.exitCode)
        terminal.registerDecoration({
          marker,
          overviewRulerOptions: { color: "#ef6f72", position: "left" },
        });
    }
    return true;
  });

  function notifyIfLong(exitCode: number | null) {
    const elapsed = commandStarted ? Date.now() - commandStarted : 0;
    commandStarted = 0;
    const away =
      !document.hasFocus() ||
      document.visibilityState !== "visible" ||
      !container?.offsetParent;
    if (!notifyLong() || elapsed < 10_000 || !away) return;
    void invoke("system_notify", {
      title: exitCode ? "Command failed" : "Command finished",
      body: `${props.label ?? "Terminal"} · ${durationText(elapsed)}${exitCode ? ` · exit code ${exitCode}` : ""}`,
    }).catch(() => undefined);
  }

  function setFont(size: number) {
    terminal.options.fontSize = size;
    if (!terminal.element) return;
    fit.fit();
    if (terminalId)
      void invoke("term_resize", {
        id: terminalId,
        cols: terminal.cols,
        rows: terminal.rows,
      }).catch(() => undefined);
  }

  function changeZoom(step: number | "reset") {
    const base = terminalOptions().fontSize;
    zoom = step === "reset" ? 0 : zoomedFont(base + zoom, step) - base;
    setFont(base + zoom);
  }

  function sendPaste(text: string) {
    pasteHistory.add(text);
    terminal.paste(text);
    terminal.focus();
  }

  function onPaste(event: ClipboardEvent) {
    const text = event.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;
    if (pastedLines(text) > 1 && !pasteSkip()) {
      event.preventDefault();
      event.stopPropagation();
      setSkipPasteCheck(false);
      setPasteRequest(text);
      return;
    }
    pasteHistory.add(text);
  }

  /** Pastes text from a shortcut, with the same multi-line check. */
  function pasteChecked(text: string) {
    if (pastedLines(text) > 1 && !pasteSkip()) {
      setSkipPasteCheck(false);
      setPasteRequest(text);
      return;
    }
    sendPaste(text);
  }

  function answerPaste(mode: "paste" | "oneLine" | "cancel") {
    const text = pasteRequest();
    setPasteRequest(undefined);
    if (skipPasteCheck() && mode !== "cancel") {
      setPasteSkip(true);
      writeFlag("weave-terminal-paste-skip", true);
    }
    if (text && mode === "paste") sendPaste(text);
    else if (text && mode === "oneLine") sendPaste(asOneLine(text));
    else terminal.focus();
  }

  function checkPasswordPrompt() {
    if (!passwordProfile()) return;
    const buffer = terminal.buffer.active;
    const line =
      buffer.getLine(buffer.baseY + buffer.cursorY)?.translateToString(true) ??
      "";
    setPasswordPrompt(isPasswordPrompt(line));
  }

  async function fillPassword() {
    const profileId = passwordProfile();
    setPasswordPrompt(false);
    terminal.focus();
    if (!terminalId || !profileId) return;
    try {
      await invoke("term_fill_password", { id: terminalId, profileId });
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  function jumpToPrompt(direction: -1 | 1) {
    const top = terminal.buffer.active.viewportY;
    const lines = promptMarkers
      .map((marker) => marker.line)
      .filter((line) => line >= 0);
    const target =
      direction < 0
        ? lines.filter((line) => line < top).at(-1)
        : lines.find((line) => line > top);
    if (target !== undefined) terminal.scrollToLine(target);
    else if (direction > 0) terminal.scrollToBottom();
  }

  function openCompletions() {
    const typed = inputTracker.current ?? "";
    const items = suggest(
      typed,
      readHistory(localStorage, historyStorageKey()),
    );
    setCompletionIndex(0);
    setCompletions(items);
    const target = scriptTarget();
    const { word, command } = currentWord(typed);
    if (target === undefined || inputTracker.current === null || !word) {
      if (!items.length)
        props.onError(
          typed
            ? `No saved commands match “${typed}”.`
            : "No command history yet. Commands you run here are remembered for completion.",
        );
      return;
    }
    // Ask the shell on the host too; it reuses this terminal's connection.
    void api
      .runTerminalScript(
        target,
        currentDirectory() || null,
        completionScript(word, command),
      )
      .then((result) => {
        if ((inputTracker.current ?? "") !== typed) return;
        const fromHost = completionLines(typed, word, result.stdout);
        const merged = [...new Set([...completions(), ...fromHost])].slice(
          0,
          200,
        );
        setCompletions(merged);
        if (!merged.length)
          props.onError(`Nothing on the host or in history matches “${word}”.`);
      })
      .catch(() => {
        if (!items.length) props.onError(`No saved commands match “${typed}”.`);
      });
  }

  async function openPathLink(path: string) {
    if (props.connection && scriptTarget() === undefined) return;
    try {
      if (props.profileId) {
        const target = path.startsWith("~/")
          ? path.slice(2)
          : resolvePath(path, currentDirectory() || "/", "/");
        const looksLikeFile = /\.[A-Za-z0-9]+$/.test(target);
        props.onOpenFiles?.(looksLikeFile ? parentFolder(target) : target);
        return;
      }
      const home = await api.home();
      const target = resolvePath(path, currentDirectory() || home, home);
      const kind = await invoke<string>("local_path_kind", { path: target });
      if (kind === "folder") props.onOpenFiles?.(target);
      else if (kind === "file")
        await invoke("open_local_file", { path: target });
      else props.onError(`${target} does not exist on ${thisComputer}.`);
    } catch (error) {
      props.onError(`Could not open ${path}: ${errorMessage(error)}`);
    }
  }

  function acceptCompletion(command: string) {
    setCompletions([]);
    terminal.focus();
    if (!terminalId) return;
    const typed = inputTracker.current;
    const data =
      typed !== null && command.startsWith(typed)
        ? command.slice(typed.length)
        : `\x15${command}`;
    inputTracker.reset();
    inputTracker.feed(command);
    void invoke("term_write", { id: terminalId, data }).catch((error) =>
      props.onError(errorMessage(error)),
    );
  }

  async function runIntegration(install: boolean) {
    const target = scriptTarget();
    if (target === undefined) return;
    setIntegrationBusy(true);
    setIntegrationOutput("");
    try {
      const result = await api.runTerminalScript(
        target,
        null,
        install ? installerScript() : uninstallerScript(),
      );
      setIntegrationOutput(
        (result.stdout + result.stderr).trim() ||
          `Finished with exit code ${result.exitCode}.`,
      );
    } catch (error) {
      setIntegrationOutput(errorMessage(error));
    } finally {
      setIntegrationBusy(false);
    }
  }
  // ⌘-click (Ctrl-click off macOS) opens file paths printed in the output.
  terminal.registerLinkProvider({
    provideLinks(row, callback) {
      const line = terminal.buffer.active.getLine(row - 1);
      if (!line) return callback(undefined);
      const links = findPathLinks(line.translateToString(true)).map((link) => ({
        range: {
          start: { x: link.start + 1, y: row },
          end: { x: link.end, y: row },
        },
        text: link.path,
        decorations: { underline: true, pointerCursor: true },
        activate: (event: MouseEvent) => {
          if (primaryKey(event)) void openPathLink(link.path);
        },
      }));
      callback(links.length ? links : undefined);
    },
  });

  terminal.loadAddon(
    new WebLinksAddon((_event, uri) => {
      if (!/^https?:\/\//i.test(uri)) return;
      void openUrl(uri).catch((error) =>
        props.onError(`Could not open link: ${errorMessage(error)}`),
      );
    }),
  );

  const searchOptions = {
    decorations: {
      matchOverviewRuler: "#8d82d3",
      activeMatchColorOverviewRuler: "#d6bc71",
      matchBackground: "#514775",
      activeMatchBackground: "#8d7841",
    },
  };

  function openSearch() {
    setSearchOpen(true);
    queueMicrotask(() => searchInput?.focus());
  }

  function closeSearch() {
    setSearchOpen(false);
    search.clearDecorations();
    terminal.focus();
  }

  function highlightLine(text: string) {
    const buffer = terminal.buffer.active;
    const cursor = buffer.baseY + buffer.cursorY;
    for (let offset = 0; offset <= 50 && cursor - offset >= 0; offset++) {
      const line = buffer.getLine(cursor - offset)?.translateToString(true);
      if (line && text && line.includes(text.trim().slice(0, 60))) {
        const marker = terminal.registerMarker(-offset);
        if (marker)
          terminal.registerDecoration({
            marker,
            width: terminal.cols,
            backgroundColor: "#6b5a1e",
            layer: "bottom",
          });
        return;
      }
    }
  }

  type Panel = "options" | "triggers" | "script" | "shell" | "automate";

  function togglePanel(panel: Panel) {
    const wasOpen = {
      options: optionsOpen(),
      triggers: triggerOpen(),
      script: scriptOpen(),
      shell: integrationOpen(),
      automate: automationOpen(),
    }[panel];
    setOptionsOpen(false);
    setTriggerOpen(false);
    setScriptOpen(false);
    setIntegrationOpen(false);
    setAutomationOpen(false);
    if (wasOpen) return;
    if (panel === "options") setOptionsOpen(true);
    if (panel === "triggers") setTriggerOpen(true);
    if (panel === "shell") setIntegrationOpen(true);
    if (panel === "automate") {
      setAutomations(readAutomations(localStorage));
      setAutomationOpen(true);
    }
    if (panel === "script") {
      reloadScripts();
      if (!selectedScriptId()) setScriptCwd(currentDirectory());
      setScriptOpen(true);
    }
  }

  function runAutomation() {
    if (!terminalId) {
      props.onError("Wait for the terminal to connect before running steps.");
      return;
    }
    let steps;
    try {
      steps = parseAutomation(automationBody());
    } catch (error) {
      setAutomationStatus({ state: "failed", message: errorMessage(error) });
      return;
    }
    automation?.stop();
    automation = new AutomationRun(steps, {
      write: (data) => {
        if (terminalId)
          void invoke("term_write", { id: terminalId, data }).catch((error) =>
            props.onError(errorMessage(error)),
          );
      },
      askSecret: (prompt) =>
        new Promise((resolve) => {
          setSecretRequest({ prompt, resolve });
          queueMicrotask(() => secretInput?.focus());
        }),
      onStatus: setAutomationStatus,
    });
    automation.start();
  }

  function answerSecret(value: string | null) {
    secretRequest()?.resolve(value);
    setSecretRequest(undefined);
    terminal.focus();
  }

  function saveAutomation() {
    const name = automationName().trim();
    if (!name || name.length > 80) {
      props.onError("Name the automation (up to 80 characters) to save it.");
      return;
    }
    try {
      parseAutomation(automationBody());
      const next = [
        ...automations().filter((item) => item.name !== name),
        { id: crypto.randomUUID(), name, body: automationBody() },
      ];
      writeAutomations(localStorage, next);
      setAutomations(next);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  function toBase64(bytes: Uint8Array) {
    let binary = "";
    for (let at = 0; at < bytes.length; at += 0x8000)
      binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
    return btoa(binary);
  }

  function fromBase64(data: string) {
    const binary = atob(data);
    const bytes = new Uint8Array(binary.length);
    for (let at = 0; at < binary.length; at++)
      bytes[at] = binary.charCodeAt(at);
    return bytes;
  }

  // sz and rz: files from the host are saved to Downloads; rz asks what to send.
  const zmodem = new ZmodemBridge({
    toTerminal: showOutput,
    toRemote: (bytes) => {
      if (terminalId)
        void invoke("term_write_base64", {
          id: terminalId,
          data: toBase64(bytes),
        }).catch((error) => props.onError(errorMessage(error)));
    },
    chooseFiles: async () => {
      const chosen = await open({
        multiple: true,
        title: "Choose files to send to the host (rz)",
      });
      const paths = Array.isArray(chosen) ? chosen : chosen ? [chosen] : [];
      const files = [];
      for (const path of paths) {
        const file = await invoke<{
          name: string;
          data: string;
          modified: number | null;
        }>("zmodem_read", { path });
        files.push({
          name: file.name,
          bytes: fromBase64(file.data),
          mtime: file.modified ? new Date(file.modified * 1000) : undefined,
        });
      }
      return files;
    },
    saveFile: (file) =>
      invoke<string>("zmodem_save", {
        name: file.name,
        data: toBase64(file.bytes),
      }),
    onStatus: (text, error) => {
      if (!text) return;
      if (error) props.onError(text);
      else {
        setTriggerAlert(text);
        window.clearTimeout(triggerAlertTimer);
        triggerAlertTimer = window.setTimeout(() => setTriggerAlert(""), 8000);
      }
    },
  });

  let outputTail = "";
  let sftpOnlySeen = false;
  const sftpOnlyDecoder = new TextDecoder();

  function recordOutput(chunk: Uint8Array) {
    zmodem.consume(chunk);
  }

  function showOutput(chunk: Uint8Array) {
    if (chunk.length) props.onActivity?.();
    if (props.profileId && !sftpOnlySeen && props.onSftpOnly) {
      outputTail = (
        outputTail + sftpOnlyDecoder.decode(chunk, { stream: true })
      ).slice(-600);
      if (saysSftpOnly(outputTail)) {
        sftpOnlySeen = true;
        setSftpOnlyNotice(true);
      }
    }
    automation?.output(automationDecoder.decode(chunk, { stream: true }));
    const matches = triggerMatcher.feedMatches(chunk, triggers());
    terminal.write(chunk, () => {
      for (const { trigger, line } of matches)
        if (trigger.action === "highlight") highlightLine(line);
      checkPasswordPrompt();
    });
    const alerts = matches.filter(
      ({ trigger }) => !trigger.action || trigger.action === "alert",
    );
    if (alerts.length) {
      setTriggerAlert(
        `Terminal output matched “${alerts[alerts.length - 1].trigger.phrase}”.`,
      );
      window.clearTimeout(triggerAlertTimer);
      triggerAlertTimer = window.setTimeout(() => setTriggerAlert(""), 8000);
    }
    for (const { trigger, line } of matches) {
      if (trigger.action === "notify") {
        void invoke("system_notify", {
          title: `Weave · ${trigger.phrase}`,
          body: line.trim() || trigger.phrase,
        }).catch((error) => props.onError(errorMessage(error)));
      } else if (
        trigger.action === "respond" &&
        trigger.response &&
        terminalId
      ) {
        void invoke("term_write", {
          id: terminalId,
          data: decodeTriggerResponse(trigger.response),
        }).catch((error) => props.onError(errorMessage(error)));
      }
    }
    if (recorder && !recorder.output(chunk)) {
      stopRecording();
      props.onError(
        "Recording reached its 4 MB limit. Save this recording or start a new one.",
      );
    }
  }

  function reloadTriggers() {
    try {
      setTriggers(readTerminalTriggers(localStorage));
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  function applyOptions(options: TerminalOptions) {
    terminal.options.fontSize = zoomedFont(options.fontSize, zoom);
    terminal.options.scrollback = options.scrollback;
    terminal.options.theme = terminalThemes[options.theme];
    terminal.options.cursorStyle = options.cursorStyle;
    terminal.options.cursorBlink = options.cursorBlink;
    if (terminal.element) {
      fit.fit();
      if (terminalId)
        void invoke("term_resize", {
          id: terminalId,
          cols: terminal.cols,
          rows: terminal.rows,
        }).catch((error) => props.onError(errorMessage(error)));
    }
  }

  function updateOptions(options: TerminalOptions) {
    try {
      writeTerminalOptions(localStorage, props.profileId, options);
      setTerminalOptions(options);
      applyOptions(options);
    } catch (error) {
      props.onError(`Could not save terminal settings: ${errorMessage(error)}`);
    }
  }

  function saveTriggers(next: TerminalTrigger[]) {
    try {
      writeTerminalTriggers(localStorage, next);
      setTriggers(next);
      window.dispatchEvent(new Event("weave-terminal-triggers-change"));
    } catch (error) {
      props.onError(`Could not save terminal triggers: ${errorMessage(error)}`);
    }
  }

  function addTrigger(event: Event) {
    event.preventDefault();
    const phrase = triggerInput().trim();
    const rule = {
      phrase,
      regex: triggerRegex() || undefined,
      action: triggerAction(),
      response: triggerAction() === "respond" ? triggerResponse() : undefined,
    };
    const problem = validateTrigger(rule);
    if (problem) {
      props.onError(problem);
      return;
    }
    if (triggers().length >= maxTriggers) {
      props.onError(`You can save up to ${maxTriggers} terminal triggers.`);
      return;
    }
    if (
      triggers().some(
        (trigger) =>
          trigger.phrase.toLocaleLowerCase() === phrase.toLocaleLowerCase(),
      )
    ) {
      props.onError("That terminal trigger is already saved.");
      return;
    }
    saveTriggers([...triggers(), { id: crypto.randomUUID(), ...rule }]);
    setTriggerInput("");
    setTriggerResponse("");
  }

  function startRecording() {
    recorder = new TerminalRecording(terminal.cols, terminal.rows);
    finishedRecording = "";
    finishedTextRecording = "";
    setRecordingReady(false);
    setRecording(true);
    props.onRecordingState?.(true);
  }

  function stopRecording() {
    if (!recorder) return;
    finishedRecording = recorder.finish();
    finishedTextRecording = recorder.plainText();
    recorder = undefined;
    setRecording(false);
    setRecordingReady(true);
  }

  async function saveRecording() {
    if (recorder) stopRecording();
    if (!finishedRecording) return;
    try {
      const destination = await save({
        title: "Save terminal recording",
        defaultPath: "weave-session.cast",
        filters: [{ name: "Asciinema recording", extensions: ["cast"] }],
      });
      if (!destination) return;
      await api.saveTerminalRecording(destination, finishedRecording);
      finishedRecording = "";
      finishedTextRecording = "";
      setRecordingReady(false);
      props.onRecordingState?.(false);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function saveTextLog() {
    if (recorder) stopRecording();
    if (!finishedRecording) return;
    try {
      const destination = await save({
        title: "Save plain-text terminal log",
        defaultPath: "weave-session.log",
        filters: [{ name: "Text log", extensions: ["log", "txt"] }],
      });
      if (!destination) return;
      await api.saveTerminalTextLog(destination, finishedTextRecording);
      finishedRecording = "";
      finishedTextRecording = "";
      setRecordingReady(false);
      props.onRecordingState?.(false);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function runScript(event: Event) {
    event.preventDefault();
    setScriptBusy(true);
    setScriptError("");
    setScriptResult(undefined);
    try {
      setScriptResult(
        await api.runTerminalScript(
          props.profileId ?? null,
          scriptCwd().trim() || null,
          scriptInput(),
        ),
      );
    } catch (error) {
      setScriptError(errorMessage(error));
    } finally {
      setScriptBusy(false);
    }
  }

  function reloadScripts() {
    try {
      setSavedScripts(readTerminalScripts(localStorage));
      if (scriptStorageError()) setScriptError("");
      setScriptStorageError(false);
    } catch (error) {
      setScriptError(errorMessage(error));
      setScriptStorageError(true);
    }
  }

  async function resetScripts() {
    if (
      !(await confirmAction(
        `Clear all saved terminal scripts on ${thisComputer}?`,
      ))
    )
      return;
    try {
      localStorage.removeItem("weave-terminal-scripts");
      setSavedScripts([]);
      setScriptStorageError(false);
      setScriptError("");
      loadScript("");
    } catch (error) {
      setScriptError(`Could not reset saved scripts: ${errorMessage(error)}`);
    }
  }

  function loadScript(id: string) {
    setSelectedScriptId(id);
    setScriptResult(undefined);
    setScriptError("");
    const selected = savedScripts().find((item) => item.id === id);
    setScriptName(selected?.name ?? "");
    setScriptInput(selected?.body ?? "");
    setScriptCwd(selected?.cwd ?? currentDirectory());
  }

  function saveScript() {
    const name = scriptName().trim();
    if (!name) {
      setScriptError("Give this script a name before saving it.");
      return;
    }
    const id = selectedScriptId() || crypto.randomUUID();
    const script: SavedTerminalScript = {
      id,
      name,
      profileId: props.profileId ?? null,
      cwd: scriptCwd().trim(),
      body: scriptInput(),
    };
    try {
      const next = [
        ...readTerminalScripts(localStorage).filter((item) => item.id !== id),
        script,
      ];
      writeTerminalScripts(localStorage, next);
      setSavedScripts(next);
      setSelectedScriptId(id);
      setScriptError("");
    } catch (error) {
      setScriptError(`Could not save script: ${errorMessage(error)}`);
    }
  }

  async function deleteScript() {
    const id = selectedScriptId();
    const selected = savedScripts().find((item) => item.id === id);
    if (
      !selected ||
      !(await confirmAction(`Delete saved script “${selected.name}”?`))
    )
      return;
    try {
      const next = readTerminalScripts(localStorage).filter(
        (item) => item.id !== id,
      );
      writeTerminalScripts(localStorage, next);
      setSavedScripts(next);
      loadScript("");
    } catch (error) {
      setScriptError(`Could not delete script: ${errorMessage(error)}`);
    }
  }

  onMount(async () => {
    reloadTriggers();
    reloadScripts();
    try {
      const saved = readTerminalOptions(localStorage, props.profileId);
      setTerminalOptions(saved);
      applyOptions(saved);
    } catch (error) {
      props.onError(errorMessage(error));
    }
    window.addEventListener("weave-terminal-triggers-change", reloadTriggers);
    onCleanup(() =>
      window.removeEventListener(
        "weave-terminal-triggers-change",
        reloadTriggers,
      ),
    );
    terminal.open(container);
    fit.fit();
    container.addEventListener("paste", onPaste, true);
    const closePastes = (event: Event) => {
      if (!pastesOpen()) return;
      if (event instanceof KeyboardEvent && event.key !== "Escape") return;
      if (
        event instanceof MouseEvent &&
        (event.target as Element | null)?.closest(
          ".terminal-pastes, .terminal-find",
        )
      )
        return;
      setPastesOpen(false);
    };
    document.addEventListener("keydown", closePastes);
    document.addEventListener("mousedown", closePastes);
    onCleanup(() => {
      document.removeEventListener("keydown", closePastes);
      document.removeEventListener("mousedown", closePastes);
    });
    onCleanup(() => container.removeEventListener("paste", onPaste, true));
    if (props.searchKey)
      onCleanup(
        registerTerminal(props.searchKey, {
          find: (query) => {
            const buffer = terminal.buffer.active;
            function* lines() {
              for (let row = 0; row < buffer.length; row++)
                yield buffer.getLine(row)?.translateToString(true) ?? "";
            }
            return findLines(lines(), query);
          },
          reveal: (line) => {
            terminal.scrollToLine(
              Math.max(0, line - Math.floor(terminal.rows / 2)),
            );
            terminal.select(0, line, terminal.cols);
            terminal.focus();
          },
        }),
      );
    const searchListener = search.onDidChangeResults((result) =>
      setSearchResult({
        index: result.resultIndex + 1,
        count: result.resultCount,
      }),
    );
    const shortcut = (event: KeyboardEvent) => {
      if (
        (isMac ? event.metaKey : event.ctrlKey && event.shiftKey) &&
        event.key.toLowerCase() === "f" &&
        (container.contains(document.activeElement) ||
          document.activeElement === searchInput)
      ) {
        event.preventDefault();
        openSearch();
      }
    };
    window.addEventListener("keydown", shortcut);
    onCleanup(() => {
      searchListener.dispose();
      window.removeEventListener("keydown", shortcut);
    });
    const resize = new ResizeObserver(() => {
      requestAnimationFrame(() => setNarrow(container.clientWidth < 760));
      fit.fit();
      recorder?.resize(terminal.cols, terminal.rows);
      if (terminalId)
        invoke("term_resize", {
          id: terminalId,
          cols: terminal.cols,
          rows: terminal.rows,
        }).catch(() => {});
    });
    resize.observe(container);
    onCleanup(() => resize.disconnect());
    try {
      unlistenOutput = await listen<{ id: string; data: number[] }>(
        "terminal-output",
        (event) => {
          const chunk = new Uint8Array(event.payload.data);
          if (event.payload.id === terminalId) recordOutput(chunk);
          else if (!terminalId)
            earlyOutput.set(event.payload.id, [
              ...(earlyOutput.get(event.payload.id) ?? []),
              chunk,
            ]);
        },
      );
      if (disposed) {
        unlistenOutput();
        return;
      }
      unlistenExit = await listen<string>("terminal-exit", (event) => {
        if (event.payload === terminalId) {
          terminal.write("\r\n\x1b[90mSession ended.\x1b[0m\r\n");
          stopRecording();
          if (!exited) props.onExit?.();
          exited = true;
        } else if (!terminalId) {
          earlyExit.add(event.payload);
        }
      });
      if (disposed) {
        unlistenOutput();
        unlistenExit();
        return;
      }
      terminalId = props.connection
        ? await invoke<string>("term_open_connection", {
            connection: {
              kind: props.connection.kind,
              profileId: props.connection.profileId ?? null,
              host: props.connection.host ?? null,
              port: props.connection.port ?? null,
              device: props.connection.device ?? null,
              baud: props.connection.baud ?? null,
              command: props.connection.command ?? null,
            },
            cols: terminal.cols,
            rows: terminal.rows,
          })
        : await invoke<string>("term_open", {
            profileId: props.profileId ?? null,
            cwd: props.cwd ?? null,
            cols: terminal.cols,
            rows: terminal.rows,
          });
      for (const chunk of earlyOutput.get(terminalId) ?? [])
        recordOutput(chunk);
      earlyOutput.clear();
      if (earlyExit.has(terminalId)) {
        terminal.write("\r\n\x1b[90mSession ended.\x1b[0m\r\n");
        stopRecording();
        exited = true;
        props.onExit?.();
      }
      earlyExit.clear();
      if (disposed) {
        unlistenOutput();
        unlistenExit();
        await invoke("term_close", { id: terminalId });
      } else {
        props.onReady?.(terminalId);
        terminal.focus();
        if (props.startupCommand?.trim())
          void invoke("term_write", {
            id: terminalId,
            data: `${props.startupCommand.trim()}\r`,
          }).catch((error) => props.onError(errorMessage(error)));
      }
    } catch (error) {
      props.onError(errorMessage(error));
      terminal.write(`\r\n${errorMessage(error)}\r\n`);
      if (!disposed) props.onExit?.();
    }
  });

  terminal.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    // Windows and Linux terminals copy and paste with Ctrl+Shift+C and V,
    // since Ctrl+C and Ctrl+V belong to the shell.
    if (!isMac && event.ctrlKey && event.shiftKey && !event.altKey) {
      if (event.code === "KeyC") {
        event.preventDefault();
        const selection = terminal.getSelection();
        if (selection) void writeText(selection).catch(() => undefined);
        return false;
      }
      if (event.code === "KeyV") {
        event.preventDefault();
        void readText()
          .then((text) => text && pasteChecked(text))
          .catch(() => undefined);
        return false;
      }
    }
    if (event.ctrlKey && event.code === "Space") {
      event.preventDefault();
      openCompletions();
      return false;
    }
    const zoomKey = isMac
      ? event.metaKey && !event.ctrlKey
      : event.ctrlKey && !event.metaKey;
    if (zoomKey && !event.altKey) {
      const step =
        event.key === "=" || event.key === "+"
          ? 1
          : event.key === "-"
            ? -1
            : event.key === "0"
              ? "reset"
              : undefined;
      if (step !== undefined) {
        event.preventDefault();
        changeZoom(step);
        return false;
      }
    }
    if (
      (isMac ? event.metaKey : event.ctrlKey && event.shiftKey) &&
      (event.key === "ArrowUp" || event.key === "ArrowDown")
    ) {
      event.preventDefault();
      jumpToPrompt(event.key === "ArrowUp" ? -1 : 1);
      return false;
    }
    return true;
  });

  terminal.onData((data) => {
    if (passwordPrompt()) setPasswordPrompt(false);
    if (terminalId) {
      const command = inputTracker.feed(data);
      if (command && promptState !== "running") {
        const buffer = terminal.buffer.active;
        const line =
          buffer
            .getLine(buffer.baseY + buffer.cursorY)
            ?.translateToString(true) ?? "";
        if (wasEchoed(line, command))
          addToHistory(localStorage, historyStorageKey(), command);
      }
      invoke("term_write", { id: terminalId, data }).catch((error) =>
        props.onError(errorMessage(error)),
      );
      props.onInput?.(data);
    }
  });

  onCleanup(() => {
    disposed = true;
    unlistenOutput?.();
    unlistenExit?.();
    if (terminalId) invoke("term_close", { id: terminalId }).catch(() => {});
    props.onClosed?.();
    props.onRecordingState?.(false);
    locationHandler.dispose();
    promptHandler.dispose();
    automation?.stop();
    secretRequest()?.resolve(null);
    window.clearTimeout(triggerAlertTimer);
    terminal.dispose();
  });

  const toolButtons = () => (
    <>
      <button
        title="Display settings for this terminal host"
        onClick={() => togglePanel("options")}
      >
        Appearance
      </button>
      <button
        title="Act when terminal output contains a phrase"
        onClick={() => togglePanel("triggers")}
      >
        Triggers{triggers().length ? ` (${triggers().length})` : ""}
      </button>
      <button
        title="Run a short shell script locally or on this SSH host"
        disabled={scriptBusy() && scriptOpen()}
        onClick={() => togglePanel("script")}
      >
        Script
      </button>
      <Show when={scriptTarget() !== undefined}>
        <button
          title={`Shell integration: folder tracking, prompt marks (${shortcutLabel(fixedShortcuts.promptUp)}/${shortcutLabel(fixedShortcuts.promptDown)}) and command status`}
          onClick={() => togglePanel("shell")}
        >
          Shell{integrationActive() ? " ✓" : ""}
        </button>
      </Show>
      <button
        title="Wait for prompts and type responses automatically"
        onClick={() => togglePanel("automate")}
      >
        Automate
      </button>
      <button
        title={`Complete from command history (${shortcutLabel(fixedShortcuts.complete)})`}
        onClick={() => openCompletions()}
      >
        Complete
      </button>
      <button
        title="Paste something you pasted earlier in Weave"
        aria-expanded={pastesOpen()}
        onClick={() => setPastesOpen(!pastesOpen())}
      >
        Pastes
      </button>
      <Show
        when={recording()}
        fallback={
          <button
            title={
              recordingReady()
                ? "Save the previous recording before starting another"
                : "Record output, including commands echoed by the shell"
            }
            disabled={recordingReady()}
            onClick={startRecording}
          >
            Record
          </button>
        }
      >
        <button
          title="Stop recording and choose a save format"
          onClick={stopRecording}
        >
          Stop
        </button>
      </Show>
      <Show when={recordingReady() && !recording()}>
        <button
          title="Save the stopped recording for playback"
          onClick={() => void saveRecording()}
        >
          Save .cast
        </button>
        <button
          title="Save the stopped recording as plain text"
          onClick={() => void saveTextLog()}
        >
          Save text log
        </button>
      </Show>
      <Show when={currentDirectory() && props.onOpenFiles}>
        <button
          title={`Open ${currentDirectory()} in Files`}
          onClick={() => props.onOpenFiles?.(currentDirectory())}
        >
          Open folder in Files
        </button>
      </Show>
    </>
  );

  return (
    <div class="terminal-stage">
      <div class="terminal-find">
        <Show when={narrow()} fallback={toolButtons()}>
          <MenuButton label="More" title="Terminal tools">
            {toolButtons()}
          </MenuButton>
        </Show>
        <Show
          when={searchOpen()}
          fallback={
            <button
              title={`Find in terminal (${shortcutLabel(fixedShortcuts.find)})`}
              onClick={openSearch}
            >
              <Search size={15} /> Find
            </button>
          }
        >
          <input
            ref={searchInput}
            aria-label="Find in terminal"
            value={searchTerm()}
            onInput={(event) => {
              const term = event.currentTarget.value;
              setSearchTerm(term);
              if (term) search.findNext(term, searchOptions);
              else {
                search.clearDecorations();
                setSearchResult({ index: 0, count: 0 });
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") closeSearch();
              if (event.key === "Enter") {
                event.preventDefault();
                if (event.shiftKey)
                  search.findPrevious(searchTerm(), searchOptions);
                else search.findNext(searchTerm(), searchOptions);
              }
            }}
          />
          <span class="terminal-find-count">
            {searchTerm()
              ? `${searchResult().index} of ${searchResult().count}`
              : "Find text"}
          </span>
          <button
            title="Previous match"
            disabled={!searchTerm()}
            onClick={() => search.findPrevious(searchTerm(), searchOptions)}
          >
            <ChevronUp size={15} />
          </button>
          <button
            title="Next match"
            disabled={!searchTerm()}
            onClick={() => search.findNext(searchTerm(), searchOptions)}
          >
            <ChevronDown size={15} />
          </button>
          <button title="Close search" onClick={closeSearch}>
            <X size={15} />
          </button>
        </Show>
      </div>
      <Show when={optionsOpen()}>
        <div class="terminal-options-panel">
          <strong>
            {props.profileId ? "Host" : "Local"} terminal appearance
          </strong>
          <label>
            Font size
            <input
              aria-label="Terminal font size"
              type="number"
              min="10"
              max="24"
              value={terminalOptions().fontSize}
              onChange={(event) => {
                const fontSize = Number(event.currentTarget.value);
                if (
                  Number.isInteger(fontSize) &&
                  fontSize >= 10 &&
                  fontSize <= 24
                )
                  updateOptions({ ...terminalOptions(), fontSize });
              }}
            />
          </label>
          <label>
            Theme
            <select
              aria-label="Terminal theme"
              value={terminalOptions().theme}
              onChange={(event) =>
                updateOptions({
                  ...terminalOptions(),
                  theme: event.currentTarget.value as TerminalOptions["theme"],
                })
              }
            >
              <option value="midnight">Midnight</option>
              <option value="light">Light</option>
              <option value="black">Black</option>
            </select>
          </label>
          <label>
            Scrollback lines
            <select
              aria-label="Terminal scrollback lines"
              value={terminalOptions().scrollback}
              onChange={(event) =>
                updateOptions({
                  ...terminalOptions(),
                  scrollback: Number(event.currentTarget.value),
                })
              }
            >
              <option value="5000">5,000</option>
              <option value="20000">20,000</option>
              <option value="100000">100,000</option>
            </select>
          </label>
          <label>
            Cursor
            <select
              aria-label="Terminal cursor style"
              value={terminalOptions().cursorStyle}
              onChange={(event) =>
                updateOptions({
                  ...terminalOptions(),
                  cursorStyle: event.currentTarget
                    .value as TerminalOptions["cursorStyle"],
                })
              }
            >
              <option value="block">Block</option>
              <option value="underline">Underline</option>
              <option value="bar">Bar</option>
            </select>
          </label>
          <label>
            Blink cursor
            <input
              aria-label="Blink terminal cursor"
              type="checkbox"
              checked={terminalOptions().cursorBlink}
              onChange={(event) =>
                updateOptions({
                  ...terminalOptions(),
                  cursorBlink: event.currentTarget.checked,
                })
              }
            />
          </label>
          <label>
            Notify when a command over 10 s ends while you are away
            <input
              aria-label="Notify when long commands finish"
              type="checkbox"
              checked={notifyLong()}
              onChange={(event) => {
                setNotifyLong(event.currentTarget.checked);
                writeFlag(
                  "weave-terminal-notify-long",
                  event.currentTarget.checked,
                );
              }}
            />
          </label>
          <Show when={pasteSkip()}>
            <button
              type="button"
              class="mini-button"
              onClick={() => {
                setPasteSkip(false);
                writeFlag("weave-terminal-paste-skip", false);
              }}
            >
              Ask again before multi-line pastes
            </button>
          </Show>
          <p class="muted">
            {shortcutLabel(fixedShortcuts.zoomIn)} and{" "}
            {shortcutLabel(fixedShortcuts.zoomOut)} zoom this terminal;{" "}
            {shortcutLabel(fixedShortcuts.zoomReset)} returns to the size above.
            Needs shell integration for command notifications.
          </p>
        </div>
      </Show>
      <Show when={scriptOpen()}>
        <form
          class="terminal-script-panel"
          onSubmit={(event) => void runScript(event)}
        >
          <strong>Run shell script</strong>
          <p>
            {props.profileId ? "SSH host" : ThisComputer} · runs in a separate
            shell with a 2-minute limit. SSH requires a trusted host and key or
            agent authentication.
          </p>
          <p>
            Saved scripts stay in this app on {thisComputer}. Do not save
            passwords or tokens in them.
          </p>
          <label>
            Saved for {props.profileId ? "this SSH host" : thisComputer}
            <select
              aria-label="Saved terminal script"
              value={selectedScriptId()}
              disabled={scriptBusy()}
              onChange={(event) => loadScript(event.currentTarget.value)}
            >
              <option value="">New script</option>
              <For
                each={scriptsForProfile(
                  savedScripts(),
                  props.profileId ?? null,
                )}
              >
                {(item) => <option value={item.id}>{item.name}</option>}
              </For>
            </select>
          </label>
          <label>
            Script name
            <input
              aria-label="Script name"
              value={scriptName()}
              onInput={(event) => setScriptName(event.currentTarget.value)}
              placeholder="Check disk space"
              maxLength={80}
              disabled={scriptBusy()}
            />
          </label>
          <label>
            Working folder (optional)
            <input
              aria-label="Script working folder"
              value={scriptCwd()}
              onInput={(event) => setScriptCwd(event.currentTarget.value)}
              placeholder={props.profileId ? "Remote folder" : "Local folder"}
              maxLength={1024}
              disabled={scriptBusy()}
            />
          </label>
          <label>
            Script
            <textarea
              aria-label="Shell script"
              value={scriptInput()}
              onInput={(event) => setScriptInput(event.currentTarget.value)}
              placeholder="pwd\nls -la"
              maxLength={65536}
              spellcheck={false}
              disabled={scriptBusy()}
            />
          </label>
          <div class="terminal-script-actions">
            <button
              type="button"
              disabled={scriptBusy() || !scriptInput().trim()}
              onClick={saveScript}
            >
              Save
            </button>
            <button
              type="button"
              disabled={scriptBusy() || !selectedScriptId()}
              onClick={deleteScript}
            >
              Delete
            </button>
            <button
              type="submit"
              disabled={scriptBusy() || !scriptInput().trim()}
            >
              {scriptBusy() ? "Running…" : "Run script"}
            </button>
            <button
              type="button"
              disabled={scriptBusy()}
              onClick={() => setScriptOpen(false)}
            >
              Close
            </button>
          </div>
          <Show when={scriptError()}>
            <p class="terminal-script-error" role="alert">
              {scriptError()}
            </p>
          </Show>
          <Show when={scriptStorageError()}>
            <button type="button" onClick={resetScripts}>
              Reset saved scripts
            </button>
          </Show>
          <Show when={scriptResult()}>
            {(result) => (
              <div class="terminal-script-result" role="status">
                <strong>
                  {result().exitCode === 0
                    ? "Finished successfully"
                    : result().exitCode === null
                      ? "Stopped without an exit code"
                      : `Exited with code ${result().exitCode}`}
                </strong>
                <Show when={result().stdout}>
                  <pre>{result().stdout}</pre>
                </Show>
                <Show when={result().stderr}>
                  <pre class="terminal-script-error">{result().stderr}</pre>
                </Show>
              </div>
            )}
          </Show>
        </form>
      </Show>
      <Show when={triggerOpen()}>
        <div class="terminal-trigger-panel">
          <strong>Output triggers</strong>
          <p>
            Act when an output line contains a saved phrase or matches a
            pattern. Responses are also typed at waiting prompts, at most once
            every 2 seconds. Use \n for Enter.
          </p>
          <form onSubmit={addTrigger} class="terminal-trigger-form">
            <input
              aria-label="New terminal trigger phrase"
              maxLength={80}
              value={triggerInput()}
              onInput={(event) => setTriggerInput(event.currentTarget.value)}
              placeholder={triggerRegex() ? "error \\d+" : "Build failed"}
            />
            <select
              aria-label="Trigger action"
              value={triggerAction()}
              onChange={(event) =>
                setTriggerAction(
                  event.currentTarget.value as TerminalTriggerAction,
                )
              }
            >
              <For each={Object.entries(triggerActionLabels)}>
                {([value, label]) => <option value={value}>{label}</option>}
              </For>
            </select>
            <Show when={triggerAction() === "respond"}>
              <input
                aria-label="Response to type"
                maxLength={200}
                value={triggerResponse()}
                onInput={(event) =>
                  setTriggerResponse(event.currentTarget.value)
                }
                placeholder="y\n"
              />
            </Show>
            <label class="terminal-trigger-regex">
              <input
                type="checkbox"
                checked={triggerRegex()}
                onChange={(event) =>
                  setTriggerRegex(event.currentTarget.checked)
                }
              />
              Regex
            </label>
            <button type="submit">Add</button>
          </form>
          <For each={triggers()} fallback={<p>No triggers saved.</p>}>
            {(trigger) => (
              <div class="terminal-trigger-row">
                <span>
                  {trigger.regex ? `/${trigger.phrase}/` : trigger.phrase}
                  <small>
                    {" "}
                    · {triggerActionLabels[trigger.action ?? "alert"]}
                    {trigger.action === "respond"
                      ? ` “${trigger.response}”`
                      : ""}
                  </small>
                </span>
                <button
                  aria-label={`Remove trigger ${trigger.phrase}`}
                  onClick={() =>
                    saveTriggers(
                      triggers().filter((item) => item.id !== trigger.id),
                    )
                  }
                >
                  Remove
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={automationOpen()}>
        <div class="terminal-trigger-panel terminal-automation-panel">
          <strong>Automation</strong>
          <p>
            One step per line: <code>expect</code> text or /regex/,{" "}
            <code>send</code> text (\n is Enter), <code>secret</code> asks you
            for a value, <code>wait</code> and <code>timeout</code> take
            seconds.
          </p>
          <Show when={automations().length}>
            <select
              aria-label="Saved automations"
              onChange={(event) => {
                const item = automations().find(
                  (saved) => saved.id === event.currentTarget.value,
                );
                if (item) {
                  setAutomationBody(item.body);
                  setAutomationName(item.name);
                }
              }}
            >
              <option value="">Open a saved automation…</option>
              <For each={automations()}>
                {(item) => <option value={item.id}>{item.name}</option>}
              </For>
            </select>
          </Show>
          <textarea
            aria-label="Automation steps"
            spellcheck={false}
            value={automationBody()}
            onInput={(event) => setAutomationBody(event.currentTarget.value)}
          />
          <div class="terminal-trigger-row">
            <input
              aria-label="Automation name"
              maxLength={80}
              placeholder="Name"
              value={automationName()}
              onInput={(event) => setAutomationName(event.currentTarget.value)}
            />
            <button onClick={saveAutomation}>Save</button>
            <Show
              when={automationStatus()?.state === "running"}
              fallback={<button onClick={runAutomation}>Run</button>}
            >
              <button onClick={() => automation?.stop()}>Stop</button>
            </Show>
          </div>
          <Show when={secretRequest()}>
            {(request) => (
              <form
                class="terminal-trigger-row"
                onSubmit={(event) => {
                  event.preventDefault();
                  answerSecret(secretInput?.value ?? "");
                }}
              >
                <input
                  ref={secretInput}
                  type="password"
                  aria-label={request().prompt}
                  placeholder={request().prompt}
                />
                <button type="submit">Send</button>
                <button type="button" onClick={() => answerSecret(null)}>
                  Cancel
                </button>
              </form>
            )}
          </Show>
          <Show when={automationStatus()}>
            {(status) => (
              <p
                class={
                  status().state === "failed" ? "terminal-automation-error" : ""
                }
              >
                {(() => {
                  const current = status();
                  if (current.state === "done") return "Finished.";
                  if (current.state === "failed") return current.message;
                  return current.waitingFor
                    ? `Step ${current.step}: waiting for “${current.waitingFor}”…`
                    : `Step ${current.step}…`;
                })()}
              </p>
            )}
          </Show>
        </div>
      </Show>
      <Show when={integrationOpen()}>
        <div class="terminal-trigger-panel terminal-integration-panel">
          <strong>Shell integration</strong>
          <p>
            Installs a small zsh and bash script on{" "}
            {props.profileId ? "this SSH host" : thisComputer} that reports the
            current folder and marks prompts and exit codes. Jump between
            prompts with {shortcutLabel(fixedShortcuts.promptUp)} and{" "}
            {shortcutLabel(fixedShortcuts.promptDown)}; failed commands are
            marked in the scrollbar.
          </p>
          <p>
            Status:{" "}
            {integrationActive()
              ? `active${lastExit() !== null ? ` · last exit code ${lastExit()}` : ""}`
              : "not detected in this tab"}
          </p>
          <div class="terminal-trigger-row">
            <button
              disabled={integrationBusy()}
              onClick={() => void runIntegration(true)}
            >
              {integrationBusy() ? "Working…" : "Install or update"}
            </button>
            <button
              disabled={integrationBusy()}
              onClick={() => void runIntegration(false)}
            >
              Uninstall
            </button>
            <button
              onClick={() => {
                clearHistory(localStorage, historyStorageKey());
                props.onError("Command history for this host was cleared.");
              }}
            >
              Clear history
            </button>
          </div>
          <Show when={integrationOutput()}>
            <pre class="terminal-integration-output">{integrationOutput()}</pre>
          </Show>
        </div>
      </Show>
      <Show when={completions().length}>
        <div
          class="terminal-completions"
          role="listbox"
          aria-label="Command completions"
          tabIndex={-1}
          ref={(element) => queueMicrotask(() => element.focus())}
          onKeyDown={(event) => {
            const count = completions().length;
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setCompletionIndex((index) => (index + 1) % count);
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setCompletionIndex((index) => (index - 1 + count) % count);
            } else if (event.key === "Enter" || event.key === "Tab") {
              event.preventDefault();
              acceptCompletion(completions()[completionIndex()]);
            } else if (event.key === "Escape") {
              event.preventDefault();
              setCompletions([]);
              terminal.focus();
            }
          }}
          onFocusOut={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node))
              setCompletions([]);
          }}
        >
          <For each={completions()}>
            {(command, index) => (
              <button
                role="option"
                aria-selected={index() === completionIndex()}
                class={index() === completionIndex() ? "active" : ""}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => acceptCompletion(command)}
              >
                {command}
              </button>
            )}
          </For>
          <small>↑↓ choose · Enter insert · Esc close</small>
        </div>
      </Show>
      <Show when={sftpOnlyNotice()}>
        <div class="terminal-paste-check terminal-sftp-only" role="alert">
          <strong>This server only allows file transfer here</strong>
          <p class="muted">
            The server allowed file transfer but refused a terminal for this
            user at this address and port. If ssh in Terminal or another app
            gets a shell, compare the address, port, user name and jump host
            with this host's editor.
          </p>
          <div class="dialog-actions">
            <button
              class="ghost-button"
              onClick={() => setSftpOnlyNotice(false)}
            >
              Close
            </button>
            <button
              class="ghost-button"
              onClick={() => {
                setSftpOnlyNotice(false);
                props.onSftpOnly?.(true);
              }}
            >
              Always open in Files
            </button>
            <button
              class="primary-button"
              onClick={() => {
                setSftpOnlyNotice(false);
                props.onSftpOnly?.(false);
              }}
            >
              Open in Files
            </button>
          </div>
        </div>
      </Show>
      <Show when={pastesOpen()}>
        <div class="terminal-pastes" role="menu" aria-label="Recent pastes">
          <For
            each={[...pasteHistory.list()]}
            fallback={
              <p class="muted">
                Text you paste here is listed until Weave quits. It is never
                saved to disk.
              </p>
            }
          >
            {(text) => (
              <button
                role="menuitem"
                title={text.length > 400 ? `${text.slice(0, 400)}…` : text}
                onClick={() => {
                  setPastesOpen(false);
                  sendPaste(text);
                }}
              >
                {text.replace(/\s+/g, " ").slice(0, 120)}
              </button>
            )}
          </For>
          <Show when={pasteHistory.list().length}>
            <button
              class="danger-text"
              onClick={() => {
                pasteHistory.clear();
                setPastesOpen(false);
              }}
            >
              Clear list
            </button>
          </Show>
        </div>
      </Show>
      <Show when={pasteRequest()}>
        {(text) => (
          <div
            class="terminal-paste-check"
            role="alertdialog"
            aria-label="Confirm multi-line paste"
          >
            <strong>Paste {pastedLines(text())} lines?</strong>
            <p class="muted">
              Each line may run as a command as soon as it arrives.
            </p>
            <pre>
              {text().length > 2000 ? `${text().slice(0, 2000)}…` : text()}
            </pre>
            <label class="check-label">
              <input
                type="checkbox"
                checked={skipPasteCheck()}
                onChange={(event) =>
                  setSkipPasteCheck(event.currentTarget.checked)
                }
              />
              Don’t ask again on {thisComputer}
            </label>
            <div class="dialog-actions">
              <button
                class="ghost-button"
                onClick={() => answerPaste("cancel")}
              >
                Cancel
              </button>
              <button
                class="ghost-button"
                onClick={() => answerPaste("oneLine")}
              >
                Paste as one line
              </button>
              <button
                class="primary-button"
                ref={(element) => queueMicrotask(() => element.focus())}
                onClick={() => answerPaste("paste")}
              >
                Paste
              </button>
            </div>
          </div>
        )}
      </Show>
      <Show when={passwordPrompt() && passwordProfile()}>
        <button
          class="terminal-password-fill"
          title="Type the password saved for this connection"
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => void fillPassword()}
        >
          Fill saved password
        </button>
      </Show>
      <Show when={triggerAlert()}>
        <div class="terminal-trigger-alert" role="status">
          {triggerAlert()}
        </div>
      </Show>
      <div
        class="terminal-host"
        ref={container}
        aria-label="Interactive terminal"
      />
    </div>
  );
}
