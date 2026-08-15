use crate::{
    analysis::{analyze_job, AnalysisConfig, JobAnalysis},
    evidence::{load_evidence_bank, preflight_items, remove_evidence, save_selected_evidence, selected_for_prompt, EvidenceBank, PreflightItem, SelectedEvidence},
    error::AppError,
    server::{load_latest_capture, CapturedJob},
    tailoring::{content_changes, load_base_resume, tailor_and_render_with_progress, workspace_root, BulletKeywordEmphasis, PipelineProgress, TailorRequest, TailorResponse, TailoringReport},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}, process::Command, time::{SystemTime, UNIX_EPOCH}};
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn ping() -> Result<String, AppError> {
    Ok("pong".to_string())
}

#[tauri::command]
pub fn get_latest_job() -> Result<Option<CapturedJob>, AppError> {
    let capture = load_latest_capture().map_err(AppError::Message)?;
    if let Some(captured) = capture.as_ref() {
        if let (Ok(root), Ok(capture_id)) = (
            workspace_root(),
            u64::try_from(captured.received_at_ms),
        ) {
            for language in ["en", "fr"] {
                let snapshot = result_snapshot_path(&root, capture_id, language);
                if !snapshot.is_file() {
                    if let Err(error) = recover_pipeline_result(&root, capture_id, language)
                    {
                        eprintln!(
                            "[pipeline-result] startup recovery failed capture={} language={}: {}",
                            captured.received_at_ms, language, error
                        );
                    }
                }
            }
        }
    }
    Ok(capture)
}

#[tauri::command]
pub fn get_latest_pipeline_result(
    language: String,
    capture_id: u64,
) -> Result<Option<StoredPipelineResult>, AppError> {
    if !matches!(language.as_str(), "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let capture = load_latest_capture()
        .map_err(AppError::Message)?
        .filter(|capture| capture.received_at_ms == u128::from(capture_id));
    if capture.is_none() {
        return Ok(None);
    }
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let path = result_snapshot_path(&root, capture_id, &language);
    if path.is_file() {
        match fs::read_to_string(&path)
            .map_err(|error| AppError::Message(error.to_string()))
            .and_then(|text| {
                serde_json::from_str::<StoredPipelineResult>(&text)
                    .map_err(|error| AppError::Message(error.to_string()))
            }) {
            Ok(result)
                if result.capture_received_at_ms == capture_id && result.language == language =>
            {
                eprintln!(
                    "[pipeline-result] snapshot loaded capture={} language={}",
                    capture_id, language
                );
                return Ok(Some(result));
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "[pipeline-result] snapshot unreadable capture={} language={}: {}",
                capture_id, language, error
            ),
        }
    }
    recover_pipeline_result(&root, capture_id, &language)
}

#[tauri::command]
pub fn get_latest_pipeline_result_any_language(
    capture_id: u64,
) -> Result<Option<StoredPipelineResult>, AppError> {
    let capture = load_latest_capture()
        .map_err(AppError::Message)?
        .filter(|capture| capture.received_at_ms == u128::from(capture_id));
    if capture.is_none() {
        return Ok(None);
    }
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let mut candidates = Vec::new();
    for language in ["en", "fr"] {
        if let Some(result) = get_latest_pipeline_result(language.to_string(), capture_id)? {
            let modified = modified_ms(&result_snapshot_path(&root, capture_id, language))
                .unwrap_or_default();
            candidates.push((modified, result));
        }
    }
    Ok(candidates
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, result)| result))
}

#[derive(Serialize)]
pub struct PipelineResult {
    pub analysis: JobAnalysis,
    pub resume: TailorResponse,
}

#[derive(Serialize)]
pub struct PreflightResult {
    pub analysis: JobAnalysis,
    pub items: Vec<PreflightItem>,
}

