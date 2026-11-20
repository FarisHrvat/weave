import { confirmAction } from "./dialogs";
import {
  certificateStore,
  keychain,
  systemName,
  thisComputer,
} from "./platform";
import { profileAddress } from "./profileLabel";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  ChevronRight,
  Command,
  FolderOpen,
  GitBranch,
  HardDrive,
  Home,
  Moon,
  Network,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Search,
  Server,
  Settings2,
  Layers3,
  Sun,
  TerminalSquare,
  Plug,
  Power,
  Lock,
  Trash2,
  X,
  SquareTerminal,
  Copy,
  ClipboardCopy,
} from "lucide-solid";
import { api, errorMessage } from "./api";
import StartupDialog from "./StartupDialog";
import { installDialogKeys } from "./dialogKeys";
import { shortcutLabel } from "./shortcuts";
import {
  groupHosts,
  knownGroups,
  loadHostMeta,
  parseTags,
  saveHostMeta,
} from "./hostGroups";
import VncViewer from "./VncViewer";
import HostOptionsPanel from "./HostOptionsPanel";
import KeyboardShortcutsDialog from "./KeyboardShortcutsDialog";
import {
  actionFor,
  loadKeybindings,
  saveKeybindings,
  type KeyAction,
} from "./keybindings";
import { describeTeamSync } from "./teamVault";
import CloudDrivePanel, {
  isCloud,
  type CloudProtocol,
} from "./CloudDrivePanel";
import { cloudBucketUrl, presetRegion, type CloudPreset } from "./cloudPresets";
import FilesView from "./FilesView";
import GitView from "./GitView";
import TerminalView from "./TerminalView";
import { invoke } from "@tauri-apps/api/core";
import TerminalSearchAll from "./TerminalSearchAll";
import AppLockScreen from "./AppLockScreen";
import SshPromptDialog from "./SshPromptDialog";
import TextPromptDialog from "./TextPromptDialog";
import ContextMenu, { openContextMenu, type MenuItem } from "./ContextMenu";
import MenuButton from "./MenuButton";
import { forgetHostLocally } from "./forgetHost";
import { readSftpOnly, writeSftpOnly } from "./sftpOnly";
import { idleLockDue, type AppLockSettings } from "./appLock";
import { broadcastTargets } from "./terminalBroadcast";
import {
  clearTerminalActivity,
  markTerminalActivity,
} from "./terminalActivity";
import ConnectDialog from "./ConnectDialog";
import OAuthSignIn from "./OAuthSignIn";
import ServerReposDialog from "./ServerReposDialog";
import SettingsVaultDialog from "./SettingsVaultDialog";
import {
  connectionKindLabels,
  describeConnection,
  mergeImportedConnections,
  readRemoteConnections,
  writeRemoteConnections,
  type RemoteConnection,
} from "./remoteConnections";
import SnippetsPanel from "./SnippetsPanel";
import {
  pinnedSnippets,
  readSnippets,
  snippetVariables,
  type Snippet,
} from "./snippets";
import NetworkPanel from "./NetworkPanel";
import NetworkToolsPanel from "./NetworkToolsPanel";
import KnownHostsPanel from "./KnownHostsPanel";
import SSHKeysPanel from "./SSHKeysPanel";
import TunnelsPanel from "./TunnelsPanel";
import { loadTunnelPresets, presetsToAutoStart } from "./tunnelPresets";
import {
  remainingTime,
  sortTransfers,
  updateRate,
  type RateSample,
} from "./transferDisplay";
import type {
  FileLocation,
  ForgeAccount,
  ForgeRepository,
  Profile,
  Repo,
  TransferRecord,
  TransferSettings,
  Workspace,
  WorkspaceLayout,
  ServerCertificate,
} from "./types";

type Page = "home" | "git" | "files" | "terminal";
type Session = {
  key: string;
  label: string;
  profileId?: string;
  cwd?: string;
  connection?: RemoteConnection;
  startupCommand?: string;
};

const blankProfile = (): Profile => ({
  id: "",
  name: "",
  host: "",
  protocol: "ssh",
  port: 22,
  username: "",
  keyPath: null,
  jumpHost: null,
  color: "#9d91ec",
  favorite: false,
  tlsPinSha256: null,
  s3Region: null,
});

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function isHostedForgeUrl(url: string): boolean {
  return /^(?:https:\/\/|ssh:\/\/git@|git@)(?:github\.com|gitlab\.com)(?:[/:])/i.test(
    url.trim(),
  );
}

