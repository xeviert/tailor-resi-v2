//! Importing a job post the app fetched or the user pasted, rather than one the browser
//! extension scraped.
//!
//! The extension is best-effort: it has no minimum-score gate, so a page whose only readable
//! job signal is an `og:description` meta tag still reaches disk, and a one-sentence
//! description is what the analysis prompt then has to work with. This module is the way out
//! of that. It produces the same capture payload `POST /captures` produces, so nothing
//! downstream of `persist_capture` knows or cares which route a job arrived by.
//!
//! A fetched page is tried against schema.org JSON-LD first. A board that publishes a
//! `JobPosting` there hands over the whole posting exactly, instantly and for no tokens, so it
//! is always worth looking. Plenty of boards do not - several of the big ATS hosts render
//! everything client-side and ship no structured data at all - which is why the AI layer is
//! there. It is the fallback, not the default.

use crate::analysis::{structured_output_text, AnalysisConfig, AnalysisError};
use crate::api_usage::{record_response_usage, UsageContext};
use crate::http::{retry_delay, shared_client, status_is_retryable};
use crate::server::html_to_block_text;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A page fetch is not a model call. `shared_client`'s 300s budget exists so a reasoning model
/// grinding through a whole resume rewrite is never cut off; applying it here would turn a dead
/// job link into a five-minute spinner. The override is per request, so the shared connection
/// pool is still the one doing the work.
const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Enough for any real posting, and a hard stop on a page that streams forever.
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const REFUSE_CONTENT_LENGTH_ABOVE: u64 = 16 * 1024 * 1024;

/// Roughly 8k tokens of stripped page text. A posting body runs 3-6k characters; the rest is
/// headroom for the navigation, cookie banners and "similar jobs" lists that survive stripping.
const MAX_PROMPT_CHARS: usize = 32_000;

/// Below this, a fetched page is a JavaScript shell rather than a document. Checking before the
/// model call means a hopeless page costs nothing.
const MIN_PAGE_CHARS: usize = 400;
const MIN_PASTED_CHARS: usize = 200;

const MAX_EXTRACTION_ATTEMPTS: u32 = 3;

/// Job boards routinely refuse anything that does not look like a browser. This does not defeat
/// a real bot check - when it fails, the paste fallback is the answer - but it does get past the
/// naive User-Agent filters that a good number of boards stop at.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImportMode {
    Url,
    Text,
}

impl ImportMode {
    fn source(self) -> &'static str {
        match self {
            ImportMode::Url => "url_import",
            ImportMode::Text => "text_import",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Extraction {
    JsonLd,
    Llm,
}

impl Extraction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Extraction::JsonLd => "json_ld",
            Extraction::Llm => "llm",
        }
    }
}

/// A capture payload ready for `server::persist_capture`, plus how it was built.
pub struct ImportedJob {
    pub payload: serde_json::Value,
    pub extraction: Extraction,
}