#[derive(Deserialize)]
pub struct GenerateTailoredResumeRequest {
    pub language: String,
    pub analysis: JobAnalysis,
    #[serde(default)]
    pub selected_evidence: Vec<SelectedEvidence>,
    #[serde(default)]
    pub bullet_keyword_emphasis: BulletKeywordEmphasis,
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
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    clear_pipeline_result(&root, capture_id, &language)?;
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
    let mut resume = tailor_and_render_with_progress(
        TailorRequest {
            language: language.clone(),
            parsed: captured.parsed,
            analysis: analysis.clone(),
            approved_evidence: vec![],
            bullet_keyword_emphasis: BulletKeywordEmphasis::Balanced,
        },
        Some(&reporter),
    )
    .await
    .map_err(|error| AppError::Message(error.to_string()))?;
    if resume.tailoring_status == "partial" && resume.latest_docx_path.is_some() {
        match latest_docx(&language).and_then(|path| launch_path(&path, false)) {
            Ok(()) => resume.docx_opened = true,
            Err(error) => {
                eprintln!("[pipeline] Failed to open validated DOCX: {error}");
                resume.docx_open_error = Some(error.to_string());
            }
        }
    }
    store_and_emit_result(
        &app,
        &root,
        capture_id,
        &language,
        &analysis,
        &resume,
    )?;
    eprintln!(
        "[pipeline-result] legacy command returning capture={} language={}",
        capture_id, language
    );
    Ok(PipelineResult { analysis, resume })
}

#[tauri::command]
pub async fn analyze_latest_job(language: String) -> Result<PreflightResult, AppError> {
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| AppError::Message("Capture a job with the browser extension first.".to_string()))?;
    let config = AnalysisConfig::from_env().ok_or_else(|| {
        AppError::Message("OPENAI_API_KEY is required to analyze a resume.".to_string())
    })?;
    let analysis = analyze_job(&config, &captured.parsed)
        .await
        .map_err(|error| AppError::Message(error.to_string()))?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let base_resume = load_base_resume(&root, &language).map_err(|error| AppError::Message(error.to_string()))?;
    let bank = load_evidence_bank(&root).map_err(|error| AppError::Message(error.to_string()))?;
    Ok(PreflightResult { items: preflight_items(&analysis, &base_resume, &bank), analysis })
}

#[tauri::command]
pub async fn generate_tailored_resume(
    app: AppHandle,
    request: GenerateTailoredResumeRequest,
) -> Result<TailorResponse, AppError> {
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| AppError::Message("Capture a job with the browser extension first.".to_string()))?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
    clear_pipeline_result(&root, capture_id, &request.language)?;
    save_selected_evidence(&root, &request.selected_evidence)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let reporter = |event: PipelineProgress| {
        if let Err(error) = app.emit("resume-pipeline-progress", event) {
            eprintln!("[pipeline] Failed to emit progress event: {error}");
        }
    };
    let language = request.language.clone();
    let analysis = request.analysis.clone();
    let mut response = tailor_and_render_with_progress(TailorRequest {
        language: language.clone(),
        parsed: captured.parsed,
        analysis: request.analysis,
        approved_evidence: selected_for_prompt(&request.selected_evidence),
        bullet_keyword_emphasis: request.bullet_keyword_emphasis,
    }, Some(&reporter)).await.map_err(|error| AppError::Message(error.to_string()))?;
    if response.tailoring_status == "partial" && response.latest_docx_path.is_some() {
        match latest_docx(&language).and_then(|path| launch_path(&path, false)) {
            Ok(()) => response.docx_opened = true,
            Err(error) => response.docx_open_error = Some(error.to_string()),
        }
    }
    store_and_emit_result(
        &app,
        &root,
        capture_id,
        &language,
        &analysis,
        &response,
    )?;
    eprintln!(
        "[pipeline-result] generate command returning capture={} language={} status={}",
        capture_id, language, response.tailoring_status
    );
    Ok(response)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredPipelineResult {
    pub schema_version: u8,
    pub capture_received_at_ms: u64,
    pub language: String,
    pub recovered_from_artifacts: bool,
    pub analysis: serde_json::Value,
    pub resume: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UiResultDiagnostic {
    pub capture_id: u64,
    pub language: String,
    pub source: String,
    pub completion_mounted: bool,
    pub completion_visible: bool,
    pub score: Option<u8>,
    pub change_count: usize,
    pub viewport_height: f64,
    pub rect_top: Option<f64>,
    pub rect_bottom: Option<f64>,
}

#[tauri::command]
pub fn record_ui_result_state(diagnostic: UiResultDiagnostic) -> Result<(), AppError> {
    if !matches!(diagnostic.language.as_str(), "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    if !matches!(diagnostic.source.as_str(), "command" | "event" | "recovery") {
        return Err(AppError::Message("Unknown result source.".to_string()));
    }
    let capture = load_latest_capture()
        .map_err(AppError::Message)?
        .filter(|capture| capture.received_at_ms == u128::from(diagnostic.capture_id));
    if capture.is_none() {
        return Err(AppError::Message(
            "UI result diagnostic does not match the latest capture.".to_string(),
        ));
    }
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let directory = root.join("data").join("tailoring-results");
    fs::create_dir_all(&directory).map_err(|error| AppError::Message(error.to_string()))?;
    let recorded_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::Message(error.to_string()))?
        .as_millis();
    let path = directory.join(format!(
        "ui-{}-{}.json",
        diagnostic.capture_id, diagnostic.language
    ));
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "recorded_at_ms": recorded_at_ms,
        "diagnostic": diagnostic,
    }))
    .map_err(|error| AppError::Message(error.to_string()))?;
    fs::write(&path, format!("{json}\n"))
        .map_err(|error| AppError::Message(error.to_string()))?;
    eprintln!("[ui-result] render diagnostic saved path={}", path.display());
    Ok(())
}

