//! Capability probe for the optional services Yours can use.
//!
//! Address checks, the AI features and read-aloud all depend on software the
//! app does not install: Reacher, an OpenAI-compatible model endpoint, and a
//! speech engine. On a fresh machine none of them may be present, so the shell
//! asks once, cheaply and off the database lock path, and tells the UI what is
//! actually there instead of failing later at click time.
//!
//! The probe proves that a service answers *and* that the app can actually use
//! it: the configured AI models must be listed by the endpoint, and the speech
//! engine chosen in Settings must be there. Neither answer is a guess from one
//! status code, because a server that answers 200 can still refuse every
//! request at call time ("model not found"), and a fallback voice is usable
//! without being the voice the user picked.
//!
//! The probe never sends mail content, an address, or a model prompt. Every
//! request is bounded by a short timeout and a redirect policy that does not
//! follow a broken service elsewhere.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Scenario;
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
    /// The service answered and the app can use exactly what it asked for.
    Connected,
    /// The service answers, but not with what the app is set to use: a
    /// configured model is not served, or the chosen speech engine is not
    /// installed here. Part of the app works; the label must not claim more.
    Degraded,
    /// It is switched off, no address is set, or nothing can do the job.
    NotConfigured,
    /// It is configured but did not answer.
    Unreachable,
}

impl ServiceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Degraded => "degraded",
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
    /// The player that will turn the engine's WAV into sound. An engine
    /// without one is silent, which is exactly what the card must not hide.
    pub player: Option<AudioPlayer>,
}

impl SpeechStatus {
    /// True when at least one engine can speak, so a missing configured
    /// engine degrades to another voice rather than to silence.
    pub fn any_available(&self) -> bool {
        self.engines.iter().any(|engine| engine.available)
    }

    /// The engine read-aloud speaks with right now, named for the UI. It is
    /// the selected engine when it is there, otherwise the fallback.
    pub fn speaking_engine_label(&self) -> Option<&'static str> {
        self.speaking_engine().map(TtsEngine::label)
    }

    /// The selected engine's label, for a "selected X, using Y" sentence.
    pub fn selected_engine_label(&self) -> &'static str {
        self.selected.label()
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

/// The programs read-aloud can play a synthesized WAV with, best first:
/// PipeWire's own client, then PulseAudio's, then ffplay, then ALSA's aplay.
/// Onyx has paplay but no pw-play, so naming one binary made the sample
/// silent there while Settings still said "Connected".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlayer {
    PwPlay,
    Paplay,
    Ffplay,
    Aplay,
}

/// Said when a speech engine exists but nothing can play what it synthesizes.
pub const NO_AUDIO_PLAYER: &str = "No audio player found; install pw-play, paplay or ffplay";

impl AudioPlayer {
    /// Every player read-aloud knows, in the order they are tried.
    pub const ALL: [Self; 4] = [Self::PwPlay, Self::Paplay, Self::Ffplay, Self::Aplay];

    pub const fn binary(self) -> &'static str {
        match self {
            Self::PwPlay => "pw-play",
            Self::Paplay => "paplay",
            Self::Ffplay => "ffplay",
            Self::Aplay => "aplay",
        }
    }

    /// Arguments a player needs before the file. ffplay is a video player: it
    /// must be told not to open a window, to quit at the end, and to stay
    /// quiet.
    pub const fn flags(self) -> &'static [&'static str] {
        match self {
            Self::Ffplay => &["-nodisp", "-autoexit", "-loglevel", "quiet"],
            Self::PwPlay | Self::Paplay | Self::Aplay => &[],
        }
    }

    /// The first player installed on this PATH. The probe and the player ask
    /// the same question in the same order, so Settings cannot promise audio
    /// the shell cannot play.
    pub fn detect_in(path: &std::ffi::OsStr) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|player| binary_in_path(player.binary(), path).is_some())
    }

    /// The first player installed on the process PATH.
    pub fn find() -> Option<Self> {
        Self::detect_in(&std::env::var_os("PATH")?)
    }
}

/// One AI tier as configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTier {
    Instant,
    Cheap,
    Intelligent,
}

impl AiTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Cheap => "cheap",
            Self::Intelligent => "intelligent",
        }
    }

    /// The tier for a stored tier setting; anything unknown routes to the
    /// intelligent model, exactly as `resolve_ai_model` does.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "instant" => Self::Instant,
            "cheap" => Self::Cheap,
            _ => Self::Intelligent,
        }
    }

    /// The model id these Settings use for the tier.
    pub fn model(self, settings: &Settings) -> &str {
        match self {
            Self::Instant => &settings.ai_model_instant,
            Self::Cheap => &settings.ai_model_cheap,
            Self::Intelligent => &settings.ai_model_intelligent,
        }
    }
}

