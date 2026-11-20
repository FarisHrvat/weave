//! A system-wide shortcut that shows or hides Weave from any app, like
//! iTerm2's hotkey window. The shortcut is saved in settings.

use crate::AppState;
use rusqlite::OptionalExtension;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SETTING: &str = "app.hotkey";

fn saved(state: &AppState) -> Option<String> {
    let db = state.db.lock().ok()?;
    db.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [SETTING],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub(crate) fn parse(text: &str) -> Result<Shortcut, String> {
    let shortcut: Shortcut = text
        .parse()
        .map_err(|_| format!("{text} is not a shortcut Weave can use."))?;
    if shortcut.mods.is_empty() && !text.starts_with('F') {
        return Err(
            "Use at least one of ⌘, ⌥, ⌃ or ⇧, so the shortcut does not block normal typing."
                .into(),
        );
    }
    Ok(shortcut)
}

fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let frontmost = window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false);
    if frontmost {
        let _ = window.hide();
    } else {
        crate::background::show_main_window(app);
    }
}

fn register<R: Runtime>(app: &AppHandle<R>, text: &str) -> Result<(), String> {
    let shortcut = parse(text)?;
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                toggle(app);
            }
        })
        .map_err(|_| {
            format!("{text} is already used by the system or another app. Choose another shortcut.")
        })
}

pub fn restore<R: Runtime>(app: &AppHandle<R>) {
    if let Some(text) = saved(&app.state::<AppState>()) {
        let _ = register(app, &text);
    }
}

#[tauri::command]
pub fn hotkey_get(state: State<'_, AppState>) -> Option<String> {
    saved(&state)
}

#[tauri::command]
pub fn hotkey_set(
    app: AppHandle,
    state: State<'_, AppState>,
    shortcut: Option<String>,
) -> Result<Option<String>, String> {
    let shortcut = shortcut
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    if let Some(text) = &shortcut {
        parse(text)?;
    }
    if let Some(previous) = saved(&state).and_then(|text| parse(&text).ok()) {
        let _ = app.global_shortcut().unregister(previous);
    }
    if let Some(text) = &shortcut {
        if let Err(error) = register(&app, text) {
            if let Some(previous) = saved(&state) {
                let _ = register(&app, &previous);
            }
            return Err(error);
        }
    }
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    match &shortcut {
        Some(text) => db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [SETTING, text.as_str()],
        ),
        None => db.execute("DELETE FROM settings WHERE key=?1", [SETTING]),
    }
    .map_err(|error| format!("Could not save the shortcut: {error}"))?;
    Ok(shortcut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_shortcuts_with_modifiers_only() {
        assert!(parse("Alt+Space").is_ok());
        assert!(parse("CommandOrControl+Shift+KeyT").is_ok());
        assert!(parse("F12").is_ok());
        assert!(parse("Super+Alt+KeyT").is_ok() && parse("Control+Backquote").is_ok());
        assert!(parse("KeyA").unwrap_err().contains("at least one"));
        assert!(parse("Alt+Nonsense").is_err());
    }
}