fn result_snapshot_path(root: &Path, capture_id: u64, language: &str) -> PathBuf {
    root.join("data")
        .join("tailoring-results")
        .join(format!("{capture_id}-{language}.json"))
}

fn clear_pipeline_result(root: &Path, capture_id: u64, language: &str) -> Result<(), AppError> {
    let path = result_snapshot_path(root, capture_id, language);
    if path.is_file() {
        fs::remove_file(&path).map_err(|error| AppError::Message(error.to_string()))?;
        eprintln!(
            "[pipeline-result] previous snapshot cleared capture={} language={}",
            capture_id, language
        );
    }
    Ok(())
}

fn persist_pipeline_result(
    root: &Path,
    result: &StoredPipelineResult,
) -> Result<PathBuf, AppError> {
    let path = result_snapshot_path(root, result.capture_received_at_ms, &result.language);
    fs::create_dir_all(path.parent().expect("result snapshot has a parent"))
        .map_err(|error| AppError::Message(error.to_string()))?;
    let json = serde_json::to_string_pretty(result)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, format!("{json}\n"))
        .map_err(|error| AppError::Message(error.to_string()))?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| AppError::Message(error.to_string()))?;
    }
    fs::rename(&temporary, &path).map_err(|error| AppError::Message(error.to_string()))?;
    Ok(path)
}

fn store_and_emit_result(
    app: &AppHandle,
    root: &Path,
    capture_id: u64,
    language: &str,
    analysis: &JobAnalysis,
    resume: &TailorResponse,
) -> Result<StoredPipelineResult, AppError> {
    let result = StoredPipelineResult {
        schema_version: 1,
        capture_received_at_ms: capture_id,
        language: language.to_string(),
        recovered_from_artifacts: false,
        analysis: serde_json::to_value(analysis)
            .map_err(|error| AppError::Message(error.to_string()))?,
        resume: serde_json::to_value(resume)
            .map_err(|error| AppError::Message(error.to_string()))?,
    };
    let path = persist_pipeline_result(root, &result)?;
    eprintln!(
        "[pipeline-result] snapshot saved capture={} language={} path={}",
        capture_id,
        language,
        path.display()
    );
    match app.emit("resume-pipeline-result", &result) {
        Ok(()) => eprintln!(
            "[pipeline-result] event emitted capture={} language={}",
            capture_id, language
        ),
        Err(error) => eprintln!(
            "[pipeline-result] event emit failed capture={} language={}: {}",
            capture_id, language, error
        ),
    }
    Ok(result)
}

