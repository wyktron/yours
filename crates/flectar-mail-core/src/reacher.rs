//! Typed client for the local Reacher email-verification service.
//!
//! Reacher checks an address in three steps - syntax, MX records, and a live
//! SMTP probe - and answers with one of four confidence levels. Flectar Mail
//! runs it on this machine only (see `docs/reacher.md`) and treats the answer
//! as a hint for compose, so the client reports transport and server failures
//! as clear errors instead of pretending the address is unknown.

use crate::error::{CoreError, Result};
use crate::models::Settings;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The loopback endpoint installed for Flectar Mail.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8084";
const CHECK_PATH: &str = "/v0/check_email";
/// A Reacher answer is about 1 KiB; this only bounds a broken server.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// Where the Reacher service listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReacherConfig {
    pub url: String,
}

impl ReacherConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// Build from saved settings. Empty means the installer default, which is
    /// also what an existing settings blob gets through serde.
    pub fn from_settings(settings: &Settings) -> Self {
        let url = settings.reacher_url.trim();
        Self::new(if url.is_empty() { DEFAULT_URL } else { url })
    }

    fn endpoint(&self) -> String {
        let base = self.url.trim().trim_end_matches('/');
        let base = if base.is_empty() { DEFAULT_URL } else { base };
        format!("{base}{CHECK_PATH}")
    }
}

impl Default for ReacherConfig {
    fn default() -> Self {
        Self::new(DEFAULT_URL)
    }
}

/// Reacher's confidence that an address is deliverable.
///
/// These are exactly Reacher's `is_reachable` values. `Unknown` is not an
/// error: it means the probe could not decide, for example because the remote
/// server greylisted us or outbound port 25 is blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReacherVerdict {
    Safe,
    Invalid,
    Unknown,
    Risky,
}

impl ReacherVerdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Invalid => "invalid",
            Self::Unknown => "unknown",
            Self::Risky => "risky",
        }
    }

    /// Parse Reacher's wire value. A value Reacher did not define is schema
    /// drift and returns `None` rather than silently becoming `Unknown`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "safe" => Some(Self::Safe),
            "invalid" => Some(Self::Invalid),
            "unknown" => Some(Self::Unknown),
            "risky" => Some(Self::Risky),
            _ => None,
        }
    }
}

/// The SMTP probe error Reacher reports when it could not get a definitive
/// answer. Reacher still returns a verdict alongside it, usually `unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReacherSmtpError {
    /// Reacher's error type, such as `AsyncSmtpError`.
    pub kind: String,
    pub message: String,
}

/// Details behind a verdict.
///
/// Reacher's 0.11 schema has no `has_risk` field. An explicit one is honoured
/// when a deployment sends it, and a `risky` verdict always sets it; this is
/// what lets a caller explain a non-green answer without a second request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReacherReason {
    pub smtp_error: Option<ReacherSmtpError>,
    pub has_risk: bool,
}

/// A verdict plus the reason Reacher gave for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReacherCheck {
    pub verdict: ReacherVerdict,
    pub reason: ReacherReason,
}

/// Check one address through Reacher, discarding the reason details.
pub async fn check(cfg: &ReacherConfig, email: &str) -> Result<ReacherVerdict> {
    Ok(check_with_reason(cfg, email).await?.verdict)
}

/// Check one address and keep the reason Reacher returned.
pub async fn check_with_reason(cfg: &ReacherConfig, email: &str) -> Result<ReacherCheck> {
    let response = http_client()?
        .post(cfg.endpoint())
        .json(&CheckRequest { to_email: email })
        .send()
        .await
        .map_err(|error| request_error(&cfg.url, error))?;
    let status = response.status().as_u16();
    let body = crate::http_body::text(response, MAX_RESPONSE_BYTES, "Reacher").await?;
    parse_response(status, &body)
}

#[derive(Serialize)]
struct CheckRequest<'a> {
    to_email: &'a str,
}

fn http_client() -> Result<&'static reqwest::Client> {
    static HTTP: once_cell::sync::Lazy<std::result::Result<reqwest::Client, String>> =
        once_cell::sync::Lazy::new(|| {
            reqwest::Client::builder()
                .user_agent("flectar-mail-reacher/0.1")
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                // Reacher is on loopback. A proxy configured for the rest of
                // the app must never carry the address (and probe) elsewhere.
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| format!("Reacher HTTP client: {error}"))
        });
    HTTP.as_ref()
        .map_err(|message| CoreError::Network(message.clone()))
}

fn request_error(url: &str, error: reqwest::Error) -> CoreError {
    if error.is_timeout() {
        CoreError::Network(format!(
            "Reacher at {url} timed out after {}s",
            REQUEST_TIMEOUT.as_secs()
        ))
    } else if error.is_connect() {
        CoreError::Network(format!("Reacher at {url} is unreachable: {error}"))
    } else {
        CoreError::Network(format!("Reacher request to {url} failed: {error}"))
    }
}

