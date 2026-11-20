import { invoke } from "@tauri-apps/api/core";
import type {
  FileListing,
  FileZillaImportReport,
  SessionImportReport,
  CommitFile,
  CommitInfo,
  Comparison,
  ImageVersion,
  BranchDeleteReport,
  S3Properties,
  S3PropertyChanges,
  TextDocument,
  ExternalEditInfo,
  GitSnapshot,
  ExternalInfo,
  ExternalPage,
  JiraInfo,
  JiraIssue,
  JiraPage,
  JiraTransition,
  GitWorktree,
  GitToolsInfo,
  GitFlowInfo,
  GitSigningInfo,
  RemoteRepository,
  GitToolSettings,
  BisectState,
  DeletedBranch,
  ExternalComment,
  ExternalRepository,
  OpenPgpKey,
  GitSignatureEntry,
  RebaseStep,
  ReflogEntry,
  GitConflict,
  GitIdentity,
  ForgeInfo,
  ForgeItem,
  ForgePage,
  ForgeCommentsPage,
  ForgeReviewThreadsPage,
  ForgeReviewFilesPage,
  ForgeAccount,
  ForgeRepository,
  Commit,
  BlamePage,
  LineHistoryEntry,
  GitLfsInfo,
  LfsLock,
  LfsMigrationResult,
  Profile,
  SshTunnel,
  TailnetStatus,
  VpnService,
  VpnCommandProfile,
  VpnCommandStatus,
  Repo,
  TransferRecord,
  TransferSettings,
  BatchPreview,
  BatchQueueResult,
  SyncPlan,
  SyncMode,
  SyncJob,
  Workspace,
  WorkspaceLayout,
  ServerCertificate,
  KnownHost,
  SshPublicKey,
} from "./types";

