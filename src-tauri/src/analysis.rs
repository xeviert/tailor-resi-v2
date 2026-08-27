use crate::api_usage::{record_response_usage, UsageContext};
use crate::http::{retry_delay, shared_client, status_is_retryable};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct AnalysisConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl AnalysisConfig {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").ok()?;
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return None;
        }

        Some(Self {
            api_key,
            model: std::env::var("OPENAI_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "gpt-5.6-luna".to_string()),
            base_url: std::env::var("OPENAI_BASE_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct KeywordSignal {
    pub term: String,
    pub category: String,
    pub importance: u8,
    pub evidence: String,
}

/// Alternate written forms of a term that an ATS would treat as different strings.
///
/// Keyword matching is literal, so "Kubernetes" and "K8s", or "CI/CD" and "continuous
/// integration", score as separate things even though they name one capability. Collecting the
/// variants lets coverage be measured against whichever form the resume happens to use, and
/// lets tailoring prefer the form the job post itself wrote.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TermVariants {
    pub term: String,
    pub variants: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobAnalysis {
    pub role_target: String,
    pub seniority: String,
    pub core_keywords: Vec<KeywordSignal>,
    pub required_skills: Vec<String>,
    pub preferred_skills: Vec<String>,
    pub tools_and_platforms: Vec<String>,
    pub domain_terms: Vec<String>,
    pub responsibility_phrases: Vec<String>,
    pub achievement_angles: Vec<String>,
    pub ats_phrase_bank: Vec<String>,
    pub must_not_claim_without_evidence: Vec<String>,
    /// Empty for an analysis stored before variants existed.
    #[serde(default)]
    pub term_variants: Vec<TermVariants>,
    pub summary: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
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
    #[error("OpenAI analysis JSON was invalid: {0}")]
    InvalidJson(String),
    #[error("OpenAI stopped early ({0}); the analysis is incomplete")]
    IncompleteResponse(String),
    #[error("OpenAI declined to analyze this job post: {0}")]
    Refused(String),
}

pub fn build_analysis_prompt(parsed_job: &serde_json::Value) -> String {
    let compact_job = serde_json::to_string(&crate::server::prompt_job_view(parsed_job))
        .unwrap_or_else(|_| "{}".to_string());
    format!(
        "Analyze this normalized job post for ATS resume-tailoring signals.\n\
         Return only the schema fields. Ground every keyword and phrase in the job post.\n\
         Do not invent credentials, experience, metrics, responsibilities, or tools not supported by the post.\n\
         Return short, atomic capability terms, normally one to six words.\n\
         Prefer exact technology, framework, certification, and domain wording when useful.\n\
         Semantically deduplicate terms across every array; choose one clear ATS-friendly label per capability.\n\
         Do not put job titles, company names, generic personality traits, or full requirement sentences in capability arrays.\n\
         This applies to responsibility_phrases too: write the capability, not the sentence. Prefer a bare capability like pair programming over a sentence like intervenir en pair programming, and revue de code over realiser des revues approfondies. Drop leading verbs, articles, and adverbs; keep the noun phrase a resume would actually contain.\n\
         Classify tools and frameworks as technology, working methods and business domains as method_domain, and claims about actions performed as responsibility.\n\
         Weight terms by how the post asks for them: a term in a requirements or must-have section, or repeated across the post, outranks one mentioned once in company boilerplate, benefits, or an equal-opportunity notice.\n\
         Be thorough rather than selective. Extract every distinct capability the post asks for, up to the schema limits; a term you leave out cannot be matched later.\n\
         In term_variants, list the alternate written forms of any extracted term that a literal keyword matcher would treat as a different string: acronym and expansion (Kubernetes / K8s, continuous integration / CI/CD), common spellings (PostgreSQL / Postgres), and the job post's own wording when it differs from your chosen label. Only list forms that mean the same capability; do not list related or broader technologies.\n\
         Focus on analysis for a later resume-writing layer; do not rewrite resume bullets.\n\
         Write every extracted field (role_target, seniority, core_keywords terms and evidence, required_skills, preferred_skills, tools_and_platforms, domain_terms, responsibility_phrases, achievement_angles, ats_phrase_bank, must_not_claim_without_evidence, term_variants, and summary) in the same language as the job post itself. Do not translate the job post's language into English or any other language.\n\n\
         Normalized job JSON:\n{compact_job}"
    )
}

fn analysis_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "role_target",
            "seniority",
            "core_keywords",
            "required_skills",
            "preferred_skills",
            "tools_and_platforms",
            "domain_terms",
            "responsibility_phrases",
            "achievement_angles",
            "ats_phrase_bank",
            "must_not_claim_without_evidence",
            "term_variants",
            "summary"
        ],
        "properties": {
            "role_target": { "type": "string" },
            "seniority": { "type": "string" },
            "core_keywords": {
                "type": "array",
                "maxItems": 20,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["term", "category", "importance", "evidence"],
                    "properties": {
                        "term": { "type": "string" },
                        "category": {
                            "type": "string",
                            "enum": ["technology", "method_domain", "responsibility"]
                        },
                        "importance": { "type": "integer", "minimum": 1, "maximum": 5 },
                        "evidence": { "type": "string" }
                    }
                }
            },
            "required_skills": { "type": "array", "maxItems": 16, "items": { "type": "string" } },
            "preferred_skills": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "tools_and_platforms": { "type": "array", "maxItems": 18, "items": { "type": "string" } },
            "domain_terms": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "responsibility_phrases": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "achievement_angles": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "ats_phrase_bank": { "type": "array", "maxItems": 15, "items": { "type": "string" } },
            "must_not_claim_without_evidence": { "type": "array", "maxItems": 12, "items": { "type": "string" } },
            "term_variants": {
                "type": "array",
                "maxItems": 24,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["term", "variants"],
                    "properties": {
                        "term": { "type": "string" },
                        "variants": { "type": "array", "maxItems": 4, "items": { "type": "string" } }
                    }
                }
            },
            "summary": { "type": "string" }
        }
    })
}