/// The model's view of a posting.
///
/// Every field name here is one an existing board parser already emits, so `prompt_job_view`
/// hands the analysis prompt the same keys whichever route the job arrived by, and `JobPanel`'s
/// generic renderer already knows how to draw all of them.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtractedJob {
    pub is_job_posting: bool,
    pub title: String,
    pub company: String,
    pub description: String,
    pub location: Option<String>,
    pub locations: Vec<String>,
    pub job_type: Option<String>,
    pub remote: Option<bool>,
    pub compensation: Option<String>,
    pub qualifications: Option<String>,
    pub skills: Vec<String>,
    pub years_experience_min: Option<u32>,
    pub years_experience_max: Option<u32>,
    pub date_posted: Option<String>,
    pub education_requirement: Option<String>,
    pub company_description: Option<String>,
    pub company_hq: Option<String>,
    pub industry_tags: Vec<String>,
    pub extraction_confidence: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("That does not look like a job post URL. Paste the full link, starting with https://.")]
    InvalidUrl,
    #[error("Could not reach {host}: {detail}. Check the link and your connection.")]
    Request { host: String, detail: String },
    #[error("The site refused the request (HTTP {status}). Many job boards block anything that is not a real browser - open the post in your browser, copy the text, and use the Paste text tab.")]
    Blocked { status: u16 },
    #[error("That job post is gone (HTTP {status}). The listing may have been taken down.")]
    NotFound { status: u16 },
    #[error("The site returned HTTP {status}.")]
    HttpStatus { status: u16 },
    #[error("That link returns {content_type}, not a web page. If it is a PDF, open it and paste the text instead.")]
    NotHtml { content_type: String },
    #[error("Only {chars} characters of text came back - the page probably builds its content with JavaScript. Open it in your browser, copy the post, and use the Paste text tab.")]
    PageTooThin { chars: usize },
    #[error("That is only {chars} characters. Paste the whole job post, including the requirements section.")]
    TextTooShort { chars: usize },
    #[error("OPENAI_API_KEY is required to import a job post with AI.")]
    MissingApiKey,
    #[error("The AI could not extract this posting: {0}")]
    Extraction(#[from] AnalysisError),
    #[error("That page does not look like a job posting - it may be a search page or a login wall. Open the posting itself, or paste its text.")]
    NotAJobPosting,
    #[error("The AI came back with no title and no description. Try the Paste text tab with the posting text.")]
    EmptyResult,
}

/// Fetch `url` and recover the posting from it.
pub async fn import_from_url(url: &str) -> Result<ImportedJob, ImportError> {
    let normalized = normalize_url(url)?;
    let page = fetch_page(&normalized).await?;

    if let Some(posting) = job_posting_from_json_ld(&page.html) {
        if json_ld_is_usable(&posting) {
            return Ok(ImportedJob {
                payload: build_payload(
                    ImportMode::Url,
                    Extraction::JsonLd,
                    &page.final_url,
                    Some(&page.page_title),
                    "",
                    posting,
                ),
                extraction: Extraction::JsonLd,
            });
        }
    }

    let body = html_to_block_text(&page.html);
    let length = body.chars().count();
    if length < MIN_PAGE_CHARS {
        return Err(ImportError::PageTooThin { chars: length });
    }

    let config = AnalysisConfig::from_env().ok_or(ImportError::MissingApiKey)?;
    let body = truncate_for_prompt(&body);
    let job = extract_with_llm(&config, Some(&page.final_url), &body).await?;
    Ok(ImportedJob {
        payload: build_payload(
            ImportMode::Url,
            Extraction::Llm,
            &page.final_url,
            Some(&page.page_title),
            &body,
            serde_json::to_value(&job).unwrap_or_default(),
        ),
        extraction: Extraction::Llm,
    })
}

/// Recover a posting from text the user pasted. Always a model call - there is no markup left
/// to read structured data out of.
pub async fn import_from_text(
    text: &str,
    source_url: Option<&str>,
) -> Result<ImportedJob, ImportError> {
    let trimmed = text.trim();
    let length = trimmed.chars().count();
    if length < MIN_PASTED_CHARS {
        return Err(ImportError::TextTooShort { chars: length });
    }

    // Copying a rendered posting out of devtools yields markup, not prose. One `contains`
    // check saves the user from pasting a wall of `<div class="...">` at the model.
    let body = if trimmed.contains("</") {
        html_to_block_text(trimmed)
    } else {
        trimmed.to_string()
    };

    // A source URL is a convenience here, not the input. A malformed one should cost the user
    // the "View source" link, not the whole import.
    let source_url = source_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| normalize_url(value).ok())
        .unwrap_or_default();

    let config = AnalysisConfig::from_env().ok_or(ImportError::MissingApiKey)?;
    let body = truncate_for_prompt(&body);
    let prompt_url = Some(source_url.as_str()).filter(|url| !url.is_empty());
    let job = extract_with_llm(&config, prompt_url, &body).await?;
    Ok(ImportedJob {
        payload: build_payload(
            ImportMode::Text,
            Extraction::Llm,
            &source_url,
            None,
            &body,
            serde_json::to_value(&job).unwrap_or_default(),
        ),
        extraction: Extraction::Llm,
    })
}

