//! Judge new mail for phishing in the background.
//!
//! The screen runs off the sync path: a local model can take seconds per
//! message, and mail must not wait for it. Verdicts land on the message rows,
//! and the mail list refreshes through the MailUpdated events the scan emits.

use std::sync::Arc;
use std::time::Duration;

use flectar_mail_core::Core;

/// How often an idle screen looks for mail that has not been judged yet.
const PASS_INTERVAL: Duration = Duration::from_secs(45);
/// How many messages one pass judges.
const BATCH: usize = 12;

/// Start the screen for this session. It does nothing until Settings turns the
/// phishing check on.
pub(crate) fn spawn(runtime: &tokio::runtime::Handle, core: Arc<Core>) {
    runtime.spawn(async move {
        loop {
            match core.scan_security(BATCH).await {
                Ok(0) => {}
                Ok(judged) => tracing::debug!(judged, "screened mail for phishing"),
                Err(error) => tracing::warn!(%error, "phishing screen failed"),
            }
            tokio::time::sleep(PASS_INTERVAL).await;
        }
    });
}