/// Parse a Reacher HTTP answer. Kept separate from the request so every wire
/// case is testable without a listener.
fn parse_response(status: u16, body: &str) -> Result<ReacherCheck> {
    if !(200..300).contains(&status) {
        return Err(CoreError::Other(format!(
            "Reacher returned HTTP {status}: {}",
            body_excerpt(body)
        )));
    }

    let wire: WireResponse = serde_json::from_str(body)
        .map_err(|error| CoreError::Other(format!("Reacher returned malformed JSON: {error}")))?;
    if wire.is_reachable.trim().is_empty() {
        return Err(CoreError::Other(
            "Reacher response has no is_reachable".to_owned(),
        ));
    }
    let verdict = ReacherVerdict::parse(&wire.is_reachable).ok_or_else(|| {
        CoreError::Other(format!(
            "Reacher returned an unknown is_reachable value: {:?}",
            wire.is_reachable
        ))
    })?;

    let smtp = wire.smtp.unwrap_or_default();
    let smtp_error = smtp
        .error
        .clone()
        .map(ReacherSmtpError::from)
        .or_else(|| smtp.direct_error());
    let has_risk = wire
        .has_risk
        .or_else(|| wire.misc.and_then(|misc| misc.has_risk))
        .or(smtp.has_risk)
        .unwrap_or(false)
        || verdict == ReacherVerdict::Risky;

    Ok(ReacherCheck {
        verdict,
        reason: ReacherReason {
            smtp_error,
            has_risk,
        },
    })
}

