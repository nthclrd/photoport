//! Auto-update, compiled only with `--features updater` (see docs/RELEASE.md:
//! it needs an updater key pair and a published `latest.json`).

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

pub fn spawn_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(updater) = app.updater() else { return };
        match updater.check().await {
            Ok(Some(update)) => {
                let _ = app.emit("photoport://update", update.version.clone());
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("update check failed: {e}"),
        }
    });
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
