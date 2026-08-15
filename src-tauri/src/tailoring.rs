use crate::{analysis::JobAnalysis, evidence::EvidenceEntry};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

const MAX_COMPANY_ROLE_SLUG_LEN: usize = 64;
const MAX_TAILORING_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Serialize)]
pub struct PipelineProgress {
    pub stage: &'static str,
    pub status: &'static str,
    pub message: String,
    pub attempt: Option<usize>,
    pub total_attempts: Option<usize>,
}

type ProgressReporter<'a> = Option<&'a (dyn Fn(PipelineProgress) + Send + Sync)>;

fn progress(
    reporter: ProgressReporter<'_>,
    stage: &'static str,
    status: &'static str,
    message: impl Into<String>,
    attempt: Option<usize>,
) {
    if let Some(reporter) = reporter {
        reporter(PipelineProgress {
            stage,
            status,
            message: message.into(),
            attempt,
            total_attempts: attempt.map(|_| MAX_TAILORING_ATTEMPTS),
        });
    }
}

#[derive(Clone, Debug)]
pub struct TailoringConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl TailoringConfig {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").ok()?;
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return None;
        }

        Some(Self {
            api_key,
            model: std::env::var("OPENAI_TAILOR_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "gpt-5.6-terra".to_string()),
            base_url: std::env::var("OPENAI_BASE_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TailoringReport {
    pub covered_keywords: Vec<String>,
    pub omitted_unsupported_keywords: Vec<String>,
    pub changed_fields: Vec<String>,
    pub safety_notes: Vec<String>,
    pub estimated_ats_coverage_score: u8,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TailoredResume {
    pub content: serde_json::Value,
    pub report: TailoringReport,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulletKeywordEmphasis {
    Low,
    #[default]
    Balanced,
    High,
}

impl BulletKeywordEmphasis {
    fn prompt_instruction(self) -> &'static str {
        match self {
            Self::Low => "Bullet keyword emphasis is LOW: add supported job language only to the strongest direct bullet matches.\n",
            Self::Balanced => "Bullet keyword emphasis is BALANCED: before relying on skills-section additions, spread one natural, supported job term or phrase across roughly half of the factually relevant experience bullets.\n",
            Self::High => "Bullet keyword emphasis is HIGH: spread one natural, supported job term or phrase across every factually relevant experience bullet where possible. Prioritize breadth across bullets over stacking multiple terms into a single bullet.\n",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TailorRequest {
    pub language: String,
    pub parsed: serde_json::Value,
    pub analysis: JobAnalysis,
    #[serde(default)]
    pub approved_evidence: Vec<EvidenceEntry>,
    #[serde(default)]
    pub bullet_keyword_emphasis: BulletKeywordEmphasis,
}

#[derive(Clone, Debug, Serialize)]
pub struct TailorResponse {
    pub success: bool,
    pub tailoring_status: &'static str,
    pub variant_slug: Option<String>,
    pub variant_json_path: Option<String>,
    pub docx_path: Option<String>,
    pub latest_docx_path: Option<String>,
    pub pdf_path: Option<String>,
    pub latest_pdf_path: Option<String>,
    pub downloads_pdf_path: Option<String>,
    pub downloads_error: Option<String>,
    pub report_json_path: Option<String>,
    pub validation_status: &'static str,
    pub fit_status: &'static str,
    pub page_count: Option<u32>,
    pub bullet_keyword_emphasis: BulletKeywordEmphasis,
    pub experience_bullets_changed: u32,
    pub report: Option<TailoringReport>,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TailoringError {
    #[error("Unsupported resume language: {0}")]
    UnsupportedLanguage(String),
    #[error("OpenAI request failed: {0}")]
    Request(String),
    #[error("OpenAI returned HTTP {status}: {body}")]
    Http { status: StatusCode, body: String },
    #[error("OpenAI response did not contain structured output text")]
    MissingOutputText,
    #[error("OpenAI returned an empty response body")]
    EmptyResponseBody,
    #[error("OpenAI returned empty structured output text")]
    EmptyOutputText,
    #[error("OpenAI tailoring JSON was invalid: {0}")]
    InvalidJson(String),
    #[error("Tailored resume failed safety validation: {0}")]
    InvalidTailoredContent(String),
    #[error("Could not locate repository root with resume/content")]
    MissingWorkspaceRoot,
    #[error("File operation failed: {0}")]
    Io(String),
    #[error("Resume render failed: {0}")]
    Render(String),
    #[error("Resume validation failed: {0}")]
    Validation(String),
    #[error("Resume did not fit on one page after {attempts} attempts: {page_counts:?}")]
    OnePageFit {
        attempts: usize,
        page_counts: Vec<u32>,
    },
    #[error("Resume one-page fit check failed: {0}")]
    Fit(String),
}

pub fn build_tailoring_prompt(
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    approved_evidence: &[EvidenceEntry],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
) -> String {
    let parsed_job = serde_json::to_string(parsed_job).unwrap_or_else(|_| "{}".to_string());
    let analysis = serde_json::to_string(analysis).unwrap_or_else(|_| "{}".to_string());
    let base_resume = serde_json::to_string(base_resume).unwrap_or_else(|_| "{}".to_string());
    let approved_evidence = serde_json::to_string(approved_evidence).unwrap_or_else(|_| "[]".to_string());

    let concise_instruction = if concise {
        "The preceding attempt overflowed to a second page. Keep every bullet and every factual claim, but rewrite the editable text more compactly: remove repetition, use concise verbs, and prefer compact ATS terminology. Do not shorten by deleting responsibilities or achievements.\n\n"
    } else {
        ""
    };

    format!(
        "Tailor this {language} resume JSON for maximum truthful ATS alignment.\n\
         Return only JSON matching the schema. Preserve the input resume shape exactly.\n\
         Rewrite only experience bullet text and skills strings.\n\
         Do not change meta, company names, locations, titles, dates, job order, number of jobs, number of bullets, or skill keys.\n\
         Aggressively incorporate ATS keywords, tools, responsibility phrases, and domain wording when the base resume supports them.\n\
         {bullet_emphasis_instruction}\
         User-attested evidence may support a skills string. Use it in an experience bullet only when its proof_note explicitly names a matching role or project; never infer a responsibility from a term alone.\n\
         Do not invent credentials, employers, tools, metrics, responsibilities, education, certifications, or experience.\n\
         Put important job keywords without base-resume or user-attested evidence into omitted_unsupported_keywords instead of adding them to the resume.\n\
         Keep each rewritten bullet close to the original length so the locked DOCX layout remains stable.\n\n\
         {concise_instruction}\
         Normalized job JSON:\n{parsed_job}\n\n\
         ATS analysis JSON:\n{analysis}\n\n\
         Base resume JSON:\n{base_resume}\n\n\
         User-attested evidence bank entries:\n{approved_evidence}",
        bullet_emphasis_instruction = bullet_keyword_emphasis.prompt_instruction(),
    )
}

fn resume_content_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["meta", "experience", "skills"],
        "properties": {
            "meta": {
                "type": "object",
                "additionalProperties": false,
                "required": ["language", "type", "template"],
                "properties": {
                    "language": { "type": "string" },
                    "type": { "type": "string" },
                    "template": { "type": "string" }
                }
            },
            "experience": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["company", "location", "title", "dates", "bullets"],
                    "properties": {
                        "company": { "type": "string" },
                        "location": { "type": "string" },
                        "title": { "type": "string" },
                        "dates": { "type": "string" },
                        "bullets": { "type": "array", "items": { "type": "string" } }
                    }
                }
            },
            "skills": {
                "type": "object",
                "additionalProperties": false,
                "required": ["frontend", "architecture_backend", "ai_data", "testing", "devops", "tools"],
                "properties": {
                    "frontend": { "type": "string" },
                    "architecture_backend": { "type": "string" },
                    "ai_data": { "type": "string" },
                    "testing": { "type": "string" },
                    "devops": { "type": "string" },
                    "tools": { "type": "string" }
                }
            }
        }
    })
}

fn tailoring_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["content", "report"],
        "properties": {
            "content": resume_content_schema(),
            "report": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "covered_keywords",
                    "omitted_unsupported_keywords",
                    "changed_fields",
                    "safety_notes",
                    "estimated_ats_coverage_score"
                ],
                "properties": {
                    "covered_keywords": { "type": "array", "items": { "type": "string" } },
                    "omitted_unsupported_keywords": { "type": "array", "items": { "type": "string" } },
                    "changed_fields": { "type": "array", "items": { "type": "string" } },
                    "safety_notes": { "type": "array", "items": { "type": "string" } },
                    "estimated_ats_coverage_score": { "type": "integer", "minimum": 0, "maximum": 100 }
                }
            }
        }
    })
}