fn build_openai_request(model: &str, parsed_job: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "prompt_cache_key": crate::http::PROMPT_CACHE_KEY_JOB_ANALYSIS,
        "input": [
            {
                "role": "system",
                "content": "You extract ATS-relevant resume-tailoring signals from job posts. You only use evidence present in the job post. You always write extracted terms and text in the same language as the job post itself, never translating them."
            },
            {
                "role": "user",
                "content": build_analysis_prompt(parsed_job)
            }
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "job_analysis",
                "strict": true,
                "schema": analysis_schema()
            }
        }
    })
}

/// Analysis is a single API call with no correction loop behind it, so a transient rate limit
/// or a 503 would otherwise fail the whole run and force the user to re-analyze by hand.
const MAX_ANALYSIS_ATTEMPTS: u32 = 3;

pub async fn analyze_job(
    config: &AnalysisConfig,
    parsed_job: &serde_json::Value,
) -> Result<JobAnalysis, AnalysisError> {
    let request_body = build_openai_request(&config.model, parsed_job);
    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));
    let mut last_error = None;

    for attempt in 0..MAX_ANALYSIS_ATTEMPTS {
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
                last_error = Some(AnalysisError::Request(error.to_string()));
                continue;
            }
        };

        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(AnalysisError::Request(error.to_string()));
                continue;
            }
        };

        if !status.is_success() {
            let error = AnalysisError::Http { status, body };
            if status_is_retryable(status) {
                last_error = Some(error);
                continue;
            }
            return Err(error);
        }

        // Before the parse: a refused or truncated response is billed too, and those are
        // exactly the calls a cost investigation needs to see.
        record_response_usage(
            "job_analysis",
            &config.model,
            &body,
            UsageContext::default(),
        );
        let analysis = parse_job_analysis_from_response(&body)?;
        return Ok(analysis);
    }

    Err(last_error.unwrap_or_else(|| AnalysisError::Request("no attempt was made".to_string())))
}

