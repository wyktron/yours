//! Text-to-speech engines the reader can speak a message with.
//!
//! The reader highlights each word as it is heard, so every engine here hands
//! back timed words: engines that report them (Kokoro, Edge subtitles) pass
//! their own; the rest are timed from the length of the audio, which keeps the
//! highlight moving even when the engine says nothing about timing.
//!
//! Engines are picked at runtime from Settings: the local Kokoro server and
//! Piper models, the system eSpeak NG, Microsoft's Edge voices, ElevenLabs, or
//! any OpenAI-compatible speech endpoint. Synthesis always returns WAV, so the
//! player stays one code path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::AsyncWriteExt as _;

use crate::error::{CoreError, Result};

/// Engine used when no preference is stored, matching every build before the
/// choice existed.
pub const DEFAULT_ENGINE: &str = "kokoro";

/// The Kokoro server started by the kokoro-tts user unit.
pub const KOKORO_DEFAULT_URL: &str = "http://127.0.0.1:8766";
/// Voice the reader starts with: British English, female, clear.
pub const KOKORO_DEFAULT_VOICE: &str = "bf_emma";

const ELEVENLABS_API: &str = "https://api.elevenlabs.io/v1";
const ELEVENLABS_DEFAULT_MODEL: &str = "eleven_multilingual_v2";
const CUSTOM_DEFAULT_MODEL: &str = "tts-1";
/// Voice a custom endpoint gets before the user names one.
pub const CUSTOM_DEFAULT_VOICE: &str = "alloy";
/// Longest a single request may take before it is reported as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Which service speaks the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TtsEngine {
    /// Local Kokoro server; reports a time for every word.
    #[default]
    Kokoro,
    /// Local Piper model on the CPU.
    Piper,
    /// The system's eSpeak NG.
    Espeak,
    /// Microsoft's Edge voices over the network; subtitles time the words.
    Edge,
    /// ElevenLabs over the network.
    ElevenLabs,
    /// Any OpenAI-compatible speech endpoint.
    Custom,
}

impl TtsEngine {
    /// Stable id stored in settings.
    pub fn id(self) -> &'static str {
        match self {
            TtsEngine::Kokoro => "kokoro",
            TtsEngine::Piper => "piper",
            TtsEngine::Espeak => "espeak",
            TtsEngine::Edge => "edge",
            TtsEngine::ElevenLabs => "elevenlabs",
            TtsEngine::Custom => "custom",
        }
    }

    /// Name shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            TtsEngine::Kokoro => "Kokoro (local)",
            TtsEngine::Piper => "Piper (local)",
            TtsEngine::Espeak => "eSpeak NG (system)",
            TtsEngine::Edge => "Edge TTS (online)",
            TtsEngine::ElevenLabs => "ElevenLabs (online)",
            TtsEngine::Custom => "Custom endpoint (online)",
        }
    }

    /// Parses a stored id; anything unknown falls back to Kokoro so a settings
    /// file from a newer build still speaks.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "piper" => TtsEngine::Piper,
            "espeak" | "espeak-ng" => TtsEngine::Espeak,
            "edge" | "edge-tts" => TtsEngine::Edge,
            "elevenlabs" | "eleven_labs" | "eleven-labs" => TtsEngine::ElevenLabs,
            "custom" | "openai" => TtsEngine::Custom,
            _ => TtsEngine::Kokoro,
        }
    }

    /// Every engine, in the order Settings lists them.
    pub fn all() -> [TtsEngine; 6] {
        [
            TtsEngine::Kokoro,
            TtsEngine::Piper,
            TtsEngine::Espeak,
            TtsEngine::Edge,
            TtsEngine::ElevenLabs,
            TtsEngine::Custom,
        ]
    }

    /// Whether the engine needs an API key before it can speak.
    pub fn needs_key(self) -> bool {
        matches!(self, TtsEngine::ElevenLabs | TtsEngine::Custom)
    }

    /// Whether the engine runs on this machine.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            TtsEngine::Kokoro | TtsEngine::Piper | TtsEngine::Espeak
        )
    }

    /// Voice used the first time this engine is chosen.
    pub fn default_voice(self) -> &'static str {
        match self {
            TtsEngine::Kokoro => KOKORO_DEFAULT_VOICE,
            TtsEngine::Piper => "en_US-lessac-medium.onnx",
            TtsEngine::Espeak => "en-us",
            TtsEngine::Edge => "en-US-AvaMultilingualNeural",
            TtsEngine::ElevenLabs => "pNInz6obpgDQGcFmaJgB",
            TtsEngine::Custom => CUSTOM_DEFAULT_VOICE,
        }
    }
}

