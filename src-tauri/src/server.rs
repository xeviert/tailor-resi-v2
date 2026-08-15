use crate::analysis::{analyze_job, AnalysisConfig, JobAnalysis};
use crate::tailoring::{failed_response, tailor_and_render, BulletKeywordEmphasis, TailorRequest, TailorResponse};
use axum::{
    extract::State,
    http::{Method, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Serialize)]
struct AnalyzeResponse {
    success: bool,
    message: &'static str,
    parsed: serde_json::Value,
    analysis_status: &'static str,
    analysis: Option<JobAnalysis>,
    analysis_error: Option<String>,
    tailoring_status: &'static str,
    variant_slug: Option<String>,
    variant_json_path: Option<String>,
    docx_path: Option<String>,
    report_json_path: Option<String>,
    validation_status: &'static str,
    fit_status: &'static str,
    page_count: Option<u32>,
    tailoring_error: Option<String>,
}

#[derive(Clone)]
struct AppState {
    app_handle: AppHandle,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CapturedJob {
    pub received_at_ms: u128,
    pub payload: serde_json::Value,
    pub parsed: serde_json::Value,
}

#[derive(Serialize)]
struct CaptureResponse {
    success: bool,
    message: &'static str,
    capture_path: String,
    parsed: serde_json::Value,
}

fn collect_names(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

fn collect_display(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|x| x["displayName"].as_str().map(String::from))
        .collect()
}

fn collect_label(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|x| x["label"].as_str().map(String::from))
        .collect()
}

fn parse_wellfound(payload: &serde_json::Value) -> serde_json::Value {
    let job = &payload["json"];
    let startup = &job["startup"];
    let mut warnings: Vec<String> = Vec::new();

    let company_size = startup["companySize"].as_str().map(|s| {
        match s {
            "SIZE_1_10" => "1-10",
            "SIZE_11_50" => "11-50",
            "SIZE_51_200" => "51-200",
            "SIZE_201_500" => "201-500",
            "SIZE_501_1000" => "501-1000",
            "SIZE_1001_PLUS" => "1001+",
            _ => s,
        }
        .to_string()
    });

    let job_type = match job["jobType"].as_str().unwrap_or("") {
        "full_time" => "Full-time",
        "part_time" => "Part-time",
        "contract" => "Contract",
        "internship" => "Internship",
        _ => "Unknown",
    }
    .to_string();

    let compensation = job["compensation"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);

    let company_hq = collect_display(&startup["locationTaggings"])
        .into_iter()
        .next();

    let title = job["title"].as_str().unwrap_or("");
    let description = job["description"].as_str().unwrap_or("");
    let company = startup["name"].as_str().unwrap_or("");

    if title.is_empty() {
        warnings.push("Missing field: title".into());
        eprintln!("[parser:wellfound] Warning: missing title");
    }
    if description.is_empty() {
        warnings.push("Missing field: description".into());
        eprintln!("[parser:wellfound] Warning: missing description");
    }
    if company.is_empty() {
        warnings.push("Missing field: company".into());
        eprintln!("[parser:wellfound] Warning: missing company");
    }

    serde_json::json!({
        "domain": "wellfound",
        "parsed": true,
        "listing_id": job["id"].as_str().unwrap_or(""),
        "url": payload["sourceUrl"].as_str().unwrap_or(""),
        "title": title,
        "primary_role": job["primaryRoleTitle"].as_str().unwrap_or(""),
        "description": description,
        "compensation": compensation,
        "equity": job["equity"],
        "job_type": job_type,
        "remote": job["remote"].as_bool().unwrap_or(false),
        "locations": collect_names(&job["locationNames"]),
        "remote_locations": collect_names(&job["acceptedRemoteLocationNames"]),
        "allow_relocation": job["allowRelocation"].as_bool().unwrap_or(false),
        "visa_sponsorship": job["visaSponsorship"].as_bool().unwrap_or(false),
        "years_experience_min": job["yearsExperienceMin"],
        "years_experience_max": job["yearsExperienceMax"],
        "skills": collect_names(&job["skills"]),
        "company": company,
        "company_logo": startup["logoUrl"].as_str(),
        "company_description": startup["highConcept"].as_str(),
        "company_size": company_size,
        "company_hq": company_hq,
        "company_tags": collect_display(&startup["marketTaggings"]),
        "company_type_tags": collect_display(&startup["companyTypeTaggings"]),
        "company_badges": collect_label(&startup["badges"]),
        "warnings": warnings,
    })
}