/// Accept what a user actually pastes, reject what must never be fetched.
pub(crate) fn normalize_url(raw: &str) -> Result<String, ImportError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ImportError::InvalidUrl);
    }
    // People paste `wellfound.com/jobs/123` as often as the full link.
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&candidate).map_err(|_| ImportError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ImportError::InvalidUrl);
    }
    // `https://` alone parses, and a host with no dot is a typo far more often than an intranet
    // hostname a job post would live on.
    match url.host_str() {
        Some(host) if host.contains('.') => Ok(url.to_string()),
        _ => Err(ImportError::InvalidUrl),
    }
}

pub(crate) struct PageFetch {
    pub final_url: String,
    pub page_title: String,
    pub html: String,
}

async fn fetch_page(url: &str) -> Result<PageFetch, ImportError> {
    let host = Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(String::from))
        .unwrap_or_else(|| "the site".to_string());
    let request_failed = |error: reqwest::Error| ImportError::Request {
        host: host.clone(),
        detail: error.to_string(),
    };

    let response = shared_client()
        .get(url)
        .timeout(PAGE_FETCH_TIMEOUT)
        .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9,fr;q=0.8")
        // Deliberately no Accept-Encoding: reqwest is built without the gzip and brotli
        // features, so advertising them would hand us bytes we cannot decode.
        .send()
        .await
        .map_err(request_failed)?;

    let status = response.status();
    if !status.is_success() {
        return Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
                ImportError::Blocked {
                    status: status.as_u16(),
                }
            }
            StatusCode::NOT_FOUND | StatusCode::GONE => ImportError::NotFound {
                status: status.as_u16(),
            },
            _ => ImportError::HttpStatus {
                status: status.as_u16(),
            },
        });
    }

    let media_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !media_type.is_empty()
        && !matches!(
            media_type.as_str(),
            "text/html" | "application/xhtml+xml" | "text/plain" | "application/xml" | "text/xml"
        )
    {
        return Err(ImportError::NotHtml {
            content_type: media_type,
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > REFUSE_CONTENT_LENGTH_ABOVE)
    {
        return Err(ImportError::NotHtml {
            content_type: "a document too large to read".to_string(),
        });
    }

    let final_url = response.url().to_string();
    let mut html = response.text().await.map_err(request_failed)?;
    if html.len() > MAX_HTML_BYTES {
        // Truncating mid-tag is harmless: the scanner treats an unterminated tag as the end.
        let cut = (0..=MAX_HTML_BYTES)
            .rev()
            .find(|index| html.is_char_boundary(*index))
            .unwrap_or(0);
        html.truncate(cut);
    }

    Ok(PageFetch {
        page_title: page_title(&html),
        final_url,
        html,
    })
}

/// The body of every `<script type="application/ld+json">` block on the page.
pub(crate) fn json_ld_blocks(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find("<script") {
        let open = cursor + offset;
        let Some(tag_end) = lower[open..].find('>').map(|end| open + end + 1) else {
            break;
        };
        let is_json_ld = lower[open..tag_end].contains("application/ld+json");
        let Some(close) = lower[tag_end..].find("</script").map(|end| tag_end + end) else {
            break;
        };
        if is_json_ld {
            blocks.push(&html[tag_end..close]);
        }
        cursor = close + 1;
    }
    blocks
}

/// The first schema.org `JobPosting` on the page, wherever it is nested.
///
/// Sites wrap these three different ways - bare object, top-level array, or `@graph` - and a
/// block that fails to parse is skipped rather than fatal, because pages routinely carry one
/// broken block alongside good ones.
pub(crate) fn job_posting_from_json_ld(html: &str) -> Option<serde_json::Value> {
    fn search(node: &serde_json::Value) -> Option<serde_json::Value> {
        match node {
            serde_json::Value::Array(items) => items.iter().find_map(search),
            serde_json::Value::Object(fields) => {
                let is_posting = match fields.get("@type") {
                    Some(serde_json::Value::String(name)) => name == "JobPosting",
                    Some(serde_json::Value::Array(names)) => {
                        names.iter().any(|name| name.as_str() == Some("JobPosting"))
                    }
                    _ => false,
                };
                if is_posting {
                    return Some(node.clone());
                }
                fields.get("@graph").and_then(search)
            }
            _ => None,
        }
    }

    json_ld_blocks(html)
        .into_iter()
        .filter_map(|block| serde_json::from_str::<serde_json::Value>(block).ok())
        .find_map(|node| search(&node))
}

/// Whether a recovered `JobPosting` is worth using.
///
/// Some sites emit a stub with nothing but `@type` and a URL. Taking it would produce an empty
/// capture and skip the model call that could have salvaged the page.
pub(crate) fn json_ld_is_usable(posting: &serde_json::Value) -> bool {
    let filled = |key: &str| {
        posting[key]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    };
    filled("title") && filled("description")
}

pub(crate) fn page_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(open) = lower.find("<title") else {
        return String::new();
    };
    let Some(start) = lower[open..].find('>').map(|end| open + end + 1) else {
        return String::new();
    };
    let end = lower[start..]
        .find("</title")
        .map_or(html.len(), |offset| start + offset);
    html[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cut page text down to what is worth paying for.
///
/// From the head, deliberately: a posting sits in the top half of a document, while footers,
/// legal boilerplate and "similar jobs" lists pile up at the bottom.
pub(crate) fn truncate_for_prompt(text: &str) -> String {
    if text.chars().count() <= MAX_PROMPT_CHARS {
        return text.to_string();
    }
    // By char boundary, not byte index. An accented French posting would otherwise panic here.
    let end = text
        .char_indices()
        .nth(MAX_PROMPT_CHARS)
        .map_or(text.len(), |(index, _)| index);
    let cut = &text[..end];
    let cut = cut
        .rfind(char::is_whitespace)
        .map_or(cut, |edge| &cut[..edge]);
    format!("{cut}\n[... page text truncated ...]")
}

pub(crate) fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "is_job_posting", "title", "company", "description", "location", "locations",
            "job_type", "remote", "compensation", "qualifications", "skills",
            "years_experience_min", "years_experience_max", "date_posted",
            "education_requirement", "company_description", "company_hq", "industry_tags",
            "extraction_confidence"
        ],
        "properties": {
            "is_job_posting": { "type": "boolean" },
            "title": { "type": "string" },
            "company": { "type": "string" },
            "description": { "type": "string" },
            "location": { "type": ["string", "null"] },
            "locations": { "type": "array", "maxItems": 6, "items": { "type": "string" } },
            "job_type": { "type": ["string", "null"] },
            "remote": { "type": ["boolean", "null"] },
            "compensation": { "type": ["string", "null"] },
            "qualifications": { "type": ["string", "null"] },
            "skills": { "type": "array", "maxItems": 20, "items": { "type": "string" } },
            "years_experience_min": { "type": ["integer", "null"] },
            "years_experience_max": { "type": ["integer", "null"] },
            "date_posted": { "type": ["string", "null"] },
            "education_requirement": { "type": ["string", "null"] },
            "company_description": { "type": ["string", "null"] },
            "company_hq": { "type": ["string", "null"] },
            "industry_tags": { "type": "array", "maxItems": 6, "items": { "type": "string" } },
            "extraction_confidence": { "type": "string", "enum": ["high", "medium", "low"] }
        }
    })
}