pub fn build_tailoring_request(
    model: &str,
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    approved_evidence: &[EvidenceEntry],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": [
            {
                "role": "system",
                "content": "You rewrite resume JSON for ATS alignment. You must be truthful, evidence-bound, and preserve all locked layout constraints."
            },
            {
                "role": "user",
                "content": build_tailoring_prompt(language, parsed_job, analysis, base_resume, approved_evidence, bullet_keyword_emphasis, concise)
            }
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "tailored_resume",
                "strict": true,
                "schema": tailoring_schema()
            }
        }
    })
}

pub async fn tailor_resume(
    config: &TailoringConfig,
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    approved_evidence: &[EvidenceEntry],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
) -> Result<TailoredResume, TailoringError> {
    validate_language(language)?;
    let client = reqwest::Client::new();
    let request_body = build_tailoring_request(
        &config.model,
        language,
        parsed_job,
        analysis,
        base_resume,
        approved_evidence,
        bullet_keyword_emphasis,
        concise,
    );
    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));

    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .json(&request_body)
        .send()
        .await
        .map_err(|error| TailoringError::Request(error.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| TailoringError::Request(error.to_string()))?;

    if !status.is_success() {
        return Err(TailoringError::Http { status, body });
    }

    parse_tailored_resume_from_response(&body)
}

pub fn parse_tailored_resume_from_response(body: &str) -> Result<TailoredResume, TailoringError> {
    if body.trim().is_empty() {
        return Err(TailoringError::EmptyResponseBody);
    }
    let response: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    let text = find_output_text(&response).ok_or(TailoringError::MissingOutputText)?;
    if text.trim().is_empty() {
        return Err(TailoringError::EmptyOutputText);
    }
    serde_json::from_str(text).map_err(|error| TailoringError::InvalidJson(error.to_string()))
}

