//! Read-aloud: speak the open message with the engine chosen in Settings and
//! move the highlight through the words as they are heard.
//!
//! One generation counter guards against overlapping requests: pressing the
//! button again while a request is still in flight supersedes it, and a
//! playback that is replaced is stopped (and its WAV removed) on drop.
//! Pause is SIGSTOP on the player; the timing clock stops with it.

use std::collections::HashMap;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use flectar_mail_core::models::Settings;
use flectar_mail_core::tts::{self, TimedWord, TtsConfig};

/// One spoken word: when it is heard, and which bytes of the document it is.
#[derive(Clone, Debug, PartialEq)]
pub struct SpokenWord {
    pub start: f64,
    pub end: f64,
    pub doc_start: usize,
    pub doc_end: usize,
}

/// The engine and voice read-aloud uses, kept in step with Settings.
static CONFIG: OnceLock<RwLock<TtsConfig>> = OnceLock::new();

/// The failure of the last request, waiting to be shown in the reader.
static LAST_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn error_slot() -> &'static Mutex<Option<String>> {
    LAST_ERROR.get_or_init(|| Mutex::new(None))
}

/// Takes the last read-aloud failure, so it is shown once. A silent failure
/// would leave a chosen engine looking like a dead button.
pub fn take_error() -> Option<String> {
    error_slot().lock().ok().and_then(|mut slot| slot.take())
}

fn config_slot() -> &'static RwLock<TtsConfig> {
    CONFIG.get_or_init(|| RwLock::new(TtsConfig::default()))
}

/// Applies the settings the reader speaks with. The next read-aloud request
/// picks the engine up; anything already speaking is left alone.
pub fn set_config(config: TtsConfig) {
    if let Ok(mut slot) = config_slot().write() {
        *slot = config;
    }
}

/// The engine, voice and addresses a stored settings row describes. A stored
/// voice the engine does not offer is left for the caller to resolve.
pub fn config_from_settings(settings: &Settings) -> TtsConfig {
    let configured = settings.tts_voices_dir.trim();
    TtsConfig {
        engine: tts::TtsEngine::parse(&settings.tts_engine),
        voice: settings.tts_voice.clone(),
        kokoro_url: tts::KOKORO_DEFAULT_URL.to_owned(),
        endpoint: settings.tts_endpoint.clone(),
        model: settings.tts_model.clone(),
        api_key: String::new(),
        voices_dir: if configured.is_empty() {
            tts::default_voices_dir()
        } else {
            std::path::PathBuf::from(configured)
        },
    }
}

/// The voices Settings is currently showing, by the label it shows them with.
/// A list of choices reports the picked label, not the id, so the id has to be
/// looked back up here.
static VOICE_LABELS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn voice_labels() -> &'static RwLock<HashMap<String, String>> {
    VOICE_LABELS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Records the labels Settings is about to show for the chosen engine.
pub fn set_voice_labels(pairs: impl IntoIterator<Item = (String, String)>) {
    if let Ok(mut labels) = voice_labels().write() {
        labels.clear();
        labels.extend(pairs);
    }
}

/// The engine voice id behind a label Settings showed, if that label is one
/// of the voices currently listed.
pub fn voice_id_for_label(label: &str) -> Option<String> {
    voice_labels()
        .read()
        .ok()?
        .get(label.trim())
        .cloned()
}

/// The clock currently speaking with.
pub fn config() -> TtsConfig {
    config_slot()
        .read()
        .map(|config| config.clone())
        .unwrap_or_default()
}

struct Playback {
    child: Child,
    wav: PathBuf,
    began: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
    words: Vec<SpokenWord>,
}

impl Playback {
    fn elapsed(&self) -> f64 {
        let now = Instant::now();
        let base = self.paused_total
            + self
                .paused_at
                .map(|at| now.saturating_duration_since(at))
                .unwrap_or_default();
        (now.saturating_duration_since(self.began) - base).as_secs_f64()
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.wav);
    }
}

#[derive(Default)]
struct Inner {
    generation: u64,
    pending: bool,
    playback: Option<Playback>,
}

/// Handle the UI polls while a message is being read.
#[derive(Clone, Default)]
pub struct Speech {
    inner: Arc<Mutex<Inner>>,
}