/// One voice an engine offers, with the name Settings shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtsVoice {
    pub id: String,
    pub label: String,
}

impl TtsVoice {
    fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// Everything synthesis needs: the engine, its voice, and how to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtsConfig {
    pub engine: TtsEngine,
    pub voice: String,
    /// Kokoro server base URL.
    pub kokoro_url: String,
    /// Base URL of a custom OpenAI-compatible endpoint.
    pub endpoint: String,
    /// Model id a cloud engine should use; empty asks for the engine default.
    pub model: String,
    /// API key for engines that need one.
    pub api_key: String,
    /// Directory Piper models live in.
    pub voices_dir: PathBuf,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: TtsEngine::Kokoro,
            voice: KOKORO_DEFAULT_VOICE.to_owned(),
            kokoro_url: KOKORO_DEFAULT_URL.to_owned(),
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
            voices_dir: default_voices_dir(),
        }
    }
}

impl TtsConfig {
    /// The voice actually used: the configured one when the engine knows it,
    /// otherwise the engine's default, so switching engines never speaks in a
    /// voice the new engine has never heard of.
    pub fn resolved_voice(&self) -> String {
        let voice = self.voice.trim();
        if self.engine == TtsEngine::Custom {
            return if voice.is_empty() {
                CUSTOM_DEFAULT_VOICE.to_owned()
            } else {
                voice.to_owned()
            };
        }
        let known = voices(self).iter().any(|candidate| candidate.id == voice);
        if known {
            voice.to_owned()
        } else {
            self.engine.default_voice().to_owned()
        }
    }

    /// The model sent to a cloud engine: the configured one, else the default.
    pub fn resolved_model(&self) -> String {
        let model = self.model.trim();
        if !model.is_empty() {
            return model.to_owned();
        }
        match self.engine {
            TtsEngine::ElevenLabs => ELEVENLABS_DEFAULT_MODEL.to_owned(),
            TtsEngine::Custom => CUSTOM_DEFAULT_MODEL.to_owned(),
            _ => String::new(),
        }
    }

    /// The address a custom endpoint is posted to. A bare base URL, a URL that
    /// ends in /v1, and the full speech URL are all accepted.
    pub fn custom_speech_url(&self) -> Result<String> {
        let base = self.endpoint.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err(CoreError::Other(
                "Set the endpoint of the speech service first.".into(),
            ));
        }
        if base.ends_with("/audio/speech") {
            return Ok(base.to_owned());
        }
        if base.ends_with("/v1") {
            return Ok(format!("{base}/audio/speech"));
        }
        Ok(format!("{base}/v1/audio/speech"))
    }
}

/// Piper models live in one directory; the setting may override it.
pub fn default_voices_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".local/share/piper-voices")
}

/// The Kokoro voice list, exactly what the server ships.
pub const KOKORO_VOICES: [&str; 28] = [
    "af_alloy",
    "af_aoede",
    "af_bella",
    "af_heart",
    "af_jessica",
    "af_kore",
    "af_nicole",
    "af_nova",
    "af_river",
    "af_sarah",
    "af_sky",
    "am_adam",
    "am_echo",
    "am_eric",
    "am_fenrir",
    "am_liam",
    "am_michael",
    "am_onyx",
    "am_puck",
    "am_santa",
    "bf_alice",
    "bf_emma",
    "bf_isabella",
    "bf_lily",
    "bm_daniel",
    "bm_fable",
    "bm_george",
    "bm_lewis",
];

fn kokoro_label(id: &str) -> String {
    let (group, name) = match id.get(..2) {
        Some("af") => ("American female", &id[3..]),
        Some("am") => ("American male", &id[3..]),
        Some("bf") => ("British female", &id[3..]),
        Some("bm") => ("British male", &id[3..]),
        _ => return id.to_owned(),
    };
    let mut chars = name.chars();
    let pretty = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    format!("{pretty} · {group}")
}