fn find_output_text(response: &serde_json::Value) -> Option<&str> {
    response["output"].as_array()?.iter().find_map(|item| {
        item["content"].as_array()?.iter().find_map(|content| {
            if content["type"].as_str() == Some("output_text") {
                content["text"].as_str()
            } else {
                None
            }
        })
    })
}

pub fn validate_tailored_content(
    language: &str,
    base: &serde_json::Value,
    tailored: &serde_json::Value,
) -> Result<(), TailoringError> {
    validate_language(language)?;

    if base["meta"]["language"] != tailored["meta"]["language"] {
        return invalid("meta.language changed");
    }
    if tailored["meta"]["language"].as_str() != Some(language) {
        return invalid("tailored meta.language does not match request language");
    }
    if base["meta"]["type"] != tailored["meta"]["type"] {
        return invalid("meta.type changed");
    }
    if base["meta"]["template"] != tailored["meta"]["template"] {
        return invalid("meta.template changed");
    }

    let base_experience = base["experience"]
        .as_array()
        .ok_or_else(|| invalid_message("base experience is not an array"))?;
    let tailored_experience = tailored["experience"]
        .as_array()
        .ok_or_else(|| invalid_message("tailored experience is not an array"))?;

    if base_experience.len() != tailored_experience.len() {
        return invalid("experience job count changed");
    }

    for (job_index, (base_job, tailored_job)) in base_experience
        .iter()
        .zip(tailored_experience.iter())
        .enumerate()
    {
        for field in ["company", "location", "title", "dates"] {
            if base_job[field] != tailored_job[field] {
                return invalid(&format!("experience.{job_index}.{field} changed"));
            }
        }

        let base_bullets = base_job["bullets"]
            .as_array()
            .ok_or_else(|| invalid_message("base bullets are not an array"))?;
        let tailored_bullets = tailored_job["bullets"]
            .as_array()
            .ok_or_else(|| invalid_message("tailored bullets are not an array"))?;

        if base_bullets.len() != tailored_bullets.len() {
            return invalid(&format!("experience.{job_index}.bullets count changed"));
        }
        if tailored_bullets.iter().any(|bullet| {
            bullet
                .as_str()
                .map(str::trim)
                .map(str::is_empty)
                .unwrap_or(true)
        }) {
            return invalid(&format!(
                "experience.{job_index}.bullets contains empty text"
            ));
        }
    }

    let base_keys = object_keys(&base["skills"])?;
    let tailored_keys = object_keys(&tailored["skills"])?;
    if base_keys != tailored_keys {
        return invalid("skills keys changed");
    }
    for key in tailored_keys {
        if tailored["skills"][&key]
            .as_str()
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true)
        {
            return invalid(&format!("skills.{key} is empty"));
        }
    }

    Ok(())
}

fn count_changed_experience_bullets(base: &serde_json::Value, tailored: &serde_json::Value) -> u32 {
    base["experience"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(
            tailored["experience"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .flat_map(|(base_job, tailored_job)| {
            base_job["bullets"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(
                    tailored_job["bullets"]
                        .as_array()
                        .into_iter()
                        .flatten(),
                )
        })
        .filter(|(base_bullet, tailored_bullet)| base_bullet != tailored_bullet)
        .count() as u32
}

fn object_keys(value: &serde_json::Value) -> Result<BTreeSet<String>, TailoringError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_message("value is not an object"))?;
    Ok(object.keys().cloned().collect())
}

fn validate_language(language: &str) -> Result<(), TailoringError> {
    match language {
        "en" | "fr" => Ok(()),
        other => Err(TailoringError::UnsupportedLanguage(other.to_string())),
    }
}

fn invalid<T>(message: &str) -> Result<T, TailoringError> {
    Err(invalid_message(message))
}

fn invalid_message(message: &str) -> TailoringError {
    TailoringError::InvalidTailoredContent(message.to_string())
}

pub fn workspace_root() -> Result<PathBuf, TailoringError> {
    let current = std::env::current_dir().map_err(|error| TailoringError::Io(error.to_string()))?;
    for candidate in current.ancestors() {
        if candidate.join("resume").join("content").is_dir() {
            return Ok(candidate.to_path_buf());
        }
    }
    Err(TailoringError::MissingWorkspaceRoot)
}

pub fn load_base_resume(root: &Path, language: &str) -> Result<serde_json::Value, TailoringError> {
    validate_language(language)?;
    let path = root
        .join("resume")
        .join("content")
        .join(format!("base.{language}.json"));
    let text =
        std::fs::read_to_string(&path).map_err(|error| TailoringError::Io(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| TailoringError::InvalidJson(error.to_string()))
}

pub fn company_role_slug(
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    language: &str,
) -> String {
    let company = parsed_job["company"].as_str().unwrap_or("unknown-company");
    let role = if analysis.role_target.trim().is_empty() {
        parsed_job["title"].as_str().unwrap_or("unknown-role")
    } else {
        analysis.role_target.as_str()
    };
    let base = slugify(&format!("{company}-{role}"));
    let language_suffix = format!("-{language}");
    let base_limit = MAX_COMPANY_ROLE_SLUG_LEN.saturating_sub(language_suffix.len());
    format!("{}{}", bounded_slug(&base, base_limit), language_suffix)
}

pub fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "tailored-resume".to_string()
    } else {
        slug
    }
}

fn bounded_slug(slug: &str, max_len: usize) -> String {
    if slug.len() <= max_len {
        return slug.to_string();
    }
    let hash = fnv1a_32(slug.as_bytes());
    let hash_suffix = format!("-{hash:08x}");
    let prefix_len = max_len.saturating_sub(hash_suffix.len());
    let prefix = slug[..prefix_len].trim_matches('-');
    format!("{prefix}{hash_suffix}")
}

fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn today_prefix() -> Result<String, TailoringError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| TailoringError::Io(error.to_string()))?
        .as_secs() as i64;
    let (year, month, day) = civil_date_from_days(seconds.div_euclid(86_400));
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