/// One configured model, whether the endpoint serves it, and which features
/// depend on it. Several features can share one tier, so this is per model,
/// not per tier: Ask routed to the intelligent model must not fail because
/// the instant model is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiModelStatus {
    pub model: String,
    /// Every tier in these settings that names this model.
    pub tiers: Vec<AiTier>,
    /// Features whose configured tier is this model.
    pub scenarios: Vec<Scenario>,
    /// False when the endpoint answered without listing it.
    pub available: bool,
}

impl AiModelStatus {
    /// True when every feature routed to this model can run.
    pub fn usable(&self) -> bool {
        self.available
    }

    /// The feature names, as the UI calls them, joined for one line.
    pub fn scenario_names(&self) -> String {
        self.scenarios
            .iter()
            .map(|scenario| scenario_name(*scenario))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Human name of an AI feature, as the UI calls it.
pub fn scenario_name(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::Ask => "Ask",
        Scenario::Draft => "Draft",
        Scenario::Summarize => "Summarize",
        Scenario::Voice => "Voice",
        Scenario::Command => "Command",
        Scenario::QuickReply => "Quick replies",
        Scenario::Categorize => "Categorize",
        Scenario::Security => "Phishing check",
    }
}

/// What the AI endpoint can actually do for these settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiStatus {
    /// "connected" when every configured model is served, "degraded" when the
    /// endpoint answered but serves only some of them, "not-configured" when
    /// it wants a key, "unreachable" when it did not answer at all.
    pub state: ServiceState,
    /// One short line: what is wrong, or what answered.
    pub detail: String,
    /// Every configured model, in tier order, with the features that need it.
    pub models: Vec<AiModelStatus>,
    /// Model ids the endpoint listed, when it would list them.
    pub available_models: Vec<String>,
}

impl AiStatus {
    /// True when the endpoint is usable for at least one feature.
    pub fn usable(&self) -> bool {
        self.models.iter().any(AiModelStatus::usable)
    }

    /// True when the endpoint answers at all, degraded or not.
    pub fn answers(&self) -> bool {
        matches!(self.state, ServiceState::Connected | ServiceState::Degraded)
    }

    /// True when the model a feature runs on is served by the endpoint.
    pub fn scenario_usable(&self, scenario: Scenario) -> bool {
        self.usable()
            && self
                .models
                .iter()
                .any(|model| model.available && model.scenarios.contains(&scenario))
    }

    /// True when the given model id is served.
    pub fn model_usable(&self, model: &str) -> bool {
        self.usable()
            && self
                .models
                .iter()
                .any(|status| status.model == model.trim() && status.available)
    }

    fn unreachable(detail: impl Into<String>) -> Self {
        Self {
            state: ServiceState::Unreachable,
            ..Self::not_configured(detail)
        }
    }

    fn not_configured(detail: impl Into<String>) -> Self {
        Self {
            state: ServiceState::NotConfigured,
            detail: detail.into(),
            models: Vec::new(),
            available_models: Vec::new(),
        }
    }
}

/// Everything the first-run guide and the Settings card need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesReport {
    pub reacher: ServiceStatus,
    /// The Settings switch, independent of whether the service answers.
    pub reacher_enabled: bool,
    pub ai: AiStatus,
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
    let configured = AiConfiguredModels::from_settings(settings);
    let (ai, speech) = tokio::join!(
        probe_ai(&settings.ai_base_url, ai_api_key, &configured),
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

/// The models these Settings are configured to use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConfiguredModels {
    /// One entry per distinct model id, in tier order.
    pub models: Vec<AiModelStatus>,
}

impl AiConfiguredModels {
    /// Read the three tier models from Settings and map every AI feature onto
    /// the model its configured tier resolves to.
    pub fn from_settings(settings: &Settings) -> Self {
        let mut models: Vec<AiModelStatus> = Vec::new();
        for scenario in Scenario::ALL {
            let tier = AiTier::parse(crate::scenario_tier(settings, scenario));
            let model = tier.model(settings).trim().to_owned();
            if model.is_empty() {
                continue;
            }
            match models.iter_mut().find(|entry| entry.model == model) {
                Some(entry) => {
                    if !entry.tiers.contains(&tier) {
                        entry.tiers.push(tier);
                    }
                    entry.scenarios.push(scenario);
                }
                None => models.push(AiModelStatus {
                    model,
                    tiers: vec![tier],
                    scenarios: vec![scenario],
                    // Decided by the probe once the endpoint lists its models.
                    available: false,
                }),
            }
        }
        Self { models }
    }

