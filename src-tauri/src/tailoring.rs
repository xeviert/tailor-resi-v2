use crate::{
    analysis::{find_refusal, incomplete_reason, JobAnalysis},
    api_usage::{record_response_usage, UsageContext},
    ats_score::{
        covered_and_omitted, load_locked_sections, score_ats_coverage, AtsCoverage, MissReason,
    },
    evidence::{
        load_evidence_bank, placement_term_is_covered_in_any, preflight_items, EvidenceBank,
        EvidenceEntry,
    },
    http::{retry_delay, shared_client, status_is_retryable},
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
const MAX_TAILORING_ATTEMPTS: usize = 4;
const MIN_REPLACED_BULLETS: usize = 1;
const MIN_REPLACED_WORDS: usize = 8;
const MAX_LIST_RUN: usize = 8;
const MIN_REPLACED_LENGTH_ALLOWANCE: usize = 140;

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
    /// Rebuilt from the measured ledger after tailoring; the model's own list is discarded.
    pub covered_keywords: Vec<String>,
    pub omitted_unsupported_keywords: Vec<String>,
    pub changed_fields: Vec<String>,
    pub safety_notes: Vec<String>,
    /// The tailoring model's self-assessment. Advisory only — nothing in the prompt tells it
    /// how to derive this, so it is kept for comparison against the measured score rather
    /// than used to drive anything.
    #[serde(rename = "estimated_ats_coverage_score")]
    pub model_estimated_ats_coverage_score: u8,
    /// Measured coverage of the produced document. `None` only for a stored result written
    /// before scoring existed, or when scoring could not run.
    #[serde(default)]
    pub ats_coverage: Option<AtsCoverage>,
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
    Replaced,
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

fn normalize_bullet_rewrite_decisions(base: &serde_json::Value, tailored: &mut TailoredResume) {
    let supplied_outcomes = tailored
        .report
        .bullet_rewrite_decisions
        .iter()
        .map(|decision| {
            (
                (decision.experience_index, decision.bullet_index),
                decision.outcome.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
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
            let claimed_replacement = supplied_outcomes.get(&(experience_index, bullet_index))
                == Some(&BulletRewriteOutcome::Replaced);
            let outcome = if before == after {
                BulletRewriteOutcome::NoRelevantMatch
            } else if claimed_replacement {
                BulletRewriteOutcome::Replaced
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
                    BulletRewriteOutcome::Replaced => {
                        "Bullet was replaced with new job-targeted content.".to_string()
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
    /// Which capture this run is tailoring for, recorded on each usage receipt so a receipt can
    /// be joined to its run instead of guessed at by timestamp. Optional so the legacy
    /// `/analyze` HTTP route, which does not know it, keeps deserializing unchanged.
    #[serde(default)]
    pub capture_id: Option<u64>,
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
    #[error("OpenAI stopped early ({0}); the tailored resume is incomplete")]
    IncompleteResponse(String),
    #[error("OpenAI declined to tailor this resume: {0}")]
    Refused(String),
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

/// The one tailoring mode's bullet rules.
///
/// There is deliberately no replacement ceiling. The old budget of three, one per role, left
/// most of the resume speaking to whatever the previous job wanted, so a posting the base
/// bullets did not address could not be reached however much evidence backed it.
const BULLET_REWRITE_INSTRUCTION: &str = "Experience bullets are the primary ATS surface, not skills. You MUST return every experience bullet with different, truthful text before changing skills; a rewrite counts only when the returned bullet text differs from the input bullet text, and unchanged text must never be labelled as rewritten.\nBeyond rephrasing, REPLACE outright as many bullets as it takes to cover this job's highest-weighted signals. There is no cap and no per-role limit: replace every bullet in the resume if that is what the job calls for, and replace at least one. A bullet whose original angle does not serve this job should be discarded rather than reworded - drop its subject entirely and write a new bullet aimed at the job's strongest ATS signals.\nA replaced bullet must still describe work this same person actually did in that same role during that same period. Ground it in the other facts of the base resume, in the user-attested evidence bank, or in a responsibility directly implied by that role's stated stack, title, and scope. Never introduce a new employer, job title, date, credential, certification, or degree, never invent a metric, and never move an existing metric onto work it did not measure.\nWrite natural professional prose: each replaced bullet must read as one specific accomplishment or responsibility with a concrete object and outcome, not a list of technologies. Keyword stuffing is a failure. Keep each replaced bullet close to the length of the bullet it replaced so the locked DOCX layout stays stable.\nMark every replaced bullet with outcome \"replaced\" and give a rationale naming both the job signal it now targets and the grounding it relies on. Every other bullet uses outcome \"rewritten\"; no_relevant_match is not allowed. A skills-only response is invalid.\n";

/// Mirrors the replacement licence above: a responsibility implied by a role's own stack, title,
/// and scope is fair game inside a bullet this run replaces outright, and nowhere else.
const INVENTION_RULE: &str = "Do not invent credentials, employers, tools, metrics, education, certifications, or experience. Inside a replaced bullet only, you may state a responsibility that is directly implied by that role's stated stack, title, and scope even without an explicit evidence entry; never one a person in that role would not routinely own.\n";

/// Renders the job-matched evidence entries for the prompt.
///
/// The obvious encoding - `serde_json::to_string` over the entry structs - spends about four
/// characters of JSON punctuation for every character of actual term, because each entry
/// repeats the same four keys while every entry in the bank carries `user_attested: true` and
/// all but one carry no proof note. Grouping by the two attributes that actually vary says
/// the same thing in roughly a third of the tokens: the terms, their kinds, which ones may be
/// placed into a role's bullets, and the rare proof note.
fn render_evidence_block(entries: &[EvidenceEntry]) -> String {
    if entries.is_empty() {
        return "User-attested evidence bank: (none matched this job)\n".to_string();
    }

    let mut block = String::from(
        "User-attested evidence bank - each term is a capability this person has actually practised:\n",
    );

    // Stable, meaningful order rather than whatever order the preflight happened to emit.
    for kind in ["technology", "method_domain", "responsibility"] {
        let terms = entries
            .iter()
            .filter(|entry| entry.user_attested && entry.kind == kind)
            .map(|entry| entry.term.as_str())
            .collect::<Vec<_>>();
        if !terms.is_empty() {
            block.push_str(&format!("{kind}: {}\n", terms.join(", ")));
        }
    }
    // An unexpected kind still has to reach the model. Grouping must never become a filter.
    let other = entries
        .iter()
        .filter(|entry| {
            entry.user_attested
                && !matches!(
                    entry.kind.as_str(),
                    "technology" | "method_domain" | "responsibility"
                )
        })
        .map(|entry| format!("{} ({})", entry.term, entry.kind))
        .collect::<Vec<_>>();
    if !other.is_empty() {
        block.push_str(&format!("other: {}\n", other.join(", ")));
    }

    // Every entry reaching this function is attested today, but the struct allows otherwise and
    // a term the user has not vouched for must not be folded in with the ones they have.
    let unattested = entries
        .iter()
        .filter(|entry| !entry.user_attested)
        .map(|entry| entry.term.as_str())
        .collect::<Vec<_>>();
    if !unattested.is_empty() {
        block.push_str(&format!(
            "Not user-attested - do not claim these: {}\n",
            unattested.join(", ")
        ));
    }

    let placeable = entries
        .iter()
        .filter(|entry| entry.user_attested && entry.allow_model_role_placement)
        .map(|entry| entry.term.as_str())
        .collect::<Vec<_>>();
    if !placeable.is_empty() {
        block.push_str(&format!(
            "Authorized for placement into an existing role's bullets: {}\n",
            placeable.join(", ")
        ));
    }

    let notes = entries
        .iter()
        .filter_map(|entry| {
            entry
                .proof_note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .map(|note| format!("- {}: {note}\n", entry.term))
        })
        .collect::<String>();
    if !notes.is_empty() {
        block.push_str("Proof notes:\n");
        block.push_str(&notes);
    }

    block
}

/// Builds the tailoring user message.
///
/// Ordering here is about cost. The provider caches the longest constant prefix of a request
/// and bills those tokens at a fraction of the normal rate, but only when that prefix is
/// byte-identical to a recent one and at least 1024 tokens long. So the message is built in
/// three zones, most-constant first:
///
/// * Zone A never varies: the instruction text, then the base resume. About 1850 tokens,
///   identical for every tailoring call in a language, and identical up to the base resume
///   across languages.
/// * Zone B varies per job: output language, job post, analysis, and matched evidence.
/// * Zone C varies per attempt: the two retry-feedback blocks.
///
/// Nothing volatile may move above something constant, or the constant text below it stops
/// being a shared prefix and gets billed in full on every call. The earlier layout put the
/// constant base resume *after* the per-job job and analysis payloads and interpolated the
/// language into the very first line, which left almost nothing cacheable - the receipts in
/// `data/api-usage/` show retries ten seconds apart getting zero cached tokens.
pub fn build_tailoring_prompt(
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    approved_evidence: &[EvidenceEntry],
    priority_attested_terms: &[String],
    concise: bool,
    correction_instruction: Option<&str>,
) -> String {
    let parsed_job = serde_json::to_string(&crate::server::prompt_job_view(parsed_job))
        .unwrap_or_else(|_| "{}".to_string());
    let analysis = serde_json::to_string(analysis).unwrap_or_else(|_| "{}".to_string());
    let base_resume = serde_json::to_string(base_resume).unwrap_or_else(|_| "{}".to_string());
    let placement_terms = priority_attested_terms.to_vec();
    let evidence_block = render_evidence_block(approved_evidence);
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
            "The user explicitly attested the following claims and authorized you to place them in the most plausible existing role: {}. You MUST incorporate every selected claim naturally in one or more experience bullets. You may completely replace the least job-relevant existing bullet claims to make room, while keeping the same jobs and bullet counts. Multiple selected claims may share a bullet when natural. The attestation supports only the named claims; do not invent adjacent details, metrics, employers, dates, credentials, or responsibilities.\n\n",
            placement_terms.join(", ")
        )
    } else {
        String::new()
    };

    format!(
        // Zone A - constant. Nothing volatile may be interpolated above the base resume.
        "Tailor the resume JSON below for maximum truthful ATS alignment against the job post that follows it.\n\
         Return only JSON matching the schema. Preserve the input resume shape exactly.\n\
         Rewrite only the professional summary, experience bullet text, and skills strings.\n\
         Do not change meta, company names, locations, titles, dates, job order, number of jobs, number of bullets, or skill keys.\n\
         Aggressively incorporate ATS keywords, tools, responsibility phrases, and domain wording when the base resume supports them.\n\
         Applicant tracking systems match literal strings, so write each supported term the way this job post writes it. The analysis term_variants list gives the alternate written forms; when an acronym and its expansion are both in common use, name both once in the skills line where it reads naturally, and use the post's own form in bullets.\n\
         {BULLET_REWRITE_INSTRUCTION}\
         Every entry in the user-attested evidence bank is a capability this person has actually practised. Treat each one as usable in an experience bullet, not only in a skills string: place it in the most plausible existing role given that role's stated stack, title, and scope. The attestation covers the named capability only - never invent an adjacent metric, employer, title, date, credential, or certification around it.\n\
         {INVENTION_RULE}\
         Put important job keywords without base-resume or user-attested evidence into omitted_unsupported_keywords instead of adding them to the resume.\n\
         Keep each rewritten bullet close to the original length so the locked DOCX layout remains stable.\n\
         Keep the professional summary to the same sentence count and approximate length as the original, leading with the job's own role wording and its highest-weighted supported terms.\n\n\
         Base resume JSON:\n{base_resume}\n\n\
         \
         Output language: {language}. The base resume above is written in it; keep every value you rewrite in that same language.\n\n\
         {placement_instruction}\
         Normalized job JSON:\n{parsed_job}\n\n\
         ATS analysis JSON:\n{analysis}\n\n\
         {evidence_block}\n\
         {concise_instruction}\
         {correction_instruction}",
    )
}

fn resume_content_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["meta", "summary", "experience", "skills"],
        "properties": {
            "summary": { "type": "string" },
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
                                "outcome": { "type": "string", "enum": ["rewritten", "no_relevant_match", "replaced"] },
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
    concise: bool,
    correction_instruction: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "prompt_cache_key": crate::http::PROMPT_CACHE_KEY_RESUME_TAILORING,
        "input": [
            {
                "role": "system",
                "content": "You rewrite resume JSON for ATS alignment. You must be truthful, evidence-bound, and preserve all locked layout constraints."
            },
            {
                "role": "user",
                "content": build_tailoring_prompt(language, parsed_job, analysis, base_resume, approved_evidence, priority_attested_terms, concise, correction_instruction)
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

/// Transport retries for one tailoring attempt.
///
/// This is not the correction loop - it is the backoff that analysis and job import have always
/// had and tailoring did not. A single 429 used to end the whole run, discarding an analysis
/// call that was already paid for and making the user start over, which costs strictly more
/// than waiting a second and asking again.
const MAX_TAILORING_REQUEST_ATTEMPTS: u32 = 3;

#[allow(clippy::too_many_arguments)]
pub async fn tailor_resume(
    config: &TailoringConfig,
    language: &str,
    parsed_job: &serde_json::Value,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    approved_evidence: &[EvidenceEntry],
    priority_attested_terms: &[String],
    concise: bool,
    correction_instruction: Option<&str>,
    usage_context: UsageContext,
) -> Result<TailoredResume, TailoringError> {
    validate_language(language)?;
    let request_body = build_tailoring_request(
        &config.model,
        language,
        parsed_job,
        analysis,
        base_resume,
        approved_evidence,
        priority_attested_terms,
        concise,
        correction_instruction,
    );
    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));
    let mut last_error = None;

    for attempt in 0..MAX_TAILORING_REQUEST_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(retry_delay(attempt - 1)).await;
        }

        let response = match shared_client()
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&request_body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(TailoringError::Request(error.to_string()));
                continue;
            }
        };

        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(TailoringError::Request(error.to_string()));
                continue;
            }
        };

        if !status.is_success() {
            let error = TailoringError::Http { status, body };
            if status_is_retryable(status) {
                last_error = Some(error);
                continue;
            }
            return Err(error);
        }

        // Before the parse: a refused or truncated response is billed like any other, and a
        // response that fails to parse is precisely the kind this loop exists to investigate.
        record_response_usage("resume_tailoring", &config.model, &body, usage_context);
        let tailored = parse_tailored_resume_from_response(&body)?;
        return Ok(tailored);
    }

    Err(last_error.unwrap_or_else(|| TailoringError::Request("no attempt was made".to_string())))
}