fn parse_wttj(payload: &serde_json::Value) -> serde_json::Value {
    let job = &payload["json"];
    let org = &job["hiringOrganization"];
    let org_addr = &org["address"];
    let mut warnings: Vec<String> = Vec::new();

    let job_type = match job["employmentType"].as_str().unwrap_or("") {
        "FULL_TIME" => "Full-time",
        "PART_TIME" => "Part-time",
        "CONTRACTOR" => "Contract",
        "INTERN" => "Internship",
        "TEMPORARY" => "Temporary",
        _ => "Unknown",
    }
    .to_string();

    let locations: Vec<String> = job["jobLocation"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|loc| {
            let city = loc["address"]["addressLocality"].as_str()?;
            let country = loc["address"]["addressCountry"].as_str().unwrap_or("");
            Some(if country.is_empty() {
                city.to_string()
            } else {
                format!("{}, {}", city, country)
            })
        })
        .collect();

    let company_hq = org_addr["addressLocality"].as_str().map(|city| {
        let country = org_addr["addressCountry"].as_str().unwrap_or("");
        if country.is_empty() {
            city.to_string()
        } else {
            format!("{}, {}", city, country)
        }
    });

    let industry_tags: Vec<String> = job["industry"]
        .as_str()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let title = job["title"].as_str().unwrap_or("");
    let description_html = job["description"].as_str().unwrap_or("");
    let company = org["name"].as_str().unwrap_or("");

    if title.is_empty() {
        warnings.push("Missing field: title".into());
        eprintln!("[parser:wttj] Warning: missing title");
    }
    if description_html.is_empty() {
        warnings.push("Missing field: description_html".into());
        eprintln!("[parser:wttj] Warning: missing description_html");
    }
    if company.is_empty() {
        warnings.push("Missing field: company".into());
        eprintln!("[parser:wttj] Warning: missing company");
    }

    serde_json::json!({
        "domain": "welcometothejungle",
        "parsed": true,
        "url": payload["sourceUrl"].as_str().unwrap_or(""),
        "title": title,
        "description_html": description_html,
        "qualifications": job["qualifications"].as_str(),
        "job_type": job_type,
        "locations": locations,
        "date_posted": job["datePosted"].as_str(),
        "valid_through": job["validThrough"].as_str(),
        "education_requirement": job["educationRequirements"]["credentialCategory"].as_str(),
        "company": company,
        "company_logo": org["logo"].as_str(),
        "company_website": org["sameAs"].as_str(),
        "company_hq": company_hq,
        "industry_tags": industry_tags,
        "warnings": warnings,
    })
}

fn parse_indeed(payload: &serde_json::Value) -> serde_json::Value {
    let job = &payload["json"];
    let mut warnings: Vec<String> = Vec::new();

    let raw_title = job["title"].as_str().unwrap_or("");
    let title = raw_title.trim_end_matches(" - job post").trim().to_string();

    let description = job["description"].as_str().unwrap_or("");
    let company = job["company"].as_str().unwrap_or("");

    if title.is_empty() {
        warnings.push("Missing field: title".into());
        eprintln!("[parser:indeed] Warning: missing title");
    }
    if description.is_empty() {
        warnings.push("Missing field: description".into());
        eprintln!("[parser:indeed] Warning: missing description");
    }
    if company.is_empty() {
        warnings.push("Missing field: company".into());
        eprintln!("[parser:indeed] Warning: missing company");
    }

    serde_json::json!({
        "domain": "indeed",
        "parsed": true,
        "url": payload["sourceUrl"].as_str().unwrap_or(""),
        "title": title,
        "description": description,
        "location": job["location"].as_str(),
        "company": company,
        "warnings": warnings,
    })
}

pub(crate) fn parse_job_data(payload: &serde_json::Value) -> serde_json::Value {
    let source_url = payload["sourceUrl"].as_str().unwrap_or("");
    if source_url.contains("wellfound.com") {
        parse_wellfound(payload)
    } else if source_url.contains("welcometothejungle.com") {
        parse_wttj(payload)
    } else if source_url.contains("indeed.com") {
        parse_indeed(payload)
    } else {
        let job = &payload["json"];
        let title = job["title"].as_str().unwrap_or("");
        let description = job["description"]
            .as_str()
            .or_else(|| job["jobDescription"].as_str())
            .unwrap_or("");
        let company = job["company"]
            .as_str()
            .or_else(|| job["hiringOrganization"]["name"].as_str())
            .unwrap_or("");
        let mut warnings = Vec::new();
        if title.is_empty() {
            warnings.push("Missing field: title".to_string());
        }
        if description.is_empty() {
            warnings.push("Missing field: description".to_string());
        }
        if company.is_empty() {
            warnings.push("Missing field: company".to_string());
        }
        serde_json::json!({
            "domain": "unknown",
            "parsed": false,
            "url": source_url,
            "title": title,
            "description": description,
            "company": company,
            "location": job["location"].as_str(),
            "warnings": warnings,
            "raw": payload
        })
    }
}

