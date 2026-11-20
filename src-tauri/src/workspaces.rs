use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub layout: Value,
}

#[tauri::command]
pub fn workspaces_list(state: State<'_, AppState>) -> Result<Vec<Workspace>, String> {
    let db = state.db.lock().map_err(|_| "Workspace database is busy")?;
    let mut statement = db
        .prepare("SELECT id, name, layout FROM workspaces ORDER BY name COLLATE NOCASE")
        .map_err(|error| format!("Could not load workspaces: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("Could not load workspaces: {error}"))?;
    rows.map(|row| {
        let (id, name, serialized) =
            row.map_err(|error| format!("Could not read a workspace: {error}"))?;
        let layout = serde_json::from_str(&serialized)
            .map_err(|error| format!("Workspace {name} has damaged layout data: {error}"))?;
        Ok(Workspace { id, name, layout })
    })
    .collect()
}

#[tauri::command]
pub fn workspaces_save(
    state: State<'_, AppState>,
    id: Option<String>,
    name: String,
    layout: Value,
) -> Result<Workspace, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a workspace name.".into());
    }
    if name.chars().count() > 80 {
        return Err("Workspace names must be 80 characters or fewer.".into());
    }
    if !layout.is_object() {
        return Err("Workspace layout must be an object.".into());
    }
    let serialized = serde_json::to_string(&layout)
        .map_err(|error| format!("Could not encode workspace: {error}"))?;
    if serialized.len() > 64 * 1024 {
        return Err("This workspace has too many tabs to save.".into());
    }
    let id = id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let db = state.db.lock().map_err(|_| "Workspace database is busy")?;
    db.execute("INSERT INTO workspaces (id, name, layout) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name, layout=excluded.layout", params![id, name, serialized])
        .map_err(|error| format!("Could not save workspace: {error}"))?;
    Ok(Workspace {
        id,
        name: name.into(),
        layout,
    })
}

#[tauri::command]
pub fn workspaces_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Workspace database is busy")?;
    let removed = db
        .execute("DELETE FROM workspaces WHERE id=?1", [id])
        .map_err(|error| format!("Could not delete workspace: {error}"))?;
    if removed == 0 {
        return Err("This workspace no longer exists.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::{collections::HashMap, sync::Mutex};
    use tauri::Manager;

    #[test]
    fn saves_restores_and_deletes_workspace() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE workspaces (id TEXT PRIMARY KEY, name TEXT NOT NULL, layout TEXT NOT NULL);").unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let layout = serde_json::json!({"page": "git", "repo": "/tmp/example"});
        let saved = workspaces_save(app.state(), None, " Thesis ".into(), layout.clone()).unwrap();
        assert_eq!(saved.name, "Thesis");
        assert_eq!(workspaces_list(app.state()).unwrap()[0].layout, layout);
        workspaces_save(
            app.state(),
            Some(saved.id.clone()),
            "Research".into(),
            layout,
        )
        .unwrap();
        assert_eq!(workspaces_list(app.state()).unwrap().len(), 1);
        workspaces_delete(app.state(), saved.id).unwrap();
        assert!(workspaces_list(app.state()).unwrap().is_empty());
    }
}