export default function App() {
  const [page, setPage] = createSignal<Page>("home");
  const [profiles, setProfiles] = createSignal<Profile[]>([]);
  const [hostQuery, setHostQuery] = createSignal("");
  const [repoQuery, setRepoQuery] = createSignal("");
  const [repos, setRepos] = createSignal<Repo[]>([]);
  const [workspaces, setWorkspaces] = createSignal<Workspace[]>([]);
  const [activeWorkspace, setActiveWorkspace] = createSignal<string>();
  const [workspaceEditor, setWorkspaceEditor] = createSignal<{
    id: string | null;
    name: string;
  }>();
  const [deletingWorkspace, setDeletingWorkspace] = createSignal<Workspace>();
  const [transfers, setTransfers] = createSignal<TransferRecord[]>([]);
  const [transferRates, setTransferRates] = createSignal<
    Record<string, RateSample>
  >({});
  const [transferSettings, setTransferSettings] =
    createSignal<TransferSettings>({ parallelism: 2, speedLimitKbps: 0 });
  const [activityOpen, setActivityOpen] = createSignal(false);
  const [activeRepo, setActiveRepo] = createSignal<string>();
  const [home, setHome] = createSignal("");
  const [fileLocations, setFileLocations] = createSignal<{
    left: FileLocation;
    right: FileLocation;
  }>();
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [activeSession, setActiveSession] = createSignal<string>();
  const [searchAllOpen, setSearchAllOpen] = createSignal(false);
  const [sftpOnlyHosts, setSftpOnlyHosts] = createSignal(
    readSftpOnly(localStorage),
  );
  function setSftpOnly(profileId: string, only: boolean) {
    const next = new Set(sftpOnlyHosts());
    if (only) next.add(profileId);
    else next.delete(profileId);
    setSftpOnlyHosts(next);
    try {
      writeSftpOnly(localStorage, next);
    } catch {
      // Still applies until Weave quits.
    }
  }
  const [lockSettings, setLockSettings] = createSignal<AppLockSettings>();
  const [locked, setLocked] = createSignal(false);
  let lastActive = Date.now();
  onMount(() => {
    void invoke<AppLockSettings>("app_lock_get")
      .then((settings) => {
        setLockSettings(settings);
        if (settings.enabled) setLocked(true);
      })
      .catch(() => undefined);
    const active = () => {
      if (!locked()) lastActive = Date.now();
    };
    const events = ["keydown", "pointerdown", "pointermove", "wheel"] as const;
    for (const name of events) window.addEventListener(name, active, true);
    const timer = window.setInterval(() => {
      if (!locked() && idleLockDue(lockSettings(), lastActive, Date.now()))
        setLocked(true);
    }, 15_000);
    onCleanup(() => {
      for (const name of events) window.removeEventListener(name, active, true);
      window.clearInterval(timer);
    });
  });
  function lockNow() {
    if (!lockSettings()?.enabled) {
      notify(
        "Turn on Lock Weave in Background and startup first, so Weave knows how to unlock.",
        true,
      );
      return;
    }
    setLocked(true);
  }
  const [readySessions, setReadySessions] = createSignal<Set<string>>(
    new Set(),
  );
  const [endedSessions, setEndedSessions] = createSignal<Set<string>>(
    new Set(),
  );
  const [splitSession, setSplitSession] = createSignal<string>();
  const [splitDirection, setSplitDirection] = createSignal<"row" | "column">(
    "row",
  );
  const [snippetsOpen, setSnippetsOpen] = createSignal(false);
  const [savedSnippets, setSavedSnippets] = createSignal<Snippet[]>([]);
  const [requestedSnippetId, setRequestedSnippetId] = createSignal<string>();
  const [connectOpen, setConnectOpen] = createSignal(false);
  const [serverReposOpen, setServerReposOpen] = createSignal(false);
  const [vaultOpen, setVaultOpen] = createSignal(false);
  const [menuBarMode, setMenuBarMode] = createSignal(true);
  const [vncSession, setVncSession] = createSignal<RemoteConnection | null>(
    null,
  );
  const [keybindings, setKeybindings] = createSignal(
    loadKeybindings(localStorage),
  );
  const [shortcutsOpen, setShortcutsOpen] = createSignal(false);
  const [startupOpen, setStartupOpen] = createSignal(false);
  const [broadcastInput, setBroadcastInput] = createSignal(false);
  const [broadcastExcluded, setBroadcastExcluded] = createSignal<Set<string>>(
    new Set(),
  );
  const [recordingSessions, setRecordingSessions] = createSignal<Set<string>>(
    new Set(),
  );
  const [activitySessions, setActivitySessions] = createSignal<Set<string>>(
    new Set(),
  );
  createEffect(() => {
    if (page() === "terminal") {
      setActivitySessions((current) =>
        clearTerminalActivity(current, [activeSession(), splitSession()]),
      );
    }
  });
  const broadcastTerminalCount = createMemo(
    () =>
      [...readySessions()].filter(
        (key) => !endedSessions().has(key) && !broadcastExcluded().has(key),
      ).length,
  );
  createEffect(() => {
    if (broadcastTerminalCount() < 2 || page() !== "terminal")
      setBroadcastInput(false);
  });
  const terminalIds = new Map<string, string>();
  const [profileEditor, setProfileEditor] = createSignal<Profile>();
  const [profilePassword, setProfilePassword] = createSignal("");
  const [s3TokenInput, setS3TokenInput] = createSignal("");
  const [cloudPreset, setCloudPreset] = createSignal<CloudPreset>("custom");
  const [cloudBucket, setCloudBucket] = createSignal("");
  const [cloudAccount, setCloudAccount] = createSignal("");
  const [cloudPresetError, setCloudPresetError] = createSignal("");
  const [clearS3Token, setClearS3Token] = createSignal(false);
  const [rememberPassword, setRememberPassword] = createSignal(false);
  const [awsProfile, setAwsProfile] = createSignal("");
  const [clientCert, setClientCert] = createSignal("");
  const [serverCert, setServerCert] = createSignal<ServerCertificate | null>(
    null,
  );
  const [serverCertLoading, setServerCertLoading] = createSignal(false);
  const [keychainIdentities, setKeychainIdentities] = createSignal<
    { id: string; name: string }[]
  >([]);
  const [awsProfiles, setAwsProfiles] = createSignal<string[]>([]);
  const [cloneOpen, setCloneOpen] = createSignal(false);
  const [cloneUrl, setCloneUrl] = createSignal("");
  const [cloneParent, setCloneParent] = createSignal("");
  const [cloneName, setCloneName] = createSignal("");
  const [cloneDepth, setCloneDepth] = createSignal("");
  const [cloneToken, setCloneToken] = createSignal("");
  const [cloneBrowseOpen, setCloneBrowseOpen] = createSignal(false);
  const [cloneBrowseServerUrl, setCloneBrowseServerUrl] = createSignal("");
  const [cloneBrowseError, setCloneBrowseError] = createSignal("");
  const [cloneProvider, setCloneProvider] = createSignal<"GitHub" | "GitLab">(
    "GitHub",
  );
  const [cloneAccounts, setCloneAccounts] = createSignal<ForgeAccount[]>([]);
  const [cloneRepositories, setCloneRepositories] = createSignal<
    ForgeRepository[]
  >([]);
  const [cloneRepoQuery, setCloneRepoQuery] = createSignal("");
  const [cloneRepoPage, setCloneRepoPage] = createSignal(0);
  const [cloneRepoHasMore, setCloneRepoHasMore] = createSignal(false);
  const [cloneRepoBusy, setCloneRepoBusy] = createSignal(false);
  const [cloneCreateOpen, setCloneCreateOpen] = createSignal(false);
  const [cloneCreateName, setCloneCreateName] = createSignal("");
  const [cloneCreateNamespace, setCloneCreateNamespace] = createSignal("");
  const [cloneCreateDescription, setCloneCreateDescription] = createSignal("");
  const [cloneCreatePrivate, setCloneCreatePrivate] = createSignal(true);
  let cloneRepoRequest = 0;
  const [cloneBusy, setCloneBusy] = createSignal(false);
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [paletteQuery, setPaletteQuery] = createSignal("");
  const [toast, setToast] = createSignal<{ message: string; error: boolean }>();
  const [light, setLight] = createSignal(
    localStorage.getItem("weave-theme") === "light",
  );
  const [hostMeta, setHostMeta] = createSignal(loadHostMeta(localStorage));
  const [editorGroup, setEditorGroup] = createSignal("");
  const [editorTags, setEditorTags] = createSignal("");
  const [collapsedGroups, setCollapsedGroups] = createSignal<Set<string>>(
    new Set(),
  );
  const visibleHosts = createMemo(() =>
    groupHosts(profiles(), hostMeta(), hostQuery()),
  );
  const visibleRepos = createMemo(() => {
    const query = repoQuery().trim().toLocaleLowerCase();
    if (!query) return repos();
    return repos().filter((repo) =>
      [repo.name, repo.path].some((value) =>
        value.toLocaleLowerCase().includes(query),
      ),
    );
  });
  let toastTimer: number | undefined;

  function notify(message: string, error = false) {
    setToast({ message, error });
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(
      () => setToast(undefined),
      error ? 8000 : 4000,
    );
  }

  async function load() {
    try {
      const [
        savedProfiles,
        savedRepos,
        homePath,
        savedTransfers,
        savedSettings,
        savedWorkspaces,
      ] = await Promise.all([
        api.profiles(),
        api.repos(),
        api.home(),
        api.transfers(),
        api.transferSettings(),
        api.workspaces(),
      ]);
      setProfiles(savedProfiles);
      setRepos(savedRepos);
      setHome(homePath);
      setFileLocations({
        left: { path: homePath, profileId: null },
        right: { path: homePath, profileId: null },
      });
      setTransfers(savedTransfers);
      setTransferSettings(savedSettings);
      setWorkspaces(savedWorkspaces);
      if (savedRepos.length) setActiveRepo(savedRepos[0].path);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function startSavedTunnels() {
    let presets;
    try {
      presets = presetsToAutoStart(
        loadTunnelPresets(localStorage),
        await api.tunnels(),
      );
    } catch (error) {
      notify(errorMessage(error), true);
      return;
    }
    const failed: string[] = [];
    for (const preset of presets) {
      try {
        await api.startTunnel(
          preset.profileId,
          preset.kind,
          preset.listenPort,
          preset.kind === "dynamic" ? "" : preset.targetHost,
          preset.kind === "dynamic" ? 0 : preset.targetPort,
        );
      } catch (error) {
        failed.push(`${preset.name}: ${errorMessage(error)}`);
      }
    }
    if (failed.length)
      notify(`Some saved tunnels did not start. ${failed.join(" ")}`, true);
  }

  onMount(() => {
    void load();
    try {
      setSavedSnippets(readSnippets());
    } catch (error) {
      notify(errorMessage(error), true);
    }
    if ("__TAURI_INTERNALS__" in window) {
      void startSavedTunnels();
      void api
        .backgroundMode()
        .then(setMenuBarMode)
        .catch(() => undefined);
    }
    let unlistenTransfers: (() => void) | undefined;
    if ("__TAURI_INTERNALS__" in window) {
      void listen<Parameters<typeof describeTeamSync>[0]>(
        "team-vault-synced",
        (event) => {
          void api.profiles().then(setProfiles);
          notify(describeTeamSync(event.payload));
        },
      );
      void listen<TransferRecord>("transfer-progress", (event) => {
        setTransferRates((current) => {
          const next = { ...current };
          const sample = updateRate(
            current[event.payload.id],
            event.payload,
            Date.now(),
          );
          if (sample) next[event.payload.id] = sample;
          else delete next[event.payload.id];
          return next;
        });
        setTransfers((current) =>
          sortTransfers([
            event.payload,
            ...current.filter((record) => record.id !== event.payload.id),
          ]),
        );
        if (event.payload.status === "failed")
          notify(event.payload.error ?? "Transfer failed.", true);
      }).then((unlisten) => {
        unlistenTransfers = unlisten;
      });
    }
    onCleanup(() => unlistenTransfers?.());
    document.documentElement.dataset.theme = light() ? "light" : "dark";
    const shortcut = (event: KeyboardEvent) => {
      if (event.key === "Escape") setPaletteOpen(false);
      if (shortcutsOpen()) return;
      const action = actionFor(keybindings(), event);
      if (!action) return;
      event.preventDefault();
      event.stopPropagation();
      runKeyAction(action);
    };
    // Capture first, so bound shortcuts work while a terminal has focus.
    window.addEventListener("keydown", shortcut, true);
    onCleanup(() => window.removeEventListener("keydown", shortcut, true));
    onCleanup(installDialogKeys());
  });

  function cycleTab(step: number) {
    const list = sessions();
    if (!list.length) return;
    const index = list.findIndex((session) => session.key === activeSession());
    const next = list[(index + step + list.length) % list.length];
    setActiveSession(next.key);
    setPage("terminal");
  }

  function runKeyAction(action: KeyAction) {
    const actions: Record<KeyAction, () => void> = {
      palette: () => {
        setPaletteOpen((value) => !value);
        setPaletteQuery("");
      },
      home: () => setPage("home"),
      terminal: () => setPage("terminal"),
      files: () => setPage("files"),
      git: () => setPage("git"),
      newTerminal: () => openTerminal(),
      closeTab: () => {
        const key = activeSession();
        if (key) closeTerminal(key);
      },
      nextTab: () => cycleTab(1),
      findTerminals: () => setSearchAllOpen(true),
      lock: () => lockNow(),
      previousTab: () => cycleTab(-1),
      splitRight: () => {
        if (activeSession()) splitTerminal("row");
      },
      splitDown: () => {
        if (activeSession()) splitTerminal("column");
      },
      broadcast: async () => {
        if (broadcastInput()) setBroadcastInput(false);
        else if (
          broadcastTerminalCount() >= 2 &&
          (await confirmAction(
            "Broadcast all typed input, including passwords, to every included connected terminal tab?",
          ))
        )
          setBroadcastInput(true);
      },
      newConnection: () => setConnectOpen(true),
      addHost: () => openProfileEditor(blankProfile()),
      theme: () => toggleTheme(),
    };
    actions[action]();
  }

  function toggleTheme() {
    const next = !light();
    setLight(next);
    document.documentElement.dataset.theme = next ? "light" : "dark";
    localStorage.setItem("weave-theme", next ? "light" : "dark");
  }

  async function chooseRepo() {
    try {
      const folder = await open({
        directory: true,
        multiple: false,
        title: "Open a Git repository",
      });
      if (typeof folder !== "string") return;
      const repo = await api.addRepo(folder);
      setRepos(await api.repos());
      setActiveRepo(repo.path);
      setPage("git");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function openAsRepo(path: string) {
    try {
      const repo = await api.addRepo(path);
      setRepos(await api.repos());
      setActiveRepo(repo.path);
      setPage("git");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function transferAction(
    action: "pause" | "resume" | "remove",
    id: string,
  ) {
    try {
      if (action === "pause") await api.pauseTransfer(id);
      if (action === "resume") await api.resumeTransfer(id);
      if (action === "remove") await api.removeTransfer(id);
      setTransfers(await api.transfers());
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function moveTransfer(id: string, direction: "up" | "down") {
    try {
      await api.moveTransfer(id, direction);
      setTransfers(await api.transfers());
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function redoTransfer(record: TransferRecord) {
    try {
      const queued =
        record.method === "scp" || record.method === "rsync"
          ? await api.enqueueDirectTransfer(
              record.method,
              record.sourceProfile,
              record.sourcePath,
              record.destinationProfile,
              record.destinationDir,
              record.conflict,
            )
          : await api.enqueueTransfer(
              record.sourceProfile,
              record.sourcePath,
              record.destinationProfile,
              record.destinationDir,
              record.conflict,
            );
      setTransfers((current) => sortTransfers([queued, ...current]));
      notify("Transfer queued again.");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function saveTransferOptions(event: Event) {
    event.preventDefault();
    try {
      const options = transferSettings();
      setTransferSettings(
        await api.saveTransferSettings(
          options.parallelism,
          options.speedLimitKbps,
        ),
      );
      notify("Transfer settings saved.");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function initRepo() {
    try {
      const folder = await open({
        directory: true,
        multiple: false,
        title: "Choose a folder for a new repository",
      });
      if (typeof folder !== "string") return;
      await api.init(folder);
      const repo = await api.addRepo(folder);
      setRepos(await api.repos());
      setActiveRepo(repo.path);
      setPage("git");
      notify("Repository initialized.");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function cloneRepo() {
    if (!cloneUrl().trim() || !cloneParent() || !cloneName().trim()) {
      notify("Enter a URL, destination folder, and name.", true);
      return;
    }
    if (
      cloneName().includes("/") ||
      cloneName().includes("\\") ||
      cloneName() === ".."
    ) {
      notify("Folder name cannot contain slashes.", true);
      return;
    }
    const destination = `${cloneParent().replace(/[\\/]$/, "")}/${cloneName().trim()}`;
    setCloneBusy(true);
    try {
      if (cloneToken().trim()) {
        if (isHostedForgeUrl(cloneUrl())) {
          await api.forgeConnectUrl(cloneUrl().trim(), cloneToken());
        } else {
          await api.forgeConnectUrlAs(
            cloneUrl().trim(),
            cloneProvider(),
            cloneToken(),
          );
        }
        setCloneToken("");
      }
      const depth = Number(cloneDepth());
      await api.clone(
        cloneUrl().trim(),
        destination,
        Number.isInteger(depth) && depth > 0 ? depth : undefined,
      );
      const repo = await api.addRepo(destination);
      setRepos(await api.repos());
      setActiveRepo(repo.path);
      setPage("git");
      closeClone();
      notify("Repository cloned.");
    } catch (error) {
      notify(errorMessage(error), true);
    } finally {
      setCloneBusy(false);
    }
  }

  function closeClone() {
    cloneRepoRequest += 1;
    setCloneToken("");
    setCloneBrowseOpen(false);
    setCloneBrowseServerUrl("");
    setCloneBrowseError("");
    setCloneCreateOpen(false);
    setCloneOpen(false);
  }

  async function loadCloneRepositories(
    provider: "GitHub" | "GitLab",
    page = 1,
    serverUrl = cloneBrowseServerUrl(),
  ) {
    const request = ++cloneRepoRequest;
    setCloneRepoBusy(true);
    setCloneBrowseError("");
    try {
      const found = serverUrl
        ? await api.forgeRepositoriesForUrl(serverUrl, page)
        : await api.forgeRepositories(provider, page);
      if (request !== cloneRepoRequest) return;
      setCloneRepositories(
        page === 1 ? found : [...cloneRepositories(), ...found],
      );
      setCloneRepoPage(page);
      setCloneRepoHasMore(found.length === 50);
    } catch (error) {
      if (request === cloneRepoRequest)
        setCloneBrowseError(errorMessage(error));
    } finally {
      if (request === cloneRepoRequest) setCloneRepoBusy(false);
    }
  }

  async function browseCloneRepositories(
    provider: "GitHub" | "GitLab" = cloneProvider(),
  ) {
    cloneRepoRequest += 1;
    setCloneRepoBusy(false);
    setCloneBrowseOpen(true);
    setCloneBrowseServerUrl("");
    setCloneBrowseError("");
    setCloneProvider(provider);
    setCloneRepoQuery("");
    setCloneRepositories([]);
    setCloneRepoPage(0);
    setCloneRepoHasMore(false);
    setCloneCreateOpen(false);
    try {
      const accounts = await api.forgeAccounts();
      if (cloneProvider() !== provider || !cloneBrowseOpen()) return;
      setCloneAccounts(accounts);
      if (
        accounts.find((account) => account.provider === provider)?.connected
      ) {
        await loadCloneRepositories(provider);
      }
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function browseSelfHostedRepositories() {
    const url = cloneUrl().trim();
    if (!url || isHostedForgeUrl(url)) {
      notify("Enter a self-hosted repository URL first.", true);
      return;
    }
    cloneRepoRequest += 1;
    setCloneBrowseServerUrl(url);
    setCloneBrowseOpen(true);
    setCloneBrowseError("");
    setCloneRepositories([]);
    setCloneRepoPage(0);
    setCloneRepoHasMore(false);
    setCloneRepoQuery("");
    setCloneRepoBusy(true);
    try {
      if (cloneToken().trim()) {
        await api.forgeConnectUrlAs(url, cloneProvider(), cloneToken());
        setCloneToken("");
      }
      await loadCloneRepositories(cloneProvider(), 1, url);
    } catch (error) {
      setCloneBrowseError(errorMessage(error));
    } finally {
      setCloneRepoBusy(false);
    }
  }

  async function connectCloneProvider() {
    if (!cloneToken().trim()) {
      notify("Enter a personal access token to connect this account.", true);
      return;
    }
    setCloneRepoBusy(true);
    try {
      await api.forgeConnectProvider(cloneProvider(), cloneToken());
      setCloneToken("");
      setCloneAccounts(await api.forgeAccounts());
      notify(`${cloneProvider()} account connected.`);
      await loadCloneRepositories(cloneProvider());
    } catch (error) {
      notify(errorMessage(error), true);
    } finally {
      setCloneRepoBusy(false);
    }
  }

  function chooseCloneRepository(
    repo: ForgeRepository,
    protocol: "https" | "ssh",
  ) {
    setCloneUrl(protocol === "https" ? repo.httpsUrl : repo.sshUrl);
    setCloneName(repo.name.split("/").at(-1) ?? "");
    setCloneBrowseOpen(false);
  }

  async function createHostedRepository() {
    if (cloneRepoBusy()) return;
    const provider = cloneProvider();
    if (!cloneCreateName().trim()) {
      setCloneBrowseError("Enter a name for the new repository.");
      return;
    }
    setCloneRepoBusy(true);
    setCloneBrowseError("");
    try {
      const created = await api.forgeCreateRepository(
        provider,
        cloneBrowseServerUrl(),
        cloneCreateName(),
        cloneCreateDescription(),
        cloneCreatePrivate(),
        cloneCreateNamespace(),
      );
      chooseCloneRepository(created, "https");
      setCloneCreateOpen(false);
      setCloneCreateName("");
      setCloneCreateNamespace("");
      setCloneCreateDescription("");
      notify(
        `${created.name} created on ${provider}. Choose a local folder to clone it.`,
      );
    } catch (error) {
      setCloneBrowseError(errorMessage(error));
    } finally {
      setCloneRepoBusy(false);
    }
  }

  const filteredCloneRepositories = createMemo(() => {
    const query = cloneRepoQuery().trim().toLocaleLowerCase();
    return query
      ? cloneRepositories().filter((repo) =>
          repo.name.toLocaleLowerCase().includes(query),
        )
      : cloneRepositories();
  });

  /** "HPC" for the first tab, then "HPC 2", "HPC 3". */
  function uniqueTabLabel(name: string) {
    const taken = new Set(sessions().map((session) => session.label));
    if (!taken.has(name)) return name;
    let number = 2;
    while (taken.has(`${name} ${number}`)) number++;
    return `${name} ${number}`;
  }

  function openTerminal(
    profile?: Profile,
    cwd?: string,
    startupCommand?: string,
  ) {
    const session = {
      key: crypto.randomUUID(),
      label: uniqueTabLabel(
        profile?.name ??
          (cwd ? cwd.split("/").pop() || "Local" : "Local shell"),
      ),
      profileId: profile?.id,
      cwd,
      startupCommand,
    };
    setSessions((current) => [...current, session]);
    setActiveSession(session.key);
    setPage("terminal");
  }

  function openRemoteTerminal(connection: RemoteConnection) {
    const session = {
      key: crypto.randomUUID(),
      label: uniqueTabLabel(connection.name),
      profileId:
        connection.kind === "mosh" || connection.kind === "x11"
          ? connection.profileId
          : undefined,
      connection,
    };
    setSessions((current) => [...current, session]);
    setActiveSession(session.key);
    setPage("terminal");
  }

  function openInFiles(profile: Profile) {
    setFileLocations((current) => ({
      left: { path: ".", profileId: profile.id },
      right: current?.right ?? { path: home(), profileId: null },
    }));
    setPage("files");
  }

  async function duplicateProfile(profile: Profile) {
    try {
      await api.saveProfile({
        ...profile,
        id: "",
        name: `${profile.name} copy`,
      });
      setProfiles(await api.profiles());
      notify(`Made a copy of “${profile.name}”. Its password is not copied.`);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  function hostMenu(event: MouseEvent, profile: Profile) {
    const items: MenuItem[] = [];
    if (profile.protocol === "ssh")
      items.push({
        label: "Open terminal",
        icon: <SquareTerminal size={15} />,
        run: () => openTerminal(profile),
      });
    items.push(
      {
        label: "Open in Files",
        icon: <FolderOpen size={15} />,
        run: () => openInFiles(profile),
      },
      "separator",
      {
        label: "Edit…",
        icon: <Settings2 size={15} />,
        run: () => openProfileEditor({ ...profile }),
      },
      {
        label: "Duplicate",
        icon: <Copy size={15} />,
        run: () => void duplicateProfile(profile),
      },
      {
        label: "Copy address",
        icon: <ClipboardCopy size={15} />,
        run: () =>
          void navigator.clipboard
            .writeText(profileAddress(profile))
            .then(() => notify("Address copied."))
            .catch((error) => notify(errorMessage(error), true)),
      },
      "separator",
      {
        label: "Delete host…",
        icon: <Trash2 size={15} />,
        danger: true,
        run: () => void deleteProfile(profile),
      },
    );
    openContextMenu(event, items);
  }

  function openConnection(profile: Profile) {
    if (profile.protocol === "ssh" && !sftpOnlyHosts().has(profile.id)) {
      openTerminal(profile);
      return;
    }
    setFileLocations((current) => ({
      left: { path: ".", profileId: profile.id },
      right: current?.right ?? { path: home(), profileId: null },
    }));
    setPage("files");
  }

  function splitTerminal(direction: "row" | "column") {
    const source = sessions().find(
      (session) => session.key === activeSession(),
    );
    if (!source) return;
    const profile = profiles().find((item) => item.id === source.profileId);
    if (source.profileId && !profile) {
      notify(
        "This host profile no longer exists. Save it again to reconnect.",
        true,
      );
      return;
    }
    setSplitSession(source.key);
    setSplitDirection(direction);
    if (source.connection) openRemoteTerminal(source.connection);
    else openTerminal(profile, source.cwd);
  }

  async function closeTerminal(key: string) {
    if (
      recordingSessions().has(key) &&
      !(await confirmAction(
        "This terminal has an unsaved recording. Close it and discard the recording?",
      ))
    )
      return;
    terminalIds.delete(key);
    setActivitySessions((current) => clearTerminalActivity(current, [key]));
    setBroadcastExcluded((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    setReadySessions((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    setEndedSessions((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    const remaining = sessions().filter((session) => session.key !== key);
    setSessions(remaining);
    if (splitSession() === key) setSplitSession(undefined);
    if (activeSession() === key) {
      setActiveSession(remaining.at(-1)?.key);
      if (splitSession() === remaining.at(-1)?.key) setSplitSession(undefined);
      if (!remaining.length) setPage("home");
    }
  }

  async function reconnectTerminal(key: string) {
    if (
      recordingSessions().has(key) &&
      !(await confirmAction(
        "This terminal has an unsaved recording. Reconnect and discard the recording?",
      ))
    )
      return;
    const replacement = crypto.randomUUID();
    setActivitySessions((current) => clearTerminalActivity(current, [key]));
    setSessions((current) =>
      current.map((session) =>
        session.key === key ? { ...session, key: replacement } : session,
      ),
    );
    if (activeSession() === key) setActiveSession(replacement);
    if (splitSession() === key) setSplitSession(replacement);
    setEndedSessions((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    setReadySessions((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    terminalIds.delete(key);
    setBroadcastExcluded((current) => {
      if (!current.has(key)) return current;
      const next = new Set(current);
      next.delete(key);
      next.add(replacement);
      return next;
    });
  }

  async function insertSnippet(text: string, sessionKey = activeSession()) {
    const id = terminalIds.get(sessionKey ?? "");
    if (!id) {
      notify(
        "Wait for the terminal to connect before inserting a command.",
        true,
      );
      return;
    }
    try {
      await api.termWrite(id, text);
      setSnippetsOpen(false);
      setRequestedSnippetId(undefined);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  function usePinnedSnippet(item: Snippet, sessionKey: string) {
    setActiveSession(sessionKey);
    if (snippetVariables(item.body).length) {
      setRequestedSnippetId(item.id);
      setSnippetsOpen(true);
    } else {
      void insertSnippet(item.body, sessionKey);
    }
  }

  async function restoreWorkspace(workspace: Workspace) {
    if (
      recordingSessions().size > 0 &&
      !(await confirmAction(
        "Opening this workspace will discard unsaved terminal recordings. Continue?",
      ))
    )
      return;
    const layout = workspace.layout;
    if (
      !["home", "git", "files", "terminal"].includes(layout.page) ||
      !Array.isArray(layout.terminals)
    ) {
      notify(`Workspace “${workspace.name}” has an invalid layout.`, true);
      return;
    }
    const restored = layout.terminals.map((terminal) => ({
      ...terminal,
      key: crypto.randomUUID(),
    }));
    setSessions(restored);
    setReadySessions(new Set<string>());
    setEndedSessions(new Set<string>());
    const activeIndex =
      Number.isInteger(layout.activeTerminal) &&
      layout.activeTerminal! >= 0 &&
      layout.activeTerminal! < restored.length
        ? layout.activeTerminal!
        : restored.length - 1;
    setActiveSession(restored[activeIndex]?.key);
    const split = layout.terminalSplit;
    setSplitSession(
      split &&
        Number.isInteger(split.index) &&
        split.index >= 0 &&
        split.index < restored.length &&
        split.index !== activeIndex
        ? restored[split.index].key
        : undefined,
    );
    setSplitDirection(split?.direction === "column" ? "column" : "row");
    setActiveRepo(layout.repo ?? undefined);
    if (layout.files?.left && layout.files?.right)
      setFileLocations(layout.files);
    setPage(
      layout.page === "terminal" && !restored.length ? "home" : layout.page,
    );
    setActiveWorkspace(workspace.id);
    notify(`Workspace “${workspace.name}” opened.`);
  }

  async function saveWorkspace(event: Event) {
    event.preventDefault();
    const editor = workspaceEditor();
    if (!editor) return;
    const layout: WorkspaceLayout = {
      page: page(),
      repo: activeRepo() ?? null,
      terminals: sessions().map(({ label, profileId, cwd, connection }) => ({
        label,
        profileId,
        cwd,
        connection,
      })),
      activeTerminal: sessions().findIndex(
        (session) => session.key === activeSession(),
      ),
      terminalSplit: splitSession()
        ? {
            index: sessions().findIndex(
              (session) => session.key === splitSession(),
            ),
            direction: splitDirection(),
          }
        : undefined,
      files: fileLocations(),
    };
    try {
      const saved = await api.saveWorkspace(editor.id, editor.name, layout);
      setWorkspaces(await api.workspaces());
      setActiveWorkspace(saved.id);
      setWorkspaceEditor(undefined);
      notify(`Workspace “${saved.name}” saved.`);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function deleteWorkspace() {
    const workspace = deletingWorkspace();
    if (!workspace) return;
    try {
      await api.deleteWorkspace(workspace.id);
      setWorkspaces(await api.workspaces());
      if (activeWorkspace() === workspace.id) setActiveWorkspace(undefined);
      setDeletingWorkspace(undefined);
      notify(`Workspace “${workspace.name}” deleted.`);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function readServerCertificate() {
    const profile = profileEditor();
    if (!profile) return;
    setServerCertLoading(true);
    setServerCert(null);
    try {
      setServerCert(
        await api.serverCertificate(
          profile.protocol,
          profile.host,
          profile.port,
          profile.id || null,
        ),
      );
    } catch (error) {
      notify(errorMessage(error), true);
    } finally {
      setServerCertLoading(false);
    }
  }

  function useServerFingerprint(keepExisting: boolean) {
    const profile = profileEditor();
    const certificate = serverCert();
    if (!profile || !certificate) return;
    const current = (profile.tlsPinSha256 ?? "")
      .split(/[\s,]+/)
      .filter(Boolean);
    const pins = keepExisting
      ? [...current, certificate.sha256].slice(-3)
      : [certificate.sha256];
    setProfileEditor({ ...profile, tlsPinSha256: pins.join(", ") });
    setServerCert(null);
  }

  async function saveProfile(profile: Profile) {
    try {
      const previous = profiles().find((saved) => saved.id === profile.id);
      if (
        profile.protocol !== "ssh" &&
        !isCloud(profile.protocol) &&
        !(
          ["ftp", "ftps", "ftps-implicit"].includes(profile.protocol) &&
          profile.username.trim().toLowerCase() === "anonymous"
        ) &&
        (!previous || previous.protocol !== profile.protocol) &&
        !profilePassword() &&
        !(profile.protocol === "s3" && awsProfile())
      ) {
        notify(
          profile.protocol === "s3"
            ? "Enter the secret access key before saving this S3 connection."
            : "Enter a password before saving this file connection.",
          true,
        );
        return;
      }
      const usesAws = profile.protocol === "s3" && !!awsProfile();
      const saved = await api.saveProfile(
        usesAws && !profile.username.trim()
          ? { ...profile, username: "aws-profile" }
          : profile,
      );
      if (saved.protocol === "s3")
        await api.awsLink(saved.id, usesAws ? awsProfile() : null);
      if (["webdav", "ftps", "ftps-implicit"].includes(saved.protocol))
        await api.webdavClientCertSet(saved.id, clientCert() || null);
      setProfileEditor(saved);
      const nextMeta = {
        ...hostMeta(),
        [saved.id]: {
          group: editorGroup().trim(),
          tags: parseTags(editorTags()),
        },
      };
      setHostMeta(nextMeta);
      try {
        saveHostMeta(localStorage, nextMeta);
      } catch {
        notify(`Could not save the group and tags on ${thisComputer}.`, true);
      }
      if (profilePassword())
        await api.setProfilePassword(
          saved.id,
          profilePassword(),
          rememberPassword(),
        );
      if (saved.protocol === "s3") {
        if (clearS3Token()) await api.clearS3Token(saved.id);
        else if (s3TokenInput())
          await api.setS3Token(saved.id, s3TokenInput(), rememberPassword());
      }
      setProfiles(await api.profiles());
      setProfilePassword("");
      setS3TokenInput("");
      closeProfileEditor();
      notify("Connection saved.");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function importSshConfig() {
    try {
      const count = await api.importSsh();
      setProfiles(await api.profiles());
      closeProfileEditor();
      notify(
        count
          ? `Imported ${count} host${count === 1 ? "" : "s"} from OpenSSH.`
          : "All named hosts are already saved.",
      );
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function importFileZilla() {
    const path = await open({
      multiple: false,
      filters: [{ name: "FileZilla XML", extensions: ["xml"] }],
    });
    if (!path) return;
    try {
      const report = await api.importFileZilla(path);
      setProfiles(await api.profiles());
      closeProfileEditor();
      notify(
        `Imported ${report.imported} FileZilla site${report.imported === 1 ? "" : "s"}. ${report.skipped} skipped. Enter passwords where required before connecting.`,
      );
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function importSessions() {
    const chosen = await open({
      multiple: true,
      filters: [
        {
          name: "WinSCP, PuTTY, MobaXterm, Cyberduck, SecureCRT or Termius CSV",
          extensions: [
            "ini",
            "reg",
            "mxtsessions",
            "mobaconf",
            "duck",
            "xml",
            "csv",
          ],
        },
      ],
    });
    const paths = Array.isArray(chosen) ? chosen : chosen ? [chosen] : [];
    if (!paths.length) return;
    try {
      const report = await api.importSessions(paths);
      const merged = mergeImportedConnections(
        readRemoteConnections(localStorage),
        report.connections,
        () => crypto.randomUUID(),
      );
      if (merged.added)
        writeRemoteConnections(localStorage, merged.connections);
      setProfiles(await api.profiles());
      closeProfileEditor();
      const extra = merged.added
        ? ` ${merged.added} Telnet, RDP or VNC session${merged.added === 1 ? " was" : "s were"} added to Connect….`
        : "";
      notify(
        `Imported ${report.imported} host${report.imported === 1 ? "" : "s"}.${extra} ${report.skipped} skipped (already saved, or a type Weave does not open). Enter passwords when connecting.`,
      );
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  async function deleteProfile(profile: Profile) {
    if (
      !(await confirmAction(
        `Delete “${profile.name}”?\n\nIts saved password and sign-ins, settings, tunnels, sync jobs, queued transfers, terminal history, snippets and bookmarks for this host are removed from ${thisComputer}. Your files on the server are not touched.`,
        "Delete host",
      ))
    )
      return;
    try {
      await api.deleteProfile(profile.id);
      forgetHostLocally(localStorage, profile.id);
      setSftpOnly(profile.id, false);
      const meta = { ...hostMeta() };
      delete meta[profile.id];
      setHostMeta(meta);
      setSavedSnippets(readSnippets());
      for (const session of sessions().filter(
        (item) =>
          item.profileId === profile.id ||
          item.connection?.profileId === profile.id,
      ))
        await closeTerminal(session.key);
      setFileLocations((current) => {
        if (!current) return current;
        const local = { path: home(), profileId: null };
        return {
          left: current.left?.profileId === profile.id ? local : current.left,
          right:
            current.right?.profileId === profile.id ? local : current.right,
        };
      });
      setProfiles(await api.profiles());
      closeProfileEditor();
      notify(`Deleted “${profile.name}” and everything saved for it.`);
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  function openProfileEditor(profile: Profile) {
    setProfilePassword("");
    setS3TokenInput("");
    setClearS3Token(false);
    setCloudPreset("custom");
    setCloudBucket("");
    setCloudAccount("");
    setCloudPresetError("");
    setRememberPassword(false);
    setAwsProfile("");
    setClientCert("");
    setServerCert(null);
    const info = hostMeta()[profile.id];
    setEditorGroup(info?.group ?? "");
    setEditorTags(info?.tags.join(", ") ?? "");
    setProfileEditor(profile);
    if ("__TAURI_INTERNALS__" in window) {
      void api
        .awsProfiles()
        .then(setAwsProfiles)
        .catch(() => setAwsProfiles([]));
      if (
        profile.id &&
        ["webdav", "ftps", "ftps-implicit"].includes(profile.protocol)
      )
        void api
          .webdavClientCertGet(profile.id)
          .then((path) => setClientCert(path ?? ""))
          .catch(() => undefined);
      if (profile.id && profile.protocol === "s3")
        void api
          .awsLinkGet(profile.id)
          .then((name) => setAwsProfile(name ?? ""))
          .catch(() => undefined);
    }
  }

  function closeProfileEditor() {
    setProfilePassword("");
    setS3TokenInput("");
    setClearS3Token(false);
    setCloudPresetError("");
    setProfileEditor(undefined);
  }

  async function clearProfilePassword(profileId: string) {
    try {
      await api.clearProfilePassword(profileId);
      setProfilePassword("");
      notify("Saved FTP password removed.");
    } catch (error) {
      notify(errorMessage(error), true);
    }
  }

  const paletteItems = createMemo(() =>
    [
      { label: "Open a repository", detail: "Git", run: chooseRepo },
      {
        label: "Clone a repository",
        detail: "Git",
        run: () => setCloneOpen(true),
      },
      { label: "Initialize a repository", detail: "Git", run: initRepo },
      {
        label: "Keyboard shortcuts",
        detail: "Settings",
        run: () => setShortcutsOpen(true),
      },
      {
        label: "Background and startup",
        detail: "Settings",
        run: () => setStartupOpen(true),
      },
      {
        label: "Export or import settings vault",
        detail: "Sync",
        run: () => setVaultOpen(true),
      },
      {
        label: "Manage server repositories",
        detail: "Git over SSH",
        run: () => setServerReposOpen(true),
      },
      {
        label: "New connection (Telnet, Serial, Mosh, X11, VNC, RDP)",
        detail: "Remote access",
        run: () => setConnectOpen(true),
      },
      { label: "Browse files", detail: "Files", run: () => setPage("files") },
      {
        label: "Open local terminal",
        detail: "Terminal",
        run: () => openTerminal(),
      },
      {
        label: "Lock Weave",
        detail: "Security",
        run: () => lockNow(),
      },
      {
        label: "Find in all terminal tabs",
        detail: "Terminal",
        run: () => setSearchAllOpen(true),
      },
      {
        label: "Add a host",
        detail: "Connections",
        run: () => openProfileEditor(blankProfile()),
      },
      {
        label: "Save current workspace",
        detail: "Workspace",
        run: () =>
          setWorkspaceEditor({
            id: activeWorkspace() ?? null,
            name:
              workspaces().find((item) => item.id === activeWorkspace())
                ?.name ?? "",
          }),
      },
      ...workspaces().map((workspace) => ({
        label: `Open ${workspace.name}`,
        detail: "Workspace",
        run: () => restoreWorkspace(workspace),
      })),
      ...repos().map((repo) => ({
        label: `Open ${repo.name}`,
        detail: "Repository",
        run: () => {
          setActiveRepo(repo.path);
          setPage("git");
        },
      })),
      ...profiles().map((profile) => ({
        label: `Connect to ${profile.name}`,
        detail: "SSH",
        run: () => openConnection(profile),
      })),
    ].filter((item) =>
      item.label.toLowerCase().includes(paletteQuery().toLowerCase()),
    ),
  );

  return (
    <>
      <div class="app-shell" inert={locked()}>
        <aside class="sidebar">
          <div class="brand">
            <span class="brand-mark">w</span>
            <span>weave</span>
            <span class="brand-beta">ALPHA</span>
          </div>
          <div class="sidebar-content">
            <div class="nav-group">
              <div class="sidebar-caption">WORKSPACE</div>
              <button
                class={`nav-item ${page() === "home" ? "active" : ""}`}
                onClick={() => setPage("home")}
              >
                <Home size={18} /> Overview
              </button>
              <button
                class={`nav-item ${page() === "git" ? "active" : ""}`}
                onClick={() => setPage("git")}
              >
                <GitBranch size={18} /> Git
              </button>
              <button
                class={`nav-item ${page() === "files" ? "active" : ""}`}
                onClick={() => setPage("files")}
              >
                <FolderOpen size={18} /> Files
              </button>
              <button
                class={`nav-item ${page() === "terminal" ? "active" : ""}`}
                onClick={() =>
                  sessions().length
                    ? (setPage("terminal"),
                      setActiveSession(sessions().at(-1)!.key))
                    : openTerminal()
                }
              >
                <TerminalSquare size={18} /> Terminal
              </button>
              <button class="nav-item" onClick={() => setConnectOpen(true)}>
                <Plug size={18} /> Connect…
              </button>
              <button class="nav-item" onClick={() => setVaultOpen(true)}>
                <Lock size={18} /> Settings vault
              </button>
            </div>
            <div class="nav-group saved-group">
              <div class="sidebar-caption caption-action">
                WORKSPACES
                <button
                  class="mini-button"
                  title="Save workspace"
                  onClick={() => setWorkspaceEditor({ id: null, name: "" })}
                >
                  <Plus size={16} />
                </button>
              </div>
              <For
                each={workspaces()}
                fallback={
                  <p class="sidebar-empty">
                    Save your current tabs as a workspace.
                  </p>
                }
              >
                {(workspace) => (
                  <div class="saved-item workspace-row">
                    <button onClick={() => restoreWorkspace(workspace)}>
                      <Layers3 size={15} />
                      <span class="truncate">{workspace.name}</span>
                    </button>
                    <button
                      class="mini-button"
                      title={`Edit ${workspace.name}`}
                      onClick={() =>
                        setWorkspaceEditor({
                          id: workspace.id,
                          name: workspace.name,
                        })
                      }
                    >
                      <Settings2 size={14} />
                    </button>
                  </div>
                )}
              </For>
            </div>
            <div class="nav-group saved-group">
              <div class="sidebar-caption caption-action">
                HOSTS{" "}
                <button
                  class="mini-button"
                  title="Add host"
                  onClick={() => openProfileEditor(blankProfile())}
                >
                  <Plus size={16} />
                </button>
              </div>
              <input
                class="sidebar-search"
                type="search"
                aria-label="Search saved connections"
                placeholder="Find a connection or #tag"
                value={hostQuery()}
                onInput={(event) => setHostQuery(event.currentTarget.value)}
              />
              <For
                each={visibleHosts()}
                fallback={
                  <p class="sidebar-empty">
                    {profiles().length
                      ? "No matching connections."
                      : "Add a connection to get started."}
                  </p>
                }
              >
                {(section) => (
                  <>
                    <Show
                      when={
                        section.group ||
                        visibleHosts().some((item) => item.group)
                      }
                    >
                      <button
                        class="host-group-heading"
                        aria-expanded={!collapsedGroups().has(section.group)}
                        onClick={() =>
                          setCollapsedGroups((current) => {
                            const next = new Set(current);
                            if (next.has(section.group))
                              next.delete(section.group);
                            else next.add(section.group);
                            return next;
                          })
                        }
                      >
                        <ChevronRight
                          size={12}
                          class={
                            collapsedGroups().has(section.group)
                              ? ""
                              : "host-group-open"
                          }
                        />
                        <span class="truncate">
                          {section.group || "Other hosts"}
                        </span>
                        <small>{section.hosts.length}</small>
                      </button>
                    </Show>
                    <Show when={!collapsedGroups().has(section.group)}>
                      <For each={section.hosts}>
                        {(profile) => (
                          <button
                            class="saved-item"
                            title={
                              hostMeta()[profile.id]?.tags.length
                                ? `#${hostMeta()[profile.id].tags.join(" #")}`
                                : undefined
                            }
                            onClick={() => openConnection(profile)}
                            onContextMenu={(event) => hostMenu(event, profile)}
                          >
                            <span
                              class="host-dot"
                              style={{ background: profile.color }}
                            />
                            <span class="truncate">{profile.name}</span>
                            <ChevronRight size={14} />
                          </button>
                        )}
                      </For>
                    </Show>
                  </>
                )}
              </For>
            </div>
            <div class="nav-group saved-group">
              <div class="sidebar-caption caption-action">
                REPOSITORIES{" "}
                <button
                  class="mini-button"
                  title="Open repository"
                  onClick={chooseRepo}
                >
                  <Plus size={16} />
                </button>
              </div>
              <input
                class="sidebar-search"
                type="search"
                aria-label="Search repositories"
                placeholder="Find a repository"
                value={repoQuery()}
                onInput={(event) => setRepoQuery(event.currentTarget.value)}
              />
              <For
                each={visibleRepos()}
                fallback={
                  <p class="sidebar-empty">
                    {repos().length
                      ? "No matching repositories."
                      : "Open a local repository."}
                  </p>
                }
              >
                {(repo) => (
                  <button
                    class="saved-item"
                    onClick={() => {
                      setActiveRepo(repo.path);
                      setPage("git");
                    }}
                  >
                    <GitBranch size={15} />
                    <span class="truncate">{repo.name}</span>
                    <ChevronRight size={14} />
                  </button>
                )}
              </For>
            </div>
          </div>
          <div class="sidebar-bottom">
            <button
              class="nav-item"
              title="Menu bar, start at login, and sync jobs while Weave is quit"
              onClick={() => setStartupOpen(true)}
            >
              <Power size={18} />{" "}
              {menuBarMode() ? "Runs in menu bar" : "Quits on close"}
            </button>
            <button class="nav-item" onClick={toggleTheme}>
              {light() ? <Moon size={18} /> : <Sun size={18} />}{" "}
              {light() ? "Dark theme" : "Light theme"}
            </button>
            <div class="sidebar-status">
              <span class="status-pulse" /> Local workspace <span>v0.1</span>
            </div>
          </div>
        </aside>
        <Show when={startupOpen()}>
          <StartupDialog
            menuBarMode={menuBarMode()}
            onMenuBarMode={setMenuBarMode}
            onLockChange={setLockSettings}
            onClose={() => setStartupOpen(false)}
          />
        </Show>

        <div class="main-shell">
          <header class="topbar">
            <div class="topbar-breadcrumb">
              <span>Workspace</span>
              <ChevronRight size={14} />
              <strong>
                {page() === "home"
                  ? "Overview"
                  : page() === "git"
                    ? "Git"
                    : page() === "files"
                      ? "Files"
                      : "Terminal"}
              </strong>
            </div>
            <button
              class="search-trigger"
              onClick={() => {
                setPaletteOpen(true);
                setPaletteQuery("");
              }}
            >
              <Search size={16} />
              <span>Search anything…</span>
              <kbd>{shortcutLabel(keybindings().palette)}</kbd>
            </button>
            <div class="topbar-right">
              <span class="topbar-local">
                <span class="status-pulse" /> Local-first workspace
              </span>
              <button
                class="icon-button"
                title="Toggle theme"
                onClick={toggleTheme}
              >
                <Settings2 size={18} />
              </button>
            </div>
          </header>
          <div class="tabbar">
            <button
              class={`tab ${page() === "home" ? "selected" : ""}`}
              onClick={() => setPage("home")}
            >
              <Home size={14} /> Overview
            </button>
            <button
              class={`tab ${page() === "files" ? "selected" : ""}`}
              onClick={() => setPage("files")}
            >
              <FolderOpen size={14} /> Files
            </button>
            <Show when={activeRepo()}>
              <button
                class={`tab ${page() === "git" ? "selected" : ""}`}
                onClick={() => setPage("git")}
              >
                <GitBranch size={14} />{" "}
                {repos().find((repo) => repo.path === activeRepo())?.name ??
                  "Git"}
              </button>
            </Show>
            <For each={sessions()}>
              {(session) => (
                <button
                  class={`tab ${page() === "terminal" && activeSession() === session.key ? "selected" : ""}`}
                  onClick={() => {
                    if (splitSession() === session.key)
                      setSplitSession(activeSession());
                    setActiveSession(session.key);
                    setPage("terminal");
                  }}
                >
                  <TerminalSquare size={14} />{" "}
                  <span class="truncate">{session.label}</span>
                  <Show when={activitySessions().has(session.key)}>
                    <span
                      class="tab-activity-dot"
                      title="New terminal output"
                    />
                  </Show>
                  <Show when={broadcastExcluded().has(session.key)}>
                    <span title="Excluded from broadcast input"> · muted</span>
                  </Show>
                  <span
                    class="tab-close"
                    title="Close terminal"
                    onClick={(event) => {
                      event.stopPropagation();
                      closeTerminal(session.key);
                    }}
                  >
                    <X size={13} />
                  </span>
                </button>
              )}
            </For>
            <button
              class="tab-add"
              title="New terminal"
              onClick={() => openTerminal()}
            >
              <Plus size={17} />
            </button>
          </div>
          <main class="main-content">
            <Show when={page() === "home"}>
              <div class="home-view">
                <div class="hero">
                  <div class="hero-copy">
                    <span class="eyebrow">YOUR WORK, CONNECTED</span>
                    <h1>Everything in its place.</h1>
                    <p>
                      Manage code, move files, and work in a terminal from one
                      calm workspace.
                    </p>
                    <div class="hero-actions">
                      <button class="primary-button" onClick={chooseRepo}>
                        <GitBranch size={17} /> Open repository
                      </button>
                      <button
                        class="ghost-button"
                        onClick={() => openProfileEditor(blankProfile())}
                      >
                        <Server size={17} /> Add a host
                      </button>
                    </div>
                  </div>
                  <div class="hero-art">
                    <div class="hero-card card-one">
                      <span class="tiny-dot" /> git status <span>✓ clean</span>
                    </div>
                    <div class="hero-card card-two">
                      <span class="tiny-dot purple" /> ~/projects <span>↗</span>
                    </div>
                    <div class="hero-card card-three">
                      <span class="terminal-caret">❯</span> ready to create
                    </div>
                    <div class="hero-orbit" />
                  </div>
                </div>
                <div class="home-grid">
                  <section class="home-card">
                    <div class="card-head">
                      <div class="card-icon purple-bg">
                        <GitBranch size={20} />
                      </div>
                      <div>
                        <h2>Repositories</h2>
                        <p>Pick up where you left off</p>
                      </div>
                      <button
                        class="mini-button"
                        title="Open repository"
                        onClick={chooseRepo}
                      >
                        <Plus size={18} />
                      </button>
                    </div>
                    <For
                      each={repos().slice(0, 4)}
                      fallback={
                        <div class="home-empty">
                          No repositories yet.{" "}
                          <button onClick={chooseRepo}>
                            Open one <ArrowRight size={14} />
                          </button>
                        </div>
                      }
                    >
                      {(repo) => (
                        <button
                          class="home-list-row"
                          onClick={() => {
                            setActiveRepo(repo.path);
                            setPage("git");
                          }}
                        >
                          <span class="row-icon">
                            <GitBranch size={17} />
                          </span>
                          <span>
                            <strong>{repo.name}</strong>
                            <small>{repo.path}</small>
                          </span>
                          <ChevronRight size={16} />
                        </button>
                      )}
                    </For>
                  </section>
                  <section class="home-card">
                    <div class="card-head">
                      <div class="card-icon peach-bg">
                        <Server size={20} />
                      </div>
                      <div>
                        <h2>Saved hosts</h2>
                        <p>Your connections, ready to go</p>
                      </div>
                      <button
                        class="mini-button"
                        title="Add host"
                        onClick={() => openProfileEditor(blankProfile())}
                      >
                        <Plus size={18} />
                      </button>
                    </div>
                    <For
                      each={profiles().slice(0, 4)}
                      fallback={
                        <div class="home-empty">
                          No hosts yet.{" "}
                          <button
                            onClick={() => openProfileEditor(blankProfile())}
                          >
                            Add one <ArrowRight size={14} />
                          </button>
                        </div>
                      }
                    >
                      {(profile) => (
                        <div
                          class="home-list-row"
                          onContextMenu={(event) => hostMenu(event, profile)}
                        >
                          <span
                            class="row-icon"
                            style={{ color: profile.color }}
                          >
                            <Server size={17} />
                          </span>
                          <span>
                            <strong>{profile.name}</strong>
                            <small>{profileAddress(profile)}</small>
                          </span>
                          <button
                            class="mini-button"
                            title="Edit host"
                            onClick={() => openProfileEditor({ ...profile })}
                          >
                            <Settings2 size={15} />
                          </button>
                          <button
                            class="mini-button"
                            title="Connect"
                            onClick={() => openConnection(profile)}
                          >
                            <ArrowRight size={17} />
                          </button>
                        </div>
                      )}
                    </For>
                  </section>
                </div>
                <div class="quick-strip">
                  <div>
                    <span class="eyebrow">QUICK START</span>
                    <strong>Start locally or connect to a remote host.</strong>
                  </div>
                  <button class="ghost-button" onClick={() => openTerminal()}>
                    <TerminalSquare size={16} /> New terminal
                  </button>
                  <button class="ghost-button" onClick={() => setPage("files")}>
                    <HardDrive size={16} /> Browse files
                  </button>
                  <button
                    class="ghost-button"
                    onClick={() => openProfileEditor(blankProfile())}
                  >
                    <Network size={16} /> Connect NAS
                  </button>
                </div>
              </div>
            </Show>
            <Show when={page() === "git"}>
              <Show
                when={activeRepo()}
                fallback={
                  <div class="module-empty">
                    <div class="module-empty-icon">
                      <GitBranch size={28} />
                    </div>
                    <h1>Bring your code in</h1>
                    <p>
                      Open a local Git repository to see changes, branches, and
                      history.
                    </p>
                    <button class="primary-button" onClick={chooseRepo}>
                      Open repository <ArrowRight size={16} />
                    </button>
                    <div class="inline-actions">
                      <button
                        class="text-button"
                        onClick={() => setCloneOpen(true)}
                      >
                        Clone from URL
                      </button>
                      <button class="text-button" onClick={initRepo}>
                        Initialize folder
                      </button>
                      <button
                        class="text-button"
                        onClick={() => setServerReposOpen(true)}
                      >
                        Server repositories
                      </button>
                    </div>
                  </div>
                }
              >
                {(path) => (
                  <GitView
                    path={path()}
                    onError={(message) => notify(message, true)}
                    onNotice={(message) => notify(message)}
                    onOpenTerminal={(cwd, command) =>
                      openTerminal(undefined, cwd, command)
                    }
                    onOpenRepository={(path) => void openAsRepo(path)}
                  />
                )}
              </Show>
            </Show>
            <Show when={page() === "files"}>
              <FilesView
                home={home()}
                locations={fileLocations()}
                onLocationChange={(side, location) =>
                  setFileLocations((current) => ({
                    left:
                      side === "left"
                        ? location
                        : (current?.left ?? { path: home(), profileId: null }),
                    right:
                      side === "right"
                        ? location
                        : (current?.right ?? { path: home(), profileId: null }),
                  }))
                }
                profiles={profiles()}
                onError={(message) => notify(message, true)}
                onNotice={(message) => notify(message)}
                onOpenTerminal={(cwd) => openTerminal(undefined, cwd)}
                onConnectHost={(profile, cwd) => openTerminal(profile, cwd)}
                onOpenAsRepo={openAsRepo}
                onTransferQueued={(record) => {
                  setTransfers((current) =>
                    sortTransfers([record, ...current]),
                  );
                  setActivityOpen(true);
                }}
              />
            </Show>
            <div
              class={`terminal-layout ${splitSession() && splitSession() !== activeSession() ? "is-split" : ""} ${splitDirection()}`}
              style={{ display: page() === "terminal" ? "flex" : "none" }}
            >
              <For each={sessions()}>
                {(session) => {
                  const sessionActions = () => (
                    <>
                      <button
                        class={
                          broadcastInput()
                            ? "ghost-button broadcast-active"
                            : "ghost-button"
                        }
                        disabled={broadcastTerminalCount() < 2}
                        title="Send typed input to included connected terminal tabs"
                        onClick={async () => {
                          if (broadcastInput()) {
                            setBroadcastInput(false);
                          } else if (
                            await confirmAction(
                              "Broadcast all typed input, including passwords, to every included connected terminal tab?",
                            )
                          ) {
                            setBroadcastInput(true);
                          }
                        }}
                      >
                        {broadcastInput() ? "Broadcast on" : "Broadcast input"}
                      </button>
                      <button
                        class={
                          broadcastExcluded().has(session.key)
                            ? "ghost-button broadcast-active"
                            : "ghost-button"
                        }
                        title="Keep this tab out of broadcast input"
                        onClick={() =>
                          setBroadcastExcluded((current) => {
                            const next = new Set(current);
                            if (next.has(session.key)) next.delete(session.key);
                            else next.add(session.key);
                            return next;
                          })
                        }
                      >
                        {broadcastExcluded().has(session.key)
                          ? "Include tab"
                          : "Exclude tab"}
                      </button>
                      <button
                        class="ghost-button"
                        onClick={() => splitTerminal("row")}
                      >
                        Split right
                      </button>
                      <button
                        class="ghost-button"
                        onClick={() => splitTerminal("column")}
                      >
                        Split below
                      </button>
                      <button
                        class="ghost-button"
                        onClick={() => {
                          setRequestedSnippetId(undefined);
                          setSnippetsOpen((value) => !value);
                        }}
                      >
                        Snippets
                      </button>
                    </>
                  );
                  return (
                    <div
                      class="terminal-page"
                      style={{
                        display:
                          page() === "terminal" &&
                          (activeSession() === session.key ||
                            (splitSession() === session.key &&
                              splitSession() !== activeSession()))
                            ? "flex"
                            : "none",
                      }}
                    >
                      <div class="page-heading terminal-heading">
                        <div>
                          <span class="eyebrow">
                            {session.connection
                              ? `${connectionKindLabels[session.connection.kind].toUpperCase()} CONNECTION`
                              : session.profileId
                                ? "SSH CONNECTION"
                                : "LOCAL TERMINAL"}
                          </span>
                          <h1>{session.label}</h1>
                          <p class="muted">
                            {session.connection
                              ? describeConnection(
                                  session.connection,
                                  profiles().find(
                                    (profile) =>
                                      profile.id === session.profileId,
                                  )?.host,
                                )
                              : session.profileId
                                ? profiles().find(
                                    (profile) =>
                                      profile.id === session.profileId,
                                  )?.host
                                : (session.cwd ?? "Your local shell")}
                          </p>
                        </div>
                        <div class="terminal-actions">
                          <Show when={activeSession() === session.key}>
                            <Show
                              when={
                                splitSession() &&
                                splitSession() !== activeSession()
                              }
                              fallback={sessionActions()}
                            >
                              <MenuButton label="Actions">
                                {sessionActions()}
                              </MenuButton>
                            </Show>
                          </Show>
                          <Show when={splitSession() === session.key}>
                            <button
                              class="ghost-button"
                              onClick={() => setSplitSession(undefined)}
                            >
                              Close split
                            </button>
                          </Show>
                          <Show when={endedSessions().has(session.key)}>
                            <button
                              class="ghost-button"
                              onClick={() => reconnectTerminal(session.key)}
                            >
                              Reconnect
                            </button>
                          </Show>
                        </div>
                        <span class="connection-badge">
                          <Show
                            when={
                              !endedSessions().has(session.key) &&
                              readySessions().has(session.key)
                            }
                          >
                            <span class="status-pulse" />
                          </Show>
                          {endedSessions().has(session.key)
                            ? "Session ended"
                            : readySessions().has(session.key)
                              ? "Session active"
                              : "Connecting"}
                        </span>
                      </div>
                      <div
                        class="terminal-frame"
                        onPointerDown={() => {
                          if (activeSession() === session.key) return;
                          setSplitSession(activeSession());
                          setActiveSession(session.key);
                        }}
                      >
                        <Show
                          when={
                            pinnedSnippets(savedSnippets(), session.profileId)
                              .length
                          }
                        >
                          <div
                            class="terminal-macro-bar"
                            aria-label="Pinned terminal commands"
                          >
                            <For
                              each={pinnedSnippets(
                                savedSnippets(),
                                session.profileId,
                              )}
                            >
                              {(item) => (
                                <button
                                  type="button"
                                  title={`Insert ${item.name} in this terminal`}
                                  onClick={() =>
                                    usePinnedSnippet(item, session.key)
                                  }
                                >
                                  {item.name}
                                </button>
                              )}
                            </For>
                          </div>
                        </Show>
                        <TerminalView
                          onSftpOnly={(always) => {
                            const profile = profiles().find(
                              (item) => item.id === session.profileId,
                            );
                            if (!profile) return;
                            if (always) {
                              setSftpOnly(profile.id, true);
                              notify(
                                `${profile.name} will open in Files from now on. Untick “File transfer only” in its editor to change this.`,
                              );
                            }
                            setFileLocations((current) => ({
                              left: { path: ".", profileId: profile.id },
                              right: current?.right ?? {
                                path: home(),
                                profileId: null,
                              },
                            }));
                            setPage("files");
                          }}
                          searchKey={session.key}
                          label={session.label}
                          profileId={session.profileId}
                          cwd={session.cwd}
                          connection={session.connection}
                          startupCommand={session.startupCommand}
                          onOpenFiles={(path) => {
                            setFileLocations((current) => ({
                              left: {
                                path,
                                profileId: session.profileId ?? null,
                              },
                              right: current?.right ?? {
                                path: home(),
                                profileId: null,
                              },
                            }));
                            setPage("files");
                          }}
                          onInput={(data) => {
                            if (!broadcastInput()) return;
                            for (const id of broadcastTargets(
                              session.key,
                              terminalIds,
                              endedSessions(),
                              broadcastExcluded(),
                            )) {
                              void api
                                .termWrite(id, data)
                                .catch((error) =>
                                  notify(
                                    `Could not broadcast input: ${errorMessage(error)}`,
                                    true,
                                  ),
                                );
                            }
                          }}
                          onActivity={() =>
                            setActivitySessions((current) =>
                              markTerminalActivity(
                                current,
                                session.key,
                                page() === "terminal" &&
                                  (activeSession() === session.key ||
                                    splitSession() === session.key),
                              ),
                            )
                          }
                          onRecordingState={(unsaved) =>
                            setRecordingSessions((current) => {
                              const next = new Set(current);
                              if (unsaved) next.add(session.key);
                              else next.delete(session.key);
                              return next;
                            })
                          }
                          onError={(message) => notify(message, true)}
                          onReady={(id) => {
                            terminalIds.set(session.key, id);
                            setReadySessions((current) =>
                              new Set(current).add(session.key),
                            );
                          }}
                          onExit={() =>
                            setEndedSessions((current) =>
                              new Set(current).add(session.key),
                            )
                          }
                          onClosed={() => terminalIds.delete(session.key)}
                        />
                      </div>
                      <Show
                        when={snippetsOpen() && activeSession() === session.key}
                      >
                        <SnippetsPanel
                          profileId={session.profileId}
                          onInsert={(text) => void insertSnippet(text)}
                          onClose={() => {
                            setSnippetsOpen(false);
                            setRequestedSnippetId(undefined);
                          }}
                          onError={(message) => notify(message, true)}
                          onChange={setSavedSnippets}
                          initialSnippetId={requestedSnippetId()}
                          onRequestHandled={() =>
                            setRequestedSnippetId(undefined)
                          }
                        />
                      </Show>
                    </div>
                  );
                }}
              </For>
            </div>
          </main>
          <footer class="activity-bar">
            <button onClick={() => setActivityOpen((value) => !value)}>
              <Activity size={15} />
              <span>Activity</span>
              <span class="muted">
                {
                  transfers().filter(
                    (item) =>
                      item.status === "running" || item.status === "queued",
                  ).length
                }{" "}
                active transfer(s)
              </span>
            </button>
            <span>Weave · local-first workspace</span>
          </footer>
        </div>

        <Show when={activityOpen()}>
          <aside class="activity-drawer" aria-label="Transfer activity">
            <div class="activity-head">
              <div>
                <span class="eyebrow">ACTIVITY</span>
                <h2>Transfers</h2>
              </div>
              <button
                class="icon-button"
                title="Close activity"
                onClick={() => setActivityOpen(false)}
              >
                <X size={18} />
              </button>
            </div>
            <form class="transfer-settings" onSubmit={saveTransferOptions}>
              <label>
                Parallel transfers
                <select
                  value={transferSettings().parallelism}
                  onChange={(event) =>
                    setTransferSettings({
                      ...transferSettings(),
                      parallelism: Number(event.currentTarget.value),
                    })
                  }
                >
                  <For each={[1, 2, 3, 4, 5, 6, 7, 8]}>
                    {(count) => <option value={count}>{count}</option>}
                  </For>
                </select>
              </label>
              <label>
                Speed limit (KB/s)
                <input
                  type="number"
                  min="0"
                  max="1000000"
                  value={transferSettings().speedLimitKbps}
                  onInput={(event) =>
                    setTransferSettings({
                      ...transferSettings(),
                      speedLimitKbps: Number(event.currentTarget.value),
                    })
                  }
                />
                <small>0 means unlimited</small>
              </label>
              <button class="ghost-button" type="submit">
                Save
              </button>
            </form>
            <div class="activity-list">
              <For
                each={transfers()}
                fallback={<div class="empty-small">No transfers yet</div>}
              >
                {(record) => (
                  <div class="transfer-card">
                    <div class="transfer-title">
                      <strong>{record.sourcePath.split("/").pop()}</strong>
                      <Show when={record.method}>
                        <span class="transfer-method">
                          {record.method === "scp" ? "SCP" : record.method}
                        </span>
                      </Show>
                      <span class={`transfer-status ${record.status}`}>
                        {record.status}
                      </span>
                    </div>
                    <div class="transfer-route">
                      {record.sourceProfile
                        ? (profiles().find(
                            (profile) => profile.id === record.sourceProfile,
                          )?.name ?? "SFTP")
                        : "This computer"}{" "}
                      →{" "}
                      {record.destinationProfile
                        ? (profiles().find(
                            (profile) =>
                              profile.id === record.destinationProfile,
                          )?.name ?? "SFTP")
                        : "This computer"}
                    </div>
                    <div class="progress-track">
                      <div
                        style={{
                          width: `${record.totalBytes ? Math.min(100, (record.bytesDone / record.totalBytes) * 100) : record.status === "done" ? 100 : 0}%`,
                        }}
                      />
                    </div>
                    <div class="transfer-bottom">
                      <span>
                        {formatBytes(record.bytesDone)} /{" "}
                        {formatBytes(record.totalBytes)}
                        <Show
                          when={remainingTime(
                            record,
                            transferRates()[record.id],
                          )}
                        >
                          {(remaining) => <span> · {remaining()}</span>}
                        </Show>
                      </span>
                      <div>
                        <Show when={record.status === "queued"}>
                          <button
                            class="mini-button"
                            title="Move earlier"
                            onClick={() => void moveTransfer(record.id, "up")}
                          >
                            <ArrowUp size={15} />
                          </button>
                          <button
                            class="mini-button"
                            title="Move later"
                            onClick={() => void moveTransfer(record.id, "down")}
                          >
                            <ArrowDown size={15} />
                          </button>
                        </Show>
                        <Show
                          when={
                            record.status === "running" ||
                            record.status === "queued"
                          }
                        >
                          <button
                            class="mini-button"
                            title="Pause"
                            onClick={() => transferAction("pause", record.id)}
                          >
                            <Pause size={15} />
                          </button>
                        </Show>
                        <Show
                          when={
                            record.status === "paused" ||
                            record.status === "failed"
                          }
                        >
                          <button
                            class="mini-button"
                            title="Resume"
                            onClick={() => transferAction("resume", record.id)}
                          >
                            <Play size={15} />
                          </button>
                        </Show>
                        <Show
                          when={
                            record.status !== "running" &&
                            record.status !== "queued"
                          }
                        >
                          <Show when={record.status === "done"}>
                            <button
                              class="mini-button"
                              title="Run this transfer again"
                              onClick={() => void redoTransfer(record)}
                            >
                              <RotateCcw size={15} />
                            </button>
                          </Show>
                          <button
                            class="mini-button"
                            title="Remove from history"
                            onClick={() => transferAction("remove", record.id)}
                          >
                            <Trash2 size={15} />
                          </button>
                        </Show>
                      </div>
                    </div>
                    <Show when={record.error}>
                      <p class="transfer-error">{record.error}</p>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </aside>
        </Show>

        <Show when={vaultOpen()}>
          <SettingsVaultDialog
            profiles={profiles()}
            onImported={() => {
              setHostMeta(loadHostMeta(localStorage));
              void api.profiles().then(setProfiles);
              void api.workspaces().then(setWorkspaces);
            }}
            onNotify={(message, error) => notify(message, error)}
            onClose={() => setVaultOpen(false)}
          />
        </Show>
        <Show when={serverReposOpen()}>
          <ServerReposDialog
            profiles={profiles()}
            onClone={(url) => {
              setCloneUrl(url);
              setCloneOpen(true);
            }}
            onNotify={(message, error) => notify(message, error)}
            onClose={() => setServerReposOpen(false)}
          />
        </Show>
        <Show when={connectOpen()}>
          <ConnectDialog
            profiles={profiles()}
            onOpenTerminal={openRemoteTerminal}
            onOpenVnc={setVncSession}
            onNotify={(message, error) => notify(message, error)}
            onClose={() => setConnectOpen(false)}
          />
        </Show>
        <Show when={shortcutsOpen()}>
          <KeyboardShortcutsDialog
            bindings={keybindings()}
            onChange={(next) => {
              setKeybindings(next);
              try {
                saveKeybindings(localStorage, next);
              } catch {
                notify(
                  `Could not save the shortcuts on ${thisComputer}.`,
                  true,
                );
              }
            }}
            onClose={() => setShortcutsOpen(false)}
          />
        </Show>
        <Show when={vncSession()}>
          {(session) => (
            <VncViewer
              name={session().name}
              host={session().host ?? ""}
              port={session().port ?? 5900}
              username={session().username}
              profileId={session().profileId}
              onClose={() => setVncSession(null)}
            />
          )}
        </Show>
        <Show when={workspaceEditor()}>
          {(editor) => (
            <div class="overlay" onClick={() => setWorkspaceEditor(undefined)}>
              <div
                class="dialog profile-dialog"
                role="dialog"
                aria-modal="true"
                aria-label="Save workspace"
                onClick={(event) => event.stopPropagation()}
              >
                <div class="dialog-head">
                  <div>
                    <span class="eyebrow">WORKSPACE</span>
                    <h2>
                      {editor().id ? "Update workspace" : "Save workspace"}
                    </h2>
                  </div>
                  <button
                    class="icon-button"
                    aria-label="Close"
                    onClick={() => setWorkspaceEditor(undefined)}
                  >
                    <X size={19} />
                  </button>
                </div>
                <p class="muted">
                  Save the current page, repository, and terminal tabs. Opening
                  a workspace reconnects its terminal sessions.
                </p>
                <form onSubmit={saveWorkspace}>
                  <div class="form-grid">
                    <label class="wide">
                      Workspace name
                      <input
                        autofocus
                        required
                        maxLength={80}
                        value={editor().name}
                        onInput={(event) =>
                          setWorkspaceEditor({
                            ...editor(),
                            name: event.currentTarget.value,
                          })
                        }
                        placeholder="Thesis"
                      />
                    </label>
                  </div>
                  <div class="dialog-actions">
                    <Show when={editor().id}>
                      <button
                        type="button"
                        class="ghost-button danger-text"
                        onClick={() => {
                          const item = workspaces().find(
                            (workspace) => workspace.id === editor().id,
                          );
                          if (item) {
                            setWorkspaceEditor(undefined);
                            setDeletingWorkspace(item);
                          }
                        }}
                      >
                        Delete
                      </button>
                    </Show>
                    <span class="spacer" />
                    <button
                      type="button"
                      class="ghost-button"
                      onClick={() => setWorkspaceEditor(undefined)}
                    >
                      Cancel
                    </button>
                    <button type="submit" class="primary-button">
                      {editor().id ? "Update layout" : "Save workspace"}
                    </button>
                  </div>
                </form>
              </div>
            </div>
          )}
        </Show>
        <Show when={deletingWorkspace()}>
          {(workspace) => (
            <div
              class="overlay"
              onClick={() => setDeletingWorkspace(undefined)}
            >
              <div
                class="dialog profile-dialog"
                role="dialog"
                aria-modal="true"
                aria-label="Delete workspace"
                onClick={(event) => event.stopPropagation()}
              >
                <div class="dialog-head">
                  <div>
                    <span class="eyebrow">WORKSPACE</span>
                    <h2>Delete “{workspace().name}”?</h2>
                  </div>
                  <button
                    class="icon-button"
                    aria-label="Close"
                    onClick={() => setDeletingWorkspace(undefined)}
                  >
                    <X size={19} />
                  </button>
                </div>
                <p class="muted">
                  This removes the saved layout. Repositories, host profiles,
                  and files remain in place.
                </p>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    class="ghost-button"
                    onClick={() => setDeletingWorkspace(undefined)}
                  >
                    Cancel
                  </button>
                  <button
                    class="ghost-button danger-text"
                    onClick={() => void deleteWorkspace()}
                  >
                    Delete workspace
                  </button>
                </div>
              </div>
            </div>
          )}
        </Show>
        <Show when={profileEditor()}>
          {(profile) => (
            <div class="overlay" onClick={() => closeProfileEditor()}>
              <div
                class="dialog profile-dialog host-dialog"
                role="dialog"
                aria-modal="true"
                aria-label="Host profile"
                onClick={(event) => event.stopPropagation()}
              >
                <div class="dialog-head">
                  <div>
                    <span class="eyebrow">CONNECTION PROFILE</span>
                    <h2>
                      {profile().id ? "Edit connection" : "Add a connection"}
                    </h2>
                  </div>
                  <button
                    class="icon-button"
                    aria-label="Close"
                    onClick={() => closeProfileEditor()}
                  >
                    <X size={19} />
                  </button>
                </div>
                <p class="muted">
                  {profile().protocol === "ssh"
                    ? "SSH uses your system agent or prompts in Terminal."
                    : "Remote passwords and access keys can be kept for this session or saved in the system keychain."}
                </p>
                <form
                  onSubmit={(event) => {
                    event.preventDefault();
                    void saveProfile(profile());
                  }}
                >
                  <div class="form-grid">
                    <label>
                      Display name
                      <input
                        required
                        value={profile().name}
                        onInput={(event) =>
                          setProfileEditor({
                            ...profile(),
                            name: event.currentTarget.value,
                          })
                        }
                        placeholder="Production server"
                      />
                    </label>
                    <label>
                      Protocol
                      <select
                        value={profile().protocol}
                        onChange={(event) => {
                          const protocol = event.currentTarget
                            .value as Profile["protocol"];
                          setProfileEditor({
                            ...profile(),
                            protocol,
                            tlsPinSha256:
                              protocol === "ftps" ||
                              protocol === "ftps-implicit" ||
                              protocol === "webdav"
                                ? profile().tlsPinSha256
                                : null,
                            s3Region:
                              protocol === "s3"
                                ? profile().s3Region || "us-east-1"
                                : null,
                            port:
                              protocol === "webdav" ||
                              protocol === "s3" ||
                              protocol === "azure" ||
                              isCloud(protocol)
                                ? 443
                                : protocol === "ssh"
                                  ? 22
                                  : protocol === "ftps-implicit"
                                    ? 990
                                    : 21,
                          });
                          setProfilePassword("");
                          setS3TokenInput("");
                          setClearS3Token(false);
                        }}
                      >
                        <option value="ssh">SSH / SFTP</option>
                        <option value="ftps">FTPS (explicit TLS)</option>
                        <option value="ftps-implicit">
                          FTPS (implicit TLS)
                        </option>
                        <option value="ftp">FTP (unencrypted)</option>
                        <option value="webdav">WebDAV (HTTPS)</option>
                        <option value="s3">S3-compatible storage</option>
                        <option value="azure">Azure Blob Storage</option>
                        <option value="dropbox">Dropbox</option>
                        <option value="gdrive">Google Drive</option>
                        <option value="onedrive">OneDrive</option>
                      </select>
                    </label>
                    <Show when={isCloud(profile().protocol)}>
                      <CloudDrivePanel
                        protocol={profile().protocol as CloudProtocol}
                        profileId={profile().id}
                        account={profile().username}
                        onSignedIn={(email) => {
                          setProfileEditor({ ...profile(), username: email });
                          void api
                            .profiles()
                            .then(setProfiles)
                            .catch(() => undefined);
                        }}
                        onNotice={(message) => notify(message)}
                      />
                    </Show>
                    <Show when={profile().protocol === "s3"}>
                      <div class="cloud-preset-fields">
                        <label>
                          Cloud provider
                          <select
                            value={cloudPreset()}
                            onChange={(event) => {
                              const preset = event.currentTarget
                                .value as CloudPreset;
                              setCloudPreset(preset);
                              setCloudPresetError("");
                              if (preset !== "custom")
                                setProfileEditor({
                                  ...profile(),
                                  s3Region: presetRegion(preset),
                                });
                            }}
                          >
                            <option value="custom">Custom S3 or MinIO</option>
                            <option value="aws">Amazon S3</option>
                            <option value="r2">Cloudflare R2</option>
                            <option value="b2">Backblaze B2</option>
                            <option value="wasabi">Wasabi</option>
                            <option value="spaces">DigitalOcean Spaces</option>
                          </select>
                        </label>
                        <Show when={cloudPreset() !== "custom"}>
                          <label>
                            Bucket name
                            <input
                              value={cloudBucket()}
                              onInput={(event) =>
                                setCloudBucket(event.currentTarget.value)
                              }
                              placeholder="my-bucket"
                            />
                          </label>
                          <Show when={cloudPreset() === "r2"}>
                            <label>
                              Cloudflare account ID
                              <input
                                value={cloudAccount()}
                                onInput={(event) =>
                                  setCloudAccount(event.currentTarget.value)
                                }
                                placeholder="32-character account ID"
                              />
                            </label>
                          </Show>
                          <button
                            type="button"
                            class="ghost-button"
                            onClick={() => {
                              try {
                                const host = cloudBucketUrl(
                                  cloudPreset(),
                                  profile().s3Region ?? "",
                                  cloudBucket(),
                                  cloudAccount(),
                                );
                                setProfileEditor({ ...profile(), host });
                                setCloudPresetError("");
                              } catch (error) {
                                setCloudPresetError(errorMessage(error));
                              }
                            }}
                          >
                            Use provider endpoint
                          </button>
                        </Show>
                        <Show when={cloudPresetError()}>
                          <p class="error-banner" role="alert">
                            {cloudPresetError()}
                          </p>
                        </Show>
                      </div>
                    </Show>
                    <Show when={!isCloud(profile().protocol)}>
                      <label>
                        {profile().protocol === "webdav"
                          ? "WebDAV URL"
                          : profile().protocol === "s3"
                            ? "Bucket URL"
                            : profile().protocol === "azure"
                              ? "Container URL"
                              : "Host"}
                        <input
                          required
                          autocapitalize="off"
                          autocorrect="off"
                          spellcheck={false}
                          value={profile().host}
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              host: event.currentTarget.value,
                            })
                          }
                          placeholder={
                            profile().protocol === "webdav"
                              ? "https://server.example.com/dav/"
                              : profile().protocol === "s3"
                                ? "https://s3.eu-central-1.amazonaws.com/my-bucket"
                                : profile().protocol === "azure"
                                  ? "https://myaccount.blob.core.windows.net/my-container"
                                  : "server.example.com"
                          }
                        />
                        <Show when={profile().protocol === "s3"}>
                          <span class="field-hint">
                            For an S3 Express directory bucket, use its zonal
                            address, such as
                            https://name--use1-az4--x-s3.s3express-use1-az4.us-east-1.amazonaws.com
                          </span>
                        </Show>
                      </label>
                      <label>
                        {profile().protocol === "s3"
                          ? "Access key ID"
                          : profile().protocol === "azure"
                            ? "Storage account"
                            : "Username"}
                        <input
                          autocapitalize="off"
                          autocorrect="off"
                          spellcheck={false}
                          required={
                            !(profile().protocol === "s3" && awsProfile()) &&
                            profile().protocol !== "azure"
                          }
                          value={
                            profile().username === "aws-profile"
                              ? ""
                              : profile().username
                          }
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              username: event.currentTarget.value,
                            })
                          }
                          placeholder={
                            profile().protocol === "s3"
                              ? "AKIA…"
                              : profile().protocol === "azure"
                                ? "Taken from the URL when left empty"
                                : "alice"
                          }
                        />
                        <Show
                          when={
                            profile().protocol === "ssh" &&
                            /[A-Z]/.test(profile().username)
                          }
                        >
                          <span class="field-hint">
                            User names are case-sensitive, and most servers use
                            lowercase. Check that this matches exactly.
                          </span>
                        </Show>
                      </label>
                    </Show>
                    <Show when={profile().protocol === "ssh" && profile().id}>
                      <label class="profile-favorite wide">
                        <input
                          type="checkbox"
                          checked={sftpOnlyHosts().has(profile().id)}
                          onChange={(event) =>
                            setSftpOnly(
                              profile().id,
                              event.currentTarget.checked,
                            )
                          }
                        />
                        <span>
                          File transfer only (SFTP)
                          <small class="muted">
                            For servers that refuse terminal sessions. Opening
                            this host goes to Files.
                          </small>
                        </span>
                      </label>
                    </Show>
                    <Show
                      when={
                        profile().protocol !== "webdav" &&
                        profile().protocol !== "s3" &&
                        profile().protocol !== "azure" &&
                        !isCloud(profile().protocol)
                      }
                    >
                      <label>
                        Port
                        <input
                          required
                          type="number"
                          min="1"
                          max="65535"
                          value={profile().port}
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              port: Number(event.currentTarget.value),
                            })
                          }
                        />
                      </label>
                    </Show>
                    <Show when={profile().protocol === "s3"}>
                      <label class="wide">
                        Credentials
                        <select
                          value={awsProfile()}
                          onChange={(event) =>
                            setAwsProfile(event.currentTarget.value)
                          }
                        >
                          <option value="">
                            Access key and secret entered here
                          </option>
                          <For each={awsProfiles()}>
                            {(name) => (
                              <option value={name}>
                                AWS CLI profile: {name}
                              </option>
                            )}
                          </For>
                        </select>
                        <Show when={awsProfile()}>
                          <span class="field-hint">
                            Keys are read from ~/.aws when connecting and
                            refreshed before temporary keys expire. The access
                            key and secret fields can stay empty.
                          </span>
                        </Show>
                      </label>
                      <label>
                        Signing region
                        <input
                          required
                          value={profile().s3Region ?? ""}
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              s3Region: event.currentTarget.value,
                            })
                          }
                          placeholder="eu-central-1"
                        />
                      </label>
                      <Show when={!awsProfile()}>
                        <label class="wide">
                          Session token{" "}
                          <span class="optional">
                            optional; leave blank to keep current
                          </span>
                          <input
                            type="password"
                            value={s3TokenInput()}
                            onInput={(event) => {
                              setS3TokenInput(event.currentTarget.value);
                              setClearS3Token(false);
                            }}
                            placeholder="Temporary AWS or S3-compatible security token"
                            autocomplete="new-password"
                          />
                        </label>
                        <Show when={profile().id}>
                          <label class="profile-favorite wide">
                            <input
                              type="checkbox"
                              checked={clearS3Token()}
                              onChange={(event) => {
                                setClearS3Token(event.currentTarget.checked);
                                if (event.currentTarget.checked)
                                  setS3TokenInput("");
                              }}
                            />
                            Remove the current session token when saving
                          </label>
                        </Show>
                      </Show>
                    </Show>
                    <Show when={profile().protocol === "ssh"}>
                      <label class="wide">
                        Private key path <span class="optional">optional</span>
                        <input
                          value={profile().keyPath ?? ""}
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              keyPath: event.currentTarget.value || null,
                            })
                          }
                          placeholder="~/.ssh/id_ed25519"
                        />
                      </label>
                      <label class="wide">
                        Jump host <span class="optional">optional</span>
                        <input
                          value={profile().jumpHost ?? ""}
                          onInput={(event) =>
                            setProfileEditor({
                              ...profile(),
                              jumpHost: event.currentTarget.value || null,
                            })
                          }
                          placeholder="user@bastion.example.com"
                        />
                      </label>
                      <Show
                        when={profile().id}
                        fallback={
                          <p class="muted wide">
                            Save the host to set keepalive, agent forwarding, a
                            proxy command, port knocking and more.
                          </p>
                        }
                      >
                        <HostOptionsPanel
                          profileId={profile().id}
                          onNotice={(message) => notify(message)}
                        />
                      </Show>
                      <NetworkPanel
                        host={profile().host}
                        port={profile().port}
                        onSelect={(peer) =>
                          setProfileEditor({
                            ...profile(),
                            name: profile().name || peer.name,
                            host: peer.ip,
                          })
                        }
                        onNotice={(message) => notify(message)}
                        onError={(message) => notify(message, true)}
                      />
                      <NetworkToolsPanel host={profile().host} />
                      <KnownHostsPanel
                        host={profile().host}
                        onNotice={(message) => notify(message)}
                      />
                      <SSHKeysPanel
                        profileId={profile().id}
                        onChoose={(privatePath) =>
                          setProfileEditor({
                            ...profile(),
                            keyPath: privatePath,
                          })
                        }
                        onInstall={(command) => {
                          setProfileEditor(undefined);
                          openTerminal(undefined, undefined, command);
                        }}
                        onNotice={(message) => notify(message)}
                      />
                      <Show when={profile().id}>
                        <TunnelsPanel
                          profileId={profile().id}
                          onNotice={(message) => notify(message)}
                        />
                      </Show>
                    </Show>
                    <Show
                      when={
                        !isCloud(profile().protocol) &&
                        !(profile().protocol === "s3" && awsProfile())
                      }
                    >
                      <label class="wide">
                        {profile().protocol === "s3"
                          ? "Secret access key"
                          : profile().protocol === "azure"
                            ? "Account key or SAS token"
                            : "Password"}{" "}
                        {profile().protocol === "ssh" ? (
                          <span class="optional">
                            optional, for hosts that ask for one
                          </span>
                        ) : profile().id ? (
                          <span class="optional">
                            leave blank to keep current
                          </span>
                        ) : ["ftp", "ftps", "ftps-implicit"].includes(
                            profile().protocol,
                          ) &&
                          profile().username.trim().toLowerCase() ===
                            "anonymous" ? (
                          <span class="optional">
                            optional for anonymous FTP
                          </span>
                        ) : null}
                        <input
                          type="password"
                          value={profilePassword()}
                          onInput={(event) =>
                            setProfilePassword(event.currentTarget.value)
                          }
                          autocomplete="new-password"
                        />
                      </label>
                      <label class="profile-favorite wide">
                        <input
                          type="checkbox"
                          checked={rememberPassword()}
                          onChange={(event) =>
                            setRememberPassword(event.currentTarget.checked)
                          }
                        />
                        Remember{" "}
                        {profile().protocol === "s3"
                          ? "secret key and session token"
                          : "password"}{" "}
                        in {keychain}
                      </label>
                      <Show when={profile().protocol === "ftp"}>
                        <p class="muted wide">
                          FTP sends your password and files without encryption.
                          Use FTPS when the server supports it.
                        </p>
                      </Show>
                    </Show>
                    <Show
                      when={
                        profile().protocol === "ftps" ||
                        profile().protocol === "ftps-implicit" ||
                        profile().protocol === "webdav"
                      }
                    >
                      <label class="wide">
                        Certificate SHA-256 fingerprint{" "}
                        <span class="optional">optional</span>
                        <span class="inline-field">
                          <input
                            value={profile().tlsPinSha256 ?? ""}
                            onInput={(event) =>
                              setProfileEditor({
                                ...profile(),
                                tlsPinSha256: event.currentTarget.value || null,
                              })
                            }
                            placeholder="64 hex characters from a trusted source"
                            spellcheck={false}
                          />
                          <button
                            type="button"
                            class="server-action"
                            disabled={serverCertLoading() || !profile().host}
                            onClick={() => void readServerCertificate()}
                          >
                            {serverCertLoading()
                              ? "Reading…"
                              : "Read from server"}
                          </button>
                        </span>
                        <Show when={serverCert()}>
                          {(certificate) => (
                            <span class="server-cert">
                              <code>{certificate().sha256}</code>
                              <span class="muted">
                                {certificate().matchesProfile
                                  ? "Already trusted by this profile."
                                  : certificate().trustedByMac
                                    ? `Trusted by ${systemName}. Compare it with the fingerprint your administrator gave you before adding it.`
                                    : `Not trusted by ${systemName}. Only add it if it matches a fingerprint you got from your administrator.`}
                              </span>
                              <Show when={!certificate().matchesProfile}>
                                <span class="inline-field">
                                  <button
                                    type="button"
                                    class="server-action"
                                    onClick={() => useServerFingerprint(false)}
                                  >
                                    Replace saved fingerprint
                                  </button>
                                  <Show when={profile().tlsPinSha256}>
                                    <button
                                      type="button"
                                      class="server-action"
                                      onClick={() => useServerFingerprint(true)}
                                    >
                                      Trust it as well
                                    </button>
                                  </Show>
                                </span>
                              </Show>
                            </span>
                          )}
                        </Show>
                        <small class="muted">
                          Weave still checks {systemName} certificate trust. A
                          fingerprint also requires the exact server certificate
                          to match. When a certificate is renewed, trust the new
                          fingerprint as well, then remove the old one after the
                          switch (up to three, separated by commas).
                        </small>
                      </label>
                    </Show>
                    <Show
                      when={["webdav", "ftps", "ftps-implicit"].includes(
                        profile().protocol,
                      )}
                    >
                      <label class="wide">
                        Client certificate{" "}
                        <span class="optional">optional</span>
                        <span class="inline-field">
                          <input
                            readOnly
                            value={clientCert()}
                            placeholder="PEM file with certificate and private key"
                          />
                          <button
                            type="button"
                            class="server-action"
                            onClick={async () => {
                              const file = await open({
                                multiple: false,
                                filters: [
                                  {
                                    name: "PEM certificate and key",
                                    extensions: ["pem", "crt", "key"],
                                  },
                                ],
                              });
                              if (typeof file === "string") setClientCert(file);
                            }}
                          >
                            Choose…
                          </button>
                          <button
                            type="button"
                            class="server-action"
                            onClick={() =>
                              void api
                                .keychainIdentities()
                                .then((items) => {
                                  setKeychainIdentities(items);
                                  if (!items.length)
                                    notify(
                                      `No certificates with a private key are in your ${certificateStore}.`,
                                      true,
                                    );
                                })
                                .catch((error) =>
                                  notify(errorMessage(error), true),
                                )
                            }
                          >
                            From {certificateStore}…
                          </button>
                          <Show when={clientCert()}>
                            <button
                              type="button"
                              class="server-action"
                              onClick={() => setClientCert("")}
                            >
                              Remove
                            </button>
                          </Show>
                        </span>
                        <Show when={keychainIdentities().length}>
                          <select
                            aria-label={`Certificate from the ${certificateStore}`}
                            value=""
                            onChange={(event) => {
                              const id = event.currentTarget.value;
                              if (!id) return;
                              void api
                                .exportKeychainIdentity(id)
                                .then((path) => {
                                  setClientCert(path);
                                  setKeychainIdentities([]);
                                })
                                .catch((error) =>
                                  notify(errorMessage(error), true),
                                );
                            }}
                          >
                            <option value="">Choose a certificate…</option>
                            <For each={keychainIdentities()}>
                              {(item) => (
                                <option value={item.id}>{item.name}</option>
                              )}
                            </For>
                          </select>
                        </Show>
                        <small class="muted">
                          For servers that require a certificate to sign in: a
                          PEM file with the certificate and key
                          {certificateStore
                            ? `, or a certificate from your ${certificateStore} (${systemName} asks before sharing its key, and Weave keeps a private copy)`
                            : ""}
                          .
                        </small>
                      </label>
                    </Show>
                    <label>
                      Group <span class="optional">optional</span>
                      <input
                        list="host-group-names"
                        value={editorGroup()}
                        placeholder="Production"
                        onInput={(event) =>
                          setEditorGroup(event.currentTarget.value)
                        }
                      />
                      <datalist id="host-group-names">
                        <For each={knownGroups(hostMeta())}>
                          {(group) => <option value={group} />}
                        </For>
                      </datalist>
                    </label>
                    <label>
                      Tags <span class="optional">comma separated</span>
                      <input
                        value={editorTags()}
                        placeholder="linux, web"
                        onInput={(event) =>
                          setEditorTags(event.currentTarget.value)
                        }
                      />
                    </label>
                    <label>
                      Accent color
                      <input
                        type="color"
                        value={profile().color}
                        onInput={(event) =>
                          setProfileEditor({
                            ...profile(),
                            color: event.currentTarget.value,
                          })
                        }
                      />
                    </label>
                    <label class="profile-favorite">
                      <input
                        type="checkbox"
                        checked={profile().favorite}
                        onChange={(event) =>
                          setProfileEditor({
                            ...profile(),
                            favorite: event.currentTarget.checked,
                          })
                        }
                      />
                      Show this host first
                    </label>
                  </div>
                  <div class="dialog-actions">
                    <Show when={profile().id}>
                      <Show
                        when={
                          profile().protocol !== "ssh" &&
                          !isCloud(profile().protocol)
                        }
                      >
                        <button
                          type="button"
                          class="ghost-button"
                          onClick={() =>
                            void clearProfilePassword(profile().id)
                          }
                        >
                          Forget password
                        </button>
                      </Show>
                      <button
                        type="button"
                        class="ghost-button"
                        onClick={() =>
                          openProfileEditor({
                            ...profile(),
                            id: "",
                            name: `${profile().name} copy`,
                          })
                        }
                      >
                        Duplicate
                      </button>
                      <button
                        type="button"
                        class="ghost-button danger-text"
                        onClick={() => deleteProfile(profile())}
                      >
                        Delete host
                      </button>
                    </Show>
                    <span class="spacer" />
                    <button
                      type="button"
                      class="ghost-button"
                      onClick={() => closeProfileEditor()}
                    >
                      Cancel
                    </button>
                    <button type="submit" class="primary-button">
                      Save host
                    </button>
                  </div>
                </form>
                <div class="import-link">
                  <button onClick={importSshConfig}>
                    Import hosts from ~/.ssh/config <ArrowRight size={14} />
                  </button>
                  <button onClick={() => void importFileZilla()}>
                    Import FileZilla sites <ArrowRight size={14} />
                  </button>
                  <button onClick={() => void importSessions()}>
                    Import WinSCP, PuTTY, MobaXterm or Cyberduck{" "}
                    <ArrowRight size={14} />
                  </button>
                  <button
                    onClick={() => {
                      closeProfileEditor();
                      setVaultOpen(true);
                    }}
                  >
                    Import from settings vault <ArrowRight size={14} />
                  </button>
                </div>
              </div>
            </div>
          )}
        </Show>
        <Show when={searchAllOpen()}>
          <TerminalSearchAll
            sessions={sessions()}
            onShow={(key) => {
              setPage("terminal");
              setActiveSession(key);
            }}
            onClose={() => setSearchAllOpen(false)}
          />
        </Show>
        <Show when={cloneOpen()}>
          <div class="overlay" onClick={closeClone}>
            <div
              class="dialog profile-dialog clone-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Clone repository"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">VERSION CONTROL</span>
                  <h2>Clone repository</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={closeClone}
                >
                  <X size={19} />
                </button>
              </div>
              <p class="muted">
                Copy a repository to this computer. SSH URLs use your SSH key;
                HTTPS URLs can use a connected account or your Git credentials.
              </p>
              <form
                onSubmit={(event) => {
                  event.preventDefault();
                  void cloneRepo();
                }}
              >
                <div class="form-grid">
                  <label class="wide">
                    Repository URL
                    <input
                      required
                      value={cloneUrl()}
                      onInput={(event) => {
                        setCloneUrl(event.currentTarget.value);
                        if (cloneBrowseServerUrl()) setCloneBrowseOpen(false);
                      }}
                      onBlur={() => {
                        if (!cloneName())
                          setCloneName(
                            cloneUrl()
                              .split("/")
                              .pop()
                              ?.replace(/\.git$/, "") ?? "",
                          );
                      }}
                      placeholder="https://github.com/team/project.git"
                    />
                  </label>
                  <div class="clone-browse wide">
                    <button
                      type="button"
                      class="ghost-button"
                      onClick={() => void browseCloneRepositories()}
                    >
                      Browse GitHub or GitLab repositories
                    </button>
                    <Show
                      when={cloneUrl().trim() && !isHostedForgeUrl(cloneUrl())}
                    >
                      <button
                        type="button"
                        class="ghost-button"
                        disabled={cloneRepoBusy()}
                        onClick={() => void browseSelfHostedRepositories()}
                      >
                        Browse this server
                      </button>
                    </Show>
                    <Show when={cloneBrowseOpen()}>
                      <div class="clone-browser">
                        <div class="clone-browser-tabs">
                          <Show
                            when={cloneBrowseServerUrl()}
                            fallback={
                              <>
                                <button
                                  type="button"
                                  class={
                                    cloneProvider() === "GitHub" ? "active" : ""
                                  }
                                  onClick={() =>
                                    void browseCloneRepositories("GitHub")
                                  }
                                >
                                  GitHub
                                </button>
                                <button
                                  type="button"
                                  class={
                                    cloneProvider() === "GitLab" ? "active" : ""
                                  }
                                  onClick={() =>
                                    void browseCloneRepositories("GitLab")
                                  }
                                >
                                  GitLab
                                </button>
                              </>
                            }
                          >
                            <strong>
                              {cloneProvider()} self-hosted repositories
                            </strong>
                          </Show>
                          <span class="spacer" />
                          <button
                            type="button"
                            class="mini-button"
                            title="Close repository browser"
                            onClick={() => setCloneBrowseOpen(false)}
                          >
                            <X size={15} />
                          </button>
                        </div>
                        <Show
                          when={
                            cloneBrowseServerUrl() ||
                            cloneAccounts().find(
                              (account) => account.provider === cloneProvider(),
                            )?.connected
                          }
                          fallback={
                            <p class="muted">
                              Enter a token below, then connect{" "}
                              {cloneProvider()} to see your repositories.
                            </p>
                          }
                        >
                          <Show when={cloneBrowseError()}>
                            <p class="clone-browser-error" role="alert">
                              {cloneBrowseError()}
                            </p>
                          </Show>
                          <input
                            aria-label="Search available repositories"
                            value={cloneRepoQuery()}
                            onInput={(event) =>
                              setCloneRepoQuery(event.currentTarget.value)
                            }
                            placeholder="Filter repositories"
                          />
                          <button
                            type="button"
                            class="ghost-button"
                            onClick={() =>
                              setCloneCreateOpen(!cloneCreateOpen())
                            }
                          >
                            {cloneCreateOpen()
                              ? "Cancel new repository"
                              : "Create repository"}
                          </button>
                          <Show when={cloneCreateOpen()}>
                            <div class="clone-create-repository">
                              <p class="muted">
                                Leave the namespace empty for your account, or
                                enter a{" "}
                                {cloneProvider() === "GitHub"
                                  ? "GitHub organization"
                                  : "GitLab group path"}
                                .
                              </p>
                              <input
                                aria-label="New repository organization or group"
                                value={cloneCreateNamespace()}
                                onInput={(event) =>
                                  setCloneCreateNamespace(
                                    event.currentTarget.value,
                                  )
                                }
                                placeholder={
                                  cloneProvider() === "GitHub"
                                    ? "organization (optional)"
                                    : "group/subgroup (optional)"
                                }
                              />
                              <input
                                aria-label="New hosted repository name"
                                value={cloneCreateName()}
                                onInput={(event) =>
                                  setCloneCreateName(event.currentTarget.value)
                                }
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault();
                                    void createHostedRepository();
                                  }
                                }}
                                placeholder="repository-name"
                              />
                              <input
                                aria-label="New hosted repository description"
                                value={cloneCreateDescription()}
                                onInput={(event) =>
                                  setCloneCreateDescription(
                                    event.currentTarget.value,
                                  )
                                }
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault();
                                    void createHostedRepository();
                                  }
                                }}
                                placeholder="Optional description"
                              />
                              <label>
                                <input
                                  type="checkbox"
                                  checked={cloneCreatePrivate()}
                                  onChange={(event) =>
                                    setCloneCreatePrivate(
                                      event.currentTarget.checked,
                                    )
                                  }
                                />
                                Private repository
                              </label>
                              <button
                                type="button"
                                class="mini-button"
                                disabled={cloneRepoBusy()}
                                onClick={() => void createHostedRepository()}
                              >
                                {cloneRepoBusy()
                                  ? "Creating…"
                                  : `Create on ${cloneProvider()}`}
                              </button>
                            </div>
                          </Show>
                          <div class="clone-repo-list">
                            <For
                              each={filteredCloneRepositories()}
                              fallback={
                                <p class="muted">
                                  {cloneRepoBusy()
                                    ? "Loading repositories…"
                                    : "No matching repositories on this page."}
                                </p>
                              }
                            >
                              {(repo) => (
                                <div class="clone-repo-row">
                                  <span class="truncate">
                                    {repo.name}
                                    {repo.private ? " · Private" : ""}
                                  </span>
                                  <button
                                    type="button"
                                    class="mini-button"
                                    onClick={() =>
                                      chooseCloneRepository(repo, "https")
                                    }
                                  >
                                    HTTPS
                                  </button>
                                  <button
                                    type="button"
                                    class="mini-button"
                                    onClick={() =>
                                      chooseCloneRepository(repo, "ssh")
                                    }
                                  >
                                    SSH
                                  </button>
                                </div>
                              )}
                            </For>
                          </div>
                          <Show when={cloneRepoHasMore()}>
                            <button
                              type="button"
                              class="ghost-button"
                              disabled={cloneRepoBusy()}
                              onClick={() =>
                                void loadCloneRepositories(
                                  cloneProvider(),
                                  cloneRepoPage() + 1,
                                )
                              }
                            >
                              Load more repositories
                            </button>
                          </Show>
                        </Show>
                      </div>
                    </Show>
                  </div>
                  <label class="wide">
                    Save inside
                    <div class="browse-field">
                      <input
                        required
                        value={cloneParent()}
                        onInput={(event) =>
                          setCloneParent(event.currentTarget.value)
                        }
                        placeholder="Choose a parent folder"
                      />
                      <button
                        type="button"
                        class="ghost-button"
                        onClick={async () => {
                          const folder = await open({
                            directory: true,
                            multiple: false,
                          });
                          if (typeof folder === "string")
                            setCloneParent(folder);
                        }}
                      >
                        Browse
                      </button>
                    </div>
                  </label>
                  <label class="wide">
                    New folder name
                    <input
                      required
                      value={cloneName()}
                      onInput={(event) =>
                        setCloneName(event.currentTarget.value)
                      }
                      placeholder="project"
                    />
                  </label>
                  <label class="wide">
                    Recent commits only{" "}
                    <span class="optional">
                      Leave empty for the full history; a number makes a faster,
                      shallow clone
                    </span>
                    <input
                      type="number"
                      min="1"
                      step="1"
                      inputMode="numeric"
                      value={cloneDepth()}
                      onInput={(event) =>
                        setCloneDepth(event.currentTarget.value)
                      }
                      placeholder="All"
                    />
                  </label>
                  <label class="wide">
                    GitHub or GitLab access token{" "}
                    <span class="optional">
                      Optional for private HTTPS repositories
                    </span>
                    <input
                      type="password"
                      autocomplete="off"
                      value={cloneToken()}
                      onInput={(event) =>
                        setCloneToken(event.currentTarget.value)
                      }
                      placeholder={`Saved securely in ${keychain}`}
                    />
                  </label>
                  <Show
                    when={cloneUrl().trim() && !isHostedForgeUrl(cloneUrl())}
                  >
                    <label class="wide">
                      Self-hosted server type
                      <select
                        value={cloneProvider()}
                        onChange={(event) => {
                          setCloneProvider(
                            event.currentTarget.value as "GitHub" | "GitLab",
                          );
                          setCloneBrowseOpen(false);
                        }}
                      >
                        <option value="GitHub">GitHub Enterprise Server</option>
                        <option value="GitLab">GitLab Self-Managed</option>
                      </select>
                    </label>
                  </Show>
                  <Show
                    when={
                      cloneBrowseOpen() &&
                      !cloneBrowseServerUrl() &&
                      !cloneAccounts().find(
                        (account) => account.provider === cloneProvider(),
                      )?.connected
                    }
                  >
                    <button
                      type="button"
                      class="ghost-button wide"
                      disabled={cloneRepoBusy()}
                      onClick={() => void connectCloneProvider()}
                    >
                      Connect {cloneProvider()} account
                    </button>
                    <OAuthSignIn
                      provider={cloneProvider()}
                      onConnected={() =>
                        void api.forgeAccounts().then(async (accounts) => {
                          setCloneAccounts(accounts);
                          notify(`${cloneProvider()} account connected.`);
                          await loadCloneRepositories(cloneProvider());
                        })
                      }
                    />
                  </Show>
                </div>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={closeClone}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    class="primary-button"
                    disabled={cloneBusy()}
                  >
                    {cloneBusy() ? "Cloning…" : "Clone repository"}
                  </button>
                </div>
              </form>
            </div>
          </div>
        </Show>

        <Show when={paletteOpen()}>
          <div
            class="overlay palette-overlay"
            onClick={() => setPaletteOpen(false)}
          >
            <div
              class="palette"
              role="dialog"
              aria-modal="true"
              aria-label="Command palette"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="palette-search">
                <Search size={19} />
                <input
                  autofocus
                  aria-label="Search actions, repositories and hosts"
                  value={paletteQuery()}
                  onInput={(event) =>
                    setPaletteQuery(event.currentTarget.value)
                  }
                  placeholder="Search actions, repositories, hosts…"
                />
                <kbd>ESC</kbd>
              </div>
              <div class="palette-results">
                <For
                  each={paletteItems()}
                  fallback={<div class="empty-small">No matching actions</div>}
                >
                  {(item) => (
                    <button
                      onClick={() => {
                        setPaletteOpen(false);
                        item.run();
                      }}
                    >
                      <Command size={17} />
                      <span>{item.label}</span>
                      <small>{item.detail}</small>
                    </button>
                  )}
                </For>
              </div>
              <div class="palette-footer">
                Tip: press{" "}
                {keybindings().palette
                  ? shortcutLabel(keybindings().palette)
                  : "the palette shortcut"}{" "}
                anywhere to jump back here.
              </div>
            </div>
          </div>
        </Show>
        <Show when={toast()}>
          {(item) => (
            <div
              class={`toast ${item().error ? "toast-error" : ""}`}
              role="status"
            >
              <span>{item().error ? "!" : "✓"}</span>
              {item().message}
              <button title="Dismiss" onClick={() => setToast(undefined)}>
                <X size={15} />
              </button>
            </div>
          )}
        </Show>
      </div>
      <TextPromptDialog />
      <ContextMenu />
      <SshPromptDialog onError={(message) => notify(message, true)} />
      <Show when={locked()}>
        <AppLockScreen
          usesPassword={lockSettings()?.usesPassword ?? false}
          onUnlocked={() => {
            lastActive = Date.now();
            setLocked(false);
          }}
        />
      </Show>
    </>
  );
}