export const api = {
  sshKeysList: () => invoke<SshPublicKey[]>("ssh_keys_list"),
  sshKeysGenerate: (name: string) =>
    invoke<SshPublicKey>("ssh_keys_generate", { name }),
  sshKeysInstallCommand: (profileId: string, publicPath: string) =>
    invoke<string>("ssh_keys_install_command", { profileId, publicPath }),
  networkPing: (host: string) => invoke<string>("network_ping", { host }),
  networkTraceroute: (host: string) =>
    invoke<string>("network_traceroute", { host }),
  networkCheckPort: (host: string, port: number) =>
    invoke<string>("network_check_port", { host, port }),
  networkWake: (mac: string, address: string, port: number) =>
    invoke<string>("network_wake", { mac, address, port }),
  knownHostsList: () => invoke<KnownHost[]>("known_hosts_list"),
  knownHostsRemove: (host: string) =>
    invoke<void>("known_hosts_remove", { host, confirmed: true }),
  fxpTransfer: (
    sourceProfile: string,
    sourcePath: string,
    destinationProfile: string,
    destinationDir: string,
    allowClearData: boolean,
  ) =>
    invoke<{
      destinationPath: string;
      bytesCopied: number;
      verifiedWith: string | null;
    }>("fxp_transfer", {
      sourceProfile,
      sourcePath,
      destinationProfile,
      destinationDir,
      allowClearData,
    }),
  externalInfo: (path: string) =>
    invoke<ExternalInfo>("external_info", { path }),
  externalConnect: (path: string, email: string, token: string) =>
    invoke<void>("external_connect", { path, email, token }),
  externalDisconnect: (path: string) =>
    invoke<void>("external_disconnect", { path }),
  externalList: (
    path: string,
    kind: "reviews" | "ci",
    page: number,
    state = "open",
  ) => invoke<ExternalPage>("external_list", { path, kind, page, state }),
  externalApprove: (path: string, number: number) =>
    invoke<void>("external_approve", { path, number }),
  externalComments: (path: string, number: number) =>
    invoke<ExternalComment[]>("external_comments", { path, number }),
  externalComment: (
    path: string,
    number: number,
    body: string,
    replyTo: { id: number; thread: number } | null = null,
  ) => invoke<void>("external_comment", { path, number, body, replyTo }),
  externalRepositories: (path: string) =>
    invoke<ExternalRepository[]>("external_repositories", { path }),
  externalCreateRepository: (path: string, name: string, priv: boolean) =>
    invoke<ExternalRepository>("external_create_repository", {
      path,
      name,
      private: priv,
    }),
  copyLink: (url: string) => invoke<void>("s3_share_copy_link", { url }),
  externalCreateReview: (
    path: string,
    title: string,
    description: string,
    source: string,
    target: string,
  ) =>
    invoke<void>("external_create_review", {
      path,
      title,
      description,
      source,
      target,
    }),
  externalMerge: (path: string, number: number) =>
    invoke<void>("external_merge", { path, number, confirmed: true }),
  jiraInfo: () => invoke<JiraInfo | null>("jira_info"),
  jiraConnect: (site: string, project: string, email: string, token: string) =>
    invoke<void>("jira_connect", { site, project, email, token }),
  jiraDisconnect: () => invoke<void>("jira_disconnect"),
  jiraList: (nextPageToken: string | null) =>
    invoke<JiraPage>("jira_list", { nextPageToken }),
  jiraCreate: (summary: string, description: string, issueType: string) =>
    invoke<JiraIssue>("jira_create", { summary, description, issueType }),
  jiraTransitions: (key: string) =>
    invoke<JiraTransition[]>("jira_transitions", { key }),
  jiraTransition: (key: string, transitionId: string) =>
    invoke<void>("jira_transition", { key, transitionId }),
  runTerminalScript: (
    profileId: string | null,
    cwd: string | null,
    script: string,
  ) =>
    invoke<{ exitCode: number | null; stdout: string; stderr: string }>(
      "term_run_script",
      { profileId, cwd, script },
    ),
  saveTerminalRecording: (path: string, content: string) =>
    invoke<void>("term_record_save", { path, content }),
  saveTerminalTextLog: (path: string, content: string) =>
    invoke<void>("term_text_log_save", { path, content }),
  profiles: () => invoke<Profile[]>("profiles_list"),
  saveProfile: (profile: Profile) =>
    invoke<Profile>("profiles_save", { profile }),
  deleteProfile: (id: string) => invoke<void>("profiles_delete", { id }),
  setProfilePassword: (
    profileId: string,
    password: string,
    remember: boolean,
  ) => invoke<void>("profile_password_set", { profileId, password, remember }),
  clearProfilePassword: (profileId: string) =>
    invoke<void>("profile_password_clear", { profileId }),
  setS3Token: (profileId: string, token: string, remember: boolean) =>
    invoke<void>("s3_token_set", { profileId, token, remember }),
  clearS3Token: (profileId: string) =>
    invoke<void>("s3_token_clear", { profileId }),
  importSsh: () => invoke<number>("profiles_import_ssh"),
  backgroundMode: () => invoke<boolean>("background_mode_get"),
  setBackgroundMode: (enabled: boolean) =>
    invoke<boolean>("background_mode_set", { enabled }),
  importSessions: (paths: string[]) =>
    invoke<SessionImportReport>("profiles_import_sessions", { paths }),
  importFileZilla: (path: string) =>
    invoke<FileZillaImportReport>("profiles_import_filezilla", { path }),
  tunnels: () => invoke<SshTunnel[]>("tunnels_list"),
  startTunnel: (
    profileId: string,
    kind: SshTunnel["kind"],
    listenPort: number,
    targetHost: string,
    targetPort: number,
  ) =>
    invoke<SshTunnel>("tunnels_start", {
      profileId,
      kind,
      listenPort,
      targetHost,
      targetPort,
    }),
  stopTunnel: (id: string) => invoke<void>("tunnels_stop", { id }),
  tailnetStatus: () => invoke<TailnetStatus>("tailnet_status"),
  tailnetConnect: () => invoke<TailnetStatus>("tailnet_connect"),
  tailnetDisconnect: () => invoke<TailnetStatus>("tailnet_disconnect"),
  tailnetCheckSsh: (ip: string, port: number) =>
    invoke<string>("tailnet_check_ssh", { ip, port }),
  vpnList: () => invoke<VpnService[]>("vpn_list"),
  vpnConnect: (id: string) => invoke<VpnService[]>("vpn_connect", { id }),
  vpnDisconnect: (id: string) => invoke<VpnService[]>("vpn_disconnect", { id }),
  vpnCheckSsh: (host: string, port: number) =>
    invoke<string>("vpn_check_ssh", { host, port }),
  vpnCommandsList: () => invoke<VpnCommandStatus[]>("vpn_commands_list"),
  vpnCommandsSave: (profile: VpnCommandProfile) =>
    invoke<VpnCommandProfile>("vpn_commands_save", { profile }),
  vpnCommandsDelete: (id: string) =>
    invoke<void>("vpn_commands_delete", { id }),
  vpnCommandsRun: (id: string, connect: boolean) =>
    invoke<VpnCommandStatus>("vpn_commands_run", { id, connect }),
  repos: () => invoke<Repo[]>("repos_list"),
  addRepo: (path: string) => invoke<Repo>("repos_add", { path }),
  removeRepo: (path: string) => invoke<void>("repos_remove", { path }),
  home: () => invoke<string>("files_home"),
  files: (path: string) => invoke<FileListing>("files_list", { path }),
  sftpFiles: (profileId: string, path: string) =>
    invoke<FileListing>("sftp_list", { profileId, path }),
  sftpMkdir: (profileId: string, parent: string, name: string) =>
    invoke<void>("sftp_mkdir", { profileId, parent, name }),
  sftpRename: (profileId: string, path: string, name: string) =>
    invoke<void>("sftp_rename", { profileId, path, name }),
  sftpDelete: (profileId: string, path: string, recursive: boolean) =>
    invoke<void>("sftp_delete", { profileId, path, recursive }),
  sftpChmod: (profileId: string, path: string, mode: number) =>
    invoke<void>("sftp_chmod", { profileId, path, mode }),
  s3ShareUrl: (profileId: string, path: string, expiresSeconds: number) =>
    invoke<string>("s3_share_url", { profileId, path, expiresSeconds }),
  s3ShareCopyLink: (url: string) => invoke<void>("s3_share_copy_link", { url }),
  serverCertificate: (
    protocol: string,
    host: string,
    port: number,
    profileId: string | null,
  ) =>
    invoke<ServerCertificate>("tls_server_certificate", {
      protocol,
      host,
      port,
      profileId,
    }),
  keychainIdentities: () =>
    invoke<{ id: string; name: string }[]>("keychain_identities"),
  exportKeychainIdentity: (id: string) =>
    invoke<string>("keychain_identity_export", { id }),
  webdavClientCertGet: (profileId: string) =>
    invoke<string | null>("webdav_client_cert_get", { profileId }),
  webdavClientCertSet: (profileId: string, path: string | null) =>
    invoke<void>("webdav_client_cert_set", { profileId, path }),
  awsProfiles: () => invoke<string[]>("s3_aws_profiles"),
  awsLinkGet: (profileId: string) =>
    invoke<string | null>("s3_aws_link_get", { profileId }),
  awsLink: (profileId: string, awsProfile: string | null) =>
    invoke<void>("s3_aws_link", { profileId, awsProfile }),
  webdavShareUrl: (
    profileId: string,
    path: string,
    expireDays: number | null,
    linkPassword: string | null,
  ) =>
    invoke<string>("webdav_share_url", {
      profileId,
      path,
      expireDays,
      linkPassword,
    }),
  mkdir: (parent: string, name: string) =>
    invoke<void>("files_mkdir", { parent, name }),
  rename: (path: string, name: string) =>
    invoke<void>("files_rename", { path, name }),
  delete: (path: string) => invoke<void>("files_delete", { path }),
  readText: (profileId: string | null, path: string) =>
    invoke<TextDocument>("editor_read", { profileId, path }),
  saveText: (
    profileId: string | null,
    path: string,
    expectedSha256: string,
    content: string,
  ) =>
    invoke<TextDocument>("editor_save", {
      profileId,
      path,
      expectedSha256,
      content,
    }),
  checkoutExternalEdit: (profileId: string, path: string) =>
    invoke<ExternalEditInfo>("external_edit_checkout", { profileId, path }),
  externalEdits: () => invoke<ExternalEditInfo[]>("external_edit_list"),
  setExternalEditAutoUpload: (id: string, enabled: boolean) =>
    invoke<ExternalEditInfo>("external_edit_auto_upload", { id, enabled }),
  uploadExternalEdit: (id: string) =>
    invoke<ExternalEditInfo>("external_edit_upload", { id }),
  discardExternalEdit: (id: string) =>
    invoke<void>("external_edit_discard", { id }),
  transfer: (source: string, destinationDir: string, conflict: string) =>
    invoke<string>("files_transfer", { source, destinationDir, conflict }),
  enqueueTransfer: (
    sourceProfile: string | null,
    sourcePath: string,
    destinationProfile: string | null,
    destinationDir: string,
    conflict: string,
  ) =>
    invoke<TransferRecord>("transfer_enqueue", {
      sourceProfile,
      sourcePath,
      destinationProfile,
      destinationDir,
      conflict,
    }),
  enqueueDirectTransfer: (
    method: "scp" | "rsync",
    sourceProfile: string | null,
    sourcePath: string,
    destinationProfile: string | null,
    destinationDir: string,
    conflict: string,
  ) =>
    invoke<TransferRecord>("transfer_enqueue_direct", {
      method,
      sourceProfile,
      sourcePath,
      destinationProfile,
      destinationDir,
      conflict,
    }),
  transfers: () => invoke<TransferRecord[]>("transfer_list"),
  previewBatch: (path: string) =>
    invoke<BatchPreview>("transfer_batch_preview", { path }),
  enqueueBatch: (path: string, expectedSha256: string) =>
    invoke<BatchQueueResult>("transfer_batch_enqueue", {
      path,
      expectedSha256,
    }),
  moveTransfer: (id: string, direction: "up" | "down") =>
    invoke<void>("transfer_move", { id, direction }),
  pauseTransfer: (id: string) => invoke<void>("transfer_pause", { id }),
  resumeTransfer: (id: string) => invoke<void>("transfer_resume", { id }),
  removeTransfer: (id: string) => invoke<void>("transfer_remove", { id }),
  transferSettings: () => invoke<TransferSettings>("transfer_settings"),
  saveTransferSettings: (parallelism: number, speedLimitKbps: number) =>
    invoke<TransferSettings>("transfer_settings_save", {
      parallelism,
      speedLimitKbps,
    }),
  syncPlan: (
    leftProfile: string | null,
    leftPath: string,
    rightProfile: string | null,
    rightPath: string,
    mode: SyncMode,
    exclude = "",
  ) =>
    invoke<SyncPlan>("sync_plan", {
      leftProfile,
      leftPath,
      rightProfile,
      rightPath,
      mode,
      exclude,
    }),
  syncMirrorDelete: (
    leftProfile: string | null,
    leftPath: string,
    rightProfile: string | null,
    rightPath: string,
    mode: SyncMode,
    expectedPaths: string[],
    exclude = "",
  ) =>
    invoke<number>("sync_mirror_delete", {
      leftProfile,
      leftPath,
      rightProfile,
      rightPath,
      mode,
      expectedPaths,
      exclude,
    }),
  syncJobs: () => invoke<SyncJob[]>("sync_jobs_list"),
  saveSyncJob: (
    name: string,
    leftProfile: string | null,
    leftPath: string,
    rightProfile: string | null,
    rightPath: string,
    mode: SyncMode,
    intervalMinutes: number,
    watchLocal: boolean,
    exclude = "",
  ) =>
    invoke<SyncJob>("sync_job_save", {
      job: {
        name,
        leftProfile,
        leftPath,
        rightProfile,
        rightPath,
        mode,
        intervalMinutes,
        watchLocal,
        exclude,
      },
    }),
  setSyncJobEnabled: (id: string, enabled: boolean) =>
    invoke<SyncJob>("sync_job_set_enabled", { id, enabled }),
  deleteSyncJob: (id: string) => invoke<void>("sync_job_delete", { id }),
  runSyncJob: (id: string) => invoke<SyncJob>("sync_job_run", { id }),
  git: (path: string) => invoke<GitSnapshot>("git_snapshot", { path }),
  gitCommitsPage: (path: string, skip: number) =>
    invoke<Commit[]>("git_commits_page", { path, skip }),
  gitCommitFiles: (path: string, commit: string) =>
    invoke<CommitFile[]>("git_commit_files", { path, commit }),
  s3ObjectProperties: (profileId: string, path: string) =>
    invoke<S3Properties>("s3_object_properties", { profileId, path }),
  s3ObjectPropertiesSet: (
    profileId: string,
    path: string,
    changes: S3PropertyChanges,
    tags: [string, string][] | null,
  ) =>
    invoke<void>("s3_object_properties_set", {
      profileId,
      path,
      changes,
      tags,
    }),
  mountInFinder: (profileId: string) =>
    invoke<string>("profile_mount_in_finder", { profileId }),
  filesExtract: (profileId: string | null, cwd: string, name: string) =>
    invoke<string>("files_extract", { profileId, cwd, name }),
  gitCommitInfo: (path: string, commit: string) =>
    invoke<CommitInfo>("git_commit_info", { path, commit }),
  gitCommitFileDiff: (path: string, commit: string, file: string) =>
    invoke<string>("git_commit_file_diff", { path, commit, file }),
  gitCompareRefs: (
    path: string,
    base: string,
    target: string,
    sinceFork: boolean,
  ) =>
    invoke<Comparison>("git_compare_refs", { path, base, target, sinceFork }),
  gitCompareFileDiff: (
    path: string,
    base: string,
    target: string,
    sinceFork: boolean,
    file: string,
    oldPath: string | null,
  ) =>
    invoke<string>("git_compare_file_diff", {
      path,
      base,
      target,
      sinceFork,
      file,
      oldPath,
    }),
  gitComparePaths: (left: string, right: string) =>
    invoke<string>("git_compare_paths", { left, right }),
  gitImageVersion: (path: string, revision: string, file: string) =>
    invoke<ImageVersion>("git_image_version", { path, revision, file }),
  gitBranchesDelete: (path: string, names: string[], force: boolean) =>
    invoke<BranchDeleteReport>("git_branches_delete", { path, names, force }),
  gitGraphOrder: (path: string) =>
    invoke<"topo" | "date">("git_graph_order", { path }),
  gitGraphOrderSet: (path: string, order: "topo" | "date") =>
    invoke<void>("git_graph_order_set", { path, order }),
  gitAiExplainCommit: (path: string, commit: string) =>
    invoke<string>("git_ai_explain_commit", { path, commit }),
  gitAiPullRequest: (path: string, base: string, target: string) =>
    invoke<string>("git_ai_pull_request", { path, base, target }),
  gitFileHistory: (path: string, file: string) =>
    invoke<Commit[]>("git_file_history", { path, file }),
  gitLineHistory: (path: string, file: string, start: number, end: number) =>
    invoke<LineHistoryEntry[]>("git_line_history", { path, file, start, end }),
  gitBlamePage: (path: string, file: string, start: number) =>
    invoke<BlamePage>("git_blame_page", { path, file, start }),
  gitLfsInfo: (path: string) => invoke<GitLfsInfo>("git_lfs_info", { path }),
  gitLfsTrack: (path: string, pattern: string, track: boolean) =>
    invoke<GitLfsInfo>("git_lfs_track", { path, pattern, track }),
  gitLfsPull: (path: string) => invoke<GitLfsInfo>("git_lfs_pull", { path }),
  gitLfsLocks: (path: string, remote: boolean) =>
    invoke<LfsLock[]>("git_lfs_locks", { path, remote }),
  gitLfsChangeLock: (path: string, file: string, lock: boolean) =>
    invoke<void>("git_lfs_change_lock", { path, file, lock }),
  gitLfsMigratePreview: (path: string, pattern: string) =>
    invoke<string>("git_lfs_migrate_preview", { path, pattern }),
  gitLfsMigrateImport: (path: string, pattern: string) =>
    invoke<LfsMigrationResult>("git_lfs_migrate_import", { path, pattern }),
  gitWorktrees: (path: string) =>
    invoke<GitWorktree[]>("git_worktrees", { path }),
  gitWorktreeCreate: (
    path: string,
    destination: string,
    branch: string,
    newBranch: boolean,
  ) =>
    invoke<string>("git_worktree_create", {
      path,
      destination,
      branch,
      newBranch,
    }),
  gitWorktreeRemove: (path: string, destination: string) =>
    invoke<void>("git_worktree_remove", { path, destination }),
  gitWorktreeLock: (path: string, destination: string, lock: boolean) =>
    invoke<GitWorktree[]>("git_worktree_lock", { path, destination, lock }),
  gitToolsInfo: (path: string) =>
    invoke<GitToolsInfo>("git_tools_info", { path }),
  gitFlowInfo: (path: string) => invoke<GitFlowInfo>("git_flow_info", { path }),
  gitFlowSetup: (
    path: string,
    settings: Omit<
      GitFlowInfo,
      "initialized" | "currentBranch" | "publishTags"
    >,
    update: boolean,
  ) => invoke<GitFlowInfo>("git_flow_setup", { path, settings, update }),
  gitFlowStart: (path: string, kind: string, name: string) =>
    invoke<GitFlowInfo>("git_flow_start", { path, kind, name }),
  gitFlowFinish: (path: string) =>
    invoke<GitFlowInfo>("git_flow_finish", { path }),
  gitFlowPublish: (path: string) =>
    invoke<string>("git_flow_publish", { path }),
  gitSubmoduleAdd: (path: string, url: string, directory: string) =>
    invoke<GitToolsInfo>("git_submodule_add", { path, url, directory }),
  gitSubmodulesUpdate: (path: string) =>
    invoke<GitToolsInfo>("git_submodules_update", { path }),
  gitSubmoduleRemove: (path: string, directory: string) =>
    invoke<GitToolsInfo>("git_submodule_remove", { path, directory }),
  gitSparseSet: (path: string, directories: string[]) =>
    invoke<GitToolsInfo>("git_sparse_set", { path, directories }),
  gitSparseSetPatterns: (path: string, patterns: string[]) =>
    invoke<GitToolsInfo>("git_sparse_set_patterns", { path, patterns }),
  gitSparseDisable: (path: string) =>
    invoke<GitToolsInfo>("git_sparse_disable", { path }),
  gitSigningInfo: (path: string) =>
    invoke<GitSigningInfo>("git_signing_info", { path }),
  gitSigningAudit: (path: string, skip: number) =>
    invoke<GitSignatureEntry[]>("git_signing_audit", { path, skip }),
  gitSigningTrustAdd: (
    path: string,
    principal: string,
    publicKeyPath: string,
  ) =>
    invoke<GitSigningInfo>("git_signing_trust_add", {
      path,
      principal,
      publicKeyPath,
    }),
  gitSigningSave: (
    path: string,
    enabled: boolean,
    format: "openpgp" | "ssh",
    key: string,
  ) =>
    invoke<GitSigningInfo>("git_signing_save", {
      path,
      enabled,
      format,
      key,
    }),
  gitRebasePlan: (path: string, base: string) =>
    invoke<RebaseStep[]>("git_rebase_plan", { path, base }),
  gitRebaseApply: (path: string, base: string, steps: RebaseStep[]) =>
    invoke<string>("git_rebase_apply", { path, base, steps }),
  openPgpKeys: () => invoke<OpenPgpKey[]>("git_openpgp_keys"),
  openPgpGenerateKey: (
    path: string,
    name: string,
    email: string,
    passphrase: string,
  ) =>
    invoke<GitSigningInfo>("git_openpgp_generate_key", {
      path,
      name,
      email,
      passphrase,
    }),
  openPgpImport: (keyFile: string) =>
    invoke<string>("git_openpgp_import", { keyFile }),
  openPgpTrust: (fingerprint: string) =>
    invoke<void>("git_openpgp_trust", { fingerprint }),
  gitCommitsFor: (path: string, refs: string[], skip: number) =>
    invoke<Commit[]>("git_commits_for", { path, refs, skip }),
  gitSubtree: (
    path: string,
    action: "add" | "pull" | "push",
    prefix: string,
    repository: string,
    branch: string,
    squash: boolean,
  ) =>
    invoke<string>("git_subtree", {
      path,
      action,
      prefix,
      repository,
      branch,
      squash,
    }),
  gitToolSettings: () => invoke<GitToolSettings>("git_tool_settings"),
  gitToolSettingsSave: (diffTool: string, mergeTool: string) =>
    invoke<GitToolSettings>("git_tool_settings_save", { diffTool, mergeTool }),
  gitOpenDifftool: (path: string, file: string, staged: boolean) =>
    invoke<void>("git_open_difftool", { path, file, staged }),
  gitOpenMergetool: (path: string, file: string) =>
    invoke<void>("git_open_mergetool", { path, file }),
  gitBisect: (
    path: string,
    action: "status" | "start" | "good" | "bad" | "skip" | "reset",
    good?: string,
    bad?: string,
  ) =>
    invoke<BisectState>("git_bisect", {
      path,
      action,
      good: good ?? null,
      bad: bad ?? null,
    }),
  gitPatchCreate: (path: string, revisions: string, folder: string) =>
    invoke<string[]>("git_patch_create", { path, revisions, folder }),
  gitPatchApply: (path: string, file: string, asCommits: boolean) =>
    invoke<string>("git_patch_apply", { path, file, asCommits }),
  gitCleanPreview: (path: string, ignored: boolean, directories: boolean) =>
    invoke<string[]>("git_clean_preview", { path, ignored, directories }),
  gitClean: (
    path: string,
    ignored: boolean,
    directories: boolean,
    expected: number,
  ) => invoke<number>("git_clean", { path, ignored, directories, expected }),
  gitStashPush: (
    path: string,
    message: string,
    includeUntracked: boolean,
    keepIndex: boolean,
    paths: string[],
  ) =>
    invoke<string>("git_stash_push", {
      path,
      message,
      includeUntracked,
      keepIndex,
      paths,
    }),
  gitTagAnnotated: (
    path: string,
    name: string,
    message: string,
    target: string,
    sign: boolean,
  ) => invoke<void>("git_tag_annotated", { path, name, message, target, sign }),
  gitPushTags: (path: string, tag: string | null, remove: boolean) =>
    invoke<string>("git_push_tags", { path, tag, delete: remove }),
  gitNotes: (path: string, commit: string) =>
    invoke<string>("git_notes", { path, commit }),
  gitNotesSet: (path: string, commit: string, text: string) =>
    invoke<void>("git_notes_set", { path, commit, text }),
  gitCustomCommand: (path: string, args: string[]) =>
    invoke<string>("git_custom_command", { path, args }),
  gitDeletedBranches: (path: string) =>
    invoke<DeletedBranch[]>("git_deleted_branches", { path }),
  gitBranchRestore: (path: string, name: string) =>
    invoke<string>("git_branch_restore", { path, name }),
  aiKeyStatus: () => invoke<boolean>("ai_key_status"),
  aiKeySet: (key: string) => invoke<void>("ai_key_set", { key }),
  aiKeyClear: () => invoke<void>("ai_key_clear"),
  gitAiCommitMessage: (path: string) =>
    invoke<string>("git_ai_commit_message", { path }),
  gitUndoInfo: (path: string) =>
    invoke<{ undo: string | null; redo: string | null }>("git_undo_info", {
      path,
    }),
  gitUndo: (path: string) => invoke<string>("git_undo", { path }),
  gitRedo: (path: string) => invoke<string>("git_redo", { path }),
  gitRebaseMerges: (path: string, onto: string) =>
    invoke<string>("git_rebase_merges", { path, onto }),
  gitRebaseMergesPlan: (path: string, onto: string) =>
    invoke<string>("git_rebase_merges_plan", { path, onto }),
  gitRebaseMergesApply: (path: string, onto: string, plan: string) =>
    invoke<string>("git_rebase_merges_apply", { path, onto, plan }),
  gitSigningGenerateKey: (path: string, principal: string) =>
    invoke<GitSigningInfo>("git_signing_generate_key", { path, principal }),
  gitRemoteReposList: (profileId: string | null, folder: string) =>
    invoke<RemoteRepository[]>("git_remote_repos_list", { profileId, folder }),
  gitRemoteRepoCreate: (
    profileId: string | null,
    path: string,
    defaultBranch: string,
    description: string,
  ) =>
    invoke<string>("git_remote_repo_create", {
      profileId,
      path,
      defaultBranch,
      description,
    }),
  gitRemoteRepoConfigure: (
    profileId: string | null,
    path: string,
    defaultBranch: string,
    description: string,
    denyForcePush: boolean,
  ) =>
    invoke<void>("git_remote_repo_configure", {
      profileId,
      path,
      defaultBranch,
      description,
      denyForcePush,
    }),
  gitRemoteHooks: (profileId: string | null, path: string) =>
    invoke<string[]>("git_remote_hooks", { profileId, path }),
  gitRemoteHookRead: (profileId: string | null, path: string, hook: string) =>
    invoke<string>("git_remote_hook_read", { profileId, path, hook }),
  gitRemoteHookSave: (
    profileId: string | null,
    path: string,
    hook: string,
    content: string,
  ) => invoke<void>("git_remote_hook_save", { profileId, path, hook, content }),
  gitRemoteHookDelete: (profileId: string | null, path: string, hook: string) =>
    invoke<void>("git_remote_hook_delete", { profileId, path, hook }),
  gitRemoteRepoDelete: (
    profileId: string | null,
    path: string,
    confirmation: string,
  ) =>
    invoke<void>("git_remote_repo_delete", { profileId, path, confirmation }),
  gitReflog: (path: string) => invoke<ReflogEntry[]>("git_reflog", { path }),
  gitReflogRestore: (path: string, commit: string) =>
    invoke<string>("git_reflog_restore", { path, commit, confirmed: true }),
  diff: (path: string, file: string, staged: boolean) =>
    invoke<string>("git_diff", { path, file, staged }),
  stageHunk: (path: string, file: string, index: number, staged: boolean) =>
    invoke<void>("git_stage_hunk", { path, file, index, staged }),
  stageLine: (
    path: string,
    file: string,
    hunkIndex: number,
    lineIndex: number,
    staged: boolean,
  ) =>
    invoke<void>("git_stage_line", {
      path,
      file,
      hunkIndex,
      lineIndex,
      staged,
    }),
  conflict: (path: string, file: string) =>
    invoke<GitConflict>("git_conflict", { path, file }),
  openConflictExternal: (path: string, file: string) =>
    invoke<void>("git_open_conflict_external", { path, file }),
  gitIdentity: (path: string) => invoke<GitIdentity>("git_identity", { path }),
  saveGitIdentity: (
    path: string,
    scope: "local" | "global",
    name: string,
    email: string,
  ) => invoke<GitIdentity>("git_identity_save", { path, scope, name, email }),
  searchCommits: (
    path: string,
    query: string,
    mode: "message" | "author" | "content",
  ) => invoke<Commit[]>("git_search", { path, query, mode }),
  resolveConflict: (path: string, file: string, result: string) =>
    invoke<void>("git_conflict_resolve", { path, file, result }),
  stage: (path: string, file: string) =>
    invoke<void>("git_stage", { path, file }),
  unstage: (path: string, file: string) =>
    invoke<void>("git_unstage", { path, file }),
  commit: (path: string, message: string) =>
    invoke<string>("git_commit", { path, message }),
  gitOperation: (
    path: string,
    operation: string,
    value?: string,
    confirmed = false,
  ) => invoke<string>("git_operation", { path, operation, value, confirmed }),
  branch: (path: string, name: string) =>
    invoke<void>("git_branch_create", { path, name }),
  checkout: (path: string, name: string) =>
    invoke<void>("git_checkout", { path, name }),
  checkoutRemote: (path: string, branch: string) =>
    invoke<void>("git_checkout_remote", { path, branch }),
  fetch: (path: string) => invoke<string>("git_fetch", { path }),
  pull: (path: string, mode?: "ff-only" | "rebase" | "merge") =>
    invoke<string>("git_pull", { path, mode: mode ?? null }),
  push: (path: string) => invoke<string>("git_push", { path }),
  publish: (path: string) => invoke<string>("git_publish", { path }),
  init: (path: string) => invoke<void>("git_init", { path }),
  clone: (url: string, destination: string, depth?: number) =>
    invoke<void>("git_clone", { url, destination, depth: depth ?? null }),
  forgeInfo: (path: string) => invoke<ForgeInfo>("forge_info", { path }),
  forgeConnect: (path: string, token: string) =>
    invoke<void>("forge_connect", { path, token }),
  forgeConnectUrl: (url: string, token: string) =>
    invoke<void>("forge_connect_url", { url, token }),
  forgeConnectUrlAs: (
    url: string,
    provider: "GitHub" | "GitLab",
    token: string,
  ) => invoke<void>("forge_connect_url_as", { url, provider, token }),
  forgeConnectSelfHosted: (
    path: string,
    provider: "GitHub" | "GitLab",
    token: string,
  ) => invoke<void>("forge_connect_self_hosted", { path, provider, token }),
  forgeConnectProvider: (provider: "GitHub" | "GitLab", token: string) =>
    invoke<void>("forge_connect_provider", { provider, token }),
  forgeAccounts: () => invoke<ForgeAccount[]>("forge_accounts"),
  forgeRepositories: (provider: "GitHub" | "GitLab", page: number) =>
    invoke<ForgeRepository[]>("forge_repositories", { provider, page }),
  forgeCreateRepository: (
    provider: "GitHub" | "GitLab",
    serverUrl: string,
    name: string,
    description: string,
    privateRepo: boolean,
    namespace: string,
  ) =>
    invoke<ForgeRepository>("forge_create_repository", {
      provider,
      serverUrl,
      name,
      description,
      private: privateRepo,
      namespace,
    }),
  forgeRepositoriesForUrl: (url: string, page: number) =>
    invoke<ForgeRepository[]>("forge_repositories_for_url", { url, page }),
  forgeDisconnect: (path: string) => invoke<void>("forge_disconnect", { path }),
  forgeList: (
    path: string,
    kind: "reviews" | "issues" | "ci",
    page: number,
    state: "open" | "all",
  ) => invoke<ForgePage>("forge_list", { path, kind, page, state }),
  forgeCreate: (
    path: string,
    kind: "reviews" | "issues",
    title: string,
    description: string,
    source?: string,
    target?: string,
  ) =>
    invoke<ForgeItem>("forge_create", {
      path,
      kind,
      title,
      description,
      source,
      target,
    }),
  forgeReview: (path: string, number: number) =>
    invoke<void>("forge_review", { path, number }),
  forgeComments: (
    path: string,
    kind: "reviews" | "issues",
    number: number,
    page: number,
  ) =>
    invoke<ForgeCommentsPage>("forge_comments", { path, kind, number, page }),
  forgeReviewThreads: (path: string, number: number, page: number) =>
    invoke<ForgeReviewThreadsPage>("forge_review_threads", {
      path,
      number,
      page,
    }),
  forgeReviewFiles: (path: string, number: number, page: number) =>
    invoke<ForgeReviewFilesPage>("forge_review_files", { path, number, page }),
  forgeReviewCreate: (
    path: string,
    number: number,
    oldPath: string,
    newPath: string,
    line: number,
    side: "LEFT" | "RIGHT",
    otherLine: number | null,
    body: string,
  ) =>
    invoke<void>("forge_review_create", {
      input: { path, number, oldPath, newPath, line, side, otherLine, body },
    }),
  forgeReviewReply: (
    path: string,
    number: number,
    threadId: string,
    body: string,
  ) => invoke<void>("forge_review_reply", { path, number, threadId, body }),
  forgeReviewResolve: (
    path: string,
    number: number,
    threadId: string,
    resolved: boolean,
  ) =>
    invoke<void>("forge_review_resolve", { path, number, threadId, resolved }),
  forgeCommentCreate: (
    path: string,
    kind: "reviews" | "issues",
    number: number,
    body: string,
  ) => invoke<void>("forge_comment_create", { path, kind, number, body }),
  forgeIssueState: (path: string, number: number, close: boolean) =>
    invoke<void>("forge_issue_state", { path, number, close }),
  forgeMerge: (path: string, number: number) =>
    invoke<void>("forge_merge", { path, number, confirmed: true }),
  termWrite: (id: string, data: string) =>
    invoke<void>("term_write", { id, data }),
  workspaces: () => invoke<Workspace[]>("workspaces_list"),
  saveWorkspace: (id: string | null, name: string, layout: WorkspaceLayout) =>
    invoke<Workspace>("workspaces_save", { id, name, layout }),
  deleteWorkspace: (id: string) => invoke<void>("workspaces_delete", { id }),
};

export function errorMessage(error: unknown): string {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "Something went wrong. Please try again.";
}
