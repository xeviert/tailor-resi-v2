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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ApiUsageRecord {
    schema_version: u8,
    recorded_at_ms: u128,
    stage: String,
    requested_model: String,
    response_id: Option<String>,
    response_model: Option<String>,
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

pub fn record_response_usage(stage: &str, requested_model: &str, body: &str) {
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
            schema_version: 1,
            recorded_at_ms,
            stage: stage.to_string(),
            requested_model: requested_model.to_string(),
            response_id: response_id.clone(),
            response_model: response
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
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
    use super::token_usage;
    use serde_json::json;

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
}
