use crate::{
    analysis::JobAnalysis,
    api_usage::record_response_usage,
    evidence::{
        equivalent_terms, placement_equivalent_terms, placement_term_is_covered, EvidenceEntry,
    },
};
use atomicwrites::{AllowOverwrite, AtomicFile};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
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
    pub bullet_rewrite_decisions: Vec<BulletRewriteDecision>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BulletRewriteDecision {
    pub experience_index: usize,
    pub bullet_index: usize,
    pub outcome: BulletRewriteOutcome,
    pub rationale: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulletRewriteOutcome {
    Rewritten,
    NoRelevantMatch,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TailoredResume {
    pub content: serde_json::Value,
    pub report: TailoringReport,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContentChange {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RetailorMetadata {
    pub source_variant_slug: String,
    pub source_ats_score: u8,
    pub selected_terms: Vec<String>,
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
            Self::High => "Bullet keyword emphasis is HIGH: experience bullets are the primary ATS surface, not skills. You MUST return every experience bullet with different, truthful text before changing skills. A rewrite counts only when the returned bullet text differs from the input bullet text; never label an unchanged bullet as rewritten. Use natural, supported job language wherever the original bullet supports it. If a bullet has no direct job-term match, still give it an accurate stylistic rephrase that preserves its original facts. In HIGH mode every bullet_rewrite_decision must be rewritten; no_relevant_match is not allowed. A skills-only response is invalid.\n",
        }
    }
}

fn normalize_high_emphasis_bullet_rewrite_decisions(
    base: &serde_json::Value,
    tailored: &mut TailoredResume,
) {
    let supplied_rationales = tailored
        .report
        .bullet_rewrite_decisions
        .iter()
        .filter_map(|decision| {
            (!decision.rationale.trim().is_empty()).then_some((
                (decision.experience_index, decision.bullet_index),
                (decision.outcome.clone(), decision.rationale.clone()),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    let mut decisions = Vec::new();
    for (experience_index, (base_job, tailored_job)) in base["experience"]
        .as_array()
        .expect("validated base experience")
        .iter()
        .zip(
            tailored.content["experience"]
                .as_array()
                .expect("validated tailored experience"),
        )
        .enumerate()
    {
        for (bullet_index, (before, after)) in base_job["bullets"]
            .as_array()
            .expect("validated base bullets")
            .iter()
            .zip(
                tailored_job["bullets"]
                    .as_array()
                    .expect("validated tailored bullets"),
            )
            .enumerate()
        {
            let outcome = if before == after {
                BulletRewriteOutcome::NoRelevantMatch
            } else {
                BulletRewriteOutcome::Rewritten
            };
            let rationale = supplied_rationales
                .get(&(experience_index, bullet_index))
                .filter(|(supplied_outcome, _)| *supplied_outcome == outcome)
                .map(|(_, rationale)| rationale.clone())
                .unwrap_or_else(|| match outcome {
                    BulletRewriteOutcome::Rewritten => {
                        "Derived from the saved experience-bullet rewrite.".to_string()
                    }
                    BulletRewriteOutcome::NoRelevantMatch => {
                        "No experience-bullet text change was returned.".to_string()
                    }
                });
            decisions.push(BulletRewriteDecision {
                experience_index,
                bullet_index,
                outcome,
                rationale,
            });
        }
    }
    tailored.report.bullet_rewrite_decisions = decisions;
}

fn unchanged_experience_bullets(base: &serde_json::Value, tailored: &serde_json::Value) -> String {
    base["experience"]
        .as_array()
        .expect("validated base experience")
        .iter()
        .zip(
            tailored["experience"]
                .as_array()
                .expect("validated tailored experience"),
        )
        .enumerate()
        .flat_map(|(experience_index, (base_job, tailored_job))| {
            base_job["bullets"]
                .as_array()
                .expect("validated base bullets")
                .iter()
                .zip(
                    tailored_job["bullets"]
                        .as_array()
                        .expect("validated tailored bullets"),
                )
                .enumerate()
                .filter_map(move |(bullet_index, (before, after))| {
                    (before == after).then(|| {
                        format!(
                            "experience {experience_index}, bullet {bullet_index}: {}",
                            before.as_str().expect("validated base bullet")
                        )
                    })
                })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug, Deserialize)]
pub struct TailorRequest {
    pub language: String,
    pub parsed: serde_json::Value,
    pub analysis: JobAnalysis,
    #[serde(default)]
    pub approved_evidence: Vec<EvidenceEntry>,
    #[serde(default)]
    pub priority_attested_terms: Vec<String>,
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
    pub downloads_docx_path: Option<String>,
    pub downloads_docx_error: Option<String>,
    pub downloads_pdf_path: Option<String>,
    pub downloads_error: Option<String>,
    pub docx_opened: bool,
    pub docx_open_error: Option<String>,
    pub report_json_path: Option<String>,
    pub validation_status: &'static str,
    pub fit_status: &'static str,
    pub page_count: Option<u32>,
    pub bullet_keyword_emphasis: BulletKeywordEmphasis,
    pub experience_bullets_changed: u32,
    pub report: Option<TailoringReport>,
    pub tailored_content: Option<serde_json::Value>,
    pub content_changes: Vec<ContentChange>,
    pub artifact: Option<ArtifactProvenance>,
    pub retailor: Option<RetailorMetadata>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArtifactProvenance {
    pub variant_slug: String,
    pub format: String,
    pub source_path: String,
    pub downloads_path: String,
    pub sha256: String,
    pub manifest_path: String,
    pub verification_status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ArtifactManifest {
    schema_version: u8,
    variant_slug: String,
    language: String,
    selected_format: String,
    variant_json_path: String,
    docx_path: String,
    docx_sha256: String,
    pdf_path: Option<String>,
    pdf_sha256: Option<String>,
    downloads_path: String,
    downloads_sha256: String,
    validation_status: String,
    fit_status: String,
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
    priority_attested_terms: &[String],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
    correction_instruction: Option<&str>,
) -> String {
    let parsed_job = serde_json::to_string(parsed_job).unwrap_or_else(|_| "{}".to_string());
    let analysis = serde_json::to_string(analysis).unwrap_or_else(|_| "{}".to_string());
    let base_resume = serde_json::to_string(base_resume).unwrap_or_else(|_| "{}".to_string());
    let placement_terms = priority_attested_terms.to_vec();
    let approved_evidence =
        serde_json::to_string(approved_evidence).unwrap_or_else(|_| "[]".to_string());
    let has_model_placement_terms = !placement_terms.is_empty();

    let concise_instruction = if concise {
        if has_model_placement_terms {
            "The preceding attempt overflowed to a second page. Keep the same bullet count and preserve every selected claim placement, but rewrite the editable text more compactly. Base claims intentionally displaced by selected user-attested claims may remain displaced; preserve the other factual claims. Remove repetition, use concise verbs, and prefer compact ATS terminology.\n\n"
        } else {
            "The preceding attempt overflowed to a second page. Keep every bullet and every factual claim, but rewrite the editable text more compactly: remove repetition, use concise verbs, and prefer compact ATS terminology. Do not shorten by deleting responsibilities or achievements.\n\n"
        }
    } else {
        ""
    };
    let correction_instruction = correction_instruction.unwrap_or("");
    let placement_instruction = if has_model_placement_terms {
        format!(
            "The user explicitly attested the following claims and authorized you to place them in the most plausible existing role: {}. You MUST incorporate every selected claim naturally in one or more experience bullets. You may completely replace the least job-relevant existing bullet claims to make room, while keeping the same jobs and bullet counts. Multiple selected claims may share a bullet when natural. The attestation supports only the named claims; do not invent adjacent details, metrics, employers, dates, credentials, or responsibilities.\n",
            placement_terms.join(", ")
        )
    } else {
        String::new()
    };

    format!(
        "Tailor this {language} resume JSON for maximum truthful ATS alignment.\n\
         Return only JSON matching the schema. Preserve the input resume shape exactly.\n\
         Rewrite only experience bullet text and skills strings.\n\
         Do not change meta, company names, locations, titles, dates, job order, number of jobs, number of bullets, or skill keys.\n\
         Aggressively incorporate ATS keywords, tools, responsibility phrases, and domain wording when the base resume supports them.\n\
         {bullet_emphasis_instruction}\
         {placement_instruction}\
         User-attested evidence may support a skills string. Use it in an experience bullet only when its proof_note explicitly names a matching role or project or allow_model_role_placement is true; never infer a responsibility from any other term alone.\n\
         Do not invent credentials, employers, tools, metrics, responsibilities, education, certifications, or experience.\n\
         Put important job keywords without base-resume or user-attested evidence into omitted_unsupported_keywords instead of adding them to the resume.\n\
         Keep each rewritten bullet close to the original length so the locked DOCX layout remains stable.\n\n\
         {concise_instruction}\
         {correction_instruction}\
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
                    "estimated_ats_coverage_score",
                    "bullet_rewrite_decisions"
                ],
                "properties": {
                    "covered_keywords": { "type": "array", "items": { "type": "string" } },
                    "omitted_unsupported_keywords": { "type": "array", "items": { "type": "string" } },
                    "changed_fields": { "type": "array", "items": { "type": "string" } },
                    "safety_notes": { "type": "array", "items": { "type": "string" } },
                    "estimated_ats_coverage_score": { "type": "integer", "minimum": 0, "maximum": 100 },
                    "bullet_rewrite_decisions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["experience_index", "bullet_index", "outcome", "rationale"],
                            "properties": {
                                "experience_index": { "type": "integer", "minimum": 0 },
                                "bullet_index": { "type": "integer", "minimum": 0 },
                                "outcome": { "type": "string", "enum": ["rewritten", "no_relevant_match"] },
                                "rationale": { "type": "string", "minLength": 1 }
                            }
                        }
                    }
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
    priority_attested_terms: &[String],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
    correction_instruction: Option<&str>,
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
                "content": build_tailoring_prompt(language, parsed_job, analysis, base_resume, approved_evidence, priority_attested_terms, bullet_keyword_emphasis, concise, correction_instruction)
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
    priority_attested_terms: &[String],
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    concise: bool,
    correction_instruction: Option<&str>,
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
        priority_attested_terms,
        bullet_keyword_emphasis,
        concise,
        correction_instruction,
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

    let tailored = parse_tailored_resume_from_response(&body)?;
    record_response_usage("resume_tailoring", &config.model, &body);
    Ok(tailored)
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

fn validate_high_emphasis_bullet_rewrites(
    base: &serde_json::Value,
    tailored: &TailoredResume,
) -> Result<(), TailoringError> {
    let mut expected = BTreeSet::new();
    let mut changed = BTreeSet::new();
    for (experience_index, (base_job, tailored_job)) in base["experience"]
        .as_array()
        .expect("validated base experience")
        .iter()
        .zip(
            tailored.content["experience"]
                .as_array()
                .expect("validated tailored experience"),
        )
        .enumerate()
    {
        for (bullet_index, (before, after)) in base_job["bullets"]
            .as_array()
            .expect("validated base bullets")
            .iter()
            .zip(
                tailored_job["bullets"]
                    .as_array()
                    .expect("validated tailored bullets"),
            )
            .enumerate()
        {
            expected.insert((experience_index, bullet_index));
            if before != after {
                changed.insert((experience_index, bullet_index));
            }
        }
    }

    if changed.len() != expected.len() {
        return invalid("high bullet emphasis requires every experience bullet to be rewritten; skills-only tailoring is not accepted");
    }

    let mut decisions = BTreeSet::new();
    for decision in &tailored.report.bullet_rewrite_decisions {
        let path = (decision.experience_index, decision.bullet_index);
        if !expected.contains(&path) {
            return invalid(&format!(
                "bullet rewrite decision references missing experience bullet {}/{}",
                decision.experience_index, decision.bullet_index
            ));
        }
        if !decisions.insert(path) {
            return invalid(&format!(
                "bullet rewrite decision is duplicated for experience bullet {}/{}",
                decision.experience_index, decision.bullet_index
            ));
        }
        match decision.outcome {
            BulletRewriteOutcome::Rewritten if !changed.contains(&path) => {
                return invalid(&format!(
                    "bullet rewrite decision marks unchanged experience bullet {}/{} as rewritten",
                    decision.experience_index, decision.bullet_index
                ));
            }
            BulletRewriteOutcome::NoRelevantMatch if changed.contains(&path) => {
                return invalid(&format!(
                    "bullet rewrite decision marks changed experience bullet {}/{} as no_relevant_match",
                    decision.experience_index, decision.bullet_index
                ));
            }
            _ => {}
        }
    }
    if decisions != expected {
        return invalid(
            "high bullet emphasis requires one rewrite decision for every experience bullet",
        );
    }
    Ok(())
}

fn count_changed_experience_bullets(base: &serde_json::Value, tailored: &serde_json::Value) -> u32 {
    base["experience"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(tailored["experience"].as_array().into_iter().flatten())
        .flat_map(|(base_job, tailored_job)| {
            base_job["bullets"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(tailored_job["bullets"].as_array().into_iter().flatten())
        })
        .filter(|(base_bullet, tailored_bullet)| base_bullet != tailored_bullet)
        .count() as u32
}

fn missing_model_placement_terms(
    tailored: &serde_json::Value,
    required_terms: &[String],
) -> Vec<String> {
    let experience_text = tailored["experience"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|job| job["bullets"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    required_terms
        .iter()
        .filter(|term| !placement_term_is_covered(&experience_text, term))
        .cloned()
        .collect()
}

fn reconcile_model_placement_report(report: &mut TailoringReport, selected_terms: &[String]) {
    report.omitted_unsupported_keywords.retain(|omitted| {
        !selected_terms.iter().any(|selected| {
            equivalent_terms(omitted, selected) || placement_equivalent_terms(omitted, selected)
        })
    });
    for selected in selected_terms {
        if !report.covered_keywords.iter().any(|covered| {
            equivalent_terms(covered, selected) || placement_equivalent_terms(covered, selected)
        }) {
            report.covered_keywords.push(selected.clone());
        }
    }
}

pub(crate) fn content_changes(
    base: &serde_json::Value,
    tailored: &serde_json::Value,
) -> Vec<ContentChange> {
    let mut changes = Vec::new();
    let base_experience = base["experience"]
        .as_array()
        .expect("validated base experience");
    let tailored_experience = tailored["experience"]
        .as_array()
        .expect("validated tailored experience");

    for (job_index, (base_job, tailored_job)) in
        base_experience.iter().zip(tailored_experience).enumerate()
    {
        let base_bullets = base_job["bullets"]
            .as_array()
            .expect("validated base bullets");
        let tailored_bullets = tailored_job["bullets"]
            .as_array()
            .expect("validated tailored bullets");
        for (bullet_index, (before, after)) in base_bullets.iter().zip(tailored_bullets).enumerate()
        {
            if before != after {
                changes.push(ContentChange {
                    path: format!("/experience/{job_index}/bullets/{bullet_index}"),
                    before: before.as_str().expect("validated base bullet").to_string(),
                    after: after
                        .as_str()
                        .expect("validated tailored bullet")
                        .to_string(),
                });
            }
        }
    }

    let base_skills = base["skills"].as_object().expect("validated base skills");
    let tailored_skills = tailored["skills"]
        .as_object()
        .expect("validated tailored skills");
    for (key, before) in base_skills {
        let after = &tailored_skills[key];
        if before != after {
            changes.push(ContentChange {
                path: format!("/skills/{key}"),
                before: before.as_str().expect("validated base skill").to_string(),
                after: after
                    .as_str()
                    .expect("validated tailored skill")
                    .to_string(),
            });
        }
    }
    changes
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
    variant_json_path: &Path,
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
        .arg("-Content")
        .arg(variant_json_path)
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
        .arg("pdf")
        .arg("-Docx")
        .arg(docx_path)
        .arg("-Out")
        .arg(pdf_path.parent().ok_or_else(|| {
            TailoringError::Fit("PDF output path must have a parent directory.".to_string())
        })?)
        .output()
        .map_err(|error| TailoringError::Fit(error.to_string()))?;
    if !output.status.success() {
        return Err(TailoringError::Fit(command_output(&output)));
    }

    let page_count = pdf_page_count(pdf_path)?;
    match page_count {
        1 => Ok(1),
        count => Err(TailoringError::OnePageFit {
            attempts: 1,
            page_counts: vec![count],
        }),
    }
}

fn pdf_page_count(pdf_path: &Path) -> Result<u32, TailoringError> {
    let document = lopdf::Document::load(pdf_path)
        .map_err(|error| TailoringError::Fit(format!("Could not read exported PDF: {error}")))?;
    Ok(document.get_pages().len() as u32)
}

fn downloads_file_path(language: &str, extension: &str) -> Result<PathBuf, TailoringError> {
    validate_language(language)?;
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            TailoringError::Io(
                "Could not determine the user home directory for Downloads.".to_string(),
            )
        })?;
    Ok(PathBuf::from(home)
        .join("Downloads")
        .join(format!("Xevier_T_CV_{language}.{extension}")))
}

fn downloads_file_path_in(
    downloads_directory: Option<&Path>,
    language: &str,
    extension: &str,
) -> Result<PathBuf, TailoringError> {
    match downloads_directory {
        Some(directory) => {
            validate_language(language)?;
            Ok(directory.join(format!("Xevier_T_CV_{language}.{extension}")))
        }
        None => downloads_file_path(language, extension),
    }
}

fn sha256_file(path: &Path) -> Result<String, TailoringError> {
    let mut file = File::open(path).map_err(|error| TailoringError::Io(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| TailoringError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn atomic_copy(source_path: &Path, destination: &Path) -> Result<(), TailoringError> {
    AtomicFile::new(destination, AllowOverwrite)
        .write(|output| -> std::io::Result<()> {
            let mut source = File::open(source_path)?;
            std::io::copy(&mut source, output)?;
            output.sync_all()
        })
        .map_err(|error| TailoringError::Io(error.to_string()))
}

static DOWNLOAD_PUBLICATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[allow(clippy::too_many_arguments)]
fn publish_verified_artifact(
    root: &Path,
    variant_slug: &str,
    source_path: &Path,
    language: &str,
    extension: &str,
    variant_json_path: &Path,
    docx_path: &Path,
    pdf_path: Option<&Path>,
    validation_status: &str,
    fit_status: &str,
    downloads_directory: Option<&Path>,
) -> Result<ArtifactProvenance, TailoringError> {
    if !matches!(extension, "pdf" | "docx") {
        return Err(TailoringError::Io(format!(
            "Unsupported resume artifact format: {extension}"
        )));
    }
    if !source_path.is_file() {
        return Err(TailoringError::Io(format!(
            "Verified variant artifact does not exist: {}",
            source_path.display()
        )));
    }

    let _guard = DOWNLOAD_PUBLICATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| TailoringError::Io("Downloads publication lock was poisoned.".to_string()))?;
    let destination = downloads_file_path_in(downloads_directory, language, extension)?;
    std::fs::create_dir_all(destination.parent().expect("Downloads path has a parent"))
        .map_err(|error| TailoringError::Io(error.to_string()))?;

    let source_sha256 = sha256_file(source_path)?;
    atomic_copy(source_path, &destination)?;
    let destination_sha256 = sha256_file(&destination)?;
    if source_sha256 != destination_sha256 {
        let _ = std::fs::remove_file(&destination);
        return Err(TailoringError::Io(
            "Downloads artifact hash did not match the selected variant; the copy was removed."
                .to_string(),
        ));
    }

    let counterpart_extension = if extension == "pdf" { "docx" } else { "pdf" };
    let counterpart = downloads_file_path_in(downloads_directory, language, counterpart_extension)?;
    if counterpart.is_file() {
        if let Err(error) = std::fs::remove_file(&counterpart) {
            let _ = std::fs::remove_file(&destination);
            return Err(TailoringError::Io(format!(
                "The verified {extension} was prepared, but the stale {counterpart_extension} could not be removed from Downloads: {error}"
            )));
        }
    }

    let docx_sha256 = sha256_file(docx_path)?;
    let (manifest_pdf_path, pdf_sha256) = match pdf_path.filter(|path| path.is_file()) {
        Some(path) => (Some(relative_path(root, path)), Some(sha256_file(path)?)),
        None => (None, None),
    };
    let manifest_path = root
        .join("resume")
        .join("variants")
        .join(variant_slug)
        .join("artifact-manifest.json");
    let manifest = ArtifactManifest {
        schema_version: 1,
        variant_slug: variant_slug.to_string(),
        language: language.to_string(),
        selected_format: extension.to_string(),
        variant_json_path: relative_path(root, variant_json_path),
        docx_path: relative_path(root, docx_path),
        docx_sha256,
        pdf_path: manifest_pdf_path,
        pdf_sha256,
        downloads_path: destination.to_string_lossy().to_string(),
        downloads_sha256: destination_sha256.clone(),
        validation_status: validation_status.to_string(),
        fit_status: fit_status.to_string(),
    };
    if let Err(error) = write_json(&manifest_path, &manifest) {
        let _ = std::fs::remove_file(&destination);
        return Err(error);
    }

    Ok(ArtifactProvenance {
        variant_slug: variant_slug.to_string(),
        format: extension.to_string(),
        source_path: relative_path(root, source_path),
        downloads_path: destination.to_string_lossy().to_string(),
        sha256: source_sha256,
        manifest_path: relative_path(root, &manifest_path),
        verification_status: "verified".to_string(),
    })
}

pub fn publish_variant_artifact(
    root: &Path,
    variant_slug: &str,
    format: &str,
) -> Result<ArtifactProvenance, TailoringError> {
    if variant_slug.is_empty()
        || !variant_slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(TailoringError::Io(
            "Variant identifier contains unsupported path characters.".to_string(),
        ));
    }
    if !matches!(format, "pdf" | "docx") {
        return Err(TailoringError::Io(format!(
            "Unsupported resume artifact format: {format}"
        )));
    }

    let variant_dir = root.join("resume").join("variants").join(variant_slug);
    if !variant_dir.is_dir() {
        return Err(TailoringError::Io(format!(
            "Resume variant does not exist: {variant_slug}"
        )));
    }
    let variant_json_path = variant_dir.join("variant.json");
    let variant: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&variant_json_path)
            .map_err(|error| TailoringError::Io(error.to_string()))?,
    )
    .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    let language = variant["meta"]["language"]
        .as_str()
        .ok_or_else(|| TailoringError::InvalidJson("Variant language is missing.".to_string()))?;
    validate_language(language)?;
    let docx_path = variant_dir.join(format!("Xevier_T_CV_{language}.docx"));
    validate_rendered_resume(root, language, &variant_json_path, &docx_path)?;
    let pdf_path = variant_dir.join(format!("Xevier_T_CV_{language}.pdf"));
    let (source_path, fit_status, manifest_pdf_path) = if format == "pdf" {
        if !pdf_path.is_file() {
            return Err(TailoringError::Io(format!(
                "The selected variant does not have a PDF: {variant_slug}"
            )));
        }
        let pages = pdf_page_count(&pdf_path)?;
        if pages != 1 {
            return Err(TailoringError::Fit(format!(
                "The selected variant PDF has {pages} pages; only a one-page PDF can be published."
            )));
        }
        (&pdf_path, "passed", Some(pdf_path.as_path()))
    } else {
        (
            &docx_path,
            if pdf_path.is_file() {
                "failed"
            } else {
                "not_run"
            },
            pdf_path.is_file().then_some(pdf_path.as_path()),
        )
    };

    publish_verified_artifact(
        root,
        variant_slug,
        source_path,
        language,
        format,
        &variant_json_path,
        &docx_path,
        manifest_pdf_path,
        "passed",
        fit_status,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn partial_tailoring_response(
    root: &Path,
    variant_slug: String,
    variant_json_path: &Path,
    report_json_path: &Path,
    docx_path: &Path,
    pdf_path: &Path,
    page_count: Option<u32>,
    bullet_keyword_emphasis: BulletKeywordEmphasis,
    experience_bullets_changed: u32,
    tailored_content: serde_json::Value,
    content_changes: Vec<ContentChange>,
    report: TailoringReport,
    validation_status: &'static str,
    fit_status: &'static str,
    publish_docx: bool,
    error: String,
) -> TailorResponse {
    let latest_docx_path = publish_docx.then(|| relative_path(root, docx_path));
    TailorResponse {
        success: false,
        tailoring_status: "partial",
        variant_slug: Some(variant_slug),
        variant_json_path: Some(relative_path(root, variant_json_path)),
        docx_path: docx_path.exists().then(|| relative_path(root, docx_path)),
        latest_docx_path,
        pdf_path: pdf_path.exists().then(|| relative_path(root, pdf_path)),
        latest_pdf_path: None,
        downloads_docx_path: None,
        downloads_docx_error: None,
        downloads_pdf_path: None,
        downloads_error: None,
        docx_opened: false,
        docx_open_error: None,
        report_json_path: Some(relative_path(root, report_json_path)),
        validation_status,
        fit_status,
        page_count,
        bullet_keyword_emphasis,
        experience_bullets_changed,
        report: Some(report),
        tailored_content: Some(tailored_content),
        content_changes,
        artifact: None,
        retailor: None,
        error: Some(error),
    }
}

fn publish_partial_docx_to_downloads(response: &mut TailorResponse, root: &Path, language: &str) {
    let (Some(relative_docx_path), Some(relative_variant_json_path), Some(variant_slug)) = (
        response.docx_path.as_deref(),
        response.variant_json_path.as_deref(),
        response.variant_slug.as_deref(),
    ) else {
        return;
    };
    let docx_path = root.join(relative_docx_path);
    let pdf_path = response.pdf_path.as_deref().map(|path| root.join(path));
    match publish_verified_artifact(
        root,
        variant_slug,
        &docx_path,
        language,
        "docx",
        &root.join(relative_variant_json_path),
        &docx_path,
        pdf_path.as_deref(),
        response.validation_status,
        response.fit_status,
        None,
    ) {
        Ok(artifact) => {
            response.downloads_docx_path = Some(artifact.downloads_path.clone());
            response.downloads_docx_error = None;
            response.artifact = Some(artifact);
        }
        Err(error) => response.downloads_docx_error = Some(error.to_string()),
    }
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
    let mut needs_concise_rewrite = false;
    let mut correction_instruction: Option<String> = None;
    let required_placement_terms = request.priority_attested_terms.clone();
    for attempt_index in 0..MAX_TAILORING_ATTEMPTS {
        let attempt = attempt_index + 1;
        progress(
            reporter,
            "resume_tailoring",
            "started",
            if correction_instruction.is_some() {
                "AI is correcting required experience-bullet content."
            } else if needs_concise_rewrite {
                "AI is making the resume more concise for a one-page fit."
            } else {
                "AI is tailoring supported resume content to the job."
            },
            Some(attempt),
        );
        let mut tailored = match tailor_resume(
            &config,
            language,
            &request.parsed,
            &request.analysis,
            &base_resume,
            &request.approved_evidence,
            &request.priority_attested_terms,
            request.bullet_keyword_emphasis,
            needs_concise_rewrite,
            correction_instruction.as_deref(),
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
        if request.bullet_keyword_emphasis == BulletKeywordEmphasis::High {
            normalize_high_emphasis_bullet_rewrite_decisions(&base_resume, &mut tailored);
            if let Err(error) = validate_high_emphasis_bullet_rewrites(&base_resume, &tailored) {
                if attempt < MAX_TAILORING_ATTEMPTS {
                    correction_instruction = Some(format!(
                        "Your preceding High-emphasis response was rejected: {error}. Replace the text of every unchanged experience bullet below. Keep every claim truthful, but use a faithful stylistic rephrase when a bullet has no direct job-keyword match. Do not compensate by adding more skills, changing only the report, or labelling unchanged text as rewritten.\n\nUnchanged bullets:\n{}\n\n",
                        unchanged_experience_bullets(&base_resume, &tailored.content),
                    ));
                    progress(
                        reporter,
                        "safety_validation",
                        "retrying",
                        "High emphasis requires experience-bullet rewrites; requesting a corrected response.",
                        Some(attempt),
                    );
                    continue;
                }
                let final_error = invalid_message(
                    "High emphasis could not rewrite every experience bullet after 3 attempts; skills-only tailoring is not accepted",
                );
                progress(
                    reporter,
                    "safety_validation",
                    "failed",
                    final_error.to_string(),
                    Some(attempt),
                );
                return Err(final_error);
            }
        }
        let missing_placement_terms =
            missing_model_placement_terms(&tailored.content, &required_placement_terms);
        if !missing_placement_terms.is_empty() {
            if attempt < MAX_TAILORING_ATTEMPTS {
                correction_instruction = Some(format!(
                    "Your preceding response omitted user-attested claims that must appear naturally in experience bullets: {}. Place every missing claim in the most plausible existing role, replacing lower-value bullet claims if needed. Keep the same job and bullet counts and do not add facts beyond the selected claims.\n\n",
                    missing_placement_terms.join(", ")
                ));
                progress(
                    reporter,
                    "safety_validation",
                    "retrying",
                    "Selected claims were missing from experience bullets; requesting a corrected response.",
                    Some(attempt),
                );
                continue;
            }
            let final_error = invalid_message(&format!(
                "selected claims could not be placed in experience bullets after {MAX_TAILORING_ATTEMPTS} attempts: {}",
                missing_placement_terms.join(", ")
            ));
            progress(
                reporter,
                "safety_validation",
                "failed",
                final_error.to_string(),
                Some(attempt),
            );
            return Err(final_error);
        }
        reconcile_model_placement_report(&mut tailored.report, &required_placement_terms);
        correction_instruction = None;
        let experience_bullets_changed =
            count_changed_experience_bullets(&base_resume, &tailored.content);
        let changes = content_changes(&base_resume, &tailored.content);
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
        let pdf_path = docx_path.with_extension("pdf");

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
            return Ok(partial_tailoring_response(
                &root,
                variant_slug,
                &variant_json_path,
                &report_json_path,
                &docx_path,
                &pdf_path,
                None,
                request.bullet_keyword_emphasis,
                experience_bullets_changed,
                tailored.content,
                changes,
                tailored.report,
                "not_run",
                "not_run",
                false,
                error.to_string(),
            ));
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
        if let Err(error) =
            validate_rendered_resume(&root, language, &variant_json_path, &docx_path)
        {
            progress(
                reporter,
                "locked_validation",
                "failed",
                error.to_string(),
                Some(attempt),
            );
            return Ok(partial_tailoring_response(
                &root,
                variant_slug,
                &variant_json_path,
                &report_json_path,
                &docx_path,
                &pdf_path,
                None,
                request.bullet_keyword_emphasis,
                experience_bullets_changed,
                tailored.content,
                changes,
                tailored.report,
                "failed",
                "not_run",
                false,
                error.to_string(),
            ));
        }
        progress(
            reporter,
            "locked_validation",
            "completed",
            "Locked resume sections are unchanged.",
            Some(attempt),
        );

        progress(
            reporter,
            "pdf_fit",
            "started",
            "Exporting PDF and checking the one-page fit.",
            Some(attempt),
        );
        match check_one_page_fit(&root, &docx_path, &pdf_path) {
            Ok(page_count) => {
                progress(
                    reporter,
                    "pdf_fit",
                    "completed",
                    "PDF exported and confirmed at one page.",
                    Some(attempt),
                );
                let (downloads_pdf_path, downloads_error, artifact) =
                    match publish_verified_artifact(
                        &root,
                        &variant_slug,
                        &pdf_path,
                        language,
                        "pdf",
                        &variant_json_path,
                        &docx_path,
                        Some(&pdf_path),
                        "passed",
                        "passed",
                        None,
                    ) {
                        Ok(artifact) => {
                            (Some(artifact.downloads_path.clone()), None, Some(artifact))
                        }
                        Err(error) => {
                            eprintln!("[downloads] Failed to publish PDF: {error}");
                            (None, Some(error.to_string()), None)
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
                    latest_pdf_path: Some(relative_path(&root, &pdf_path)),
                    downloads_docx_path: None,
                    downloads_docx_error: None,
                    downloads_pdf_path,
                    downloads_error,
                    docx_opened: false,
                    docx_open_error: None,
                    report_json_path: Some(relative_path(&root, &report_json_path)),
                    validation_status: "passed",
                    fit_status: "passed",
                    page_count: Some(page_count),
                    bullet_keyword_emphasis: request.bullet_keyword_emphasis,
                    experience_bullets_changed,
                    report: Some(tailored.report),
                    tailored_content: Some(tailored.content),
                    content_changes: changes,
                    artifact,
                    retailor: None,
                    error: None,
                });
            }
            Err(TailoringError::OnePageFit {
                page_counts: counts,
                ..
            }) => {
                page_counts.extend(counts);
                if attempt < MAX_TAILORING_ATTEMPTS {
                    needs_concise_rewrite = true;
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
                    let mut response = partial_tailoring_response(
                        &root,
                        variant_slug,
                        &variant_json_path,
                        &report_json_path,
                        &docx_path,
                        &pdf_path,
                        page_counts.last().copied(),
                        request.bullet_keyword_emphasis,
                        experience_bullets_changed,
                        tailored.content,
                        changes,
                        tailored.report,
                        "passed",
                        "failed",
                        true,
                        error.to_string(),
                    );
                    publish_partial_docx_to_downloads(&mut response, &root, language);
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
                let mut response = partial_tailoring_response(
                    &root,
                    variant_slug,
                    &variant_json_path,
                    &report_json_path,
                    &docx_path,
                    &pdf_path,
                    None,
                    request.bullet_keyword_emphasis,
                    experience_bullets_changed,
                    tailored.content,
                    changes,
                    tailored.report,
                    "passed",
                    "failed",
                    true,
                    error.to_string(),
                );
                publish_partial_docx_to_downloads(&mut response, &root, language);
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
        downloads_docx_path: None,
        downloads_docx_error: None,
        downloads_pdf_path: None,
        downloads_error: None,
        docx_opened: false,
        docx_open_error: None,
        report_json_path: None,
        validation_status: "not_run",
        fit_status: "not_run",
        page_count: None,
        bullet_keyword_emphasis: BulletKeywordEmphasis::Balanced,
        experience_bullets_changed: 0,
        report: None,
        tailored_content: None,
        content_changes: vec![],
        artifact: None,
        retailor: None,
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
        build_tailoring_prompt, civil_date_from_days, company_role_slug, content_changes,
        missing_model_placement_terms, normalize_high_emphasis_bullet_rewrite_decisions,
        parse_tailored_resume_from_response, partial_tailoring_response, pdf_page_count,
        publish_verified_artifact, reconcile_model_placement_report, sha256_file, slugify,
        unchanged_experience_bullets, validate_high_emphasis_bullet_rewrites,
        validate_tailored_content, write_variant_files, BulletKeywordEmphasis,
        BulletRewriteDecision, BulletRewriteOutcome, TailorRequest, TailoredResume,
        TailoringReport, MAX_COMPANY_ROLE_SLUG_LEN,
    };
    use crate::analysis::{JobAnalysis, KeywordSignal};
    use crate::evidence::EvidenceEntry;
    use lopdf::{dictionary, Document, Object};
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
            &[],
            BulletKeywordEmphasis::Balanced,
            false,
            None,
        );

        assert!(prompt.contains("Rewrite only experience bullet text and skills strings"));
        assert!(prompt.contains("Do not invent"));
        assert!(prompt.contains("omitted_unsupported_keywords"));
        assert!(prompt.contains("Rust Engineer"));
    }

    #[test]
    fn tailoring_prompt_authorizes_selected_claim_replacement() {
        let evidence = vec![EvidenceEntry {
            term: "Angular dans l’expérience".to_string(),
            kind: "technology".to_string(),
            proof_note: None,
            user_attested: true,
            allow_model_role_placement: true,
        }];
        let prompt = build_tailoring_prompt(
            "fr",
            &json!({"title": "Développeur"}),
            &analysis(),
            &base_resume(),
            &evidence,
            &["Angular dans l’expérience".to_string()],
            BulletKeywordEmphasis::Balanced,
            false,
            None,
        );

        assert!(prompt.contains("authorized you to place them in the most plausible existing role"));
        assert!(prompt.contains("completely replace the least job-relevant existing bullet"));
        assert!(prompt.contains("Angular dans l’expérience"));
        assert!(prompt.contains("do not invent adjacent details"));
    }

    #[test]
    fn selected_claims_must_appear_in_experience_and_are_reconciled_in_report() {
        let tailored = json!({
            "experience": [{"bullets": ["Built Angular interfaces for internal users."]}]
        });
        let selected = vec![
            "Angular dans l’expérience".to_string(),
            "GCP dans l’expérience".to_string(),
        ];
        assert_eq!(
            missing_model_placement_terms(&tailored, &selected),
            vec!["GCP dans l’expérience".to_string()]
        );

        let mut report = TailoringReport {
            covered_keywords: vec![],
            omitted_unsupported_keywords: vec!["Angular dans l’expérience".to_string()],
            changed_fields: vec![],
            safety_notes: vec![],
            estimated_ats_coverage_score: 73,
            bullet_rewrite_decisions: vec![],
        };
        reconcile_model_placement_report(&mut report, &selected[..1]);
        assert!(report.omitted_unsupported_keywords.is_empty());
        assert_eq!(report.covered_keywords, vec!["Angular dans l’expérience"]);
        assert_eq!(report.estimated_ats_coverage_score, 73);
    }

    #[test]
    fn formats_epoch_day_without_a_shell_dependency() {
        assert_eq!(civil_date_from_days(0), (1970, 1, 1));
        assert_eq!(civil_date_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn concise_retry_prompt_preserves_content_constraints() {
        let prompt = build_tailoring_prompt(
            "en",
            &json!({}),
            &analysis(),
            &base_resume(),
            &[],
            &[],
            BulletKeywordEmphasis::Balanced,
            true,
            None,
        );
        assert!(prompt.contains("overflowed to a second page"));
        assert!(prompt.contains("Do not shorten by deleting responsibilities"));
    }

    #[test]
    fn high_bullet_emphasis_prioritizes_breadth() {
        let prompt = build_tailoring_prompt(
            "en",
            &json!({}),
            &analysis(),
            &base_resume(),
            &[],
            &[],
            BulletKeywordEmphasis::High,
            false,
            None,
        );
        assert!(prompt.contains("every experience bullet with different, truthful text"));
        assert!(prompt.contains("primary ATS surface, not skills"));
        assert!(prompt.contains("A skills-only response is invalid"));
    }

    #[test]
    fn omitted_bullet_emphasis_defaults_to_balanced() {
        let request: TailorRequest = serde_json::from_value(json!({
            "language": "en",
            "parsed": {},
            "analysis": analysis()
        }))
        .unwrap();
        assert_eq!(
            request.bullet_keyword_emphasis,
            BulletKeywordEmphasis::Balanced
        );
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
                bullet_rewrite_decisions: vec![],
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
    fn content_change_list_only_includes_editable_changed_values() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");
        tailored["skills"]["architecture_backend"] =
            json!("Architecture & Backend: Rust, API Design");

        let changes = content_changes(&base, &tailored);

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "/experience/0/bullets/0");
        assert_eq!(changes[0].before, "Built APIs.");
        assert_eq!(changes[0].after, "Built reliable Rust APIs.");
        assert_eq!(changes[1].path, "/skills/architecture_backend");
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

    fn high_emphasis_tailored_resume(
        content: serde_json::Value,
        decisions: Vec<BulletRewriteDecision>,
    ) -> TailoredResume {
        TailoredResume {
            content,
            report: TailoringReport {
                covered_keywords: vec!["Rust".to_string()],
                omitted_unsupported_keywords: vec![],
                changed_fields: vec!["experience[0].bullets[0]".to_string()],
                safety_notes: vec![],
                estimated_ats_coverage_score: 80,
                bullet_rewrite_decisions: decisions,
            },
        }
    }

    #[test]
    fn high_emphasis_rejects_skills_only_tailoring() {
        let base = base_resume();
        let mut content = base.clone();
        content["skills"]["architecture_backend"] =
            json!("Architecture & Backend: Rust, APIs, Axum");
        let tailored = high_emphasis_tailored_resume(
            content,
            vec![
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 0,
                    outcome: BulletRewriteOutcome::NoRelevantMatch,
                    rationale: "No job match.".to_string(),
                },
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 1,
                    outcome: BulletRewriteOutcome::NoRelevantMatch,
                    rationale: "No job match.".to_string(),
                },
            ],
        );

        let error = validate_high_emphasis_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error.to_string().contains("skills-only"));
    }

    #[test]
    fn high_emphasis_rejects_an_unchanged_bullet() {
        let base = base_resume();
        let mut content = base.clone();
        content["experience"][0]["bullets"][1] = json!("Improved reliable Rust services.");
        let tailored = high_emphasis_tailored_resume(
            content,
            vec![
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 0,
                    outcome: BulletRewriteOutcome::Rewritten,
                    rationale: "Rust API match.".to_string(),
                },
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 1,
                    outcome: BulletRewriteOutcome::NoRelevantMatch,
                    rationale: "No job match.".to_string(),
                },
            ],
        );

        let error = validate_high_emphasis_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error.to_string().contains("every experience bullet"));
    }

    #[test]
    fn high_emphasis_normalization_uses_saved_bullet_text_as_the_source_of_truth() {
        let base = base_resume();
        let mut content = base.clone();
        content["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");
        content["experience"][0]["bullets"][1] =
            json!("Strengthened production service reliability.");
        let mut tailored = high_emphasis_tailored_resume(
            content,
            vec![
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 0,
                    outcome: BulletRewriteOutcome::NoRelevantMatch,
                    rationale: "No job match.".to_string(),
                },
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 1,
                    outcome: BulletRewriteOutcome::Rewritten,
                    rationale: "Rust API match.".to_string(),
                },
            ],
        );

        normalize_high_emphasis_bullet_rewrite_decisions(&base, &mut tailored);

        assert_eq!(
            tailored.report.bullet_rewrite_decisions[0].outcome,
            BulletRewriteOutcome::Rewritten
        );
        assert_eq!(
            tailored.report.bullet_rewrite_decisions[1].outcome,
            BulletRewriteOutcome::Rewritten
        );
        validate_high_emphasis_bullet_rewrites(&base, &tailored).unwrap();
    }

    #[test]
    fn high_emphasis_rejects_an_incomplete_bullet_audit() {
        let base = base_resume();
        let mut content = base.clone();
        content["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");
        content["experience"][0]["bullets"][1] =
            json!("Strengthened production service reliability.");
        let tailored = high_emphasis_tailored_resume(
            content,
            vec![BulletRewriteDecision {
                experience_index: 0,
                bullet_index: 0,
                outcome: BulletRewriteOutcome::Rewritten,
                rationale: "API work aligns with Rust API development.".to_string(),
            }],
        );

        let error = validate_high_emphasis_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error.to_string().contains("one rewrite decision"));
    }

    #[test]
    fn high_emphasis_accepts_complete_truthful_bullet_audit() {
        let base = base_resume();
        let mut content = base.clone();
        content["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");
        content["experience"][0]["bullets"][1] =
            json!("Strengthened production service reliability.");
        let tailored = high_emphasis_tailored_resume(
            content,
            vec![
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 0,
                    outcome: BulletRewriteOutcome::Rewritten,
                    rationale: "API work aligns with Rust API development.".to_string(),
                },
                BulletRewriteDecision {
                    experience_index: 0,
                    bullet_index: 1,
                    outcome: BulletRewriteOutcome::Rewritten,
                    rationale: "Reliability work aligns with the target role.".to_string(),
                },
            ],
        );

        validate_high_emphasis_bullet_rewrites(&base, &tailored).unwrap();
    }

    #[test]
    fn high_emphasis_lists_each_unchanged_bullet_for_the_retry_prompt() {
        let base = base_resume();
        let mut content = base.clone();
        content["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");

        assert_eq!(
            unchanged_experience_bullets(&base, &content),
            "experience 0, bullet 1: Improved reliability."
        );
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
                bullet_rewrite_decisions: vec![],
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
    fn partial_result_points_to_the_variant_without_creating_a_generated_alias() {
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

        let response = partial_tailoring_response(
            &root,
            "test-en".to_string(),
            &variant_json_path,
            &report_json_path,
            &docx_path,
            &pdf_path,
            None,
            BulletKeywordEmphasis::Balanced,
            0,
            base_resume(),
            vec![],
            TailoringReport {
                covered_keywords: vec![],
                omitted_unsupported_keywords: vec![],
                changed_fields: vec![],
                safety_notes: vec![],
                estimated_ats_coverage_score: 80,
                bullet_rewrite_decisions: vec![],
            },
            "passed",
            "failed",
            true,
            "PDF export failed".to_string(),
        );

        assert_eq!(response.tailoring_status, "partial");
        assert_eq!(response.validation_status, "passed");
        assert_eq!(response.fit_status, "failed");
        assert!(response.tailored_content.is_some());
        assert_eq!(response.report.unwrap().estimated_ats_coverage_score, 80);
        assert!(response.content_changes.is_empty());
        assert_eq!(
            response.latest_docx_path.as_deref(),
            Some("resume/variants/test-en/Xevier_T_CV_en.docx")
        );
        assert!(!generated_dir.join("Xevier_T_CV_en.docx").exists());
        assert_eq!(
            std::fs::read(generated_template_output).unwrap(),
            b"existing generated output"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_result_keeps_summary_when_no_document_is_available() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-summary-only-result-{suffix}"));
        let variant_dir = root.join("resume/variants/test-en");
        std::fs::create_dir_all(&variant_dir).unwrap();
        let variant_json_path = variant_dir.join("variant.json");
        let report_json_path = variant_dir.join("tailoring-report.json");
        let docx_path = variant_dir.join("Xevier_T_CV_en.docx");
        let pdf_path = variant_dir.join("Xevier_T_CV_en.pdf");
        std::fs::write(&variant_json_path, b"{}").unwrap();
        std::fs::write(&report_json_path, b"{}").unwrap();

        let response = partial_tailoring_response(
            &root,
            "test-en".to_string(),
            &variant_json_path,
            &report_json_path,
            &docx_path,
            &pdf_path,
            None,
            BulletKeywordEmphasis::High,
            2,
            base_resume(),
            vec![],
            TailoringReport {
                covered_keywords: vec!["Rust".to_string()],
                omitted_unsupported_keywords: vec!["Kubernetes".to_string()],
                changed_fields: vec!["experience.bullets".to_string()],
                safety_notes: vec![],
                estimated_ats_coverage_score: 73,
                bullet_rewrite_decisions: vec![],
            },
            "failed",
            "not_run",
            false,
            "Locked-section validation failed".to_string(),
        );

        assert_eq!(response.tailoring_status, "partial");
        assert_eq!(response.validation_status, "failed");
        assert_eq!(response.fit_status, "not_run");
        assert!(response.latest_docx_path.is_none());
        assert!(response.latest_pdf_path.is_none());
        assert!(response.tailored_content.is_some());
        assert_eq!(response.report.unwrap().estimated_ats_coverage_score, 73);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn counts_pages_from_pdf_structure_instead_of_raw_text() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("resume-page-count-{suffix}.pdf"));
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();
        let catalog_id = document.new_object_id();
        document.objects.insert(
            page_id,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => Object::Reference(pages_id),
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        document.objects.insert(
            catalog_id,
            Object::Dictionary(dictionary! {
                "Type" => "Catalog",
                "Pages" => Object::Reference(pages_id),
            }),
        );
        document.trailer.set("Root", Object::Reference(catalog_id));
        document.save(&path).unwrap();

        assert_eq!(pdf_page_count(&path).unwrap(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn verified_docx_fallback_replaces_stable_docx_and_removes_stale_pdf() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-docx-publish-{suffix}"));
        let variant_dir = root.join("resume/variants/test-en");
        let downloads = root.join("Downloads");
        std::fs::create_dir_all(&variant_dir).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        let variant_json = variant_dir.join("variant.json");
        let docx = variant_dir.join("Xevier_T_CV_en.docx");
        std::fs::write(&variant_json, serde_json::to_vec(&base_resume()).unwrap()).unwrap();
        std::fs::write(&docx, b"validated tailored docx").unwrap();
        std::fs::write(downloads.join("Xevier_T_CV_en.docx"), b"stale base docx").unwrap();
        std::fs::write(downloads.join("Xevier_T_CV_en.pdf"), b"stale prior pdf").unwrap();

        let artifact = publish_verified_artifact(
            &root,
            "test-en",
            &docx,
            "en",
            "docx",
            &variant_json,
            &docx,
            None,
            "passed",
            "failed",
            Some(&downloads),
        )
        .unwrap();

        let published = downloads.join("Xevier_T_CV_en.docx");
        assert_eq!(
            std::fs::read(&published).unwrap(),
            b"validated tailored docx"
        );
        assert!(!downloads.join("Xevier_T_CV_en.pdf").exists());
        assert_eq!(artifact.sha256, sha256_file(&published).unwrap());
        assert!(variant_dir.join("artifact-manifest.json").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_pdf_replaces_stable_pdf_and_removes_stale_docx() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-pdf-publish-{suffix}"));
        let variant_dir = root.join("resume/variants/test-en");
        let downloads = root.join("Downloads");
        std::fs::create_dir_all(&variant_dir).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        let variant_json = variant_dir.join("variant.json");
        let docx = variant_dir.join("Xevier_T_CV_en.docx");
        let pdf = variant_dir.join("Xevier_T_CV_en.pdf");
        std::fs::write(&variant_json, serde_json::to_vec(&base_resume()).unwrap()).unwrap();
        std::fs::write(&docx, b"validated tailored docx").unwrap();
        std::fs::write(&pdf, b"one-page tailored pdf").unwrap();
        std::fs::write(downloads.join("Xevier_T_CV_en.docx"), b"stale prior docx").unwrap();
        std::fs::write(downloads.join("Xevier_T_CV_en.pdf"), b"stale base pdf").unwrap();

        let artifact = publish_verified_artifact(
            &root,
            "test-en",
            &pdf,
            "en",
            "pdf",
            &variant_json,
            &docx,
            Some(&pdf),
            "passed",
            "passed",
            Some(&downloads),
        )
        .unwrap();

        let published = downloads.join("Xevier_T_CV_en.pdf");
        assert_eq!(std::fs::read(&published).unwrap(), b"one-page tailored pdf");
        assert!(!downloads.join("Xevier_T_CV_en.docx").exists());
        assert_eq!(artifact.sha256, sha256_file(&published).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