pub(crate) fn build_extraction_prompt(source_url: Option<&str>, body: &str) -> String {
    let source_line = match source_url {
        Some(url) if !url.is_empty() => format!("Source URL: {url}\n"),
        _ => String::new(),
    };
    format!(
        "Recover the job posting from the text below, which was taken from a web page.\n\
         Copy the posting's own wording into description. Do not summarize, shorten, paraphrase, or translate it.\n\
         Return description as plain text: no HTML tags and no Markdown. Separate sections and list items with newlines, one line per list item.\n\
         Include the responsibilities and the requirements in full. Those carry the ATS signal, and anything you leave out cannot be matched later.\n\
         Leave out site navigation, cookie notices, boilerplate that is not part of the posting, similar-jobs lists, and application forms.\n\
         Write every field in the same language as the posting itself. Do not translate it into English or any other language.\n\
         Do not invent a title, company, location, salary, or requirement the text does not state. Return null for anything absent.\n\
         For job_type use exactly one of: Full-time, Part-time, Contract, Internship, Temporary, Unknown.\n\
         Set extraction_confidence to low when the text is fragmentary or you had to guess where the posting begins and ends.\n\
         If this text is not a single job posting - a search results page, a login wall, an error page, a company home page - set is_job_posting to false and leave the other fields empty.\n\n\
         {source_line}Page text:\n{body}"
    )
}