fn modified_ms(path: &Path) -> Option<u128> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn recover_pipeline_result(
    root: &Path,
    capture_id: u64,
    language: &str,
) -> Result<Option<StoredPipelineResult>, AppError> {
    let variants = root.join("resume").join("variants");
    let suffix = format!("-{language}");
    let latest = fs::read_dir(&variants)
        .map_err(|error| AppError::Message(error.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        .filter_map(|entry| modified_ms(&entry.path()).map(|modified| (modified, entry.path())))
        .filter(|(modified, _)| *modified >= u128::from(capture_id))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path);
    let Some(variant_dir) = latest else {
        return Ok(None);
    };
    let variant_path = variant_dir.join("variant.json");
    let report_path = variant_dir.join("tailoring-report.json");
    if !variant_path.is_file() || !report_path.is_file() {
        return Ok(None);
    }
    let tailored: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&variant_path).map_err(|error| AppError::Message(error.to_string()))?,
    )
    .map_err(|error| AppError::Message(error.to_string()))?;
    let report: TailoringReport = serde_json::from_str(
        &fs::read_to_string(&report_path).map_err(|error| AppError::Message(error.to_string()))?,
    )
    .map_err(|error| AppError::Message(error.to_string()))?;
    let base = load_base_resume(root, language)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let changes = content_changes(&base, &tailored);
    let experience_bullets_changed = changes
        .iter()
        .filter(|change| change.path.starts_with("/experience/"))
        .count();
    let total_bullets = base["experience"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["bullets"].as_array().map(Vec::len).unwrap_or(0))
                .sum::<usize>()
        })
        .unwrap_or(0);
    let emphasis = if total_bullets > 0 && experience_bullets_changed == total_bullets {
        "high"
    } else {
        "balanced"
    };
    let docx_path = variant_dir.join(format!("Xevier_T_CV_{language}.docx"));
    let pdf_path = variant_dir.join(format!("Xevier_T_CV_{language}.pdf"));
    let latest_docx = root
        .join("resume")
        .join("generated")
        .join(format!("Xevier_T_CV_{language}.docx"));
    let latest_pdf = root
        .join("resume")
        .join("generated")
        .join(format!("Xevier_T_CV_{language}.pdf"));
    let current_artifact = |path: &Path| {
        path.is_file()
            && modified_ms(path)
                .map(|modified| modified >= u128::from(capture_id))
                .unwrap_or(false)
    };
    let pdf_ready = current_artifact(&pdf_path);
    let docx_ready = current_artifact(&docx_path);
    let summary = format!(
        "Tailoring completed with {} estimated ATS coverage. {} supported keywords were covered; {} unsupported keywords were omitted.",
        report.estimated_ats_coverage_score,
        report.covered_keywords.len(),
        report.omitted_unsupported_keywords.len()
    );
    let result = StoredPipelineResult {
        schema_version: 1,
        capture_received_at_ms: capture_id,
        language: language.to_string(),
        recovered_from_artifacts: true,
        analysis: serde_json::json!({ "summary": summary }),
        resume: serde_json::json!({
            "success": pdf_ready,
            "tailoring_status": if pdf_ready { "completed" } else { "partial" },
            "validation_status": if docx_ready { "passed" } else { "not_run" },
            "fit_status": if pdf_ready { "passed" } else { "not_run" },
            "page_count": if pdf_ready { Some(1) } else { None },
            "bullet_keyword_emphasis": emphasis,
            "experience_bullets_changed": experience_bullets_changed,
            "report": report,
            "tailored_content": tailored,
            "content_changes": changes,
            "variant_slug": variant_dir.file_name().and_then(|name| name.to_str()),
            "variant_json_path": relative_path(root, &variant_path),
            "report_json_path": relative_path(root, &report_path),
            "docx_path": docx_ready.then(|| relative_path(root, &docx_path)),
            "latest_docx_path": current_artifact(&latest_docx).then(|| relative_path(root, &latest_docx)),
            "pdf_path": pdf_ready.then(|| relative_path(root, &pdf_path)),
            "latest_pdf_path": current_artifact(&latest_pdf).then(|| relative_path(root, &latest_pdf)),
            "downloads_docx_path": null,
            "downloads_docx_error": null,
            "downloads_pdf_path": null,
            "downloads_error": null,
            "docx_opened": false,
            "docx_open_error": null,
            "error": if pdf_ready { None::<String> } else { Some("Recovered tailoring data; PDF was not available.".to_string()) }
        }),
    };
    persist_pipeline_result(root, &result)?;
    eprintln!(
        "[pipeline-result] recovered artifacts capture={} language={} variant={}",
        capture_id,
        language,
        variant_dir.display()
    );
    Ok(Some(result))
}

