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

pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|error| {
                eprintln!("[http] Falling back to a default client: {error}");
                reqwest::Client::new()
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
