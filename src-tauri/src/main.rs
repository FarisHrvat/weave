mod account_oauth;
mod ai_commit;
mod app_lock;
mod aws_credentials;
mod azure;
mod background;
mod bitbucket_server;
mod cloud;
mod cryptomator;
mod direct_transfer;
mod editor;
mod external_forge;
mod file_servers;
mod file_tools;
mod files;
mod forge;
mod forge_oauth;
mod ftp;
mod fxp;
mod git;
mod git_compare;
mod git_extra;
mod git_flow;
mod git_lfs;
mod git_openpgp;
mod git_rebase;
mod git_recovery;
mod git_remote;
mod git_tools;
mod git_undo;
mod gitea;
mod host_options;
mod hotkey;
mod jira;
mod keychain_identity;
mod known_hosts;
mod network_shares;
mod network_tools;
mod platform;
mod profiles;
mod remote_access;
mod s3;
mod session_import;
mod settings_vault;
mod sftp;
mod ssh;
mod ssh_keys;
mod ssh_prompt;
mod startup;
mod sync_jobs;
mod tailnet;
mod team_vault;
mod terminal;
mod tls_pins;
mod transfer;
mod tunnels;
mod vault;
mod vnc;
mod vpn;
mod webdav;
mod workspaces;
mod zmodem;

use rusqlite::Connection;
use std::{
    collections::HashMap,
    fs,
    sync::{atomic::AtomicBool, Arc, Mutex},
};
use tauri::Manager;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub terminals: Mutex<HashMap<String, terminal::TerminalSession>>,
    pub transfers: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub queue_lock: Mutex<()>,
    pub session_passwords: Mutex<HashMap<String, zeroize::Zeroizing<String>>>,
    pub session_s3_tokens: Mutex<HashMap<String, zeroize::Zeroizing<String>>>,
    pub tunnels: Mutex<Vec<tunnels::Tunnel>>,
    pub external_edits: Mutex<HashMap<String, transfer::ExternalEditSession>>,
}

