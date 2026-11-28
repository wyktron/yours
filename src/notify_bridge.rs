//! Tuta's mail notifications, read aloud.
//!
//! Tuta has no IMAP, so Yours cannot sync it. What it does have is a desktop
//! app that shows one system notification per new message, and every such
//! notification reaches the session bus as a method call to
//! \`org.freedesktop.Notifications.Notify\`. This module listens for those
//! calls, keeps the ones that come from a Tuta application, and reads a short
//! line aloud through the same engine and quiet-hours gate as new mail.
//!
//! Nothing here is part of the mail store: the bridge is a notification
//! listener, not a sync. A machine with no session bus (a headless test run,
//! a sandbox that hides it) leaves an inert bridge that Settings and the
//! services probe report as "Tuta notifications: not available".

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{AppServices, AppWindow};
use chrono::NaiveTime;
use flectar_mail_core::{Core, models::Settings, quiet_hours};
use futures_util::StreamExt;
use slint::ComponentHandle as _;

/// Applications whose notifications are read aloud by default. Matching is
/// case-insensitive and open-ended, so "Tutanota", "tutanota-desktop" and the
/// Flatpak's "com.tutanota.Tutanota" all match the first entry.
const DEFAULT_APPS: &[&str] = &["tutanota", "tuta", "tuta-mail"];

/// Yours' own notifications never come back through the voice.
const OWN_APPS: &[&str] = &["yours", "flectar"];

/// A repeated notification is read once: notification clients may send the
/// same line twice for one message, and a replace arrives moments later.
const DEDUPE_WINDOW: Duration = Duration::from_secs(4);

/// Longest line quoted from a notification, in characters.
const MAX_SPOKEN_CHARS: usize = 120;

/// A summary longer than this, with a newline, or ending in a sentence mark
/// reads as a subject rather than a sender name; the app name is used then.
const MAX_SENDER_CHARS: usize = 60;
const MAX_SENDER_WORDS: usize = 5;

/// How long to wait before trying the bus again after it refused or closed.
const BUS_RETRY: Duration = Duration::from_secs(20);

/// What Settings and the services probe show about the bridge.
#[derive(Clone, Debug)]
pub(crate) struct Status {
    /// "checking", "connected" or "unavailable".
    pub(crate) state: &'static str,
    pub(crate) detail: String,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    state: "checking",
    detail: String::new(),
});

/// The window Settings is showing, so a status change can be painted without
/// waiting for another probe.
static APP: Mutex<Option<slint::Weak<AppWindow>>> = Mutex::new(None);

/// The runtime and the store the bridge reads settings through, kept so a
/// notification raised inside Yours' own Tuta web view can be read aloud
/// without going through the session bus.
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();
static CORE: OnceLock<std::sync::Arc<Core>> = OnceLock::new();

/// The embedded window may send the same line twice for one message (a
/// replace arrives moments after the original), and that repeat stays silent.
static WEB_DEDUPE: Mutex<Dedupe> = Mutex::new(Dedupe { recent: Vec::new() });

/// Whether anything has shown a desktop file for a Tuta application.
static DETECTED_FILES: OnceLock<bool> = OnceLock::new();

/// Whether a Tuta-looking application has already notified on this bus.
static OBSERVED_APP: AtomicBool = AtomicBool::new(false);

pub(crate) fn status() -> Status {
    STATUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Paint the bridge's state into the Settings globals that show it.
pub(crate) fn publish(app: &AppWindow) {
    let status = status();
    let services = app.global::<AppServices>();
    services.set_tuta_state(status.state.into());
    services.set_tuta_detail(status.detail.into());
}

fn set_status(state: &'static str, detail: impl Into<String>) {
    let detail = detail.into();
    let changed = {
        let mut slot = STATUS.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.state == state && slot.detail == detail {
            false
        } else {
            slot.state = state;
            slot.detail = detail.clone();
            true
        }
    };
    if !changed {
        return;
    }
    if state == "connected" {
        eprintln!("notify bridge: listening for org.freedesktop.Notifications.Notify on the session bus");
    } else {
        eprintln!("notify bridge: Tuta notifications: not available ({detail})");
    }
    if let Some(weak) = APP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        let _ = weak.upgrade_in_event_loop(|app| publish(&app));
    }
}

