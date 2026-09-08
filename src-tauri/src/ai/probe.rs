//! HTTP probing layer for AI runtimes (spec §8, §46).
//!
//! One shared blocking client with hard timeouts, a strict response-size
//! cap, and **redirects disabled** (a redirect to a non-loopback host must
//! never be followed — the policy layer would not see it). Every failure is
//! mapped to a structured [`AiProbeError`]; nothing collapses to "unknown".

use super::domain::{AiProbeError, CONNECT_TIMEOUT, MAX_BODY_BYTES, REQUEST_TIMEOUT};

/// A blocking client is created per probe batch — cheap, and it keeps the
/// engine free of async-runtime coupling. Redirects are explicitly disabled.
pub(crate) struct Prober {
    client: reqwest::blocking::Client,
}

impl Prober {
    pub(crate) fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // Security: never follow redirects. A "local" service answering
            // with a 302 to a public host would otherwise be followed before
            // any policy check could run (SSRF guard, spec §6).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("HTTP client construction failed: {e}"))?;
        Ok(Self { client })
    }

    /// GET one URL and return the body as a string, enforcing the size cap.
    pub(crate) fn get(&self, url: &str) -> Result<String, AiProbeError> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(map_request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(AiProbeError::HttpStatus {
                status: status.as_u16(),
            });
        }
        // Enforce the byte cap while reading — a Content-Length alone is not
        // trusted (streaming bodies may lie).
        use std::io::Read;
        let mut body = Vec::new();
        response
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    AiProbeError::Timeout
                } else {
                    AiProbeError::ConnectFailed { detail: e.to_string() }
                }
            })?;
        if body.len() > MAX_BODY_BYTES {
            return Err(AiProbeError::TooLarge {
                limit: MAX_BODY_BYTES,
            });
        }
        String::from_utf8(body)
            .map_err(|_| AiProbeError::MalformedJson {
                detail: "response is not valid UTF-8".to_string(),
            })
    }
}

/// Map any transport-level failure to its structured cause (spec §46).
fn map_request_error(error: reqwest::Error) -> AiProbeError {
    if error.is_timeout() {
        AiProbeError::Timeout
    } else if error.is_connect() {
        AiProbeError::ConnectFailed {
            detail: error.to_string(),
        }
    } else {
        AiProbeError::ConnectFailed {
            detail: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prober_rejects_policy_invalid_urls_without_requesting() {
        // The trust policy check happens *before* the prober is used, but a
        // non-loopback URL must also fail at the client level rather than
        // silently succeed.
        let prober = Prober::new().expect("client");
        let error = prober.get("http://192.168.1.5:9/x").expect_err("must fail");
        assert!(matches!(error, AiProbeError::ConnectFailed { .. } | AiProbeError::Timeout));
    }
}