/// eSpeak NG language variants worth offering.
const ESPEAK_VOICES: [(&str, &str); 20] = [
    ("en", "English (GB)"),
    ("en-us", "English (US)"),
    ("en-gb-scotland", "English (Scotland)"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("hu", "Hungarian"),
    ("bg", "Bulgarian"),
    ("el", "Greek"),
    ("sv", "Swedish"),
    ("cs", "Czech"),
    ("ca", "Catalan"),
];

/// Edge voices offered out of the box, English and the languages this machine
/// actually speaks.
const EDGE_VOICES: [&str; 32] = [
    "en-US-AvaMultilingualNeural",
    "en-US-AndrewMultilingualNeural",
    "en-US-EmmaMultilingualNeural",
    "en-US-BrianMultilingualNeural",
    "en-US-AriaNeural",
    "en-US-GuyNeural",
    "en-US-JennyNeural",
    "en-US-MichelleNeural",
    "en-US-RogerNeural",
    "en-US-SteffanNeural",
    "en-US-ChristopherNeural",
    "en-US-EricNeural",
    "en-GB-SoniaNeural",
    "en-GB-RyanNeural",
    "en-GB-LibbyNeural",
    "en-GB-ThomasNeural",
    "ro-RO-EmilNeural",
    "ro-RO-AlinaNeural",
    "ru-RU-SvetlanaNeural",
    "ru-RU-DmitryNeural",
    "de-DE-KatjaNeural",
    "de-DE-ConradNeural",
    "fr-FR-DeniseNeural",
    "fr-FR-HenriNeural",
    "es-ES-ElviraNeural",
    "es-ES-AlvaroNeural",
    "it-IT-ElsaNeural",
    "it-IT-DiegoNeural",
    "pt-PT-RaquelNeural",
    "pt-PT-DuarteNeural",
    "nl-NL-FennaNeural",
    "pl-PL-ZofiaNeural",
];

fn edge_label(id: &str) -> String {
    let locale = id.split('-').take(2).collect::<Vec<_>>().join("-");
    let name = id
        .rsplit('-')
        .next()
        .unwrap_or(id)
        .trim_end_matches("Neural")
        .trim_end_matches("Multilingual");
    format!("{name} · {locale}")
}

/// Voices known to the ElevenLabs account this machine used; replaced by the
/// live list whenever the API answers.
const ELEVENLABS_KNOWN: [(&str, &str); 12] = [
    ("pNInz6obpgDQGcFmaJgB", "Adam · dominant, firm"),
    ("EXAVITQu4vr4xnSDxMaL", "Sarah · mature, reassuring"),
    ("Xb7hH8MSUJpSbSDYk0k2", "Alice · clear, engaging"),
    ("JBFqnCBsd6RMkjVDRZzb", "George · warm storyteller"),
    ("IKne3meq5aSn9XLyUdCD", "Charlie · deep, energetic"),
    ("XrExE9yKIg1WjnnlVkGX", "Matilda · professional"),
    ("cgSgspJ2msm6clMCkdW9", "Jessica · playful, bright"),
    ("cjVigY5qzO86Huf0OWal", "Eric · smooth, trustworthy"),
    ("hpp4J3VqNfWAUOO0d1Us", "Bella · professional, warm"),
    ("nPczCjzI2devNBz1zQrb", "Brian · deep, comforting"),
    ("pFZP5JQG7iQjIQuC4Bku", "Lily · velvety actress"),
    ("onwK4e9ZLuTAKqWW03F9", "Daniel · steady broadcaster"),
];

/// Voices an engine can speak with right now, without touching the network.
pub fn voices(config: &TtsConfig) -> Vec<TtsVoice> {
    match config.engine {
        TtsEngine::Kokoro => KOKORO_VOICES
            .iter()
            .map(|id| TtsVoice::new(*id, kokoro_label(id)))
            .collect(),
        TtsEngine::Piper => piper_voices(&config.voices_dir),
        TtsEngine::Espeak => ESPEAK_VOICES
            .iter()
            .map(|(id, label)| TtsVoice::new(*id, *label))
            .collect(),
        TtsEngine::Edge => EDGE_VOICES
            .iter()
            .map(|id| TtsVoice::new(*id, edge_label(id)))
            .collect(),
        TtsEngine::ElevenLabs => ELEVENLABS_KNOWN
            .iter()
            .map(|(id, label)| TtsVoice::new(*id, *label))
            .collect(),
        // A custom endpoint names its own voices; the field is free text.
        TtsEngine::Custom => Vec::new(),
    }
}

/// Piper voices are the model files in the voices directory.
pub fn piper_voices(dir: &Path) -> Vec<TtsVoice> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut voices: Vec<TtsVoice> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".onnx")?;
            Some(TtsVoice::new(name.clone(), id.replace('_', " ")))
        })
        .collect();
    voices.sort_by(|left, right| left.id.cmp(&right.id));
    voices
}

/// Voices that need the network: ElevenLabs lists what the account owns.
pub async fn voices_async(config: &TtsConfig) -> Vec<TtsVoice> {
    if config.engine != TtsEngine::ElevenLabs || config.api_key.trim().is_empty() {
        return voices(config);
    }
    match elevenlabs_voices(&config.api_key).await {
        Ok(live) if !live.is_empty() => live,
        _ => voices(config),
    }
}

