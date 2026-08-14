mod analysis;
mod commands;
mod config;
mod error;
mod server;
mod tailoring;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    config::load_development_env();

    tauri::Builder::default()
        .plugin(tauri_plugin_sql::Builder::default().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::get_latest_job,
            commands::run_resume_pipeline,
            commands::open_latest_pdf,
            commands::reveal_latest_pdf
        ])
        .setup(|app| {
            // Auto-open DevTools in debug builds
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            // Spawn the axum bridge server — detached, runs for app lifetime
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                server::start_server(handle).await;
                eprintln!("[server] Axum server exited unexpectedly");
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
