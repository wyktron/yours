//! Settings preference callback wiring.
//!
//! These callbacks share one lifecycle and persistence boundary: update the
//! session immediately where needed, then write through the core when it is
//! available. Keeping them together prevents the application composition root
//! from accumulating one closure per setting.

use super::*;
use flectar_mail_core::models::CustomTheme;
use flectar_mail_core::services::AiTier;

fn apply_account_presentation_settings(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    settings: &flectar_mail_core::models::Settings,
) {
    state.borrow_mut().account_presentation = AccountPresentationSettings::from_settings(settings);
    refresh_connected_accounts(app, state);
    refresh_rows_only(app, state, runtime);
    // A profile change can change which accounts the mail view shows.
    app.invoke_mail_profile_changed();
}

/// Persists the download location and only then reflects it in the UI, so a
/// failed write never leaves Settings showing a directory that was not saved.
#[cfg(desktop)]
fn save_download_directory(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    directory: String,
) {
    let Some(core) = state.borrow().core.clone() else {
        app.set_sync_status(UiMessage::plain(
            "Download location is saved once an account is set up.",
        ));
        return;
    };
    match runtime.block_on(core.set_download_directory(&directory)) {
        Ok(()) => {
            app.set_download_directory(
                crate::documents::download_directory_label(&directory).into(),
            );
            app.set_sync_status(UiMessage::plain("Download location saved."));
        }
        Err(error) => app.set_sync_status(UiMessage::detail(
            "Could not save the download location: {}",
            error,
        )),
    }
}

/// Wires the Updates card and the sidebar notice to src/updates.rs, and
/// starts checking in builds that offer it.
pub(super) fn register_update_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    cache_dir: &std::path::Path,
) {
    let updates = app.global::<Updates>();
    updates.set_offered(crate::updates::offered());
    let app_weak = app.as_weak();
    let state_for_updates = Rc::clone(state);
    let runtime_for_updates = runtime.clone();
    updates.on_set_enabled(move |enabled| {
        crate::updates::set_enabled(enabled);
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_updates.borrow().core.clone() else {
            return;
        };
        match runtime_for_updates.block_on(core.set_check_for_updates(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Update preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the update preference: {}",
                error,
            )),
        }
    });
    updates.on_check_now(crate::updates::check_now);
    updates.on_dismiss(crate::updates::dismiss);
    let app_weak = app.as_weak();
    updates.on_open_release(move || {
        if let (Some(app), Some(page)) = (app_weak.upgrade(), crate::updates::release_page()) {
            app.invoke_open_email_link(page.into());
        }
    });
    let app_weak = app.as_weak();
    crate::updates::start(runtime, cache_dir, move |view| {
        let app_weak = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let updates = app.global::<Updates>();
            updates.set_enabled(view.enabled);
            updates.set_checking(view.checking);
            updates.set_failed(view.failed);
            updates.set_current(view.current);
            updates.set_dismissed(view.dismissed);
            let (version, summary) = view
                .available
                .map(|release| (release.version, release.summary))
                .unwrap_or_default();
            updates.set_version(version.into());
            updates.set_summary(summary.into());
        });
    });
}

