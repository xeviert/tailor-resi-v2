use std::sync::OnceLock;
use std::time::Duration;

/// Both AI stages talk to the same host, so they share one client: connection reuse across
/// the tailoring retry loop matters more than isolation, and a per-call `Client::new()` leaks
/// a fresh connection pool every attempt.
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// A reasoning model working through a full resume rewrite is slow, so this is generous. It
/// exists to stop a hung connection from wedging the desktop app forever, not to bound
/// normal latency.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Cache keys sent as `prompt_cache_key` on every request.
///
/// The key influences routing only: a low-volume app whose requests scatter across machines
/// keeps missing the warm one, and grouping a stage's calls under one key is what keeps them
/// landing together. It does not pin a machine and it does not by itself make anything
/// cacheable - tailoring needed an explicit cache breakpoint for that, see
/// `build_tailoring_request`.
///
/// The key is per *stage*, deliberately not per capture or per job. Everything sharing a
/// stage also shares that stage's constant prompt prefix, and routing them together is the
/// entire point. Bump the trailing version whenever a stage's constant prefix text changes,
/// so stale entries are not fought over.
pub const PROMPT_CACHE_KEY_JOB_ANALYSIS: &str = "resume-workbench:job_analysis:v1";
pub const PROMPT_CACHE_KEY_JOB_IMPORT: &str = "resume-workbench:job_import:v1";
pub const PROMPT_CACHE_KEY_RESUME_TAILORING: &str = "resume-workbench:resume_tailoring:v2";

pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|error| {
                // The fallback has to carry the same budget. A bare `Client::new()` has no
                // timeout at all, so a builder failure would quietly turn every OpenAI call
                // in the app into an unbounded wait - the exact wedge the timeout prevents.
                eprintln!("[http] Falling back to a default client: {error}");
                reqwest::Client::builder()
                    .timeout(REQUEST_TIMEOUT)
                    .connect_timeout(CONNECT_TIMEOUT)
                    .build()
                    .expect("a client with only timeouts set must build")
            })
    })
}

/// Whether a failed OpenAI call is worth sending again.
///
/// Rate limits and server faults are transient. Every other 4xx is a request the caller built
/// wrong — a bad key, a malformed schema — and repeating it only wastes the user's time.
pub fn status_is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Exponential backoff: 1s, 2s, 4s, ...
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(1000u64 << attempt.min(4))
}

#[cfg(test)]
mod tests {
    use super::{retry_delay, status_is_retryable};
    use reqwest::StatusCode;

    #[test]
    fn retries_rate_limits_and_server_faults_only() {
        assert!(status_is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(status_is_retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(status_is_retryable(StatusCode::BAD_GATEWAY));
        assert!(status_is_retryable(StatusCode::SERVICE_UNAVAILABLE));

        assert!(!status_is_retryable(StatusCode::UNAUTHORIZED));
        assert!(!status_is_retryable(StatusCode::BAD_REQUEST));
        assert!(!status_is_retryable(StatusCode::NOT_FOUND));
        assert!(!status_is_retryable(StatusCode::FORBIDDEN));
    }

    #[test]
    fn backoff_grows_and_then_caps() {
        assert_eq!(retry_delay(0).as_millis(), 1000);
        assert_eq!(retry_delay(1).as_millis(), 2000);
        assert_eq!(retry_delay(2).as_millis(), 4000);
        assert_eq!(retry_delay(9), retry_delay(4));
    }
}