// Gregorian civil date from days since 1970-01-01, adapted from Howard Hinnant's
// public-domain algorithm. Keeping this local avoids a platform-specific shell call.
fn civil_date_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (
        year + if month <= 2 { 1 } else { 0 },
        month as u32,
        day as u32,
    )
}

pub fn write_variant_files(
    root: &Path,
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    tailored: &TailoredResume,
) -> Result<(String, PathBuf, PathBuf, PathBuf), TailoringError> {
    let variant_slug = format!(
        "{}-{}",
        today_prefix()?,
        company_role_slug(parsed_job, analysis, language)
    );
    let variant_dir = root.join("resume").join("variants").join(&variant_slug);
    std::fs::create_dir_all(&variant_dir).map_err(|error| TailoringError::Io(error.to_string()))?;

    let variant_json_path = variant_dir.join("variant.json");
    let report_json_path = variant_dir.join("tailoring-report.json");
    let docx_path = variant_dir.join(format!("Xevier_T_CV_{language}.docx"));

    write_json(&variant_json_path, &tailored.content)?;
    write_json(&report_json_path, &tailored.report)?;

    Ok((variant_slug, variant_json_path, report_json_path, docx_path))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TailoringError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    std::fs::write(path, format!("{json}\n")).map_err(|error| TailoringError::Io(error.to_string()))
}

fn render_resume(
    root: &Path,
    language: &str,
    variant_json_path: &Path,
    docx_path: &Path,
) -> Result<(), TailoringError> {
    let script = root
        .join("resume")
        .join("scripts")
        .join("ResumeWorkbench.ps1");

    let render = powershell_command()
        .arg("-File")
        .arg(&script)
        .arg("render")
        .arg("-Lang")
        .arg(language)
        .arg("-Content")
        .arg(variant_json_path)
        .arg("-Out")
        .arg(docx_path)
        .output()
        .map_err(|error| TailoringError::Render(error.to_string()))?;
    if !render.status.success() {
        return Err(TailoringError::Render(command_output(&render)));
    }

    Ok(())
}

fn validate_rendered_resume(
    root: &Path,
    language: &str,
    docx_path: &Path,
) -> Result<(), TailoringError> {
    let script = root
        .join("resume")
        .join("scripts")
        .join("ResumeWorkbench.ps1");
    let validate = powershell_command()
        .arg("-File")
        .arg(&script)
        .arg("validate")
        .arg("-Lang")
        .arg(language)
        .arg("-Docx")
        .arg(docx_path)
        .output()
        .map_err(|error| TailoringError::Validation(error.to_string()))?;
    if !validate.status.success() {
        return Err(TailoringError::Validation(command_output(&validate)));
    }

    Ok(())
}

fn command_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}{}", stdout, stderr).trim().to_string()
}

fn check_one_page_fit(
    root: &Path,
    docx_path: &Path,
    pdf_path: &Path,
) -> Result<u32, TailoringError> {
    let script = root
        .join("resume")
        .join("scripts")
        .join("ResumeWorkbench.ps1");
    let output = powershell_command()
        .args(["-File"])
        .arg(script)
        .arg("fit")
        .arg("-Docx")
        .arg(docx_path)
        .arg("-Out")
        .arg(pdf_path)
        .output()
        .map_err(|error| TailoringError::Fit(error.to_string()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let result = text
        .lines()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());
    let page_count = result
        .and_then(|value| value["page_count"].as_u64())
        .map(|count| count as u32);
    match (output.status.success(), page_count) {
        (true, Some(1)) => Ok(1),
        (false, Some(count)) => Err(TailoringError::OnePageFit {
            attempts: 1,
            page_counts: vec![count],
        }),
        _ => Err(TailoringError::Fit(command_output(&output))),
    }
}

fn publish_latest_docx(
    root: &Path,
    language: &str,
    docx_path: &Path,
) -> Result<PathBuf, TailoringError> {
    let latest_docx_path = root
        .join("resume")
        .join("generated")
        .join(format!("Xevier_T_CV_{language}.docx"));
    std::fs::create_dir_all(latest_docx_path.parent().unwrap())
        .map_err(|error| TailoringError::Io(error.to_string()))?;
    std::fs::copy(docx_path, &latest_docx_path)
        .map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(latest_docx_path)
}

fn downloads_pdf_path(language: &str) -> Result<PathBuf, TailoringError> {
    validate_language(language)?;
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| TailoringError::Io("Could not determine the user home directory for Downloads.".to_string()))?;
    Ok(PathBuf::from(home)
        .join("Downloads")
        .join(format!("Xevier_T_CV_{language}.pdf")))
}