#[tauri::command]
pub fn get_evidence_bank() -> Result<EvidenceBank, AppError> {
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    load_evidence_bank(&root).map_err(|error| AppError::Message(error.to_string()))
}

#[tauri::command]
pub fn remove_evidence_bank_entry(term: String) -> Result<EvidenceBank, AppError> {
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    remove_evidence(&root, &term).map_err(|error| AppError::Message(error.to_string()))
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

#[cfg(test)]
mod tests {
    use super::{persist_pipeline_result, recover_pipeline_result, StoredPipelineResult};
    use crate::tailoring::TailoringReport;
    use serde_json::json;

    fn temporary_root(label: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("resume-{label}-{suffix}"))
    }

    #[test]
    fn persists_a_capture_matched_result_snapshot() {
        let root = temporary_root("stored-pipeline-result");
        let result = StoredPipelineResult {
            schema_version: 1,
            capture_received_at_ms: 1234,
            language: "en".to_string(),
            recovered_from_artifacts: false,
            analysis: json!({ "summary": "Summary" }),
            resume: json!({ "tailoring_status": "completed" }),
        };

        let path = persist_pipeline_result(&root, &result).unwrap();
        let stored: StoredPipelineResult =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

        assert_eq!(stored.capture_received_at_ms, 1234);
        assert_eq!(stored.language, "en");
        assert_eq!(stored.analysis["summary"], "Summary");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_summary_and_changes_from_existing_variant_artifacts() {
        let root = temporary_root("artifact-result-recovery");
        let content_dir = root.join("resume/content");
        let variant_dir = root.join("resume/variants/2026-08-16-example-role-en");
        let generated_dir = root.join("resume/generated");
        std::fs::create_dir_all(&content_dir).unwrap();
        std::fs::create_dir_all(&variant_dir).unwrap();
        std::fs::create_dir_all(&generated_dir).unwrap();
        let base = json!({
            "experience": [{ "bullets": ["Built APIs."] }],
            "skills": { "backend": "Backend: Rust" }
        });
        let tailored = json!({
            "experience": [{ "bullets": ["Built scalable APIs."] }],
            "skills": { "backend": "Backend: Rust, REST APIs" }
        });
        std::fs::write(
            content_dir.join("base.en.json"),
            serde_json::to_string(&base).unwrap(),
        )
        .unwrap();
        std::fs::write(
            variant_dir.join("variant.json"),
            serde_json::to_string(&tailored).unwrap(),
        )
        .unwrap();
        std::fs::write(
            variant_dir.join("tailoring-report.json"),
            serde_json::to_string(&TailoringReport {
                covered_keywords: vec!["Rust".to_string(), "REST APIs".to_string()],
                omitted_unsupported_keywords: vec!["Kubernetes".to_string()],
                changed_fields: vec!["experience.bullets".to_string()],
                safety_notes: vec![],
                estimated_ats_coverage_score: 81,
                bullet_rewrite_decisions: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(variant_dir.join("Xevier_T_CV_en.docx"), b"docx").unwrap();
        std::fs::write(variant_dir.join("Xevier_T_CV_en.pdf"), b"pdf").unwrap();
        std::fs::write(generated_dir.join("Xevier_T_CV_en.pdf"), b"pdf").unwrap();

        let recovered = recover_pipeline_result(&root, 0, "en").unwrap().unwrap();

        assert!(recovered.recovered_from_artifacts);
        assert_eq!(recovered.resume["report"]["estimated_ats_coverage_score"], 81);
        assert_eq!(recovered.resume["experience_bullets_changed"], 1);
        assert_eq!(recovered.resume["content_changes"].as_array().unwrap().len(), 2);
        assert_eq!(recovered.resume["latest_pdf_path"], "resume/generated/Xevier_T_CV_en.pdf");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn artifact_recovery_rejects_results_older_than_the_capture() {
        let root = temporary_root("stale-artifact-result");
        let variant_dir = root.join("resume/variants/2026-08-16-example-role-en");
        std::fs::create_dir_all(&variant_dir).unwrap();

        let recovered = recover_pipeline_result(&root, u64::MAX, "en").unwrap();

        assert!(recovered.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
