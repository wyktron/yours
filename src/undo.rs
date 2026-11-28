//! Taking back what was just done, from a notice at the foot of the main
//! window: a message sent with an undo delay, which waits out the delay the
//! user chose (#225), or mail just archived, moved, deleted, marked or
//! starred (#226). Without a delay the composer waits for a message itself
//! (src/composer.rs).

use super::*;
use crate::mail_work::UndoableBatch;
use flectar_mail_core::models::QueueSendResult;

/// How long "Message sent." stays.
const SENT_NOTICE_FOR: Duration = Duration::from_secs(4);
/// How long the notice of an archive, a move or a change of marks stays,
/// with Undo.
const GESTURE_NOTICE_FOR: Duration = Duration::from_secs(10);
/// How long a notice that something could not be undone stays.
const ANSWER_NOTICE_FOR: Duration = Duration::from_secs(4);
/// How long the one-time missing-key breach hint stays. Its x dismisses it
/// sooner; the stored flag is what keeps it from ever returning.
const BREACH_HINT_FOR: Duration = Duration::from_secs(20);
/// At exit, how long messages still in their delay may take to leave, so
/// that they don't wait for the next start.
const EXIT_SEND_LIMIT: Duration = Duration::from_secs(10);

/// A message on its way out this run.
struct Pending {
    action_id: i64,
    draft_id: i64,
    /// Taken back: how it ended is no news.
    undone: bool,
    /// Its time is still ahead: quitting must leave it queued, not send it now.
    scheduled: bool,
}

/// What the notice tells of.
#[derive(Clone, PartialEq, Eq)]
enum Shown {
    /// A sent message, and its draft.
    Send { action_id: i64, draft_id: i64 },
    /// The actions of one archive, move, deletion or change of marks.
    Gesture { action_ids: Vec<i64> },
    /// A sender just blocked, and the moves of their inbox mail to Spam.
    Block {
        address: String,
        action_ids: Vec<i64>,
    },
    /// News with nothing to take back.
    Note,
}

struct Undo {
    app: slint::Weak<AppWindow>,
    state: Rc<RefCell<InboxState>>,
    runtime: tokio::runtime::Handle,
    pending: RefCell<Vec<Pending>>,
    shown: RefCell<Option<Shown>>,
    /// Counts the notices shown, so that a timer hides only its own.
    serial: Cell<u64>,
    hide: slint::Timer,
}

thread_local! {
    static UNDO: RefCell<Option<Rc<Undo>>> = const { RefCell::new(None) };
}

fn with_undo(f: impl FnOnce(&Rc<Undo>)) {
    UNDO.with(|slot| {
        if let Some(undo) = slot.borrow().as_ref() {
            f(undo);
        }
    });
}

/// Connect the notice to the main window.
pub(super) fn install(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
) {
    UNDO.with(|slot| {
        *slot.borrow_mut() = Some(Rc::new(Undo {
            app: app.as_weak(),
            state: Rc::clone(state),
            runtime: runtime.clone(),
            pending: RefCell::default(),
            shown: RefCell::default(),
            serial: Cell::new(0),
            hide: slint::Timer::default(),
        }));
    });
    let notice = app.global::<Notice>();
    notice.on_act(|| with_undo(|undo| undo.act()));
    notice.on_dismiss(|| with_undo(|undo| undo.hide_notice()));
}

/// Whether a queued message waits out an undo delay before it leaves.
pub(super) fn holds(queued: &QueueSendResult) -> bool {
    queued.dispatch_at > chrono::Utc::now().timestamp_millis()
}

/// A message left its composer and waits out its delay.
pub(super) fn sending(app: &AppWindow, queued: QueueSendResult) {
    with_undo(|undo| {
        let Some(core) = undo.state.borrow().core.clone() else {
            return;
        };
        undo.pending.borrow_mut().push(Pending {
            action_id: queued.action_id,
            draft_id: queued.draft_id,
            undone: false,
            scheduled: false,
        });
        undo.show(
            app,
            Shown::Send {
                action_id: queued.action_id,
                draft_id: queued.draft_id,
            },
            UiMessage::plain("Sending message…"),
            "undo",
            None,
        );
        let action_id = queued.action_id;
        undo.runtime.spawn(async move {
            // A phone keeps the app running until the message is out.
            #[cfg(background_checks)]
            let _awake = crate::platform::stay_awake();
            let result = core.wait_for_send(action_id).await;
            let _ = slint::invoke_from_event_loop(move || {
                with_undo(|undo| undo.finished(action_id, result));
            });
        });
    });
}