fn publish_downloads_pdf(pdf_path: &Path, language: &str) -> Result<PathBuf, TailoringError> {
    let destination = downloads_pdf_path(language)?;
    std::fs::create_dir_all(destination.parent().expect("Downloads path has a parent"))
        .map_err(|error| TailoringError::Io(error.to_string()))?;
    std::fs::copy(pdf_path, &destination).map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(destination)
}

#[allow(clippy::too_many_arguments)]
fn partial_docx_response(
    root: &Path,
    language: &str,
    variant_slug: String,
    variant_json_path: &Path,
    report_json_path: &Path,
    docx_path: &Path,
    pdf_path: &Path,
    page_count: Option<u32>,
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    experience_bullets_changed: u32,
    report: TailoringReport,
    error: String,
) -> Result<TailorResponse, TailoringError> {
    let latest_docx_path = publish_latest_docx(root, language, docx_path)?;
    Ok(TailorResponse {
        success: false,
        tailoring_status: "partial",
        variant_slug: Some(variant_slug),
        variant_json_path: Some(relative_path(root, variant_json_path)),
        docx_path: Some(relative_path(root, docx_path)),
        latest_docx_path: Some(relative_path(root, &latest_docx_path)),
        pdf_path: pdf_path.exists().then(|| relative_path(root, pdf_path)),
        latest_pdf_path: None,
        downloads_pdf_path: None,
        downloads_error: None,
        report_json_path: Some(relative_path(root, report_json_path)),
        validation_status: "passed",
        fit_status: "failed",
        page_count,
        bullet_keyword_emphasis,
        experience_bullets_changed,
        report: Some(report),
        error: Some(error),
    })
}

fn powershell_command() -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-ExecutionPolicy", "Bypass"]);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new("pwsh");
        command.arg("-NoProfile");
        command
    }
}

pub async fn tailor_and_render(request: TailorRequest) -> Result<TailorResponse, TailoringError> {
    tailor_and_render_with_progress(request, None).await
}

