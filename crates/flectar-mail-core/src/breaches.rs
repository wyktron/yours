//! Known data breaches for a sender's address.
//!
//! The default provider is XposedOrNot's keyless Community Edition endpoint:
//! only the address is sent, and it allows two requests per second per IP.
//! When a paid Have I Been Pwned key is stored, the same scan uses HIBP's
//! `breachedaccount` endpoint instead; HIBP allows roughly one request every
//! 1.5 seconds. Every request is paced, a 429 is honoured with a backoff
//! instead of a retry storm, and mail never waits for this: the core runs the
//! checks on its runtime after the list has the mail.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// The breach lookup endpoint. The address is appended, percent-encoded.
pub const API_BASE: &str = "https://haveibeenpwned.com/api/v3/breachedaccount";
/// HIBP asks clients to identify themselves.
pub const USER_AGENT: &str = "Yours-Mail-BreachCheck/1.0";
/// Where the account's paid HIBP key is created; Settings links to it. Kept
/// here so the key page has one owner on the Rust side too.
pub const KEY_URL: &str = "https://haveibeenpwned.com/API/Key";
/// The keyless community provider: XposedOrNot's breach analytics endpoint.
/// It needs no account or API key; the address is the only argument.
pub const XON_API_BASE: &str = "https://api.xposedornot.com/v1/breach-analytics";
/// XON's Community Edition allows two requests per second per IP.
pub const XON_MIN_INTERVAL: Duration = Duration::from_millis(500);
/// What a 429 waits when XON sends no usable Retry-After.
pub const XON_DEFAULT_BACKOFF: Duration = Duration::from_secs(1);
/// A Retry-After longer than this drops the check instead of stalling mail.
pub const XON_MAX_BACKOFF: Duration = Duration::from_secs(10);
/// A lookup that takes longer than this is treated as offline.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// HIBP asks for at most one request per 1.5 seconds.
pub const MIN_INTERVAL: Duration = Duration::from_millis(1500);
/// How long a 429 waits when the server sends no usable Retry-After.
pub const DEFAULT_BACKOFF: Duration = Duration::from_secs(2);
/// A Retry-After longer than this is not followed; the check is dropped for
/// now instead of holding the mail screen for minutes.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Extra attempts after the first 429.
pub const MAX_RETRIES: usize = 2;

/// One breach an address appears in, as HIBP describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Breach {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub domain: String,
    /// HIBP's ISO date of the breach, e.g. "2019-01-07T00:00:00Z".
    #[serde(default)]
    pub breach_date: String,
    #[serde(default)]
    pub pwn_count: i64,
    #[serde(default)]
    pub data_classes: Vec<String>,
}

/// What a lookup found. A 404 from HIBP means "not in any known breach".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Breaches(Vec<Breach>),
    None,
}

/// Why a lookup did not produce a verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupError {
    /// HIBP throttled the request. `retry_after` is its Retry-After in
    /// seconds, when it sent one.
    RateLimited { retry_after: Option<u64> },
    /// HIBP rejected the API key (HTTP 401/403).
    Unauthorized { status: u16 },
    /// Any other HTTP status, with a short body excerpt for the log.
    Http { status: u16, body: String },
    /// The response did not parse as a breach list.
    Invalid(String),
    /// The request never reached HIBP.
    Offline(String),
}