#[derive(serde::Deserialize)]
struct ElevenLabsVoice {
    voice_id: String,
    name: String,
}

#[derive(serde::Deserialize)]
struct ElevenLabsList {
    #[serde(default)]
    voices: Vec<ElevenLabsVoice>,
}

/// The voices an ElevenLabs key can reach.
pub async fn elevenlabs_voices(api_key: &str) -> Result<Vec<TtsVoice>> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{ELEVENLABS_API}/voices"))
        .timeout(Duration::from_secs(20))
        .header("xi-api-key", api_key.trim())
        .send()
        .await
        .map_err(|error| CoreError::Network(format!("ElevenLabs voices: {error}")))?;
    if !response.status().is_success() {
        return Err(CoreError::Network(format!(
            "ElevenLabs refused the key with HTTP {}",
            response.status()
        )));
    }
    let list = response
        .json::<ElevenLabsList>()
        .await
        .map_err(|error| CoreError::Network(format!("ElevenLabs voices: {error}")))?;
    Ok(list
        .voices
        .into_iter()
        .map(|voice| TtsVoice::new(voice.voice_id, voice.name))
        .collect())
}

/// One spoken word: when it is heard.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Finished speech: audio the player can open, and when each word is heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Synthesis {
    pub wav: Vec<u8>,
    pub words: Vec<TimedWord>,
}

impl Synthesis {
    /// Length of the audio in seconds, when it can be measured.
    pub fn duration(&self) -> Option<f64> {
        wav_duration(&self.wav)
    }
}

/// Speak text with the configured engine.
pub async fn synthesize(config: &TtsConfig, text: &str) -> Result<Synthesis> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CoreError::Other("There is nothing to read.".into()));
    }
    let voice = config.resolved_voice();
    let attempt = tokio::time::timeout(REQUEST_TIMEOUT, async {
        match config.engine {
            TtsEngine::Kokoro => kokoro(config, &voice, text).await,
            TtsEngine::Piper => piper(config, &voice, text).await,
            TtsEngine::Espeak => espeak(&voice, text).await,
            TtsEngine::Edge => edge(&voice, text).await,
            TtsEngine::ElevenLabs => elevenlabs(config, &voice, text).await,
            TtsEngine::Custom => custom(config, &voice, text).await,
        }
    })
    .await;
    match attempt {
        Ok(result) => result,
        Err(_) => Err(CoreError::Other(format!(
            "{} did not answer within {} seconds.",
            config.engine.label(),
            REQUEST_TIMEOUT.as_secs()
        ))),
    }
}

/// The reader speaks a short sample so a voice can be checked in Settings.
pub const SAMPLE_TEXT: &str =
    "This is the voice Flectar reads your mail with. Every word lights up as it is spoken.";

////////////////
// Kokoro
////////////////

#[derive(serde::Deserialize)]
struct KokoroTimed {
    wav: String,
    #[serde(default)]
    words: Vec<KokoroWord>,
}

#[derive(serde::Deserialize)]
struct KokoroWord {
    t: String,
    s: f64,
    e: f64,
}