fn build_extraction_request(
    model: &str,
    source_url: Option<&str>,
    body: &str,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "prompt_cache_key": crate::http::PROMPT_CACHE_KEY_JOB_IMPORT,
        "input": [
            {
                "role": "system",
                "content": "You recover a single job posting from web page text. You only use text present in the input, never inventing details, and you always keep the posting's original language."
            },
            {
                "role": "user",
                "content": build_extraction_prompt(source_url, body)
            }
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "job_extraction",
                "strict": true,
                "schema": extraction_schema()
            }
        }
    })
}

pub(crate) fn parse_extraction_from_response(body: &str) -> Result<ExtractedJob, ImportError> {
    let text = structured_output_text(body)?;
    let job: ExtractedJob = serde_json::from_str(&text)
        .map_err(|error| ImportError::Extraction(AnalysisError::InvalidJson(error.to_string())))?;
    if !job.is_job_posting {
        return Err(ImportError::NotAJobPosting);
    }
    if job.title.trim().is_empty() && job.description.trim().is_empty() {
        return Err(ImportError::EmptyResult);
    }
    Ok(job)
}

async fn extract_with_llm(
    config: &AnalysisConfig,
    source_url: Option<&str>,
    body: &str,
) -> Result<ExtractedJob, ImportError> {
    let request_body = build_extraction_request(&config.model, source_url, body);
    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));
    let mut last_error = None;

    for attempt in 0..MAX_EXTRACTION_ATTEMPTS {
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
        let response_body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(AnalysisError::Request(error.to_string()));
                continue;
            }
        };

        if !status.is_success() {
            let error = AnalysisError::Http {
                status,
                body: response_body,
            };
            if status_is_retryable(status) {
                last_error = Some(error);
                continue;
            }
            return Err(error.into());
        }

        // Before the parse; see the note in `analysis::analyze_job`.
        record_response_usage(
            "job_import",
            &config.model,
            &response_body,
            UsageContext::default(),
        );
        let job = parse_extraction_from_response(&response_body)?;
        return Ok(job);
    }

    Err(ImportError::Extraction(last_error.unwrap_or_else(|| {
        AnalysisError::Request("no attempt was made".to_string())
    })))
}