pub fn parse_tailored_resume_from_response(body: &str) -> Result<TailoredResume, TailoringError> {
    if body.trim().is_empty() {
        return Err(TailoringError::EmptyResponseBody);
    }
    let response: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    if let Some(refusal) = find_refusal(&response) {
        return Err(TailoringError::Refused(refusal.to_string()));
    }
    let text = match find_output_text(&response) {
        Some(text) => text,
        None => {
            return Err(match incomplete_reason(&response) {
                Some(reason) => TailoringError::IncompleteResponse(reason.to_string()),
                None => TailoringError::MissingOutputText,
            });
        }
    };
    if text.trim().is_empty() {
        return Err(TailoringError::EmptyOutputText);
    }
    if let Some(reason) = incomplete_reason(&response) {
        return Err(TailoringError::IncompleteResponse(reason.to_string()));
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

    if tailored["summary"]
        .as_str()
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true)
    {
        return invalid("summary is empty");
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

fn validate_full_bullet_rewrites(
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
        return invalid(
            "every experience bullet must be rewritten; skills-only tailoring is not accepted",
        );
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
        return invalid("one rewrite decision is required for every experience bullet");
    }
    Ok(())
}

/// Returns false when a replaced bullet is too short to be a real accomplishment.
fn replacement_is_substantial(text: &str) -> bool {
    text.split_whitespace().count() >= MIN_REPLACED_WORDS
}

/// Returns false when a replaced bullet reads as a keyword dump rather than prose.
///
/// The prompt is the primary quality control; this is a deliberately loose floor that only
/// catches egregious output. An existing base bullet already names six tools in one list, so
/// the run limit sits above that.
fn replacement_reads_as_prose(text: &str) -> bool {
    let mut longest_run = 0usize;
    let mut current_run = 0usize;
    for fragment in text.split(',') {
        if fragment.split_whitespace().count() <= 3 {
            current_run += 1;
            longest_run = longest_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    longest_run <= MAX_LIST_RUN
}

/// Keeps a replacement close enough to the bullet it replaced that the locked DOCX layout holds.
/// The absolute allowance lets a very short base bullet grow to a normal length.
fn replacement_length_is_stable(base_bullet: &str, replaced_bullet: &str) -> bool {
    let limit = (base_bullet.chars().count() * 3 / 2).max(MIN_REPLACED_LENGTH_ALLOWANCE);
    replaced_bullet.chars().count() <= limit
}

/// Every bullet must be rewritten and at least one replaced outright.
///
/// There is no upper bound and no per-role limit: replacing every bullet in the resume is a
/// legitimate answer to a job the base bullets do not speak to. The floor is what stops a run
/// from passing on rephrasing alone. What still constrains a replacement is its content - it
/// must read as prose and stay near the length of the bullet it replaced, because the DOCX
/// layout is locked to one page.
fn validate_bullet_rewrites(
    base: &serde_json::Value,
    tailored: &TailoredResume,
) -> Result<(), TailoringError> {
    validate_full_bullet_rewrites(base, tailored)?;

    let replacements = tailored
        .report
        .bullet_rewrite_decisions
        .iter()
        .filter(|decision| decision.outcome == BulletRewriteOutcome::Replaced)
        .collect::<Vec<_>>();

    if replacements.len() < MIN_REPLACED_BULLETS {
        return invalid(&format!(
            "tailoring requires at least {MIN_REPLACED_BULLETS} replaced experience bullet, but {} were marked replaced",
            replacements.len()
        ));
    }

    for replacement in &replacements {
        let base_bullet = base["experience"][replacement.experience_index]["bullets"]
            [replacement.bullet_index]
            .as_str()
            .unwrap_or_default();
        let replaced_bullet = tailored.content["experience"][replacement.experience_index]
            ["bullets"][replacement.bullet_index]
            .as_str()
            .unwrap_or_default();

        if !replacement_is_substantial(replaced_bullet) {
            return invalid(&format!(
                "replaced experience bullet {}/{} is too short to describe a real accomplishment",
                replacement.experience_index, replacement.bullet_index
            ));
        }
        if !replacement_reads_as_prose(replaced_bullet) {
            return invalid(&format!(
                "replaced experience bullet {}/{} reads as a keyword list instead of prose",
                replacement.experience_index, replacement.bullet_index
            ));
        }
        if !replacement_length_is_stable(base_bullet, replaced_bullet) {
            return invalid(&format!(
                "replaced experience bullet {}/{} is too long for the locked layout",
                replacement.experience_index, replacement.bullet_index
            ));
        }
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

/// Names the selected claims that did not actually land in a bullet.
///
/// Each bullet is tested on its own rather than against the concatenation of all of them. Joined,
/// a phrase counted as placed whenever its individual words happened to be scattered across
/// different bullets, so a term the user explicitly asked for could be reported as covered while
/// appearing nowhere as a claim - and the retry below never fired.
fn missing_model_placement_terms(
    tailored: &serde_json::Value,
    required_terms: &[String],
) -> Vec<String> {
    let bullets = tailored["experience"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|job| job["bullets"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    required_terms
        .iter()
        .filter(|term| !placement_term_is_covered_in_any(bullets.iter().copied(), term))
        .cloned()
        .collect()
}

/// Measures how much of the job's keyword surface the produced resume actually covers, and
/// rebuilds the report's covered/omitted lists from that measurement.
///
/// The model supplies its own versions of both lists, but nothing ever checked them against
/// the text it wrote — a term could sit in `covered_keywords` while being absent from every
/// bullet. Measuring here makes the report describe the document rather than the intent.
///
/// A failure to read the locked sections or the evidence bank degrades the result rather than
/// failing the run: a resume that renders is worth more to the user than a coverage number.
/// Takes the bank by reference rather than reading it.
///
/// This runs once per successful attempt, and the evidence bank is a 78 KB file with several
/// hundred entries. Re-reading and re-parsing it on every attempt of a four-attempt run bought
/// nothing: it cannot change while a run is in flight.
fn apply_measured_coverage(
    root: &Path,
    language: &str,
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    bank: &EvidenceBank,
    tailored: &mut TailoredResume,
) {
    let locked = match load_locked_sections(root, language) {
        Ok(locked) => locked,
        Err(error) => {
            eprintln!("[ats] Scoring without the locked sections: {error}");
            None
        }
    };

    let preflight = preflight_items(analysis, base_resume, bank);
    let coverage = score_ats_coverage(analysis, &tailored.content, locked.as_ref(), &preflight);
    let (covered, omitted) = covered_and_omitted(&coverage);

    tailored.report.covered_keywords = covered;
    tailored.report.omitted_unsupported_keywords = omitted;
    tailored.report.ats_coverage = Some(coverage);
}

/// Weight below which a dropped-but-supported term is not worth another model call.
///
/// The ledger weights are `required_skills` 5, `tools_and_platforms` and
/// `responsibility_phrases` 4, `preferred_skills` and `domain_terms` 3, and for a
/// `core_keyword` the model's own 1-5 importance. Retrying for a 3 spends a whole generation
/// on something the post itself called optional; 4 is where it is saying it needs this.
const MIN_UNPLACED_EVIDENCE_WEIGHT: u8 = 4;

/// Names the high-value terms this person can already prove that the resume still does not carry.
///
/// Approving a term at the evidence step is permission, not obligation. The prompt only asks the
/// model to incorporate supported terms "aggressively", and against a fixed bullet count and a
/// one-page budget that request loses to whatever the model rated higher - while the only hard
/// requirement, `priority_attested_terms`, is empty on a first run. Nothing used to notice, so
/// free and truthful coverage was dropped silently and the summary screen had to ask the user to
/// go and fetch it by hand.
fn unplaced_supported_terms(coverage: &AtsCoverage) -> Vec<String> {
    coverage
        .terms
        .iter()
        .filter(|term| {
            term.miss_reason == Some(MissReason::EvidenceNotPlaced)
                && term.weight >= MIN_UNPLACED_EVIDENCE_WEIGHT
        })
        .map(|term| term.term.clone())
        .collect()
}

pub(crate) fn content_changes(
    base: &serde_json::Value,
    tailored: &serde_json::Value,
) -> Vec<ContentChange> {
    let mut changes = Vec::new();

    // Not `.expect()` like the rest of this function: recovery from artifacts replays a
    // `variant.json` that may have been written before the summary field existed.
    if let (Some(before), Some(after)) = (base["summary"].as_str(), tailored["summary"].as_str()) {
        if before != after {
            changes.push(ContentChange {
                path: "/summary".to_string(),
                before: before.to_string(),
                after: after.to_string(),
            });
        }
    }

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
    // Loaded once, like the base resume above: neither can change while the run is in flight,
    // and this one is a 78 KB parse.
    let evidence_bank = load_evidence_bank(&root).unwrap_or_else(|error| {
        eprintln!("[ats] Scoring without the evidence bank: {error}");
        EvidenceBank::default()
    });
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
            needs_concise_rewrite,
            correction_instruction.as_deref(),
            UsageContext::attempt(request.capture_id, attempt as u32),
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
            // A shape or locked-field violation used to end the run here, throwing away a
            // generation that was already billed and leaving the user to start over - which
            // spends another full call to ask the same question. The bullet-rewrite and
            // placement validators below have always handed the model its mistake and tried
            // again; this one is no different in kind, so it uses the same attempt budget.
            // The constraint itself is unchanged: a response that never satisfies it still
            // fails, just after the budget is spent rather than before.
            if attempt < MAX_TAILORING_ATTEMPTS {
                correction_instruction = Some(format!(
                    "Your preceding response was rejected: {error}. Return the resume JSON with \
                     exactly the input shape: same meta, same companies, locations, titles, dates \
                     and job order, same number of jobs, same number of bullets per job, and the \
                     same skill keys. Rewrite only the professional summary, the experience \
                     bullet text, and the skills strings.\n\n"
                ));
                continue;
            }
            return Err(error);
        }
        normalize_bullet_rewrite_decisions(&base_resume, &mut tailored);
        if let Err(error) = validate_bullet_rewrites(&base_resume, &tailored) {
            if attempt < MAX_TAILORING_ATTEMPTS {
                let unchanged = unchanged_experience_bullets(&base_resume, &tailored.content);
                let unchanged_section = if unchanged.is_empty() {
                    String::new()
                } else {
                    format!("\n\nUnchanged bullets:\n{unchanged}")
                };
                correction_instruction = Some(format!(
                    "Your preceding response was rejected: {error}. Return every experience bullet with different, truthful text, and mark at least {MIN_REPLACED_BULLETS} of them as \"replaced\". Replace as many as this job warrants - there is no cap and no per-role limit. A replaced bullet must drop its original angle, target this job's strongest signals, stay grounded in this person's real work in that role, and read as natural prose rather than a list of technologies. Do not compensate by adding more skills or by changing only the report.{unchanged_section}\n\n"
                ));
                progress(
                    reporter,
                    "safety_validation",
                    "retrying",
                    "Tailoring requires grounded bullet replacements; requesting a corrected response.",
                    Some(attempt),
                );
                continue;
            }
            let final_error = invalid_message(&format!(
                "tailoring could not satisfy the experience-bullet requirements after {MAX_TAILORING_ATTEMPTS} attempts: {error}"
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
        apply_measured_coverage(
            &root,
            language,
            &request.analysis,
            &base_resume,
            &evidence_bank,
            &mut tailored,
        );
        // Terms this person can already prove, that the pass dropped anyway, are worth exactly
        // one more call: they need no attestation, so placing them raises the score without
        // asking the user to vouch for anything. This fires only on the first attempt, which
        // leaves the entire remaining budget to the validators above - it is an improvement pass
        // on a response that already satisfied them, and it must never be the reason a run runs
        // out of attempts. If the corrected response drops them again, that is the answer and the
        // run proceeds: a rendered resume is worth more to the user than a coverage number.
        if attempt == 1 {
            let unplaced = tailored
                .report
                .ats_coverage
                .as_ref()
                .map(unplaced_supported_terms)
                .unwrap_or_default();
            if !unplaced.is_empty() {
                correction_instruction = Some(format!(
                    "Your preceding response left out job keywords this person already has \
                     evidence for: {}. These need no new attestation - the base resume or the \
                     user-attested evidence bank already supports them. Incorporate every one of \
                     them naturally into the experience bullets or the skills strings, using the \
                     job post's own wording. Keep the same jobs, the same bullet counts, and the \
                     same locked fields, and do not invent any adjacent metric, employer, title, \
                     date, credential, or certification around them.\n\n",
                    unplaced.join(", ")
                ));
                progress(
                    reporter,
                    "safety_validation",
                    "retrying",
                    format!(
                        "Keywords this resume already has evidence for were left out; requesting a corrected response: {}.",
                        unplaced.join(", ")
                    ),
                    Some(attempt),
                );
                continue;
            }
        }
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
        missing_model_placement_terms, normalize_bullet_rewrite_decisions,
        parse_tailored_resume_from_response, partial_tailoring_response, pdf_page_count,
        publish_verified_artifact, render_evidence_block, replacement_is_substantial,
        replacement_length_is_stable, replacement_reads_as_prose, sha256_file, slugify,
        unchanged_experience_bullets, unplaced_supported_terms, validate_bullet_rewrites,
        validate_full_bullet_rewrites, validate_tailored_content, write_variant_files,
        BulletRewriteDecision, BulletRewriteOutcome, TailoredResume, TailoringReport,
        MAX_COMPANY_ROLE_SLUG_LEN,
    };
    use crate::analysis::{JobAnalysis, KeywordSignal};
    use crate::ats_score::{AtsCoverage, MissReason, TermCoverage};
    use crate::evidence::EvidenceEntry;
    use lopdf::{dictionary, Document, Object};
    use serde_json::json;

    fn missed_term(term: &str, weight: u8, reason: Option<MissReason>) -> TermCoverage {
        TermCoverage {
            term: term.to_string(),
            kind: "technology".to_string(),
            group: "required".to_string(),
            weight,
            covered: reason.is_none(),
            coverage_ratio: if reason.is_none() { 1.0 } else { 0.0 },
            matched_in: None,
            in_editable_surface: reason.is_none(),
            miss_reason: reason,
        }
    }

    fn coverage_of(terms: Vec<TermCoverage>) -> AtsCoverage {
        AtsCoverage {
            score: 50,
            covered_weight: 0,
            total_weight: terms.iter().map(|term| u32::from(term.weight)).sum(),
            editable_covered_weight: 0,
            categories: Vec::new(),
            terms,
        }
    }

    /// The retry exists for coverage the run could have had for free, so it must fire on a miss
    /// the preflight already cleared - and stay quiet for one that needs the user to attest.
    #[test]
    fn only_already_supported_misses_are_worth_another_attempt() {
        let coverage = coverage_of(vec![
            missed_term("Kubernetes", 5, Some(MissReason::EvidenceNotPlaced)),
            missed_term("Terraform", 5, Some(MissReason::NoEvidence)),
            missed_term("Rust", 5, None),
        ]);
        assert_eq!(
            unplaced_supported_terms(&coverage),
            vec!["Kubernetes".to_string()]
        );
    }

    /// A whole extra generation is too expensive to spend on a term the post itself called
    /// optional, so the low-weight groups - `preferred_skills` and `domain_terms` at 3, and a
    /// `core_keyword` the model rated below that - do not trigger it.
    #[test]
    fn low_weight_misses_do_not_trigger_another_attempt() {
        let coverage = coverage_of(vec![
            missed_term("GraphQL", 3, Some(MissReason::EvidenceNotPlaced)),
            missed_term("gRPC", 1, Some(MissReason::EvidenceNotPlaced)),
        ]);
        assert!(unplaced_supported_terms(&coverage).is_empty());
    }

    /// A run whose document scored nothing at all still has to finish. The gate reads coverage
    /// through an `Option`, and treating a missing measurement as "nothing was left behind" is
    /// what keeps a scoring failure from turning into an extra billed attempt.
    #[test]
    fn a_fully_covered_document_asks_for_no_extra_attempt() {
        let coverage = coverage_of(vec![missed_term("Rust", 5, None)]);
        assert!(unplaced_supported_terms(&coverage).is_empty());
    }

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
            term_variants: vec![],
            summary: "Emphasize Rust API work.".to_string(),
        }
    }

    fn base_resume() -> serde_json::Value {
        json!({
            "meta": { "language": "en", "type": "base", "template": "Xevier_T_CV_en.template.docx" },
            "summary": "Engineer with six years building reliable backend services.",
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

    /// Two roles so the "at most one replacement per role" rule is exercisable.
    fn multi_role_base_resume() -> serde_json::Value {
        json!({
            "meta": { "language": "en", "type": "base", "template": "Xevier_T_CV_en.template.docx" },
            "summary": "Engineer with six years building reliable backend services.",
            "experience": [
                {
                    "company": "Acme",
                    "location": "Remote",
                    "title": "Engineer",
                    "dates": "2024 - Present",
                    "bullets": ["Built APIs for the billing platform.", "Improved reliability of nightly jobs."]
                },
                {
                    "company": "Globex",
                    "location": "Remote",
                    "title": "Developer",
                    "dates": "2022 - 2024",
                    "bullets": ["Shipped the customer dashboard rewrite."]
                }
            ],
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

    fn decision(
        experience_index: usize,
        bullet_index: usize,
        outcome: BulletRewriteOutcome,
    ) -> BulletRewriteDecision {
        BulletRewriteDecision {
            experience_index,
            bullet_index,
            outcome,
            rationale: "Targets the job's primary signal.".to_string(),
        }
    }

    fn max_report(decisions: Vec<BulletRewriteDecision>) -> TailoringReport {
        TailoringReport {
            covered_keywords: vec![],
            omitted_unsupported_keywords: vec![],
            changed_fields: vec![],
            safety_notes: vec![],
            model_estimated_ats_coverage_score: 80,
            ats_coverage: None,
            bullet_rewrite_decisions: decisions,
        }
    }

    /// All three bullets differ from the base, so only the replacement rules can fail.
    fn max_tailored(bullets: [&str; 3], decisions: Vec<BulletRewriteDecision>) -> TailoredResume {
        TailoredResume {
            content: json!({
                "meta": { "language": "en", "type": "base", "template": "Xevier_T_CV_en.template.docx" },
                "experience": [
                    { "bullets": [bullets[0], bullets[1]] },
                    { "bullets": [bullets[2]] }
                ],
                "skills": {}
            }),
            report: max_report(decisions),
        }
    }

    const REWRITTEN_A: &str = "Built and operated billing platform APIs used across the company.";
    const REWRITTEN_B: &str = "Raised nightly job reliability through better retry handling.";
    const REPLACED_C: &str =
        "Owned end-to-end delivery of the customer dashboard rewrite, partnering with design on scope.";

    #[test]
    fn tailoring_prompt_requires_uncapped_grounded_replacements() {
        let prompt = build_tailoring_prompt(
            "en",
            &json!({"title": "Rust Engineer"}),
            &analysis(),
            &base_resume(),
            &[],
            &[],
            false,
            None,
        );

        assert!(prompt.contains("There is no cap and no per-role limit"));
        assert!(prompt.contains("replace at least one"));
        assert!(prompt.contains("not a list of technologies"));
        assert!(prompt.contains("Keyword stuffing is a failure"));
        assert!(prompt.contains("never invent a metric"));
        assert!(prompt.contains("Inside a replaced bullet only, you may state a responsibility"));
        // The whole bank is bullet-eligible now; the old skills-string gate must be gone.
        assert!(prompt.contains("Treat each one as usable in an experience bullet"));
        assert!(!prompt.contains("Use it in an experience bullet only when its proof_note"));
    }

    #[test]
    fn accepts_a_grounded_replacement() {
        let base = multi_role_base_resume();
        let tailored = max_tailored(
            [REWRITTEN_A, REWRITTEN_B, REPLACED_C],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        validate_bullet_rewrites(&base, &tailored).unwrap();
    }

    #[test]
    fn requires_at_least_one_replacement() {
        let base = multi_role_base_resume();
        let tailored = max_tailored(
            [REWRITTEN_A, REWRITTEN_B, REPLACED_C],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Rewritten),
            ],
        );

        let error = validate_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error
            .to_string()
            .contains("requires at least 1 replaced experience bullet"));
    }

    /// The point of the change: four roles, four replacements, no complaint. The old rule
    /// capped this at three and would have rejected it.
    #[test]
    fn accepts_more_replacements_than_the_old_budget_allowed() {
        let base = json!({
            "meta": { "language": "en", "type": "base", "template": "t.docx" },
            "experience": [
                { "bullets": ["Base one here for the team."] },
                { "bullets": ["Base two here for the team."] },
                { "bullets": ["Base three here for the team."] },
                { "bullets": ["Base four here for the team."] }
            ],
            "skills": {}
        });
        let tailored = TailoredResume {
            content: json!({
                "experience": [
                    { "bullets": [REPLACED_C] },
                    { "bullets": [REPLACED_C] },
                    { "bullets": [REPLACED_C] },
                    { "bullets": [REPLACED_C] }
                ]
            }),
            report: max_report(vec![
                decision(0, 0, BulletRewriteOutcome::Replaced),
                decision(1, 0, BulletRewriteOutcome::Replaced),
                decision(2, 0, BulletRewriteOutcome::Replaced),
                decision(3, 0, BulletRewriteOutcome::Replaced),
            ]),
        };

        validate_bullet_rewrites(&base, &tailored).unwrap();
    }

    #[test]
    fn accepts_two_replacements_inside_one_role() {
        let base = multi_role_base_resume();
        let tailored = max_tailored(
            [REPLACED_C, REPLACED_C, REWRITTEN_A],
            vec![
                decision(0, 0, BulletRewriteOutcome::Replaced),
                decision(0, 1, BulletRewriteOutcome::Replaced),
                decision(1, 0, BulletRewriteOutcome::Rewritten),
            ],
        );

        validate_bullet_rewrites(&base, &tailored).unwrap();
    }

    #[test]
    fn rejects_a_replacement_too_short_to_be_an_accomplishment() {
        let base = multi_role_base_resume();
        let tailored = max_tailored(
            [REWRITTEN_A, REWRITTEN_B, "Owned the dashboard."],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        let error = validate_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error.to_string().contains("too short"));
    }

    #[test]
    fn inherits_the_every_bullet_rewritten_rule() {
        let base = multi_role_base_resume();
        let tailored = max_tailored(
            [
                "Built APIs for the billing platform.",
                REWRITTEN_B,
                REPLACED_C,
            ],
            vec![
                decision(0, 0, BulletRewriteOutcome::NoRelevantMatch),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        let error = validate_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error
            .to_string()
            .contains("every experience bullet must be rewritten"));
    }

    #[test]
    fn rejects_a_term_stuffed_replacement() {
        let base = multi_role_base_resume();
        let stuffed =
            "Rust, Kubernetes, Axum, Docker, Terraform, Kafka, Redis, GraphQL, gRPC, Postgres, Helm";
        let tailored = max_tailored(
            [REWRITTEN_A, REWRITTEN_B, stuffed],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        let error = validate_bullet_rewrites(&base, &tailored).unwrap_err();
        assert!(error.to_string().contains("reads as a keyword list"));
    }

    #[test]
    fn prose_check_accepts_the_tool_list_style_the_base_resume_already_uses() {
        let existing = "Architected RAG and multi-agent systems using Claude, OpenAI, LangChain, LangGraph, ChromaDB, and pgvector, reducing average response times by 40%.";
        assert!(replacement_reads_as_prose(existing));
        assert!(!replacement_reads_as_prose(
            "Rust, Go, Java, Kotlin, Scala, Elixir, Ruby, Perl, Lua, Nim, Zig"
        ));
        // Length is the other half of the pair, and the validator checks both.
        assert!(!replacement_is_substantial("Rust, Go, Java"));
        assert!(!replacement_is_substantial("Too short."));
        assert!(replacement_is_substantial(existing));
    }

    #[test]
    fn replacement_length_guard_allows_growth_but_caps_runaway_text() {
        let short_base = "Shipped the dashboard.";
        assert!(replacement_length_is_stable(short_base, REPLACED_C));
        assert!(!replacement_length_is_stable(short_base, &"x".repeat(200)));
    }

    #[test]
    fn normalizer_preserves_a_replaced_claim_on_changed_text() {
        let base = multi_role_base_resume();
        let mut tailored = max_tailored(
            [REWRITTEN_A, REWRITTEN_B, REPLACED_C],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        normalize_bullet_rewrite_decisions(&base, &mut tailored);
        assert_eq!(
            tailored.report.bullet_rewrite_decisions[2].outcome,
            BulletRewriteOutcome::Replaced
        );
    }

    #[test]
    fn normalizer_downgrades_a_replaced_claim_on_unchanged_text() {
        let base = multi_role_base_resume();
        let mut tailored = max_tailored(
            [
                REWRITTEN_A,
                REWRITTEN_B,
                "Shipped the customer dashboard rewrite.",
            ],
            vec![
                decision(0, 0, BulletRewriteOutcome::Rewritten),
                decision(0, 1, BulletRewriteOutcome::Rewritten),
                decision(1, 0, BulletRewriteOutcome::Replaced),
            ],
        );

        normalize_bullet_rewrite_decisions(&base, &mut tailored);
        assert_eq!(
            tailored.report.bullet_rewrite_decisions[2].outcome,
            BulletRewriteOutcome::NoRelevantMatch
        );
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
            false,
            None,
        );

        assert!(prompt.contains(
            "Rewrite only the professional summary, experience bullet text, and skills strings"
        ));
        assert!(prompt.contains("Do not invent"));
        assert!(prompt.contains("omitted_unsupported_keywords"));
        assert!(prompt.contains("Rust Engineer"));
    }

    /// The provider bills a cached prefix at a fraction of the normal rate, but only when that
    /// prefix is byte-identical to a recent request and at least 1024 tokens long. That makes
    /// the constant zone a property worth testing rather than a style preference: anything
    /// volatile that drifts above the marker silently costs full price on every call from then
    /// on, with no failing test and nothing visible in the UI to notice.
    #[test]
    fn tailoring_prompt_keeps_a_stable_cacheable_prefix() {
        const MARKER: &str = "\nOutput language: ";

        let evidence = vec![EvidenceEntry {
            term: "Kubernetes".to_string(),
            kind: "technology".to_string(),
            proof_note: Some("ran the 2024 migration".to_string()),
            user_attested: true,
            allow_model_role_placement: true,
        }];
        let other_job = json!({
            "title": "Platform Lead",
            "company": "Globex",
            "description": "Totally different posting with different wording."
        });
        let job = json!({"title": "Rust Engineer"});

        // Every axis the prompt varies on: job, language, evidence, the re-tailor placement
        // path, and both retry-feedback blocks.
        let variants = [
            build_tailoring_prompt(
                "en",
                &job,
                &analysis(),
                &base_resume(),
                &[],
                &[],
                false,
                None,
            ),
            build_tailoring_prompt(
                "en",
                &other_job,
                &analysis(),
                &base_resume(),
                &[],
                &[],
                false,
                None,
            ),
            build_tailoring_prompt(
                "fr",
                &job,
                &analysis(),
                &base_resume(),
                &[],
                &[],
                false,
                None,
            ),
            build_tailoring_prompt(
                "en",
                &job,
                &analysis(),
                &base_resume(),
                &evidence,
                &[],
                false,
                None,
            ),
            build_tailoring_prompt(
                "en",
                &job,
                &analysis(),
                &base_resume(),
                &evidence,
                &["Kubernetes".to_string()],
                false,
                None,
            ),
            build_tailoring_prompt(
                "en",
                &job,
                &analysis(),
                &base_resume(),
                &[],
                &[],
                true,
                None,
            ),
            build_tailoring_prompt(
                "en",
                &job,
                &analysis(),
                &base_resume(),
                &[],
                &[],
                false,
                Some("Your preceding response was rejected: every bullet must change.\n\n"),
            ),
        ];

        let expected = variants[0].split(MARKER).next().unwrap().to_string();
        for (index, prompt) in variants.iter().enumerate() {
            assert_eq!(
                prompt.matches(MARKER).count(),
                1,
                "variant {index} does not have exactly one zone boundary"
            );
            assert_eq!(
                prompt.split(MARKER).next().unwrap(),
                expected,
                "variant {index} changed the constant prefix, which disables prompt caching"
            );
        }

        // Roughly four characters per token. Under 1024 tokens the provider caches nothing at
        // all, so trimming the instruction text far enough would quietly switch caching off.
        assert!(
            expected.len() > 4 * 1024,
            "constant prefix is only {} chars, too short to be cached",
            expected.len()
        );
    }

    /// Across languages the base resume itself differs, so the shared prefix ends where the
    /// resume begins. That head is about 3,800 chars - roughly 950 tokens, just under the
    /// 1024-token floor - so EN and FR do not share a cache entry in practice, and padding the
    /// instructions with filler to clear the bar would buy a discount by adding the very tokens
    /// it discounts. The cached unit is therefore per language: instructions plus that
    /// language's base resume, which `tailoring_prompt_keeps_a_stable_cacheable_prefix` checks
    /// clears the floor comfortably. Keeping the instruction head identical anyway costs
    /// nothing and is what makes the per-language prefixes stable.
    #[test]
    fn tailoring_instructions_are_identical_across_languages() {
        let fr_base = json!({
            "meta": { "language": "fr", "type": "base", "template": "Xevier_T_CV_fr.template.docx" },
            "summary": "Ingénieur avec six ans d'expérience.",
            "experience": [{
                "company": "Acme",
                "location": "Remote",
                "title": "Ingénieur",
                "dates": "2024 - Present",
                "bullets": ["Construit des API.", "Amélioré la fiabilité."]
            }],
            "skills": { "frontend": "Frontend: React" }
        });
        let head = |prompt: &str| {
            prompt
                .split("Base resume JSON:")
                .next()
                .unwrap()
                .to_string()
        };
        let en = build_tailoring_prompt(
            "en",
            &json!({"title": "Rust Engineer"}),
            &analysis(),
            &base_resume(),
            &[],
            &[],
            false,
            None,
        );
        let fr = build_tailoring_prompt(
            "fr",
            &json!({"title": "Développeur"}),
            &analysis(),
            &fr_base,
            &[],
            &[],
            false,
            None,
        );

        assert_eq!(head(&en), head(&fr));
    }

    #[test]
    fn evidence_block_keeps_every_term_and_costs_less_than_json() {
        let entries = vec![
            EvidenceEntry {
                term: "Rust".to_string(),
                kind: "technology".to_string(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: false,
            },
            EvidenceEntry {
                term: "Kubernetes".to_string(),
                kind: "technology".to_string(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: true,
            },
            EvidenceEntry {
                term: "event sourcing".to_string(),
                kind: "method_domain".to_string(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: false,
            },
            EvidenceEntry {
                term: "incident response".to_string(),
                kind: "responsibility".to_string(),
                proof_note: Some("carried the on-call rotation".to_string()),
                user_attested: true,
                allow_model_role_placement: true,
            },
        ];

        let block = render_evidence_block(&entries);

        // Nothing the JSON form carried may be lost: term, kind, placement right, proof note.
        for entry in &entries {
            assert!(
                block.contains(&entry.term),
                "{} missing from {block}",
                entry.term
            );
        }
        assert!(block.contains("technology: Rust, Kubernetes"));
        assert!(block.contains("method_domain: event sourcing"));
        assert!(block.contains("responsibility: incident response"));
        assert!(block.contains(
            "Authorized for placement into an existing role's bullets: Kubernetes, incident response"
        ));
        assert!(block.contains("- incident response: carried the on-call rotation"));

        let as_json = serde_json::to_string(&entries).unwrap();
        assert!(
            block.len() < as_json.len(),
            "compact block ({} chars) should undercut the JSON form ({} chars)",
            block.len(),
            as_json.len()
        );
    }

    /// Grouping by attribute must never become a filter: a term the user has not vouched for
    /// cannot be listed alongside the ones they have.
    #[test]
    fn evidence_block_separates_unattested_and_unknown_kinds() {
        let entries = vec![
            EvidenceEntry {
                term: "Terraform".to_string(),
                kind: "technology".to_string(),
                proof_note: None,
                user_attested: false,
                allow_model_role_placement: false,
            },
            EvidenceEntry {
                term: "stakeholder alignment".to_string(),
                kind: "something_new".to_string(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: false,
            },
        ];

        let block = render_evidence_block(&entries);

        assert!(block.contains("Not user-attested - do not claim these: Terraform"));
        assert!(!block.contains("technology: Terraform"));
        assert!(block.contains("other: stakeholder alignment (something_new)"));
    }

    #[test]
    fn evidence_block_states_when_nothing_matched() {
        assert!(render_evidence_block(&[]).contains("(none matched this job)"));
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
            false,
            None,
        );

        assert!(prompt.contains("authorized you to place them in the most plausible existing role"));
        assert!(prompt.contains("completely replace the least job-relevant existing bullet"));
        assert!(prompt.contains("Angular dans l’expérience"));
        assert!(prompt.contains("do not invent adjacent details"));
    }

    #[test]
    fn selected_claims_must_appear_in_experience() {
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
    }

    /// The regression: a phrase whose words are spread over separate bullets was reported as
    /// placed, so the retry never fired and the term was quietly marked covered.
    #[test]
    fn a_claim_split_across_two_bullets_still_counts_as_missing() {
        let tailored = json!({
            "experience": [{
                "bullets": [
                    "Led the AI platform roadmap across three product teams.",
                    "Built agent tooling on top of a workflow orchestration layer."
                ]
            }]
        });
        let selected = vec!["AI agent orchestration".to_string()];

        assert_eq!(
            missing_model_placement_terms(&tailored, &selected),
            vec!["AI agent orchestration".to_string()]
        );

        let placed = json!({
            "experience": [{
                "bullets": ["Shipped AI agent orchestration for internal support tooling."]
            }]
        });
        assert!(missing_model_placement_terms(&placed, &selected).is_empty());
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
            true,
            None,
        );
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
                model_estimated_ats_coverage_score: 82,
            ats_coverage: None,
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
        assert_eq!(parsed.report.model_estimated_ats_coverage_score, 82);
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
        tailored["summary"] = json!("Rust engineer with six years building reliable APIs.");
        tailored["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");
        tailored["skills"]["architecture_backend"] =
            json!("Architecture & Backend: Rust, API Design");

        let changes = content_changes(&base, &tailored);

        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].path, "/summary");
        assert_eq!(
            changes[0].after,
            "Rust engineer with six years building reliable APIs."
        );
        assert_eq!(changes[1].path, "/experience/0/bullets/0");
        assert_eq!(changes[1].before, "Built APIs.");
        assert_eq!(changes[1].after, "Built reliable Rust APIs.");
        assert_eq!(changes[2].path, "/skills/architecture_backend");
    }

    #[test]
    fn an_untouched_summary_is_not_reported_as_a_change() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["experience"][0]["bullets"][0] = json!("Built reliable Rust APIs.");

        let changes = content_changes(&base, &tailored);

        assert!(changes.iter().all(|change| change.path != "/summary"));
    }

    /// A variant archived before the summary existed still has to diff without panicking.
    #[test]
    fn a_legacy_variant_without_a_summary_still_diffs() {
        let mut base = base_resume();
        let mut tailored = base.clone();
        base.as_object_mut().unwrap().remove("summary");
        tailored.as_object_mut().unwrap().remove("summary");
        tailored["skills"]["frontend"] = json!("Frontend: React, Redux");

        let changes = content_changes(&base, &tailored);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/skills/frontend");
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
    fn validation_rejects_an_empty_summary() {
        let base = base_resume();
        let mut tailored = base.clone();
        tailored["summary"] = json!("   ");

        let err = validate_tailored_content("en", &base, &tailored).unwrap_err();

        assert!(err.to_string().contains("summary is empty"));
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
                model_estimated_ats_coverage_score: 80,
                ats_coverage: None,
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

        let error = validate_full_bullet_rewrites(&base, &tailored).unwrap_err();
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

        let error = validate_full_bullet_rewrites(&base, &tailored).unwrap_err();
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

        normalize_bullet_rewrite_decisions(&base, &mut tailored);

        assert_eq!(
            tailored.report.bullet_rewrite_decisions[0].outcome,
            BulletRewriteOutcome::Rewritten
        );
        assert_eq!(
            tailored.report.bullet_rewrite_decisions[1].outcome,
            BulletRewriteOutcome::Rewritten
        );
        validate_full_bullet_rewrites(&base, &tailored).unwrap();
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

        let error = validate_full_bullet_rewrites(&base, &tailored).unwrap_err();
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

        validate_full_bullet_rewrites(&base, &tailored).unwrap();
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
                model_estimated_ats_coverage_score: 80,
                ats_coverage: None,
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
            0,
            base_resume(),
            vec![],
            TailoringReport {
                covered_keywords: vec![],
                omitted_unsupported_keywords: vec![],
                changed_fields: vec![],
                safety_notes: vec![],
                model_estimated_ats_coverage_score: 80,
                ats_coverage: None,
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
        assert_eq!(
            response.report.unwrap().model_estimated_ats_coverage_score,
            80
        );
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
            2,
            base_resume(),
            vec![],
            TailoringReport {
                covered_keywords: vec!["Rust".to_string()],
                omitted_unsupported_keywords: vec!["Kubernetes".to_string()],
                changed_fields: vec!["experience.bullets".to_string()],
                safety_notes: vec![],
                model_estimated_ats_coverage_score: 73,
                ats_coverage: None,
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
        assert_eq!(
            response.report.unwrap().model_estimated_ats_coverage_score,
            73
        );
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
