//! Keeps Weave running in the menu bar after its window closes, so sync
//! jobs, queued transfers and SSH tunnels continue until the user quits.

use crate::AppState;
use rusqlite::OptionalExtension;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime, State,
};

const SETTING: &str = "app.keep_in_menu_bar";

pub fn enabled(state: &AppState) -> bool {
    let Ok(db) = state.db.lock() else {
        return true;
    };
    db.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [SETTING],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .is_none_or(|value| value != "false")
}

#[tauri::command]
pub fn background_mode_get(state: State<'_, AppState>) -> bool {
    enabled(&state)
}

#[tauri::command]
pub fn background_mode_set(state: State<'_, AppState>, enabled: bool) -> Result<bool, String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    db.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [SETTING, if enabled { "true" } else { "false" }],
    )
    .map_err(|error| format!("Could not save the menu bar setting: {error}"))?;
    Ok(enabled)
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Weave", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Weave", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("weave")
        .tooltip("Weave")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_on_and_remembers_the_choice() {
        let state = AppState::for_tests();
        state
            .db
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        assert!(enabled(&state));
        state
            .db
            .lock()
            .unwrap()
            .execute("INSERT INTO settings VALUES (?1, 'false')", [SETTING])
            .unwrap();
        assert!(!enabled(&state));
    }
}