#[cfg(test)]
impl AppState {
    pub fn for_tests() -> Self {
        AppState {
            db: Mutex::new(Connection::open_in_memory().expect("in-memory database")),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            session_passwords: Mutex::new(HashMap::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            external_edits: Mutex::new(HashMap::new()),
        }
    }
}

/// The running app, for code that only has a profile in hand.
pub(crate) static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

fn main() {
    if ssh_prompt::serve_askpass() || forge::serve_git_askpass() || remote_access::serve_helper() {
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            if let Err(error) = ssh_prompt::start(app.handle().clone()) {
                eprintln!("{error}");
            }
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let _ = APP.set(app.handle().clone());
            let launch = startup::launch_mode();
            let lock_path = data_dir.join("scheduler.lock");
            if launch == startup::Launch::SyncOnly {
                // Weave is open and already runs the jobs.
                if !startup::take_lock(&lock_path, false)? {
                    std::process::exit(0);
                }
            }
            #[cfg(target_os = "macos")]
            if launch != startup::Launch::Normal {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let connection = Connection::open(data_dir.join("weave.db"))?;
            connection.execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE IF NOT EXISTS profiles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    host TEXT NOT NULL,
                    protocol TEXT NOT NULL DEFAULT 'ssh',
                    port INTEGER NOT NULL DEFAULT 22,
                    username TEXT NOT NULL,
                    key_path TEXT,
                    jump_host TEXT,
                    color TEXT NOT NULL DEFAULT '#938cff',
                    favorite INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE IF NOT EXISTS repos (
                    path TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    last_opened INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS workspaces (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    layout TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS transfers (
                    id TEXT PRIMARY KEY,
                    source_profile TEXT,
                    source_path TEXT NOT NULL,
                    destination_profile TEXT,
                    destination_dir TEXT NOT NULL,
                    target_path TEXT,
                    conflict TEXT NOT NULL,
                    status TEXT NOT NULL,
                    bytes_done INTEGER NOT NULL DEFAULT 0,
                    total_bytes INTEGER NOT NULL DEFAULT 0,
                    error TEXT,
                    queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 UPDATE transfers SET status='paused' WHERE status='running';",
            )?;
            profiles::migrate_profiles(&connection)?;
            transfer::migrate_queue_order(&connection)?;
            sync_jobs::migrate(&connection)?;
            webdav::load_client_identities(&connection)?;
            let external_edits = transfer::restore_external_edits(&data_dir.join("external-edits"))
                .map_err(std::io::Error::other)?;
            app.manage(AppState {
                db: Mutex::new(connection),
                terminals: Mutex::new(HashMap::new()),
                transfers: Mutex::new(HashMap::new()),
                queue_lock: Mutex::new(()),
                session_passwords: Mutex::new(HashMap::new()),
                session_s3_tokens: Mutex::new(HashMap::new()),
                tunnels: Mutex::new(Vec::new()),
                external_edits: Mutex::new(external_edits),
            });
            if launch == startup::Launch::SyncOnly {
                transfer::start_pending(app.handle()).map_err(std::io::Error::other)?;
                sync_jobs::run_due_and_exit(app.handle().clone());
                return Ok(());
            }
            background::setup_tray(app.handle())?;
            if launch == startup::Launch::Normal {
                background::show_main_window(app.handle());
            }
            startup::refresh_agents();
            hotkey::restore(app.handle());
            team_vault::start(app.handle().clone());
            transfer::watch_external_edits(app.handle().clone());
            // A running sync agent finishes its copies before this window takes over.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if startup::take_lock(&lock_path, true).unwrap_or(false) {
                    let _ = transfer::start_pending(&handle);
                    sync_jobs::start_scheduler(handle);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if background::enabled(window.state::<AppState>().inner()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            background::background_mode_get,
            background::background_mode_set,
            startup::startup_settings_get,
            keychain_identity::keychain_identities,
            keychain_identity::keychain_identity_export,
            hotkey::hotkey_get,
            host_options::host_options_get,
            host_options::host_options_set,
            hotkey::hotkey_set,
            startup::startup_settings_set,
            profiles::profiles_list,
            profiles::profiles_save,
            profiles::profiles_delete,
            vault::profile_password_set,
            vault::profile_password_clear,
            vault::s3_token_set,
            vault::s3_token_clear,
            profiles::profiles_import_ssh,
            profiles::profiles_import_filezilla,
            session_import::profiles_import_sessions,
            tailnet::tailnet_status,
            tailnet::tailnet_connect,
            tailnet::tailnet_disconnect,
            tailnet::tailnet_check_ssh,
            vpn::vpn_list,
            vpn::vpn_connect,
            vpn::vpn_disconnect,
            vpn::vpn_check_ssh,
            vpn::vpn_commands_list,
            vpn::vpn_commands_save,
            vpn::vpn_commands_delete,
            vpn::vpn_commands_run,
            known_hosts::known_hosts_list,
            known_hosts::known_hosts_remove,
            ssh_keys::ssh_keys_list,
            ssh_keys::ssh_keys_generate,
            ssh_keys::ssh_keys_install_command,
            network_tools::network_ping,
            network_tools::network_traceroute,
            network_tools::network_check_port,
            network_tools::network_wake,
            profiles::repos_list,
            profiles::repos_add,
            profiles::repos_remove,
            files::files_home,
            files::files_list,
            files::files_mkdir,
            files::files_rename,
            files::files_delete,
            files::files_transfer,
            network_shares::shares_mounted,
            network_shares::shares_connect,
            network_shares::shares_eject,
            network_shares::profile_mount_in_finder,
            file_tools::files_find,
            file_tools::files_preview,
            file_tools::files_symlink,
            file_tools::files_chown,
            file_tools::files_run_command,
            file_tools::files_archive,
            file_tools::files_extract,
            editor::editor_read,
            editor::editor_save,
            transfer::external_edit_checkout,
            transfer::external_edit_list,
            transfer::external_edit_upload,
            transfer::external_edit_discard,
            git::git_snapshot,
            git::git_commits_page,
            git::git_worktrees,
            git::git_worktree_create,
            git::git_worktree_remove,
            git::git_worktree_lock,
            git_tools::git_tools_info,
            git_tools::git_submodule_add,
            git_tools::git_submodules_update,
            git_tools::git_submodule_remove,
            git_tools::git_sparse_set,
            git_tools::git_sparse_set_patterns,
            git_tools::git_sparse_disable,
            git_tools::git_signing_info,
            git_tools::git_signing_save,
            git_tools::git_signing_audit,
            git_tools::git_signing_trust_add,
            git_tools::git_signing_generate_key,
            git_openpgp::git_openpgp_keys,
            git_openpgp::git_openpgp_generate_key,
            git_openpgp::git_openpgp_import,
            git_openpgp::git_openpgp_trust,
            git_flow::git_flow_info,
            git_flow::git_flow_setup,
            git_flow::git_flow_start,
            git_flow::git_flow_finish,
            git_rebase::git_rebase_plan,
            git_rebase::git_rebase_apply,
            git_rebase::git_rebase_merges,
            git_rebase::git_rebase_merges_plan,
            git_rebase::git_rebase_merges_apply,
            git_remote::git_remote_repos_list,
            git_remote::git_remote_repo_create,
            git_remote::git_remote_repo_configure,
            git_remote::git_remote_repo_delete,
            git_remote::git_remote_hooks,
            git_remote::git_remote_hook_read,
            git_remote::git_remote_hook_save,
            git_remote::git_remote_hook_delete,
            git_recovery::git_reflog,
            git_recovery::git_reflog_restore,
            git_undo::git_undo_info,
            git_undo::git_undo,
            git_undo::git_redo,
            git_extra::git_commits_for,
            git_extra::git_subtree,
            git_extra::git_tool_settings,
            git_extra::git_tool_settings_save,
            git_extra::git_open_difftool,
            git_extra::git_open_mergetool,
            git_extra::git_bisect,
            git_extra::git_patch_create,
            git_extra::git_patch_apply,
            git_extra::git_clean_preview,
            git_extra::git_clean,
            git_extra::git_stash_push,
            git_extra::git_tag_annotated,
            git_extra::git_push_tags,
            git_extra::git_notes,
            git_extra::git_notes_set,
            git_extra::git_custom_command,
            git_extra::git_deleted_branches,
            git_extra::git_branch_restore,
            ai_commit::ai_key_status,
            ai_commit::ai_key_set,
            ai_commit::ai_key_clear,
            ai_commit::git_ai_commit_message,
            ai_commit::git_ai_explain_commit,
            ai_commit::git_ai_pull_request,
            gitea::gitea_connect,
            gitea::gitea_disconnect,
            gitea::gitea_repositories,
            gitea::gitea_create,
            gitea::gitea_delete,
            gitea::gitea_collaborators,
            gitea::gitea_collaborator_set,
            gitea::gitolite_config,
            gitea::gitolite_config_save,
            git::git_diff,
            git::git_stage_hunk,
            git::git_stage_line,
            git::git_conflict,
            git::git_open_conflict_external,
            git::git_conflict_resolve,
            git::git_identity,
            git::git_identity_save,
            git::git_search,
            git::git_file_history,
            git::git_commit_files,
            git_compare::git_commit_info,
            git_compare::git_commit_file_diff,
            git_compare::git_compare_refs,
            git_compare::git_compare_file_diff,
            git_compare::git_compare_paths,
            git_compare::git_image_version,
            git_compare::git_branches_delete,
            git_compare::git_graph_order,
            git_compare::git_graph_order_set,
            git::git_line_history,
            git::git_blame_page,
            git::git_lfs_info,
            git::git_lfs_track,
            git::git_lfs_pull,
            git_lfs::git_lfs_locks,
            git_lfs::git_lfs_change_lock,
            git_lfs::git_lfs_migrate_preview,
            git_lfs::git_lfs_migrate_import,
            git::git_stage,
            git::git_unstage,
            git::git_commit,
            git::git_operation,
            git::git_branch_create,
            git::git_checkout,
            git::git_checkout_remote,
            git::git_fetch,
            git::git_pull,
            git::git_push,
            git::git_publish,
            git::git_flow_publish,
            git::git_init,
            git::git_clone,
            forge::forge_info,
            forge::forge_connect,
            forge::forge_connect_url,
            forge::forge_connect_url_as,
            forge::forge_connect_self_hosted,
            forge::forge_connect_provider,
            forge_oauth::forge_oauth_start,
            forge_oauth::forge_oauth_poll,
            forge::forge_accounts,
            forge::forge_repositories,
            forge::forge_create_repository,
            forge::forge_repositories_for_url,
            forge::forge_disconnect,
            forge::forge_list,
            forge::forge_create,
            forge::forge_issue_state,
            forge::forge_review,
            forge::forge_comments,
            forge::forge_comment_create,
            forge::forge_review_threads,
            forge::forge_review_files,
            forge::forge_review_create,
            forge::forge_review_reply,
            forge::forge_review_resolve,
            forge::forge_merge,
            external_forge::external_info,
            external_forge::external_connect,
            external_forge::external_disconnect,
            external_forge::external_oauth_bitbucket,
            external_forge::external_oauth_azure_start,
            external_forge::external_oauth_azure_poll,
            external_forge::external_list,
            external_forge::external_create_review,
            external_forge::external_merge,
            external_forge::external_approve,
            external_forge::external_comments,
            external_forge::external_comment,
            external_forge::external_repositories,
            external_forge::external_create_repository,
            jira::jira_info,
            jira::jira_connect,
            jira::jira_oauth_connect,
            jira::jira_disconnect,
            jira::jira_list,
            jira::jira_create,
            jira::jira_transitions,
            jira::jira_transition,
            terminal::term_open,
            terminal::system_notify,
            remote_access::term_open_connection,
            remote_access::serial_devices,
            remote_access::remote_tools_status,
            remote_access::remote_desktop_open,
            terminal::term_record_save,
            terminal::term_text_log_save,
            terminal::term_run_script,
            terminal::term_write,
            ssh_prompt::ssh_prompt_answer,
            app_lock::app_lock_get,
            app_lock::app_lock_set,
            app_lock::app_unlock,
            terminal::term_fill_password,
            terminal::term_write_base64,
            zmodem::zmodem_save,
            zmodem::zmodem_read,
            terminal::term_resize,
            terminal::term_close,
            tunnels::tunnels_list,
            tunnels::tunnels_start,
            tunnels::tunnels_stop,
            sftp::sftp_list,
            sftp::sftp_mkdir,
            sftp::sftp_rename,
            sftp::sftp_delete,
            sftp::sftp_chmod,
            s3::s3_share_url,
            s3::s3_object_properties,
            s3::s3_object_properties_set,
            s3::s3_share_copy_link,
            s3::webdav_share_url,
            webdav::webdav_client_cert_get,
            webdav::webdav_client_cert_set,
            cryptomator::cryptomator_unlock,
            cryptomator::cryptomator_create,
            cryptomator::cryptomator_lock,
            cryptomator::cryptomator_list,
            cryptomator::cryptomator_mkdir,
            cryptomator::cryptomator_add,
            cryptomator::cryptomator_extract,
            cryptomator::cryptomator_delete,
            cryptomator::cryptomator_rename,
            cryptomator::cryptomator_change_password,
            aws_credentials::s3_aws_profiles,
            aws_credentials::s3_aws_link_get,
            aws_credentials::s3_aws_link,
            tls_pins::tls_server_certificate,
            transfer::script::files_script_run,
            transfer::script::files_script_stop,
            cloud::cloud_app_get,
            cloud::cloud_app_set,
            cloud::cloud_signed_in,
            cloud::cloud_sign_in,
            cloud::cloud_sign_out,
            vnc::vnc_bridge_start,
            file_tools::local_path_kind,
            file_tools::open_local_file,
            file_servers::file_server_start,
            file_servers::file_server_stop,
            file_servers::file_servers_list,
            transfer::external_edit_auto_upload,
            transfer::transfer_enqueue,
            transfer::transfer_enqueue_direct,
            fxp::fxp_transfer,
            transfer::transfer_batch_preview,
            transfer::transfer_batch_enqueue,
            transfer::transfer_list,
            transfer::transfer_move,
            transfer::transfer_pause,
            transfer::transfer_resume,
            transfer::transfer_remove,
            transfer::transfer_settings,
            transfer::transfer_settings_save,
            transfer::sync_plan,
            transfer::sync_mirror_delete,
            sync_jobs::sync_jobs_list,
            sync_jobs::sync_job_save,
            sync_jobs::sync_job_set_enabled,
            sync_jobs::sync_job_delete,
            sync_jobs::sync_job_run,
            workspaces::workspaces_list,
            workspaces::workspaces_save,
            workspaces::workspaces_delete,
            settings_vault::settings_vault_export,
            settings_vault::settings_vault_import,
            team_vault::team_vault_status,
            team_vault::team_vault_configure,
            team_vault::team_vault_disconnect,
            team_vault::team_vault_share,
            team_vault::team_vault_sync,
        ])
        .build(tauri::generate_context!())
        .expect("Weave failed to start")
        .run(|app, event| match event {
            tauri::RunEvent::Exit => tunnels::stop_all(app.state::<AppState>().inner()),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => background::show_main_window(app),
            _ => {}
        });
}
