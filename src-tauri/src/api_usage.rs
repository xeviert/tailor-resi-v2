use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApiTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}

/// Environment flag that turns on prompt fingerprinting.
///
/// Off by default: the fingerprints exist to investigate caching, not to be carried by every
/// receipt forever.
pub const PROMPT_DEBUG_ENV: &str = "RESUME_WORKBENCH_PROMPT_DEBUG";

/// Identifies what was actually sent, so two receipts can be compared without guesswork.
///
/// The receipts show that a cached-token count above zero has only ever meant a whole-request
/// repeat - a partial prefix hit has never been recorded - and reading a request's bytes back
/// out of a provider response is impossible. So the request hashes itself on the way out.
/// `prefix_hash` covers the zone that is meant to be constant across a run's attempts;
/// `body_hash` covers the whole request, which is what tells a genuine retry apart from the
/// same call being issued twice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PromptFingerprint {
    pub prefix_hash: u64,
    pub prefix_chars: u32,
    pub body_hash: u64,
}

/// FNV-1a, written out rather than taken from `DefaultHasher`.
///
/// These hashes are compared across receipts written days apart, possibly by different builds.
/// `DefaultHasher`'s output is explicitly not guaranteed stable between Rust releases, which
/// would silently turn "the prefix changed" into an artefact of a toolchain upgrade.
fn fnv1a(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn prompt_debug_enabled() -> bool {
    std::env::var(PROMPT_DEBUG_ENV)
        .map(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// Fingerprints an outgoing Responses API request, or `None` when the flag is off.
///
/// `prefix_marker` is the first byte of the volatile zone - everything above it is the text the
/// provider is supposed to be able to reuse. Both AI stages order their prompt so that such a
/// marker exists; each one owns its own.
pub fn fingerprint_request(
    request_body: &serde_json::Value,
    prefix_marker: &str,
) -> Option<PromptFingerprint> {
    if !prompt_debug_enabled() {
        return None;
    }
    Some(fingerprint_request_always(request_body, prefix_marker))
}

/// The flagless form, so tests can exercise it without touching the environment.
pub(crate) fn fingerprint_request_always(
    request_body: &serde_json::Value,
    fallback_marker: &str,
) -> PromptFingerprint {
    let serialized = serde_json::to_string(request_body).unwrap_or_default();
    let prefix = declared_prefix(request_body, fallback_marker);

    PromptFingerprint {
        prefix_hash: fnv1a(prefix),
        prefix_chars: prefix.chars().count() as u32,
        body_hash: fnv1a(&serialized),
    }
}

/// The text a request declares as its reusable prefix.
///
/// Preferring the block that carries a `prompt_cache_breakpoint` means the fingerprint reports
/// what was actually offered to the provider for caching, not what the caller believes it
/// built. A stage with no breakpoint - analysis, whose constant zone is under the cache floor
/// anyway - falls back to splitting its single message at its own zone marker.
fn declared_prefix<'a>(request_body: &'a serde_json::Value, fallback_marker: &str) -> &'a str {
    let messages = request_body
        .get("input")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let breakpointed = messages
        .iter()
        .filter_map(|message| message.get("content")?.as_array())
        .flatten()
        .find(|block| block.get("prompt_cache_breakpoint").is_some())
        .and_then(|block| block.get("text"))
        .and_then(serde_json::Value::as_str);
    if let Some(text) = breakpointed {
        return text;
    }

    let user_message = messages
        .iter()
        .find(|message| message.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    // No marker means the prompt no longer has the zone boundary it is built around. Hashing
    // the whole message then reports a prefix that changes every call, which is the truth.
    user_message
        .split_once(fallback_marker)
        .map(|(head, _)| head)
        .unwrap_or(user_message)
}

/// What run a receipt belongs to.
///
/// Without this a receipt can only be tied back to the run that caused it by timestamp
/// proximity, which is guesswork exactly where it matters most - a four-attempt tailoring run
/// writes four receipts seconds apart and they all look alike.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageContext {
    pub capture_id: Option<u64>,
    pub attempt: Option<u32>,
    /// Present only while `RESUME_WORKBENCH_PROMPT_DEBUG` is set.
    pub prompt: Option<PromptFingerprint>,
}

impl UsageContext {
    pub fn attempt(capture_id: Option<u64>, attempt: u32) -> Self {
        Self {
            capture_id,
            attempt: Some(attempt),
            prompt: None,
        }
    }

    #[must_use]
    pub fn with_prompt(mut self, prompt: Option<PromptFingerprint>) -> Self {
        self.prompt = prompt;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ApiUsageRecord {
    schema_version: u8,
    recorded_at_ms: u128,
    stage: String,
    requested_model: String,
    response_id: Option<String>,
    response_model: Option<String>,
    /// Which capture this call was serving, and which attempt within its run.
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    /// Fingerprints of the request that produced this receipt. Diagnostic, and absent unless
    /// `RESUME_WORKBENCH_PROMPT_DEBUG` was set for the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_prefix_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_prefix_chars: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_body_hash: Option<String>,
    usage: ApiTokenUsage,
}

fn token_usage(response: &serde_json::Value) -> Option<ApiTokenUsage> {
    let usage = response.get("usage")?;
    Some(ApiTokenUsage {
        input_tokens: usage.get("input_tokens")?.as_u64()?,
        output_tokens: usage.get("output_tokens")?.as_u64()?,
        total_tokens: usage.get("total_tokens")?.as_u64()?,
        cached_input_tokens: usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(serde_json::Value::as_u64),
        reasoning_output_tokens: usage
            .get("output_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(serde_json::Value::as_u64),
    })
}

fn workspace_root() -> Option<PathBuf> {
    let current = std::env::current_dir().ok()?;
    current
        .ancestors()
        .find(|candidate| candidate.join("resume").join("content").is_dir())
        .map(Path::to_path_buf)
}

fn safe_component(value: &str) -> String {
    let safe = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    safe.trim_matches('-').chars().take(80).collect()
}

/// Persists a usage-only receipt for one API response.
///
/// Call this before parsing the response, not after. A refused, incomplete, or malformed
/// response is billed just like a good one, and recording it afterwards means the failures that
/// drive the retry loop - the expensive ones - are the exact calls missing from the ledger.
pub fn record_response_usage(
    stage: &str,
    requested_model: &str,
    body: &str,
    context: UsageContext,
) {
    let result = (|| -> Result<(), String> {
        let response: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        let Some(usage) = token_usage(&response) else {
            return Ok(());
        };
        let root = workspace_root().ok_or_else(|| "workspace root not found".to_string())?;
        let recorded_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis();
        let response_id = response
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let record = ApiUsageRecord {
            schema_version: 2,
            recorded_at_ms,
            stage: stage.to_string(),
            requested_model: requested_model.to_string(),
            response_id: response_id.clone(),
            response_model: response
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            capture_id: context.capture_id,
            attempt: context.attempt,
            prompt_prefix_hash: context
                .prompt
                .map(|prompt| format!("{:016x}", prompt.prefix_hash)),
            prompt_prefix_chars: context.prompt.map(|prompt| prompt.prefix_chars),
            request_body_hash: context
                .prompt
                .map(|prompt| format!("{:016x}", prompt.body_hash)),
            usage,
        };
        let directory = root.join("data").join("api-usage");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let identifier = response_id
            .as_deref()
            .map(safe_component)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "response".to_string());
        let path = directory.join(format!(
            "{recorded_at_ms}-{}-{identifier}.json",
            safe_component(stage)
        ));
        let json = serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?;
        fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("[api-usage] Could not persist {stage} usage metadata: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{fingerprint_request_always, token_usage};
    use serde_json::json;

    const MARKER: &str = "\nVolatile: ";

    fn request(user_content: &str) -> serde_json::Value {
        json!({
            "model": "test-model",
            "input": [
                { "role": "system", "content": "system text" },
                { "role": "user", "content": user_content }
            ]
        })
    }

    #[test]
    fn extracts_responses_api_token_usage() {
        let usage = token_usage(&json!({
            "usage": {
                "input_tokens": 120,
                "output_tokens": 45,
                "total_tokens": 165,
                "input_tokens_details": { "cached_tokens": 20 },
                "output_tokens_details": { "reasoning_tokens": 11 }
            }
        }))
        .unwrap();
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.cached_input_tokens, Some(20));
        assert_eq!(usage.reasoning_output_tokens, Some(11));
    }

    /// The whole point of the fingerprint: two requests that share a constant zone must agree
    /// on `prefix_hash` while disagreeing on `body_hash`. If a run's attempts ever show two
    /// different prefix hashes, the prompt builder - not the provider - is why nothing caches.
    #[test]
    fn a_shared_prefix_hashes_the_same_while_the_body_differs() {
        let first = fingerprint_request_always(&request("constant head\nVolatile: job one"), MARKER);
        let second =
            fingerprint_request_always(&request("constant head\nVolatile: job two"), MARKER);

        assert_eq!(first.prefix_hash, second.prefix_hash);
        assert_eq!(first.prefix_chars, "constant head".chars().count() as u32);
        assert_ne!(first.body_hash, second.body_hash);
    }

    /// A duplicated call and a genuine retry look identical in a receipt today. The body hash
    /// is what separates them.
    #[test]
    fn an_identical_request_hashes_identically_end_to_end() {
        let body = request("constant head\nVolatile: same job");
        assert_eq!(
            fingerprint_request_always(&body, MARKER),
            fingerprint_request_always(&body, MARKER)
        );
    }

    /// A changed constant zone must not be reported as a stable prefix, or the diagnostic would
    /// hide the very drift it exists to catch.
    #[test]
    fn a_changed_constant_zone_changes_the_prefix_hash() {
        let first = fingerprint_request_always(&request("constant head\nVolatile: job"), MARKER);
        let second = fingerprint_request_always(&request("edited head\nVolatile: job"), MARKER);

        assert_ne!(first.prefix_hash, second.prefix_hash);
    }

    /// If the marker is gone the prompt has lost its zone boundary, so the honest answer is
    /// that nothing is constant - not a prefix hash that happens to look stable.
    #[test]
    fn a_missing_marker_falls_back_to_the_whole_message() {
        let fingerprint = fingerprint_request_always(&request("no boundary here"), MARKER);
        assert_eq!(
            fingerprint.prefix_chars,
            "no boundary here".chars().count() as u32
        );
    }
}
