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
use flectar_mail_core::Scenario;
use flectar_mail_core::services::{
    AiModelStatus, AiStatus, ServiceState, ServicesReport, SpeechStatus,
};
use std::sync::Mutex;

/// The model list the endpoint returned last, newest first in time. The
/// Settings page shows it under the model field.
static AI_CATALOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// AI usability per feature, as last probed. A pointer, not a flag: each AI
/// feature runs on its own model tier, and a model the endpoint does not
/// serve disables only the features routed to it. `None` until the first
/// probe lands, so an existing setup is not disabled while it is in flight.
static AI_STATUS: Mutex<Option<AiStatus>> = Mutex::new(None);

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
    let services = app.global::<AppServices>();
    services.set_checking(true);
    let catalog = ai_catalog();
    if !catalog.is_empty() {
        services.set_ai_catalog(catalog.into());
    }
    let Some(core) = state.borrow().core.as_ref().map(|core| core.file_core()) else {
        publish_unavailable(app);
        return;
    };
    let weak = app.as_weak();
    runtime.spawn(async move {
        let report = core.probe_services().await;
        // The probe just recorded what the endpoint serves; show it in
        // Settings so a missing model can be picked from what is there.
        let listed = match &report {
            Ok(report) if report.ai.answers() => core.ai_list_models().await.ok(),
            _ => None,
        };
        let _ = weak.upgrade_in_event_loop(move |app| {
            if let Some(models) = listed {
                publish_ai_catalog(&app, models);
            }
            match report {
                Ok(report) => publish(&app, &report),
                Err(error) => {
                    tracing::warn!(%error, "optional service probe could not read settings");
                    publish_unavailable(&app);
                }
            }
        });
    });
}

fn publish(app: &AppWindow, report: &ServicesReport) {
    if let Ok(mut slot) = AI_STATUS.lock() {
        *slot = Some(report.ai.clone());
    }

    let services = app.global::<AppServices>();
    services.set_checking(false);
    services.set_reacher_enabled(report.reacher_enabled);
    services.set_reacher_state(report.reacher.state.as_str().into());
    publish_ai(&services, &report.ai);
    publish_speech(&services, &report.speech);

    crate::composer::set_address_checks_available(report.reacher.state == ServiceState::Connected);
    // Read-aloud speaks whenever an engine can, fallback included; the card
    // is what says which engine is doing the speaking.
    crate::speech::set_availability(
        report.speech.any_available(),
        report.speech.fallback.clone(),
    );
}

/// One line per configured model: what it is, the features that run on it,
/// and whether the endpoint serves it.
fn model_lines(models: &[AiModelStatus]) -> String {
    models
        .iter()
        .map(|entry| {
            let features = entry.scenario_names();
            let features = if features.is_empty() {
                entry
                    .tiers
                    .iter()
                    .map(|tier| tier.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                features
            };
            let tiers = entry
                .tiers
                .iter()
                .map(|tier| tier.as_str())
                .collect::<Vec<_>>()
                .join("/");
            if entry.available {
                format!("{} ({tiers}): served · {features}", entry.model)
            } else {
                format!("{} ({tiers}): missing · {features}", entry.model)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn publish_ai(services: &AppServices, ai: &AiStatus) {
    services.set_ai_state(ai.state.as_str().into());
    services.set_ai_detail(ai.detail.clone().into());
    services.set_ai_models(model_lines(&ai.models).into());
}

fn publish_speech(services: &AppServices, speech: &SpeechStatus) {
    services.set_speech_state(speech.state.as_str().into());
    services.set_speech_detail(speech.detail().into());
    services.set_speech_selected(speech.selected_engine_label().into());
    let using = speech
        .speaking_engine_label()
        .map(str::to_owned)
        .unwrap_or_default();
    services.set_speech_using(using.into());
}

/// Remember and show the ids the endpoint serves, so Settings can offer them
/// and the AI section can say why a configured model does not work.
fn publish_ai_catalog(app: &AppWindow, models: Vec<String>) {
    if let Ok(mut slot) = AI_CATALOG.lock() {
        *slot = models.clone();
    }
    app.global::<AppServices>()
        .set_ai_catalog(models.join("\n").into());
}

fn publish_unavailable(app: &AppWindow) {
    if let Ok(mut slot) = AI_STATUS.lock() {
        *slot = None;
    }
    let services = app.global::<AppServices>();
    services.set_checking(false);
    services.set_reacher_state("not-configured".into());
    services.set_ai_state("not-configured".into());
    services.set_ai_detail(String::new().into());
    services.set_ai_models(String::new().into());
    services.set_speech_state("not-configured".into());
    services.set_speech_detail(String::new().into());
    services.set_speech_selected(String::new().into());
    services.set_speech_using(String::new().into());
    crate::composer::set_address_checks_available(false);
    crate::speech::set_availability(false, None);
}

/// The models the endpoint served the last time it was asked, as one per
/// line. Empty when it never answered or listed nothing.
pub(crate) fn ai_catalog() -> String {
    let guard = match AI_CATALOG.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.join("\n")
}

pub(crate) fn security_screen_unavailable() -> bool {
    ai_unavailable_for(Scenario::Security)
}

/// True when the last probe proved the AI endpoint cannot run `scenario`,
/// because it is down, unconfigured, or does not serve the feature's model.
/// Unknown is allowed through, so a slow probe does not disable a setup that
/// works. A feature whose model is served stays enabled when another tier's
/// model is missing.
pub(crate) fn ai_unavailable_for(scenario: Scenario) -> bool {
    // A poisoned lock only means another thread panicked while publishing;
    // the answer it wrote is still what the last probe found.
    let guard = match AI_STATUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .as_ref()
        .is_some_and(|status| !status.scenario_usable(scenario))
}