    /// An explicit list, for tests and callers that already know the ids.
    pub fn new(models: impl IntoIterator<Item = String>) -> Self {
        let mut entries: Vec<AiModelStatus> = Vec::new();
        for model in models {
            let model = model.trim().to_owned();
            if model.is_empty() || entries.iter().any(|entry| entry.model == model) {
                continue;
            }
            entries.push(AiModelStatus {
                model,
                tiers: Vec::new(),
                scenarios: Vec::new(),
                available: false,
            });
        }
        Self { models: entries }
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// Ask the OpenAI-compatible models endpoint what it serves and check the
/// configured models against that list. A 2xx alone proves the server is
/// listening, not that it serves the models the app is set to use: Ollama
/// answers 200 with whatever it has pulled, and every chat call then fails
/// with "model not found". A 401/403 means the endpoint is there but needs a
/// key, which the UI calls "not configured" rather than a network failure.
pub async fn probe_ai(base_url: &str, api_key: &str, configured: &AiConfiguredModels) -> AiStatus {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return AiStatus::not_configured("No AI endpoint is set.");
    }
    let client = match probe_client(false) {
        Ok(client) => client,
        Err(error) => return AiStatus::unreachable(error),
    };
    let mut request = client
        .get(format!("{base}/models"))
        .header(reqwest::header::ACCEPT, "application/json");
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let (status, body) = match request.send().await {
        Ok(response) => {
            let status = response.status();
            if matches!(status.as_u16(), 401 | 403) {
                return AiStatus::not_configured("The AI endpoint requires an API key.");
            }
            (status, read_body(response).await)
        }
        Err(error) => {
            // A redirect is not followed, so it did not answer here.
            return AiStatus::unreachable(probe_error("AI endpoint", error));
        }
    };
    if !status.is_success() {
        // Ollama's OpenAI shim refuses /v1/models on some builds while still
        // serving /api/tags; ask that before calling the endpoint unreachable.
        if let Some(available) = ollama_tags_models(&client, &base, api_key).await {
            return decide_ai(configured, available, "the Ollama tags endpoint");
        }
        return AiStatus::unreachable(format!("The AI endpoint answered HTTP {status}."));
    }
    let body = match body {
        Ok(body) => body,
        Err(error) => return AiStatus::unreachable(error),
    };
    let available = match parse_model_ids(&body) {
        Some(available) => available,
        // It answered, but with no readable list: nothing to check, so the
        // configured models are not held against it.
        None => {
            tracing::debug!(status = %status, "AI model list could not be read");
            configured
                .models
                .iter()
                .map(|entry| entry.model.clone())
                .collect()
        }
    };
    decide_ai(configured, available, "the models endpoint")
}

/// Decide the AI state from the ids the endpoint listed. The one place the
/// "connected / degraded" rule lives, so the OpenAI and Ollama shapes agree.
fn decide_ai(configured: &AiConfiguredModels, available: Vec<String>, source: &str) -> AiStatus {
    let listed: std::collections::HashSet<&str> = available.iter().map(|id| id.trim()).collect();
    let mut models: Vec<AiModelStatus> = configured
        .models
        .iter()
        .map(|entry| AiModelStatus {
            available: listed.contains(entry.model.as_str()),
            ..entry.clone()
        })
        .collect();
    let missing: Vec<&AiModelStatus> = models.iter().filter(|entry| !entry.available).collect();
    let state = if models.is_empty() {
        // Nothing is configured to check; the endpoint still answers, but the
        // app has no model to call, so this is not a working AI setup.
        ServiceState::NotConfigured
    } else if missing.is_empty() {
        ServiceState::Connected
    } else {
        ServiceState::Degraded
    };
    let detail = match state {
        ServiceState::Connected => format!(
            "The AI endpoint serves every configured model ({}).",
            models
                .iter()
                .map(|entry| entry.model.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ServiceState::Degraded => {
            let names = missing
                .iter()
                .map(|entry| entry.model.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let available_shown: Vec<&str> = available
                .iter()
                .map(|id| id.as_str())
                .filter(|id| !id.is_empty())
                .take(3)
                .collect();
            let available_text = if available_shown.is_empty() {
                format!("{source} lists no models")
            } else {
                format!("available: {}", available_shown.join(", "))
            };
            format!("Model {names} is not served here; {available_text}.")
        }
        _ => "No AI model is configured.".to_owned(),
    };
    models.shrink_to_fit();
    AiStatus {
        state,
        detail: truncate(detail),
        models,
        available_models: available,
    }
}

/// Model ids from a models response: OpenAI's `{"data":[{"id":".."}]}`,
/// Ollama's `/api/tags` `{"models":[{"name":".."}]}`, or a bare array.
/// None when the body is not a model list at all.
pub fn parse_model_ids(body: &str) -> Option<Vec<String>> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let entries = parsed
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| parsed.get("models").and_then(serde_json::Value::as_array))
        .or_else(|| parsed.as_array())?;
    let mut ids: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            if let Some(id) = entry.as_str() {
                return Some(id.trim().to_owned());
            }
            ["id", "name", "model"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(serde_json::Value::as_str))
                .map(|id| id.trim().to_owned())
        })
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    Some(ids)
}

/// Ollama's `/api/tags` as OpenRouter-style ids; None when it does not answer.
async fn ollama_tags_models(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Option<Vec<String>> {
    let mut request = client
        .get(format!("{base}/api/tags"))
        .header(reqwest::header::ACCEPT, "application/json");
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = read_body(response).await.ok()?;
    parse_model_ids(&body)
}

/// Read a response body, bounded: a models list is small and a probe must not
/// wait on a runaway stream.
async fn read_body(response: reqwest::Response) -> std::result::Result<String, String> {
    const MAX_PROBE_BODY: usize = 512 * 1024;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| probe_error("AI endpoint", error))?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PROBE_BODY)]);
    Ok(text.into_owned())
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
    let player = AudioPlayer::find();
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
    decide_speech(config.engine, engines, piper_voice.as_deref(), player)
}