async fn kokoro(config: &TtsConfig, voice: &str, text: &str) -> Result<Synthesis> {
    let url = format!("{}/speak_timed", config.kokoro_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .form(&[("text", text), ("voice", voice)])
        .send()
        .await
        .map_err(|error| CoreError::Network(format!("Kokoro at {url}: {error}")))?;
    if !response.status().is_success() {
        return Err(CoreError::Network(format!(
            "Kokoro at {url} answered HTTP {}",
            response.status()
        )));
    }
    let timed = response
        .json::<KokoroTimed>()
        .await
        .map_err(|error| CoreError::Network(format!("Kokoro at {url}: {error}")))?;
    let wav = base64::engine::general_purpose::STANDARD
        .decode(&timed.wav)
        .map_err(|error| CoreError::Other(format!("Kokoro sent unusable audio: {error}")))?;
    let words = timed
        .words
        .into_iter()
        .map(|word| TimedWord {
            text: word.t,
            start: word.s,
            end: word.e,
        })
        .collect();
    Ok(Synthesis { wav, words })
}

////////////////
// Piper
////////////////

async fn piper(config: &TtsConfig, voice: &str, text: &str) -> Result<Synthesis> {
    let model = config.voices_dir.join(voice);
    if !model.is_file() {
        return Err(CoreError::Other(format!(
            "Piper has no voice named {voice} in {}.",
            config.voices_dir.display()
        )));
    }
    let out = temp_path("piper", "wav");
    let mut child = tokio::process::Command::new("piper")
        .arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(&out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            CoreError::Other(format!(
                "Could not start piper ({error}). Install it, or pick another engine."
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
    }
    let status = child.wait().await?;
    let wav = tokio::fs::read(&out).await.unwrap_or_default();
    let _ = tokio::fs::remove_file(&out).await;
    if !status.success() || wav.is_empty() {
        return Err(CoreError::Other(
            "Piper could not speak this text; its model may be missing or corrupt.".into(),
        ));
    }
    let words = timings(text, wav_duration(&wav));
    Ok(Synthesis { wav, words })
}

////////////////
// eSpeak NG
////////////////

async fn espeak(voice: &str, text: &str) -> Result<Synthesis> {
    let out = temp_path("espeak", "wav");
    let mut child = tokio::process::Command::new("espeak-ng")
        .arg("-v")
        .arg(voice)
        .arg("-w")
        .arg(&out)
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            CoreError::Other(format!(
                "Could not start espeak-ng ({error}). Install it, or pick another engine."
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
    }
    let status = child.wait().await?;
    let wav = tokio::fs::read(&out).await.unwrap_or_default();
    let _ = tokio::fs::remove_file(&out).await;
    if !status.success() || wav.is_empty() {
        return Err(CoreError::Other(
            "eSpeak NG could not speak this text; check the installed voices.".into(),
        ));
    }
    let words = timings(text, wav_duration(&wav));
    Ok(Synthesis { wav, words })
}

////////////////
// Edge
////////////////

async fn edge(voice: &str, text: &str) -> Result<Synthesis> {
    let script = temp_path("edge", "txt");
    let media = temp_path("edge", "mp3");
    let subtitles = temp_path("edge", "srt");
    if let Err(error) = tokio::fs::write(&script, text).await {
        return Err(CoreError::Io(error));
    }
    let status = tokio::process::Command::new("edge-tts")
        .arg("-f")
        .arg(&script)
        .arg("-v")
        .arg(voice)
        .arg("--write-media")
        .arg(&media)
        .arg("--write-subtitles")
        .arg(&subtitles)
        .arg("--no-progress")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|error| {
            CoreError::Other(format!(
                "Could not start edge-tts ({error}). Install it, or pick another engine."
            ))
        })?;
    let mp3 = tokio::fs::read(&media).await.unwrap_or_default();
    let cues = tokio::fs::read_to_string(&subtitles)
        .await
        .map(|text| parse_srt(&text))
        .unwrap_or_default();
    let _ = tokio::fs::remove_file(&script).await;
    let _ = tokio::fs::remove_file(&media).await;
    let _ = tokio::fs::remove_file(&subtitles).await;
    if !status.success() || mp3.is_empty() {
        return Err(CoreError::Other(
            "Edge TTS could not speak this text; check the voice id and the network.".into(),
        ));
    }
    let wav = to_wav(mp3, "mp3").await?;
    let words = if cues.is_empty() {
        timings(text, wav_duration(&wav))
    } else {
        cue_timings(&cues)
    };
    Ok(Synthesis { wav, words })
}

////////////////
// ElevenLabs
////////////////

async fn elevenlabs(config: &TtsConfig, voice: &str, text: &str) -> Result<Synthesis> {
    let key = config.api_key.trim();
    if key.is_empty() {
        return Err(CoreError::Other(
            "Save an ElevenLabs API key in Settings first.".into(),
        ));
    }
    let url = format!(
        "{ELEVENLABS_API}/text-to-speech/{}?output_format=pcm_24000",
        urlencode(voice)
    );
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("xi-api-key", key)
        .json(&serde_json::json!({
            "text": text,
            "model_id": config.resolved_model(),
        }))
        .send()
        .await
        .map_err(|error| CoreError::Network(format!("ElevenLabs: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CoreError::Network(format!(
            "ElevenLabs answered HTTP {status}: {}",
            body.trim().chars().take(200).collect::<String>()
        )));
    }
    let pcm = response
        .bytes()
        .await
        .map_err(|error| CoreError::Network(format!("ElevenLabs: {error}")))?;
    if pcm.is_empty() {
        return Err(CoreError::Other(
            "ElevenLabs sent no audio; check that the voice is still in the account.".into(),
        ));
    }
    let wav = pcm_to_wav(&pcm, 24_000, 1);
    let words = timings(text, wav_duration(&wav));
    Ok(Synthesis { wav, words })
}

////////////////
// Custom OpenAI-compatible endpoint
////////////////

async fn custom(config: &TtsConfig, voice: &str, text: &str) -> Result<Synthesis> {
    let url = config.custom_speech_url()?;
    let mut request = reqwest::Client::new().post(&url).json(&serde_json::json!({
        "model": config.resolved_model(),
        "voice": voice,
        "input": text,
        "response_format": "wav",
    }));
    let key = config.api_key.trim();
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| CoreError::Network(format!("{url}: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CoreError::Network(format!(
            "{url} answered HTTP {status}: {}",
            body.trim().chars().take(200).collect::<String>()
        )));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| CoreError::Network(format!("{url}: {error}")))?;
    if body.is_empty() {
        return Err(CoreError::Other(format!("{url} sent no audio.")));
    }
    let wav = if body.starts_with(b"RIFF") {
        body.to_vec()
    } else {
        to_wav(body.to_vec(), "mp3").await?
    };
    let words = timings(text, wav_duration(&wav));
    Ok(Synthesis { wav, words })
}