pub async fn tailor_and_render_with_progress(
    request: TailorRequest,
    reporter: Option<&(dyn Fn(PipelineProgress) + Send + Sync)>,
) -> Result<TailorResponse, TailoringError> {
    let language = request.language.as_str();
    let root = workspace_root().map_err(|error| {
        progress(
            reporter,
            "resume_tailoring",
            "failed",
            error.to_string(),
            Some(1),
        );
        error
    })?;
    let base_resume = load_base_resume(&root, language).map_err(|error| {
        progress(
            reporter,
            "resume_tailoring",
            "failed",
            error.to_string(),
            Some(1),
        );
        error
    })?;
    let config = TailoringConfig::from_env().ok_or_else(|| {
        let error = TailoringError::Request("OPENAI_API_KEY is required for tailoring".to_string());
        progress(
            reporter,
            "resume_tailoring",
            "failed",
            error.to_string(),
            Some(1),
        );
        error
    })?;
    let mut page_counts = Vec::new();
    for attempt_index in 0..MAX_TAILORING_ATTEMPTS {
        let attempt = attempt_index + 1;
        progress(
            reporter,
            "resume_tailoring",
            "started",
            if attempt == 1 {
                "AI is tailoring supported resume content to the job."
            } else {
                "AI is making the resume more concise for a one-page fit."
            },
            Some(attempt),
        );
        let tailored = match tailor_resume(
            &config,
            language,
            &request.parsed,
            &request.analysis,
            &base_resume,
            &request.approved_evidence,
            request.bullet_keyword_emphasis,
            attempt_index > 0,
        )
        .await
        {
            Ok(tailored) => tailored,
            Err(error) => {
                progress(
                    reporter,
                    "resume_tailoring",
                    "failed",
                    error.to_string(),
                    Some(attempt),
                );
                return Err(error);
            }
        };
        progress(
            reporter,
            "resume_tailoring",
            "completed",
            "AI resume tailoring completed.",
            Some(attempt),
        );

        progress(
            reporter,
            "safety_validation",
            "started",
            "Checking factual and locked-layout constraints.",
            Some(attempt),
        );
        if let Err(error) = validate_tailored_content(language, &base_resume, &tailored.content) {
            progress(
                reporter,
                "safety_validation",
                "failed",
                error.to_string(),
                Some(attempt),
            );
            return Err(error);
        }
        let experience_bullets_changed = count_changed_experience_bullets(&base_resume, &tailored.content);
        progress(
            reporter,
            "safety_validation",
            "completed",
            "Tailored content passed safety validation.",
            Some(attempt),
        );

        progress(
            reporter,
            "variant_write",
            "started",
            "Saving the job-specific resume variant.",
            Some(attempt),
        );
        let (variant_slug, variant_json_path, report_json_path, docx_path) =
            match write_variant_files(
                &root,
                language,
                &request.parsed,
                &request.analysis,
                &tailored,
            ) {
                Ok(paths) => paths,
                Err(error) => {
                    progress(
                        reporter,
                        "variant_write",
                        "failed",
                        error.to_string(),
                        Some(attempt),
                    );
                    return Err(error);
                }
            };
        progress(
            reporter,
            "variant_write",
            "completed",
            "Variant JSON and tailoring report saved.",
            Some(attempt),
        );

        progress(
            reporter,
            "docx_render",
            "started",
            "Rendering the locked-layout DOCX resume.",
            Some(attempt),
        );
        if let Err(error) = render_resume(&root, language, &variant_json_path, &docx_path) {
            progress(
                reporter,
                "docx_render",
                "failed",
                error.to_string(),
                Some(attempt),
            );
            return Err(error);
        }
        progress(
            reporter,
            "docx_render",
            "completed",
            "DOCX resume rendered.",
            Some(attempt),
        );

        progress(
            reporter,
            "locked_validation",
            "started",
            "Validating locked resume sections.",
            Some(attempt),
        );
        if let Err(error) = validate_rendered_resume(&root, language, &docx_path) {
            progress(
                reporter,
                "locked_validation",
                "failed",
                error.to_string(),
                Some(attempt),
            );
            return Err(error);
        }
        progress(
            reporter,
            "locked_validation",
            "completed",
            "Locked resume sections are unchanged.",
            Some(attempt),
        );

        let pdf_path = docx_path.with_extension("pdf");
        progress(
            reporter,
            "pdf_fit",
            "started",
            "Exporting PDF and checking the one-page fit.",
            Some(attempt),
        );
        match check_one_page_fit(&root, &docx_path, &pdf_path) {
            Ok(page_count) => {
                let latest_pdf_path = root
                    .join("resume")
                    .join("generated")
                    .join(format!("Xevier_T_CV_{language}.pdf"));
                if let Err(error) = std::fs::create_dir_all(latest_pdf_path.parent().unwrap()) {
                    let error = TailoringError::Io(error.to_string());
                    progress(
                        reporter,
                        "pdf_fit",
                        "failed",
                        error.to_string(),
                        Some(attempt),
                    );
                    return Err(error);
                }
                if let Err(error) = std::fs::copy(&pdf_path, &latest_pdf_path) {
                    let error = TailoringError::Io(error.to_string());
                    progress(
                        reporter,
                        "pdf_fit",
                        "failed",
                        error.to_string(),
                        Some(attempt),
                    );
                    return Err(error);
                }
                progress(
                    reporter,
                    "pdf_fit",
                    "completed",
                    "PDF exported and confirmed at one page.",
                    Some(attempt),
                );
                let (downloads_pdf_path, downloads_error) = match publish_downloads_pdf(&latest_pdf_path, language) {
                    Ok(path) => (Some(path.to_string_lossy().to_string()), None),
                    Err(error) => {
                        eprintln!("[downloads] Failed to publish PDF: {error}");
                        (None, Some(error.to_string()))
                    }
                };
                progress(
                    reporter,
                    "complete",
                    "completed",
                    "Resume pipeline completed successfully.",
                    Some(attempt),
                );
                return Ok(TailorResponse {
                    success: true,
                    tailoring_status: "completed",
                    variant_slug: Some(variant_slug),
                    variant_json_path: Some(relative_path(&root, &variant_json_path)),
                    docx_path: Some(relative_path(&root, &docx_path)),
                    latest_docx_path: None,
                    pdf_path: Some(relative_path(&root, &pdf_path)),
                    latest_pdf_path: Some(relative_path(&root, &latest_pdf_path)),
                    downloads_pdf_path,
                    downloads_error,
                    report_json_path: Some(relative_path(&root, &report_json_path)),
                    validation_status: "passed",
                    fit_status: "passed",
                    page_count: Some(page_count),
                    bullet_keyword_emphasis: request.bullet_keyword_emphasis,
                    experience_bullets_changed,
                    report: Some(tailored.report),
                    error: None,
                });
            }
            Err(TailoringError::OnePageFit {
                page_counts: counts,
                ..
            }) => {
                page_counts.extend(counts);
                if attempt < MAX_TAILORING_ATTEMPTS {
                    progress(
                        reporter,
                        "pdf_fit",
                        "retrying",
                        "Resume exceeded one page; starting a concise rewrite.",
                        Some(attempt),
                    );
                } else {
                    let error = TailoringError::OnePageFit {
                        attempts: MAX_TAILORING_ATTEMPTS,
                        page_counts: page_counts.clone(),
                    };
                    progress(
                        reporter,
                        "pdf_fit",
                        "failed",
                        error.to_string(),
                        Some(attempt),
                    );
                    let response = partial_docx_response(
                        &root,
                        language,
                        variant_slug,
                        &variant_json_path,
                        &report_json_path,
                        &docx_path,
                        &pdf_path,
                        page_counts.last().copied(),
                        request.bullet_keyword_emphasis,
                        experience_bullets_changed,
                        tailored.report,
                        error.to_string(),
                    )?;
                    progress(
                        reporter,
                        "complete",
                        "completed",
                        "Validated DOCX saved; PDF is not ready.",
                        Some(attempt),
                    );
                    return Ok(response);
                }
            }
            Err(error) => {
                progress(
                    reporter,
                    "pdf_fit",
                    "failed",
                    error.to_string(),
                    Some(attempt),
                );
                let response = partial_docx_response(
                    &root,
                    language,
                    variant_slug,
                    &variant_json_path,
                    &report_json_path,
                    &docx_path,
                    &pdf_path,
                    None,
                    request.bullet_keyword_emphasis,
                    experience_bullets_changed,
                    tailored.report,
                    error.to_string(),
                )?;
                progress(
                    reporter,
                    "complete",
                    "completed",
                    "Validated DOCX saved; PDF is not ready.",
                    Some(attempt),
                );
                return Ok(response);
            }
        }
    }
    unreachable!("the tailoring loop returns on its final attempt")
}