fn capture_directory() -> Result<PathBuf, String> {
    crate::tailoring::workspace_root()
        .map(|root| root.join("data").join("job-captures"))
        .map_err(|error| error.to_string())
}

pub fn persist_capture(payload: &serde_json::Value) -> Result<CapturedJob, String> {
    let received_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let captured = CapturedJob {
        received_at_ms,
        payload: payload.clone(),
        parsed: parse_job_data(payload),
    };
    let directory = capture_directory()?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&captured).map_err(|error| error.to_string())?;
    fs::write(directory.join(format!("{received_at_ms}-job.json")), &text)
        .map_err(|error| error.to_string())?;
    write_latest_capture(&directory.join("latest.json"), &format!("{text}\n"))?;
    Ok(captured)
}

fn write_latest_capture(path: &std::path::Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Capture path has no parent: {}", path.display()))?;
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temporary_path = parent.join(format!(
        ".latest-{}-{unique_suffix}.tmp",
        std::process::id()
    ));

    fs::write(&temporary_path, text).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.to_string());
    }
    Ok(())
}

fn parse_latest_capture(text: &str) -> Result<Option<CapturedJob>, serde_json::Error> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(text).map(Some)
}

pub fn load_latest_capture() -> Result<Option<CapturedJob>, String> {
    let path = capture_directory()?.join("latest.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    match parse_latest_capture(&text) {
        Ok(capture) => Ok(capture),
        Err(error) => {
            eprintln!(
                "[capture] Ignoring invalid latest capture at {}: {error}",
                path.display()
            );
            Ok(None)
        }
    }
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "app": "resi-tailor",
        "bridge": "tauri-rust"
    }))
}

async fn capture_handler(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    match persist_capture(&payload) {
        Ok(captured) => {
            if let Err(error) = state.app_handle.emit("job-data-received", &captured) {
                eprintln!("[server] Failed to emit capture event: {error}");
            }
            (
                StatusCode::OK,
                Json(serde_json::json!(CaptureResponse {
                    success: true,
                    message: "Job data captured",
                    capture_path: format!("data/job-captures/{}-job.json", captured.received_at_ms),
                    parsed: captured.parsed,
                })),
            )
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "success": false, "error": error })),
        ),
    }
}

async fn analyze_handler(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<AnalyzeResponse>) {
    let parsed = parse_job_data(&payload);
    if let Err(e) = state.app_handle.emit("job-data-received", &parsed) {
        eprintln!("[server] Failed to emit event: {e}");
    }

    let (analysis_status, analysis, analysis_error) = match AnalysisConfig::from_env() {
        Some(config) => match analyze_job(&config, &parsed).await {
            Ok(analysis) => {
                let event_payload = serde_json::json!({
                    "parsed": parsed.clone(),
                    "analysis": analysis.clone(),
                });
                if let Err(e) = state
                    .app_handle
                    .emit("job-analysis-received", event_payload)
                {
                    eprintln!("[server] Failed to emit analysis event: {e}");
                }
                ("completed", Some(analysis), None)
            }
            Err(error) => {
                let message = error.to_string();
                eprintln!("[analysis] {message}");
                ("failed", None, Some(message))
            }
        },
        None => ("skipped_no_api_key", None, None),
    };

    let (
        tailoring_status,
        variant_slug,
        variant_json_path,
        docx_path,
        report_json_path,
        validation_status,
        fit_status,
        page_count,
        tailoring_error,
    ) = match analysis.clone() {
        Some(analysis) => {
            let request = TailorRequest {
                language: "en".to_string(),
                parsed: parsed.clone(),
                analysis,
                approved_evidence: vec![],
                bullet_keyword_emphasis: BulletKeywordEmphasis::Balanced,
            };
            match tailor_and_render(request).await {
                Ok(response) => {
                    if let Err(e) = state.app_handle.emit("resume-tailored", &response) {
                        eprintln!("[server] Failed to emit resume-tailored event: {e}");
                    }
                    (
                        response.tailoring_status,
                        response.variant_slug,
                        response.variant_json_path,
                        response.docx_path,
                        response.report_json_path,
                        response.validation_status,
                        response.fit_status,
                        response.page_count,
                        response.error,
                    )
                }
                Err(error) => {
                    let message = error.to_string();
                    eprintln!("[tailoring] {message}");
                    (
                        "failed",
                        None,
                        None,
                        None,
                        None,
                        "not_run",
                        "not_run",
                        None,
                        Some(message),
                    )
                }
            }
        }
        None => (
            "not_run", None, None, None, None, "not_run", "not_run", None, None,
        ),
    };

    (
        StatusCode::OK,
        Json(AnalyzeResponse {
            success: true,
            message: "Job data received",
            parsed,
            analysis_status,
            analysis,
            analysis_error,
            tailoring_status,
            variant_slug,
            variant_json_path,
            docx_path,
            report_json_path,
            validation_status,
            fit_status,
            page_count,
            tailoring_error,
        }),
    )
}

