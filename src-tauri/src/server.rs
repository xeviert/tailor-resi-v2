use crate::analysis::{analyze_job, AnalysisConfig, JobAnalysis};
use crate::commands::{failure_summary, store_and_emit_outcome};
use crate::tailoring::{
    failed_response, tailor_and_render, workspace_root, TailorRequest, TailorResponse,
};
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

/// Tags whose *bodies* are not prose. On a whole fetched page these carry minified
/// JavaScript, CSS selectors and inline SVG path data, none of which mean anything to a
/// reader or to an extraction model, and all of which would otherwise be paid for as prompt
/// tokens.
const SKIP_BODY_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "template", "iframe", "head",
];

/// Tags that end a line of prose. Everything else (`<b>`, `<a>`, `<span>`) sits *inside* a
/// sentence, so treating it as a line break would shred every paragraph into fragments.
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "br", "li", "ul", "ol", "tr", "td", "th", "table", "section", "article",
    "header", "footer", "nav", "aside", "main", "h1", "h2", "h3", "h4", "h5", "h6",
    "blockquote", "pre", "hr", "dl", "dt", "dd", "form", "fieldset", "figure", "figcaption",
];

/// The lowercased element name in `<p class="x">` or `</p>`; empty for `<!doctype …>`.
fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect()
}

/// Convert HTML to plain text.
///
/// Job boards differ on which field carries the posting body: the schema.org parser receives
/// `description` as HTML, while the other parsers receive it as plain text. Stripping here lets
/// every parser emit a plain `description` so downstream consumers (language detection, analysis
/// prompts) can rely on one field.
///
/// `block_separator` is what a *block* tag boundary becomes. A description fragment wants `" "`,
/// because it is rendered as one run of prose. A whole fetched page wants `"\n"`: the paragraph
/// and list structure of a posting is most of the signal an extraction model has to work with,
/// and flattening a requirements list into one line throws it away.
fn strip_html(html: &str, block_separator: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut text = String::with_capacity(html.len());
    let mut cursor = 0usize;

    while cursor < html.len() {
        let Some(offset) = html[cursor..].find('<') else {
            push_decoded(&mut text, &html[cursor..]);
            break;
        };
        let open = cursor + offset;
        push_decoded(&mut text, &html[cursor..open]);

        // A comment ends at `-->`, not at the first `>`: `<!-- a > b -->` is one comment, and
        // stopping early leaks its tail into the text.
        if lower[open..].starts_with("<!--") {
            cursor = lower[open + 4..]
                .find("-->")
                .map_or(html.len(), |end| open + 4 + end + 3);
            continue;
        }

        let tag_end = html[open..]
            .find('>')
            .map_or(html.len(), |end| open + end + 1);
        let name = tag_name(&lower[open..tag_end]);
        let closing = lower[open..].starts_with("</");

        if !closing && SKIP_BODY_TAGS.contains(&name.as_str()) {
            text.push_str(block_separator);
            // `<svg/>` has no body to skip; searching for its close tag would swallow the
            // entire rest of the document.
            let self_closing = html[open..tag_end].trim_end_matches('>').ends_with('/');
            cursor = if self_closing {
                tag_end
            } else {
                let close = format!("</{name}");
                lower[tag_end..]
                    .find(&close)
                    .map_or(html.len(), |end| tag_end + end)
            };
            continue;
        }

        // A tag boundary must not glue neighbouring words together.
        text.push_str(if BLOCK_TAGS.contains(&name.as_str()) {
            block_separator
        } else {
            " "
        });
        cursor = tag_end;
    }
    text
}