/// Assemble the capture payload. The only place the routing discriminators are written.
pub(crate) fn build_payload(
    mode: ImportMode,
    extraction: Extraction,
    source_url: &str,
    page_title: Option<&str>,
    source_text: &str,
    job: serde_json::Value,
) -> serde_json::Value {
    let imported_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    serde_json::json!({
        "source": mode.source(),
        "extraction": extraction.label(),
        "sourceUrl": source_url,
        "pageTitle": page_title.unwrap_or(""),
        "importedAtMs": imported_at_ms,
        // The only handle anyone gets when an extraction comes out wrong. Safe to keep:
        // `prompt_job_view` reads `parsed`, never `payload`, and the one path that re-embeds a
        // payload into a prompt is the unknown-domain fallback, which the `source`
        // discriminator routes around.
        "sourceText": source_text,
        "json": job,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responses_body(payload: serde_json::Value) -> String {
        serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": serde_json::to_string(&payload).unwrap()
                }]
            }]
        })
        .to_string()
    }

    fn extracted(overrides: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "is_job_posting": true,
            "title": "Rust Engineer",
            "company": "Acme",
            "description": "Build services.",
            "location": null,
            "locations": [],
            "job_type": null,
            "remote": null,
            "compensation": null,
            "qualifications": null,
            "skills": [],
            "years_experience_min": null,
            "years_experience_max": null,
            "date_posted": null,
            "education_requirement": null,
            "company_description": null,
            "company_hq": null,
            "industry_tags": [],
            "extraction_confidence": "high"
        });
        for (key, value) in overrides.as_object().cloned().unwrap_or_default() {
            base[key] = value;
        }
        base
    }

    #[test]
    fn normalize_url_adds_a_scheme_and_rejects_what_must_not_be_fetched() {
        assert_eq!(
            normalize_url("  wellfound.com/jobs/123 ").unwrap(),
            "https://wellfound.com/jobs/123"
        );
        assert!(normalize_url("https://jobs.example.com/1").is_ok());
        assert!(normalize_url("http://jobs.example.com/1").is_ok());

        for rejected in [
            "",
            "   ",
            "javascript:alert(1)",
            "file:///c:/secrets",
            "localhost",
        ] {
            assert!(
                normalize_url(rejected).is_err(),
                "{rejected} should be rejected"
            );
        }
    }

    #[test]
    fn finds_the_job_posting_among_several_json_ld_blocks() {
        let html = r#"
            <script type="application/ld+json">{"@type":"WebSite","name":"Board"}</script>
            <script>var notJsonLd = 1;</script>
            <script type="application/ld+json">{"@type":"JobPosting","title":"Rust Engineer","description":"<p>Build.</p>"}</script>
        "#;
        let posting = job_posting_from_json_ld(html).expect("a posting");
        assert_eq!(posting["title"], "Rust Engineer");
    }

    #[test]
    fn finds_a_job_posting_inside_an_at_graph_or_a_top_level_array() {
        let graph = r#"<script type="application/ld+json">
            {"@graph":[{"@type":"Organization"},{"@type":"JobPosting","title":"In graph","description":"d"}]}
        </script>"#;
        assert_eq!(job_posting_from_json_ld(graph).unwrap()["title"], "In graph");

        let array = r#"<script type="application/ld+json">
            [{"@type":"BreadcrumbList"},{"@type":["JobPosting"],"title":"In array","description":"d"}]
        </script>"#;
        assert_eq!(job_posting_from_json_ld(array).unwrap()["title"], "In array");
    }

    #[test]
    fn a_malformed_json_ld_block_does_not_abort_the_scan() {
        let html = r#"
            <script type="application/ld+json">{ this is not json </script>
            <script type="application/ld+json">{"@type":"JobPosting","title":"Survivor","description":"d"}</script>
        "#;
        assert_eq!(job_posting_from_json_ld(html).unwrap()["title"], "Survivor");
    }

    #[test]
    fn a_page_without_a_posting_falls_through() {
        let html =
            r#"<script type="application/ld+json">{"@type":"Organization","name":"Acme"}</script>"#;
        assert!(job_posting_from_json_ld(html).is_none());
    }

    #[test]
    fn a_stub_job_posting_is_not_usable() {
        let stub = serde_json::json!({ "@type": "JobPosting", "url": "https://example.test/1" });
        assert!(!json_ld_is_usable(&stub));
        assert!(!json_ld_is_usable(
            &serde_json::json!({ "title": "Engineer", "description": "  " })
        ));
        assert!(json_ld_is_usable(
            &serde_json::json!({ "title": "Engineer", "description": "Build things." })
        ));
    }

    #[test]
    fn reads_the_page_title() {
        assert_eq!(
            page_title("<html><head><title>Rust Engineer at Acme</title></head>"),
            "Rust Engineer at Acme"
        );
        assert_eq!(page_title("<html><body>no title</body></html>"), "");
    }

    #[test]
    fn truncate_for_prompt_cuts_on_a_word_boundary_and_marks_it() {
        let short = "a short posting";
        assert_eq!(truncate_for_prompt(short), short);

        // Multi-byte throughout: a byte-index slice would panic here.
        let long = "exp\u{e9}rience ".repeat(MAX_PROMPT_CHARS);
        let cut = truncate_for_prompt(&long);
        assert!(cut.ends_with("[... page text truncated ...]"));
        assert!(cut.contains("exp\u{e9}rience"));
        assert!(cut.chars().count() < long.chars().count());
    }

    #[test]
    fn extraction_prompt_carries_the_source_url_and_the_page_text() {
        let prompt = build_extraction_prompt(Some("https://jobs.example.com/1"), "Build services.");
        assert!(prompt.contains("https://jobs.example.com/1"));
        assert!(prompt.contains("Build services."));
        assert!(prompt.contains("Do not summarize"));
        assert!(prompt.contains("same language as the posting"));

        // No URL means no dangling "Source URL:" line.
        assert!(!build_extraction_prompt(None, "text").contains("Source URL"));
    }

    #[test]
    fn accepts_a_well_formed_extraction() {
        let job = parse_extraction_from_response(&responses_body(extracted(serde_json::json!({}))))
            .unwrap();
        assert_eq!(job.title, "Rust Engineer");
        assert_eq!(job.company, "Acme");
    }

    #[test]
    fn rejects_a_page_the_model_says_is_not_a_posting() {
        let body = responses_body(extracted(serde_json::json!({
            "is_job_posting": false,
            "title": "",
            "description": ""
        })));
        assert!(matches!(
            parse_extraction_from_response(&body),
            Err(ImportError::NotAJobPosting)
        ));
    }

    #[test]
    fn rejects_an_extraction_with_nothing_in_it() {
        let body = responses_body(extracted(serde_json::json!({
            "title": "  ",
            "description": ""
        })));
        assert!(matches!(
            parse_extraction_from_response(&body),
            Err(ImportError::EmptyResult)
        ));
    }

    #[test]
    fn an_extraction_refusal_is_reported_as_a_refusal_not_as_bad_json() {
        let body = serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "refusal", "refusal": "I cannot help with that." }]
            }]
        })
        .to_string();

        let message = parse_extraction_from_response(&body)
            .unwrap_err()
            .to_string();
        assert!(message.contains("declined"), "{message}");
        assert!(!message.contains("invalid"), "{message}");
    }

    #[test]
    fn a_truncated_extraction_is_reported_as_incomplete() {
        let body = serde_json::json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": []
        })
        .to_string();

        let message = parse_extraction_from_response(&body)
            .unwrap_err()
            .to_string();
        assert!(message.contains("max_output_tokens"), "{message}");
    }

    #[test]
    fn the_built_payload_names_its_own_parser() {
        let payload = build_payload(
            ImportMode::Url,
            Extraction::JsonLd,
            "https://boards.greenhouse.io/acme/jobs/1",
            Some("Rust Engineer at Acme"),
            "",
            serde_json::json!({ "@type": "JobPosting" }),
        );
        assert_eq!(payload["source"], "url_import");
        assert_eq!(payload["extraction"], "json_ld");
        assert_eq!(payload["pageTitle"], "Rust Engineer at Acme");

        let pasted = build_payload(
            ImportMode::Text,
            Extraction::Llm,
            "",
            None,
            "the page text",
            serde_json::json!({}),
        );
        assert_eq!(pasted["source"], "text_import");
        assert_eq!(pasted["extraction"], "llm");
        assert_eq!(pasted["sourceText"], "the page text");
    }

    #[test]
    fn blocked_and_thin_page_errors_both_name_the_paste_fallback() {
        // These two are the expected failures, not the exotic ones: a large share of boards
        // either refuse a non-browser request or render client-side. If either message stops
        // pointing at the paste tab, the second input mode becomes undiscoverable.
        for error in [
            ImportError::Blocked { status: 403 },
            ImportError::PageTooThin { chars: 12 },
        ] {
            assert!(error.to_string().contains("Paste text tab"), "{error}");
        }
    }
}