/// A message took a send-later time: the notice names it, and the queue keeps
/// it until then. It must not leave early when the app quits.
pub(super) fn scheduled(app: &AppWindow, queued: QueueSendResult, at_secs: i64) {
    with_undo(|undo| {
        let Some(core) = undo.state.borrow().core.clone() else {
            return;
        };
        undo.pending.borrow_mut().push(Pending {
            action_id: queued.action_id,
            draft_id: queued.draft_id,
            undone: false,
            scheduled: true,
        });
        undo.show(
            app,
            Shown::Send {
                action_id: queued.action_id,
                draft_id: queued.draft_id,
            },
            UiMessage::detail("Scheduled to send {}.", scheduled_at(at_secs)),
            "undo",
            None,
        );
        let action_id = queued.action_id;
        let dispatch_at = queued.dispatch_at;
        undo.runtime.spawn(async move {
            // Sleep until its time rather than polling the queue for hours.
            let wait_ms = (dispatch_at - chrono::Utc::now().timestamp_millis()).max(0);
            tokio::time::sleep(Duration::from_millis(wait_ms as u64)).await;
            // A phone keeps the app running until the message is out.
            #[cfg(background_checks)]
            let _awake = crate::platform::stay_awake();
            let result = core.wait_for_send(action_id).await;
            let _ = slint::invoke_from_event_loop(move || {
                with_undo(|undo| undo.finished(action_id, result));
            });
        });
    });
}

/// The local date and time a scheduled send is due, as its notice names it.
fn scheduled_at(at_secs: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(at_secs, 0)
        .single()
        .map(|at| {
            crate::date_format::current()
                .date_time(crate::date_format::DateStyle::Medium, at.naive_local())
        })
        .unwrap_or_default()
}

/// Mail was just archived, moved, deleted, marked or starred: tell of it,
/// with Undo.
pub(super) fn gesture(app: &AppWindow, action_ids: Vec<i64>, batch: UndoableBatch) {
    with_undo(|undo| {
        undo.show(
            app,
            Shown::Gesture { action_ids },
            gesture_message(batch),
            "undo",
            Some(GESTURE_NOTICE_FOR),
        );
    });
}

/// A sender was just blocked: tell of it, with Undo, which unblocks them
/// and puts their mail back.
pub(super) fn blocked(app: &AppWindow, address: String, action_ids: Vec<i64>) {
    with_undo(|undo| {
        let message = UiMessage::detail("Blocked {}.", &address);
        undo.show(
            app,
            Shown::Block {
                address,
                action_ids,
            },
            message,
            "undo",
            Some(GESTURE_NOTICE_FOR),
        );
    });
}

/// A rule just ran on `count` messages in the Inbox: tell of it, with Undo.
pub(super) fn ruled(app: &AppWindow, count: i32, action_ids: Vec<i64>) {
    with_undo(|undo| {
        undo.show(
            app,
            Shown::Gesture { action_ids },
            UiMessage::detail("Rule applied to {} messages.", count),
            "undo",
            Some(GESTURE_NOTICE_FOR),
        );
    });
}

/// Tell of something done that cannot be taken back here, such as leaving
/// a mailing list.
pub(super) fn tell(app: &AppWindow, message: UiMessage) {
    with_undo(|undo| undo.show(app, Shown::Note, message, "", Some(ANSWER_NOTICE_FOR)));
}

/// The one-time hint that the breach scan could not reach its provider. Its
/// button opens Settings > General, where the switch and optional HIBP key
/// live; the x dismisses it. The caller has already stored the shown flag, so
/// it never repeats.
pub(super) fn breach_hint(app: &AppWindow) {
    with_undo(|undo| {
        undo.show(
            app,
            Shown::Note,
            // A literal: the Slint dispatcher owns the wording and its
            // translation, like every other UiMessage key.
            UiMessage::plain(
                "Breach scan is on, but the breach provider could not be reached - check your connection.",
            ),
            "breach-key",
            Some(BREACH_HINT_FOR),
        );
    });
}

/// What the notice says of a batch.
fn gesture_message(batch: UndoableBatch) -> UiMessage {
    let count = batch.count;
    match (batch.kind, batch.conversations) {
        ("archive", true) => UiMessage::detail("{} conversations archived.", count),
        ("archive", false) => UiMessage::detail("{} messages archived.", count),
        ("trash", true) => UiMessage::detail("{} conversations moved to Trash.", count),
        ("trash", false) => UiMessage::detail("{} messages moved to Trash.", count),
        ("spam", true) => UiMessage::detail("{} conversations marked as spam.", count),
        ("spam", false) => UiMessage::detail("{} messages marked as spam.", count),
        ("move", true) => UiMessage::detail("{} conversations moved.", count),
        ("move", false) => UiMessage::detail("{} messages moved.", count),
        ("read", true) => UiMessage::detail("{} conversations marked as read.", count),
        ("read", false) => UiMessage::detail("{} messages marked as read.", count),
        ("unread", true) => UiMessage::detail("{} conversations marked as unread.", count),
        ("unread", false) => UiMessage::detail("{} messages marked as unread.", count),
        ("star", true) => UiMessage::detail("{} conversations starred.", count),
        ("star", false) => UiMessage::detail("{} messages starred.", count),
        (_, true) => UiMessage::detail("{} conversations unstarred.", count),
        (_, false) => UiMessage::detail("{} messages unstarred.", count),
    }
}

