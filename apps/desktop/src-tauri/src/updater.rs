//! In-app updates (Windows). The check reads the small, signed `latest.json` on our `Windows` release; the download
//! is verified against the public key in `tauri.conf.json` before the installer runs, so a swapped file is refused.
//! Whether to check at all (never while Hide my IP or Tor mode is on) is decided by the screen that calls these.

use tauri_plugin_updater::UpdaterExt;

/// When this build was made (build.rs).
const BUILT_AT: &str = env!("PRIVCORD_BUILT_AT");

/// An update whose signature names another version than the manifest, or is older than this build, is a rollback.
fn is_rollback(u: &tauri_plugin_updater::Update) -> bool {
    privcord_desktop_host::update_guard::is_rollback(&u.signature, &u.version, BUILT_AT.parse().unwrap_or(0))
}

#[derive(serde::Serialize)]
pub struct UpdateFound {
    version: String,
    notes: Option<String>,
}

/// `None` when this is already the newest build.
#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Result<Option<UpdateFound>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .filter(|u| !is_rollback(u));
    Ok(found.map(|u| UpdateFound {
        version: u.version,
        notes: u.body,
    }))
}

/// Downloads the newest build, checks its signature and starts the installer (the app closes and restarts).
#[tauri::command]
pub async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Err("PrivCord is already up to date.".into());
    };
    if is_rollback(&update) {
        return Err("That update is older than this version, so it was not installed.".into());
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())
}