impl SpeechStatus {
    /// What the card says: which engine is missing and which one speaks
    /// instead, and separately whether anything can play it. A missing player
    /// makes read-aloud silent even with an engine present, so it is named
    /// rather than blamed on the engine.
    pub fn detail(&self) -> String {
        match self.state {
            ServiceState::Connected => {
                format!("{} is available.", self.selected_engine_label())
            }
            ServiceState::Degraded => match (&self.fallback, self.player) {
                (Some((engine, _)), None) => format!(
                    "Selected engine {} is not available here; using {}. {NO_AUDIO_PLAYER}",
                    self.selected_engine_label(),
                    engine.label()
                ),
                (Some((engine, _)), Some(_)) => format!(
                    "Selected engine {} is not available here; using {}.",
                    self.selected_engine_label(),
                    engine.label()
                ),
                (None, _) => NO_AUDIO_PLAYER.to_owned(),
            },
            ServiceState::NotConfigured | ServiceState::Unreachable => {
                "No speech engine was found.".to_owned()
            }
        }
    }
}

/// The decision half of probe_speech, kept pure so the "no engine", "no
/// player" and fallback rules are unit-testable without touching the machine.
pub fn decide_speech(
    selected: TtsEngine,
    engines: Vec<EngineAvailability>,
    piper_voice: Option<&str>,
    player: Option<AudioPlayer>,
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
    // Usable as chosen needs both halves: the engine synthesizes and some
    // player turns the WAV into sound. A fallback keeps read-aloud working,
    // which is why it exists, but the label must not hide that the engine the
    // user picked is missing: that is degraded, with both engines named. An
    // engine with no player is degraded too, with the player named instead.
    let any_engine = engines.iter().any(|engine| engine.available);
    let state = if !any_engine {
        ServiceState::NotConfigured
    } else if selected_available && player.is_some() {
        ServiceState::Connected
    } else {
        ServiceState::Degraded
    };
    debug_assert_eq!(
        any_engine,
        matches!(state, ServiceState::Connected | ServiceState::Degraded)
    );
    SpeechStatus {
        state,
        selected,
        selected_available,
        engines,
        fallback,
        player,
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
    binary_in_path(name, &std::env::var_os("PATH")?)
}

/// The same lookup against an explicit PATH, so a test can supply one.
fn binary_in_path(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    for directory in std::env::split_paths(path) {
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

/// The last model list an endpoint gave us, kept so an AI call can be refused
/// for a model the endpoint does not serve. The probe network path lives in
/// the core so the shell can share it without a second copy of the rule.
static AI_LISTING: std::sync::Mutex<Option<AiListing>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
struct AiListing {
    base_url: String,
    /// Model ids, or `None` when the endpoint answered but gave no list.
    available: Option<Vec<String>>,
}

/// Remember what the probe saw. `None` means the endpoint answered without a
/// readable model list: calls are then allowed rather than assumed broken.
pub fn record_ai_listing(base_url: &str, available: Option<Vec<String>>) {
    if let Ok(mut slot) = AI_LISTING.lock() {
        *slot = Some(AiListing {
            base_url: base_url.trim().trim_end_matches('/').to_owned(),
            available,
        });
    }
}

/// Settings > AI agents listed the models the endpoint serves. Returns true
/// when this changed what callers will see.
pub fn refresh_ai_listed_config(base_url: &str, models: Vec<String>) -> bool {
    let base = base_url.trim().trim_end_matches('/').to_owned();
    let changed = AI_LISTING
        .lock()
        .map(|slot| {
            slot.as_ref().is_none_or(|listing| {
                listing.base_url != base || listing.available.as_ref() != Some(&models)
            })
        })
        .unwrap_or(false);
    record_ai_listing(&base, Some(models));
    changed
}

/// What the last model list says about these Settings. The probe records it
/// after every check and Settings > AI agents after every list, so an AI call
/// can refuse a model the endpoint does not serve. Until some list arrives
/// every model counts as usable: the probe runs at startup, and a wrong "no"
/// would disable a working setup.
pub fn ai_status_for(settings: &Settings) -> AiStatus {
    let base = settings.ai_base_url.trim().trim_end_matches('/');
    let configured = AiConfiguredModels::from_settings(settings);
    let listing = AI_LISTING.lock().ok().and_then(|slot| slot.clone());
    match listing {
        Some(listing) if listing.base_url == base => match listing.available {
            Some(available) => decide_ai(&configured, available, "the models endpoint"),
            // The endpoint answered, but not with a list: nothing to check.
            None => decide_ai(
                &configured,
                configured
                    .models
                    .iter()
                    .map(|entry| entry.model.clone())
                    .collect(),
                "the models endpoint",
            ),
        },
        Some(listing) => {
            let mut status = AiStatus::unreachable(format!(
                "The AI endpoint changed since {} was checked.",
                listing.base_url
            ));
            status.models = configured.models;
            status
        }
        // Nothing listed yet: unknown is allowed through.
        None => {
            let available: Vec<String> = configured
                .models
                .iter()
                .map(|entry| entry.model.clone())
                .collect();
            decide_ai(&configured, available, "the models endpoint")
        }
    }
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
        let report = probe(&settings, &api_key, &tts_config).await;
        // Remember the model list so AI calls can refuse a model the endpoint
        // does not serve. An endpoint that gave no readable list records None,
        // which lets calls through rather than guessing.
        record_ai_listing(
            &settings.ai_base_url,
            report
                .ai
                .answers()
                .then(|| report.ai.available_models.clone()),
        );
        // The app has no log subscriber, so the shell reads these lines from
        // stderr. They carry service states and model ids only: no address,
        // mail, or key ever reaches them.
        eprintln!(
            "service probe: ai={} detail={:?} speech={} selected={}(available={}) speaking={} fallback={} player={} reacher={}",
            report.ai.state.as_str(),
            report.ai.detail,
            report.speech.state.as_str(),
            report.speech.selected.id(),
            report.speech.selected_available,
            report
                .speech
                .speaking_engine()
                .map(TtsEngine::id)
                .unwrap_or("-"),
            report
                .speech
                .fallback
                .as_ref()
                .map(|(engine, _)| engine.id())
                .unwrap_or("-"),
            report
                .speech
                .player
                .map(AudioPlayer::binary)
                .unwrap_or("-"),
            report.reacher.state.as_str(),
        );
        for model in &report.ai.models {
            eprintln!(
                "service probe: model {} served={} features={}",
                model.model,
                model.available,
                model.scenario_names()
            );
        }
        Ok(report)
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

    /// Settings whose three tiers name the given models, with every feature
    /// left on the shipped tier routing (ask/draft -> intelligent,
    /// summarize/categorize -> instant, voice -> cheap).
    fn configured(instant: &str, cheap: &str, intelligent: &str) -> AiConfiguredModels {
        let settings = Settings {
            ai_model_instant: instant.into(),
            ai_model_cheap: cheap.into(),
            ai_model_intelligent: intelligent.into(),
            ..Settings::default()
        };
        AiConfiguredModels::from_settings(&settings)
    }

    /// Onyx's Ollama: the endpoint answers 200, but serves none of the
    /// configured gemma4 models.
    const ONYX_MODELS: &str = r#"{"object":"list","data":[{"id":"qwen2.5vl:7b"},{"id":"glm-ocr:latest"},{"id":"qwen3.5:9b"}]}"#;

    #[tokio::test]
    async fn ai_is_connected_only_when_every_configured_model_is_served() {
        let (url, server) = fake_server(
            "200 OK",
            r#"{"data":[{"id":"gemma4:e4b"},{"id":"other:1b"}]}"#,
        );
        let connected = probe_ai(
            &url,
            "",
            &configured("gemma4:e4b", "gemma4:e4b", "gemma4:e4b"),
        )
        .await;
        server.join().expect("fake server finished");
        assert_eq!(connected.state, ServiceState::Connected, "{connected:?}");
        assert!(connected.usable());
        for scenario in Scenario::ALL {
            assert!(
                connected.scenario_usable(scenario),
                "{scenario:?} should run"
            );
        }

        // The Ollama /api/tags shape is understood too.
        let (url, server) = fake_server(
            "200 OK",
            r#"{"models":[{"name":"gemma4:e4b","model":"gemma4:e4b"}]}"#,
        );
        let ollama = probe_ai(
            &url,
            "",
            &configured("gemma4:e4b", "gemma4:e4b", "gemma4:e4b"),
        )
        .await;
        server.join().expect("fake server finished");
        assert_eq!(ollama.state, ServiceState::Connected, "{ollama:?}");
    }

    #[tokio::test]
    async fn a_200_without_the_configured_model_is_degraded_and_named() {
        let (url, server) = fake_server("200 OK", ONYX_MODELS);
        let status = probe_ai(
            &url,
            "",
            &configured("gemma4:e4b", "gemma4:e4b", "gemma4:e4b"),
        )
        .await;
        server.join().expect("fake server finished");
        assert_eq!(status.state, ServiceState::Degraded, "{status:?}");
        assert!(status.detail.contains("gemma4:e4b"), "{}", status.detail);
        assert!(status.detail.contains("qwen2.5vl:7b"), "{}", status.detail);
        // Up to three of the served models are offered as a fix.
        assert!(status.detail.contains("qwen3.5:9b"), "{}", status.detail);
        assert!(!status.usable());
        assert!(status.answers());
        assert!(!status.scenario_usable(Scenario::Summarize));
    }

    #[tokio::test]
    async fn a_missing_instant_model_does_not_disable_the_draft() {
        // Summarize/categorize run on the instant model; ask/draft on the
        // intelligent one, which this endpoint does serve.
        let (url, server) = fake_server("200 OK", r#"{"data":[{"id":"smart"},{"id":"mid"}]}"#);
        let status = probe_ai(&url, "", &configured("fast", "mid", "smart")).await;
        server.join().expect("fake server finished");
        assert_eq!(status.state, ServiceState::Degraded, "{status:?}");
        assert!(status.scenario_usable(Scenario::Draft), "{status:?}");
        assert!(status.scenario_usable(Scenario::Ask));
        assert!(status.scenario_usable(Scenario::Voice));
        assert!(!status.scenario_usable(Scenario::Summarize));
        assert!(!status.scenario_usable(Scenario::Categorize));
        assert!(!status.scenario_usable(Scenario::Security));
        assert!(status.model_usable("smart"));
        assert!(!status.model_usable("fast"));
        let summarize = status
            .models
            .iter()
            .find(|model| model.model == "fast")
            .expect("instant model reported");
        assert_eq!(summarize.tiers, vec![AiTier::Instant]);
        assert!(summarize.scenario_names().contains("Summarize"));
    }

    #[tokio::test]
    async fn ai_key_required_is_not_configured_and_a_dead_port_is_unreachable() {
        let (url, server) = fake_server("401 Unauthorized", "{}");
        let key_needed = probe_ai(&url, "", &configured("m", "m", "m")).await;
        server.join().expect("fake server finished");
        assert_eq!(
            key_needed.state,
            ServiceState::NotConfigured,
            "{key_needed:?}"
        );
        assert!(!key_needed.usable());

        let (url, server) = fake_server("403 Forbidden", "{}");
        let forbidden = probe_ai(&url, "", &configured("m", "m", "m")).await;
        server.join().expect("fake server finished");
        assert_eq!(
            forbidden.state,
            ServiceState::NotConfigured,
            "{forbidden:?}"
        );

        let unreachable = probe_ai(&closed_port(), "", &configured("m", "m", "m")).await;
        assert_eq!(
            unreachable.state,
            ServiceState::Unreachable,
            "{unreachable:?}"
        );

        let no_endpoint = probe_ai("", "", &configured("m", "m", "m")).await;
        assert_eq!(no_endpoint.state, ServiceState::NotConfigured);
    }

    #[test]
    fn configured_models_group_shared_tiers_and_features() {
        let settings = Settings {
            ai_model_instant: "same".into(),
            ai_model_cheap: "same".into(),
            ai_model_intelligent: "same".into(),
            ..Settings::default()
        };
        let configured = AiConfiguredModels::from_settings(&settings);
        assert_eq!(configured.models.len(), 1, "one id is checked once");
        let entry = &configured.models[0];
        // Tiers are recorded in the order features first need them.
        for tier in [AiTier::Instant, AiTier::Cheap, AiTier::Intelligent] {
            assert!(entry.tiers.contains(&tier), "{:?}", entry.tiers);
        }
        assert_eq!(entry.tiers.len(), 3);
        assert_eq!(entry.scenarios.len(), Scenario::ALL.len());
    }

    /// Onyx's real answer: three models that are not the configured gemma4.
    #[test]
    fn onyx_style_endpoint_reads_as_degraded_with_the_missing_model_named() {
        let status = decide_ai(
            &configured("gemma4:e4b", "gemma4:e4b", "gemma4:e4b"),
            vec![
                "glm-ocr:latest".to_owned(),
                "qwen2.5vl:7b".to_owned(),
                "qwen3.5:9b".to_owned(),
            ],
            "the models endpoint",
        );
        assert_eq!(status.state, ServiceState::Degraded, "{status:?}");
        assert!(
            status
                .detail
                .starts_with("Model gemma4:e4b is not served here;"),
            "{}",
            status.detail
        );
        // Only three of the served models are listed, however many there are.
        let many = decide_ai(
            &configured("missing", "missing", "missing"),
            vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
            "the models endpoint",
        );
        assert_eq!(many.detail.matches(", ").count(), 2, "{}", many.detail);
    }

    /// Unlisted tiers must not disable a call for a feature routed elsewhere.
    #[test]
    fn the_last_list_decides_usability_per_scenario() {
        let settings = Settings {
            ai_base_url: "http://127.0.0.1:11434/v1".into(),
            ai_model_instant: "gemma4:e4b".into(),
            ai_model_cheap: "gemma4:e4b".into(),
            ai_model_intelligent: "gemma4:e4b".into(),
            ..Settings::default()
        };
        record_ai_listing(
            "http://127.0.0.1:11434/v1/",
            Some(vec!["qwen3.5:9b".to_owned()]),
        );
        let status = ai_status_for(&settings);
        assert_eq!(status.state, ServiceState::Degraded, "{status:?}");
        assert!(status.answers());
        for scenario in Scenario::ALL {
            assert!(!status.scenario_usable(scenario), "{scenario:?}");
        }
        // The same list, but the draft's tier model is the one served.
        let settings = Settings {
            ai_model_intelligent: "qwen3.5:9b".into(),
            ..settings.clone()
        };
        let status = ai_status_for(&settings);
        assert!(status.scenario_usable(Scenario::Draft), "{status:?}");
        assert!(status.scenario_usable(Scenario::Ask));
        assert!(!status.scenario_usable(Scenario::Summarize));
        // Another endpoint has not been listed: unknown is allowed through.
        let elsewhere = Settings {
            ai_base_url: "https://api.example.test/v1".into(),
            ..settings.clone()
        };
        // The endpoint answering without a readable list is not held against it.
        record_ai_listing("http://127.0.0.1:11434/v1", None);
        assert!(ai_status_for(&settings).scenario_usable(Scenario::Draft));
        // Listed for another endpoint: this one has not been checked.
        record_ai_listing("https://other.example.test/v1", Some(Vec::new()));
        assert_eq!(ai_status_for(&elsewhere).state, ServiceState::Unreachable);
        assert_eq!(
            ai_status_for(&elsewhere).state,
            ServiceState::Unreachable,
            "a changed endpoint has not been checked"
        );
        record_ai_listing("", None);
    }

    #[test]
    fn model_list_parsing_accepts_openai_ollama_and_bare_arrays() {
        assert_eq!(
            parse_model_ids(r#"{"data":[{"id":"b"},{"id":"a"}]}"#),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            parse_model_ids(r#"{"models":[{"name":"a:latest"}]}"#),
            Some(vec!["a:latest".to_owned()])
        );
        assert_eq!(parse_model_ids(r#"["a","a"]"#), Some(vec!["a".to_owned()]));
        assert_eq!(parse_model_ids("<html>"), None);
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
            Some(AudioPlayer::Paplay),
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
            Some(AudioPlayer::Paplay),
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
            Some(AudioPlayer::Paplay),
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
            Some(AudioPlayer::Paplay),
        );
        assert!(status.selected_available);
        assert_eq!(status.state, ServiceState::Connected);
        assert_eq!(status.fallback, None);
        assert_eq!(status.speaking_engine(), Some(TtsEngine::Kokoro));
    }

    #[test]
    fn selected_engine_that_works_reads_as_connected() {
        let status = decide_speech(
            TtsEngine::Piper,
            availability(&[(TtsEngine::Piper, true), (TtsEngine::Espeak, true)]),
            Some("en_US-lessac-medium.onnx"),
            Some(AudioPlayer::Paplay),
        );
        assert_eq!(status.state, ServiceState::Connected, "{status:?}");
        assert_eq!(status.speaking_engine(), Some(TtsEngine::Piper));
        assert!(status.detail().contains("Piper"), "{}", status.detail());
    }

    /// Onyx: no Kokoro on 127.0.0.1:8766, but espeak-ng and piper are there.
    #[test]
    fn a_fallback_speaks_but_reads_as_degraded_and_names_both_engines() {
        let status = decide_speech(
            TtsEngine::Kokoro,
            availability(&[
                (TtsEngine::Kokoro, false),
                (TtsEngine::Piper, true),
                (TtsEngine::Espeak, true),
            ]),
            Some("en_US-lessac-medium.onnx"),
            Some(AudioPlayer::Paplay),
        );
        assert_eq!(status.state, ServiceState::Degraded, "{status:?}");
        // Still truthful about what works: read-aloud stays on with the
        // better local engine, not eSpeak.
        assert!(status.any_available());
        assert_eq!(status.speaking_engine(), Some(TtsEngine::Piper));
        assert_eq!(status.selected_engine_label(), "Kokoro (local)");
        assert_eq!(status.speaking_engine_label(), Some("Piper (local)"));
        assert_eq!(
            status.detail(),
            "Selected engine Kokoro (local) is not available here; using Piper (local)."
        );
        // Both engines stay exposed for the card.
        let engines: Vec<(TtsEngine, bool)> = status
            .engines
            .iter()
            .map(|engine| (engine.engine, engine.available))
            .collect();
        assert!(engines.contains(&(TtsEngine::Kokoro, false)));
        assert!(engines.contains(&(TtsEngine::Piper, true)));

        // eSpeak alone is the same degraded answer, naming eSpeak NG.
        let espeak_only = decide_speech(
            TtsEngine::Kokoro,
            availability(&[(TtsEngine::Kokoro, false), (TtsEngine::Espeak, true)]),
            None,
            Some(AudioPlayer::Paplay),
        );
        assert_eq!(espeak_only.state, ServiceState::Degraded);
        assert_eq!(
            espeak_only.detail(),
            "Selected engine Kokoro (local) is not available here; using eSpeak NG (system)."
        );
    }

    /// A throwaway directory standing in for the PATH, holding the named
    /// players as files, the way the probe finds them on a real PATH.
    struct FakePath {
        dir: PathBuf,
        path: std::ffi::OsString,
    }

    impl FakePath {
        fn new(names: &[&str]) -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "flectar-audio-players-{}-{unique}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create fake PATH directory");
            for name in names {
                std::fs::write(dir.join(name), b"#!/bin/sh\n").expect("write fake player");
            }
            let path = dir.clone().into_os_string();
            Self { dir, path }
        }

        fn os(&self) -> &std::ffi::OsStr {
            &self.path
        }
    }

    impl Drop for FakePath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn player_resolution_takes_the_first_player_installed_in_order() {
        assert_eq!(AudioPlayer::detect_in(FakePath::new(&[]).os()), None);

        let alsa = FakePath::new(&["aplay"]);
        assert_eq!(AudioPlayer::detect_in(alsa.os()), Some(AudioPlayer::Aplay));

        let ffmpeg_and_alsa = FakePath::new(&["aplay", "ffplay"]);
        assert_eq!(
            AudioPlayer::detect_in(ffmpeg_and_alsa.os()),
            Some(AudioPlayer::Ffplay)
        );

        let pulse = FakePath::new(&["ffplay", "paplay"]);
        assert_eq!(AudioPlayer::detect_in(pulse.os()), Some(AudioPlayer::Paplay));

        // PipeWire wins when it is there, whatever else is installed.
        let pipewire = FakePath::new(&["aplay", "paplay", "pw-play"]);
        assert_eq!(
            AudioPlayer::detect_in(pipewire.os()),
            Some(AudioPlayer::PwPlay)
        );

        // Only the known players count, not any program on the PATH.
        let unrelated = FakePath::new(&["mpv", "play"]);
        assert_eq!(AudioPlayer::detect_in(unrelated.os()), None);
    }

    #[test]
    fn ffplay_is_told_to_stay_headless_and_quiet() {
        assert_eq!(
            AudioPlayer::Ffplay.flags(),
            &["-nodisp", "-autoexit", "-loglevel", "quiet"]
        );
        for player in [
            AudioPlayer::PwPlay,
            AudioPlayer::Paplay,
            AudioPlayer::Aplay,
        ] {
            assert!(
                player.flags().is_empty(),
                "{} needs no flags",
                player.binary()
            );
        }
    }

    /// Onyx: Piper and paplay are installed, pw-play is not. Engine without
    /// any player is degraded and says what to install instead of leaving the
    /// sample silent.
    #[test]
    fn engine_without_a_player_reads_as_degraded_and_names_what_to_install() {
        let no_player = decide_speech(
            TtsEngine::Piper,
            availability(&[(TtsEngine::Piper, true), (TtsEngine::Espeak, true)]),
            Some("en_US-lessac-medium.onnx"),
            None,
        );
        assert_eq!(no_player.state, ServiceState::Degraded, "{no_player:?}");
        assert!(no_player.any_available(), "the engine itself still works");
        assert_eq!(no_player.speaking_engine(), Some(TtsEngine::Piper));
        assert_eq!(no_player.detail(), NO_AUDIO_PLAYER);

        // A falling-back engine without a player names both problems.
        let fallback_no_player = decide_speech(
            TtsEngine::Kokoro,
            availability(&[(TtsEngine::Kokoro, false), (TtsEngine::Espeak, true)]),
            None,
            None,
        );
        assert_eq!(fallback_no_player.state, ServiceState::Degraded);
        assert_eq!(
            fallback_no_player.detail(),
            format!(
                "Selected engine Kokoro (local) is not available here; using eSpeak NG (system). {}",
                NO_AUDIO_PLAYER
            )
        );
    }

    #[test]
    fn engine_and_player_together_read_as_connected() {
        let status = decide_speech(
            TtsEngine::Piper,
            availability(&[(TtsEngine::Piper, true)]),
            Some("en_US-lessac-medium.onnx"),
            Some(AudioPlayer::Paplay),
        );
        assert_eq!(status.state, ServiceState::Connected, "{status:?}");
        assert_eq!(status.player, Some(AudioPlayer::Paplay));
        assert_eq!(status.detail(), "Piper (local) is available.");

        // The selected engine being there is not enough on its own: a
        // fallback engine plus a player stays degraded, as it did before.
        let fallback = decide_speech(
            TtsEngine::Kokoro,
            availability(&[(TtsEngine::Kokoro, false), (TtsEngine::Espeak, true)]),
            None,
            Some(AudioPlayer::Ffplay),
        );
        assert_eq!(fallback.state, ServiceState::Degraded);
    }

    #[test]
    fn no_engine_and_no_player_stays_not_configured() {
        let status = decide_speech(
            TtsEngine::Piper,
            availability(&[(TtsEngine::Piper, false), (TtsEngine::Espeak, false)]),
            None,
            None,
        );
        assert_eq!(status.state, ServiceState::NotConfigured);
        assert!(!status.any_available());
        // The missing engine is the first thing to fix, so it is what the card
        // says; the player only matters once something can synthesize.
        assert_eq!(status.detail(), "No speech engine was found.");
    }
}