impl LookupError {
    pub fn message(&self) -> String {
        match self {
            Self::RateLimited { retry_after } => match retry_after {
                Some(seconds) => format!("HIBP rate limit (HTTP 429, retry after {seconds}s)"),
                None => "HIBP rate limit (HTTP 429)".to_owned(),
            },
            Self::Unauthorized { status } => {
                format!("HIBP rejected the API key (HTTP {status})")
            }
            Self::Http { status, body } => format!("HIBP answered HTTP {status}: {body}"),
            Self::Invalid(reason) => format!("HIBP sent an unreadable answer: {reason}"),
            Self::Offline(reason) => format!("HIBP is unreachable: {reason}"),
        }
    }
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl From<LookupError> for CoreError {
    fn from(error: LookupError) -> Self {
        CoreError::Other(error.message())
    }
}

/// The last request's start, shared by every check on this runtime, so one
/// address cannot make the next one violate HIBP's pacing.
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// The last XposedOrNot request's start, kept apart from HIBP's pacing: the
/// community endpoint allows two requests per second per IP.
static LAST_XON_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// The last hard failure a sender check hit, for the services card and the
/// one-time error hint. Cleared by the next successful lookup, so it reports
/// a current condition rather than a permanent scar.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// The current hard failure, when the last lookup pass could not reach its
/// provider. A rate limit is soft and never lands here.
pub fn last_error() -> Option<String> {
    LAST_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Remember why a lookup pass failed, for the services card and the one-time
/// hint. The caller shows both only after hint_due.
pub fn note_error(message: &str) {
    *LAST_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message.to_owned());
}

/// A lookup answered: there is no current failure to report.
pub fn clear_error() {
    *LAST_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Whether a sender address is worth asking HIBP about. Local addresses and
/// anything that is not an address are not.
pub fn should_check_address(address: &str) -> bool {
    let address = address.trim();
    let Some((local, domain)) = address.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.contains("..")
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !address.chars().any(char::is_whitespace)
}

/// Whether this sender should be looked up now: the check is on, the address
/// is a real one, and no fresh verdict is cached.
pub fn should_check_sender(enabled: bool, address: &str, cached: bool) -> bool {
    enabled && !cached && should_check_address(address)
}

/// Which service answers the breach lookup. XposedOrNot is keyless and is the
/// default; a stored HIBP key switches the scan to the paid HIBP endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    XposedOrNot,
    HaveIBeenPwned,
}

impl Provider {
    /// The provider name for logs and the services card.
    pub const fn label(self) -> &'static str {
        match self {
            Self::XposedOrNot => "XposedOrNot",
            Self::HaveIBeenPwned => "Have I Been Pwned",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XposedOrNot => "xposedornot",
            Self::HaveIBeenPwned => "haveibeenpwned",
        }
    }
}

/// A stored HIBP key is the opt-in for the paid provider; without one the
/// scan uses the keyless XposedOrNot API, so the feature always works.
pub const fn select_provider(hibp_key_present: bool) -> Provider {
    if hibp_key_present {
        Provider::HaveIBeenPwned
    } else {
        Provider::XposedOrNot
    }
}

/// Whether the one-time hard-error hint is still owed. It is raised only when
/// a lookup could not reach the provider, never for a missing key.
pub fn hint_due(enabled: bool, hint_shown: bool) -> bool {
    enabled && !hint_shown
}

/// What one sender-check pass did, for the shell's status line and hint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    /// How many senders this pass asked the provider about.
    pub checked: usize,
    /// The provider this pass used: HIBP when a key is stored, else XON.
    pub provider: Provider,
    /// Why the pass stopped early, when it could not reach its provider. A
    /// rate limit is soft and leaves this empty.
    pub hard_error: Option<String>,
}