/// Start listening, once the mail store and the runtime are up. A bridge that
/// cannot reach a bus keeps retrying, so a desktop bus that starts late is
/// picked up without restarting Yours.
pub(crate) fn start(
    runtime: &tokio::runtime::Handle,
    core: std::sync::Arc<Core>,
    app: slint::Weak<AppWindow>,
) {
    if let Ok(mut slot) = APP.lock() {
        *slot = Some(app);
    }
    // The embedded Tuta window's own notifications are announced in this
    // process too (src/tuta_window.rs forwards them here), so it needs the
    // same runtime and store.
    let _ = RUNTIME.set(runtime.clone());
    let _ = CORE.set(std::sync::Arc::clone(&core));
    let runtime = runtime.clone();
    runtime.clone().spawn(async move {
        loop {
            match connect().await {
                Ok(mut messages) => {
                    set_status(
                        "connected",
                        "Tuta notifications are read aloud as they arrive.",
                    );
                    let mut dedupe = Dedupe::default();
                    while let Some(message) = messages.next().await {
                        let Ok(message) = message else {
                            break;
                        };
                        let Some(notification) = parse(&message) else {
                            continue;
                        };
                        handle(&runtime, &core, &mut dedupe, notification).await;
                    }
                    set_status("unavailable", "the session bus connection ended");
                }
                Err(error) => set_status("unavailable", error),
            }
            tokio::time::sleep(BUS_RETRY).await;
        }
    });
}


/// Read a notification raised by the Tuta web app embedded in Yours aloud,
/// with the same switch, quiet hours, sender extraction and dedupe as the
/// notifications seen on the session bus.
///
/// The embedded page's notifications never reach the bus: WebKitGTK hands
/// them to the window, which forwards the line here. So the bus listener
/// above keeps covering the desktop Tuta application, while this entry covers
/// Yours' own web view. `summary` is the notification's title, usually the
/// sender, and `body` its text.
///
/// Returns true when the line was handed to the speech engine (or was a
/// repeated copy). False means the notification was not spoken, and the
/// caller should let WebKitGTK show it normally instead.
pub fn announce_web_notification(summary: &str, body: &str) -> bool {
    let (Some(runtime), Some(core)) = (RUNTIME.get(), CORE.get()) else {
        return false;
    };
    let Ok(settings) = runtime.block_on(core.get_settings()) else {
        return false;
    };
    if !announces_with(
        &settings,
        chrono::Local::now().time(),
        detected(),
        crate::speech::available(),
    ) {
        eprintln!(
            "tuta web app: staying silent for {summary:?} (announcing is off, quiet hours, or no speech engine)"
        );
        return false;
    }
    if !WEB_DEDUPE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .accept(("tuta-web-app", summary, body), Instant::now())
    {
        // The same line again is the same message; keep it off screen too.
        return true;
    }
    let sender = sender_from("Tuta", summary);
    let line = spoken_line(&sender);
    // The voice is inaudible in a log, so the line is printed: that is how a
    // live check proves the embedded page's notification was read.
    eprintln!("tuta web app: read aloud: {line}");
    crate::speech::announce(runtime, &line);
    true
}

/// One Notify call worth looking at.
struct Notification {
    app_name: String,
    desktop_entry: Option<String>,
    summary: String,
    body: String,
}