impl Speech {
    /// True while a request is in flight or audio is playing.
    pub fn is_active(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.pending || inner.playback.is_some())
            .unwrap_or(false)
    }

    pub fn is_paused(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .playback
                    .as_ref()
                    .is_some_and(|playback| playback.paused_at.is_some())
            })
            .unwrap_or(false)
    }

    /// The word being spoken right now, if any.
    pub fn current_word(&self) -> Option<SpokenWord> {
        let mut inner = self.inner.lock().ok()?;
        let playback = inner.playback.as_mut()?;
        let finished =
            playback.paused_at.is_none() && matches!(playback.child.try_wait(), Ok(Some(_)));
        if finished {
            inner.playback = None;
            return None;
        }
        let elapsed = playback.elapsed();
        playback
            .words
            .iter()
            .rev()
            .find(|word| word.start <= elapsed)
            .cloned()
    }

    /// Freeze the player and the highlight clock in place.
    pub fn pause(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(playback) = inner.playback.as_mut() else {
            return;
        };
        if playback.paused_at.is_some() {
            return;
        }
        let stopped = unsafe { libc::kill(playback.child.id() as i32, libc::SIGSTOP) } == 0;
        if stopped {
            playback.paused_at = Some(Instant::now());
        }
    }

    /// Continue a paused playback where it stopped.
    pub fn resume(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(playback) = inner.playback.as_mut() else {
            return;
        };
        let Some(paused_at) = playback.paused_at.take() else {
            return;
        };
        let resumed = unsafe { libc::kill(playback.child.id() as i32, libc::SIGCONT) } == 0;
        if resumed {
            playback.paused_total += Instant::now().saturating_duration_since(paused_at);
        } else {
            playback.paused_at = Some(paused_at);
        }
    }

    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.generation = inner.generation.wrapping_add(1);
            inner.pending = false;
            // Dropping the playback kills the player and removes its WAV.
            inner.playback = None;
        }
    }

    /// Synthesize text from from_word and start playing it.
    ///
    /// words are byte ranges of every word in text. Any in-flight request or
    /// playing audio is superseded.
    pub fn start(
        &self,
        runtime: &tokio::runtime::Handle,
        text: String,
        words: Vec<(usize, usize)>,
        from_word: usize,
    ) {
        let Some(&(start_byte, _)) = words.get(from_word) else {
            return;
        };
        let generation = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            inner.generation = inner.generation.wrapping_add(1);
            inner.pending = true;
            inner.playback = None;
            inner.generation
        };
        let spoken: String = text[start_byte..].to_string();
        let base = start_byte;
        let tail: Vec<(usize, usize)> = words[from_word..]
            .iter()
            .map(|(start, end)| (start - base, end - base))
            .collect();
        let speech = self.clone();
        runtime.spawn(async move {
            let config = config();
            let synthesis = match tts::synthesize(&config, &spoken).await {
                Ok(synthesis) => synthesis,
                Err(error) => {
                    tracing::warn!(
                        engine = config.engine.id(),
                        %error,
                        "read-aloud could not synthesize"
                    );
                    if let Ok(mut slot) = error_slot().lock() {
                        *slot = Some(error.to_string());
                    }
                    speech.finish_request(generation, None);
                    return;
                }
            };
            let path = std::env::temp_dir()
                .join(format!("flectar-tts-{}-{}.wav", std::process::id(), generation));
            let write = || -> std::io::Result<()> {
                use std::io::Write as _;
                let mut file = BufWriter::new(std::fs::File::create(&path)?);
                file.write_all(&synthesis.wav)?;
                file.flush()
            };
            if write().is_err() {
                speech.finish_request(generation, None);
                return;
            }
            let aligned: Vec<SpokenWord> = align_words(&synthesis.words, &tail, &spoken)
                .into_iter()
                .map(|word| SpokenWord {
                    doc_start: word.doc_start + base,
                    doc_end: word.doc_end + base,
                    ..word
                })
                .collect();
            let mut player = Command::new("pw-play");
            player
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // A player that outlives Flectar keeps its stream half-open in the
            // stream-init state, and enough of those stop every later stream
            // from activating: the reader then speaks into silence while the
            // player waits forever. Tying the player's life to this process
            // leaves nothing behind when Flectar crashes or is killed.
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::process::CommandExt as _;
                // SAFETY: the closure only calls an async-signal-safe system
                // call between fork and exec.
                unsafe {
                    player.pre_exec(|| {
                        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                        Ok(())
                    });
                }
            }
            let Ok(child) = player.spawn() else {
                let _ = std::fs::remove_file(&path);
                speech.finish_request(generation, None);
                return;
            };
            speech.finish_request(
                generation,
                Some(Playback {
                    child,
                    wav: path,
                    began: Instant::now(),
                    paused_at: None,
                    paused_total: Duration::ZERO,
                    words: aligned,
                }),
            );
        });
    }

    /// Install a finished request unless a newer one superseded it.
    fn finish_request(&self, generation: u64, playback: Option<Playback>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.generation != generation {
            // Superseded: dropping playback kills the player it just started.
            drop(playback);
            return;
        }
        inner.pending = false;
        inner.playback = playback;
    }
}

