//! Capability probe for the optional services Yours can use.
//!
//! Address checks, the AI features and read-aloud all depend on software the
//! app does not install: Reacher, an OpenAI-compatible model endpoint, and a
//! speech engine. On a fresh machine none of them may be present, so the shell
//! asks once, cheaply and off the database lock path, and tells the UI what is
//! actually there instead of failing later at click time.
//!
//! The probe only proves that a service answers. It never sends mail content,
//! an address, or a model prompt. Every request is bounded by a short timeout
//! and a redirect policy that does not follow a broken service elsewhere.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::models::Settings;
use crate::tts::{TtsConfig, TtsEngine};

/// A probe gives up quickly: it runs at startup and from Settings, and a
/// missing service must not delay either.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1_200);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DETAIL_CHARS: usize = 180;

/// How one optional service looks right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// The service answered.
    Connected,
    /// It is switched off or no address is set.
    NotConfigured,
    /// It is configured but did not answer.
    Unreachable,
}

impl ServiceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::NotConfigured => "not-configured",
            Self::Unreachable => "unreachable",
        }
    }
}

/// A service's state plus one short line the UI may show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub state: ServiceState,
    pub detail: String,
}

impl ServiceStatus {
    fn connected(detail: impl Into<String>) -> Self {
        Self {
            state: ServiceState::Connected,
            detail: detail.into(),
        }
    }

    fn not_configured(detail: impl Into<String>) -> Self {
        Self {
            state: ServiceState::NotConfigured,
            detail: detail.into(),
        }
    }

    fn unreachable(detail: impl Into<String>) -> Self {
        Self {
            state: ServiceState::Unreachable,
            detail: detail.into(),
        }
    }
}

/// One engine's availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineAvailability {
    pub engine: TtsEngine,
    pub available: bool,
}

/// What read-aloud can use on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechStatus {
    pub state: ServiceState,
    /// The engine chosen in Settings.
    pub selected: TtsEngine,
    pub selected_available: bool,
    /// Any engine that can speak, in Settings order.
    pub engines: Vec<EngineAvailability>,
    /// The engine read-aloud falls back to, and the voice for it. eSpeak NG is
    /// deliberately last: it is the always-present safety net, not a
    /// replacement for a better voice when one is installed.
    pub fallback: Option<(TtsEngine, String)>,
}

impl SpeechStatus {
    /// True when at least one engine can speak, so a missing configured
    /// engine degrades to another voice rather than to silence.
    pub fn any_available(&self) -> bool {
        self.engines.iter().any(|engine| engine.available)
    }

    /// The engine read-aloud should actually use: the configured one when it
    /// is there, otherwise the fallback.
    pub fn speaking_engine(&self) -> Option<TtsEngine> {
        if self.selected_available {
            Some(self.selected)
        } else {
            self.fallback.as_ref().map(|(engine, _)| *engine)
        }
    }
}

/// Everything the first-run guide and the Settings card need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesReport {
    pub reacher: ServiceStatus,
    /// The Settings switch, independent of whether the service answers.
    pub reacher_enabled: bool,
    pub ai: ServiceStatus,
    pub speech: SpeechStatus,
}

/// Probe all three services. Settings decide what is configured; nothing here
/// changes stored state.
pub async fn probe(
    settings: &Settings,
    ai_api_key: &str,
    tts_config: &TtsConfig,
) -> ServicesReport {
    let reacher = if settings.reacher_enabled {
        probe_reacher(&settings.reacher_url).await
    } else {
        ServiceStatus::not_configured("Address checks are turned off.")
    };
    let (ai, speech) = tokio::join!(
        probe_ai(&settings.ai_base_url, ai_api_key),
        probe_speech(tts_config)
    );
    ServicesReport {
        reacher,
        reacher_enabled: settings.reacher_enabled,
        ai,
        speech,
    }
}

/// GET a Reacher base URL. Any HTTP answer proves the service is listening;
/// connecting is the question, not the status code.
pub async fn probe_reacher(url: &str) -> ServiceStatus {
    let base = url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        crate::reacher::DEFAULT_URL
    } else {
        base
    };
    let client = match probe_client(true) {
        Ok(client) => client,
        Err(error) => return ServiceStatus::unreachable(error),
    };
    match client.get(base).send().await {
        Ok(response) if !response.status().is_server_error() => {
            ServiceStatus::connected(format!("Reacher answered (HTTP {}).", response.status()))
        }
        Ok(response) => {
            ServiceStatus::unreachable(format!("Reacher answered HTTP {}.", response.status()))
        }
        Err(error) => ServiceStatus::unreachable(probe_error("Reacher", error)),
    }
}

