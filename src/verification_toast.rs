//! The corner toast for a new message that asks for a one-time code. It
//! appears without taking the keyboard, offers the code for copying, and
//! opens the message. Core reports arrivals as MailNew whether or not the
//! system shows a notification, so the toast works while the window is open
//! and the user is anywhere in it.

use crate::{AppWindow, CodeToast};
use flectar_mail_core::{Core, models::VerificationCodeMail, models::now_ms};
use slint::{ComponentHandle, Weak};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// How long the toast stays up before it takes itself away.
const TOAST_LIFETIME: Duration = Duration::from_secs(20);
/// Let the rest of a sync step land before reading its new conversations.
const BURST_QUIET: Duration = Duration::from_millis(750);
/// A sync catching up replays old mail through the same arrival event, so only
/// a message this recent is news, as the voice's announcement also decides.
const RECENT_MS: i64 = 5 * 60 * 1000;

/// Hands the conversations of a new-mail event to the toast worker.
pub(crate) struct CodeToasts(UnboundedSender<Vec<i64>>);

impl CodeToasts {
    /// Note that new mail reached these conversations.
    pub(crate) fn arrived(&self, thread_ids: Vec<i64>) {
        // The worker ends with the app; a send after that is nothing to show.
        let _ = self.0.send(thread_ids);
    }
}

/// Start watching new mail for verification codes, showing the newest one.
pub(crate) fn spawn(
    runtime: &tokio::runtime::Handle,
    core: Arc<Core>,
    app: Weak<AppWindow>,
) -> CodeToasts {
    let (sender, arrivals) = unbounded_channel();
    runtime.spawn(watch(runtime.clone(), core, app, arrivals));
    CodeToasts(sender)
}

async fn watch(
    runtime: tokio::runtime::Handle,
    core: Arc<Core>,
    app: Weak<AppWindow>,
    mut arrivals: UnboundedReceiver<Vec<i64>>,
) {
    // A message can be named by more than one event; show each code once.
    let mut shown = BTreeSet::new();
    while let Some(mut thread_ids) = arrivals.recv().await {
        // Core reports an account or protocol step at a time, so let the rest
        // of the burst arrive before reading the conversations.
        tokio::time::sleep(BURST_QUIET).await;
        while let Ok(more) = arrivals.try_recv() {
            thread_ids.extend(more);
        }
        thread_ids.sort_unstable();
        thread_ids.dedup();
        let mails = match core.verification_code_arrivals(thread_ids).await {
            Ok(mails) => mails,
            Err(error) => {
                tracing::warn!(%error, "code toast: could not read new mail");
                continue;
            }
        };
        let now = now_ms();
        let Some(mail) = mails.into_iter().find(|mail| {
            toast_worthy(Some(mail.code.as_str()), mail.received_at, now)
                && !shown.contains(&mail.message_id)
        }) else {
            continue;
        };
        shown.insert(mail.message_id);
        // A line in the log when the toast is raised, so a report can tell a
        // code that never arrived from one the window did not show.
        eprintln!(
            "code toast: showing {} from {} for message {}",
            mail.code, mail.sender, mail.message_id
        );
        show(&runtime, &app, mail);
    }
}

/// Whether an arriving code deserves the toast: it has a code, and it is new
/// enough that a sync is not merely replaying the mailbox.
fn toast_worthy(code: Option<&str>, received_at: i64, now: i64) -> bool {
    code.is_some_and(|code| !code.trim().is_empty()) && now.saturating_sub(received_at) <= RECENT_MS
}

/// Put the toast on screen on the window's thread, then take it away if it is
/// still the one showing after TOAST_LIFETIME.
fn show(runtime: &tokio::runtime::Handle, app: &Weak<AppWindow>, mail: VerificationCodeMail) {
    let account_id = to_i32(mail.account_id);
    let thread_id = to_i32(mail.thread_id);
    let toast_id = to_i32(mail.message_id);
    let weak = app.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let toast = app.global::<CodeToast>();
        toast.set_account_id(account_id);
        toast.set_thread_id(thread_id);
        toast.set_message_id(toast_id);
        toast.set_sender(mail.sender.into());
        toast.set_address(mail.address.into());
        toast.set_subject(mail.subject.into());
        toast.set_code(mail.code.into());
        toast.set_shown(true);
    });

    let runtime = runtime.clone();
    let weak = app.clone();
    runtime.spawn(async move {
        tokio::time::sleep(TOAST_LIFETIME).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let toast = app.global::<CodeToast>();
            // A newer arrival may have replaced this one.
            if toast.get_shown() && toast.get_message_id() == toast_id {
                toast.set_shown(false);
            }
        });
    });
}

fn to_i32(id: i64) -> i32 {
    i32::try_from(id).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_recent_arrivals_with_a_code_deserve_the_toast() {
        let now = 1_800_000_000_000;
        assert!(toast_worthy(Some("482913"), now - 1_000, now));
        assert!(toast_worthy(Some("482913"), now - RECENT_MS, now));
        assert!(!toast_worthy(Some("482913"), now - RECENT_MS - 1, now));
        assert!(!toast_worthy(Some("   "), now, now));
        assert!(!toast_worthy(None, now, now));
        // A clock slightly ahead of the message must not hide its code.
        assert!(toast_worthy(Some("482913"), now + 60_000, now));
    }
}