/// Read the fields the bridge uses from a Notify call. The body signature is
/// "susssasa{sv}i": app name, replaces id, icon, summary, body, actions,
/// hints and timeout.
fn parse(message: &zbus::Message) -> Option<Notification> {
    let header = message.header();
    if header.interface().map(|name| name.as_str()) != Some("org.freedesktop.Notifications") {
        return None;
    }
    if header.member().map(|name| name.as_str()) != Some("Notify") {
        return None;
    }
    type NotifyBody = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, zbus::zvariant::OwnedValue>,
        i32,
    );
    let (app_name, _, _, summary, body, _, hints, _) =
        message.body().deserialize::<NotifyBody>().ok()?;
    // The hint names the desktop entry the notification belongs to, which is
    // what a Flatpak's portal keeps while the app name may be empty.
    let desktop_entry = hints
        .get("desktop-entry")
        .and_then(|value| <&str>::try_from(value).ok())
        .map(str::to_owned);
    Some(Notification {
        app_name,
        desktop_entry,
        summary,
        body,
    })
}

/// Decide what to do with one notification: mark a Tuta app as detected, and
/// read the line aloud when the settings, the hour and the engine allow it.
async fn handle(
    runtime: &tokio::runtime::Handle,
    core: &Core,
    dedupe: &mut Dedupe,
    notification: Notification,
) {
    let Notification {
        app_name,
        desktop_entry,
        summary,
        body,
    } = notification;
    let defaults = configured_apps("");
    let named_tuta = name_matches(&app_name, &defaults)
        || desktop_entry
            .as_deref()
            .is_some_and(|entry| name_matches(entry, &defaults));
    if named_tuta {
        OBSERVED_APP.store(true, Ordering::Relaxed);
    }
    let Ok(settings) = core.get_settings().await else {
        return;
    };
    if !qualifies(
        &app_name,
        desktop_entry.as_deref(),
        &body,
        &settings.tuta_extra_apps,
    ) {
        return;
    }
    if !announces_with(
        &settings,
        chrono::Local::now().time(),
        detected(),
        crate::speech::available(),
    ) {
        // A line here is how a live check proves the bridge saw a Tuta
        // notification and chose silence: switch off, quiet hours, or no engine.
        eprintln!(
            "notify bridge: staying silent for a notification from {app_name:?} (announcing is off, quiet hours, or no speech engine)"
        );
        return;
    }
    if !dedupe.accept(
        (&app_name, &summary, &body),
        Instant::now(),
    ) {
        return;
    }
    let sender = sender_from(&app_name, &summary);
    let line = spoken_line(&sender);
    // The voice is inaudible in a log, so the line and the app that caused it
    // are printed: that is how a live check proves the bridge spoke.
    eprintln!("notify bridge: read aloud: {line} (app {app_name:?})");
    crate::speech::announce(runtime, &line);
}

/// Whether the bridge is switched on, either by the saved choice or by a
/// detected Tuta app on a machine where the user never chose.
pub(crate) fn effective_enabled(settings: &Settings) -> bool {
    enabled_with(settings, detected())
}

fn enabled_with(settings: &Settings, detected: bool) -> bool {
    settings.tuta_notifications.unwrap_or(detected)
}

/// Whether a Tuta application has been seen: on the bus, or as a desktop file.
pub(crate) fn detected() -> bool {
    *DETECTED_FILES.get_or_init(desktop_files_mention_tuta) || OBSERVED_APP.load(Ordering::Relaxed)
}

/// Whether a notification may be spoken right now: the switch, detection, the
/// quiet hours and a working speech engine. The last two are shared with the
/// new-mail announcement, so the two voices obey one rule.
fn announces_with(
    settings: &Settings,
    now: NaiveTime,
    detected: bool,
    speech_ready: bool,
) -> bool {
    enabled_with(settings, detected)
        && speech_ready
        && !quiet_hours::is_quiet_at(now, settings)
}