////////////////
// Audio and timing helpers
////////////////

/// Seconds of audio in a PCM WAV file, from its data chunk and byte rate.
pub fn wav_duration(wav: &[u8]) -> Option<f64> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut position = 12usize;
    let mut byte_rate = 0u64;
    while position + 8 <= wav.len() {
        let id = &wav[position..position + 4];
        let size = u32::from_le_bytes(wav[position + 4..position + 8].try_into().ok()?) as usize;
        if id == b"fmt " && position + 20 <= wav.len() {
            byte_rate =
                u32::from_le_bytes(wav[position + 16..position + 20].try_into().ok()?) as u64;
        } else if id == b"data" {
            if byte_rate == 0 {
                return None;
            }
            let available = wav.len().saturating_sub(position + 8);
            let data = size.min(available);
            return Some(data as f64 / byte_rate as f64);
        }
        position = position.checked_add(8 + size + (size & 1))?;
    }
    None
}

/// Wrap headerless 16-bit PCM into a WAV file the player accepts.
pub fn pcm_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Convert compressed audio to WAV with ffmpeg, which the Edge and custom
/// engines rely on for MP3 answers.
async fn to_wav(audio: Vec<u8>, extension: &str) -> Result<Vec<u8>> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "wav",
            "-acodec",
            "pcm_s16le",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            CoreError::Other(format!(
                "This engine returned {extension} audio and converting it needs ffmpeg ({error})."
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(&audio).await;
        });
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(CoreError::Other(
            "The audio could not be converted to WAV with ffmpeg.".into(),
        ));
    }
    Ok(output.stdout)
}

/// Timing for every word when the engine reports none: spread the measured
/// audio across the words, longer words taking proportionally longer.
pub fn timings(text: &str, duration: Option<f64>) -> Vec<TimedWord> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(duration) = duration.filter(|value| *value > 0.0) else {
        return Vec::new();
    };
    if words.is_empty() {
        return Vec::new();
    }
    distribute(&words, 0.0, duration)
}

/// Hands each word a slice of start..end sized by its length.
fn distribute(words: &[&str], start: f64, end: f64) -> Vec<TimedWord> {
    let weights: Vec<f64> = words
        .iter()
        .map(|word| word.chars().count() as f64 + 1.0)
        .collect();
    let total: f64 = weights.iter().sum();
    let span = (end - start).max(0.0);
    let mut cursor = start;
    let mut timed = Vec::with_capacity(words.len());
    for (word, weight) in words.iter().zip(weights) {
        let share = span * weight / total;
        timed.push(TimedWord {
            text: (*word).to_owned(),
            start: cursor,
            end: cursor + share,
        });
        cursor += share;
    }
    timed
}

/// One subtitle cue from Edge: a sentence and when it is heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Parse the SRT subtitles Edge TTS writes next to its audio.
pub fn parse_srt(source: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((start, end)) = parse_srt_times(line) else {
            continue;
        };
        let mut text = String::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() {
                let _ = lines.next();
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(next.trim());
            let _ = lines.next();
        }
        if !text.trim().is_empty() {
            cues.push(Cue { start, end, text });
        }
    }
    cues
}

fn parse_srt_times(line: &str) -> Option<(f64, f64)> {
    let (start, rest) = line.split_once("-->")?;
    Some((parse_srt_time(start)?, parse_srt_time(rest)?))
}

fn parse_srt_time(value: &str) -> Option<f64> {
    let value = value.trim();
    let (clock, millis) = value.split_once(',')?;
    let mut parts = clock.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    let millis: f64 = millis.trim().parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0)
}

