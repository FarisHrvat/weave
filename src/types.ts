import type { RemoteConnection } from "./remoteConnections";
export type KnownHost = {
  hosts: string;
  keyType: string;
  fingerprint: string;
  marker: string | null;
  hashed: boolean;
};

export type SshPublicKey = {
  name: string;
  publicPath: string;
  keyType: string;
  fingerprint: string;
  comment: string;
  hasPrivate: boolean;
};

export type Profile = {
  id: string;
  name: string;
  host: string;
  protocol:
    | "ssh"
    | "ftp"
    | "ftps"
    | "ftps-implicit"
    | "webdav"
    | "s3"
    | "azure"
    | "dropbox"
    | "gdrive"
    | "onedrive";
  port: number;
  username: string;
  keyPath: string | null;
  jumpHost: string | null;
  color: string;
  favorite: boolean;
  tlsPinSha256: string | null;
  s3Region: string | null;
};
export type FileZillaImportReport = { imported: number; skipped: number };
export type SessionImportReport = FileZillaImportReport & {
  connections: {
    kind: "telnet" | "rdp" | "vnc";
    name: string;
    host: string;
    port: number;
    username?: string;
  }[];
};

export type SshTunnel = {
  id: string;
  profileId: string;
  profileName: string;
  kind: "local" | "remote" | "dynamic";
  listenPort: number;
  targetHost: string | null;
  targetPort: number | null;
};

export type TailnetPeer = {
  name: string;
  dnsName: string;
  ip: string;
  online: boolean;
};
export type TailnetStatus = {
  installed: boolean;
  state: string;
  deviceName: string;
  peers: TailnetPeer[];
  installUrl: string;
};
export type VpnService = {
  id: string;
  name: string;
  kind: string;
  state: string;
  enabled: boolean;
};
export type VpnCommandProfile = {
  id: string;
  name: string;
  executable: string;
  connectArgs: string[];
  disconnectArgs: string[];
  statusArgs: string[];
  connectedText: string;
};
export type VpnCommandStatus = {
  profile: VpnCommandProfile;
  state: string;
};

export type Repo = { path: string; name: string };
export type FileEntry = {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  modified: number | null;
  readonly: boolean;
};
export type FileListing = {
  path: string;
  parent: string | null;
  entries: FileEntry[];
};
export type TextDocument = { content: string; sha256: string };
export type ExternalEditInfo = {
  id: string;
  localPath: string;
  remotePath: string;
  changed: boolean;
  warning: string | null;
  autoUpload: boolean;
};
export type GitFile = {
  path: string;
  staged: boolean;
  changed: boolean;
  status: string;
};
export type GitConflict = {
  file: string;
  base: string | null;
  ours: string | null;
  theirs: string | null;
};
export type GitIdentity = {
  localName: string | null;
  localEmail: string | null;
  globalName: string | null;
  globalEmail: string | null;
};
export type ForgeInfo = {
  provider: "GitHub" | "GitLab";
  host: string;
  project: string;
  connected: boolean;
  url: string;
};
export type ForgeItem = {
  number: number;
  title: string;
  state: string;
  url: string;
  detail: string;
};
export type ForgePage = { items: ForgeItem[]; hasMore: boolean };
export type ForgeComment = {
  id: number;
  author: string;
  body: string;
  createdAt: string;
};
export type ForgeCommentsPage = { items: ForgeComment[]; hasMore: boolean };
export type ForgeReviewThread = {
  id: string;
  providerId: string | null;
  path: string;
  line: number | null;
  side: string;
  resolved: boolean;
  comments: ForgeComment[];
};
export type ForgeReviewThreadsPage = {
  items: ForgeReviewThread[];
  hasMore: boolean;
};
export type ForgeReviewFile = {
  oldPath: string;
  newPath: string;
  patch: string;
};
export type ForgeReviewFilesPage = {
  items: ForgeReviewFile[];
  hasMore: boolean;
};
export type ForgeAccount = {
  provider: "GitHub" | "GitLab";
  connected: boolean;
};
export type ForgeRepository = {
  name: string;
  httpsUrl: string;
  sshUrl: string;
  private: boolean;
};
export type Commit = {
  shortId: string;
  id: string;
  subject: string;
  author: string;
  relativeDate: string;
  refs: string;
  graph: string;
};
export type BlameLine = {
  number: number;
  commit: string;
  author: string;
  summary: string;
  content: string;
};
export type BlamePage = {
  lines: BlameLine[];
  hasMore: boolean;
};
export type LineHistoryEntry = {
  commit: Commit;
  patch: string;
};
export type GitLfsInfo = {
  version: string;
  trackedPatterns: string;
  trackedFiles: string[];
};
export type LfsLock = {
  id: string;
  path: string;
  owner: string;
  lockedAt: string;
  ours: boolean;
};
export type LfsMigrationResult = {
  recoveryBranch: string;
  info: GitLfsInfo;
};
export type GitSnapshot = {
  path: string;
  name: string;
  branch: string;
  branches: string[];
  files: GitFile[];
  commits: Commit[];
  ahead: number;
  behind: number;
  remote: string | null;
  tags: string[];
  stashes: string[];
  remotes: string[];
  remoteUrls: { name: string; url: string }[];
  remoteBranches: string[];
};
export type ExternalInfo = {
  provider: string;
  organization: string;
  repository: string;
  connected: boolean;
  url: string;
};
export type ExternalItem = {
  number: number;
  title: string;
  state: string;
  url: string;
  detail: string;
};
export type ExternalPage = { items: ExternalItem[]; hasMore: boolean };
export type JiraInfo = {
  site: string;
  project: string;
  email: string;
  connected: boolean;
};
export type JiraIssue = {
  key: string;
  summary: string;
  status: string;
  issueType: string;
  url: string;
};
export type JiraPage = { issues: JiraIssue[]; nextPageToken: string | null };
export type JiraTransition = { id: string; name: string };
export type GitWorktree = {
  path: string;
  branch: string;
  main: boolean;
  locked: boolean;
};
export type GitToolsInfo = {
  submodules: { name: string; path: string; url: string }[];
  sparseEnabled: boolean;
  sparseCone: boolean;
  sparseDirectories: string[];
};
export type GitFlowInfo = {
  initialized: boolean;
  mainBranch: string;
  developBranch: string;
  featurePrefix: string;
  releasePrefix: string;
  hotfixPrefix: string;
  tagPrefix: string;
  currentBranch: string;
  publishTags: string[];
};
export type GitSigningInfo = {
  enabled: boolean;
  format: string;
  key: string;
  headSignature: string;
  headSigner: string;
  allowedSignersFile: string;
};
export type GitSignatureEntry = {
  commit: string;
  subject: string;
  status: string;
  signer: string;
};
export type RebaseStep = {
  id: string;
  subject: string;
  action: "pick" | "reword" | "squash" | "fixup" | "drop";
  message: string;
};
export type ReflogEntry = {
  selector: string;
  commit: string;
  action: string;
};
export type TransferRecord = {
  id: string;
  sourceProfile: string | null;
  sourcePath: string;
  destinationProfile: string | null;
  destinationDir: string;
  targetPath: string | null;
  conflict: string;
  status: string;
  bytesDone: number;
  totalBytes: number;
  error: string | null;
  createdAt: number;
  queueOrder: number;
  method: string;
};
export type TransferSettings = { parallelism: number; speedLimitKbps: number };
export type BatchTransfer = {
  sourceProfile: string | null;
  sourcePath: string;
  destinationProfile: string | null;
  destinationDir: string;
  conflict: "overwrite" | "skip" | "rename" | "newer";
};
export type BatchPreview = { sha256: string; transfers: BatchTransfer[] };
export type BatchQueueResult = {
  records: TransferRecord[];
  warning: string | null;
};
export type SyncCopy = {
  sourceProfile: string | null;
  sourcePath: string;
  destinationProfile: string | null;
  destinationDir: string;
  reason: string;
};
export type SyncDelete = { destinationProfile: string | null; path: string };
export type SyncPlan = {
  copies: SyncCopy[];
  deletes: SyncDelete[];
  issues: string[];
};
export type SyncMode =
  | "left-to-right"
  | "right-to-left"
  | "two-way"
  | "mirror-left-to-right"
  | "mirror-right-to-left";
