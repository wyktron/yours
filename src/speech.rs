//! Read-aloud: local Kokoro TTS (`/speak_timed`) played through pw-play, with
//! per-word timing the reader uses to move the text highlight.
//!
//! One generation counter guards against overlapping requests: pressing the
//! button again while a request is still in flight supersedes it, and a
//! playback that is replaced is stopped (and its WAV removed) on drop.
//! Pause is `SIGSTOP` on the player; the timing clock stops with it.

use std::io::BufWriter;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// The Kokoro TTS server started by the kokoro-tts user unit.
const ENDPOINT: &str = "http://127.0.0.1:8766/speak_timed";
const DEFAULT_VOICE: &str = "bf_emma";

static VOICE: std::sync::OnceLock<std::sync::RwLock<String>> = std::sync::OnceLock::new();

fn voice() -> String {
    VOICE
        .get_or_init(|| std::sync::RwLock::new(DEFAULT_VOICE.to_owned()))
        .read()
        .map(|value| value.clone())
        .unwrap_or_else(|_| DEFAULT_VOICE.to_owned())
}

/// Sets the Kokoro voice used for read-aloud.
pub fn set_voice(value: &str) {
    let chosen = value.trim();
    let chosen = if chosen.is_empty() { DEFAULT_VOICE } else { chosen };
    if let Ok(mut slot) = VOICE
        .get_or_init(|| std::sync::RwLock::new(DEFAULT_VOICE.to_owned()))
        .write()
    {
        *slot = chosen.to_owned();
    }
}

/// One spoken word: when it is heard, and which bytes of the document it is.
#[derive(Clone, Debug, PartialEq)]
pub struct SpokenWord {
    pub start: f64,
    pub end: f64,
    pub doc_start: usize,
    pub doc_end: usize,
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
        if playback.paused_at.is_none() {
            if let Ok(Some(_)) = playback.child.try_wait() {
                inner.playback = None;
                return None;
            }
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
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(playback) = inner.playback.as_mut() {
                if playback.paused_at.is_none() {
                    let ok = unsafe { libc::kill(playback.child.id() as i32, libc::SIGSTOP) } == 0;
                    if ok {
                        playback.paused_at = Some(Instant::now());
                    }
                }
            }
        }
    }

    /// Continue a paused playback where it stopped.
    pub fn resume(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(playback) = inner.playback.as_mut() {
                if let Some(at) = playback.paused_at.take() {
                    let resumed = unsafe { libc::kill(playback.child.id() as i32, libc::SIGCONT) } == 0;
                    if resumed {
                        playback.paused_total += Instant::now().saturating_duration_since(at);
                    } else {
                        playback.paused_at = Some(at);
                    }
                }
            }
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

    /// Synthesize `text` from `from_word` and start playing it.
    ///
    /// `words` are byte ranges of every word **in `text`**. Any in-flight
    /// request or playing audio is superseded.
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
            let client = reqwest::Client::new();
            let response = client
                .post(ENDPOINT)
                .timeout(Duration::from_secs(120))
                .form(&[("text", spoken.as_str()), ("voice", voice().as_str())])
                .send()
                .await;
            let Ok(response) = response else {
                speech.finish_request(generation, None);
                return;
            };
            let Ok(body) = response.text().await else {
                speech.finish_request(generation, None);
                return;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
                speech.finish_request(generation, None);
                return;
            };
            let Some(wav_b64) = value.get("wav").and_then(|v| v.as_str()) else {
                speech.finish_request(generation, None);
                return;
            };
            let Ok(wav) = base64::engine::general_purpose::STANDARD.decode(wav_b64) else {
                speech.finish_request(generation, None);
                return;
            };
            let timed: Vec<(String, f64, f64)> = value
                .get("words")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            Some((
                                item.get("t")?.as_str()?.to_string(),
                                item.get("s")?.as_f64()?,
                                item.get("e")?.as_f64()?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let path = std::env::temp_dir()
                .join(format!("flectar-tts-{}-{}.wav", std::process::id(), generation));
            let write = || -> std::io::Result<()> {
                use std::io::Write as _;
                let mut file = BufWriter::new(std::fs::File::create(&path)?);
                file.write_all(&wav)?;
                file.flush()
            };
            if write().is_err() {
                speech.finish_request(generation, None);
                return;
            }
            let aligned: Vec<SpokenWord> = align(&timed, &tail, &spoken)
                .into_iter()
                .map(|word| SpokenWord {
                    doc_start: word.doc_start + base,
                    doc_end: word.doc_end + base,
                    ..word
                })
                .collect();
            let Ok(child) = Command::new("pw-play")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
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
            // Superseded: dropping `playback` kills the player it just started.
            drop(playback);
            return;
        }
        inner.pending = false;
        inner.playback = playback;
    }
}

/// Map each spoken word onto the document word it came from.
///
/// Kokoro preserves order but may merge or split tokens (`42` -> `forty-two`),
/// so walk both lists and fall back to the next unclaimed document word.
fn align(spoken: &[(String, f64, f64)], words: &[(usize, usize)], text: &str) -> Vec<SpokenWord> {
    let mut mapped = Vec::with_capacity(spoken.len());
    let mut cursor = 0usize;
    for (word, start, end) in spoken {
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

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Byte ranges of every whitespace-separated word in `text`.
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
}