/// GET the OpenAI-compatible models list. A 401/403 means the endpoint is
/// there but needs a key, which the UI calls "not configured" rather than
/// pretending the network failed.
pub async fn probe_ai(base_url: &str, api_key: &str) -> ServiceStatus {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return ServiceStatus::not_configured("No AI endpoint is set.");
    }
    let client = match probe_client(false) {
        Ok(client) => client,
        Err(error) => return ServiceStatus::unreachable(error),
    };
    let mut request = client
        .get(format!("{base}/models"))
        .header(reqwest::header::ACCEPT, "application/json");
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => ServiceStatus::connected(format!(
            "The AI endpoint answered (HTTP {}).",
            response.status()
        )),
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => {
            ServiceStatus::not_configured("The AI endpoint requires an API key.")
        }
        Ok(response) => ServiceStatus::unreachable(format!(
            "The AI endpoint answered HTTP {}.",
            response.status()
        )),
        Err(error) => ServiceStatus::unreachable(probe_error("AI endpoint", error)),
    }
}

/// Detect every speech engine without synthesizing anything: a process on the
/// PATH, a Piper model, a listening Kokoro server, or a configured online
/// service.
pub async fn probe_speech(config: &TtsConfig) -> SpeechStatus {
    let kokoro = kokoro_available(&config.kokoro_url).await;
    let piper_voice = piper_voice(&config.voices_dir);
    let piper = piper_voice.is_some() && binary_on_path("piper").is_some();
    let espeak = binary_on_path("espeak-ng").is_some();
    let edge = binary_on_path("edge-tts").is_some();
    let elevenlabs = !config.api_key.trim().is_empty();
    let custom = !config.endpoint.trim().is_empty();
    let engines = vec![
        EngineAvailability {
            engine: TtsEngine::Kokoro,
            available: kokoro,
        },
        EngineAvailability {
            engine: TtsEngine::Piper,
            available: piper,
        },
        EngineAvailability {
            engine: TtsEngine::Espeak,
            available: espeak,
        },
        EngineAvailability {
            engine: TtsEngine::Edge,
            available: edge,
        },
        EngineAvailability {
            engine: TtsEngine::ElevenLabs,
            available: elevenlabs,
        },
        EngineAvailability {
            engine: TtsEngine::Custom,
            available: custom,
        },
    ];
    decide_speech(config.engine, engines, piper_voice.as_deref())
}

/// The decision half of probe_speech, kept pure so the "no engine" and
/// fallback rules are unit-testable without touching the machine.
pub fn decide_speech(
    selected: TtsEngine,
    engines: Vec<EngineAvailability>,
    piper_voice: Option<&str>,
) -> SpeechStatus {
    let selected_available = engines
        .iter()
        .find(|engine| engine.engine == selected)
        .is_some_and(|engine| engine.available);
    let fallback = if selected_available {
        None
    } else {
        // eSpeak NG is last: a better local engine, then the online ones,
        // then the system safety net.
        [
            TtsEngine::Kokoro,
            TtsEngine::Piper,
            TtsEngine::Edge,
            TtsEngine::ElevenLabs,
            TtsEngine::Custom,
            TtsEngine::Espeak,
        ]
        .into_iter()
        .filter(|engine| *engine != selected)
        .find(|engine| {
            engines
                .iter()
                .any(|candidate| candidate.engine == *engine && candidate.available)
        })
        .map(|engine| {
            let voice = match engine {
                TtsEngine::Piper => piper_voice
                    .map(str::to_owned)
                    .unwrap_or_else(|| engine.default_voice().to_owned()),
                _ => engine.default_voice().to_owned(),
            };
            (engine, voice)
        })
    };
    let any_available = engines.iter().any(|engine| engine.available);
    let state = if selected_available || fallback.is_some() {
        ServiceState::Connected
    } else {
        ServiceState::NotConfigured
    };
    // any_available is implied by the fallback search, but keep it honest if
    // a caller hands in an empty engine list.
    debug_assert_eq!(any_available, state == ServiceState::Connected);
    SpeechStatus {
        state,
        selected,
        selected_available,
        engines,
        fallback,
    }
}

async fn kokoro_available(url: &str) -> bool {
    let base = url.trim().trim_end_matches('/');
    if base.is_empty() {
        return false;
    }
    let Ok(client) = probe_client(true) else {
        return false;
    };
    matches!(client.get(base).send().await, Ok(_))
}

/// The first Piper voice model in the voices directory.
fn piper_voice(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut voices: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(".onnx").then_some(name)
        })
        .collect();
    voices.sort();
    voices.into_iter().next()
}

fn probe_client(no_proxy: bool) -> std::result::Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Flectar-Mail/", env!("CARGO_PKG_VERSION")));
    if no_proxy {
        // Reacher and Kokoro are local. A proxy set for the rest of the app
        // must not carry the probe, or an address, elsewhere.
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| format!("probe HTTP client: {error}"))
}