/// Whether a send still waiting may leave at exit. One waiting out the undo
/// delay does: it was meant to go now. A scheduled one does not: its time may
/// be days away, and the queue keeps it across restarts.
fn leaves_at_exit(pending: &Pending) -> bool {
    !pending.undone && !pending.scheduled
}

/// The app is ending: messages still waiting out their delay leave now
/// rather than at the next start.
pub(super) fn send_before_exit(runtime: &tokio::runtime::Handle) {
    with_undo(|undo| {
        let waiting = undo
            .pending
            .borrow()
            .iter()
            .filter(|pending| leaves_at_exit(pending))
            .map(|pending| pending.action_id)
            .collect::<Vec<_>>();
        let Some(core) = undo.state.borrow().core.clone() else {
            return;
        };
        if waiting.is_empty() {
            return;
        }
        // The time limit is set inside the runtime, whose timer it needs.
        let _ = runtime.block_on(async {
            tokio::time::timeout(EXIT_SEND_LIMIT, async {
                for &action_id in &waiting {
                    let _ = core.send_now(action_id).await;
                }
                for &action_id in &waiting {
                    let _ = core.wait_for_send(action_id).await;
                }
            })
            .await
        });
    });
}

impl Undo {
    /// Show `message` about `shown`; `action` is what its button does.
    fn show(
        self: &Rc<Self>,
        app: &AppWindow,
        shown: Shown,
        message: UiMessage,
        action: &str,
        hide_after: Option<Duration>,
    ) {
        let notice = app.global::<Notice>();
        notice.set_text(message);
        notice.set_action(action.into());
        notice.set_error(action == "open-draft");
        notice.set_shown(true);
        *self.shown.borrow_mut() = Some(shown);
        let serial = self.serial.get().wrapping_add(1);
        self.serial.set(serial);
        match hide_after {
            Some(after) => {
                let weak = Rc::downgrade(self);
                self.hide
                    .start(slint::TimerMode::SingleShot, after, move || {
                        if let Some(undo) = weak.upgrade()
                            && undo.serial.get() == serial
                        {
                            undo.hide_notice();
                        }
                    });
            }
            None => self.hide.stop(),
        }
    }

    fn hide_notice(&self) {
        self.hide.stop();
        *self.shown.borrow_mut() = None;
        if let Some(app) = self.app.upgrade() {
            app.global::<Notice>().set_shown(false);
        }
    }

    /// The notice's button: take back what it tells of, or open the draft
    /// of a message that could not be sent.
    fn act(self: &Rc<Self>) {
        let (Some(app), Some(shown)) = (self.app.upgrade(), self.shown.borrow().clone()) else {
            return;
        };
        let action = app.global::<Notice>().get_action();
        match (action.as_str(), shown) {
            (
                "undo",
                Shown::Send {
                    action_id,
                    draft_id,
                },
            ) => self.undo_send(&app, action_id, draft_id),
            ("undo", Shown::Gesture { action_ids }) => self.undo_gesture(&app, action_ids),
            (
                "undo",
                Shown::Block {
                    address,
                    action_ids,
                },
            ) => self.undo_block(&app, address, action_ids),
            ("open-draft", Shown::Send { draft_id, .. }) => {
                self.hide_notice();
                composer::reopen_draft(draft_id);
            }
            // The missing-key hint's button: open the page that owns the key.
            ("breach-key", Shown::Note) => {
                self.hide_notice();
                app.set_settings_tab("General".into());
                app.invoke_open_settings();
            }
            _ => {}
        }
    }

    fn undo_send(self: &Rc<Self>, app: &AppWindow, action_id: i64, draft_id: i64) {
        let Some(core) = self.state.borrow().core.clone() else {
            return;
        };
        let shown = Shown::Send {
            action_id,
            draft_id,
        };
        match self.runtime.block_on(core.cancel_send(action_id)) {
            Ok(true) => {
                if let Some(pending) = self
                    .pending
                    .borrow_mut()
                    .iter_mut()
                    .find(|pending| pending.action_id == action_id)
                {
                    pending.undone = true;
                }
                self.hide_notice();
                composer::reopen_draft(draft_id);
            }
            // Already on its way: its end comes as usual.
            Ok(false) => self.show(
                app,
                shown,
                UiMessage::plain("This message has already been sent."),
                "",
                None,
            ),
            Err(error) => self.show(
                app,
                shown,
                UiMessage::detail("Could not take the message back: {}", error),
                "",
                None,
            ),
        }
    }

