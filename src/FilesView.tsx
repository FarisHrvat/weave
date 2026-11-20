import { confirmAction } from "./dialogs";
import { fixedShortcuts, shortcutLabel } from "./shortcuts";
import {
  fileManager,
  isMac,
  primaryKey,
  systemName,
  thisComputer,
} from "./platform";
import MenuButton from "./MenuButton";
import CryptomatorDialog from "./CryptomatorDialog";
import FileToolDialogs, { type FileTool } from "./FileToolDialogs";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  batch,
  createEffect,
  createSignal,
  For,
  on,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ArrowRight,
  ChevronRight,
  Copy,
  File,
  FileText,
  Folder,
  FolderPlus,
  HardDrive,
  Pencil,
  RefreshCw,
  Server,
  Shield,
  Star,
  Trash2,
  X,
  Eye,
  Link2,
  SquareTerminal,
  Users,
  Archive,
  PackageOpen,
  ClipboardCopy,
  Tags,
} from "lucide-solid";
import { api, errorMessage } from "./api";
import { isArchive } from "./archives";
import { selectFileEntry, type FileSelection } from "./fileSelection";
import {
  bookmarkFor,
  loadBookmarks,
  saveBookmarks,
  type FileBookmark,
} from "./bookmarks";
import type {
  FileEntry,
  ExternalEditInfo,
  BatchPreview,
  FileListing,
  FileLocation,
  Profile,
  SyncPlan,
  SyncMode,
  SyncJob,
  TransferRecord,
} from "./types";

type Side = "left" | "right";
type FileDialog =
  | { kind: "copy"; source: string; sourceSide: Side; destinationSide: Side }
  | { kind: "mkdir"; side: Side }
  | { kind: "delete-many"; side: Side; entries: FileEntry[] }
  | { kind: "rename" | "delete" | "chmod"; side: Side; entry: FileEntry };
type Props = {
  home: string;
  locations?: { left: FileLocation; right: FileLocation };
  onLocationChange: (side: Side, location: FileLocation) => void;
  profiles: Profile[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
  onOpenTerminal: (cwd: string) => void;
  onConnectHost: (profile: Profile, cwd?: string) => void;
  onTransferQueued: (record: TransferRecord) => void;
  onOpenAsRepo: (path: string) => void;
};

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

function date(seconds: number | null): string {
  return seconds
    ? new Date(seconds * 1000).toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        year: "numeric",
      })
    : "—";
}

function protocolLabel(protocol: Profile["protocol"] | undefined): string {
  if (!protocol) return "Remote files";
  if (protocol === "ssh") return "SFTP";
  if (protocol === "webdav") return "WebDAV";
  if (protocol === "ftps-implicit") return "FTPS implicit";
  if (protocol === "azure") return "Azure Blob";
  if (protocol === "dropbox") return "Dropbox";
  if (protocol === "gdrive") return "Google Drive";
  if (protocol === "onedrive") return "OneDrive";
  return protocol.toUpperCase();
}