#[derive(Debug, Default, Deserialize)]
struct WireResponse {
    #[serde(default)]
    is_reachable: String,
    #[serde(default)]
    smtp: Option<SmtpPayload>,
    #[serde(default)]
    misc: Option<MiscPayload>,
    /// Not in Reacher 0.11, but accepted so a deployment that adds it is not
    /// silently ignored.
    #[serde(default)]
    has_risk: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct MiscPayload {
    #[serde(default)]
    has_risk: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct SmtpPayload {
    #[serde(default)]
    error: Option<SmtpErrorPayload>,
    /// Reacher's schema may put a CoreError at `smtp` directly instead of in
    /// the usual `smtp.error` envelope; both shapes are accepted.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    has_risk: Option<bool>,
}

impl SmtpPayload {
    fn direct_error(&self) -> Option<ReacherSmtpError> {
        let kind = self.kind.as_deref()?.trim();
        if kind.is_empty() {
            return None;
        }
        Some(ReacherSmtpError {
            kind: kind.to_owned(),
            message: self.message.clone().unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum SmtpErrorPayload {
    Structured {
        #[serde(default, rename = "type")]
        kind: String,
        #[serde(default)]
        message: String,
    },
    Text(String),
}

impl From<SmtpErrorPayload> for ReacherSmtpError {
    fn from(error: SmtpErrorPayload) -> Self {
        match error {
            SmtpErrorPayload::Structured { kind, message } => Self {
                kind: if kind.trim().is_empty() {
                    "smtp".to_owned()
                } else {
                    kind
                },
                message,
            },
            SmtpErrorPayload::Text(message) => Self {
                kind: "smtp".to_owned(),
                message,
            },
        }
    }
}

/// A short, single-line excerpt of an error body for the caller's message.
fn body_excerpt(body: &str) -> String {
    const MAX_CHARS: usize = 240;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty response".to_owned();
    }
    let mut chars = trimmed.chars();
    let mut excerpt: String = chars
        .by_ref()
        .take(MAX_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if chars.next().is_some() {
        excerpt.push('…');
    }
    excerpt
}

impl crate::Core {
    /// Check `email` with the Reacher service named in Settings.
    ///
    /// Settings are read per call so turning verification off takes effect
    /// without a restart. A disabled service answers `Unknown` instead of an
    /// error: verification is a hint, and writing a message must keep working
    /// when the local service is off.
    pub async fn reacher_check(&self, email: &str) -> Result<ReacherVerdict> {
        let settings = self.get_settings().await?;
        if !settings.reacher_enabled {
            return Ok(ReacherVerdict::Unknown);
        }
        let cfg = ReacherConfig::from_settings(&settings);
        check(&cfg, email).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE: &str = r#"{
        "input": "alice@example.com",
        "is_reachable": "safe",
        "misc": {"is_disposable": false, "is_role_account": false},
        "mx": {"accepts_mail": true, "records": ["mx.example.com."]},
        "smtp": {"can_connect_smtp": true, "has_full_inbox": false,
                 "is_catch_all": false, "is_deliverable": true,
                 "is_disabled": false},
        "syntax": {"is_valid_syntax": true}
    }"#;

    const RISKY: &str = r#"{
        "is_reachable": "risky",
        "misc": {"is_disposable": false, "is_role_account": true},
        "smtp": {"can_connect_smtp": true, "has_full_inbox": false,
                 "is_catch_all": false, "is_deliverable": true,
                 "is_disabled": false}
    }"#;

    const INVALID: &str = r#"{
        "is_reachable": "invalid",
        "mx": {"accepts_mail": true, "records": ["mx.example.com."]},
        "smtp": {"can_connect_smtp": true, "has_full_inbox": false,
                 "is_catch_all": false, "is_deliverable": false,
                 "is_disabled": false}
    }"#;

    const UNKNOWN_GREYLISTED: &str = r#"{
        "input": "contact@victorgirbu.com",
        "is_reachable": "unknown",
        "misc": {"is_disposable": false, "is_role_account": true},
        "mx": {"accepts_mail": true, "records": ["mailserver.purelymail.com."]},
        "smtp": {"error": {"type": "AsyncSmtpError",
                  "message": "transient: 4.7.1 Try again later (greylist)"}},
        "syntax": {"is_valid_syntax": true}
    }"#;

    fn parse(body: &str) -> ReacherCheck {
        parse_response(200, body).expect("valid Reacher response")
    }

    #[test]
    fn safe_response_is_safe() {
        let check = parse(SAFE);
        assert_eq!(check.verdict, ReacherVerdict::Safe);
        assert!(!check.reason.has_risk);
        assert_eq!(check.reason.smtp_error, None);
    }

    #[test]
    fn risky_response_is_risky_and_has_risk() {
        let check = parse(RISKY);
        assert_eq!(check.verdict, ReacherVerdict::Risky);
        assert!(check.reason.has_risk);
    }

    #[test]
    fn invalid_response_is_invalid() {
        let check = parse(INVALID);
        assert_eq!(check.verdict, ReacherVerdict::Invalid);
        assert_eq!(check.reason.smtp_error, None);
    }

    #[test]
    fn unknown_response_keeps_the_smtp_error() {
        let check = parse(UNKNOWN_GREYLISTED);
        assert_eq!(check.verdict, ReacherVerdict::Unknown);
        assert_eq!(
            check.reason.smtp_error,
            Some(ReacherSmtpError {
                kind: "AsyncSmtpError".to_owned(),
                message: "transient: 4.7.1 Try again later (greylist)".to_owned(),
            })
        );
    }

    #[test]
    fn a_bare_core_error_in_smtp_is_kept() {
        let check = parse(
            r#"{"is_reachable": "unknown",
                "smtp": {"type": "SmtpError", "message": "connection refused"}}"#,
        );
        assert_eq!(
            check.reason.smtp_error,
            Some(ReacherSmtpError {
                kind: "SmtpError".to_owned(),
                message: "connection refused".to_owned(),
            })
        );
    }

    #[test]
    fn explicit_has_risk_is_honoured() {
        let check = parse(r#"{"is_reachable": "safe", "has_risk": true}"#);
        assert_eq!(check.verdict, ReacherVerdict::Safe);
        assert!(check.reason.has_risk);
    }

    #[test]
    fn malformed_json_is_a_clear_error() {
        let error = parse_response(200, "{").unwrap_err().to_string();
        assert!(error.contains("malformed JSON"), "{error}");
    }

    #[test]
    fn http_error_is_a_clear_error() {
        let error = parse_response(500, "backend exploded")
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 500"), "{error}");
        assert!(error.contains("backend exploded"), "{error}");
    }

    #[test]
    fn missing_or_unknown_verdict_is_an_error() {
        let missing = parse_response(200, "{}").unwrap_err().to_string();
        assert!(missing.contains("is_reachable"), "{missing}");
        let unknown = parse_response(200, r#"{"is_reachable": "maybe"}"#)
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("unknown is_reachable value"), "{unknown}");
    }

    #[test]
    fn endpoint_appends_the_check_path_once() {
        assert_eq!(
            ReacherConfig::new("http://127.0.0.1:8084/").endpoint(),
            "http://127.0.0.1:8084/v0/check_email"
        );
        assert_eq!(
            ReacherConfig::new("").endpoint(),
            "http://127.0.0.1:8084/v0/check_email"
        );
    }

    #[test]
    fn settings_default_to_the_installed_service_and_use_camel_case() {
        let settings = Settings::default();
        assert_eq!(settings.reacher_url, DEFAULT_URL);
        assert!(settings.reacher_enabled);
        let json = serde_json::to_value(settings).unwrap();
        assert_eq!(json["reacherUrl"].as_str(), Some(DEFAULT_URL));
        assert_eq!(json["reacherEnabled"], true);
    }

    #[test]
    fn an_empty_setting_still_reaches_the_installed_service() {
        let settings = Settings {
            reacher_url: "  ".into(),
            ..Settings::default()
        };
        assert_eq!(
            ReacherConfig::from_settings(&settings).endpoint(),
            "http://127.0.0.1:8084/v0/check_email"
        );
    }
}