export type SyncJob = {
  id: string;
  name: string;
  leftProfile: string | null;
  leftPath: string;
  rightProfile: string | null;
  rightPath: string;
  mode: SyncMode;
  intervalMinutes: number;
  enabled: boolean;
  running: boolean;
  lastRun: number | null;
  nextRun: number;
  lastQueued: number;
  lastError: string | null;
  watchLocal: boolean;
  exclude: string;
  lastFingerprint: string | null;
};
export type WorkspaceLayout = {
  page: "home" | "git" | "files" | "terminal";
  repo: string | null;
  terminals: {
    label: string;
    profileId?: string;
    cwd?: string;
    connection?: RemoteConnection;
    startupCommand?: string;
  }[];
  activeTerminal?: number;
  terminalSplit?: { index: number; direction: "row" | "column" };
  files?: { left: FileLocation; right: FileLocation };
};
export type FileLocation = { path: string; profileId: string | null };
export type Workspace = { id: string; name: string; layout: WorkspaceLayout };

export type RemoteRepository = {
  path: string;
  description: string;
  defaultBranch: string;
  denyForcePush: boolean;
};

export type OpenPgpKey = {
  fingerprint: string;
  userId: string;
  validity: string;
  secret: boolean;
};

export type ExternalComment = {
  id: number;
  thread: number;
  author: string;
  body: string;
  created: string;
  location: string;
};

export type ExternalRepository = {
  name: string;
  cloneUrl: string;
  webUrl: string;
  private: boolean;
};

export type GitToolSettings = {
  diffTool: string;
  mergeTool: string;
  tools: { id: string; name: string; installed: boolean }[];
};

export type BisectState = {
  active: boolean;
  current: string;
  log: string;
  found: string;
};

export type DeletedBranch = { name: string; commit: string; deletedAt: number };
export type ServerCertificate = {
  sha256: string;
  trustedByMac: boolean;
  matchesProfile: boolean;
};
export type CommitFile = {
  status: "A" | "M" | "D" | "R" | "C" | "T";
  path: string;
  oldPath: string | null;
};
export type CommitInfo = {
  id: string;
  parents: string[];
  author: string;
  authorEmail: string;
  authorDate: string;
  committer: string;
  committerDate: string;
  subject: string;
  body: string;
};
export type Comparison = {
  files: CommitFile[];
  ahead: number;
  behind: number;
  left: string;
};
export type ImageVersion = { dataUrl: string | null; size: number };
export type BranchDeleteReport = {
  deleted: string[];
  kept: [string, string][];
};
export type S3Properties = {
  contentType: string;
  cacheControl: string;
  contentDisposition: string;
  contentEncoding: string;
  storageClass: string;
  encryption: string;
  etag: string;
  lastModified: string;
  size: number;
  metadata: [string, string][];
  tags: [string, string][] | null;
};
export type S3PropertyChanges = {
  contentType: string;
  cacheControl: string;
  contentDisposition: string;
  contentEncoding: string;
  storageClass: string;
  metadata: [string, string][];
};
