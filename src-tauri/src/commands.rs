use crate::{
    analysis::{analyze_job, AnalysisConfig, JobAnalysis},
    error::AppError,
    server::{load_latest_capture, CapturedJob},
    tailoring::{tailor_and_render_with_progress, PipelineProgress, TailorRequest, TailorResponse},
};
use serde::Serialize;
use std::{path::Path, process::Command};
use tauri::{AppHandle, Emitter};

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
pub async fn run_resume_pipeline(
    app: AppHandle,
    language: String,
) -> Result<PipelineResult, AppError> {
    let reporter = |event: PipelineProgress| {
        if let Err(error) = app.emit("resume-pipeline-progress", event) {
            eprintln!("[pipeline] Failed to emit progress event: {error}");
        }
    };
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    reporter(PipelineProgress {
        stage: "ats_analysis",
        status: "started",
        message: "AI is analyzing ATS keywords, requirements, and role signals.".to_string(),
        attempt: None,
        total_attempts: None,
    });
    let config = match AnalysisConfig::from_env() {
        Some(config) => config,
        None => {
            let message = "OPENAI_API_KEY is required to analyze and tailor a resume.".to_string();
            reporter(PipelineProgress {
                stage: "ats_analysis",
                status: "failed",
                message: message.clone(),
                attempt: None,
                total_attempts: None,
            });
            return Err(AppError::Message(message));
        }
    };
    let analysis = match analyze_job(&config, &captured.parsed).await {
        Ok(analysis) => analysis,
        Err(error) => {
            let message = error.to_string();
            reporter(PipelineProgress {
                stage: "ats_analysis",
                status: "failed",
                message: message.clone(),
                attempt: None,
                total_attempts: None,
            });
            return Err(AppError::Message(message));
        }
    };
    reporter(PipelineProgress {
        stage: "ats_analysis",
        status: "completed",
        message: "ATS analysis completed.".to_string(),
        attempt: None,
        total_attempts: None,
    });
    let resume = tailor_and_render_with_progress(
        TailorRequest {
            language: language.clone(),
            parsed: captured.parsed,
            analysis: analysis.clone(),
        },
        Some(&reporter),
    )
    .await
    .map_err(|error| AppError::Message(error.to_string()))?;
    if resume.tailoring_status == "partial" {
        if let Err(error) = launch_path(&latest_docx(&language)?, false) {
            eprintln!("[pipeline] Failed to open validated DOCX: {error}");
        }
    }
    Ok(PipelineResult { analysis, resume })
}

fn latest_generated(language: &str, extension: &str) -> Result<std::path::PathBuf, AppError> {
    if !matches!(language, "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let root =
        crate::tailoring::workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let path = root
        .join("resume")
        .join("generated")
        .join(format!("Xevier_T_CV_{language}.{extension}"));
    if !path.exists() {
        return Err(AppError::Message(format!(
            "No generated {} exists for this language yet.",
            extension.to_uppercase()
        )));
    }
    Ok(path)
}

fn latest_pdf(language: &str) -> Result<std::path::PathBuf, AppError> {
    latest_generated(language, "pdf")
}

fn latest_docx(language: &str) -> Result<std::path::PathBuf, AppError> {
    latest_generated(language, "docx")
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

#[tauri::command]
pub fn open_latest_docx(language: String) -> Result<(), AppError> {
    launch_path(&latest_docx(&language)?, false)
}

#[tauri::command]
pub fn reveal_latest_docx(language: String) -> Result<(), AppError> {
    launch_path(&latest_docx(&language)?, true)
}
