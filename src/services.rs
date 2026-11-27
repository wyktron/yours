//! The optional-service capability probe, and the UI state it publishes.
//!
//! The probe itself lives in the core so it can share the HTTP clients and
//! credentials there. This module owns the shell side: one quick run after the
//! store opens, an on-demand run from Settings, and the few static flags the
//! composer, read-aloud and phishing screen read without touching the UI.
//!
//! Network work is spawned on the app runtime after the settings snapshot is
//! read, so it is never on the UI thread and never holds the database lock.

use super::*;
use flectar_mail_core::services::{ServiceState, ServicesReport};
use std::sync::atomic::{AtomicU8, Ordering};

const AVAILABLE: u8 = 1;
const UNAVAILABLE: u8 = 2;

/// The AI state as last probed. Unavailable until the probe proves otherwise,
/// so a first run cannot fail at the first click.
static AI_STATE: AtomicU8 = AtomicU8::new(UNAVAILABLE);

/// Wire the Settings card's buttons before the first probe has run.
pub(crate) fn register(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    let weak = app.as_weak();
    let state = Rc::clone(state);
    let runtime = runtime.clone();
    app.global::<AppServices>().on_check_again(move || {
        if let Some(app) = weak.upgrade() {
            refresh(&app, &state, &runtime);
        }
    });

    let weak = app.as_weak();
    app.global::<AppServices>().on_open_ai_settings(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        app.set_settings_tab("Agents".into());
        app.invoke_open_settings();
    });
}

/// Probe now. Safe to call from startup, from a settings save, or from the
/// card's Check again button; a newer run simply replaces an older answer.
pub(crate) fn refresh(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    app.global::<AppServices>().set_checking(true);
    let Some(core) = state.borrow().core.as_ref().map(|core| core.file_core()) else {
        publish_unavailable(app);
        return;
    };
    let weak = app.as_weak();
    runtime.spawn(async move {
        let report = core.probe_services().await;
        let _ = weak.upgrade_in_event_loop(move |app| match report {
            Ok(report) => publish(&app, &report),
            Err(error) => {
                tracing::warn!(%error, "optional service probe could not read settings");
                publish_unavailable(&app);
            }
        });
    });
}

fn publish(app: &AppWindow, report: &ServicesReport) {
    AI_STATE.store(state_code(report.ai.state), Ordering::Relaxed);

    let services = app.global::<AppServices>();
    services.set_checking(false);
    services.set_reacher_enabled(report.reacher_enabled);
    services.set_reacher_state(report.reacher.state.as_str().into());
    services.set_ai_state(report.ai.state.as_str().into());
    services.set_speech_state(report.speech.state.as_str().into());

    crate::composer::set_address_checks_available(report.reacher.state == ServiceState::Connected);
    crate::speech::set_availability(
        report.speech.any_available(),
        report.speech.fallback.clone(),
    );
}

fn publish_unavailable(app: &AppWindow) {
    AI_STATE.store(UNAVAILABLE, Ordering::Relaxed);
    let services = app.global::<AppServices>();
    services.set_checking(false);
    services.set_reacher_state("not-configured".into());
    services.set_ai_state("not-configured".into());
    services.set_speech_state("not-configured".into());
    crate::composer::set_address_checks_available(false);
    crate::speech::set_availability(false, None);
}

const fn state_code(state: ServiceState) -> u8 {
    match state {
        ServiceState::Connected => AVAILABLE,
        ServiceState::NotConfigured => UNAVAILABLE,
        ServiceState::Unreachable => UNAVAILABLE,
    }
}

/// True only when the last probe proved there is no AI endpoint. Unknown is
/// allowed through, so a slow probe does not disable an existing setup.
pub(crate) fn ai_unavailable() -> bool {
    AI_STATE.load(Ordering::Relaxed) == UNAVAILABLE
}