export default function FilesView(props: Props) {
  const [left, setLeft] = createSignal<FileListing>();
  const [right, setRight] = createSignal<FileListing>();
  const [leftPath, setLeftPath] = createSignal(props.home);
  const [rightPath, setRightPath] = createSignal(props.home);
  const [leftProfile, setLeftProfile] = createSignal<string | null>(null);
  const [rightProfile, setRightProfile] = createSignal<string | null>(null);
  const [selection, setSelection] = createSignal<FileSelection>();
  const [selectionAnchor, setSelectionAnchor] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [sort, setSort] = createSignal<"name" | "size" | "modified">("name");
  const [view, setView] = createSignal<"list" | "grid">("list");
  const [leftFilter, setLeftFilter] = createSignal("");
  const [rightFilter, setRightFilter] = createSignal("");
  const [dialog, setDialog] = createSignal<FileDialog>();
  const [dialogValue, setDialogValue] = createSignal("");
  const [syncOpen, setSyncOpen] = createSignal(false);
  const [syncMode, setSyncMode] = createSignal<SyncMode>("left-to-right");
  const [syncPreview, setSyncPreview] = createSignal<SyncPlan>();
  const [syncJobsOpen, setSyncJobsOpen] = createSignal(false);
  const [syncJobs, setSyncJobs] = createSignal<SyncJob[]>([]);
  const [jobName, setJobName] = createSignal("");
  const [jobMode, setJobMode] = createSignal<SyncMode>("left-to-right");
  const [jobInterval, setJobInterval] = createSignal(60);
  const [jobWatchLocal, setJobWatchLocal] = createSignal(false);
  const [excludeMask, setExcludeMask] = createSignal("");
  const [editing, setEditing] = createSignal<{
    path: string;
    profileId: string | null;
    original: string;
    content: string;
    sha256: string;
  }>();
  const [externalEdit, setExternalEdit] = createSignal<ExternalEditInfo>();
  const [externalEdits, setExternalEdits] = createSignal<ExternalEditInfo[]>(
    [],
  );
  const [confirmDiscardExternal, setConfirmDiscardExternal] =
    createSignal(false);
  const [batchPath, setBatchPath] = createSignal("");
  const [batchPreview, setBatchPreview] = createSignal<BatchPreview>();
  const [batchError, setBatchError] = createSignal("");
  const [shareTarget, setShareTarget] = createSignal<{
    profileId: string;
    path: string;
    protocol: "s3" | "webdav";
  }>();
  const [shareDays, setShareDays] = createSignal(7);
  const [sharePassword, setSharePassword] = createSignal("");
  const [shareLifetime, setShareLifetime] = createSignal(3600);
  const [vaultOpen, setVaultOpen] = createSignal(false);
  const [fileTool, setFileTool] = createSignal<FileTool>();
  const [linkPanes, setLinkPanes] = createSignal(false);
  const [shareUrl, setShareUrl] = createSignal("");
  const [directOpen, setDirectOpen] = createSignal(false);
  const [fxpOpen, setFxpOpen] = createSignal(false);
  const [fxpClearData, setFxpClearData] = createSignal(false);
  const [directMethod, setDirectMethod] = createSignal<"scp" | "rsync">("scp");
  const [directConflict, setDirectConflict] = createSignal("rename");
  const [bookmarks, setBookmarks] = createSignal<FileBookmark[]>(
    loadBookmarks(localStorage),
  );
  const navigationRequest: Record<Side, number> = { left: 0, right: 0 };

  const listingFor = (side: Side) => (side === "left" ? left() : right());
  const selected = () => {
    const current = selection();
    return current?.entries.length === 1
      ? { side: current.side, entry: current.entries[0] }
      : undefined;
  };
  const profileFor = (side: Side) =>
    side === "left" ? leftProfile() : rightProfile();
  const profileDetails = (side: Side) =>
    props.profiles.find((profile) => profile.id === profileFor(side));
  const canDirectTransfer = () => {
    const current = selection();
    if (!current?.entries.length) return false;
    const other: Side = current.side === "left" ? "right" : "left";
    const source = profileDetails(current.side);
    const destination = profileDetails(other);
    return (
      (source?.protocol === "ssh" && !destination) ||
      (!source && destination?.protocol === "ssh") ||
      (source?.protocol === "ssh" && destination?.protocol === "ssh")
    );
  };
  const directBetweenHosts = () =>
    !!profileFor("left") &&
    !!profileFor("right") &&
    profileDetails("left")?.protocol === "ssh" &&
    profileDetails("right")?.protocol === "ssh";
  const canFxpTransfer = () => {
    const current = selected();
    if (!current || current.entry.isDir) return false;
    const other: Side = current.side === "left" ? "right" : "left";
    const source = profileDetails(current.side);
    const destination = profileDetails(other);
    const ftpFamily = (protocol?: string) =>
      protocol === "ftp" || protocol === "ftps" || protocol === "ftps-implicit";
    return (
      ftpFamily(source?.protocol) &&
      ftpFamily(destination?.protocol) &&
      source?.id !== destination?.id
    );
  };
  const fxpSecure = () => {
    const current = selected();
    if (!current) return false;
    const other: Side = current.side === "left" ? "right" : "left";
    return [current.side, other].some((side) =>
      profileDetails(side)?.protocol.startsWith("ftps"),
    );
  };

  async function runFxpTransfer() {
    const current = selected();
    if (!current || !canFxpTransfer()) return;
    const other: Side = current.side === "left" ? "right" : "left";
    const destination = listingFor(other);
    const sourceId = profileFor(current.side);
    const destinationId = profileFor(other);
    if (!destination || !sourceId || !destinationId) {
      props.onError("Open both FTP folders before starting FXP.");
      return;
    }
    setBusy(true);
    try {
      const result = await api.fxpTransfer(
        sourceId,
        current.entry.path,
        destinationId,
        destination.path,
        fxpClearData(),
      );
      setFxpOpen(false);
      setFxpClearData(false);
      await refresh();
      props.onNotice(
        `FXP copied ${size(result.bytesCopied)} directly between the servers to ${result.destinationPath}${result.verifiedWith ? `, verified with ${result.verifiedWith}` : ", checked by size"}.`,
      );
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function runDirectTransfer() {
    const current = selection();
    if (!current || !canDirectTransfer()) return;
    const other: Side = current.side === "left" ? "right" : "left";
    const destination = listingFor(other);
    if (!destination) {
      props.onError("Open a destination folder before starting SCP or rsync.");
      return;
    }
    setBusy(true);
    let queued = 0;
    try {
      for (const entry of current.entries) {
        const record = await api.enqueueDirectTransfer(
          directMethod(),
          profileFor(current.side),
          entry.path,
          profileFor(other),
          destination.path,
          directConflict(),
        );
        props.onTransferQueued(record);
        queued += 1;
      }
      setDirectOpen(false);
      props.onNotice(
        `${directMethod() === "scp" ? "SCP" : "rsync"} transfer${queued === 1 ? "" : "s"} added to the activity queue.`,
      );
    } catch (error) {
      props.onError(
        queued
          ? `Queued ${queued} of ${current.entries.length} items. ${errorMessage(error)}`
          : errorMessage(error),
      );
    } finally {
      setBusy(false);
    }
  }
  const canWatchJobSource = () =>
    jobMode() === "left-to-right" || jobMode() === "right-to-left";
  const jobSourceIsLocal = () =>
    jobMode() === "left-to-right" ? !leftProfile() : !rightProfile();
  createEffect(() => {
    if (!canWatchJobSource()) setJobWatchLocal(false);
  });

  function toggleBookmark(side: Side) {
    const listing = listingFor(side);
    if (!listing) return;
    const location = { path: listing.path, profileId: profileFor(side) };
    const existing = bookmarkFor(bookmarks(), location);
    const next = existing
      ? bookmarks().filter((bookmark) => bookmark.id !== existing.id)
      : [
          ...bookmarks(),
          {
            ...location,
            id: crypto.randomUUID(),
            name: listing.path.split("/").filter(Boolean).at(-1) ?? "/",
          },
        ];
    try {
      saveBookmarks(localStorage, next);
      setBookmarks(next);
      props.onNotice(existing ? "Bookmark removed." : "Folder bookmarked.");
    } catch (error) {
      props.onError(`Could not save folder bookmark: ${errorMessage(error)}`);
    }
  }

  async function navigate(
    side: Side,
    path: string,
    profileId = profileFor(side),
    follow = true,
  ): Promise<boolean> {
    const request = ++navigationRequest[side];
    const before = listingFor(side);
    try {
      const listing = profileId
        ? await api.sftpFiles(profileId, path)
        : await api.files(path);
      if (request !== navigationRequest[side]) return false;
      batch(() => {
        if (side === "left") {
          setLeft(listing);
          setLeftPath(listing.path);
          setLeftProfile(profileId);
        } else {
          setRight(listing);
          setRightPath(listing.path);
          setRightProfile(profileId);
        }
        props.onLocationChange(side, { path: listing.path, profileId });
        setSelection(undefined);
        setSelectionAnchor(undefined);
      });
      if (follow && linkPanes() && before && profileId === profileFor(side))
        await followInOtherPane(side, before, listing);
      return true;
    } catch (error) {
      if (request === navigationRequest[side])
        props.onError(errorMessage(error));
      return false;
    }
  }

  /** Repeats a move into a child folder or up to the parent in the other pane. */
  async function followInOtherPane(
    side: Side,
    before: FileListing,
    after: FileListing,
  ) {
    const other: Side = side === "left" ? "right" : "left";
    const current = listingFor(other);
    if (!current) return;
    const join = (folder: string, name: string) =>
      `${folder.replace(/\/+$/, "")}/${name}`;
    let target: string | undefined;
    if (after.parent === before.path) {
      const name = after.path.replace(/\/+$/, "").split("/").pop() ?? "";
      target = join(current.path, name);
    } else if (before.parent === after.path) {
      target = current.parent ?? undefined;
    }
    if (!target) return;
    const moved = await navigate(other, target, profileFor(other), false);
    if (!moved) {
      setLinkPanes(false);
      props.onNotice(
        "The other pane has no matching folder, so linked browsing was turned off.",
      );
    }
  }

  async function reveal(side: Side, path: string) {
    const parent =
      path.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/";
    if (!(await navigate(side, parent, profileFor(side), false))) return;
    const entry = listingFor(side)?.entries.find((item) => item.path === path);
    if (entry) setSelection({ side, entries: [entry] });
  }

  onMount(() => {
    // Files dropped from the file manager are copied into the pane under the pointer.
    if (!("__TAURI_INTERNALS__" in window)) return;
    const stop = getCurrentWebview().onDragDropEvent(async (event) => {
      if (event.payload.type !== "drop") return;
      const scale = window.devicePixelRatio || 1;
      const element = document.elementFromPoint(
        event.payload.position.x / scale,
        event.payload.position.y / scale,
      );
      const side = element
        ?.closest("[data-pane-side]")
        ?.getAttribute("data-pane-side") as Side | null;
      const destination = side ? listingFor(side) : undefined;
      if (!side || !destination || !event.payload.paths.length) return;
      let queued = 0;
      try {
        for (const path of event.payload.paths) {
          props.onTransferQueued(
            await api.enqueueTransfer(
              null,
              path,
              profileFor(side),
              destination.path,
              "rename",
            ),
          );
          queued += 1;
        }
        props.onNotice(`${queued} item(s) added to the activity queue.`);
      } catch (error) {
        props.onError(errorMessage(error));
      }
    });
    onCleanup(() => void stop.then((unlisten) => unlisten()));
  });

  // Only a new requested location navigates; listing updates never loop back here.
  createEffect(
    on(
      () =>
        [
          props.home,
          props.locations?.left?.path,
          props.locations?.left?.profileId,
        ] as const,
      ([home, path, profileId]) => {
        if (!home) return;
        const location =
          path === undefined
            ? { path: home, profileId: null }
            : { path, profileId: profileId ?? null };
        if (
          !left() ||
          left()?.path !== location.path ||
          leftProfile() !== location.profileId
        )
          void navigate("left", location.path, location.profileId);
      },
    ),
  );
  // Only a new requested location navigates; listing updates never loop back here.
  createEffect(
    on(
      () =>
        [
          props.home,
          props.locations?.right?.path,
          props.locations?.right?.profileId,
        ] as const,
      ([home, path, profileId]) => {
        if (!home) return;
        const location =
          path === undefined
            ? { path: home, profileId: null }
            : { path, profileId: profileId ?? null };
        if (
          !right() ||
          right()?.path !== location.path ||
          rightProfile() !== location.profileId
        )
          void navigate("right", location.path, location.profileId);
      },
    ),
  );

  // Listeners are added asynchronously, so cleanup is registered up front.
  const stopListening: (() => void)[] = [];
  let unmounted = false;
  onCleanup(() => {
    unmounted = true;
    stopListening.forEach((stop) => stop());
  });
  const keep = (stop: () => void) => {
    if (unmounted) stop();
    else stopListening.push(stop);
  };

  onMount(async () => {
    void api
      .externalEdits()
      .then(setExternalEdits)
      .catch((error) => props.onError(errorMessage(error)));
    keep(
      await listen<{ id: string; remotePath: string; error: string | null }>(
        "external-edit-uploaded",
        (event) => {
          void api.externalEdits().then((items) => {
            setExternalEdits(items);
            const open = externalEdit();
            if (open)
              setExternalEdit(items.find((item) => item.id === open.id));
          });
          if (event.payload.error)
            props.onError(
              `Automatic upload of ${event.payload.remotePath} stopped: ${event.payload.error}`,
            );
          else {
            props.onNotice(`Uploaded ${event.payload.remotePath}.`);
            void refresh();
          }
        },
      ),
    );
    const unlisten = await listen<TransferRecord>(
      "transfer-progress",
      (event) => {
        if (["done", "skipped"].includes(event.payload.status)) void refresh();
        if (
          syncJobsOpen() &&
          ["done", "skipped", "failed"].includes(event.payload.status)
        ) {
          void api
            .syncJobs()
            .then(setSyncJobs)
            .catch((error) =>
              props.onError(
                `Could not refresh sync jobs: ${errorMessage(error)}`,
              ),
            );
        }
      },
    );
    keep(unlisten);
  });

  function openPreview() {
    const current = selected();
    if (!current || current.entry.isDir) return;
    setFileTool({
      kind: "preview",
      profileId: profileFor(current.side),
      path: current.entry.path,
      name: current.entry.name,
    });
  }

  function openFind() {
    const side = selection()?.side ?? "left";
    const listing = listingFor(side);
    if (listing)
      setFileTool({
        kind: "find",
        profileId: profileFor(side),
        root: listing.path,
      });
  }

  onMount(() => {
    const keys = (event: KeyboardEvent) => {
      if (!primaryKey(event) || fileTool()) return;
      const key = event.key.toLowerCase();
      if (key === "y" && selected()) {
        event.preventDefault();
        openPreview();
      } else if (key === "f" && event.shiftKey) {
        event.preventDefault();
        openFind();
      }
    };
    window.addEventListener("keydown", keys);
    onCleanup(() => window.removeEventListener("keydown", keys));
  });

  function canUseShell(side: "left" | "right") {
    return !profileFor(side) || profileDetails(side)?.protocol === "ssh";
  }

  async function extractSelected() {
    const item = selected();
    if (!item) return;
    setBusy(true);
    try {
      const folder = await api.filesExtract(
        profileFor(item.side),
        listingFor(item.side)?.path ?? "",
        item.entry.name,
      );
      props.onNotice(`Extracted into ${folder}.`);
      await refresh();
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function mountInFinder() {
    const profileId = profileFor(selection()?.side ?? "left");
    if (!profileId) return;
    setBusy(true);
    try {
      const path = await api.mountInFinder(profileId);
      props.onNotice(`Mounted at ${path}.`);
      await revealItemInDir(path).catch(() => undefined);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function copyPaths() {
    const current = selection();
    if (!current) return;
    const profile = profileDetails(current.side);
    const paths = current.entries.map((entry) =>
      profile && profile.protocol === "ssh"
        ? `${profile.username ? `${profile.username}@` : ""}${profile.host}:${entry.path}`
        : entry.path,
    );
    try {
      await navigator.clipboard.writeText(paths.join("\n"));
      props.onNotice(
        paths.length === 1 ? "Path copied." : `${paths.length} paths copied.`,
      );
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function refresh() {
    await Promise.all([
      navigate("left", left()?.path ?? props.home, leftProfile()),
      navigate("right", right()?.path ?? props.home, rightProfile()),
    ]);
  }

  async function copy(source: string, sourceSide: Side, destinationSide: Side) {
    const destination = listingFor(destinationSide);
    if (!destination) return;
    const name = source.split("/").pop();
    const exists = destination.entries.some((entry) => entry.name === name);
    if (exists) {
      setDialog({ kind: "copy", source, sourceSide, destinationSide });
      setDialogValue("rename");
      return;
    }
    await enqueue(source, sourceSide, destinationSide, "rename");
  }

  async function copyItems(
    paths: string[],
    sourceSide: Side,
    destinationSide: Side,
  ) {
    const destination = listingFor(destinationSide);
    if (!paths.length || !destination) return;
    if (paths.length === 1) {
      await copy(paths[0], sourceSide, destinationSide);
      return;
    }
    setBusy(true);
    let queued = 0;
    try {
      for (const path of paths) {
        const record = await api.enqueueTransfer(
          profileFor(sourceSide),
          path,
          profileFor(destinationSide),
          destination.path,
          "rename",
        );
        props.onTransferQueued(record);
        queued += 1;
      }
      props.onNotice(`${queued} items added to the activity queue.`);
    } catch (error) {
      props.onError(
        `Queued ${queued} of ${paths.length} items. ${errorMessage(error)}`,
      );
    } finally {
      setBusy(false);
    }
  }

  async function copySelection(sourceSide: Side, destinationSide: Side) {
    const current = selection();
    if (!current || current.side !== sourceSide) return;
    await copyItems(
      current.entries.map((entry) => entry.path),
      sourceSide,
      destinationSide,
    );
  }

  async function enqueue(
    source: string,
    sourceSide: Side,
    destinationSide: Side,
    conflict: string,
  ) {
    const destination = listingFor(destinationSide);
    if (!destination) return;
    setBusy(true);
    try {
      const record = await api.enqueueTransfer(
        profileFor(sourceSide),
        source,
        profileFor(destinationSide),
        destination.path,
        conflict,
      );
      props.onTransferQueued(record);
      props.onNotice("Transfer added to the activity queue.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function createFolder(side: Side) {
    if (!listingFor(side)) return;
    setDialogValue("");
    setDialog({ kind: "mkdir", side });
  }

  async function rename() {
    const item = selected();
    if (!item) return;
    setDialogValue(item.entry.name);
    setDialog({ kind: "rename", ...item });
  }

  async function remove() {
    const current = selection();
    if (current && current.entries.length > 1) {
      setDialog({
        kind: "delete-many",
        side: current.side,
        entries: current.entries,
      });
      return;
    }
    const item = selected();
    if (!item) return;
    setDialogValue("");
    setDialog({ kind: "delete", ...item });
  }

  async function changePermissions() {
    const item = selected();
    if (!item || !profileFor(item.side)) return;
    setDialogValue(item.entry.isDir ? "755" : "644");
    setDialog({ kind: "chmod", ...item });
  }

  async function openText(side: Side, entry: FileEntry) {
    if (entry.isDir) return;
    setBusy(true);
    try {
      const profileId = profileFor(side);
      const document = await api.readText(profileId, entry.path);
      setEditing({
        path: entry.path,
        profileId,
        original: document.content,
        content: document.content,
        sha256: document.sha256,
      });
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function saveText() {
    const current = editing();
    if (!current || current.content === current.original) return;
    setBusy(true);
    try {
      const saved = await api.saveText(
        current.profileId,
        current.path,
        current.sha256,
        current.content,
      );
      setEditing({
        ...current,
        original: saved.content,
        content: saved.content,
        sha256: saved.sha256,
      });
      await refresh();
      props.onNotice("Text file saved.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function openExternal(side: Side, entry: FileEntry) {
    if (entry.isDir || busy()) return;
    if (externalEdit()) {
      props.onError(
        "Keep the current editing copy for later or finish it before opening another file.",
      );
      return;
    }
    setBusy(true);
    try {
      const profileId = profileFor(side);
      if (!profileId) {
        await openPath(entry.path);
      } else {
        const session = await api.checkoutExternalEdit(profileId, entry.path);
        setExternalEdit(session);
        setExternalEdits(await api.externalEdits());
        await openPath(session.localPath);
      }
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function uploadExternal() {
    const session = externalEdit();
    if (!session) return;
    setBusy(true);
    try {
      const result = await api.uploadExternalEdit(session.id);
      setExternalEdit(result);
      setExternalEdits(await api.externalEdits());
      await refresh();
      props.onNotice(
        result.warning ??
          (result.changed
            ? "Remote file updated from your editing copy."
            : "The editing copy has no saved changes to upload."),
      );
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function discardExternal() {
    const session = externalEdit();
    if (!session) return;
    setBusy(true);
    try {
      await api.discardExternalEdit(session.id);
      const remaining = await api.externalEdits();
      setExternalEdits(remaining);
      setExternalEdit(remaining[0]);
      setConfirmDiscardExternal(false);
      props.onNotice("External editing copy removed.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function createShareUrl() {
    const target = shareTarget();
    if (!target || busy()) return;
    setBusy(true);
    try {
      setShareUrl(
        target.protocol === "s3"
          ? await api.s3ShareUrl(target.profileId, target.path, shareLifetime())
          : await api.webdavShareUrl(
              target.profileId,
              target.path,
              shareDays() || null,
              sharePassword() || null,
            ),
      );
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function copyShareUrl() {
    try {
      await api.s3ShareCopyLink(shareUrl());
      props.onNotice(
        "Share link copied. Anyone with this link can download the file until it expires.",
      );
    } catch (error) {
      props.onError(
        `Could not copy the link. Select and copy it manually: ${errorMessage(error)}`,
      );
    }
  }

  async function chooseBatchPlan() {
    try {
      const path = await open({
        directory: false,
        multiple: false,
        title: "Choose a copy plan",
        filters: [{ name: "JSON copy plan", extensions: ["json"] }],
      });
      if (typeof path !== "string") return;
      setBatchPath(path);
      setBatchError("");
      setBusy(true);
      setBatchPreview(await api.previewBatch(path));
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function queueBatchPlan() {
    const preview = batchPreview();
    if (!preview || busy()) return;
    setBusy(true);
    setBatchError("");
    try {
      const result = await api.enqueueBatch(batchPath(), preview.sha256);
      for (const record of result.records) props.onTransferQueued(record);
      setBatchPreview(undefined);
      props.onNotice(
        result.warning ??
          `${result.records.length} transfers added to the queue.`,
      );
    } catch (error) {
      setBatchError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function compareFolders(mode: SyncMode) {
    const source = left();
    const target = right();
    if (!source || !target) {
      props.onError("Open both file panes before comparing folders.");
      return;
    }
    setSyncOpen(true);
    setSyncMode(mode);
    setSyncPreview(undefined);
    setBusy(true);
    try {
      setSyncPreview(
        await api.syncPlan(
          leftProfile(),
          source.path,
          rightProfile(),
          target.path,
          mode,
          excludeMask(),
        ),
      );
    } catch (error) {
      props.onError(errorMessage(error));
      setSyncOpen(false);
    } finally {
      setBusy(false);
    }
  }

  async function queueSync() {
    const plan = syncPreview();
    if (!plan) return;
    setBusy(true);
    let queued = 0;
    try {
      for (const copy of plan.copies) {
        const record = await api.enqueueTransfer(
          copy.sourceProfile,
          copy.sourcePath,
          copy.destinationProfile,
          copy.destinationDir,
          "newer",
        );
        props.onTransferQueued(record);
        queued++;
      }
      setSyncOpen(false);
      props.onNotice(
        queued
          ? `Queued ${queued} sync transfer${queued === 1 ? "" : "s"}.${plan.deletes.length ? " Compare again after they finish to review destination extras." : ""}`
          : plan.issues.length
            ? `No copies queued. Review ${plan.issues.length} item${plan.issues.length === 1 ? "" : "s"} that need manual attention.`
            : "Both folders match by name, size, and modification time.",
      );
    } catch (error) {
      props.onError(
        `Queued ${queued} of ${plan.copies.length} transfers. ${errorMessage(error)}`,
      );
    } finally {
      setBusy(false);
    }
  }

  async function openSyncJobs() {
    setSyncJobsOpen(true);
    try {
      setSyncJobs(await api.syncJobs());
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  async function saveSyncJob() {
    const source = left();
    const target = right();
    if (!source || !target) {
      props.onError("Open both file panes before scheduling a sync job.");
      return;
    }
    setBusy(true);
    try {
      await api.saveSyncJob(
        jobName(),
        leftProfile(),
        source.path,
        rightProfile(),
        target.path,
        jobMode(),
        jobInterval(),
        jobWatchLocal(),
        excludeMask(),
      );
      setJobName("");
      setSyncJobs(await api.syncJobs());
      props.onNotice("Sync job saved. It runs while Weave is open.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function runSyncJob(job: SyncJob) {
    setBusy(true);
    try {
      const result = await api.runSyncJob(job.id);
      props.onNotice(
        `Sync job ${job.name}: ${result.lastQueued} transfer${result.lastQueued === 1 ? "" : "s"} queued.`,
      );
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      try {
        setSyncJobs(await api.syncJobs());
      } catch (error) {
        props.onError(`Could not refresh sync jobs: ${errorMessage(error)}`);
      }
      setBusy(false);
    }
  }

  async function setSyncJobEnabled(job: SyncJob, enabled: boolean) {
    setBusy(true);
    try {
      await api.setSyncJobEnabled(job.id, enabled);
      setSyncJobs(await api.syncJobs());
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function deleteSyncJob(job: SyncJob) {
    if (!(await confirmAction(`Delete the saved sync job “${job.name}”?`)))
      return;
    setBusy(true);
    try {
      await api.deleteSyncJob(job.id);
      setSyncJobs(await api.syncJobs());
      props.onNotice("Sync job deleted.");
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function deleteMirrorExtras() {
    const plan = syncPreview();
    const source = left();
    const target = right();
    if (
      !plan ||
      !source ||
      !target ||
      !plan.deletes.length ||
      plan.copies.length ||
      plan.issues.length
    )
      return;
    const destination =
      syncMode() === "mirror-left-to-right" ? target.path : source.path;
    if (
      !(await confirmAction(
        `Delete ${plan.deletes.length} destination-only item${plan.deletes.length === 1 ? "" : "s"} from ${destination}? This cannot be undone.`,
      ))
    )
      return;
    setBusy(true);
    try {
      const count = await api.syncMirrorDelete(
        leftProfile(),
        source.path,
        rightProfile(),
        target.path,
        syncMode(),
        plan.deletes.map((item) => item.path),
        excludeMask(),
      );
      setSyncOpen(false);
      await refresh();
      props.onNotice(
        `Deleted ${count} destination-only item${count === 1 ? "" : "s"}.`,
      );
    } catch (error) {
      props.onError(errorMessage(error));
      setSyncOpen(false);
    } finally {
      setBusy(false);
    }
  }

  async function submitDialog(event: Event) {
    event.preventDefault();
    const current = dialog();
    if (!current) return;
    const value = dialogValue().trim();
    setBusy(true);
    try {
      if (current.kind === "copy")
        await enqueue(
          current.source,
          current.sourceSide,
          current.destinationSide,
          value,
        );
      if (current.kind === "mkdir") {
        const path = listingFor(current.side)?.path;
        if (!path)
          throw new Error(
            "The folder is no longer open. Refresh and try again.",
          );
        if (profileFor(current.side))
          await api.sftpMkdir(profileFor(current.side)!, path, value);
        else await api.mkdir(path, value);
        await navigate(current.side, path);
        props.onNotice("Folder created.");
      }
      if (current.kind === "rename") {
        if (profileFor(current.side))
          await api.sftpRename(
            profileFor(current.side)!,
            current.entry.path,
            value,
          );
        else await api.rename(current.entry.path, value);
        await refresh();
        props.onNotice("Renamed successfully.");
      }
      if (current.kind === "delete") {
        if (profileFor(current.side))
          await api.sftpDelete(
            profileFor(current.side)!,
            current.entry.path,
            current.entry.isDir,
          );
        else await api.delete(current.entry.path);
        await refresh();
        props.onNotice("Deleted successfully.");
      }
      if (current.kind === "delete-many") {
        let deleted = 0;
        for (const entry of current.entries) {
          try {
            if (profileFor(current.side))
              await api.sftpDelete(
                profileFor(current.side)!,
                entry.path,
                entry.isDir,
              );
            else await api.delete(entry.path);
          } catch (error) {
            await refresh();
            throw new Error(
              `Deleted ${deleted} of ${current.entries.length} items. Could not delete “${entry.name}”: ${errorMessage(error)}`,
            );
          }
          deleted += 1;
        }
        await refresh();
        props.onNotice(`${deleted} items deleted.`);
      }
      if (current.kind === "chmod") {
        if (!/^[0-7]{3,4}$/.test(value))
          throw new Error(
            "Use three or four octal digits, such as 644 or 0755.",
          );
        await api.sftpChmod(
          profileFor(current.side)!,
          current.entry.path,
          parseInt(value, 8),
        );
        await refresh();
        props.onNotice("Permissions updated.");
      }
      setDialog(undefined);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  function ordered(entries: FileEntry[]) {
    if (sort() === "name") return entries;
    return [...entries].sort(
      (a, b) =>
        Number(b.isDir) - Number(a.isDir) ||
        (sort() === "size"
          ? b.size - a.size
          : (b.modified ?? 0) - (a.modified ?? 0)),
    );
  }

  const dialogItemName = () => {
    const current = dialog();
    return current && "entry" in current ? current.entry.name : "";
  };
  const dialogDeleteCount = () => {
    const current = dialog();
    return current?.kind === "delete-many" ? current.entries.length : 0;
  };

  function pane(side: Side) {
    const listing = () => (side === "left" ? left() : right());
    const path = () => (side === "left" ? leftPath() : rightPath());
    const filter = () => (side === "left" ? leftFilter() : rightFilter());
    const setFilter = side === "left" ? setLeftFilter : setRightFilter;
    const visible = () =>
      ordered(
        (listing()?.entries ?? []).filter((entry) =>
          entry.name.toLocaleLowerCase().includes(filter().toLocaleLowerCase()),
        ),
      );
    function selectEntry(entry: FileEntry, event: MouseEvent | KeyboardEvent) {
      const next = selectFileEntry(
        selection(),
        selectionAnchor(),
        side,
        visible(),
        entry,
        {
          shift: event.shiftKey,
          additive: event.metaKey || event.ctrlKey,
        },
      );
      setSelection(next.selection);
      setSelectionAnchor(next.anchor);
    }
    const setPath = side === "left" ? setLeftPath : setRightPath;
    return (
      <section
        class="file-pane panel"
        data-pane-side={side}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => {
          event.preventDefault();
          const raw = event.dataTransfer?.getData("text/x-weave-path");
          if (raw) {
            try {
              const source = JSON.parse(raw) as {
                paths?: unknown;
                side?: unknown;
              };
              if (
                (source.side === "left" || source.side === "right") &&
                source.side !== side &&
                Array.isArray(source.paths) &&
                source.paths.every((path) => typeof path === "string")
              )
                void copyItems(source.paths, source.side, side);
            } catch {
              props.onError(
                "This dragged item could not be read. Try copying it with the arrow button.",
              );
            }
          }
        }}
      >
        <div class="pane-head">
          <div class="pane-identity">
            <span class="drive-icon">
              {profileFor(side) ? (
                <Server size={18} />
              ) : (
                <HardDrive size={18} />
              )}
            </span>
            <div>
              <strong>{profileDetails(side)?.name ?? "Local computer"}</strong>
              <small>
                {profileFor(side)
                  ? profileDetails(side)?.protocol === "ssh"
                    ? "SFTP · shared SSH session"
                    : protocolLabel(profileDetails(side)?.protocol)
                  : "Local files"}
              </small>
            </div>
          </div>
          <select
            class="location-select"
            aria-label={`${side} pane location`}
            value={profileFor(side) ?? ""}
            onChange={(event) => {
              const id = event.currentTarget.value || null;
              void navigate(side, id ? "." : props.home, id);
            }}
          >
            <option value="">This computer</option>
            <For each={props.profiles}>
              {(profile) => (
                <option value={profile.id}>
                  {profile.name} · {protocolLabel(profile.protocol)}
                </option>
              )}
            </For>
          </select>
          <button
            class="mini-button"
            title="New folder"
            onClick={() => createFolder(side)}
          >
            <FolderPlus size={17} />
          </button>
          <button
            class="mini-button"
            title="Refresh"
            onClick={() => navigate(side, listing()?.path ?? props.home)}
          >
            <RefreshCw size={16} />
          </button>
          <button
            class="mini-button"
            title={
              listing() &&
              bookmarkFor(bookmarks(), {
                path: listing()!.path,
                profileId: profileFor(side),
              })
                ? "Remove folder bookmark"
                : "Bookmark this folder"
            }
            aria-label="Toggle folder bookmark"
            disabled={!listing()}
            onClick={() => toggleBookmark(side)}
          >
            <Star
              size={16}
              fill={
                listing() &&
                bookmarkFor(bookmarks(), {
                  path: listing()!.path,
                  profileId: profileFor(side),
                })
                  ? "currentColor"
                  : "none"
              }
            />
          </button>
        </div>
        <div class="path-bar">
          <button
            class="mini-button"
            title="Parent folder"
            disabled={!listing()?.parent}
            onClick={() =>
              listing()?.parent && navigate(side, listing()!.parent!)
            }
          >
            <ArrowLeft size={16} />
          </button>
          <input
            value={path()}
            onInput={(event) => setPath(event.currentTarget.value)}
            onKeyDown={(event) =>
              event.key === "Enter" && navigate(side, path())
            }
            aria-label={`${side} pane path`}
          />
          <button
            class="mini-button"
            title="Go to folder"
            onClick={() => navigate(side, path())}
          >
            <ChevronRight size={16} />
          </button>
        </div>
        <Show
          when={bookmarks().some(
            (bookmark) => bookmark.profileId === profileFor(side),
          )}
        >
          <select
            class="location-select"
            aria-label={`${side} pane bookmarked folders`}
            value=""
            onChange={(event) => {
              const bookmark = bookmarks().find(
                (item) => item.id === event.currentTarget.value,
              );
              if (bookmark)
                void navigate(side, bookmark.path, bookmark.profileId);
              event.currentTarget.value = "";
            }}
          >
            <option value="">Bookmarked folders</option>
            <For
              each={bookmarks().filter(
                (bookmark) => bookmark.profileId === profileFor(side),
              )}
            >
              {(bookmark) => (
                <option value={bookmark.id}>
                  {bookmark.name} · {bookmark.path}
                </option>
              )}
            </For>
          </select>
        </Show>
        <div class="file-filter-bar">
          <input
            aria-label={`${side} pane filter`}
            value={filter()}
            onInput={(event) => setFilter(event.currentTarget.value)}
            placeholder="Filter this folder…"
          />
          <button
            class={view() === "list" ? "active" : ""}
            title="List view"
            onClick={() => setView("list")}
          >
            List
          </button>
          <button
            class={view() === "grid" ? "active" : ""}
            title="Grid view"
            onClick={() => setView("grid")}
          >
            Grid
          </button>
        </div>
        <Show when={view() === "list"}>
          <div class="file-table-head">
            <button onClick={() => setSort("name")}>Name</button>
            <button onClick={() => setSort("size")}>Size</button>
            <button onClick={() => setSort("modified")}>Modified</button>
          </div>
        </Show>
        <div
          class={`file-list ${view() === "grid" ? "file-grid" : ""}`}
          role="listbox"
          aria-label={`${side} files`}
          aria-multiselectable="true"
        >
          <For
            each={visible()}
            fallback={
              <div class="empty-small">
                {filter() ? "No matching files" : "This folder is empty"}
              </div>
            }
          >
            {(entry) => (
              <div
                class={`file-row ${selection()?.side === side && selection()?.entries.some((item) => item.path === entry.path) ? "selected" : ""}`}
                role="option"
                aria-selected={
                  selection()?.side === side &&
                  selection()?.entries.some((item) => item.path === entry.path)
                }
                tabIndex={0}
                draggable
                onDragStart={(event) => {
                  const current = selection();
                  const selectedPaths =
                    current?.side === side &&
                    current.entries.some((item) => item.path === entry.path)
                      ? current.entries.map((item) => item.path)
                      : [entry.path];
                  event.dataTransfer?.setData(
                    "text/x-weave-path",
                    JSON.stringify({ paths: selectedPaths, side }),
                  );
                }}
                onClick={(event) => selectEntry(entry, event)}
                onKeyDown={(event) => {
                  if (event.key === " ") {
                    event.preventDefault();
                    selectEntry(entry, event);
                  }
                  if (event.key === "Enter") {
                    event.preventDefault();
                    if (entry.isDir) void navigate(side, entry.path);
                    else void openText(side, entry);
                  }
                }}
                onDblClick={() =>
                  entry.isDir
                    ? void navigate(side, entry.path)
                    : void openText(side, entry)
                }
              >
                <span class="file-name">
                  {entry.isDir ? (
                    <Folder size={17} fill="#b0a2f4" stroke="#b0a2f4" />
                  ) : entry.name.match(/\.(md|txt|json|ts|js|py|rs)$/i) ? (
                    <FileText size={17} />
                  ) : (
                    <File size={17} />
                  )}
                  <span class="truncate">{entry.name}</span>
                </span>
                <span>{entry.isDir ? "—" : size(entry.size)}</span>
                <span>{date(entry.modified)}</span>
              </div>
            )}
          </For>
        </div>
        <div class="pane-footer">
          <span>
            {visible().length} of {listing()?.entries.length ?? 0} items
            {selection()?.side === side && selection()!.entries.length > 0
              ? ` · ${selection()!.entries.length} selected`
              : ""}
          </span>
          <Show
            when={!profileFor(side) || profileDetails(side)?.protocol === "ssh"}
          >
            <button
              onClick={() =>
                profileDetails(side)
                  ? props.onConnectHost(profileDetails(side)!, listing()?.path)
                  : props.onOpenTerminal(listing()?.path ?? props.home)
              }
            >
              {profileFor(side)
                ? "Open SSH terminal here"
                : "Open terminal here"}{" "}
              <ChevronRight size={14} />
            </button>
          </Show>
        </div>
      </section>
    );
  }

  return (
    <div class="files-view">
      <div class="page-heading">
        <div>
          <span class="eyebrow">FILE TRANSFER</span>
          <h1>Files</h1>
          <p class="muted page-description">
            Local folders, SFTP, FTP, FTPS, WebDAV, S3, Azure Blob and cloud
            drives side by side. {isMac ? "Command" : "Ctrl"}-click or
            Shift-click to select several items; transfers run in the activity
            queue.
          </p>
        </div>
        <div class="toolbar-actions">
          <Show when={externalEdits().length > 0}>
            <button
              class="ghost-button"
              onClick={() => setExternalEdit(externalEdits()[0])}
            >
              Saved edits ({externalEdits().length})
            </button>
          </Show>
          <button
            class="ghost-button"
            disabled={!left() || !right() || busy()}
            onClick={() => void compareFolders(syncMode())}
          >
            Compare folders
          </button>
          <button class="ghost-button" onClick={() => void openSyncJobs()}>
            Sync jobs
          </button>
          <button
            class={
              linkPanes() ? "ghost-button broadcast-active" : "ghost-button"
            }
            aria-pressed={linkPanes()}
            title="Open the same subfolder in both panes as you browse"
            onClick={() => setLinkPanes(!linkPanes())}
          >
            {linkPanes() ? "Panes linked" : "Link panes"}
          </button>
          <MenuButton
            label="Tools"
            title="Copy plans, vaults, network shares, search, scripts and file servers"
          >
            <button
              class="menu-item"
              disabled={busy()}
              onClick={() => void chooseBatchPlan()}
            >
              Import copy plan
            </button>
            <button class="menu-item" onClick={() => setVaultOpen(true)}>
              Cryptomator vault
            </button>
            <button
              class="menu-item"
              title="Connect to SMB or NFS network shares"
              onClick={() => setFileTool({ kind: "shares" })}
            >
              Network share…
            </button>
            <button
              class="menu-item"
              title={`Find files by name in the selected pane's folder (${shortcutLabel(fixedShortcuts.findFiles)})`}
              onClick={() => openFind()}
            >
              Find…
            </button>
            <button
              class="menu-item"
              title="Run a script of file commands (open, get, put, sync…)"
              onClick={() =>
                setFileTool({
                  kind: "script",
                  localDir:
                    (!leftProfile()
                      ? left()?.path
                      : !rightProfile()
                        ? right()?.path
                        : undefined) ?? "",
                })
              }
            >
              Script…
            </button>
            <button
              class="menu-item"
              title="Share the local folder over TFTP or HTTP"
              disabled={!!leftProfile() && !!rightProfile()}
              onClick={() =>
                setFileTool({
                  kind: "servers",
                  folder:
                    (!leftProfile()
                      ? left()?.path
                      : !rightProfile()
                        ? right()?.path
                        : undefined) ?? "",
                })
              }
            >
              Serve…
            </button>
          </MenuButton>
          <span class="toolbar-divider" />
          <button
            class="ghost-button"
            disabled={!selected() || selected()?.entry.isDir}
            title={`Preview images, PDFs and text (${shortcutLabel(fixedShortcuts.preview)})`}
            onClick={() => openPreview()}
          >
            <Eye size={15} /> Preview
          </button>
          <button
            class="ghost-button"
            disabled={!selected() || selected()?.entry.isDir || busy()}
            onClick={() => {
              const item = selected();
              if (item) void openText(item.side, item.entry);
            }}
          >
            <FileText size={15} /> Open text
          </button>
          <button class="ghost-button" disabled={!selected()} onClick={rename}>
            <Pencil size={15} /> Rename
          </button>
          <MenuButton label="More" title="More actions for the selection">
            <button
              class="menu-item"
              disabled={!selected() || selected()?.entry.isDir || busy()}
              onClick={() => {
                const item = selected();
                if (item) void openExternal(item.side, item.entry);
              }}
            >
              <File size={15} /> Open in default app
            </button>
            <button
              class="menu-item"
              disabled={(() => {
                const item = selected();
                const protocol = profileDetails(item?.side ?? "left")?.protocol;
                return (
                  busy() ||
                  !item ||
                  !(
                    (protocol === "s3" && !item.entry.isDir) ||
                    protocol === "webdav"
                  )
                );
              })()}
              title="Create a link others can use to download (S3 files, or Nextcloud and ownCloud files and folders)"
              onClick={() => {
                const item = selected();
                const profileId = item && profileFor(item.side);
                const protocol = profileDetails(item?.side ?? "left")?.protocol;
                if (
                  item &&
                  profileId &&
                  (protocol === "s3" || protocol === "webdav")
                ) {
                  setShareTarget({
                    profileId,
                    path: item.entry.path,
                    protocol,
                  });
                  setShareUrl("");
                  setSharePassword("");
                }
              }}
            >
              <Copy size={15} /> Share link
            </button>
            <button
              class="menu-item"
              disabled={
                !selected() ||
                selected()!.entry.isDir ||
                profileDetails(selected()!.side)?.protocol !== "s3"
              }
              title="Content type, cache control, storage class, metadata and tags"
              onClick={() => {
                const item = selected();
                const profileId = item && profileFor(item.side);
                if (item && profileId)
                  setFileTool({
                    kind: "s3props",
                    profileId,
                    path: item.entry.path,
                    name: item.entry.name,
                  });
              }}
            >
              <Tags size={15} /> S3 properties…
            </button>
            <button
              class="menu-item"
              disabled={!selected() || !profileFor(selected()?.side ?? "left")}
              onClick={changePermissions}
            >
              <Shield size={15} /> Permissions
            </button>
            <button
              class="menu-item"
              disabled={!listingFor(selection()?.side ?? "left")}
              title="Create a symbolic link in this folder"
              onClick={() => {
                const side = selection()?.side ?? "left";
                setFileTool({
                  kind: "symlink",
                  profileId: profileFor(side),
                  folder: listingFor(side)?.path ?? "",
                  target: selected()?.entry.path ?? "",
                });
              }}
            >
              <Link2 size={15} /> Link
            </button>
            <button
              class="menu-item"
              disabled={!selection()?.entries.length}
              title="Change the owner and group of the selected items"
              onClick={() => {
                const current = selection();
                if (current)
                  setFileTool({
                    kind: "owner",
                    profileId: profileFor(current.side),
                    paths: current.entries.map((entry) => entry.path),
                  });
              }}
            >
              <Users size={15} /> Owner
            </button>
            <button
              class="menu-item"
              disabled={!listingFor(selection()?.side ?? "left")}
              title="Run a shell command on the selected items"
              onClick={() => {
                const side = selection()?.side ?? "left";
                setFileTool({
                  kind: "command",
                  profileId: profileFor(side),
                  cwd: listingFor(side)?.path ?? "",
                  paths: selection()?.entries.map((entry) => entry.path) ?? [],
                });
              }}
            >
              <SquareTerminal size={15} /> Command
            </button>
            <button
              class="menu-item"
              disabled={
                !selection()?.entries.length ||
                !canUseShell(selection()?.side ?? "left")
              }
              title={`Pack the selected items into a zip or tar archive (${thisComputer} and SSH)`}
              onClick={() => {
                const current = selection();
                if (current)
                  setFileTool({
                    kind: "archive",
                    profileId: profileFor(current.side),
                    cwd: listingFor(current.side)?.path ?? "",
                    names: current.entries.map((entry) => entry.name),
                  });
              }}
            >
              <Archive size={15} /> Compress…
            </button>
            <button
              class="menu-item"
              disabled={
                busy() ||
                !selected() ||
                selected()!.entry.isDir ||
                !isArchive(selected()!.entry.name) ||
                !canUseShell(selected()!.side)
              }
              title={`Unpack into a new folder next to the archive (${thisComputer} and SSH)`}
              onClick={() => void extractSelected()}
            >
              <PackageOpen size={15} /> Extract here
            </button>
            <button
              class="menu-item"
              disabled={!selection()?.entries.length}
              title="Copy the full paths of the selected items"
              onClick={() => void copyPaths()}
            >
              <ClipboardCopy size={15} /> Copy path
            </button>
            <button
              class="menu-item"
              disabled={
                busy() ||
                !["webdav", "ftp"].includes(
                  profileDetails(selection()?.side ?? "left")?.protocol ?? "",
                )
              }
              title={`Show this WebDAV or FTP connection as a drive in ${fileManager} (${systemName} asks for the password)`}
              onClick={() => void mountInFinder()}
            >
              <HardDrive size={15} /> Mount in {fileManager}
            </button>
            <button
              class="menu-item"
              disabled={
                !selected()?.entry.isDir ||
                !!profileFor(selected()?.side ?? "left")
              }
              onClick={() =>
                selected() && props.onOpenAsRepo(selected()!.entry.path)
              }
            >
              Open as Git
            </button>
          </MenuButton>
          <button
            class="ghost-button danger-text"
            disabled={!selection()?.entries.length}
            onClick={remove}
          >
            <Trash2 size={15} /> Delete
          </button>
        </div>
      </div>
      <div class="dual-pane">
        {pane("left")}
        <div class="transfer-controls">
          <button
            class="transfer-arrow"
            title="Copy selected items to right; matching names are kept as separate copies"
            disabled={
              busy() ||
              selection()?.side !== "left" ||
              !selection()?.entries.length
            }
            onClick={() => void copySelection("left", "right")}
          >
            <ArrowRight size={20} />
          </button>
          <button
            class="transfer-arrow"
            title="Copy selected items to left; matching names are kept as separate copies"
            disabled={
              busy() ||
              selection()?.side !== "right" ||
              !selection()?.entries.length
            }
            onClick={() => void copySelection("right", "left")}
          >
            <ArrowLeft size={20} />
          </button>
          <Copy size={16} class="transfer-symbol" />
          <button
            class="ghost-button direct-transfer-button"
            title={`Direct copy with SCP or rsync between ${thisComputer} and an SSH host`}
            aria-label="Direct copy with SCP or rsync"
            disabled={busy() || !canDirectTransfer()}
            onClick={() => {
              if (directBetweenHosts()) setDirectMethod("scp");
              setDirectOpen(true);
            }}
          >
            SSH
          </button>
          <button
            class="ghost-button direct-transfer-button"
            title="Copy the selected file directly between two plain FTP servers"
            aria-label="Direct FXP copy between FTP servers"
            disabled={busy() || !canFxpTransfer()}
            onClick={() => setFxpOpen(true)}
          >
            FXP
          </button>
        </div>
        {pane("right")}
      </div>
      <Show when={directOpen()}>
        <div class="overlay" onClick={() => !busy() && setDirectOpen(false)}>
          <div
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Direct SSH transfer"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">FILES</span>
                <h2>Direct SSH transfer</h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                disabled={busy()}
                onClick={() => setDirectOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <p class="muted">
              Copy the selection between {thisComputer} and the SSH host with
              the system's own tools. The copies run in the activity queue,
              where they can be paused and retried. rsync continues partial
              files.
            </p>
            <Show when={directBetweenHosts()}>
              <p class="muted">
                Between two SSH hosts, SCP streams the data through{" "}
                {thisComputer}, so neither host needs access to the other.
              </p>
            </Show>
            <label class="git-action-field">
              Method
              <select
                value={directMethod()}
                onChange={(event) =>
                  setDirectMethod(event.currentTarget.value as "scp" | "rsync")
                }
              >
                <option value="scp">SCP</option>
                <option value="rsync" disabled={directBetweenHosts()}>
                  rsync
                </option>
              </select>
            </label>
            <label class="git-action-field">
              If the name exists
              <select
                value={directConflict()}
                onChange={(event) =>
                  setDirectConflict(event.currentTarget.value)
                }
              >
                <option value="rename">Keep both</option>
                <option value="overwrite">Replace</option>
                <option value="newer">Replace if newer</option>
                <option value="skip">Skip</option>
              </select>
            </label>
            <div class="dialog-actions">
              <span class="spacer" />
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() => setDirectOpen(false)}
              >
                Cancel
              </button>
              <button
                class="primary-button"
                disabled={busy()}
                onClick={() => void runDirectTransfer()}
              >
                {busy() ? "Queuing…" : "Add to queue"}
              </button>
            </div>
          </div>
        </div>
      </Show>
      <Show when={fxpOpen()}>
        <div class="overlay" onClick={() => !busy() && setFxpOpen(false)}>
          <div
            class="dialog profile-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Direct FTP transfer"
            onClick={(event) => event.stopPropagation()}
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">FILES</span>
                <h2>Direct FTP transfer</h2>
              </div>
              <button
                class="icon-button"
                aria-label="Close"
                disabled={busy()}
                onClick={() => setFxpOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <p class="muted">
              FXP sends this file directly between the two FTP servers. Both
              servers must allow server-to-server transfers, and an existing
              destination file will not be replaced.
            </p>
            <Show
              when={fxpSecure()}
              fallback={
                <p class="muted">
                  These are plain FTP servers, so nothing is encrypted.
                </p>
              }
            >
              <p class="muted">
                Sign-in and commands are encrypted. The file data is encrypted
                too when the servers support it (SSCN).
              </p>
              <label class="sync-job-watch">
                <input
                  type="checkbox"
                  checked={fxpClearData()}
                  disabled={busy()}
                  onChange={(event) =>
                    setFxpClearData(event.currentTarget.checked)
                  }
                />
                Allow an unencrypted copy if the servers cannot encrypt it
              </label>
            </Show>
            <div class="dialog-actions">
              <span class="spacer" />
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() => setFxpOpen(false)}
              >
                Cancel
              </button>
              <button
                class="primary-button"
                disabled={busy()}
                onClick={() => void runFxpTransfer()}
              >
                {busy() ? "Copying…" : "Start FXP"}
              </button>
            </div>
          </div>
        </div>
      </Show>
      <div class="files-tip">
        Weave asks for passwords and verification codes when a server needs
        them; save a password in the connection to have it filled in.
      </div>
      <Show when={dialog()}>
        {(current) => (
          <div class="overlay" onClick={() => setDialog(undefined)}>
            <div
              class="dialog profile-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="File operation"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">FILES</span>
                  <h2>
                    {current().kind === "copy"
                      ? "File already exists"
                      : current().kind === "mkdir"
                        ? "New folder"
                        : current().kind === "rename"
                          ? "Rename item"
                          : current().kind === "delete-many"
                            ? "Delete items"
                            : current().kind === "delete"
                              ? "Delete item"
                              : "Permissions"}
                  </h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setDialog(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <Show
                when={
                  current().kind === "delete" ||
                  current().kind === "delete-many"
                }
              >
                <p class="muted">
                  {current().kind === "delete-many"
                    ? `Delete ${dialogDeleteCount()} selected items and their contents? This cannot be undone.`
                    : `Delete “${dialogItemName()}” and its contents? This cannot be undone.`}
                </p>
              </Show>
              <form onSubmit={submitDialog}>
                <Show when={current().kind === "copy"}>
                  <label class="git-action-field">
                    If the destination has the same name
                    <select
                      value={dialogValue()}
                      onChange={(event) =>
                        setDialogValue(event.currentTarget.value)
                      }
                    >
                      <option value="rename">Keep both</option>
                      <option value="overwrite">Replace destination</option>
                      <option value="newer">Replace only if newer</option>
                      <option value="skip">Skip this item</option>
                    </select>
                  </label>
                </Show>
                <Show
                  when={["mkdir", "rename", "chmod"].includes(current().kind)}
                >
                  <label class="git-action-field">
                    {current().kind === "chmod" ? "Octal permissions" : "Name"}
                    <input
                      autofocus
                      required
                      value={dialogValue()}
                      onInput={(event) =>
                        setDialogValue(event.currentTarget.value)
                      }
                    />
                  </label>
                </Show>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => setDialog(undefined)}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    class={
                      current().kind === "delete" ||
                      current().kind === "delete-many"
                        ? "ghost-button danger-text"
                        : "primary-button"
                    }
                    disabled={busy()}
                  >
                    {current().kind === "delete"
                      ? "Delete"
                      : current().kind === "copy"
                        ? "Add to queue"
                        : "Save"}
                  </button>
                </div>
              </form>
            </div>
          </div>
        )}
      </Show>
      <Show when={editing()}>
        {(editor) => (
          <div class="overlay">
            <div
              class="dialog editor-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Text file editor"
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">
                    {editor().profileId
                      ? "REMOTE TEXT FILE"
                      : "LOCAL TEXT FILE"}
                  </span>
                  <h2>{editor().path.split("/").pop()}</h2>
                  <p class="muted">{editor().path}</p>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close text editor"
                  title="Close editor"
                  disabled={busy() || editor().content !== editor().original}
                  onClick={() => setEditing(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <textarea
                aria-label="File contents"
                spellcheck={false}
                disabled={busy()}
                value={editor().content}
                onInput={(event) =>
                  setEditing({
                    ...editor(),
                    content: event.currentTarget.value,
                  })
                }
                onKeyDown={(event) => {
                  if (
                    (event.metaKey || event.ctrlKey) &&
                    event.key.toLowerCase() === "s"
                  ) {
                    event.preventDefault();
                    void saveText();
                  }
                }}
              />
              <div class="dialog-actions">
                <span class="muted">
                  {editor().content === editor().original
                    ? "No unsaved changes"
                    : "Unsaved changes"}
                </span>
                <span class="spacer" />
                <button
                  class="ghost-button"
                  disabled={busy()}
                  onClick={() => setEditing(undefined)}
                >
                  {editor().content === editor().original
                    ? "Close"
                    : "Discard changes"}
                </button>
                <button
                  class="primary-button"
                  disabled={busy() || editor().content === editor().original}
                  onClick={() => void saveText()}
                >
                  Save file
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={batchPreview()}>
        {(plan) => (
          <div class="overlay">
            <div
              class="dialog batch-plan-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Copy plan preview"
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">COPY PLAN</span>
                  <h2>Review {plan().transfers.length} transfers</h2>
                  <p class="muted">{batchPath()}</p>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close copy plan"
                  disabled={busy()}
                  onClick={() => setBatchPreview(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <p class="muted">
                Each row uses the selected conflict rule. The entire plan is
                added to Activity only after you confirm.
              </p>
              <div class="batch-plan-list">
                <For each={plan().transfers}>
                  {(item, index) => (
                    <div class="batch-plan-row">
                      <strong>
                        {index() + 1}.{" "}
                        {item.sourcePath.split("/").pop() || item.sourcePath}
                      </strong>
                      <span>
                        {props.profiles.find(
                          (profile) => profile.id === item.sourceProfile,
                        )?.name ?? "This computer"}
                        : {item.sourcePath}
                      </span>
                      <span>
                        →{" "}
                        {props.profiles.find(
                          (profile) => profile.id === item.destinationProfile,
                        )?.name ?? "This computer"}
                        : {item.destinationDir}
                      </span>
                      <small>
                        {item.conflict === "rename"
                          ? "Keep both"
                          : item.conflict === "newer"
                            ? "Replace only if newer"
                            : item.conflict === "skip"
                              ? "Skip existing"
                              : "Replace existing"}
                      </small>
                    </div>
                  )}
                </For>
              </div>
              <Show when={batchError()}>
                <p class="error-text" role="alert">
                  {batchError()}
                </p>
              </Show>
              <div class="dialog-actions">
                <button
                  class="ghost-button"
                  disabled={busy()}
                  onClick={() => void chooseBatchPlan()}
                >
                  Choose another file
                </button>
                <span class="spacer" />
                <button
                  class="ghost-button"
                  disabled={busy()}
                  onClick={() => setBatchPreview(undefined)}
                >
                  Cancel
                </button>
                <button
                  class="primary-button"
                  disabled={busy()}
                  onClick={() => void queueBatchPlan()}
                >
                  {busy() ? "Adding…" : "Add all to queue"}
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={fileTool()}>
        {(tool) => (
          <FileToolDialogs
            tool={tool()}
            onReveal={(path) => void reveal(selection()?.side ?? "left", path)}
            onOpenFolder={(path) =>
              void navigate(selection()?.side ?? "left", path, null)
            }
            onDone={(message) => {
              props.onNotice(message);
              void refresh();
            }}
            onClose={() => setFileTool(undefined)}
          />
        )}
      </Show>
      <Show when={vaultOpen()}>
        <CryptomatorDialog
          onNotice={props.onNotice}
          onClose={() => setVaultOpen(false)}
        />
      </Show>
      <Show when={shareTarget()}>
        {(target) => (
          <div class="overlay">
            <div
              class="dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Share link"
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">
                    {target().protocol === "s3"
                      ? "S3 FILE LINK"
                      : "PUBLIC LINK"}
                  </span>
                  <h2>Share {target().path.split("/").pop()}</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setShareTarget(undefined)}
                >
                  <X size={18} />
                </button>
              </div>
              <p class="muted">
                {target().protocol === "s3"
                  ? "Anyone with the link can download this file until it expires. Temporary credentials may expire sooner. Share it only with people you trust."
                  : "Creates a read-only public link on your Nextcloud or ownCloud server. You can remove it later in the server's sharing settings."}
              </p>
              <label class="field-label" for="share-lifetime">
                Link expires after
              </label>
              <Show
                when={target().protocol === "s3"}
                fallback={
                  <>
                    <select
                      id="share-lifetime"
                      value={shareDays()}
                      disabled={busy()}
                      onChange={(event) => {
                        setShareDays(Number(event.currentTarget.value));
                        setShareUrl("");
                      }}
                    >
                      <option value={1}>1 day</option>
                      <option value={7}>7 days</option>
                      <option value={30}>30 days</option>
                      <option value={0}>
                        Never (unless the server requires it)
                      </option>
                    </select>
                    <label class="field-label" for="share-password">
                      Link password (optional)
                    </label>
                    <input
                      id="share-password"
                      type="password"
                      autocomplete="new-password"
                      value={sharePassword()}
                      onInput={(event) => {
                        setSharePassword(event.currentTarget.value);
                        setShareUrl("");
                      }}
                    />
                  </>
                }
              >
                <select
                  id="share-lifetime"
                  value={shareLifetime()}
                  disabled={busy()}
                  onChange={(event) => {
                    setShareLifetime(Number(event.currentTarget.value));
                    setShareUrl("");
                  }}
                >
                  <option value={900}>15 minutes</option>
                  <option value={3600}>1 hour</option>
                  <option value={86400}>1 day</option>
                  <option value={604800}>7 days</option>
                </select>
              </Show>
              <Show when={shareUrl()}>
                <label class="field-label" for="share-url">
                  Download link
                </label>
                <input
                  id="share-url"
                  value={shareUrl()}
                  readOnly
                  onFocus={(event) => event.currentTarget.select()}
                />
              </Show>
              <div class="dialog-actions">
                <button
                  class="ghost-button"
                  onClick={() => setShareTarget(undefined)}
                >
                  Close
                </button>
                <span class="spacer" />
                <Show when={shareUrl()}>
                  <button
                    class="ghost-button"
                    onClick={() => void copyShareUrl()}
                  >
                    <Copy size={15} /> Copy link
                  </button>
                </Show>
                <button
                  class="primary-button"
                  disabled={busy()}
                  onClick={() => void createShareUrl()}
                >
                  {busy() ? "Creating…" : "Create link"}
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={externalEdit()}>
        {(session) => (
          <div class="overlay">
            <div
              class="dialog"
              role="dialog"
              aria-modal="true"
              aria-label="External file editor"
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">REMOTE EDITING COPY</span>
                  <h2>{session().remotePath.split("/").pop()}</h2>
                  <p class="muted">{session().remotePath}</p>
                </div>
              </div>
              <p class="muted">
                Save the file in its app, then choose Upload changes. Weave
                checks whether the remote file changed before replacing it. This
                copy stays available after Weave restarts until you discard it.
                The copy is stored as an ordinary file in Weave's app data
                folder.
              </p>
              <label class="profile-favorite">
                <input
                  type="checkbox"
                  checked={session().autoUpload}
                  disabled={busy()}
                  onChange={(event) => {
                    const enabled = event.currentTarget.checked;
                    void api
                      .setExternalEditAutoUpload(session().id, enabled)
                      .then((updated) => {
                        setExternalEdit(updated);
                        setExternalEdits((items) =>
                          items.map((item) =>
                            item.id === updated.id ? updated : item,
                          ),
                        );
                      })
                      .catch((error) => props.onError(errorMessage(error)));
                  }}
                />
                Upload automatically each time I save
              </label>
              <Show when={externalEdits().length > 1}>
                <label class="git-action-field">
                  Saved editing copies
                  <select
                    value={session().id}
                    onChange={(event) => {
                      const next = externalEdits().find(
                        (item) => item.id === event.currentTarget.value,
                      );
                      if (next) setExternalEdit(next);
                      setConfirmDiscardExternal(false);
                    }}
                  >
                    <For each={externalEdits()}>
                      {(item) => (
                        <option value={item.id}>{item.remotePath}</option>
                      )}
                    </For>
                  </select>
                </label>
              </Show>
              <div class="dialog-actions">
                <button
                  class="ghost-button"
                  disabled={busy()}
                  onClick={() =>
                    void openPath(session().localPath).catch((error) =>
                      props.onError(errorMessage(error)),
                    )
                  }
                >
                  Reopen copy
                </button>
                <span class="spacer" />
                <button
                  class="ghost-button"
                  disabled={busy()}
                  onClick={() => {
                    setExternalEdit(undefined);
                    setConfirmDiscardExternal(false);
                  }}
                >
                  Keep for later
                </button>
                <Show
                  when={confirmDiscardExternal()}
                  fallback={
                    <button
                      class="ghost-button danger-text"
                      disabled={busy()}
                      onClick={() => setConfirmDiscardExternal(true)}
                    >
                      Discard copy
                    </button>
                  }
                >
                  <button
                    class="ghost-button"
                    disabled={busy()}
                    onClick={() => setConfirmDiscardExternal(false)}
                  >
                    Keep copy
                  </button>
                  <button
                    class="ghost-button danger-text"
                    disabled={busy()}
                    onClick={() => void discardExternal()}
                  >
                    Delete local copy
                  </button>
                </Show>
                <button
                  class="primary-button"
                  disabled={busy()}
                  onClick={() => void uploadExternal()}
                >
                  Upload changes
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={syncOpen()}>
        <div class="overlay">
          <div
            class="dialog sync-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Compare and sync folders"
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">FOLDER SYNC</span>
                <h2>Compare and sync</h2>
                <p class="muted">
                  Preview copies before queuing them. Mirror modes also show
                  destination-only items for a separate, confirmed cleanup after
                  transfers finish.
                </p>
              </div>
              <button
                class="icon-button"
                aria-label="Close folder comparison"
                disabled={busy()}
                onClick={() => setSyncOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <label class="git-action-field">
              Direction
              <select
                value={syncMode()}
                disabled={busy()}
                onChange={(event) =>
                  void compareFolders(event.currentTarget.value as SyncMode)
                }
              >
                <option value="left-to-right">Left → right</option>
                <option value="right-to-left">Right → left</option>
                <option value="two-way">Two way, newer files win</option>
                <option value="mirror-left-to-right">
                  Mirror left → right
                </option>
                <option value="mirror-right-to-left">
                  Mirror right → left
                </option>
              </select>
            </label>
            <label class="git-action-field">
              Leave out names matching
              <input
                placeholder="*.tmp; node_modules; .DS_Store"
                value={excludeMask()}
                disabled={busy()}
                onInput={(event) => setExcludeMask(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void compareFolders(syncMode());
                }}
                onBlur={() => void compareFolders(syncMode())}
              />
            </label>
            <div class="sync-results">
              <Show when={syncPreview()} fallback={<p>Comparing folders…</p>}>
                {(plan) => (
                  <>
                    <strong>
                      {plan().copies.length} to copy · {plan().deletes.length}{" "}
                      extra at destination · {plan().issues.length} need review
                    </strong>
                    <For each={plan().copies}>
                      {(copy) => (
                        <div class="sync-result">
                          <span>{copy.sourcePath}</span>
                          <small>
                            {copy.reason} → {copy.destinationDir}
                          </small>
                        </div>
                      )}
                    </For>
                    <For each={plan().deletes}>
                      {(item) => (
                        <div class="sync-result sync-issue">
                          <span>{item.path}</span>
                          <small>
                            Destination-only item proposed for deletion
                          </small>
                        </div>
                      )}
                    </For>
                    <For each={plan().issues}>
                      {(issue) => (
                        <div class="sync-result sync-issue">{issue}</div>
                      )}
                    </For>
                  </>
                )}
              </Show>
            </div>
            <div class="dialog-actions">
              <span class="spacer" />
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() => setSyncOpen(false)}
              >
                Cancel
              </button>
              <button
                class="primary-button"
                disabled={busy() || !syncPreview()?.copies.length}
                onClick={() => void queueSync()}
              >
                Add copies to queue
              </button>
              <Show
                when={
                  syncMode().startsWith("mirror-") &&
                  syncPreview()?.deletes.length
                }
              >
                <button
                  class="ghost-button danger-text"
                  disabled={
                    busy() ||
                    !!syncPreview()?.copies.length ||
                    !!syncPreview()?.issues.length
                  }
                  onClick={() => void deleteMirrorExtras()}
                >
                  Delete destination extras
                </button>
              </Show>
            </div>
          </div>
        </div>
      </Show>
      <Show when={syncJobsOpen()}>
        <div class="overlay">
          <div
            class="dialog sync-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Scheduled sync jobs"
          >
            <div class="dialog-head">
              <div>
                <span class="eyebrow">AUTOMATIC SYNC</span>
                <h2>Sync jobs</h2>
                <p class="muted">
                  Saved jobs compare folders and queue copies while Weave is
                  open. They never delete destination files. Check Activity for
                  transfer results.
                </p>
              </div>
              <button
                class="icon-button"
                aria-label="Close sync jobs"
                disabled={busy()}
                onClick={() => setSyncJobsOpen(false)}
              >
                <X size={19} />
              </button>
            </div>
            <div class="sync-job-form">
              <strong>New job from the open folders</strong>
              <p class="muted">
                {left()?.path ?? "Open a left folder"} →{" "}
                {right()?.path ?? "Open a right folder"}
              </p>
              <label class="git-action-field">
                Name
                <input
                  value={jobName()}
                  maxlength={100}
                  placeholder="For example, NAS backup"
                  onInput={(event) => setJobName(event.currentTarget.value)}
                />
              </label>
              <div class="sync-job-fields">
                <label class="git-action-field">
                  Direction
                  <select
                    value={jobMode()}
                    onChange={(event) => {
                      setJobMode(event.currentTarget.value as SyncMode);
                      setJobWatchLocal(false);
                    }}
                  >
                    <option value="left-to-right">Left → right</option>
                    <option value="right-to-left">Right → left</option>
                    <option value="two-way">Two way, newer files win</option>
                    <option value="mirror-left-to-right">
                      Mirror left → right (also deletes extras)
                    </option>
                    <option value="mirror-right-to-left">
                      Mirror right → left (also deletes extras)
                    </option>
                  </select>
                </label>
                <label class="git-action-field">
                  Every
                  <select
                    value={jobInterval()}
                    onChange={(event) =>
                      setJobInterval(Number(event.currentTarget.value))
                    }
                  >
                    <option value={15}>15 minutes</option>
                    <option value={60}>1 hour</option>
                    <option value={360}>6 hours</option>
                    <option value={1440}>1 day</option>
                    <option value={10080}>7 days</option>
                  </select>
                </label>
              </div>
              <label class="git-action-field">
                Leave out names matching
                <input
                  placeholder="*.tmp; node_modules; .DS_Store"
                  value={excludeMask()}
                  onInput={(event) => setExcludeMask(event.currentTarget.value)}
                />
              </label>
              <label class="sync-job-watch">
                <input
                  type="checkbox"
                  checked={jobWatchLocal()}
                  disabled={!canWatchJobSource()}
                  onChange={(event) =>
                    setJobWatchLocal(event.currentTarget.checked)
                  }
                />
                {jobSourceIsLocal()
                  ? "Also run when the source folder changes (checks every 30 seconds)"
                  : "Also run when the source folder changes (checks the server every 2 minutes)"}
              </label>
              <button
                class="primary-button"
                disabled={busy() || !left() || !right() || !jobName().trim()}
                onClick={() => void saveSyncJob()}
              >
                Save job
              </button>
            </div>
            <div class="sync-job-list">
              <strong>Saved jobs</strong>
              <Show
                when={syncJobs().length}
                fallback={<p class="muted">No saved sync jobs yet.</p>}
              >
                <For each={syncJobs()}>
                  {(job) => (
                    <div class="sync-job-row">
                      <div>
                        <strong>{job.name}</strong>
                        <small>
                          {job.leftPath} → {job.rightPath} · every{" "}
                          {job.intervalMinutes} minutes
                          {job.watchLocal ? " · runs on source changes" : ""}
                        </small>
                        <small>
                          {job.running
                            ? "Running comparison…"
                            : job.enabled
                              ? `Next check ${new Date(job.nextRun * 1000).toLocaleString()}`
                              : "Paused"}
                          {job.lastRun
                            ? ` · last check queued ${job.lastQueued}`
                            : ""}
                        </small>
                        <Show when={job.lastError}>
                          <small class="danger-text">{job.lastError}</small>
                        </Show>
                      </div>
                      <div class="sync-job-actions">
                        <button
                          class="ghost-button"
                          disabled={busy() || job.running}
                          onClick={() => void runSyncJob(job)}
                        >
                          Run now
                        </button>
                        <button
                          class="ghost-button"
                          disabled={busy()}
                          onClick={() =>
                            void setSyncJobEnabled(job, !job.enabled)
                          }
                        >
                          {job.enabled ? "Pause" : "Resume"}
                        </button>
                        <button
                          class="ghost-button danger-text"
                          disabled={busy() || job.running}
                          onClick={() => void deleteSyncJob(job)}
                        >
                          Delete
                        </button>
                      </div>
                    </div>
                  )}
                </For>
              </Show>
            </div>
            <div class="dialog-actions">
              <span class="spacer" />
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() => void openSyncJobs()}
              >
                Refresh
              </button>
              <button
                class="ghost-button"
                disabled={busy()}
                onClick={() => setSyncJobsOpen(false)}
              >
                Close
              </button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
