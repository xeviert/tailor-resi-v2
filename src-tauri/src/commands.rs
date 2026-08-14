use crate::{
    analysis::{analyze_job, AnalysisConfig, JobAnalysis},
    error::AppError,
    server::{load_latest_capture, CapturedJob},
    tailoring::{tailor_and_render, TailorRequest, TailorResponse},
};
use serde::Serialize;
use std::{path::Path, process::Command};

#[tauri::command]
pub fn ping() -> Result<String, AppError> {
    Ok("pong".to_string())
}

#[tauri::command]
pub fn get_latest_job() -> Result<Option<CapturedJob>, AppError> {
    load_latest_capture().map_err(AppError::Message)
}

#[derive(Serialize)]
pub struct PipelineResult {
    pub analysis: JobAnalysis,
    pub resume: TailorResponse,
}

#[tauri::command]
pub async fn run_resume_pipeline(language: String) -> Result<PipelineResult, AppError> {
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    let config = AnalysisConfig::from_env().ok_or_else(|| {
        AppError::Message("OPENAI_API_KEY is required to analyze and tailor a resume.".to_string())
    })?;
    let analysis = analyze_job(&config, &captured.parsed)
        .await
        .map_err(|error| AppError::Message(error.to_string()))?;
    let resume = tailor_and_render(TailorRequest {
        language,
        parsed: captured.parsed,
        analysis: analysis.clone(),
    })
    .await
    .map_err(|error| AppError::Message(error.to_string()))?;
    Ok(PipelineResult { analysis, resume })
}

fn latest_pdf(language: &str) -> Result<std::path::PathBuf, AppError> {
    if !matches!(language, "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let root =
        crate::tailoring::workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let path = root
        .join("resume")
        .join("generated")
        .join(format!("Xevier_T_CV_{language}.pdf"));
    if !path.exists() {
        return Err(AppError::Message(
            "No generated PDF exists for this language yet.".to_string(),
        ));
    }
    Ok(path)
}

fn launch_path(path: &Path, reveal: bool) -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer.exe");
        if reveal {
            command.arg("/select,");
        }
        command.arg(path);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        });
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(path);
        command
    };
    command.spawn().map_err(AppError::from)?;
    Ok(())
}

#[tauri::command]
pub fn open_latest_pdf(language: String) -> Result<(), AppError> {
    launch_path(&latest_pdf(&language)?, false)
}

#[tauri::command]
pub fn reveal_latest_pdf(language: String) -> Result<(), AppError> {
    launch_path(&latest_pdf(&language)?, true)
}
