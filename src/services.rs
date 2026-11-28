//! The optional-service capability probe, and the UI state it publishes.
//!
//! The probe itself lives in the core so it can share the HTTP clients and
//! credentials there. This module owns the shell side: one quick run after the
//! store opens, an on-demand run from Settings, and the few static flags the
//! composer, read-aloud and phishing screen read without touching the UI.
//!
//! The same run lists the models the AI endpoint serves. Settings turns that
//! list into the AI model pickers, so a configured model the endpoint does not
//! serve can be replaced by one it does, without typing an id from memory.
//!
//! Network work is spawned on the app runtime after the settings snapshot is
//! read, so it is never on the UI thread and never holds the database lock.

use super::*;
use flectar_mail_core::Scenario;
use flectar_mail_core::models::Settings;
use flectar_mail_core::services::{
    AiModelStatus, AiStatus, AiTier, AiTierModels, ServiceState, ServicesReport, SpeechStatus,
};
use std::sync::Mutex;

/// The model list the endpoint returned last. Kept across a probe that did not
/// answer, so the pickers keep offering what was there rather than going
/// blank on a transient outage.
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
    let state_for_check = Rc::clone(state);
    let runtime_for_check = runtime.clone();
    app.global::<AppServices>().on_check_again(move || {
        if let Some(app) = weak.upgrade() {
            refresh(&app, &state_for_check, &runtime_for_check);
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

    // The AI section's own refresh, so a model picked in another window or a
    // server that just changed its list can be re-read without leaving it.
    let weak = app.as_weak();
    let state_for_refresh = Rc::clone(&state);
    let runtime_for_refresh = runtime.clone();
    app.global::<AiModels>().on_refresh(move || {
        if let Some(app) = weak.upgrade() {
            refresh(&app, &state_for_refresh, &runtime_for_refresh);
        }
    });
}

/// Probe now. Safe to call from startup, from a settings save, or from the
/// card's Check again / Refresh models buttons; a newer run simply replaces
/// an older answer.
pub(crate) fn refresh(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    let services = app.global::<AppServices>();
    services.set_checking(true);
    // Keep the last list and the configured tiers visible while the probe
    // runs, so the pickers do not go blank.
    let catalog = ai_catalog();
    publish_ai_tiers(app, &tier_models_from_ui(app), &catalog, false);
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
    if let Ok(mut slot) = AI_STATUS.lock() {
        *slot = Some(report.ai.clone());
    }

    let services = app.global::<AppServices>();
    services.set_checking(false);
    services.set_reacher_enabled(report.reacher_enabled);
    services.set_reacher_state(report.reacher.state.as_str().into());
    publish_ai(&services, &report.ai);
    publish_speech(&services, &report.speech);
    // The Tuta bridge is a shell service, not one the core probe can reach:
    // its own status says whether the session bus lets it listen.
    crate::notify_bridge::publish(app);
    // The same report carries the model list, so the pickers need no second
    // request to the endpoint.
    publish_ai_picker(app, &report.ai);

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

/// Fill the AI model pickers from the probe's answer: every model the endpoint
/// listed, what each tier uses now, and whether that model is served.
fn publish_ai_picker(app: &AppWindow, ai: &AiStatus) {
    // An endpoint that did not answer keeps the last known list rather than
    // pretending it serves nothing; one that answered with no models reports
    // exactly that, and manual entry stays available.
    let catalog = if ai.answers() {
        if let Ok(mut slot) = AI_CATALOG.lock() {
            *slot = ai.available_models.clone();
        }
        ai.available_models.clone()
    } else {
        ai_catalog()
    };
    publish_ai_tiers(app, &ai.tier_models, &catalog, ai.answers());
}

/// Show what Settings is configured to use before the first probe lands, so
/// the pickers are never blank for a configured setup.
pub(crate) fn publish_ai_tier_models(app: &AppWindow, settings: &Settings) {
    let catalog = ai_catalog();
    publish_ai_tiers(app, &AiTierModels::from_settings(settings), &catalog, false);
}

/// Show a model just picked in Settings, before the re-probe returns, and
/// re-shape the dropdowns around it.
pub(crate) fn publish_ai_pick(app: &AppWindow, tier: Option<AiTier>, model: &str) {
    let mut tiers = tier_models_from_ui(app);
    match tier {
        Some(tier) => tiers.set(tier, model),
        None => {
            tiers.instant = model.trim().to_owned();
            tiers.cheap = model.trim().to_owned();
            tiers.intelligent = model.trim().to_owned();
        }
    }
    let listed = app.global::<AiModels>().get_listed();
    publish_ai_tiers(app, &tiers, &ai_catalog(), listed);
}

/// The tiers as the pickers show them now, used when only the list changes.
fn tier_models_from_ui(app: &AppWindow) -> AiTierModels {
    let ui = app.global::<AiModels>();
    AiTierModels {
        instant: ui.get_instant().to_string(),
        cheap: ui.get_cheap().to_string(),
        intelligent: ui.get_intelligent().to_string(),
    }
}

/// Show a catalog and the configured tiers in the pickers. Choices are
/// published before the values: a ComboBox clamps its selection to its model,
/// so the configured model must already be in the list when it arrives.
fn publish_ai_tiers(app: &AppWindow, tiers: &AiTierModels, catalog: &[String], listed: bool) {
    let ui = app.global::<AiModels>();
    ui.set_available(string_model(catalog));
    ui.set_listed(listed);
    let mut intelligent_choices = Vec::new();
    for tier in AiTier::ALL {
        let choices = tier_choices(catalog, tiers.get(tier));
        if tier == AiTier::Intelligent {
            intelligent_choices = choices.clone();
        }
        set_tier_choices(&ui, tier, &choices);
    }
    ui.set_all_choices(string_model(&intelligent_choices));
    for tier in AiTier::ALL {
        let current = tiers.get(tier);
        set_tier_model(&ui, tier, current);
        set_tier_served(
            &ui,
            tier,
            !current.is_empty() && catalog.iter().any(|model| model == current),
        );
    }
}

/// The dropdown list for one tier: every served model in the probe's sorted
/// order, with the configured model in front when the endpoint does not serve
/// it, so the box shows the model to change (marked in the caption) instead of
/// going blank.
fn tier_choices(catalog: &[String], current: &str) -> Vec<String> {
    let mut choices = Vec::with_capacity(catalog.len() + 1);
    if !current.is_empty() && !catalog.iter().any(|model| model == current) {
        choices.push(current.to_owned());
    }
    choices.extend(catalog.iter().cloned());
    choices
}

fn string_model(values: &[String]) -> slint::ModelRc<slint::SharedString> {
    slint::ModelRc::new(slint::VecModel::from(
        values
            .iter()
            .map(|value| slint::SharedString::from(value.as_str()))
            .collect::<Vec<_>>(),
    ))
}

fn set_tier_choices(ui: &AiModels, tier: AiTier, choices: &[String]) {
    let choices = string_model(choices);
    match tier {
        AiTier::Instant => ui.set_instant_choices(choices),
        AiTier::Cheap => ui.set_cheap_choices(choices),
        AiTier::Intelligent => ui.set_intelligent_choices(choices),
    }
}

fn set_tier_model(ui: &AiModels, tier: AiTier, model: &str) {
    let model = slint::SharedString::from(model);
    match tier {
        AiTier::Instant => ui.set_instant(model),
        AiTier::Cheap => ui.set_cheap(model),
        AiTier::Intelligent => ui.set_intelligent(model),
    }
}

fn set_tier_served(ui: &AiModels, tier: AiTier, served: bool) {
    match tier {
        AiTier::Instant => ui.set_instant_served(served),
        AiTier::Cheap => ui.set_cheap_served(served),
        AiTier::Intelligent => ui.set_intelligent_served(served),
    }
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
    // The bridge's own state is independent of the core: report the truth.
    crate::notify_bridge::publish(app);
    // The configured tiers stay visible; only the served list is unknown.
    let tiers = tier_models_from_ui(app);
    let catalog = ai_catalog();
    publish_ai_tiers(app, &tiers, &catalog, false);
    crate::composer::set_address_checks_available(false);
    crate::speech::set_availability(false, None);
}

/// The models the endpoint served the last time it was asked. Empty when it
/// never answered or listed nothing.
pub(crate) fn ai_catalog() -> Vec<String> {
    let guard = match AI_CATALOG.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clone()
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