/// The structured-output JSON text carried by a Responses API body, or the specific reason
/// there is none.
///
/// Every structured call this app makes shares the same envelope, so it shares this reader
/// too: a refusal, a truncated response and a malformed schema are three different problems
/// and each one deserves its own message, whichever stage hit it.
pub(crate) fn structured_output_text(body: &str) -> Result<String, AnalysisError> {
    if body.trim().is_empty() {
        return Err(AnalysisError::EmptyResponseBody);
    }
    let response: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| AnalysisError::InvalidJson(error.to_string()))?;

    if let Some(refusal) = find_refusal(&response) {
        return Err(AnalysisError::Refused(refusal.to_string()));
    }
    let text = match find_output_text(&response) {
        Some(text) => text,
        None => {
            // A response truncated by the token limit has no `output_text` at all. Reporting
            // that as invalid JSON sends the reader looking for a schema bug that isn't there.
            return Err(match incomplete_reason(&response) {
                Some(reason) => AnalysisError::IncompleteResponse(reason.to_string()),
                None => AnalysisError::MissingOutputText,
            });
        }
    };
    if text.trim().is_empty() {
        return Err(AnalysisError::EmptyOutputText);
    }
    if let Some(reason) = incomplete_reason(&response) {
        return Err(AnalysisError::IncompleteResponse(reason.to_string()));
    }
    Ok(text.to_string())
}

pub fn parse_job_analysis_from_response(body: &str) -> Result<JobAnalysis, AnalysisError> {
    let text = structured_output_text(body)?;
    serde_json::from_str(&text).map_err(|error| AnalysisError::InvalidJson(error.to_string()))
}

/// `Some(reason)` when the model stopped before finishing, e.g. `"max_output_tokens"`.
pub(crate) fn incomplete_reason(response: &serde_json::Value) -> Option<&str> {
    if response["status"].as_str() != Some("incomplete") {
        return None;
    }
    Some(
        response["incomplete_details"]["reason"]
            .as_str()
            .unwrap_or("reason unknown"),
    )
}