/// Append a run of markup-free HTML text, resolving character references as it goes.
fn push_decoded(text: &mut String, raw: &str) {
    let mut entity: Option<String> = None;
    for character in raw.chars() {
        match character {
            // A second `&` means the first one was a stray ampersand, not an entity opener.
            '&' => {
                if let Some(name) = entity.take() {
                    text.push('&');
                    text.push_str(&name);
                }
                entity = Some(String::new());
            }
            ';' if entity.is_some() => {
                let name = entity.take().unwrap_or_default();
                match decode_entity_owned(&name) {
                    Some(decoded) => text.push_str(&decoded),
                    // Showing an entity we cannot resolve beats silently dropping content.
                    None => {
                        text.push('&');
                        text.push_str(&name);
                        text.push(';');
                    }
                }
            }
            _ => match entity.as_mut() {
                // Entity names are short; anything longer is a stray ampersand.
                Some(name) if name.len() < 12 => name.push(character),
                Some(_) => {
                    let name = entity.take().unwrap_or_default();
                    text.push('&');
                    text.push_str(&name);
                    text.push(character);
                }
                None => text.push(character),
            },
        }
    }
    if let Some(name) = entity {
        text.push('&');
        text.push_str(&name);
    }
}

fn html_to_text(html: &str) -> String {
    strip_html(html, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whole-page variant of [`strip_html`]: one line per block, blank lines dropped.
pub(crate) fn html_to_block_text(html: &str) -> String {
    strip_html(html, "\n")
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entity_owned(name: &str) -> Option<String> {
    let known = decode_entity(name);
    if !known.is_empty() {
        return Some(known.to_string());
    }
    // Numeric references are open-ended, so a named table can never cover them. A French
    // posting served as `&#233;` would otherwise lose every accent it has.
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code).map(String::from)
}

fn decode_entity(name: &str) -> &'static str {
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "#39" => "'",
        "nbsp" | "#160" => " ",
        "rsquo" | "#8217" => "\u{2019}",
        "lsquo" | "#8216" => "\u{2018}",
        "ldquo" | "#8220" => "\u{201C}",
        "rdquo" | "#8221" => "\u{201D}",
        "hellip" | "#8230" => "\u{2026}",
        "ndash" | "#8211" => "\u{2013}",
        "mdash" | "#8212" => "\u{2014}",
        "eacute" => "\u{e9}",
        "egrave" => "\u{e8}",
        "agrave" => "\u{e0}",
        "ccedil" => "\u{e7}",
        "ocirc" => "\u{f4}",
        "icirc" => "\u{ee}",
        "ecirc" => "\u{ea}",
        "ugrave" => "\u{f9}",
        _ => "",
    }
}