pub(super) fn register_settings_preference_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    let app_weak = app.as_weak();
    let state_for_notifications = Rc::clone(state);
    let runtime_for_notifications = runtime.clone();
    app.on_save_notification_settings(move |enabled, sound, scope| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_notifications.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Notification preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_notifications.block_on(core.set_notification_settings(
            enabled,
            sound,
            scope.as_str(),
        )) {
            Ok(()) => {
                crate::notifications::preferences_changed();
                crate::notifications::ui::refresh_availability();
                app.set_sync_status(UiMessage::plain("Notification preferences saved."));
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save notifications: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_quiet = Rc::clone(state);
    let runtime_for_quiet = runtime.clone();
    app.on_save_quiet_hours(move |enabled, start, end| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let (start, end) = (start.to_string(), end.to_string());
        let Some(core) = state_for_quiet.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Notification preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_quiet.block_on(core.set_quiet_hours(enabled, &start, &end)) {
            Ok((start, end)) => {
                app.set_quiet_hours(enabled);
                app.set_quiet_start(start.into());
                app.set_quiet_end(end.into());
                app.set_sync_status(UiMessage::plain("Quiet hours saved."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save quiet hours: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_badge = Rc::clone(state);
    let runtime_for_badge = runtime.clone();
    app.on_save_unread_badge(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_badge.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Notification preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_badge.block_on(core.set_unread_badge(enabled)) {
            Ok(()) => {
                crate::notifications::preferences_changed();
                app.set_sync_status(UiMessage::plain("Notification preferences saved."));
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save notifications: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_sync_interval = Rc::clone(state);
    let runtime_for_sync_interval = runtime.clone();
    app.on_save_sync_interval(move |minutes| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_sync_interval.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Sync interval is active for this session.",
            ));
            return;
        };
        match runtime_for_sync_interval.block_on(core.set_sync_interval_minutes(i64::from(minutes)))
        {
            Ok(()) => app.set_sync_status(UiMessage::detail(
                "Sync interval set to {} minutes.",
                minutes,
            )),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save sync interval: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_undo_send = Rc::clone(state);
    let runtime_for_undo_send = runtime.clone();
    app.on_save_undo_send(move |seconds| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_undo_send.borrow().core.clone() else {
            return;
        };
        match runtime_for_undo_send.block_on(core.set_undo_send_seconds(i64::from(seconds))) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Undo send preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the undo send preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_text_style = Rc::clone(state);
    let runtime_for_text_style = runtime.clone();
    app.on_save_compose_text_style(move |font, size| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_text_style.borrow().core.clone() else {
            return;
        };
        match runtime_for_text_style.block_on(core.set_compose_text_style(font.into(), size.into()))
        {
            Ok(()) => app.set_sync_status(UiMessage::plain("Text style preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the text style preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_mark_read = Rc::clone(state);
    let runtime_for_mark_read = runtime.clone();
    app.on_save_mark_read(move |choice| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(choice) = flectar_mail_core::models::MarkRead::parse(&choice) else {
            return;
        };
        state_for_mark_read.borrow_mut().mark_read = choice;
        // A wait that started under the old choice ends with it.
        crate::read_marking::cancel();
        let Some(core) = state_for_mark_read.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Message-opening preference updated for this session.",
            ));
            return;
        };
        match runtime_for_mark_read.block_on(core.set_mark_read(choice)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Message-opening preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save message-opening preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_contacts = Rc::clone(state);
    let runtime_for_contacts = runtime.clone();
    app.on_save_contact_discovery_settings(move |outgoing, incoming, all_accounts, suggest| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_contacts.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Contact suggestion preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_contacts.block_on(core.set_contact_discovery_settings(
            outgoing,
            incoming,
            all_accounts,
            suggest,
        )) {
            Ok(()) => {
                app.set_sync_status(UiMessage::plain("Contact suggestion preferences saved."))
            }
            Err(error) => {
                if let Ok(settings) = runtime_for_contacts.block_on(core.load_settings()) {
                    app.set_collect_outgoing_contacts(settings.collect_outgoing_contacts);
                    app.set_collect_incoming_contacts(settings.collect_incoming_contacts);
                    app.set_contact_suggest_all_accounts(settings.contact_suggest_all_accounts);
                    app.set_suggest_learned_contacts(settings.suggest_learned_contacts);
                }
                app.set_sync_status(UiMessage::detail(
                    "Could not save contact suggestion preferences: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_clear_contacts = Rc::clone(state);
    let runtime_for_clear_contacts = runtime.clone();
    app.on_clear_contact_suggestions(move || {
        let Some(app) = app_weak.upgrade() else {
            return false;
        };
        let Some(core) = state_for_clear_contacts.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Contact storage is still starting. Try again shortly.",
            ));
            return false;
        };
        match runtime_for_clear_contacts.block_on(core.clear_contact_suggestions()) {
            Ok(removed) => {
                app.set_sync_status(UiMessage::detail("Cleared {} suggested people.", removed));
                app.invoke_search_contacts(app.get_contact_search_query());
                true
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail(
                    "Could not clear suggested people: {}",
                    error,
                ));
                false
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_theme = Rc::clone(state);
    let runtime_for_theme = runtime.clone();
    app.on_save_theme(move |theme| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_theme.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Theme updated for this session."));
            return;
        };
        match runtime_for_theme.block_on(core.set_theme(&theme)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Theme preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail("Could not save theme: {}", error)),
        }
    });

    let app_weak = app.as_weak();
    // Only desktop hosts pick a filesystem directory; mobile hosts export
    // through their own document provider and hide the setting entirely.
    #[cfg(desktop)]
    {
        let state_for_download = Rc::clone(state);
        let runtime_for_download = runtime.clone();
        let app_weak_for_download = app_weak.clone();
        let picking_download = Rc::new(Cell::new(false));
        app.on_choose_download_directory(move || {
            let Some(app) = app_weak_for_download.upgrade() else {
                return;
            };
            if picking_download.replace(true) {
                return;
            }
            let title = translated(&app, &UiMessage::plain("Choose download location")).to_string();
            let core = state_for_download.borrow().core.clone();
            // The portal picker needs the runtime: rfd's blocking dialog
            // panics on the UI thread, which has no Tokio context.
            let request: crate::documents::DocumentFuture<Option<std::path::PathBuf>> =
                Box::pin(async move {
                    // Starts in the saved folder as stored, not as Settings
                    // shows it: inside Flatpak the shown host path may be
                    // outside the sandbox.
                    let current = match core {
                        Some(core) => core.load_settings().await.ok().and_then(|settings| {
                            crate::documents::resolve_download_directory(
                                &settings.download_directory,
                            )
                        }),
                        None => None,
                    };
                    let dialog = rfd::AsyncFileDialog::new().set_title(title);
                    let dialog = match current {
                        Some(directory) => {
                            dialog.set_directory(crate::documents::picker_directory(&directory))
                        }
                        None => dialog,
                    };
                    Ok(dialog
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_owned()))
                });
            let state = Rc::clone(&state_for_download);
            let runtime = runtime_for_download.clone();
            let picking = Rc::clone(&picking_download);
            let app_weak = app.as_weak();
            crate::documents::run_request(&runtime_for_download, request, move |picked| {
                picking.set(false);
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match picked {
                    Ok(Some(directory)) => save_download_directory(
                        &app,
                        &state,
                        &runtime,
                        directory.to_string_lossy().into_owned(),
                    ),
                    Ok(None) => {}
                    Err(error) => app.set_sync_status(UiMessage::detail(
                        "Could not save the download location: {}",
                        error,
                    )),
                }
            });
        });
        let state_for_clear = Rc::clone(state);
        let runtime_for_clear = runtime.clone();
        let app_weak_for_clear = app_weak.clone();
        app.on_clear_download_directory(move || {
            let Some(app) = app_weak_for_clear.upgrade() else {
                return;
            };
            save_download_directory(&app, &state_for_clear, &runtime_for_clear, String::new());
        });
    }
    let state_for_theme_preset = Rc::clone(state);
    let runtime_for_theme_preset = runtime.clone();
    app.on_save_theme_preset(move |preset| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_theme_preset.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Theme palette updated for this session."));
            return;
        };
        match runtime_for_theme_preset.block_on(core.set_theme_preset(preset.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Theme palette saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save theme palette: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_appearance = Rc::clone(state);
    let runtime_for_appearance = runtime.clone();
    app.global::<MessageColors>().on_choose(move |preference| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if flectar_mail_core::models::MessageAppearance::parse(&preference).is_none() {
            return;
        }
        let colors = app.global::<MessageColors>();
        let previous = colors.get_preference();
        colors.set_preference(preference.clone());
        let Some(core) = state_for_appearance.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Message appearance updated for this session.",
            ));
            return;
        };
        match runtime_for_appearance.block_on(core.set_message_appearance(&preference)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Message appearance preference saved.")),
            Err(error) => {
                colors.set_preference(previous);
                app.set_sync_status(UiMessage::detail(
                    "Could not save message appearance: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_motion = Rc::clone(state);
    let runtime_for_motion = runtime.clone();
    app.global::<crate::MotionSettings>()
        .on_choose(move |preference| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if flectar_mail_core::models::ReduceMotion::parse(&preference).is_none() {
                return;
            }
            let motion = app.global::<crate::MotionSettings>();
            let previous = motion.get_preference();
            motion.set_preference(preference.clone());
            let Some(core) = state_for_motion.borrow().core.clone() else {
                app.set_sync_status(UiMessage::plain("Reduce motion updated for this session."));
                return;
            };
            match runtime_for_motion.block_on(core.set_reduce_motion(&preference)) {
                Ok(()) => app.set_sync_status(UiMessage::plain("Reduce motion preference saved.")),
                Err(error) => {
                    motion.set_preference(previous);
                    app.set_sync_status(UiMessage::detail(
                        "Could not save the reduce motion preference: {}",
                        error,
                    ));
                }
            }
        });

    let app_weak = app.as_weak();
    let state_for_text_size = Rc::clone(state);
    let runtime_for_text_size = runtime.clone();
    app.global::<crate::InterfaceText>()
        .on_choose(move |choice| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(size) = flectar_mail_core::models::InterfaceTextSize::parse(&choice) else {
                return;
            };
            let previous = flectar_mail_core::models::InterfaceTextSize::parse(
                &app.global::<crate::InterfaceText>().get_size(),
            )
            .unwrap_or_default();
            crate::text_size::apply(&app, size);
            let Some(core) = state_for_text_size.borrow().core.clone() else {
                app.set_sync_status(UiMessage::plain(
                    "Interface text size updated for this session.",
                ));
                return;
            };
            match runtime_for_text_size.block_on(core.set_interface_text_size(&choice)) {
                Ok(()) => app.set_sync_status(UiMessage::plain("Interface text size saved.")),
                Err(error) => {
                    crate::text_size::apply(&app, previous);
                    app.set_sync_status(UiMessage::detail(
                        "Could not save the interface text size: {}",
                        error,
                    ));
                }
            }
        });

    let app_weak = app.as_weak();
    let state_for_receipts = Rc::clone(state);
    let runtime_for_receipts = runtime.clone();
    app.global::<crate::ReadReceipts>()
        .on_choose(move |choice| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if flectar_mail_core::models::ReadReceiptPolicy::parse(&choice).is_none() {
                return;
            }
            let receipts = app.global::<crate::ReadReceipts>();
            let previous = receipts.get_policy();
            receipts.set_policy(choice.clone());
            let Some(core) = state_for_receipts.borrow().core.clone() else {
                receipts.set_policy(previous);
                return;
            };
            match runtime_for_receipts.block_on(core.set_read_receipts(&choice)) {
                Ok(()) => app.set_sync_status(UiMessage::plain("Read receipt preference saved.")),
                Err(error) => {
                    receipts.set_policy(previous);
                    app.set_sync_status(UiMessage::detail(
                        "Could not save the read receipt preference: {}",
                        error,
                    ));
                }
            }
        });

    let app_weak = app.as_weak();
    let state_for_invitations = Rc::clone(state);
    let runtime_for_invitations = runtime.clone();
    app.global::<InvitationSettings>().on_choose(move |choice| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if flectar_mail_core::models::AddInvitations::parse(&choice).is_none() {
            return;
        }
        let invitations = app.global::<InvitationSettings>();
        let previous = invitations.get_add();
        invitations.set_add(choice.clone());
        let Some(core) = state_for_invitations.borrow().core.clone() else {
            invitations.set_add(previous);
            return;
        };
        match runtime_for_invitations.block_on(core.set_add_invitations(&choice)) {
            Ok(()) => {
                app.set_sync_status(UiMessage::plain("Calendar invitations preference saved."))
            }
            Err(error) => {
                invitations.set_add(previous);
                app.set_sync_status(UiMessage::detail(
                    "Could not save the calendar invitations preference: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_custom_theme = Rc::clone(state);
    let runtime_for_custom_theme = runtime.clone();
    app.on_save_custom_theme(
        move |light_primary,
              light_page,
              light_surface,
              light_text,
              light_border,
              dark_primary,
              dark_page,
              dark_surface,
              dark_text,
              dark_border| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let custom_theme = CustomTheme {
                light_primary: theme::color_to_hex(light_primary),
                light_page_background: theme::color_to_hex(light_page),
                light_surface: theme::color_to_hex(light_surface),
                light_text: theme::color_to_hex(light_text),
                light_border: theme::color_to_hex(light_border),
                dark_primary: theme::color_to_hex(dark_primary),
                dark_page_background: theme::color_to_hex(dark_page),
                dark_surface: theme::color_to_hex(dark_surface),
                dark_text: theme::color_to_hex(dark_text),
                dark_border: theme::color_to_hex(dark_border),
            };
            let Some(core) = state_for_custom_theme.borrow().core.clone() else {
                app.set_sync_status(UiMessage::plain("Custom theme updated for this session."));
                return false;
            };
            match runtime_for_custom_theme.block_on(core.set_custom_theme(custom_theme)) {
                Ok(()) => {
                    app.set_sync_status(UiMessage::plain("Custom theme saved."));
                    true
                }
                Err(error) => {
                    app.set_sync_status(UiMessage::detail(
                        "Could not save custom theme: {}",
                        error,
                    ));
                    false
                }
            }
        },
    );

    let app_weak = app.as_weak();
    let state_for_language = Rc::clone(state);
    let runtime_for_language = runtime.clone();
    app.on_save_language(move |language| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        apply_language(&app, language.as_str());
        let Some(core) = state_for_language.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Language updated for this session."));
            return;
        };
        match runtime_for_language.block_on(core.set_language(language.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Language preference saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save language: {}", error))
            }
        }
    });

    // Read aloud: the engine, the voice it speaks with, and the endpoint and
    // key an online engine needs. Every save writes through the core and then
    // reloads what the speech layer will speak with, so the next request uses
    // the new choice.
    let app_weak = app.as_weak();
    let state_for_tts_voice = Rc::clone(state);
    let runtime_for_tts_voice = runtime.clone();
    app.on_save_tts_voice(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        // A list of choices reports the label it showed, so an id is looked
        // back up; a hand-typed voice (the custom endpoint) is used as given.
        let value = crate::speech::voice_id_for_label(value.as_str())
            .unwrap_or_else(|| value.to_string());
        let saved = save_tts_setting(
            &app,
            &state_for_tts_voice,
            &runtime_for_tts_voice,
            |settings| settings.tts_voice = value,
        );
        if saved {
            refresh_tts(&app, &state_for_tts_voice, &runtime_for_tts_voice);
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_engine = Rc::clone(state);
    let runtime_for_tts_engine = runtime.clone();
    app.on_save_tts_engine(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let engine = flectar_mail_core::tts::TtsEngine::from_label(value.as_str());
        let saved = save_tts_setting(
            &app,
            &state_for_tts_engine,
            &runtime_for_tts_engine,
            |settings| settings.tts_engine = engine.id().to_owned(),
        );
        if saved {
            app.set_tts_engine(engine.id().into());
            app.set_tts_needs_key(engine.needs_key());
            refresh_tts(&app, &state_for_tts_engine, &runtime_for_tts_engine);
            crate::services::refresh(&app, &state_for_tts_engine, &runtime_for_tts_engine);
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_key = Rc::clone(state);
    let runtime_for_tts_key = runtime.clone();
    app.on_save_tts_key(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_tts_key.borrow().core.as_ref().map(|core| core.file_core()) else {
            return;
        };
        let engine = flectar_mail_core::tts::TtsEngine::parse(app.get_tts_engine().as_str());
        let key = value.to_string();
        let key_for_save = key.clone();
        let result = runtime_for_tts_key.block_on(async move {
            core.set_tts_key(engine.id(), &key_for_save)
                .await
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(()) => {
                app.set_tts_key_present(!key.trim().is_empty());
                app.set_sync_status(UiMessage::plain("Read-aloud key saved."));
                refresh_tts(&app, &state_for_tts_key, &runtime_for_tts_key);
                crate::services::refresh(&app, &state_for_tts_key, &runtime_for_tts_key);
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the read-aloud key: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_endpoint = Rc::clone(state);
    let runtime_for_tts_endpoint = runtime.clone();
    app.on_save_tts_endpoint(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let value = value.to_string();
        let saved = save_tts_setting(
            &app,
            &state_for_tts_endpoint,
            &runtime_for_tts_endpoint,
            |settings| settings.tts_endpoint = value,
        );
        if saved {
            refresh_tts(&app, &state_for_tts_endpoint, &runtime_for_tts_endpoint);
            crate::services::refresh(&app, &state_for_tts_endpoint, &runtime_for_tts_endpoint);
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_model = Rc::clone(state);
    let runtime_for_tts_model = runtime.clone();
    app.on_save_tts_model(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let value = value.to_string();
        let saved = save_tts_setting(
            &app,
            &state_for_tts_model,
            &runtime_for_tts_model,
            |settings| settings.tts_model = value,
        );
        if saved {
            refresh_tts(&app, &state_for_tts_model, &runtime_for_tts_model);
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_announce = Rc::clone(state);
    let runtime_for_tts_announce = runtime.clone();
    app.on_save_tts_announce(move |checked| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let saved = save_tts_setting(
            &app,
            &state_for_tts_announce,
            &runtime_for_tts_announce,
            |settings| settings.tts_announce_mail = checked,
        );
        if saved {
            app.set_tts_announce_mail(checked);
        }
    });

    // Tuta's notifications are their own switch: the bridge listens for the
    // Tuta app's notifications on the session bus, and the voice reads them.
    let app_weak = app.as_weak();
    let state_for_tuta = Rc::clone(state);
    let runtime_for_tuta = runtime.clone();
    app.on_save_tuta_notifications(move |checked| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let saved = save_notification_setting(
            &app,
            &state_for_tuta,
            &runtime_for_tuta,
            |settings| settings.tuta_notifications = Some(checked),
        );
        if saved {
            app.set_tuta_notifications(checked);
        }
    });

    let app_weak = app.as_weak();
    let state_for_tuta_apps = Rc::clone(state);
    let runtime_for_tuta_apps = runtime.clone();
    app.on_save_tuta_extra_apps(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let value = value.to_string();
        let stored = value.clone();
        let saved = save_notification_setting(
            &app,
            &state_for_tuta_apps,
            &runtime_for_tuta_apps,
            move |settings| settings.tuta_extra_apps = stored,
        );
        if saved {
            app.set_tuta_extra_apps(value.into());
        }
    });

    // AI assistant: one model for every feature, the endpoint it runs on, and
    // the phishing screen. The model fields behind the tier names are kept in
    // step, so a model change applies to sorting, summaries and drafts alike.
    let app_weak = app.as_weak();
    let state_for_ai = Rc::clone(state);
    let runtime_for_ai = runtime.clone();
    app.on_save_ai_phishing(move |checked| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if save_ai_setting(&app, &state_for_ai, &runtime_for_ai, |settings| {
            settings.ai_phishing_enabled = checked;
        }) {
            app.set_ai_phishing(checked);
        }
    });

    // Settings > AI agents picks a model from the endpoint's own list. One
    // tier changes, or "all" sets the three at once; either way the services
    // probe runs again so the status line says connected or degraded for the
    // new models at once.
    let app_weak = app.as_weak();
    let state_for_ai_pick = Rc::clone(state);
    let runtime_for_ai_pick = runtime.clone();
    app.global::<AiModels>().on_pick(move |tier, model| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let tier = match tier.as_str() {
            "all" => None,
            "instant" => Some(AiTier::Instant),
            "cheap" => Some(AiTier::Cheap),
            "intelligent" => Some(AiTier::Intelligent),
            other => {
                tracing::warn!(tier = %other, "unknown AI model tier picked in Settings");
                return;
            }
        };
        save_ai_model_pick(
            &app,
            &state_for_ai_pick,
            &runtime_for_ai_pick,
            tier,
            model.as_str(),
        );
        // Re-probe either way: after a successful pick this updates the state
        // and the detail line; after a failed one it restores the stored
        // models instead of leaving the dropdown showing something unsaved.
        crate::services::refresh(&app, &state_for_ai_pick, &runtime_for_ai_pick);
    });

    let app_weak = app.as_weak();
    let state_for_ai_endpoint = Rc::clone(state);
    let runtime_for_ai_endpoint = runtime.clone();
    app.on_save_ai_endpoint(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let endpoint = value.to_string();
        if save_ai_setting(
            &app,
            &state_for_ai_endpoint,
            &runtime_for_ai_endpoint,
            |settings| settings.ai_base_url = endpoint,
        ) {
            app.set_ai_endpoint(value);
            crate::services::refresh(&app, &state_for_ai_endpoint, &runtime_for_ai_endpoint);
        }
    });

    let app_weak = app.as_weak();
    let state_for_ai_key = Rc::clone(state);
    let runtime_for_ai_key = runtime.clone();
    app.on_save_ai_key(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_ai_key.borrow().core.as_ref().map(|core| core.file_core()) else {
            return;
        };
        let key = value.to_string();
        let result = runtime_for_ai_key.block_on(async move {
            core.set_ai_key(key)
                .await
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(()) => {
                app.set_ai_key_saved(!value.trim().is_empty());
                app.set_sync_status(UiMessage::plain("AI key saved."));
                crate::services::refresh(&app, &state_for_ai_key, &runtime_for_ai_key);
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the AI key: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_tts_preview = Rc::clone(state);
    let runtime_for_tts_preview = runtime.clone();
    app.on_preview_tts_voice(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        // An engine on a machine with no player is silence: onyx has paplay
        // but no pw-play, and the sample used to do nothing at all there. Say
        // what to install instead of spawning a player that cannot exist.
        if crate::speech::player().is_none() {
            tracing::error!("read-aloud sample: no audio player on the PATH");
            app.set_sync_status(UiMessage::detail(
                "Read aloud failed: {}",
                crate::speech::NO_PLAYER_HINT,
            ));
            return;
        }
        let Some(core) = state_for_tts_preview
            .borrow()
            .core
            .as_ref()
            .map(|core| core.file_core())
        else {
            return;
        };
        match runtime_for_tts_preview.block_on(core.tts_config()) {
            Ok(config) => {
                crate::speech::set_config(config);
                crate::speech::preview(&runtime_for_tts_preview, flectar_mail_core::tts::SAMPLE_TEXT);
            }
            Err(error) => tracing::warn!(%error, "read-aloud sample could not load settings"),
        }
    });

    let app_weak = app.as_weak();
    let state_for_time_format = Rc::clone(state);
    let runtime_for_time_format = runtime.clone();
    app.on_save_time_format(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(format) = flectar_mail_core::models::TimeFormat::parse(&value) else {
            return;
        };
        date_format::reload(&app);
        let Some(core) = state_for_time_format.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Time format updated for this session."));
            return;
        };
        match runtime_for_time_format.block_on(core.set_time_format(format)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Time format preference saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save time format: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_app_icon = Rc::clone(state);
    let runtime_for_app_icon = runtime.clone();
    app.global::<AppIcon>().on_choose(move |variant| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let previous = crate::app_icon::choice(app.global::<AppIcon>().get_variant());
        let icon = crate::app_icon::choice(variant);
        crate::app_icon::apply(&app, icon);
        let Some(core) = state_for_app_icon.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("App icon updated for this session."));
            return;
        };
        match runtime_for_app_icon.block_on(core.set_app_icon(icon)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("App icon saved.")),
            Err(error) => {
                crate::app_icon::apply(&app, previous);
                app.set_sync_status(UiMessage::detail("Could not save app icon: {}", error));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_sidebar_icons = Rc::clone(state);
    let runtime_for_sidebar_icons = runtime.clone();
    app.on_save_sidebar_icon_style(move |monochrome| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_sidebar_icons.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Sidebar icon style updated for this session.",
            ));
            return;
        };
        match runtime_for_sidebar_icons.block_on(core.set_monochrome_sidebar_icons(monochrome)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Sidebar icon preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save sidebar icon preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_avatars = Rc::clone(state);
    let runtime_for_avatars = runtime.clone();
    app.on_save_avatar_visibility(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_avatars.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Avatar preference updated for this session.",
            ));
            return;
        };
        match runtime_for_avatars.block_on(core.set_show_avatars(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Avatar preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save avatar preference: {}",
                error,
            )),
        }
    });

    // Density and preview lines shape the same rows, so they save together.
    let app_weak = app.as_weak();
    let state_for_rows = Rc::clone(state);
    let runtime_for_rows = runtime.clone();
    app.on_save_list_rows(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let density = if app.get_mail_compact_rows() {
            flectar_mail_core::models::ListDensity::Compact
        } else {
            flectar_mail_core::models::ListDensity::Comfortable
        };
        let preview_lines = u8::try_from(app.get_mail_preview_lines().clamp(0, 3)).unwrap_or(1);
        let Some(core) = state_for_rows.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Message list preference updated for this session.",
            ));
            return;
        };
        match runtime_for_rows.block_on(core.set_list_rows(density, preview_lines)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Message list preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save message list preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_spelling = Rc::clone(state);
    let runtime_for_spelling = runtime.clone();
    app.on_save_spelling_enabled(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let languages = crate::spelling::service().and_then(|spelling| spelling.chosen_languages());
        save_spelling(
            &app,
            &state_for_spelling,
            &runtime_for_spelling,
            enabled,
            languages,
        );
    });

    let app_weak = app.as_weak();
    let state_for_reacher = Rc::clone(state);
    let runtime_for_reacher = runtime.clone();
    app.on_save_reacher_enabled(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        crate::composer::set_address_checks_enabled(enabled);
        app.set_reacher_enabled(enabled);
        let Some(core) = state_for_reacher.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Address check settings are active for this session.",
            ));
            crate::composer::address_checks_changed();
            crate::services::refresh(&app, &state_for_reacher, &runtime_for_reacher);
            return;
        };
        match runtime_for_reacher.block_on(core.set_reacher_enabled(enabled)) {
            Ok(()) => {
                app.set_sync_status(UiMessage::plain("Address check settings saved."));
                crate::composer::address_checks_changed();
                crate::services::refresh(&app, &state_for_reacher, &runtime_for_reacher);
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save address check settings: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_reacher_url = Rc::clone(state);
    let runtime_for_reacher_url = runtime.clone();
    app.on_save_reacher_url(move |url| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_reacher_url.borrow().core.clone() else {
            app.set_reacher_url(url);
            app.set_sync_status(UiMessage::plain(
                "Address check settings are active for this session.",
            ));
            crate::services::refresh(&app, &state_for_reacher_url, &runtime_for_reacher_url);
            return;
        };
        match runtime_for_reacher_url.block_on(core.set_reacher_url(&url)) {
            Ok(()) => {
                app.set_reacher_url(url);
                app.set_sync_status(UiMessage::plain("Address check settings saved."));
                // Answers from the old service must not keep colouring chips.
                crate::composer::reset_address_checks();
                crate::composer::address_checks_changed();
                crate::services::refresh(&app, &state_for_reacher_url, &runtime_for_reacher_url);
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save address check settings: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_tabs = Rc::clone(state);
    let runtime_for_tabs = runtime.clone();
    app.on_save_show_tabs(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_tabs.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Tabs preference updated for this session.",
            ));
            return;
        };
        match runtime_for_tabs.block_on(core.set_show_tabs(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Tabs preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save tabs preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_grouping = Rc::clone(state);
    let runtime_for_grouping = runtime.clone();
    app.on_save_conversation_view(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let grouping = if enabled {
            MailGrouping::Conversations
        } else {
            MailGrouping::Messages
        };
        apply_mail_grouping(&app, &state_for_grouping, &runtime_for_grouping, grouping);
        let Some(core) = state_for_grouping.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Conversation view updated for this session.",
            ));
            return;
        };
        match runtime_for_grouping.block_on(core.set_conversation_view(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Conversation view preference saved.")),
            Err(error) => {
                let previous = if enabled {
                    MailGrouping::Messages
                } else {
                    MailGrouping::Conversations
                };
                apply_mail_grouping(&app, &state_for_grouping, &runtime_for_grouping, previous);
                app.set_sync_status(UiMessage::detail(
                    "Could not save conversation view preference: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_date_groups = Rc::clone(state);
    let runtime_for_date_groups = runtime.clone();
    app.on_save_group_mail_by_date(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        state_for_date_groups.borrow_mut().mail_groups.clear();
        refresh_rows_only(&app, &state_for_date_groups, &runtime_for_date_groups);
        let Some(core) = state_for_date_groups.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Mail grouping is active for this session.",
            ));
            return;
        };
        match runtime_for_date_groups.block_on(core.set_group_mail_by_date(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Mail grouping preference saved.")),
            Err(error) => {
                app.set_group_mail_by_date(!enabled);
                refresh_rows_only(&app, &state_for_date_groups, &runtime_for_date_groups);
                app.set_sync_status(UiMessage::detail(
                    "Could not save mail grouping preference: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_profiles = Rc::clone(state);
    let runtime_for_profiles = runtime.clone();
    app.on_save_mail_profile(move |profile_id, name, color| {
        let Some(app) = app_weak.upgrade() else {
            return Default::default();
        };
        let Some(core) = state_for_profiles.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return Default::default();
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        let (mut settings, profile) = match runtime_for_profiles.block_on(core.save_mail_profile(
            profile_id,
            name.to_string(),
            theme::color_to_hex(color),
        )) {
            Ok(saved) => saved,
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save profile: {}", error));
                return Default::default();
            }
        };
        // The editor's checked accounts become the profile's accounts.
        let members = app.get_profile_members();
        let mut failure = None;
        for member in members.iter() {
            let account_id = i64::from(member.id);
            let in_profile = profile.account_ids.contains(&account_id);
            let assignment = match (member.checked, in_profile) {
                (true, false) => Some(profile.id.clone()),
                (false, true) => None,
                _ => continue,
            };
            match runtime_for_profiles.block_on(core.assign_account_profile(account_id, assignment))
            {
                Ok(updated) => settings = updated,
                Err(error) => failure = Some(error),
            }
        }
        apply_account_presentation_settings(
            &app,
            &state_for_profiles,
            &runtime_for_profiles,
            &settings,
        );
        match failure {
            Some(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account profile: {}",
                error,
            )),
            None => app.set_sync_status(UiMessage::plain("Profile saved.")),
        }
        profile.id.into()
    });

    let app_weak = app.as_weak();
    app.on_prepare_profile_members(move |profile_id, checked_account| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let rows = connected_account_rows(&app)
            .into_iter()
            .map(|account| {
                let member = !profile_id.is_empty() && account.profile_id == profile_id;
                ProfileMemberRow {
                    id: account.id,
                    name: account.name,
                    email: account.email,
                    initials: account.initials,
                    avatar: account.avatar_small,
                    has_avatar: account.has_avatar,
                    other_profile: if member {
                        Default::default()
                    } else {
                        account.profile_name
                    },
                    checked: member || account.id == checked_account,
                }
            })
            .collect::<Vec<_>>();
        app.set_profile_members(ModelRc::new(VecModel::from(rows)));
    });

    let app_weak = app.as_weak();
    let state_for_profile_assignment = Rc::clone(state);
    let runtime_for_profile_assignment = runtime.clone();
    app.on_assign_account_profile(move |account_id, profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_profile_assignment.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        match runtime_for_profile_assignment
            .block_on(core.assign_account_profile(i64::from(account_id), profile_id))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_profile_assignment,
                    &runtime_for_profile_assignment,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Account profile updated."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account profile: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_account_color = Rc::clone(state);
    let runtime_for_account_color = runtime.clone();
    app.on_save_account_color(move |account_id, color, generated| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_account_color.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Account color changes require local mail data.",
            ));
            return;
        };
        let color = (!generated).then(|| theme::color_to_hex(color));
        match runtime_for_account_color
            .block_on(core.set_account_color(i64::from(account_id), color))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_account_color,
                    &runtime_for_account_color,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Account color updated."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account color: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_active_profile = Rc::clone(state);
    let runtime_for_active_profile = runtime.clone();
    app.on_select_mail_profile(move |profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_active_profile.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        match runtime_for_active_profile.block_on(core.set_active_mail_profile(profile_id)) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_active_profile,
                &runtime_for_active_profile,
                &settings,
            ),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not switch profile: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_profile_delete = Rc::clone(state);
    let runtime_for_profile_delete = runtime.clone();
    app.on_delete_mail_profile(move |profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_profile_delete.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        match runtime_for_profile_delete.block_on(core.delete_mail_profile(profile_id.to_string()))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_profile_delete,
                    &runtime_for_profile_delete,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Profile deleted."));
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not delete profile: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_marker_visibility = Rc::clone(state);
    let runtime_for_marker_visibility = runtime.clone();
    app.on_save_account_markers_visibility(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_marker_visibility.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Marker preference is active for this session.",
            ));
            state_for_marker_visibility
                .borrow_mut()
                .account_presentation
                .show_markers = enabled;
            refresh_rows_only(
                &app,
                &state_for_marker_visibility,
                &runtime_for_marker_visibility,
            );
            return;
        };
        match runtime_for_marker_visibility.block_on(core.set_show_account_badges(enabled)) {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_marker_visibility,
                    &runtime_for_marker_visibility,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Mail-list marker preference saved."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save mail-list marker preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_workspace_layout = Rc::clone(state);
    let runtime_for_workspace_layout = runtime.clone();
    app.on_save_workspace_layout(move |layout| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_workspace_layout.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Workspace layout updated for this session.",
            ));
            return;
        };
        match runtime_for_workspace_layout.block_on(core.set_workspace_layout(layout.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Workspace layout preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save workspace layout: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_list_pane_width = Rc::clone(state);
    let runtime_for_list_pane_width = runtime.clone();
    app.on_save_workspace_list_pane_width(move |width| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let width =
            flectar_mail_core::models::normalized_workspace_list_pane_width(width.round() as i64);
        app.set_workspace_list_pane_width(width as f32);
        let Some(core) = state_for_list_pane_width.borrow().core.clone() else {
            return;
        };
        if let Err(error) =
            runtime_for_list_pane_width.block_on(core.set_workspace_list_pane_width(width))
        {
            app.set_sync_status(UiMessage::detail(
                "Could not save list pane width: {}",
                error,
            ));
        }
    });

    // Calendar-only accounts stay in Settings so this switch can bring them
    // back, but leave every mail surface at once.
    let app_weak = app.as_weak();
    let state_for_calendar_only = Rc::clone(state);
    let runtime_for_calendar_only = runtime.clone();
    app.global::<AccountMailPreferences>()
        .on_set_calendar_only(move |account_id, enabled| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(core) = state_for_calendar_only.borrow().core.clone() else {
                app.set_sync_status(UiMessage::plain("Folder settings require local mail data."));
                return;
            };
            match runtime_for_calendar_only
                .block_on(core.set_account_calendar_only(i64::from(account_id), enabled))
            {
                Ok(()) => {
                    app.global::<AccountMailPreferences>()
                        .set_calendar_only(enabled);
                    let accounts = runtime_for_calendar_only.block_on(core.load_accounts());
                    let configs = runtime_for_calendar_only.block_on(core.load_account_configs());
                    match (accounts, configs) {
                        (Ok(accounts), Ok(configs)) => {
                            crate::account_controller::update_connected_accounts(
                                &app,
                                &state_for_calendar_only,
                                &runtime_for_calendar_only,
                                accounts,
                                configs,
                            );
                            if let Err(error) = crate::mail_view_model::refresh_from_source(
                                &app,
                                &state_for_calendar_only,
                                &runtime_for_calendar_only,
                                false,
                                &[],
                            ) {
                                app.set_render_status(UiMessage::detail(
                                    "Mail refresh failed: {}",
                                    error,
                                ));
                            }
                            app.set_sync_status(UiMessage::plain("Folder settings updated."));
                        }
                        (Err(error), _) | (_, Err(error)) => app.set_sync_status(
                            UiMessage::detail("Could not update folder settings: {}", error),
                        ),
                    }
                }
                Err(error) => app.set_sync_status(UiMessage::detail(
                    "Could not update folder settings: {}",
                    error,
                )),
            }
        });
}

/// Both entry points share one asynchronous persistence path and busy state.
/// Check spelling or not, in `languages` or the system's, from the settings or a message's
/// menu. Open messages follow at once.
pub(super) fn save_spelling(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    enabled: bool,
    languages: Option<Vec<String>>,
) {
    crate::spelling::apply_settings(enabled, languages.clone());
    crate::spelling::apply_to_window(app);
    let Some(core) = state.borrow().core.clone() else {
        return;
    };
    if let Err(error) = runtime.block_on(core.set_spelling(enabled, languages)) {
        app.set_sync_status(UiMessage::detail(
            "Could not save spelling preference: {}",
            error,
        ));
    }
}

pub(super) fn register_oauth_settings_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    let weak = app.as_weak();
    let single_state = Rc::clone(state);
    let single_runtime = runtime.clone();
    app.on_save_oauth_app(move |provider, client_id, client_secret| {
        let Some(app) = weak.upgrade() else { return };
        let (google, microsoft) = match provider.as_str() {
            "gmail" => (
                Some((client_id.to_string(), client_secret.to_string())),
                None,
            ),
            "microsoft" => (None, Some(client_id.to_string())),
            _ => return,
        };
        save_oauth_settings(
            &app,
            &single_state,
            &single_runtime,
            google,
            microsoft,
            false,
        );
    });

    let weak = app.as_weak();
    let state = Rc::clone(state);
    let runtime = runtime.clone();
    app.on_save_oauth_apps(move |google_id, google_secret, microsoft_id| {
        let Some(app) = weak.upgrade() else { return };
        save_oauth_settings(
            &app,
            &state,
            &runtime,
            Some((google_id.to_string(), google_secret.to_string())),
            Some(microsoft_id.to_string()),
            true,
        );
    });
}

fn save_oauth_settings(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    google: Option<(String, String)>,
    microsoft: Option<String>,
    close_dialog: bool,
) {
    if app.get_oauth_settings_saving() || app.get_oauth_in_progress() {
        return;
    }
    let Some(core) = state.borrow().core.clone() else {
        let message = UiMessage::plain("Local mail data is unavailable. Retry startup.");
        app.set_sync_status(message.clone());
        app.set_oauth_settings_error(ui_message::translated(app, &message));
        return;
    };
    let update_google = google.is_some();
    let update_microsoft = microsoft.is_some();
    app.set_oauth_settings_error("".into());
    app.set_oauth_settings_saving(true);
    let weak = app.as_weak();
    runtime.spawn(async move {
        let result = core.set_oauth_apps(google, microsoft).await;
        let _ = weak.upgrade_in_event_loop(move |app| {
            app.set_oauth_settings_saving(false);
            match result {
                Ok(settings) => {
                    if update_google {
                        app.set_google_client_id(settings.google_client_id.clone().into());
                        app.set_google_client_secret(settings.google_client_secret.clone().into());
                    }
                    if update_microsoft {
                        app.set_ms_client_id(settings.ms_client_id.clone().into());
                    }
                    app.set_custom_oauth_configured(
                        !settings.google_client_id.is_empty() || !settings.ms_client_id.is_empty(),
                    );
                    startup::refresh_oauth_availability(&app);
                    app.set_sync_status(UiMessage::plain("OAuth app keys saved."));
                    if close_dialog {
                        app.set_oauth_setup_open(false);
                    }
                }
                Err(error) => {
                    let message = UiMessage::detail("OAuth settings failed: {}", error);
                    app.set_sync_status(message.clone());
                    app.set_oauth_settings_error(ui_message::translated(&app, &message));
                }
            }
        });
    });
}

/// Writes one read-aloud preference through the core as a whole settings row.
fn save_tts_setting(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    change: impl FnOnce(&mut flectar_mail_core::models::Settings),
) -> bool {
    match write_settings(state, runtime, change) {
        Some(Ok(())) => {
            app.set_sync_status(UiMessage::plain("Read-aloud settings saved."));
            true
        }
        Some(Err(error)) => {
            app.set_sync_status(UiMessage::detail(
                "Could not save the read-aloud settings: {}",
                error,
            ));
            false
        }
        None => false,
    }
}

/// Persist a notification preference; its status line speaks about
/// notifications rather than read-aloud.
fn save_notification_setting(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    change: impl FnOnce(&mut flectar_mail_core::models::Settings),
) -> bool {
    match write_settings(state, runtime, change) {
        Some(Ok(())) => {
            app.set_sync_status(UiMessage::plain("Notification preferences saved."));
            true
        }
        Some(Err(error)) => {
            app.set_sync_status(UiMessage::detail("Could not save notifications: {}", error));
            false
        }
        None => false,
    }
}

/// Persist an AI setting; the status line speaks about AI, not read-aloud.
fn save_ai_setting(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    change: impl FnOnce(&mut flectar_mail_core::models::Settings),
) -> bool {
    match write_settings(state, runtime, change) {
        Some(Ok(())) => {
            app.set_sync_status(UiMessage::plain("AI settings saved."));
            true
        }
        Some(Err(error)) => {
            app.set_sync_status(UiMessage::detail("Could not save AI settings: {}", error));
            false
        }
        None => false,
    }
}

/// Write one setting through the core. `None` means there is no core yet;
/// otherwise the write's result, so the caller says what happened in its own
/// words. A failed write leaves the UI as it was, so Settings never shows
/// something that was not stored.
fn write_settings(
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    change: impl FnOnce(&mut flectar_mail_core::models::Settings),
) -> Option<std::result::Result<(), String>> {
    let core = state.borrow().core.as_ref().map(|core| core.file_core())?;
    Some(runtime.block_on(async move {
        let mut settings = core.get_settings().await.map_err(|error| error.to_string())?;
        change(&mut settings);
        core.set_settings(settings)
            .await
            .map_err(|error| error.to_string())
    }))
}

/// Persist a model picked in Settings > AI agents: one tier, or every tier
/// when the pick came from the shared picker. The caller re-probes afterwards
/// so the status line reflects the new models. Returns whether it was stored.
fn save_ai_model_pick(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    tier: Option<AiTier>,
    model: &str,
) -> bool {
    let model = model.trim().to_owned();
    if model.is_empty() {
        return false;
    }
    let saved = save_ai_setting(app, state, runtime, |settings| match tier {
        Some(tier) => tier.set_model(settings, &model),
        None => {
            settings.ai_model_instant = model.clone();
            settings.ai_model_cheap = model.clone();
            settings.ai_model_intelligent = model.clone();
        }
    });
    if saved {
        // Show the pick before the probe returns, so the dropdown keeps it.
        crate::services::publish_ai_pick(app, tier, &model);
    }
    saved
}

/// Reloads the engine's stored key and live voice list, then shows what the
/// speech layer will speak with.
pub(crate) fn refresh_tts(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    let Some(core) = state.borrow().core.as_ref().map(|core| core.file_core()) else {
        return;
    };
    let weak = app.as_weak();
    runtime.spawn(async move {
        let Ok(config) = core.tts_config().await else {
            return;
        };
        let voices = core.tts_voices(config.engine.id()).await.unwrap_or_default();
        crate::speech::set_config(config.clone());
        let _ = weak.upgrade_in_event_loop(move |app| {
            crate::startup::publish_tts_voices(&app, &config, voices);
            app.set_tts_needs_key(config.engine.needs_key());
            app.set_tts_key_present(!config.api_key.trim().is_empty());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_settings_discards_an_unsaved_palette() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        struct Headless(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window))).unwrap();
        let app = AppWindow::new().unwrap();
        let theme = app.global::<AppTheme>();
        let draft = app.global::<ThemeDraft>();
        let saved = theme.get_custom_light_primary();
        app.invoke_open_settings();
        slint::platform::update_timers_and_animations();

        // The editor previews a color before it is saved.
        draft.invoke_begin();
        theme.set_custom_light_primary(slint::Color::from_rgb_u8(0xff, 0, 0));
        app.invoke_close_settings();
        slint::platform::update_timers_and_animations();
        assert_eq!(theme.get_custom_light_primary(), saved);
        assert!(!draft.get_dirty());

        // A saved palette stays.
        app.invoke_open_settings();
        slint::platform::update_timers_and_animations();
        draft.invoke_begin();
        let chosen = slint::Color::from_rgb_u8(0, 0x80, 0);
        theme.set_custom_light_primary(chosen);
        draft.set_dirty(false);
        app.invoke_close_settings();
        slint::platform::update_timers_and_animations();
        assert_eq!(theme.get_custom_light_primary(), chosen);
    }

    /// The two pick paths: a per-tier pick stores that tier alone, and the
    /// shared "every feature" pick stores all three, through the same settings
    /// write the rest of Settings uses.
    #[test]
    fn picking_a_model_persists_the_tiers_it_covers() {
        use flectar_mail_core::accounts::credentials::DevelopmentFileCredentialStore;
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        struct Headless(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window))).unwrap();
        let app = AppWindow::new().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let core = runtime
            .block_on(CoreMailSource::start(
                flectar_mail_core::config::Paths::for_tests(dir.path()),
                Arc::new(DevelopmentFileCredentialStore::new(
                    dir.path().join("credentials.json"),
                )),
                Arc::new(flectar_mail_core::oauth::redirect::LoopbackRedirectBroker::default()),
            ))
            .unwrap();
        let (favicons, _) = bounded_ui_channel();
        let (avatars, _) = bounded_ui_channel();
        let mut state = InboxState::empty(
            None,
            UiSender::new(favicons, UiWake::new(app.as_weak(), |_| {})),
            None,
            UiSender::new(avatars, UiWake::new(app.as_weak(), |_| {})),
            false,
            false,
            WarmStartCacheWriter::spawn(runtime.handle(), dir.path().join("warm.json")),
        );
        state.core = Some(core.clone());
        let state = Rc::new(RefCell::new(state));
        let stored = || runtime.block_on(core.file_core().get_settings()).unwrap();

        // A pick for one tier changes only that tier.
        assert!(save_ai_model_pick(
            &app,
            &state,
            runtime.handle(),
            Some(AiTier::Instant),
            "qwen3.5:9b",
        ));
        let settings = stored();
        assert_eq!(settings.ai_model_instant, "qwen3.5:9b");
        assert_ne!(settings.ai_model_cheap, "qwen3.5:9b");
        assert_ne!(settings.ai_model_intelligent, "qwen3.5:9b");

        // The shared picker sets every tier.
        assert!(save_ai_model_pick(
            &app,
            &state,
            runtime.handle(),
            None,
            "glm-ocr:latest",
        ));
        let settings = stored();
        assert_eq!(settings.ai_model_instant, "glm-ocr:latest");
        assert_eq!(settings.ai_model_cheap, "glm-ocr:latest");
        assert_eq!(settings.ai_model_intelligent, "glm-ocr:latest");
    }
}