/// Whether a Notify call is one the bridge reads aloud. Empty bodies, Yours'
/// own notifications and anything whose app does not match are skipped.
fn qualifies(app_name: &str, desktop_entry: Option<&str>, body: &str, extra: &str) -> bool {
    if body.trim().is_empty() {
        return false;
    }
    if is_own_app(app_name) || desktop_entry.is_some_and(is_own_app) {
        return false;
    }
    let apps = configured_apps(extra);
    name_matches(app_name, &apps)
        || desktop_entry.is_some_and(|entry| name_matches(entry, &apps))
}

/// The default applications plus the comma-separated list from Settings, all
/// lowercased. Empty entries are ignored and duplicates kept out.
fn configured_apps(extra: &str) -> Vec<String> {
    let mut apps: Vec<String> = DEFAULT_APPS.iter().map(|name| (*name).to_owned()).collect();
    for name in extra.split(',') {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !apps.contains(&name) {
            apps.push(name);
        }
    }
    apps
}

fn is_own_app(name: &str) -> bool {
    let name = name.trim().to_lowercase();
    !name.is_empty() && OWN_APPS.iter().any(|own| name.contains(own))
}

/// Whether a notification's app name, or its desktop-entry hint, names one of
/// the configured applications. Names are matched as substrings so a flatpak
/// id such as "com.tutanota.Tutanota" matches "tutanota".
fn name_matches(name: &str, apps: &[String]) -> bool {
    let name = name.trim().to_lowercase();
    !name.is_empty() && apps.iter().any(|app| name.contains(app.as_str()))
}

/// What the voice calls the sender. A summary that reads like a sender name is
/// used as-is; anything else (a subject, a code, an online-service line) falls
/// back to the application's name rather than reading an arbitrary body.
fn sender_from(app_name: &str, summary: &str) -> String {
    let summary = clean_line(summary, MAX_SPOKEN_CHARS);
    if sender_like(&summary) {
        return summary;
    }
    let app_name = clean_line(app_name, MAX_SPOKEN_CHARS);
    if app_name.is_empty() {
        "Tuta".to_owned()
    } else {
        app_name
    }
}

fn sender_like(summary: &str) -> bool {
    !summary.is_empty()
        && summary.chars().count() <= MAX_SENDER_CHARS
        && summary.split_whitespace().count() <= MAX_SENDER_WORDS
        && !summary.ends_with(['.', '!', '?', ':', ';'])
        // A code, a date or a price is a subject or a body line, not a name.
        && !summary.chars().any(|character| character.is_ascii_digit())
        && !summary.contains("http://")
        && !summary.contains("https://")
}

fn spoken_line(sender: &str) -> String {
    if sender.trim().is_empty() {
        "You've got email.".to_owned()
    } else {
        format!("You've got email from {sender}.")
    }
}

/// One line of text safe to speak: control characters and newlines become
/// spaces, runs of whitespace collapse, and a long line is cut at a word
/// boundary within \`max_chars\` characters.
fn clean_line(text: &str, max_chars: usize) -> String {
    let mut line = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !line.is_empty();
        } else {
            if pending_space {
                line.push(' ');
                pending_space = false;
            }
            line.push(character);
        }
    }
    let line = line.trim();
    if line.chars().count() <= max_chars {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(max_chars).collect();
    if let Some(space) = cut.rfind(' ') {
        cut.truncate(space);
    }
    cut.trim_end().to_owned()
}

/// Notifications said once. An identical line inside the window is a repeat
/// (or a replace), not another message.
#[derive(Default)]
struct Dedupe {
    recent: Vec<((String, String, String), Instant)>,
}

impl Dedupe {
    fn accept(&mut self, key: (&str, &str, &str), now: Instant) -> bool {
        self.recent
            .retain(|(_, at)| now.saturating_duration_since(*at) < DEDUPE_WINDOW);
        let key = (
            key.0.trim().to_lowercase(),
            key.1.trim().to_lowercase(),
            key.2.trim().to_lowercase(),
        );
        if self.recent.iter().any(|(seen, _)| *seen == key) {
            return false;
        }
        self.recent.push((key, now));
        true
    }
}