fn probe_error(service: &str, error: reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "timed out".to_owned()
    } else if error.is_connect() {
        "did not answer".to_owned()
    } else {
        error.to_string()
    };
    truncate(format!("{service} {reason}."))
}

fn truncate(mut text: String) -> String {
    if text.chars().count() > MAX_DETAIL_CHARS {
        text = text.chars().take(MAX_DETAIL_CHARS).collect::<String>() + "…";
    }
    text
}

/// Look for an executable on the PATH, with Windows extensions where needed.
fn binary_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        for extension in ["exe", "cmd", "bat"] {
            let candidate = directory.join(format!("{name}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

impl crate::Core {
    /// Probe the optional services from the current settings. Reads settings
    /// and the AI key first, then does all network work without holding a
    /// database lock.
    pub async fn probe_services(&self) -> crate::Result<ServicesReport> {
        let settings = self
            .db
            .read(|conn| crate::db::repo::settings::get(conn))
            .await?;
        let api_key = crate::accounts::credentials::load_async(
            self.credentials.clone(),
            0,
            crate::accounts::credentials::Slot::AiApiKey,
        )
        .await
        .unwrap_or_default();
        let tts_config = self.tts_config_from(&settings).await;
        Ok(probe(&settings, &api_key, &tts_config).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    fn fake_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let address = listener.local_addr().expect("fake server address");
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}"), handle)
    }

    fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind closed port");
        let address = listener.local_addr().expect("closed port address");
        drop(listener);
        format!("http://{address}")
    }

    fn availability(available: &[(TtsEngine, bool)]) -> Vec<EngineAvailability> {
        available
            .iter()
            .map(|(engine, ready)| EngineAvailability {
                engine: *engine,
                available: *ready,
            })
            .collect()
    }

    #[tokio::test]
    async fn reacher_reachable_and_unreachable_are_distinguished() {
        let (url, server) = fake_server("200 OK", "ok");
        let reachable = probe_reacher(&url).await;
        server.join().expect("fake server finished");
        assert_eq!(reachable.state, ServiceState::Connected, "{:?}", reachable);

        let unreachable = probe_reacher(&closed_port()).await;
        assert_eq!(unreachable.state, ServiceState::Unreachable);
    }

    #[tokio::test]
    async fn ai_models_endpoint_decides_reachability() {
        let (url, server) = fake_server("200 OK", r#"{"data":[]}"#);
        let connected = probe_ai(&url, "").await;
        server.join().expect("fake server finished");
        assert_eq!(connected.state, ServiceState::Connected, "{:?}", connected);

        let (url, server) = fake_server("401 Unauthorized", "{}");
        let key_needed = probe_ai(&url, "").await;
        server.join().expect("fake server finished");
        assert_eq!(
            key_needed.state,
            ServiceState::NotConfigured,
            "{:?}",
            key_needed
        );

        let unreachable = probe_ai(&closed_port(), "").await;
        assert_eq!(unreachable.state, ServiceState::Unreachable);
    }

    #[test]
    fn no_engine_means_speech_stays_off() {
        let status = decide_speech(
            TtsEngine::Kokoro,
            availability(&[
                (TtsEngine::Kokoro, false),
                (TtsEngine::Piper, false),
                (TtsEngine::Espeak, false),
            ]),
            None,
        );
        assert_eq!(status.state, ServiceState::NotConfigured);
        assert!(!status.any_available());
        assert_eq!(status.speaking_engine(), None);
    }

    #[test]
    fn espeak_is_the_last_resort_not_the_first_choice() {
        let status = decide_speech(
            TtsEngine::Kokoro,
            availability(&[
                (TtsEngine::Kokoro, false),
                (TtsEngine::Piper, false),
                (TtsEngine::Espeak, true),
            ]),
            None,
        );
        assert_eq!(
            status.fallback,
            Some((TtsEngine::Espeak, "en-us".to_owned()))
        );
        assert_eq!(status.speaking_engine(), Some(TtsEngine::Espeak));

        // A better local engine wins over eSpeak NG.
        let status = decide_speech(
            TtsEngine::Kokoro,
            availability(&[
                (TtsEngine::Kokoro, false),
                (TtsEngine::Piper, true),
                (TtsEngine::Espeak, true),
            ]),
            Some("en_US-lessac-medium.onnx"),
        );
        assert_eq!(
            status.fallback,
            Some((TtsEngine::Piper, "en_US-lessac-medium.onnx".to_owned()))
        );
    }

    #[test]
    fn configured_engine_that_works_has_no_fallback() {
        let status = decide_speech(
            TtsEngine::Kokoro,
            availability(&[(TtsEngine::Kokoro, true), (TtsEngine::Espeak, true)]),
            None,
        );
        assert!(status.selected_available);
        assert_eq!(status.fallback, None);
        assert_eq!(status.speaking_engine(), Some(TtsEngine::Kokoro));
    }
}