pub fn failed_response(error: String) -> TailorResponse {
    TailorResponse {
        success: false,
        tailoring_status: "failed",
        variant_slug: None,
        variant_json_path: None,
        docx_path: None,
        latest_docx_path: None,
        pdf_path: None,
        latest_pdf_path: None,
        downloads_pdf_path: None,
        downloads_error: None,
        report_json_path: None,
        validation_status: "not_run",
        fit_status: "not_run",
        page_count: None,
        bullet_keyword_emphasis: BulletKeywordEmphasis::Balanced,
        experience_bullets_changed: 0,
        report: None,
        error: Some(error),
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{
        build_tailoring_prompt, civil_date_from_days, company_role_slug,
        parse_tailored_resume_from_response, partial_docx_response, slugify,
        validate_tailored_content, write_variant_files, TailoredResume, TailoringReport,
        MAX_COMPANY_ROLE_SLUG_LEN,
    };
    use crate::analysis::{JobAnalysis, KeywordSignal};
    use serde_json::json;

    fn analysis() -> JobAnalysis {
        JobAnalysis {
            role_target: "Rust Engineer".to_string(),
            seniority: "Senior".to_string(),
            core_keywords: vec![KeywordSignal {
                term: "Rust".to_string(),
                category: "technology".to_string(),
                importance: 5,
                evidence: "Job asks for Rust".to_string(),
            }],
            required_skills: vec!["Rust".to_string()],
            preferred_skills: vec!["Kubernetes".to_string()],
            tools_and_platforms: vec!["Axum".to_string()],
            domain_terms: vec!["API development".to_string()],
            responsibility_phrases: vec!["Build APIs".to_string()],
            achievement_angles: vec!["Reliable services".to_string()],
            ats_phrase_bank: vec!["Rust API development".to_string()],
            must_not_claim_without_evidence: vec!["Kubernetes".to_string()],
            summary: "Emphasize Rust API work.".to_string(),
        }
    }

    fn base_resume() -> serde_json::Value {
        json!({
            "meta": { "language": "en", "type": "base", "template": "Xevier_T_CV_en.template.docx" },
            "experience": [{
                "company": "Acme",
                "location": "Remote",
                "title": "Engineer",
                "dates": "2024 - Present",
                "bullets": ["Built APIs.", "Improved reliability."]
            }],
            "skills": {
                "frontend": "Frontend: React",
                "architecture_backend": "Architecture & Backend: Rust, APIs",
                "ai_data": "AI & Data: OpenAI",
                "testing": "Testing: Vitest",
                "devops": "DevOps: Docker",
                "tools": "Tools: Git"
            }
        })
    }

    #[test]
    fn tailoring_prompt_contains_constraints() {
        let prompt = build_tailoring_prompt(
            "en",
            &json!({"title": "Rust Engineer"}),
            &analysis(),
            &base_resume(),
            &[],
            false,
        );

        assert!(prompt.contains("Rewrite only experience bullet text and skills strings"));
        assert!(prompt.contains("Do not invent"));
        assert!(prompt.contains("omitted_unsupported_keywords"));
        assert!(prompt.contains("Rust Engineer"));
    }

    #[test]
    fn formats_epoch_day_without_a_shell_dependency() {
        assert_eq!(civil_date_from_days(0), (1970, 1, 1));
        assert_eq!(civil_date_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn concise_retry_prompt_preserves_content_constraints() {
        let prompt = build_tailoring_prompt("en", &json!({}), &analysis(), &base_resume(), &[], true);
        assert!(prompt.contains("overflowed to a second page"));
        assert!(prompt.contains("Do not shorten by deleting responsibilities"));
    }

    #[test]
    fn parses_tailored_resume_response() {
        let tailored = json!({
            "content": base_resume(),
            "report": TailoringReport {
                covered_keywords: vec!["Rust".to_string()],
                omitted_unsupported_keywords: vec!["Kubernetes".to_string()],
                changed_fields: vec!["skills.architecture_backend".to_string()],
                safety_notes: vec!["No unsupported claims added.".to_string()],
                estimated_ats_coverage_score: 82,
            }
        });
        let body = json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": serde_json::to_string(&tailored).unwrap()
                }]
            }]
        })
        .to_string();

        let parsed = parse_tailored_resume_from_response(&body).unwrap();
        assert_eq!(parsed.report.estimated_ats_coverage_score, 82);
        assert_eq!(parsed.report.omitted_unsupported_keywords[0], "Kubernetes");
    }

    #[test]
    fn validation_rejects_changed_locked_job_fields() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["experience"][0]["company"] = json!("Different");

        let err = validate_tailored_content("en", &base, &tailored).unwrap_err();
        assert!(err.to_string().contains("company changed"));
    }

    #[test]
    fn validation_rejects_changed_meta_type() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["meta"]["type"] = json!("tailored");

        let err = validate_tailored_content("en", &base, &tailored).unwrap_err();
        assert!(err.to_string().contains("meta.type changed"));
    }

    #[test]
    fn validation_rejects_changed_bullet_count() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["experience"][0]["bullets"] = json!(["Only one bullet."]);

        let err = validate_tailored_content("en", &base, &tailored).unwrap_err();
        assert!(err.to_string().contains("bullets count changed"));
    }

    #[test]
    fn validation_accepts_bullet_and_skill_rewrites() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["experience"][0]["bullets"][0] = json!("Built Rust APIs for reliable services.");
        tailored["skills"]["architecture_backend"] =
            json!("Architecture & Backend: Rust, API Design, Axum");

        validate_tailored_content("en", &base, &tailored).unwrap();
    }

    #[test]
    fn slugify_keeps_paths_safe() {
        assert_eq!(
            slugify("Acme AI / Senior Rust Engineer en"),
            "acme-ai-senior-rust-engineer-en"
        );
    }

    #[test]
    fn long_company_role_slugs_are_bounded_and_deterministic() {
        let parsed = json!({ "company": "Spendesk" });
        let mut first = analysis();
        first.role_target = "Backend Software Engineer IC3 focused on backend foundations for AI-native conversational and agentic product experiences at Spendesk".to_string();
        let first_slug = company_role_slug(&parsed, &first, "en");
        let repeated_slug = company_role_slug(&parsed, &first, "en");
        let mut second = first.clone();
        second.role_target.push_str(" with a distinct ending");
        let second_slug = company_role_slug(&parsed, &second, "en");

        assert!(first_slug.len() <= MAX_COMPANY_ROLE_SLUG_LEN);
        assert!(first_slug.ends_with("-en"));
        assert_eq!(first_slug, repeated_slug);
        assert_ne!(first_slug, second_slug);
    }

    #[test]
    fn variant_docx_filename_does_not_repeat_the_slug() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-variant-path-{suffix}"));
        let parsed = json!({ "company": "Spendesk" });
        let mut job_analysis = analysis();
        job_analysis.role_target = "Backend Software Engineer IC3 focused on backend foundations for AI-native conversational and agentic product experiences at Spendesk".to_string();
        let tailored = TailoredResume {
            content: base_resume(),
            report: TailoringReport {
                covered_keywords: vec![],
                omitted_unsupported_keywords: vec![],
                changed_fields: vec![],
                safety_notes: vec![],
                estimated_ats_coverage_score: 80,
            },
        };

        let (variant_slug, _, _, docx_path) =
            write_variant_files(&root, "en", &parsed, &job_analysis, &tailored).unwrap();

        assert!(variant_slug.len() <= 11 + MAX_COMPANY_ROLE_SLUG_LEN);
        assert_eq!(docx_path.file_name().unwrap(), "Xevier_T_CV_en.docx");
        assert!(docx_path.to_string_lossy().len() < 240);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_result_publishes_a_distinct_stable_docx() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-partial-result-{suffix}"));
        let variant_dir = root.join("resume/variants/test-en");
        let generated_dir = root.join("resume/generated");
        std::fs::create_dir_all(&variant_dir).unwrap();
        std::fs::create_dir_all(&generated_dir).unwrap();
        let docx_path = variant_dir.join("Xevier_T_CV_en.docx");
        let variant_json_path = variant_dir.join("variant.json");
        let report_json_path = variant_dir.join("tailoring-report.json");
        let pdf_path = variant_dir.join("Xevier_T_CV_en.pdf");
        let generated_template_output = generated_dir.join("Xevier_T_CV_en.generated.docx");
        std::fs::write(&docx_path, b"validated tailored docx").unwrap();
        std::fs::write(&variant_json_path, b"{}").unwrap();
        std::fs::write(&report_json_path, b"{}").unwrap();
        std::fs::write(&generated_template_output, b"existing generated output").unwrap();

        let response = partial_docx_response(
            &root,
            "en",
            "test-en".to_string(),
            &variant_json_path,
            &report_json_path,
            &docx_path,
            &pdf_path,
            None,
            TailoringReport {
                covered_keywords: vec![],
                omitted_unsupported_keywords: vec![],
                changed_fields: vec![],
                safety_notes: vec![],
                estimated_ats_coverage_score: 80,
            },
            "PDF export failed".to_string(),
        )
        .unwrap();

        assert_eq!(response.tailoring_status, "partial");
        assert_eq!(response.validation_status, "passed");
        assert_eq!(response.fit_status, "failed");
        assert_eq!(
            response.latest_docx_path.as_deref(),
            Some("resume/generated/Xevier_T_CV_en.docx")
        );
        assert_eq!(
            std::fs::read(generated_dir.join("Xevier_T_CV_en.docx")).unwrap(),
            b"validated tailored docx"
        );
        assert_eq!(
            std::fs::read(generated_template_output).unwrap(),
            b"existing generated output"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