pub(crate) fn find_refusal(response: &serde_json::Value) -> Option<&str> {
    response["output"].as_array()?.iter().find_map(|item| {
        item["content"].as_array()?.iter().find_map(|content| {
            if content["type"].as_str() == Some("refusal") {
                content["refusal"].as_str()
            } else {
                None
            }
        })
    })
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

#[cfg(test)]
mod tests {
    use super::{build_analysis_prompt, parse_job_analysis_from_response};
    use serde_json::json;

    #[test]
    fn prompt_contains_normalized_job_context() {
        let prompt = build_analysis_prompt(&json!({
            "domain": "indeed",
            "title": "Rust Engineer",
            "description": "Build APIs with Rust and Axum."
        }));

        assert!(prompt.contains("Rust Engineer"));
        assert!(prompt.contains("Axum"));
        assert!(prompt.contains("Do not invent"));
        assert!(prompt.contains("Semantically deduplicate"));
        assert!(prompt.contains("one to six words"));
        assert!(prompt.contains("same language as the job post"));
    }

    /// Records why this stage gets no prefix-cache discount, so nobody re-derives it.
    ///
    /// The ordering is right - instructions first, job post last - but the constant part is too
    /// small to qualify. The system message and this instruction block come to roughly 2,600
    /// chars, about 650 tokens, and the provider caches nothing below 1024. The output schema is
    /// constant and would close the gap, but it travels in `text.format.schema`, which serializes
    /// *after* the messages, so it sits below the volatile job post and cannot extend the prefix.
    ///
    /// Padding the instructions to clear the bar would buy a 90% discount by adding the very
    /// tokens being discounted, which is a loss. The real saving for this stage is not calling it
    /// twice for the same capture at all - see the snapshot reuse in `analyze_latest_job`.
    ///
    /// This test fails if the prompt ever grows past the floor, at which point caching becomes
    /// reachable and the comment above is stale.
    #[test]
    fn analysis_prompt_static_prefix_is_below_the_cache_floor() {
        const MARKER: &str = "\nNormalized job JSON:";
        let prompt = build_analysis_prompt(&json!({"title": "Rust Engineer"}));
        let static_prefix = prompt.split(MARKER).next().unwrap();

        // Roughly four characters per token against the provider's 1024-token floor.
        assert!(
            static_prefix.len() < 4 * 1024,
            "static analysis prefix reached {} chars: it now clears the cache floor, so revisit \
             the note above and the reuse path that stands in for caching here",
            static_prefix.len()
        );
    }

    /// The job post must stay strictly last. Anything volatile moved above the instructions
    /// truncates the shared prefix to nothing.
    #[test]
    fn analysis_prompt_puts_the_job_post_last() {
        let prompt = build_analysis_prompt(&json!({"title": "Rust Engineer"}));
        let job_start = prompt.find("Rust Engineer").unwrap();
        let instructions_end = prompt.find("Normalized job JSON:").unwrap();

        assert!(
            job_start > instructions_end,
            "job content appeared above the instruction block"
        );
    }

    #[test]
    fn parses_responses_api_output_text() {
        let body = json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": serde_json::to_string(&json!({
                        "role_target": "Rust Engineer",
                        "seniority": "Mid-level",
                        "core_keywords": [{
                            "term": "Rust",
                            "category": "technology",
                            "importance": 5,
                            "evidence": "Job title and description mention Rust"
                        }],
                        "required_skills": ["Rust"],
                        "preferred_skills": ["Axum"],
                        "tools_and_platforms": ["Axum"],
                        "domain_terms": ["API development"],
                        "responsibility_phrases": ["Build APIs"],
                        "achievement_angles": ["Reliable API delivery"],
                        "ats_phrase_bank": ["Rust API development"],
                        "must_not_claim_without_evidence": ["Kubernetes"],
                        "summary": "Emphasize Rust API work."
                    })).unwrap()
                }]
            }]
        })
        .to_string();

        let analysis = parse_job_analysis_from_response(&body).unwrap();
        assert_eq!(analysis.role_target, "Rust Engineer");
        assert_eq!(analysis.core_keywords[0].importance, 5);
        assert_eq!(analysis.must_not_claim_without_evidence[0], "Kubernetes");
    }

    #[test]
    fn rejects_missing_output_text() {
        let err = parse_job_analysis_from_response(r#"{"output":[]}"#).unwrap_err();
        assert!(err.to_string().contains("structured output text"));
    }

    #[test]
    fn rejects_an_empty_responses_api_body() {
        let err = parse_job_analysis_from_response("  \n").unwrap_err();
        assert!(err.to_string().contains("empty response body"));
    }

    #[test]
    fn reports_a_truncated_response_as_incomplete_not_as_bad_json() {
        let body = json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": []
        })
        .to_string();

        let error = parse_job_analysis_from_response(&body).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("incomplete"), "{message}");
        assert!(message.contains("max_output_tokens"), "{message}");
        assert!(!message.contains("invalid"), "{message}");
    }

    #[test]
    fn reports_a_refusal_distinctly() {
        let body = json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "refusal", "refusal": "I cannot help with that." }]
            }]
        })
        .to_string();

        let error = parse_job_analysis_from_response(&body).unwrap_err();
        assert!(error.to_string().contains("declined"), "{error}");
    }

    #[test]
    fn analysis_prompt_omits_the_duplicate_html_description() {
        let prompt = build_analysis_prompt(&json!({
            "title": "Rust Engineer",
            "description": "Build APIs with Rust and Axum.",
            "description_html": "<p>Build APIs with Rust and Axum.</p>"
        }));

        assert!(prompt.contains("Build APIs with Rust and Axum."));
        assert!(!prompt.contains("description_html"));
        assert!(!prompt.contains("<p>"));
    }
}