/// Connect to the session bus and ask to see Notify calls. The preferred way
/// is BecomeMonitor, the read-only view the bus specification defines; a bus
/// that refuses it may still honor the legacy eavesdropping match rule.
async fn connect() -> Result<zbus::MessageStream, String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| format!("no session bus ({error})"))?;
    let rules: &[&str] =
        &["type='method_call',interface='org.freedesktop.Notifications',member='Notify'"];
    let monitored = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus.Monitoring"),
            "BecomeMonitor",
            &(rules, 0u32),
        )
        .await
        .is_ok();
    if !monitored {
        // dbus-daemon still grants eavesdropping on the session bus; some
        // brokers (dbus-broker) refuse the rule, which leaves the bridge inert.
        let rule = format!("{},eavesdrop='true'", rules[0]);
        connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "AddMatch",
                &(rule.as_str(),),
            )
            .await
            .map_err(|error| format!("the session bus refused monitoring ({error})"))?;
    }
    Ok(zbus::MessageStream::from(connection))
}

/// Whether a Tuta application's desktop file is installed. The desktop entry
/// is the detection Settings trusts for its default, alongside what the bridge
/// sees on the bus.
fn desktop_files_mention_tuta() -> bool {
    let mut directories: Vec<PathBuf> = Vec::new();
    match std::env::var_os("XDG_DATA_HOME") {
        Some(home) => directories.push(PathBuf::from(home).join("applications")),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                directories.push(PathBuf::from(home).join(".local/share/applications"));
            }
        }
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    for directory in data_dirs.split(':').filter(|directory| !directory.is_empty()) {
        directories.push(PathBuf::from(directory).join("applications"));
    }
    // A Tuta flatpak exports its entry to the host's flatpak export tree.
    directories.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }
    let names: Vec<String> = directories
        .iter()
        .flat_map(|directory| {
            std::fs::read_dir(directory)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>()
        })
        .collect();
    any_tuta_desktop_name(names.iter().map(String::as_str))
}