/// schema.org `JobPosting`.
///
/// Welcome to the Jungle embeds one of these, and so does most of the ATS-hosted web, so the
/// same reader serves the board scraper and the URL importer. `domain` is the label the parsed
/// capture carries; it is what `JobPanel` switches on to pick a renderer.
fn parse_schema_org_job_posting(payload: &serde_json::Value, domain: &str) -> serde_json::Value {
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
    let mut description = html_to_text(description_html);
    // Some feeds escape the description twice, so one pass leaves `&lt;p&gt;` as visible
    // `<p>` markup. A second pass costs nothing and turns that back into prose.
    if description.contains("</") {
        description = html_to_text(&description);
    }
    let company = org["name"].as_str().unwrap_or("");

    if title.is_empty() {
        warnings.push("Missing field: title".into());
        eprintln!("[parser:{domain}] Warning: missing title");
    }
    if description_html.is_empty() {
        warnings.push("Missing field: description_html".into());
        eprintln!("[parser:{domain}] Warning: missing description_html");
    }
    if company.is_empty() {
        warnings.push("Missing field: company".into());
        eprintln!("[parser:{domain}] Warning: missing company");
    }

    serde_json::json!({
        "domain": domain,
        "parsed": true,
        "url": payload["sourceUrl"].as_str().unwrap_or(""),
        "title": title,
        "description": description,
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

/// The label a manually imported capture carries as its `domain`.
///
/// The full host, never a shortened form: `JobPanel` switches on `domain`, so a bare `indeed`
/// would route an imported capture into the Indeed renderer, which reads Indeed's own field
/// shape. A full host matches none of those arms and falls through to the generic renderer.
fn imported_domain_label(payload: &serde_json::Value) -> String {
    let source_url = payload["sourceUrl"].as_str().unwrap_or("").trim();
    if source_url.is_empty() {
        return "pasted-text".to_string();
    }
    reqwest::Url::parse(source_url)
        .ok()
        .and_then(|url| url.host_str().map(String::from))
        .unwrap_or_else(|| "imported".to_string())
}

/// A capture the app extracted for itself with the AI layer.
///
/// `payload["json"]` is the model's `ExtractedJob`, whose keys deliberately match the ones the
/// board parsers emit, so everything downstream of here sees no difference.
fn parse_extracted_job(payload: &serde_json::Value, domain: &str) -> serde_json::Value {
    let job = &payload["json"];
    let mut warnings: Vec<String> = Vec::new();

    let title = job["title"].as_str().unwrap_or("").trim();
    let description = job["description"].as_str().unwrap_or("").trim();
    let company = job["company"].as_str().unwrap_or("").trim();

    if title.is_empty() {
        warnings.push("Missing field: title".into());
        eprintln!("[parser:{domain}] Warning: missing title");
    }
    if description.is_empty() {
        warnings.push("Missing field: description".into());
        eprintln!("[parser:{domain}] Warning: missing description");
    }
    if company.is_empty() {
        warnings.push("Missing field: company".into());
        eprintln!("[parser:{domain}] Warning: missing company");
    }

    // An AI-derived capture always announces itself. The user is looking at this panel
    // precisely because some earlier capture was wrong, so "a model wrote this" is the single
    // most useful thing the panel can tell them.
    warnings.push(
        if payload["source"].as_str() == Some("text_import") {
            "Imported by AI from pasted text - check the details against the original post."
        } else {
            "Imported by AI from the page text - check the details against the original post."
        }
        .into(),
    );
    if job["extraction_confidence"].as_str().unwrap_or("high") != "high" {
        warnings.push("The AI was not confident about this extraction.".into());
    }

    let mut parsed = serde_json::json!({
        "domain": domain,
        "parsed": true,
        "url": payload["sourceUrl"].as_str().unwrap_or(""),
        "title": title,
        "description": description,
        "company": company,
        "location": job["location"],
        "locations": job["locations"],
        "job_type": job["job_type"],
        "remote": job["remote"],
        "compensation": job["compensation"],
        "qualifications": job["qualifications"],
        "skills": job["skills"],
        "years_experience_min": job["years_experience_min"],
        "years_experience_max": job["years_experience_max"],
        "date_posted": job["date_posted"],
        "education_requirement": job["education_requirement"],
        "company_description": job["company_description"],
        "company_hq": job["company_hq"],
        "industry_tags": job["industry_tags"],
        "warnings": warnings,
    });
    // The model returns null for anything the post did not state. `prompt_job_view` drops
    // nulls on the way to a prompt, but the capture the panel renders should not carry them
    // either.
    if let Some(object) = parsed.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    parsed
}

pub(crate) fn parse_job_data(payload: &serde_json::Value) -> serde_json::Value {
    // An imported capture names its own parser, and must do so before the board chain gets a
    // look. Its `sourceUrl` may well point at a board that has a scraper below, but the payload
    // was never produced by that scraper, so that parser would read the wrong shape out of
    // `json` and silently drop the posting.
    if matches!(
        payload["source"].as_str(),
        Some("url_import" | "text_import")
    ) {
        let domain = imported_domain_label(payload);
        return if payload["extraction"].as_str() == Some("json_ld") {
            parse_schema_org_job_posting(payload, &domain)
        } else {
            parse_extracted_job(payload, &domain)
        };
    }

    let source_url = payload["sourceUrl"].as_str().unwrap_or("");
    if source_url.contains("wellfound.com") {
        parse_wellfound(payload)
    } else if source_url.contains("welcometothejungle.com") {
        parse_schema_org_job_posting(payload, "welcometothejungle")
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

/// Fields that carry no ATS signal, or that duplicate a field the model already gets.
///
/// `description_html` is the same prose as `description` wrapped in markup, and for a
/// Welcome to the Jungle capture it is the single largest field in the payload. `raw` is the
/// whole original extension payload, re-embedded by the unknown-domain fallback. Both prompts
/// are built from the same parsed capture, so anything left here is paid for on every
/// tailoring attempt as well as on analysis.
const PROMPT_IRRELEVANT_FIELDS: &[&str] = &[
    "description_html",
    "raw",
    "company_logo",
    "parsed",
    "warnings",
];

/// The view of a parsed capture that goes into an LLM prompt.
pub fn prompt_job_view(parsed: &serde_json::Value) -> serde_json::Value {
    let Some(object) = parsed.as_object() else {
        return parsed.clone();
    };
    object
        .iter()
        .filter(|(key, _)| !PROMPT_IRRELEVANT_FIELDS.contains(&key.as_str()))
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

pub(crate) fn capture_directory() -> Result<PathBuf, String> {
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

/// Drop the pointer to the job the app opens with.
///
/// Removing a file that is already gone is the state the caller asked for, not a failure -
/// Start over pressed twice must not report an error. Only the pointer goes: the
/// timestamped capture it was copied from stays in the same directory.
pub(crate) fn remove_latest_capture(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
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
        "bridge": "tauri-rust",
        "result_protocol_version": 2
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
    let parsed_fallback = parse_job_data(&payload);
    let captured = match persist_capture(&payload) {
        Ok(captured) => captured,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AnalyzeResponse {
                    success: false,
                    message: "Job data could not be persisted",
                    parsed: parsed_fallback,
                    analysis_status: "failed",
                    analysis: None,
                    analysis_error: Some(error),
                    tailoring_status: "not_run",
                    variant_slug: None,
                    variant_json_path: None,
                    docx_path: None,
                    report_json_path: None,
                    validation_status: "not_run",
                    fit_status: "not_run",
                    page_count: None,
                    tailoring_error: None,
                }),
            );
        }
    };
    let parsed = captured.parsed.clone();
    if let Err(e) = state.app_handle.emit("job-data-received", &captured) {
        eprintln!("[server] Failed to emit event: {e}");
    }
    let capture_id = match u64::try_from(captured.received_at_ms) {
        Ok(capture_id) => capture_id,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AnalyzeResponse {
                    success: false,
                    message: "Job data received",
                    parsed,
                    analysis_status: "failed",
                    analysis: None,
                    analysis_error: Some("Capture timestamp is out of range.".to_string()),
                    tailoring_status: "not_run",
                    variant_slug: None,
                    variant_json_path: None,
                    docx_path: None,
                    report_json_path: None,
                    validation_status: "not_run",
                    fit_status: "not_run",
                    page_count: None,
                    tailoring_error: None,
                }),
            );
        }
    };
    let root = workspace_root().ok();

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
                if let Some(root) = root.as_ref() {
                    if let Err(error) = store_and_emit_outcome(
                        &state.app_handle,
                        root,
                        capture_id,
                        "en",
                        "analysis_ready",
                        analysis.summary.clone(),
                        Some(&analysis),
                        None,
                        None,
                        None,
                    ) {
                        eprintln!("[pipeline-result] HTTP analysis outcome failed: {error}");
                    }
                }
                ("completed", Some(analysis), None)
            }
            Err(error) => {
                let message = error.to_string();
                eprintln!("[analysis] {message}");
                if let Some(root) = root.as_ref() {
                    if let Err(error) = store_and_emit_outcome(
                        &state.app_handle,
                        root,
                        capture_id,
                        "en",
                        "failed",
                        failure_summary("ats_analysis", &message, None),
                        None,
                        None,
                        Some("ats_analysis"),
                        Some(message.clone()),
                    ) {
                        eprintln!("[pipeline-result] HTTP failure outcome failed: {error}");
                    }
                }
                ("failed", None, Some(message))
            }
        },
        None => {
            let message = "OPENAI_API_KEY is required to analyze and tailor a resume.".to_string();
            if let Some(root) = root.as_ref() {
                if let Err(error) = store_and_emit_outcome(
                    &state.app_handle,
                    root,
                    capture_id,
                    "en",
                    "failed",
                    failure_summary("ats_analysis", &message, None),
                    None,
                    None,
                    Some("ats_analysis"),
                    Some(message.clone()),
                ) {
                    eprintln!("[pipeline-result] HTTP failure outcome failed: {error}");
                }
            }
            ("skipped_no_api_key", None, Some(message))
        }
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
                analysis: analysis.clone(),
                approved_evidence: vec![],
                priority_attested_terms: vec![],
            };
            match tailor_and_render(request).await {
                Ok(response) => {
                    if let Some(root) = root.as_ref() {
                        if let Err(error) = store_and_emit_outcome(
                            &state.app_handle,
                            root,
                            capture_id,
                            "en",
                            response.tailoring_status,
                            analysis.summary.clone(),
                            Some(&analysis),
                            Some(&response),
                            (response.tailoring_status == "failed").then_some("resume_tailoring"),
                            response.error.clone(),
                        ) {
                            eprintln!("[pipeline-result] HTTP tailoring outcome failed: {error}");
                        }
                    }
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
                    let response = failed_response(message.clone());
                    if let Some(root) = root.as_ref() {
                        if let Err(error) = store_and_emit_outcome(
                            &state.app_handle,
                            root,
                            capture_id,
                            "en",
                            "failed",
                            failure_summary("resume_tailoring", &message, Some(&analysis)),
                            Some(&analysis),
                            Some(&response),
                            Some("resume_tailoring"),
                            Some(message.clone()),
                        ) {
                            eprintln!(
                                "[pipeline-result] HTTP tailoring failure outcome failed: {error}"
                            );
                        }
                    }
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
    use super::{
        html_to_block_text, html_to_text, parse_job_data, parse_latest_capture, prompt_job_view,
        remove_latest_capture, write_latest_capture, CapturedJob,
    };
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
        // Every parser must emit a plain-text description, not just the HTML variant.
        assert_eq!(parsed["description"], "Build services.");
        assert_eq!(parsed["description_html"], "<p>Build services.</p>");
    }

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        assert_eq!(
            html_to_text(
                "<h3><strong>Notre &eacute;quipe</strong></h3><p>90 ing&eacute;nieurs</p>"
            ),
            "Notre \u{e9}quipe 90 ing\u{e9}nieurs"
        );
        // Adjacent block elements must not glue words together.
        assert_eq!(html_to_text("<li>Rust</li><li>Axum</li>"), "Rust Axum");
        assert_eq!(html_to_text("R&amp;D at 100&nbsp;%"), "R&D at 100 %");
        // A stray ampersand is preserved rather than swallowed as an entity.
        assert_eq!(html_to_text("salary & bonus"), "salary & bonus");
        assert_eq!(html_to_text(""), "");
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
        assert!(
            parse_latest_capture(r#"{"receivedAt":"2026-08-14T12:33:44Z","json":{}}"#).is_err()
        );
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

    #[test]
    fn clearing_the_latest_capture_is_idempotent_and_keeps_the_archive() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("resi-tailor-clear-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let latest = directory.join("latest.json");
        let archived = directory.join("1787328720134-job.json");
        write_latest_capture(&latest, "{\"version\":1}
").unwrap();
        fs::write(&archived, "{\"version\":1}
").unwrap();

        remove_latest_capture(&latest).unwrap();
        assert!(!latest.exists());
        // Starting over abandons a job; it does not erase the capture it came from.
        assert!(archived.is_file());

        // Pressing Start over again is not an error.
        remove_latest_capture(&latest).unwrap();

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn prompt_job_view_drops_the_duplicate_html_description() {
        let parsed = serde_json::json!({
            "title": "Lead Front Engineer",
            "description": "Build services.",
            "description_html": "<p>Build services.</p>",
            "company_logo": "https://example.test/logo.png",
            "parsed": true,
            "warnings": [],
            "location": serde_json::Value::Null,
            "url": "https://example.test/job"
        });

        let view = prompt_job_view(&parsed);

        assert_eq!(view["description"], "Build services.");
        assert_eq!(view["title"], "Lead Front Engineer");
        assert_eq!(view["url"], "https://example.test/job");
        for dropped in [
            "description_html",
            "company_logo",
            "parsed",
            "warnings",
            "location",
        ] {
            assert!(view.get(dropped).is_none(), "{dropped} should not be sent");
        }
    }

    #[test]
    fn prompt_job_view_drops_the_raw_payload_the_fallback_parser_embeds() {
        let parsed = parse_job_data(&serde_json::json!({
            "sourceUrl": "https://jobs.example.test/posting/1",
            "json": { "title": "Engineer", "description": "Ship things.", "company": "Acme" }
        }));

        assert!(parsed.get("raw").is_some(), "the fallback still stores it");
        assert!(prompt_job_view(&parsed).get("raw").is_none());
    }

    #[test]
    fn prompt_job_view_passes_through_a_non_object() {
        assert_eq!(
            prompt_job_view(&serde_json::json!("not an object")),
            serde_json::json!("not an object")
        );
    }

    #[test]
    fn html_to_text_drops_script_and_style_bodies() {
        let page = "<style>.a{color:red}</style><p>Rust Engineer</p>\
                    <script>window.__NEXT_DATA__={\"secret\":1}</script><p>Build services.</p>";
        let text = html_to_text(page);

        assert_eq!(text, "Rust Engineer Build services.");
        assert!(!text.contains("__NEXT_DATA__"));
        assert!(!text.contains("color:red"));
    }

    #[test]
    fn html_to_text_ends_a_comment_at_the_comment_close() {
        // The `>` inside the comment does not end it, so nothing after that `>` may leak into
        // the text. A comment is also not a word boundary, so it must not split `ad` either.
        assert_eq!(
            html_to_text("<p>Rust<!-- version > 1.70 --> and Axum</p>"),
            "Rust and Axum"
        );
        assert_eq!(html_to_text("<p>a<!-- b > c -->d</p>"), "ad");
    }

    #[test]
    fn html_to_text_keeps_inline_markup_inside_one_sentence() {
        assert_eq!(
            html_to_text("<p>We need <b>Rust</b> and <i>Axum</i>.</p>"),
            "We need Rust and Axum ."
        );
    }

    #[test]
    fn html_to_text_decodes_numeric_character_references() {
        // A French post served as numeric references would otherwise lose every accent.
        assert_eq!(
            html_to_text("exp&#233;rience &#x00e9;quipe"),
            "exp\u{e9}rience \u{e9}quipe"
        );
    }

    #[test]
    fn html_to_block_text_keeps_one_line_per_block() {
        let page = "<html><head><title>t</title><style>.a{}</style></head><body>\
                    <h2>Requirements</h2><ul><li>Rust</li><li>Axum</li></ul></body></html>";

        assert_eq!(html_to_block_text(page), "Requirements\nRust\nAxum");
    }

    #[test]
    fn an_imported_capture_bypasses_the_board_parsers() {
        // The whole design rests on this. An imported capture's URL may point at a board that
        // has a scraper, but the payload was never produced by that scraper.
        let parsed = parse_job_data(&json!({
            "source": "url_import",
            "extraction": "llm",
            "sourceUrl": "https://www.indeed.com/viewjob?jk=1",
            "json": {
                "is_job_posting": true,
                "title": "Rust Engineer",
                "company": "Acme",
                "description": "Build reliable services.",
                "skills": ["Rust", "Axum"],
                "extraction_confidence": "high"
            }
        }));

        assert_ne!(parsed["domain"], "indeed");
        assert_eq!(parsed["domain"], "www.indeed.com");
        assert_eq!(parsed["parsed"], true);
        assert_eq!(parsed["title"], "Rust Engineer");
        assert_eq!(parsed["skills"][1], "Axum");
    }

    #[test]
    fn an_imported_json_ld_capture_reuses_the_schema_org_parser() {
        let parsed = parse_job_data(&json!({
            "source": "url_import",
            "extraction": "json_ld",
            "sourceUrl": "https://boards.greenhouse.io/acme/jobs/1",
            "json": {
                "title": "Rust Engineer",
                "description": "<p>Build services.</p>",
                "employmentType": "FULL_TIME",
                "hiringOrganization": { "name": "Acme" },
                "jobLocation": [{
                    "address": { "addressLocality": "Paris", "addressCountry": "France" }
                }]
            }
        }));

        assert_eq!(parsed["domain"], "boards.greenhouse.io");
        assert_eq!(parsed["description"], "Build services.");
        assert_eq!(parsed["description_html"], "<p>Build services.</p>");
        assert_eq!(parsed["locations"][0], "Paris, France");
        assert_eq!(parsed["job_type"], "Full-time");
    }

    #[test]
    fn a_double_escaped_description_is_stripped_twice() {
        let parsed = parse_job_data(&json!({
            "source": "url_import",
            "extraction": "json_ld",
            "sourceUrl": "https://boards.greenhouse.io/acme/jobs/1",
            "json": { "title": "Engineer", "description": "&lt;p&gt;Build services.&lt;/p&gt;" }
        }));

        assert_eq!(parsed["description"], "Build services.");
    }

    #[test]
    fn a_pasted_text_capture_has_no_host_to_name() {
        let parsed = parse_job_data(&json!({
            "source": "text_import",
            "extraction": "llm",
            "sourceUrl": "",
            "json": {
                "title": "Rust Engineer",
                "company": "Acme",
                "description": "Build services.",
                "extraction_confidence": "high"
            }
        }));

        assert_eq!(parsed["domain"], "pasted-text");
        assert_eq!(parsed["url"], "");
    }

    #[test]
    fn an_ai_imported_capture_says_so_and_flags_low_confidence() {
        let parsed = parse_job_data(&json!({
            "source": "text_import",
            "extraction": "llm",
            "sourceUrl": "",
            "json": {
                "title": "Rust Engineer",
                "company": "Acme",
                "description": "Build services.",
                "extraction_confidence": "low"
            }
        }));
        let joined = parsed["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|warning| warning.as_str())
            .collect::<Vec<_>>()
            .join(" | ");

        assert!(
            joined.contains("Imported by AI from pasted text"),
            "{joined}"
        );
        assert!(joined.contains("not confident"), "{joined}");
    }

    #[test]
    fn an_imported_capture_drops_the_fields_the_post_did_not_state() {
        let parsed = parse_job_data(&json!({
            "source": "url_import",
            "extraction": "llm",
            "sourceUrl": "https://jobs.example.test/1",
            "json": {
                "title": "Rust Engineer",
                "company": "Acme",
                "description": "Build services.",
                "compensation": null,
                "remote": null,
                "extraction_confidence": "high"
            }
        }));

        assert!(parsed.get("compensation").is_none());
        assert!(parsed.get("remote").is_none());
    }

    #[test]
    fn prompt_job_view_on_an_imported_capture_carries_no_page_text() {
        // `sourceText` is a debugging handle on the payload. It must never become a prompt.
        let payload = json!({
            "source": "url_import",
            "extraction": "llm",
            "sourceUrl": "https://jobs.example.test/1",
            "sourceText": "the entire fetched page, navigation and all",
            "json": {
                "title": "Rust Engineer",
                "company": "Acme",
                "description": "Build services.",
                "extraction_confidence": "high"
            }
        });
        let view = prompt_job_view(&parse_job_data(&payload));

        assert!(view.get("sourceText").is_none());
        assert!(view.get("raw").is_none());
        assert_eq!(view["description"], "Build services.");
    }
}