/// The URL HIBP's breach lookup answers on for this address.
pub fn account_url(address: &str) -> String {
    let encoded: String = percent_encoding::utf8_percent_encode(
        address.trim(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    format!("{API_BASE}/{encoded}?truncateResponse=false")
}

/// How long to wait before retrying, from a response status and HIBP's
/// Retry-After header. `None` means the status is not a rate limit.
pub fn backoff_for(status: u16, retry_after: Option<u64>) -> Option<Duration> {
    if status != 429 {
        return None;
    }
    let asked = retry_after
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_BACKOFF);
    Some(asked.clamp(DEFAULT_BACKOFF.min(MIN_INTERVAL), MAX_BACKOFF))
}

/// Whether a request may start now, or how long the pacing still asks for.
pub fn pacing_wait(last: Option<Instant>, now: Instant) -> Option<Duration> {
    let last = last?;
    let elapsed = now.saturating_duration_since(last);
    (elapsed < MIN_INTERVAL).then(|| MIN_INTERVAL - elapsed)
}

/// A breach list from HIBP's JSON body. An empty array is "no breaches".
pub fn parse_breaches(body: &str) -> std::result::Result<Vec<Breach>, LookupError> {
    serde_json::from_str::<Vec<Breach>>(body.trim())
        .map_err(|error| LookupError::Invalid(error.to_string()))
}

/// The verdict for a response, without any network. 200 parses the list,
/// 404 is "no breaches", 429 and 401 are errors the caller handles.
pub fn outcome(
    status: u16,
    retry_after: Option<u64>,
    body: &str,
) -> std::result::Result<Lookup, LookupError> {
    match status {
        200 => {
            let breaches = parse_breaches(body)?;
            if breaches.is_empty() {
                Ok(Lookup::None)
            } else {
                Ok(Lookup::Breaches(breaches))
            }
        }
        404 => Ok(Lookup::None),
        429 => Err(LookupError::RateLimited { retry_after }),
        401 | 403 => Err(LookupError::Unauthorized { status }),
        status => Err(LookupError::Http {
            status,
            body: body.trim().chars().take(200).collect(),
        }),
    }
}

async fn pace() {
    let last = *LAST_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(wait) = pacing_wait(last, Instant::now()) {
        tokio::time::sleep(wait).await;
    }
    *LAST_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
}

fn retry_after_seconds(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Ask HIBP whether this address appears in a known breach. A 429 backs off
/// up to `MAX_BACKOFF` and retries at most `MAX_RETRIES` times, honouring
/// HIBP's pacing between every attempt.
pub async fn lookup(address: &str, api_key: &str) -> std::result::Result<Lookup, LookupError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(LookupError::Unauthorized { status: 401 });
    }
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| LookupError::Offline(error.to_string()))?;
    let url = account_url(address);
    let mut attempt = 0usize;
    loop {
        pace().await;
        let response = client
            .get(&url)
            .header("hibp-api-key", api_key)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return Err(LookupError::Offline(error.to_string())),
        };
        let status = response.status().as_u16();
        let retry_after = retry_after_seconds(&response);
        let body = response.text().await.unwrap_or_default();
        match outcome(status, retry_after, &body) {
            Ok(found) => return Ok(found),
            Err(LookupError::RateLimited { retry_after }) if attempt < MAX_RETRIES => {
                let wait = backoff_for(status, retry_after).unwrap_or(DEFAULT_BACKOFF);
                let wait = wait.min(MAX_BACKOFF);
                tracing::warn!(address, wait_ms = wait.as_millis(), "HIBP rate limited; backing off");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// How long a XposedOrNot 429 waits: Retry-After when it sends a usable one,
/// otherwise one second, clamped so a hostile header cannot stall a pass.
pub fn xon_backoff_for(status: u16, retry_after: Option<u64>) -> Option<Duration> {
    if status != 429 {
        return None;
    }
    let asked = retry_after
        .map(Duration::from_secs)
        .unwrap_or(XON_DEFAULT_BACKOFF);
    Some(asked.clamp(XON_MIN_INTERVAL, XON_MAX_BACKOFF))
}

/// The XposedOrNot URL for this address. The address is the only thing sent.
pub fn xon_url(address: &str) -> String {
    let encoded: String = percent_encoding::utf8_percent_encode(
        address.trim(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    format!("{XON_API_BASE}?email={encoded}")
}

fn text_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

fn number_field(value: &serde_json::Value, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|name| {
        let field = value.get(*name)?;
        field
            .as_i64()
            .or_else(|| field.as_str().and_then(|text| text.parse().ok()))
    })
}

/// The breach list in XposedOrNot's breach-analytics body. The schema is read
/// defensively: a missing or null ExposedBreaches means "no breaches", and an
/// unfamiliar field leaves that field empty rather than failing the address.
/// Only the fields the skull popup renders are kept.
pub fn parse_xon(body: &str) -> std::result::Result<Lookup, LookupError> {
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| LookupError::Invalid(error.to_string()))?;
    // XON reports its own refusals inside a 200-shaped body as {detail: ...}.
    if let Some(detail) = value.get("detail") {
        let message = detail
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .or_else(|| detail.get("error").and_then(serde_json::Value::as_str))
            .unwrap_or("XposedOrNot refused the lookup");
        return Err(LookupError::Invalid(message.to_owned()));
    }
    let Some(details) = value
        .get("ExposedBreaches")
        .and_then(|exposed| exposed.get("breaches_details"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Lookup::None);
    };
    let mut breaches: Vec<Breach> = Vec::new();
    for entry in details {
        let title = text_field(entry, &["breach", "name", "title"]).unwrap_or_default();
        let domain = text_field(entry, &["domain"]).unwrap_or_default();
        if title.is_empty() && domain.is_empty() {
            continue;
        }
        let date = text_field(entry, &["xposed_date", "breach_date", "breachDate"])
            .or_else(|| {
                text_field(entry, &["added"]).map(|added| added.chars().take(10).collect())
            })
            .unwrap_or_default();
        let data_classes = text_field(entry, &["xposed_data", "data_classes"])
            .map(|classes| {
                classes
                    .split(';')
                    .map(str::trim)
                    .filter(|class| !class.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let breach = Breach {
            name: if title.is_empty() {
                domain.clone()
            } else {
                title.clone()
            },
            title,
            domain,
            breach_date: date,
            pwn_count: number_field(entry, &["xposed_records", "pwn_count", "PwnCount"])
                .unwrap_or(0),
            data_classes,
        };
        // The endpoint sometimes lists one breach twice; the popup should not.
        if !breaches
            .iter()
            .any(|seen| seen.name == breach.name && seen.domain == breach.domain)
        {
            breaches.push(breach);
        }
    }
    if breaches.is_empty() {
        Ok(Lookup::None)
    } else {
        Ok(Lookup::Breaches(breaches))
    }
}

/// The verdict for an XposedOrNot response without any network. 200 parses the
/// analytics body, 404 is "no breaches", 429 and 401 are errors the caller
/// handles.
pub fn outcome_xon(
    status: u16,
    retry_after: Option<u64>,
    body: &str,
) -> std::result::Result<Lookup, LookupError> {
    match status {
        200 => parse_xon(body),
        404 => Ok(Lookup::None),
        429 => Err(LookupError::RateLimited { retry_after }),
        401 | 403 => Err(LookupError::Unauthorized { status }),
        status => Err(LookupError::Http {
            status,
            body: body.trim().chars().take(200).collect(),
        }),
    }
}

async fn pace_xon() {
    let last =
        *LAST_XON_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(last) = last {
        let elapsed = last.elapsed();
        if elapsed < XON_MIN_INTERVAL {
            tokio::time::sleep(XON_MIN_INTERVAL - elapsed).await;
        }
    }
    *LAST_XON_REQUEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
}

/// Ask XposedOrNot (Community Edition, no key) whether this address appears in
/// a known breach. Two requests per second are allowed per IP, so requests are
/// paced; a 429 honours Retry-After and retries at most MAX_RETRIES times.
pub async fn lookup_xon(address: &str) -> std::result::Result<Lookup, LookupError> {
    if !should_check_address(address) {
        return Err(LookupError::Invalid("not a sender address".to_owned()));
    }
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| LookupError::Offline(error.to_string()))?;
    let url = xon_url(address);
    let mut attempt = 0usize;
    loop {
        pace_xon().await;
        let response = client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return Err(LookupError::Offline(error.to_string())),
        };
        let status = response.status().as_u16();
        let retry_after = retry_after_seconds(&response);
        let body = response.text().await.unwrap_or_default();
        match outcome_xon(status, retry_after, &body) {
            Ok(found) => return Ok(found),
            Err(LookupError::RateLimited { retry_after }) if attempt < MAX_RETRIES => {
                let wait =
                    xon_backoff_for(status, retry_after).unwrap_or(XON_DEFAULT_BACKOFF);
                tracing::warn!(address, wait_ms = wait.as_millis(), "XposedOrNot rate limited; backing off");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"Name":"Adobe","Title":"Adobe","Domain":"adobe.com","BreachDate":"2013-10-04",
       "PwnCount":152445165,"DataClasses":["Email addresses","Password hints"],
       "IsVerified":true,"IsSensitive":false},
      {"Name":"LinkedIn","Title":"LinkedIn","Domain":"linkedin.com","BreachDate":"2012-05-05",
       "PwnCount":117430911,"DataClasses":["Email addresses","Passwords"],
       "IsVerified":true,"IsSensitive":false}
    ]"#;

    #[test]
    fn parses_the_breach_list_and_keeps_the_fields_the_popup_shows() {
        let Lookup::Breaches(breaches) = outcome(200, None, SAMPLE).unwrap() else {
            panic!("expected breaches");
        };
        assert_eq!(breaches.len(), 2);
        assert_eq!(breaches[0].name, "Adobe");
        assert_eq!(breaches[0].title, "Adobe");
        assert_eq!(breaches[0].domain, "adobe.com");
        assert_eq!(breaches[0].breach_date, "2013-10-04");
        assert_eq!(breaches[0].pwn_count, 152445165);
        assert_eq!(
            breaches[0].data_classes,
            vec!["Email addresses".to_owned(), "Password hints".to_owned()]
        );
        assert_eq!(breaches[1].name, "LinkedIn");
    }

    #[test]
    fn a_404_means_no_breaches() {
        assert_eq!(outcome(404, None, "").unwrap(), Lookup::None);
        assert_eq!(outcome(404, None, "Not Found").unwrap(), Lookup::None);
    }

    #[test]
    fn an_empty_array_also_means_no_breaches() {
        assert_eq!(outcome(200, None, "[]").unwrap(), Lookup::None);
    }

    #[test]
    fn rate_limits_back_off_instead_of_hammering() {
        assert_eq!(backoff_for(200, None), None);
        assert_eq!(backoff_for(404, None), None);
        assert_eq!(backoff_for(429, None), Some(DEFAULT_BACKOFF));
        assert_eq!(backoff_for(429, Some(3)), Some(Duration::from_secs(3)));
        // A server asking for a very short wait is still paced.
        assert!(backoff_for(429, Some(0)).unwrap() >= Duration::from_millis(1500));
        // A server asking for a very long wait is not followed off a cliff.
        assert_eq!(backoff_for(429, Some(3600)), Some(MAX_BACKOFF));
        assert_eq!(
            outcome(429, Some(7), ""),
            Err(LookupError::RateLimited {
                retry_after: Some(7)
            })
        );
    }

    #[test]
    fn rejects_a_bad_key_as_unauthorized() {
        assert_eq!(
            outcome(401, None, "{\"message\":\"Access denied\"}"),
            Err(LookupError::Unauthorized { status: 401 })
        );
        assert_eq!(
            outcome(403, None, ""),
            Err(LookupError::Unauthorized { status: 403 })
        );
    }

    #[test]
    fn paces_requests_1_5_seconds_apart() {
        let start = Instant::now();
        assert_eq!(pacing_wait(None, start), None);
        assert_eq!(pacing_wait(Some(start), start), Some(MIN_INTERVAL));
        assert_eq!(
            pacing_wait(Some(start), start + Duration::from_millis(500)),
            Some(Duration::from_millis(1000))
        );
        assert_eq!(pacing_wait(Some(start), start + MIN_INTERVAL), None);
        assert_eq!(
            pacing_wait(Some(start), start + Duration::from_secs(9)),
            None
        );
    }

    #[test]
    fn only_real_addresses_are_checked() {
        assert!(should_check_address("contact@victorgirbu.com"));
        assert!(should_check_address("  Person@Example.CO.UK "));
        assert!(!should_check_address(""));
        assert!(!should_check_address("no-at-sign"));
        assert!(!should_check_address("@example.com"));
        assert!(!should_check_address("contact@"));
        assert!(!should_check_address("contact@localhost"));
        assert!(!should_check_address("a b@example.com"));
        assert!(!should_check_address("contact@example..com"));
    }

    #[test]
    fn a_stored_key_switches_the_provider_and_no_key_uses_xposedornot() {
        assert_eq!(select_provider(false), Provider::XposedOrNot);
        assert_eq!(select_provider(true), Provider::HaveIBeenPwned);
        assert_eq!(Provider::XposedOrNot.label(), "XposedOrNot");
        assert_eq!(Provider::HaveIBeenPwned.as_str(), "haveibeenpwned");
    }

    #[test]
    fn the_hard_error_hint_is_owed_once_while_the_scan_is_on() {
        assert!(hint_due(true, false));
        // Shown (or dismissed) already: never raise it again.
        assert!(!hint_due(true, true));
        // A switched-off scan never raises a hint.
        assert!(!hint_due(false, false));
        assert!(!hint_due(false, true));
    }

    /// A real Community Edition answer, captured on 2026-11-28, kept in
    /// testdata and trimmed to three breaches. This is the schema the parser
    /// must keep working against, not a hand-written imitation of it.
    const XON_SAMPLE: &str = include_str!("../testdata/xon-breach-analytics.json");

    /// The real answer for an address with no known breach: a 200 with null
    /// ExposedBreaches, not a 404. Captured 2026-11-28.
    const XON_CLEAN: &str = include_str!("../testdata/xon-clean.json");

    #[test]
    fn parses_a_real_xon_answer_into_the_fields_the_popup_shows() {
        let Lookup::Breaches(breaches) = parse_xon(XON_SAMPLE).unwrap() else {
            panic!("the captured answer has breaches");
        };
        assert_eq!(breaches.len(), 3);
        let neogen = &breaches[0];
        assert_eq!(neogen.name, "Neogen");
        assert_eq!(neogen.title, "Neogen");
        assert_eq!(neogen.domain, "neogen.com");
        assert_eq!(neogen.breach_date, "2026");
        assert_eq!(neogen.pwn_count, 445_899);
        assert!(neogen.data_classes.contains(&"Email addresses".to_owned()));
        assert!(neogen.data_classes.contains(&"Job titles".to_owned()));
        // Every mapped breach can name itself in the popup.
        for breach in &breaches {
            assert!(!breach.title.is_empty(), "{breach:?}");
            assert!(!breach.domain.is_empty(), "{breach:?}");
        }
    }

    #[test]
    fn an_address_outside_every_xon_breach_is_a_clean_answer() {
        assert_eq!(parse_xon(XON_CLEAN).unwrap(), Lookup::None);
        assert_eq!(outcome_xon(200, None, XON_CLEAN).unwrap(), Lookup::None);
        // A 404 is treated the same way, defensively.
        assert_eq!(outcome_xon(404, None, "").unwrap(), Lookup::None);
    }

    #[test]
    fn a_xon_refusal_body_is_not_mistaken_for_a_clean_address() {
        let body = r#"{"detail":{"error":"Rate limit exceeded","detail":"Rate limit exceeded","retry_after":1}}"#;
        let error = parse_xon(body).unwrap_err();
        assert!(matches!(error, LookupError::Invalid(_)), "{error:?}");
    }

    #[test]
    fn xon_rate_limits_back_off_within_its_own_bounds() {
        assert_eq!(xon_backoff_for(200, None), None);
        assert_eq!(xon_backoff_for(429, None), Some(XON_DEFAULT_BACKOFF));
        assert_eq!(xon_backoff_for(429, Some(3)), Some(Duration::from_secs(3)));
        // Never faster than its two-per-second allowance.
        assert_eq!(
            xon_backoff_for(429, Some(0)),
            Some(XON_MIN_INTERVAL)
        );
        // A very long Retry-After drops the check rather than stalling it.
        assert_eq!(
            xon_backoff_for(429, Some(3600)),
            Some(XON_MAX_BACKOFF)
        );
        assert_eq!(
            outcome_xon(429, Some(1), ""),
            Err(LookupError::RateLimited { retry_after: Some(1) })
        );
    }

    #[test]
    fn the_address_is_the_only_thing_the_xon_url_carries() {
        assert_eq!(
            xon_url("first.last+tag@example.com"),
            "https://api.xposedornot.com/v1/breach-analytics?email=first%2Elast%2Btag%40example%2Ecom"
        );
    }

    #[test]
    fn only_uncached_senders_are_checked() {
        assert!(should_check_sender(true, "person@example.com", false));
        // A fresh verdict, including a cached 404, is not asked again.
        assert!(!should_check_sender(true, "person@example.com", true));
        // The Settings switch turns the whole check off.
        assert!(!should_check_sender(false, "person@example.com", false));
        // Addresses HIBP cannot answer for are never checked.
        assert!(!should_check_sender(true, "", false));
        assert!(!should_check_sender(true, "not-an-address", false));
    }

    #[test]
    fn the_url_percent_encodes_the_address() {
        assert_eq!(
            account_url("first.last+tag@example.com"),
            "https://haveibeenpwned.com/api/v3/breachedaccount/first%2Elast%2Btag%40example%2Ecom?truncateResponse=false"
        );
    }
}