fn any_tuta_desktop_name<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    names.into_iter().any(|name| {
        let name = name.to_lowercase();
        name.ends_with(".desktop") && name.contains("tuta")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn settings(tuta: Option<bool>) -> Settings {
        Settings {
            tuta_notifications: tuta,
            // The default window is 22:00-08:00; tests that care set their own.
            quiet_hours: false,
            ..Settings::default()
        }
    }

    #[test]
    fn tuta_applications_are_recognized_by_name_and_hint() {
        assert!(qualifies("Tuta", None, "New email", ""));
        assert!(qualifies("tutanota-desktop", None, "New email", ""));
        assert!(qualifies("Tutanota", None, "New email", ""));
        // A Flatpak or portal passes the app id, not the display name.
        assert!(qualifies("", Some("com.tutanota.Tutanota"), "New email", ""));
        // The app name may be a browser while the hint names the web app.
        assert!(qualifies("Chromium", Some("com.tutanota.Tutanota"), "New email", ""));
        assert!(!qualifies("Thunderbird", None, "New email", ""));
        assert!(!qualifies("", None, "New email", ""));
    }

    #[test]
    fn your_own_notifications_are_ignored() {
        assert!(!qualifies("Yours", None, "You've got email from Ana.", ""));
        assert!(!qualifies("org.yours.mail", None, "New email", ""));
        assert!(!qualifies("Flectar Mail", None, "New email", ""));
        assert!(!qualifies("tutanota", Some("org.yours.mail"), "New email", ""));
    }

    #[test]
    fn notifications_without_a_body_are_ignored() {
        assert!(!qualifies("Tuta", None, "", ""));
        assert!(!qualifies("Tuta", None, "   \n\t", ""));
        assert!(qualifies("Tuta", None, "New email", ""));
    }

    #[test]
    fn extra_application_names_extend_the_list() {
        assert!(qualifies("Signal", None, "New message", "signal"));
        assert!(qualifies("signal-desktop", None, "New message", " Signal , zulip "));
        assert!(qualifies("Zulip", None, "New message", "signal, zulip"));
        // Blank and repeated entries are harmless.
        assert!(qualifies("Tuta", None, "New email", " , , "));
        assert!(!qualifies("Slack", None, "New message", "signal, zulip"));
    }

    #[test]
    fn the_sender_comes_from_a_name_like_summary() {
        assert_eq!(sender_from("Tuta", "Ana"), "Ana");
        assert_eq!(spoken_line(&sender_from("Tuta", "Ana")), "You've got email from Ana.");
        // Whitespace and newlines in a name are flattened.
        assert_eq!(sender_from("Tuta", " Ana\nPopescu "), "Ana Popescu");
    }

    #[test]
    fn a_subject_like_summary_falls_back_to_the_application() {
        assert_eq!(sender_from("Tuta", ""), "Tuta");
        assert_eq!(sender_from("Tuta", "Your invoice is ready."), "Tuta");
        assert_eq!(sender_from("Tuta", "Your verification code is 123456"), "Tuta");
        assert_eq!(
            spoken_line(&sender_from("Signal", "")),
            "You've got email from Signal."
        );
        // With no app name either, the line still names the bridge's subject.
        assert_eq!(sender_from("", ""), "Tuta");
    }

    #[test]
    fn a_long_line_is_cut_at_a_word_boundary() {
        let long = "word ".repeat(60);
        let clean = clean_line(&long, 20);
        assert!(clean.chars().count() <= 20);
        assert!(!clean.ends_with(' '));
        assert!(clean.starts_with("word word"));
        // Newlines and tabs become spaces, so spoken text stays one line.
        assert_eq!(clean_line("first\nsecond\tthird", 120), "first second third");
    }

    #[test]
    fn identical_notifications_are_read_once() {
        let mut seen = Dedupe::default();
        let start = Instant::now();
        assert!(seen.accept(("Tuta", "Ana", "New email"), start));
        assert!(!seen.accept(("Tuta", "Ana", "New email"), start + Duration::from_secs(1)));
        assert!(seen.accept(("Tuta", "Ana", "Another email"), start + Duration::from_secs(1)));
        // The dedupe window is short: a second real message later is read.
        assert!(seen.accept(("Tuta", "Ana", "New email"), start + DEDUPE_WINDOW));
    }

    #[test]
    fn the_switch_detection_and_quiet_hours_decide() {
        // The saved switch has the last word over detection.
        assert!(!announces_with(&settings(Some(false)), at(12, 0), true, true));
        assert!(announces_with(&settings(Some(true)), at(12, 0), false, true));
        // Never chosen: a detected Tuta is on, an undetected machine is off.
        assert!(announces_with(&settings(None), at(12, 0), true, true));
        assert!(!announces_with(&settings(None), at(12, 0), false, true));
        // No engine means no announcement, and the quiet window silences it.
        assert!(!announces_with(&settings(Some(true)), at(12, 0), true, false));
        let quiet = Settings {
            quiet_hours: true,
            ..settings(Some(true))
        };
        assert!(!announces_with(&quiet, at(23, 0), true, true));
        assert!(announces_with(&quiet, at(12, 0), true, true));
    }

    #[test]
    fn a_desktop_file_naming_tuta_detects_the_application() {
        assert!(any_tuta_desktop_name(
            ["com.tutanota.Tutanota.desktop", "firefox.desktop"].into_iter()
        ));
        assert!(any_tuta_desktop_name(
            ["de.tutao.tutanota.desktop"].into_iter()
        ));
        assert!(!any_tuta_desktop_name(
            ["firefox.desktop", "thunderbird.desktop"].into_iter()
        ));
        // A directory entry that is not a desktop file is not detection.
        assert!(!any_tuta_desktop_name(["tutanota.desktop.bak"].into_iter()));
    }
}