async fn tailor_handler(
    State(state): State<AppState>,
    Json(request): Json<TailorRequest>,
) -> (StatusCode, Json<TailorResponse>) {
    match tailor_and_render(request).await {
        Ok(response) => {
            if let Err(e) = state.app_handle.emit("resume-tailored", &response) {
                eprintln!("[server] Failed to emit resume-tailored event: {e}");
            }
            (StatusCode::OK, Json(response))
        }
        Err(error) => {
            let message = error.to_string();
            eprintln!("[tailoring] {message}");
            (StatusCode::BAD_REQUEST, Json(failed_response(message)))
        }
    }
}

async fn ollama_proxy_handler(
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Read optional _baseUrl from payload, default to localhost:11434
    let base_url = payload
        .get("_baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("http://localhost:11434");

    let model_name = payload.get("model").and_then(|v| v.as_str()).unwrap_or("?");

    eprintln!(
        "[ollama-proxy] Forwarding to {}/api/chat, model={}",
        base_url, model_name
    );

    // Strip _baseUrl from the forwarded payload
    let mut forward = payload.clone();
    if let Some(obj) = forward.as_object_mut() {
        obj.remove("_baseUrl");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    match client
        .post(format!("{base_url}/api/chat"))
        .json(&forward)
        .send()
        .await
    {
        Ok(resp) => {
            let resp_status = resp.status();
            eprintln!("[ollama-proxy] Ollama response status: {}", resp_status);
            let status = if resp_status.is_success() {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            };
            match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    eprintln!(
                        "[ollama-proxy] Success, response size: {} bytes",
                        serde_json::to_string(&body).unwrap_or_default().len()
                    );
                    (status, Json(body))
                }
                Err(e) => {
                    eprintln!("[ollama-proxy] Failed to parse response: {e}");
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                }
            }
        }
        Err(e) => {
            eprintln!("[ollama-proxy] Request error: {e}");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        }
    }
}

