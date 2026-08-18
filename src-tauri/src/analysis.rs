use crate::api_usage::record_response_usage;
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
}

pub fn build_analysis_prompt(parsed_job: &serde_json::Value) -> String {
    let compact_job = serde_json::to_string(parsed_job).unwrap_or_else(|_| "{}".to_string());
    format!(
        "Analyze this normalized job post for ATS resume-tailoring signals.\n\
         Return only the schema fields. Ground every keyword and phrase in the job post.\n\
         Do not invent credentials, experience, metrics, responsibilities, or tools not supported by the post.\n\
         Return short, atomic capability terms, normally one to six words.\n\
         Prefer exact technology, framework, certification, and domain wording when useful.\n\
         Semantically deduplicate terms across every array; choose one clear ATS-friendly label per capability.\n\
         Do not put job titles, company names, generic personality traits, or full requirement sentences in capability arrays.\n\
         Classify tools and frameworks as technology, working methods and business domains as method_domain, and claims about actions performed as responsibility.\n\
         Focus on analysis for a later resume-writing layer; do not rewrite resume bullets.\n\
         Write every extracted field (role_target, seniority, core_keywords terms and evidence, required_skills, preferred_skills, tools_and_platforms, domain_terms, responsibility_phrases, achievement_angles, ats_phrase_bank, must_not_claim_without_evidence, and summary) in the same language as the job post itself. Do not translate the job post's language into English or any other language.\n\n\
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
            "summary"
        ],
        "properties": {
            "role_target": { "type": "string" },
            "seniority": { "type": "string" },
            "core_keywords": {
                "type": "array",
                "maxItems": 12,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["term", "category", "importance", "evidence"],
                    "properties": {
                        "term": { "type": "string" },
                        "category": { "type": "string" },
                        "importance": { "type": "integer", "minimum": 1, "maximum": 5 },
                        "evidence": { "type": "string" }
                    }
                }
            },
            "required_skills": { "type": "array", "maxItems": 10, "items": { "type": "string" } },
            "preferred_skills": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "tools_and_platforms": { "type": "array", "maxItems": 12, "items": { "type": "string" } },
            "domain_terms": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "responsibility_phrases": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "achievement_angles": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "ats_phrase_bank": { "type": "array", "maxItems": 15, "items": { "type": "string" } },
            "must_not_claim_without_evidence": { "type": "array", "maxItems": 12, "items": { "type": "string" } },
            "summary": { "type": "string" }
        }
    })
}

fn build_openai_request(model: &str, parsed_job: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "model": model,
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

pub async fn analyze_job(
    config: &AnalysisConfig,
    parsed_job: &serde_json::Value,
) -> Result<JobAnalysis, AnalysisError> {
    let client = reqwest::Client::new();
    let request_body = build_openai_request(&config.model, parsed_job);
    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));

    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .json(&request_body)
        .send()
        .await
        .map_err(|error| AnalysisError::Request(error.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AnalysisError::Request(error.to_string()))?;

    if !status.is_success() {
        return Err(AnalysisError::Http { status, body });
    }

    let analysis = parse_job_analysis_from_response(&body)?;
    record_response_usage("job_analysis", &config.model, &body);
    Ok(analysis)
}

pub fn parse_job_analysis_from_response(body: &str) -> Result<JobAnalysis, AnalysisError> {
    if body.trim().is_empty() {
        return Err(AnalysisError::EmptyResponseBody);
    }
    let response: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| AnalysisError::InvalidJson(error.to_string()))?;

    let text = find_output_text(&response).ok_or(AnalysisError::MissingOutputText)?;
    if text.trim().is_empty() {
        return Err(AnalysisError::EmptyOutputText);
    }
    serde_json::from_str(text).map_err(|error| AnalysisError::InvalidJson(error.to_string()))
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
}