/// Speak a short line outside the reader: the Settings sample and the new-mail
/// announcement share one player, so they cannot talk over each other.
fn speak_short(runtime: &tokio::runtime::Handle, text: &str) {
    static SHORT: OnceLock<Speech> = OnceLock::new();
    let speech = SHORT.get_or_init(Speech::default).clone();
    speech.start(runtime, text.to_owned(), word_ranges(text), 0);
}

/// Speak a short sample with the current settings, without touching the
/// reader's highlight. Settings uses this to let a voice be heard.
pub fn preview(runtime: &tokio::runtime::Handle, text: &str) {
    speak_short(runtime, text);
}

/// Say that new mail arrived, with the engine chosen in Settings.
pub fn announce(runtime: &tokio::runtime::Handle, text: &str) {
    speak_short(runtime, text);
}

/// Map each spoken word onto the document word it came from.
///
/// Engines may merge or split tokens (42 -> forty-two), so walk both lists and
/// fall back to the next unclaimed document word.
fn align(timed: &[(String, f64, f64)], words: &[(usize, usize)], text: &str) -> Vec<SpokenWord> {
    let mut mapped = Vec::with_capacity(timed.len());
    let mut cursor = 0usize;
    for (word, start, end) in timed {
        let needle = normalize(word);
        if needle.is_empty() {
            continue;
        }
        let mut found = None;
        let mut probe = cursor;
        while probe < words.len() {
            let (s, e) = words[probe];
            let doc = normalize(&text[s..e]);
            if doc == needle || doc.starts_with(&needle) || needle.starts_with(&doc) {
                found = Some(probe);
                break;
            }
            if probe.saturating_sub(cursor) > 4 {
                break;
            }
            probe += 1;
        }
        let probe = match found {
            Some(index) => index,
            None => {
                if cursor >= words.len() {
                    continue;
                }
                cursor
            }
        };
        let (s, e) = words[probe];
        mapped.push(SpokenWord {
            start: *start,
            end: *end,
            doc_start: s,
            doc_end: e,
        });
        cursor = (probe + 1).min(words.len());
    }
    mapped
}

/// Same as align, taking the engine's own timed words.
fn align_words(timed: &[TimedWord], words: &[(usize, usize)], text: &str) -> Vec<SpokenWord> {
    let pairs: Vec<(String, f64, f64)> = timed
        .iter()
        .map(|word| (word.text.clone(), word.start, word.end))
        .collect();
    align(&pairs, words, text)
}

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Byte ranges of every whitespace-separated word in text.
pub fn word_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                ranges.push((s, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(s) = start {
        ranges.push((s, text.len()));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_ranges_split_on_whitespace() {
        let text = "Hello  world\nagain";
        assert_eq!(word_ranges(text), vec![(0, 5), (7, 12), (13, 18)]);
    }

    #[test]
    fn align_maps_by_order_with_punctuation() {
        let text = "Bună ziua, domnule.";
        let words = word_ranges(text);
        let spoken = vec![
            ("Bună".to_string(), 0.0, 0.4),
            ("ziua,".to_string(), 0.4, 0.8),
            ("domnule.".to_string(), 0.8, 1.2),
        ];
        let mapped = align(&spoken, &words, text);
        assert_eq!(mapped.len(), 3);
        assert_eq!(&text[mapped[1].doc_start..mapped[1].doc_end], "ziua,");
    }

    #[test]
    fn timed_words_align_through_the_engine_types() {
        let text = "One two three";
        let words = word_ranges(text);
        let timed = tts::timings(text, Some(3.0));
        let mapped = align_words(&timed, &words, text);
        assert_eq!(mapped.len(), 3);
        assert_eq!(&text[mapped[2].doc_start..mapped[2].doc_end], "three");
        assert!(mapped[0].end <= mapped[1].end);
    }

    #[test]
    fn voice_labels_map_back_to_ids() {
        set_voice_labels([
            ("Blackbeard · gritty pirate".to_owned(), "6VgigPFWF0sNZy1BthVg".to_owned()),
            ("Adam · dominant, firm".to_owned(), "pNInz6obpgDQGcFmaJgB".to_owned()),
        ]);
        assert_eq!(
            voice_id_for_label("Blackbeard · gritty pirate").as_deref(),
            Some("6VgigPFWF0sNZy1BthVg")
        );
        assert_eq!(
            voice_id_for_label("  Adam · dominant, firm ").as_deref(),
            Some("pNInz6obpgDQGcFmaJgB")
        );
        assert_eq!(voice_id_for_label("a typed custom voice"), None);
        set_voice_labels(Vec::new());
    }

    #[test]
    fn the_default_configuration_is_kokoro() {
        let config = config();
        assert_eq!(config.engine, tts::TtsEngine::Kokoro);
        set_config(TtsConfig::default());
    }
}