pub async fn start_server(app_handle: AppHandle) {
    let state = AppState { app_handle };

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            let s = origin.as_bytes();
            s.starts_with(b"chrome-extension://")
                || s.starts_with(b"http://localhost")
                || s.starts_with(b"http://127.0.0.1")
                || s.starts_with(b"tauri://")
        }))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/captures", post(capture_handler))
        .route("/analyze", post(analyze_handler))
        .route("/tailor", post(tailor_handler))
        .route("/api/ollama", post(ollama_proxy_handler))
        .layer(cors)
        .with_state(state.clone());

    match tokio::net::TcpListener::bind("127.0.0.1:3000").await {
        Ok(listener) => {
            println!("[server] Listening on http://127.0.0.1:3000");
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("[server] Serve error: {e}");
            }
        }
        Err(e) => {
            // Non-fatal: app continues without the extension bridge
            eprintln!("[server] Failed to bind 127.0.0.1:3000 - {e}");
            let _ = state.app_handle.emit("server-start-failed", e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_job_data, parse_latest_capture, write_latest_capture, CapturedJob};
    use serde_json::json;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parses_indeed_payload() {
        let parsed = parse_job_data(&json!({
            "sourceUrl": "https://www.indeed.com/viewjob?jk=123",
            "json": {
                "title": "Senior Rust Engineer - job post",
                "company": "Acme",
                "location": "Remote",
                "description": "Build reliable systems."
            }
        }));

        assert_eq!(parsed["domain"], "indeed");
        assert_eq!(parsed["parsed"], true);
        assert_eq!(parsed["title"], "Senior Rust Engineer");
        assert_eq!(parsed["company"], "Acme");
        assert_eq!(parsed["location"], "Remote");
        assert!(parsed["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn parses_wellfound_payload() {
        let parsed = parse_job_data(&json!({
            "sourceUrl": "https://wellfound.com/jobs/123",
            "json": {
                "id": "123",
                "title": "AI Engineer",
                "primaryRoleTitle": "Software Engineer",
                "description": "Build AI tooling.",
                "jobType": "full_time",
                "remote": true,
                "locationNames": ["Paris"],
                "acceptedRemoteLocationNames": ["Europe"],
                "skills": ["Rust", "TypeScript"],
                "startup": {
                    "name": "Acme AI",
                    "companySize": "SIZE_11_50",
                    "highConcept": "AI products",
                    "locationTaggings": [{ "displayName": "Paris" }],
                    "marketTaggings": [{ "displayName": "AI" }],
                    "companyTypeTaggings": [{ "displayName": "SaaS" }],
                    "badges": [{ "label": "Hiring" }]
                }
            }
        }));

        assert_eq!(parsed["domain"], "wellfound");
        assert_eq!(parsed["parsed"], true);
        assert_eq!(parsed["company"], "Acme AI");
        assert_eq!(parsed["company_size"], "11-50");
        assert_eq!(parsed["remote"], true);
        assert_eq!(parsed["skills"][0], "Rust");
    }

    #[test]
    fn parses_welcome_to_the_jungle_payload() {
        let parsed = parse_job_data(&json!({
            "sourceUrl": "https://www.welcometothejungle.com/en/companies/acme/jobs/rust-engineer",
            "json": {
                "title": "Rust Engineer",
                "description": "<p>Build services.</p>",
                "qualifications": "Rust experience",
                "employmentType": "FULL_TIME",
                "industry": "Software, AI",
                "datePosted": "2026-08-14",
                "hiringOrganization": {
                    "name": "Acme",
                    "logo": "https://example.com/logo.png",
                    "sameAs": "https://example.com",
                    "address": {
                        "addressLocality": "Paris",
                        "addressCountry": "France"
                    }
                },
                "jobLocation": [{
                    "address": {
                        "addressLocality": "Paris",
                        "addressCountry": "France"
                    }
                }]
            }
        }));

        assert_eq!(parsed["domain"], "welcometothejungle");
        assert_eq!(parsed["parsed"], true);
        assert_eq!(parsed["company"], "Acme");
        assert_eq!(parsed["locations"][0], "Paris, France");
        assert_eq!(parsed["industry_tags"][1], "AI");
    }

    #[test]
    fn unknown_domain_keeps_raw_payload() {
        let payload = json!({
            "sourceUrl": "https://example.com/jobs/123",
            "json": { "title": "Engineer" }
        });
        let parsed = parse_job_data(&payload);

        assert_eq!(parsed["domain"], "unknown");
        assert_eq!(parsed["parsed"], false);
        assert_eq!(parsed["raw"], payload);
    }

    #[test]
    fn captured_job_round_trips_with_normalized_data() {
        let captured = CapturedJob {
            received_at_ms: 42,
            payload: json!({ "sourceUrl": "https://example.com" }),
            parsed: json!({ "title": "Engineer" }),
        };
        let serialized = serde_json::to_string(&captured).unwrap();
        let restored: CapturedJob = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.received_at_ms, 42);
        assert_eq!(restored.parsed["title"], "Engineer");
    }

    #[test]
    fn blank_or_invalid_latest_capture_is_treated_as_missing() {
        assert!(parse_latest_capture(" \n\t ").unwrap().is_none());
        assert!(parse_latest_capture("not json").is_err());
        assert!(parse_latest_capture(r#"{"receivedAt":"2026-08-14T12:33:44Z","json":{}}"#).is_err());
    }

    #[test]
    fn latest_capture_write_replaces_complete_file_without_temp_artifacts() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("resi-tailor-capture-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let latest = directory.join("latest.json");

        write_latest_capture(&latest, "{\"version\":1}\n").unwrap();
        write_latest_capture(&latest, "{\"version\":2}\n").unwrap();

        assert_eq!(fs::read_to_string(&latest).unwrap(), "{\"version\":2}\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
