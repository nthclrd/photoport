use tauri::Manager;

mod commands;
mod protocols;
mod state;
mod video;
#[cfg(feature = "updater")]
mod updater;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,idevice=warn".into()),
        )
        .with_target(false)
        .init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state::AppState::default())
        .register_asynchronous_uri_scheme_protocol("thumb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(protocols::serve_thumb(app, request).await);
            });
        })
        .register_asynchronous_uri_scheme_protocol("media", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(protocols::serve_media(app, request).await);
            });
        })
        .setup(|app| {
            if let Ok(dir) = app.path().app_cache_dir() {
                let _ = app.state::<state::AppState>().cache_dir.set(dir);
            }
            if let Some(thumbs) = protocols::thumbs_dir(app.handle()) {
                std::thread::spawn(move || protocols::prune_thumb_cache(&thumbs));
            }
            #[cfg(feature = "updater")]
            updater::spawn_check(app.handle().clone());
            Ok(())
        });

    #[cfg(feature = "updater")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::device_status,
            commands::scan_device,
            commands::start_import,
            commands::cancel_import,
            commands::open_apple_devices,
            commands::open_folder,
            commands::smart_prepare,
            commands::smart_search,
            #[cfg(feature = "updater")]
            updater::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("tauri app failed to run");
}