/// Word timings from subtitle cues: words inside a cue share the cue's time.
pub fn cue_timings(cues: &[Cue]) -> Vec<TimedWord> {
    let mut timed = Vec::new();
    for cue in cues {
        let words: Vec<&str> = cue.text.split_whitespace().collect();
        timed.extend(distribute(&words, cue.start, cue.end));
    }
    timed
}

fn temp_path(tag: &str, extension: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "flectar-tts-{tag}-{}-{unique}.{extension}",
        std::process::id()
    ))
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The stored API keys, one per engine, kept as JSON in a single secret.
pub fn parse_api_keys(stored: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(stored).unwrap_or_default()
}

/// The key stored for this engine inside the JSON secret, if any.
pub fn api_key_from_json(stored: &str, engine: &str) -> Option<String> {
    parse_api_keys(stored)
        .get(engine)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|key| !key.is_empty())
}

/// The JSON secret with this engine's key set, or removed when empty.
pub fn api_key_to_json(stored: &str, engine: &str, key: &str) -> String {
    let mut map = parse_api_keys(stored);
    let key = key.trim();
    if key.is_empty() {
        map.remove(engine);
    } else {
        map.insert(engine.to_owned(), serde_json::Value::String(key.to_owned()));
    }
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_an_id_that_parses_back() {
        for engine in TtsEngine::all() {
            assert_eq!(TtsEngine::parse(engine.id()), engine, "{}", engine.id());
        }
        assert_eq!(TtsEngine::parse(""), TtsEngine::Kokoro);
        assert_eq!(TtsEngine::parse("nonsense"), TtsEngine::Kokoro);
    }

    #[test]
    fn kokoro_labels_read_as_voice_names() {
        assert_eq!(kokoro_label("bf_emma"), "Emma · British female");
        assert_eq!(kokoro_label("am_adam"), "Adam · American male");
        assert_eq!(kokoro_label("weird"), "weird");
    }

    #[test]
    fn edge_labels_name_the_voice_and_locale() {
        assert_eq!(edge_label("ro-RO-EmilNeural"), "Emil · ro-RO");
        assert_eq!(edge_label("en-US-AvaMultilingualNeural"), "Ava · en-US");
    }

    #[test]
    fn voice_falls_back_when_the_engine_does_not_know_it() {
        let mut config = TtsConfig {
            engine: TtsEngine::Edge,
            voice: "bf_emma".into(),
            ..Default::default()
        };
        assert_eq!(config.resolved_voice(), TtsEngine::Edge.default_voice());
        config.voice = "ro-RO-EmilNeural".into();
        assert_eq!(config.resolved_voice(), "ro-RO-EmilNeural");
    }

    #[test]
    fn custom_endpoint_accepts_base_v1_and_full_urls() {
        let mut config = TtsConfig {
            engine: TtsEngine::Custom,
            endpoint: "http://127.0.0.1:8880".into(),
            ..Default::default()
        };
        assert_eq!(
            config.custom_speech_url().unwrap(),
            "http://127.0.0.1:8880/v1/audio/speech"
        );
        config.endpoint = "http://127.0.0.1:8880/v1/".into();
        assert_eq!(
            config.custom_speech_url().unwrap(),
            "http://127.0.0.1:8880/v1/audio/speech"
        );
        config.endpoint = "http://host/v1/audio/speech".into();
        assert_eq!(
            config.custom_speech_url().unwrap(),
            "http://host/v1/audio/speech"
        );
        config.endpoint = "  ".into();
        assert!(config.custom_speech_url().is_err());
    }

    #[test]
    fn timings_spread_over_the_audio_and_grow_the_clock() {
        let timed = timings("one two three", Some(3.0));
        assert_eq!(timed.len(), 3);
        assert_eq!(timed[0].start, 0.0);
        assert!((timed[2].end - 3.0).abs() < 1e-9);
        assert!(timed[1].start >= timed[0].end - 1e-9);
        assert!(timings("one", None).is_empty());
        assert!(timings("   ", Some(1.0)).is_empty());
    }

    #[test]
    fn srt_cues_become_word_times() {
        let srt = "1\n00:00:00,050 --> 00:00:02,050\nBună ziua\n\n2\n00:00:02,100 --> 00:00:04,000\ndomnului\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start - 0.05).abs() < 1e-9);
        assert!((cues[1].start - 2.1).abs() < 1e-9);
        let timed = cue_timings(&cues);
        assert_eq!(timed.len(), 3);
        assert_eq!(timed[0].text, "Bună");
        assert!(timed[2].end <= 4.0001);
    }

    #[test]
    fn duration_reads_the_wav_header() {
        let pcm = vec![0u8; 24_000 * 2];
        let wav = pcm_to_wav(&pcm, 24_000, 1);
        let seconds = wav_duration(&wav).unwrap();
        assert!((seconds - 1.0).abs() < 1e-6);
        assert!(wav_duration(b"not a wav").is_none());
        assert_eq!(wav_duration(&pcm_to_wav(&[], 24_000, 1)), Some(0.0));
    }

    #[test]
    fn api_keys_round_trip_as_json() {
        let stored = api_key_to_json("", "elevenlabs", " secret ");
        assert_eq!(
            api_key_from_json(&stored, "elevenlabs").as_deref(),
            Some("secret")
        );
        let stored = api_key_to_json(&stored, "custom", "other");
        assert_eq!(
            api_key_from_json(&stored, "elevenlabs").as_deref(),
            Some("secret")
        );
        assert_eq!(
            api_key_from_json(&stored, "custom").as_deref(),
            Some("other")
        );
        let stored = api_key_to_json(&stored, "custom", "");
        assert_eq!(api_key_from_json(&stored, "custom"), None);
        assert_eq!(api_key_from_json("not json", "custom"), None);
    }

    #[test]
    fn piper_voices_are_the_models_on_disk() {
        let dir = std::env::temp_dir().join(format!("flectar-piper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("en_US-lessac-medium.onnx"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let found = piper_voices(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "en_US-lessac-medium.onnx");
        assert_eq!(found[0].label, "en US-lessac-medium");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Speaks one line through every engine this machine has, so a broken
    /// service or a changed voice id is caught without opening the app.
    ///
    /// Needs the local services, edge-tts, ffmpeg and an ElevenLabs key:
    ///   FLECTAR_TTS_LIVE=1 cargo test -p flectar-mail-core --lib live_engines \
    ///     -- --ignored --nocapture
    /// A custom endpoint is tried only when FLECTAR_TTS_CUSTOM_URL is set.
    #[tokio::test]
    #[ignore = "needs the local speech services and network access"]
    async fn live_engines_speak() {
        if std::env::var("FLECTAR_TTS_LIVE").is_err() {
            return;
        }
        let text = "One two three, the reader follows along.";
        let elevenlabs_key = std::env::var("FLECTAR_TTS_KEY")
            .ok()
            .or_else(|| {
                std::fs::read_to_string(std::env::var("HOME").ok()? + "/.config/elevenlabs/api_key")
                    .ok()
                    .map(|key| key.trim().to_owned())
            })
            .filter(|key| !key.is_empty());
        let custom_url = std::env::var("FLECTAR_TTS_CUSTOM_URL").ok();
        for engine in TtsEngine::all() {
            if engine == TtsEngine::ElevenLabs && elevenlabs_key.is_none() {
                eprintln!("skipping ElevenLabs: no key");
                continue;
            }
            if engine == TtsEngine::Custom && custom_url.is_none() {
                eprintln!("skipping custom: no FLECTAR_TTS_CUSTOM_URL");
                continue;
            }
            let mut config = TtsConfig {
                engine,
                voice: engine.default_voice().to_owned(),
                ..Default::default()
            };
            if engine == TtsEngine::ElevenLabs {
                config.api_key = elevenlabs_key.clone().unwrap_or_default();
            }
            if engine == TtsEngine::Custom {
                config.endpoint = custom_url.clone().unwrap_or_default();
            }
            let synthesis = synthesize(&config, text)
                .await
                .unwrap_or_else(|error| panic!("{}: {error}", engine.id()));
            let duration = synthesis.duration().expect("audio has a duration");
            eprintln!(
                "{} ({}): {:.2}s of audio, {} timed words",
                engine.id(),
                config.resolved_voice(),
                duration,
                synthesis.words.len()
            );
            assert!(duration > 0.2, "{} produced almost no audio", engine.id());
            assert!(
                !synthesis.words.is_empty(),
                "{} produced no word timing",
                engine.id()
            );
        }
    }

    #[test]
    fn every_engine_except_custom_offers_voices() {
        for engine in TtsEngine::all() {
            let config = TtsConfig {
                engine,
                ..Default::default()
            };
            let found = voices(&config);
            if engine == TtsEngine::Custom {
                assert!(found.is_empty());
            } else {
                assert!(!found.is_empty(), "{} has no voices", engine.id());
                assert!(
                    found.iter().any(|voice| voice.id == engine.default_voice()),
                    "{} default voice is not listed",
                    engine.id()
                );
            }
        }
    }
}