    fn undo_gesture(self: &Rc<Self>, app: &AppWindow, action_ids: Vec<i64>) {
        let Some(core) = self.state.borrow().core.clone() else {
            return;
        };
        let shown = Shown::Gesture {
            action_ids: action_ids.clone(),
        };
        // The mail list follows the core's update of the mail put back.
        match self.runtime.block_on(core.undo_actions(action_ids)) {
            Ok(true) => self.hide_notice(),
            Ok(false) => self.show(
                app,
                shown,
                UiMessage::plain("This can no longer be undone."),
                "",
                Some(ANSWER_NOTICE_FOR),
            ),
            Err(error) => self.show(
                app,
                shown,
                UiMessage::detail("Could not undo: {}", error),
                "",
                Some(ANSWER_NOTICE_FOR),
            ),
        }
    }

    fn undo_block(self: &Rc<Self>, app: &AppWindow, address: String, action_ids: Vec<i64>) {
        let Some(core) = self.state.borrow().core.clone() else {
            return;
        };
        let shown = Shown::Block {
            address: address.clone(),
            action_ids: action_ids.clone(),
        };
        let unblocked = self.runtime.block_on(core.unblock_sender(&address));
        if unblocked.is_ok() {
            crate::block_sender::unblocked(&address);
        }
        let result = match unblocked {
            Err(error) => Err(error),
            Ok(()) if action_ids.is_empty() => Ok(true),
            Ok(()) => self.runtime.block_on(core.undo_actions(action_ids)),
        };
        match result {
            Ok(true) => self.hide_notice(),
            Ok(false) => self.show(
                app,
                shown,
                UiMessage::plain("This can no longer be undone."),
                "",
                Some(ANSWER_NOTICE_FOR),
            ),
            Err(error) => self.show(
                app,
                shown,
                UiMessage::detail("Could not undo: {}", error),
                "",
                Some(ANSWER_NOTICE_FOR),
            ),
        }
    }

    /// A message left, or could not.
    fn finished(self: &Rc<Self>, action_id: i64, result: Result<(), String>) {
        let ended = {
            let mut pending = self.pending.borrow_mut();
            pending
                .iter()
                .position(|pending| pending.action_id == action_id)
                .map(|index| pending.remove(index))
        };
        let (Some(ended), Some(app)) = (ended, self.app.upgrade()) else {
            return;
        };
        if ended.undone {
            return;
        }
        let _ = refresh_from_source(&app, &self.state, &self.runtime, true, &[]);
        let shown = Shown::Send {
            action_id,
            draft_id: ended.draft_id,
        };
        let showing = self.shown.borrow().as_ref() == Some(&shown);
        match result {
            Ok(()) if showing => self.show(
                &app,
                shown,
                UiMessage::plain("Message sent."),
                "",
                Some(SENT_NOTICE_FOR),
            ),
            Ok(()) => {}
            // The draft stays in Drafts to fix and send again.
            Err(error) => self.show(
                &app,
                shown,
                UiMessage::detail("Message was not sent: {} Your draft is preserved.", error),
                "open-draft",
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Pending, leaves_at_exit, scheduled_at};

    fn waiting(scheduled: bool, undone: bool) -> Pending {
        Pending {
            action_id: 1,
            draft_id: 2,
            undone,
            scheduled,
        }
    }

    #[test]
    fn verify_a_scheduled_send_waits_for_its_time_instead_of_leaving_at_exit() {
        assert!(
            leaves_at_exit(&waiting(false, false)),
            "a send in its undo delay leaves now"
        );
        assert!(
            !leaves_at_exit(&waiting(true, false)),
            "a scheduled send waits for its own time"
        );
        assert!(!leaves_at_exit(&waiting(false, true)));
        assert!(!leaves_at_exit(&waiting(true, true)));
    }

    #[test]
    fn verify_a_scheduled_send_names_its_local_date_and_time() {
        let label = scheduled_at(1_800_000_000);
        assert!(label.contains("2027"), "the date is shown: {label}");
        assert!(label.contains(':'), "the time is shown: {label}");
        assert_eq!(
            scheduled_at(1_800_000_000),
            label,
            "the same instant reads the same"
        );
        assert_ne!(scheduled_at(1_800_000_000), scheduled_at(1_800_003_600));
    }
}
