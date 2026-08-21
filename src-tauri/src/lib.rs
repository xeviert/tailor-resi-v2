mod analysis;
mod api_usage;
mod ats_score;
mod commands;
mod config;
mod error;
mod evidence;
mod http;
mod job_import;
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
            commands::get_latest_pipeline_result,
            commands::get_latest_pipeline_result_any_language,
            commands::record_ui_result_state,
            commands::import_job_from_url,
            commands::import_job_from_text,
            commands::analyze_latest_job,
            commands::prepare_evidence_preflight,
            commands::generate_tailored_resume,
            commands::retailor_resume_with_evidence,
            commands::get_evidence_bank,
            commands::remove_evidence_bank_entry,
            commands::run_resume_pipeline,
            commands::open_latest_pdf,
            commands::reveal_latest_pdf,
            commands::open_latest_docx,
            commands::reveal_latest_docx,
            commands::open_result_artifact,
            commands::reveal_result_artifact
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
