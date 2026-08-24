use crate::{
    analysis::{analyze_job, AnalysisConfig, JobAnalysis},
    error::AppError,
    evidence::{
        append_banked_terms, approved_evidence_for, equivalent_terms, infer_selected_term_kind,
        load_evidence_bank, placement_equivalent_terms, preflight_items, remove_evidence,
        save_selected_evidence, EvidenceBank, EvidenceEntry, PreflightItem, SelectedEvidence,
    },
    job_import::{import_from_text, import_from_url, ImportedJob},
    server::{
        capture_directory, load_latest_capture, persist_capture, remove_latest_capture,
        CapturedJob,
    },
    tailoring::{
        content_changes, failed_response, load_base_resume, publish_variant_artifact,
        tailor_and_render_with_progress, workspace_root, ArtifactProvenance, BulletKeywordEmphasis,
        BulletRewriteOutcome, PipelineProgress, RetailorMetadata, TailorRequest, TailorResponse,
        TailoringReport,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn ping() -> Result<String, AppError> {
    Ok("pong".to_string())
}

#[tauri::command(async)]
pub fn get_latest_job() -> Result<Option<CapturedJob>, AppError> {
    let capture = load_latest_capture().map_err(AppError::Message)?;
    if let Some(captured) = capture.as_ref() {
        if let (Ok(root), Ok(capture_id)) =
            (workspace_root(), u64::try_from(captured.received_at_ms))
        {
            for language in ["en", "fr"] {
                let snapshot = result_snapshot_path(&root, capture_id, language);
                if !snapshot.is_file() {
                    if let Err(error) = recover_pipeline_result(&root, capture_id, language) {
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

/// Forget the job on screen so the app returns to the empty import screen.
///
/// Only the `latest.json` pointer is removed. The timestamped capture it was copied from
/// stays in `data/job-captures/`, and every variant and result snapshot the job produced
/// stays where it was written - starting over abandons a job, it does not erase its history.
#[tauri::command(async)]
pub fn clear_latest_job() -> Result<(), AppError> {
    let path = capture_directory()
        .map_err(AppError::Message)?
        .join("latest.json");
    remove_latest_capture(&path).map_err(AppError::Message)?;
    eprintln!("[capture] cleared latest capture at {}", path.display());
    Ok(())
}

#[tauri::command(async)]
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
                    .map(normalize_stored_result)
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

#[tauri::command(async)]
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
            let modified =
                modified_ms(&result_snapshot_path(&root, capture_id, language)).unwrap_or_default();
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

fn prepare_preflight_result(
    root: &Path,
    language: &str,
    analysis: JobAnalysis,
) -> Result<PreflightResult, AppError> {
    if !matches!(language, "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let base_resume =
        load_base_resume(root, language).map_err(|error| AppError::Message(error.to_string()))?;
    let bank = load_evidence_bank(root).map_err(|error| AppError::Message(error.to_string()))?;
    Ok(PreflightResult {
        items: preflight_items(&analysis, &base_resume, &bank),
        analysis,
    })
}

/// Saves this session's confirmations, then assembles everything the tailoring model is
/// allowed to rely on: the freshly confirmed terms plus every previously attested bank entry
/// the preflight resolved for this job.
fn build_approved_evidence(
    root: &Path,
    language: &str,
    analysis: &JobAnalysis,
    selected: &[SelectedEvidence],
) -> Result<Vec<EvidenceEntry>, AppError> {
    let bank = save_selected_evidence(root, selected)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let base_resume =
        load_base_resume(root, language).map_err(|error| AppError::Message(error.to_string()))?;
    let preflight = preflight_items(analysis, &base_resume, &bank);
    Ok(approved_evidence_for(&preflight, &bank, selected))
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

#[derive(Deserialize)]
pub struct RetailorResumeRequest {
    pub capture_id: u64,
    pub language: String,
    pub source_variant_slug: String,
    pub selected_terms: Vec<String>,
}

fn validate_selected_omitted_terms(
    selected_terms: &[String],
    omitted_terms: &[&str],
) -> Result<Vec<String>, AppError> {
    if selected_terms.is_empty() {
        return Err(AppError::Message(
            "Select at least one omitted phrase before re-tailoring.".to_string(),
        ));
    }
    if selected_terms.len() > 30 {
        return Err(AppError::Message(
            "No more than 30 omitted phrases can be selected at once.".to_string(),
        ));
    }
    let mut validated: Vec<String> = Vec::new();
    for selected in selected_terms {
        let selected = selected.trim();
        if selected.is_empty() || selected.chars().count() > 200 {
            return Err(AppError::Message(
                "Selected phrases must contain between 1 and 200 characters.".to_string(),
            ));
        }
        let canonical = omitted_terms
            .iter()
            .find(|omitted| omitted.trim().eq_ignore_ascii_case(selected))
            .ok_or_else(|| {
                AppError::Message(format!(
                    "The selected phrase is not in the source result's omitted list: {selected}"
                ))
            })?;
        if !validated.iter().any(|existing| {
            equivalent_terms(existing, canonical) || placement_equivalent_terms(existing, canonical)
        }) {
            validated.push((*canonical).to_string());
        }
    }
    if validated.is_empty() {
        return Err(AppError::Message(
            "Select at least one distinct omitted phrase before re-tailoring.".to_string(),
        ));
    }
    Ok(validated)
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
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("ats_analysis", &message, None),
                None,
                None,
                Some("ats_analysis"),
                Some(message.clone()),
            )?;
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
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("ats_analysis", &message, None),
                None,
                None,
                Some("ats_analysis"),
                Some(message.clone()),
            )?;
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
    let mut resume = match tailor_and_render_with_progress(
        TailorRequest {
            language: language.clone(),
            parsed: captured.parsed,
            analysis: analysis.clone(),
            approved_evidence: vec![],
            priority_attested_terms: vec![],
            bullet_keyword_emphasis: BulletKeywordEmphasis::Balanced,
        },
        Some(&reporter),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let message = error.to_string();
            let response = failed_response(message.clone());
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("resume_tailoring", &message, Some(&analysis)),
                Some(&analysis),
                Some(&response),
                Some("resume_tailoring"),
                Some(message.clone()),
            )?;
            return Err(AppError::Message(message));
        }
    };
    if resume.tailoring_status == "partial" && resume.artifact.is_some() {
        let downloads_path = resume
            .artifact
            .as_ref()
            .map(|artifact| PathBuf::from(&artifact.downloads_path))
            .expect("partial artifact checked above");
        match launch_path(&downloads_path, false) {
            Ok(()) => resume.docx_opened = true,
            Err(error) => {
                eprintln!("[pipeline] Failed to open validated DOCX: {error}");
                resume.docx_open_error = Some(error.to_string());
            }
        }
    }
    store_and_emit_result(&app, &root, capture_id, &language, &analysis, &resume)?;
    eprintln!(
        "[pipeline-result] legacy command returning capture={} language={}",
        capture_id, language
    );
    Ok(PipelineResult { analysis, resume })
}

/// Import a job post by fetching the page the user pasted a link to.
#[tauri::command]
pub async fn import_job_from_url(app: AppHandle, url: String) -> Result<CapturedJob, AppError> {
    let imported = import_from_url(&url)
        .await
        .map_err(|error| AppError::Message(error.to_string()))?;
    finish_import(&app, imported)
}

/// Import a job post from text the user pasted, for the boards that refuse to be fetched.
#[tauri::command]
pub async fn import_job_from_text(
    app: AppHandle,
    text: String,
    source_url: Option<String>,
) -> Result<CapturedJob, AppError> {
    let imported = import_from_text(&text, source_url.as_deref())
        .await
        .map_err(|error| AppError::Message(error.to_string()))?;
    finish_import(&app, imported)
}

/// Persist and announce an import exactly the way `/captures` announces an extension capture.
///
/// The event is what actually drives the UI: its listener owns the whole new-capture reset, so
/// the caller must not apply the returned capture itself. Doing both would make the listener
/// discard the event as a duplicate and skip the reset, stranding the previous run's result and
/// preflight on top of a different job.
fn finish_import(app: &AppHandle, imported: ImportedJob) -> Result<CapturedJob, AppError> {
    let captured = persist_capture(&imported.payload).map_err(AppError::Message)?;
    // Which path ran is the difference between a free import and a paid one, so it is worth
    // being able to see without going digging in `data/api-usage/`.
    eprintln!(
        "[job-import] capture {} extracted via {}",
        captured.received_at_ms,
        imported.extraction.label()
    );
    // The event is the only thing that reaches the UI - the panel deliberately ignores the
    // capture returned here. Swallowing an emit failure would leave the job on disk and the
    // window still showing the previous one, with nothing to say why.
    app.emit("job-data-received", &captured).map_err(|error| {
        eprintln!("[job-import] Failed to emit capture event: {error}");
        AppError::Message(format!(
            "The job was imported but the app could not display it ({error}). Restart the desktop app to load it."
        ))
    })?;
    Ok(captured)
}

#[tauri::command]
pub async fn analyze_latest_job(
    app: AppHandle,
    language: String,
) -> Result<PreflightResult, AppError> {
    // Analysis used to run silently: the review screen only mounts its progress panel once
    // an event has arrived, so a call that can legitimately take minutes showed nothing but
    // a button reading "Working...". Reporting the same two stages the failure summaries
    // already name means the panel has something to draw from the first second.
    let reporter = |event: PipelineProgress| {
        if let Err(error) = app.emit("resume-pipeline-progress", event) {
            eprintln!("[analysis] Failed to emit progress event: {error}");
        }
    };
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
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
            let message = "OPENAI_API_KEY is required to analyze a resume.".to_string();
            reporter(PipelineProgress {
                stage: "ats_analysis",
                status: "failed",
                message: message.clone(),
                attempt: None,
                total_attempts: None,
            });
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("ats_analysis", &message, None),
                None,
                None,
                Some("ats_analysis"),
                Some(message.clone()),
            )?;
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
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("ats_analysis", &message, None),
                None,
                None,
                Some("ats_analysis"),
                Some(message.clone()),
            )?;
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
    reporter(PipelineProgress {
        stage: "evidence_preflight",
        status: "started",
        message: "Matching analyzed terms against the base resume and evidence bank."
            .to_string(),
        attempt: None,
        total_attempts: None,
    });
    let result = match prepare_preflight_result(&root, &language, analysis.clone()) {
        Ok(result) => result,
        Err(error) => {
            let message = error.to_string();
            reporter(PipelineProgress {
                stage: "evidence_preflight",
                status: "failed",
                message: message.clone(),
                attempt: None,
                total_attempts: None,
            });
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("evidence_preflight", &message, Some(&analysis)),
                Some(&analysis),
                None,
                Some("evidence_preflight"),
                Some(message.clone()),
            )?;
            return Err(AppError::Message(message));
        }
    };
    reporter(PipelineProgress {
        stage: "evidence_preflight",
        status: "completed",
        message: "Evidence resolved.".to_string(),
        attempt: None,
        total_attempts: None,
    });
    store_and_emit_outcome(
        &app,
        &root,
        capture_id,
        &language,
        "analysis_ready",
        result.analysis.summary.clone(),
        Some(&result.analysis),
        None,
        None,
        None,
    )?;
    Ok(result)
}

#[tauri::command(async)]
pub fn prepare_evidence_preflight(
    app: AppHandle,
    language: String,
    analysis: JobAnalysis,
) -> Result<PreflightResult, AppError> {
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
    let result = match prepare_preflight_result(&root, &language, analysis.clone()) {
        Ok(result) => result,
        Err(error) => {
            let message = error.to_string();
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("evidence_preflight", &message, Some(&analysis)),
                Some(&analysis),
                None,
                Some("evidence_preflight"),
                Some(message.clone()),
            )?;
            return Err(AppError::Message(message));
        }
    };
    store_and_emit_outcome(
        &app,
        &root,
        capture_id,
        &language,
        "analysis_ready",
        result.analysis.summary.clone(),
        Some(&result.analysis),
        None,
        None,
        None,
    )?;
    Ok(result)
}

#[tauri::command]
pub async fn generate_tailored_resume(
    app: AppHandle,
    request: GenerateTailoredResumeRequest,
) -> Result<TailorResponse, AppError> {
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
    let language = request.language.clone();
    let analysis = request.analysis.clone();
    let reporter = |event: PipelineProgress| {
        if let Err(error) = app.emit("resume-pipeline-progress", event) {
            eprintln!("[pipeline] Failed to emit progress event: {error}");
        }
    };
    reporter(PipelineProgress {
        stage: "resume_tailoring",
        status: "started",
        message: "Preparing reviewed evidence and resume inputs.".to_string(),
        attempt: None,
        total_attempts: None,
    });
    let approved_evidence =
        match build_approved_evidence(&root, &language, &analysis, &request.selected_evidence) {
            Ok(approved) => approved,
            Err(error) => {
                let message = error.to_string();
                reporter(PipelineProgress {
                    stage: "resume_tailoring",
                    status: "failed",
                    message: message.clone(),
                    attempt: None,
                    total_attempts: None,
                });
                store_and_emit_outcome(
                    &app,
                    &root,
                    capture_id,
                    &language,
                    "failed",
                    failure_summary("evidence_save", &message, Some(&analysis)),
                    Some(&analysis),
                    None,
                    Some("evidence_save"),
                    Some(message.clone()),
                )?;
                return Err(AppError::Message(message));
            }
        };
    let mut response = match tailor_and_render_with_progress(
        TailorRequest {
            language: language.clone(),
            parsed: captured.parsed,
            analysis: request.analysis,
            approved_evidence,
            priority_attested_terms: vec![],
            bullet_keyword_emphasis: request.bullet_keyword_emphasis,
        },
        Some(&reporter),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let message = error.to_string();
            let mut response = failed_response(message.clone());
            response.bullet_keyword_emphasis = request.bullet_keyword_emphasis;
            store_and_emit_outcome(
                &app,
                &root,
                capture_id,
                &language,
                "failed",
                failure_summary("resume_tailoring", &message, Some(&analysis)),
                Some(&analysis),
                Some(&response),
                Some("resume_tailoring"),
                Some(message.clone()),
            )?;
            return Err(AppError::Message(message));
        }
    };
    if response.tailoring_status == "partial" && response.artifact.is_some() {
        let downloads_path = response
            .artifact
            .as_ref()
            .map(|artifact| PathBuf::from(&artifact.downloads_path))
            .expect("partial artifact checked above");
        match launch_path(&downloads_path, false) {
            Ok(()) => response.docx_opened = true,
            Err(error) => response.docx_open_error = Some(error.to_string()),
        }
    }
    store_and_emit_result(&app, &root, capture_id, &language, &analysis, &response)?;
    eprintln!(
        "[pipeline-result] generate command returning capture={} language={} status={}",
        capture_id, language, response.tailoring_status
    );
    Ok(response)
}

#[tauri::command]
pub async fn retailor_resume_with_evidence(
    app: AppHandle,
    request: RetailorResumeRequest,
) -> Result<TailorResponse, AppError> {
    if !matches!(request.language.as_str(), "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let captured = load_latest_capture()
        .map_err(AppError::Message)?
        .ok_or_else(|| {
            AppError::Message("Capture a job with the browser extension first.".to_string())
        })?;
    let capture_id = u64::try_from(captured.received_at_ms)
        .map_err(|_| AppError::Message("Capture timestamp is out of range.".to_string()))?;
    if capture_id != request.capture_id {
        return Err(AppError::Message(
            "The displayed result no longer matches the latest captured job.".to_string(),
        ));
    }
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let snapshot_path = result_snapshot_path(&root, capture_id, &request.language);
    let stored = fs::read_to_string(&snapshot_path)
        .map_err(|_| {
            AppError::Message(
                "The source tailoring result could not be loaded. Run tailoring again first."
                    .to_string(),
            )
        })
        .and_then(|text| {
            serde_json::from_str::<StoredPipelineResult>(&text)
                .map(normalize_stored_result)
                .map_err(|error| AppError::Message(error.to_string()))
        })?;
    if stored.capture_received_at_ms != capture_id || stored.language != request.language {
        return Err(AppError::Message(
            "The stored result does not match the requested capture and language.".to_string(),
        ));
    }
    let source_variant_slug = stored.resume["variant_slug"]
        .as_str()
        .ok_or_else(|| AppError::Message("The source result has no saved variant.".to_string()))?;
    if source_variant_slug != request.source_variant_slug {
        return Err(AppError::Message(
            "The displayed variant is stale. Reload the latest result before re-tailoring."
                .to_string(),
        ));
    }
    // Prefer the measured score. Results stored before scoring existed only carry the model's
    // estimate, and refusing to re-tailor those would strand the user's existing history.
    let source_score = stored.resume["report"]["ats_coverage"]["score"]
        .as_u64()
        .or_else(|| stored.resume["report"]["estimated_ats_coverage_score"].as_u64())
        .and_then(|score| u8::try_from(score).ok())
        .ok_or_else(|| AppError::Message("The source result has no ATS score.".to_string()))?;
    let omitted_terms = stored.resume["report"]["omitted_unsupported_keywords"]
        .as_array()
        .ok_or_else(|| {
            AppError::Message("The source result has no omitted phrase list.".to_string())
        })?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    let selected_terms = validate_selected_omitted_terms(&request.selected_terms, &omitted_terms)?;

    let analysis: JobAnalysis = serde_json::from_value(stored.analysis.clone())
        .map_err(|error| AppError::Message(format!("Stored job analysis is invalid: {error}")))?;
    let selected_evidence = selected_terms
        .iter()
        .map(|term| SelectedEvidence {
            term: term.clone(),
            kind: infer_selected_term_kind(&analysis, term),
            proof_note: None,
            allow_model_role_placement: true,
        })
        .collect::<Vec<_>>();
    let bank = save_selected_evidence(&root, &selected_evidence)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let base_resume = load_base_resume(&root, &request.language)
        .map_err(|error| AppError::Message(error.to_string()))?;
    let preflight = preflight_items(&analysis, &base_resume, &bank);
    let mut approved_evidence = approved_evidence_for(&preflight, &bank, &selected_evidence);
    append_banked_terms(&mut approved_evidence, &bank, &selected_terms);
    let bullet_keyword_emphasis: BulletKeywordEmphasis =
        serde_json::from_value(stored.resume["bullet_keyword_emphasis"].clone())
            .unwrap_or_default();
    let reporter = |event: PipelineProgress| {
        if let Err(error) = app.emit("resume-pipeline-progress", event) {
            eprintln!("[pipeline] Failed to emit progress event: {error}");
        }
    };
    reporter(PipelineProgress {
        stage: "resume_tailoring",
        status: "started",
        message: format!(
            "Re-tailoring with {} selected user-attested claim(s).",
            selected_terms.len()
        ),
        attempt: None,
        total_attempts: None,
    });
    let mut response = tailor_and_render_with_progress(
        TailorRequest {
            language: request.language.clone(),
            parsed: captured.parsed,
            analysis: analysis.clone(),
            approved_evidence,
            priority_attested_terms: selected_terms.clone(),
            bullet_keyword_emphasis,
        },
        Some(&reporter),
    )
    .await
    .map_err(|error| AppError::Message(error.to_string()))?;
    response.retailor = Some(RetailorMetadata {
        source_variant_slug: request.source_variant_slug,
        source_ats_score: source_score,
        selected_terms,
    });
    if response.tailoring_status == "partial" && response.artifact.is_some() {
        let downloads_path = response
            .artifact
            .as_ref()
            .map(|artifact| PathBuf::from(&artifact.downloads_path))
            .expect("partial artifact checked above");
        match launch_path(&downloads_path, false) {
            Ok(()) => response.docx_opened = true,
            Err(error) => response.docx_open_error = Some(error.to_string()),
        }
    }
    store_and_emit_result(
        &app,
        &root,
        capture_id,
        &request.language,
        &analysis,
        &response,
    )?;
    Ok(response)
}

const RESULT_SCHEMA_VERSION: u8 = 2;

pub(crate) fn failure_summary(stage: &str, error: &str, analysis: Option<&JobAnalysis>) -> String {
    match analysis {
        Some(analysis) => format!(
            "{} The run then failed during {}: {}",
            analysis.summary,
            stage.replace('_', " "),
            error
        ),
        None => format!(
            "No AI analysis was produced. The run failed during {}: {}",
            stage.replace('_', " "),
            error
        ),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredPipelineResult {
    pub schema_version: u8,
    pub capture_received_at_ms: u64,
    pub language: String,
    pub recovered_from_artifacts: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub failed_stage: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub analysis: serde_json::Value,
    #[serde(default)]
    pub resume: serde_json::Value,
}

fn normalize_stored_result(mut result: StoredPipelineResult) -> StoredPipelineResult {
    if result.summary.trim().is_empty() {
        result.summary = result.analysis["summary"]
            .as_str()
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("A previous run completed, but its summary was unavailable.")
            .to_string();
    }
    if result.status.trim().is_empty() {
        result.status = result.resume["tailoring_status"]
            .as_str()
            .unwrap_or_else(|| {
                if result.analysis.is_null() {
                    "failed"
                } else {
                    "analysis_ready"
                }
            })
            .to_string();
    }
    result
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

#[tauri::command(async)]
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
    fs::write(&path, format!("{json}\n")).map_err(|error| AppError::Message(error.to_string()))?;
    eprintln!(
        "[ui-result] render diagnostic saved path={}",
        path.display()
    );
    Ok(())
}

fn result_snapshot_path(root: &Path, capture_id: u64, language: &str) -> PathBuf {
    root.join("data")
        .join("tailoring-results")
        .join(format!("{capture_id}-{language}.json"))
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

pub(crate) fn store_and_emit_outcome(
    app: &AppHandle,
    root: &Path,
    capture_id: u64,
    language: &str,
    status: &str,
    summary: String,
    analysis: Option<&JobAnalysis>,
    resume: Option<&TailorResponse>,
    failed_stage: Option<&str>,
    error: Option<String>,
) -> Result<StoredPipelineResult, AppError> {
    let result = StoredPipelineResult {
        schema_version: RESULT_SCHEMA_VERSION,
        capture_received_at_ms: capture_id,
        language: language.to_string(),
        recovered_from_artifacts: false,
        status: status.to_string(),
        summary,
        failed_stage: failed_stage.map(str::to_string),
        error,
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

fn store_and_emit_result(
    app: &AppHandle,
    root: &Path,
    capture_id: u64,
    language: &str,
    analysis: &JobAnalysis,
    resume: &TailorResponse,
) -> Result<StoredPipelineResult, AppError> {
    store_and_emit_outcome(
        app,
        root,
        capture_id,
        language,
        resume.tailoring_status,
        analysis.summary.clone(),
        Some(analysis),
        Some(resume),
        (resume.tailoring_status == "failed").then_some("resume_tailoring"),
        resume.error.clone(),
    )
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
        .filter_map(|entry| {
            let variant = entry.path().join("variant.json");
            modified_ms(&variant).map(|modified| (modified, entry.path()))
        })
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
    let base =
        load_base_resume(root, language).map_err(|error| AppError::Message(error.to_string()))?;
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
    let replaced_any_bullet = report
        .bullet_rewrite_decisions
        .iter()
        .any(|decision| decision.outcome == BulletRewriteOutcome::Replaced);
    let emphasis = if replaced_any_bullet {
        "max"
    } else if total_bullets > 0 && experience_bullets_changed == total_bullets {
        "high"
    } else {
        "balanced"
    };
    let docx_path = variant_dir.join(format!("Xevier_T_CV_{language}.docx"));
    let pdf_path = variant_dir.join(format!("Xevier_T_CV_{language}.pdf"));
    let current_artifact = |path: &Path| {
        path.is_file()
            && modified_ms(path)
                .map(|modified| modified >= u128::from(capture_id))
                .unwrap_or(false)
    };
    let pdf_ready = current_artifact(&pdf_path);
    let docx_ready = current_artifact(&docx_path);
    let summary = format!(
        "Tailoring completed with {} measured ATS coverage. {} job keywords were covered; {} were not.",
        report
            .ats_coverage
            .as_ref()
            .map(|coverage| coverage.score)
            .unwrap_or(report.model_estimated_ats_coverage_score),
        report.covered_keywords.len(),
        report.omitted_unsupported_keywords.len()
    );
    let result = StoredPipelineResult {
        schema_version: RESULT_SCHEMA_VERSION,
        capture_received_at_ms: capture_id,
        language: language.to_string(),
        recovered_from_artifacts: true,
        status: if pdf_ready { "completed" } else { "partial" }.to_string(),
        summary: summary.clone(),
        failed_stage: None,
        error: if pdf_ready {
            None
        } else {
            Some("Recovered tailoring data; PDF was not available.".to_string())
        },
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
            "latest_docx_path": docx_ready.then(|| relative_path(root, &docx_path)),
            "pdf_path": pdf_ready.then(|| relative_path(root, &pdf_path)),
            "latest_pdf_path": pdf_ready.then(|| relative_path(root, &pdf_path)),
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

fn latest_variant_slug(language: &str, extension: &str) -> Result<String, AppError> {
    if !matches!(language, "en" | "fr") {
        return Err(AppError::Message("Language must be en or fr.".to_string()));
    }
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let variants = root.join("resume").join("variants");
    let file_name = format!("Xevier_T_CV_{language}.{extension}");
    fs::read_dir(&variants)
        .map_err(|error| AppError::Message(error.to_string()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let artifact = entry.path().join(&file_name);
            artifact.is_file().then_some((artifact, entry.file_name()))
        })
        .filter_map(|(artifact, slug)| {
            modified_ms(&artifact).map(|modified| (modified, slug.to_string_lossy().to_string()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, slug)| slug)
        .ok_or_else(|| {
            AppError::Message(format!(
                "No verified {} variant exists for this language yet.",
                extension.to_uppercase()
            ))
        })
}

fn publish_and_launch_variant(
    variant_slug: &str,
    format: &str,
    reveal: bool,
) -> Result<ArtifactProvenance, AppError> {
    let root = workspace_root().map_err(|error| AppError::Message(error.to_string()))?;
    let artifact = publish_variant_artifact(&root, variant_slug, format)
        .map_err(|error| AppError::Message(error.to_string()))?;
    launch_path(Path::new(&artifact.downloads_path), reveal)?;
    Ok(artifact)
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
    let slug = latest_variant_slug(&language, "pdf")?;
    publish_and_launch_variant(&slug, "pdf", false).map(|_| ())
}

#[tauri::command]
pub fn reveal_latest_pdf(language: String) -> Result<(), AppError> {
    let slug = latest_variant_slug(&language, "pdf")?;
    publish_and_launch_variant(&slug, "pdf", true).map(|_| ())
}

#[tauri::command]
pub fn open_latest_docx(language: String) -> Result<(), AppError> {
    let slug = latest_variant_slug(&language, "docx")?;
    publish_and_launch_variant(&slug, "docx", false).map(|_| ())
}

#[tauri::command]
pub fn reveal_latest_docx(language: String) -> Result<(), AppError> {
    let slug = latest_variant_slug(&language, "docx")?;
    publish_and_launch_variant(&slug, "docx", true).map(|_| ())
}

#[tauri::command]
pub fn open_result_artifact(
    variant_slug: String,
    format: String,
) -> Result<ArtifactProvenance, AppError> {
    publish_and_launch_variant(&variant_slug, &format, false)
}

#[tauri::command]
pub fn reveal_result_artifact(
    variant_slug: String,
    format: String,
) -> Result<ArtifactProvenance, AppError> {
    publish_and_launch_variant(&variant_slug, &format, true)
}

#[cfg(test)]
mod tests {
    use super::{
        failure_summary, normalize_stored_result, persist_pipeline_result,
        prepare_preflight_result, recover_pipeline_result, validate_selected_omitted_terms,
        StoredPipelineResult,
    };
    use crate::analysis::JobAnalysis;
    use crate::tailoring::TailoringReport;
    use serde_json::json;

    fn temporary_root(label: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("resume-{label}-{suffix}"))
    }

    fn sample_analysis() -> JobAnalysis {
        serde_json::from_value(json!({
            "role_target": "Backend Engineer",
            "seniority": "mid",
            "core_keywords": [],
            "required_skills": ["Rust"],
            "preferred_skills": [],
            "tools_and_platforms": [],
            "domain_terms": [],
            "responsibility_phrases": [],
            "achievement_angles": [],
            "ats_phrase_bank": [],
            "must_not_claim_without_evidence": [],
            "summary": "Rust backend role"
        }))
        .unwrap()
    }

    #[test]
    fn prepares_language_specific_evidence_without_reanalyzing_the_job() {
        let root = temporary_root("preflight-language");
        let content = root.join("resume").join("content");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(
            content.join("base.fr.json"),
            r#"{"skills":{"programming":"Rust"}}"#,
        )
        .unwrap();

        let result = prepare_preflight_result(&root, "fr", sample_analysis()).unwrap();

        assert_eq!(result.analysis.summary, "Rust backend role");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].term, "Rust");
        assert_eq!(result.items[0].source, "base_resume");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_an_unknown_preflight_language_before_reading_resume_files() {
        let root = temporary_root("preflight-invalid-language");
        let error = match prepare_preflight_result(&root, "de", sample_analysis()) {
            Ok(_) => panic!("unknown language unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Language must be en or fr"));
    }

    #[test]
    fn persists_a_capture_matched_result_snapshot() {
        let root = temporary_root("stored-pipeline-result");
        let result = StoredPipelineResult {
            schema_version: 2,
            capture_received_at_ms: 1234,
            language: "en".to_string(),
            recovered_from_artifacts: false,
            status: "completed".to_string(),
            summary: "Summary".to_string(),
            failed_stage: None,
            error: None,
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
    fn normalizes_a_legacy_result_into_a_visible_summary() {
        let legacy = r#"{
            "schema_version": 1,
            "capture_received_at_ms": 1234,
            "language": "fr",
            "recovered_from_artifacts": false,
            "analysis": { "summary": "Legacy ATS summary" },
            "resume": { "tailoring_status": "partial" }
        }"#;
        let stored: StoredPipelineResult = serde_json::from_str(legacy).unwrap();
        let normalized = normalize_stored_result(stored);

        assert_eq!(normalized.status, "partial");
        assert_eq!(normalized.summary, "Legacy ATS summary");
    }

    #[test]
    fn validates_and_deduplicates_selected_omitted_terms() {
        let selected = vec![" Angular ".to_string(), "angular".to_string()];
        let validated = validate_selected_omitted_terms(&selected, &["Angular", "GCP"]).unwrap();
        assert_eq!(validated, vec!["Angular"]);

        let error =
            validate_selected_omitted_terms(&["Kubernetes".to_string()], &["Angular", "GCP"])
                .unwrap_err();
        assert!(error.to_string().contains("not in the source result"));
    }

    #[test]
    fn failure_summary_is_honest_when_analysis_did_not_complete() {
        let summary = failure_summary("ats_analysis", "request timed out", None);

        assert!(summary.starts_with("No AI analysis was produced."));
        assert!(summary.contains("request timed out"));
    }

    #[test]
    fn failure_summary_keeps_real_analysis_for_a_downstream_failure() {
        let analysis: JobAnalysis = serde_json::from_value(json!({
            "role_target": "Backend Engineer",
            "seniority": "Senior",
            "core_keywords": [],
            "required_skills": [],
            "preferred_skills": [],
            "tools_and_platforms": [],
            "domain_terms": [],
            "responsibility_phrases": [],
            "achievement_angles": [],
            "ats_phrase_bank": [],
            "must_not_claim_without_evidence": [],
            "summary": "Prioritize reliable backend delivery."
        }))
        .unwrap();
        let summary = failure_summary("docx_render", "LibreOffice failed", Some(&analysis));

        assert!(summary.starts_with("Prioritize reliable backend delivery."));
        assert!(summary.contains("LibreOffice failed"));
    }

    #[test]
    fn recovers_summary_and_changes_from_existing_variant_artifacts() {
        let root = temporary_root("artifact-result-recovery");
        let content_dir = root.join("resume/content");
        let variant_dir = root.join("resume/variants/2026-08-16-example-role-en");
        std::fs::create_dir_all(&content_dir).unwrap();
        std::fs::create_dir_all(&variant_dir).unwrap();
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
                model_estimated_ats_coverage_score: 81,
                ats_coverage: None,
                bullet_rewrite_decisions: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(variant_dir.join("Xevier_T_CV_en.docx"), b"docx").unwrap();
        std::fs::write(variant_dir.join("Xevier_T_CV_en.pdf"), b"pdf").unwrap();

        let recovered = recover_pipeline_result(&root, 0, "en").unwrap().unwrap();

        assert!(recovered.recovered_from_artifacts);
        assert_eq!(
            recovered.resume["report"]["estimated_ats_coverage_score"],
            81
        );
        assert_eq!(recovered.resume["experience_bullets_changed"], 1);
        assert_eq!(
            recovered.resume["content_changes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            recovered.resume["latest_pdf_path"],
            "resume/variants/2026-08-16-example-role-en/Xevier_T_CV_en.pdf"
        );
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
