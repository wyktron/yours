//! Background startup snapshot and coalesced core-event ingestion.

use crate::{
    AccountPresentationSettings, AccountRow, AppTheme, AppWindow, InboxState, InvitationSettings,
    MessageColors, PAGE_SIZE, Updates,
    account_controller::apply_connected_account_rows,
    avatar_initials,
    calendar::{
        LocalCalendarAccount, LocalCalendarEvent, LocalCalendarSource, calendar_accounts,
        calendar_range_millis, calendar_sources, core_calendar_event,
    },
    mail::{self, CoreMailSource, MailCursor, MailMessage, MailboxEntry},
    mail_scope::Scope,
    mail_view_model::{refresh_list_metadata, refresh_rows_only},
    startup_metrics::StartupMetrics,
    theme::stored_color,
    ui_dispatch::UiWake,
};
use flectar_mail_core::{
    config::Paths,
    tts,
    events::{CoreEvent, TransferReport},
    models::{
        Account, AccountConfig, CalendarConnection, CardDavConnection, ListOrder, MailGrouping,
        MessageCursor, Settings, ThreadCursor, normalized_workspace_list_pane_width,
    },
};
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::io::AsyncReadExt;

// Version 4 keys account mailboxes by id instead of display name. Version 5
// stores the provider and mail protocol names of the current account model.
// Version 6 keeps message dates as timestamps instead of English text.
// Version 7 records whether the rows are conversations or single messages.
// Version 8 stores mailbox badges as numbers. Version 9 leaves out
// calendar-only accounts.
const WARM_START_FORMAT_VERSION: u32 = 9;
const MAX_WARM_START_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WARM_START_ACCOUNTS: usize = 64;
const MAX_WARM_START_MESSAGES: usize = 25;
const MAX_WARM_START_MAILBOXES: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WarmStartMessage {
    pub(crate) id: i32,
    pub(crate) thread_id: Option<i64>,
    #[serde(default)]
    pub(crate) message_id: Option<i64>,
    #[serde(default = "missing_folder_id")]
    pub(crate) account_id: i64,
    pub(crate) account: String,
    pub(crate) folder: String,
    pub(crate) sender: String,
    pub(crate) address: String,
    pub(crate) domain: String,
    pub(crate) initials: String,
    pub(crate) subject: String,
    pub(crate) preview: String,
    pub(crate) date_ms: i64,
    pub(crate) label: String,
    pub(crate) unread: bool,
    pub(crate) starred: bool,
    pub(crate) has_attachments: bool,
    #[serde(default = "single_message")]
    pub(crate) message_count: i64,
    #[serde(default)]
    pub(crate) has_replied: bool,
    #[serde(default)]
    pub(crate) high_importance: bool,
    /// The AI's phishing judgement, kept so a warm start paints the row and
    /// can explain it in the reader before the mail store answers.
    pub(crate) security: String,
    #[serde(default)]
    pub(crate) security_reason: String,
    #[serde(default)]
    pub(crate) security_model: String,
    #[serde(default)]
    pub(crate) security_checked_at: Option<i64>,
    /// Known breaches of the sender address, so a warm start keeps the skull
    /// before the mail store answers.
    #[serde(default)]
    pub(crate) breaches: Vec<flectar_mail_core::breaches::Breach>,
    #[serde(default)]
    pub(crate) labels: Vec<i64>,
    pub(crate) sender_verification: String,
    #[serde(default)]
    pub(crate) account_uses_labels: bool,
    // Verification codes come from message bodies, which stay out of this
    // file, and file chips act on attachment ids that a stored body can
    // replace; the list shows both once the mail store answers.
}

impl From<&mail::MailMessage> for WarmStartMessage {
    fn from(message: &mail::MailMessage) -> Self {
        Self {
            id: message.id,
            thread_id: message.thread_id,
            message_id: message.message_id,
            account_id: message.account_id,
            account: message.account.clone(),
            folder: message.folder.clone(),
            sender: message.sender.clone(),
            address: message.address.clone(),
            domain: message.domain.clone(),
            initials: message.initials.clone(),
            subject: message.subject.clone(),
            preview: message.preview.clone(),
            date_ms: message.date_ms,
            label: message.label.clone(),
            unread: message.unread,
            starred: message.starred,
            has_attachments: message.has_attachments,
            message_count: message.message_count,
            has_replied: message.has_replied,
            high_importance: message.high_importance,
            security: message.security.clone(),
            breaches: message.breaches.clone(),
            security_reason: message.security_reason.clone(),
            security_model: message.security_model.clone(),
            security_checked_at: message.security_checked_at,
            labels: message.labels.clone(),
            sender_verification: message.sender_verification.clone(),
            account_uses_labels: message.account_uses_labels,
        }
    }
}

impl From<WarmStartMessage> for mail::MailMessage {
    fn from(message: WarmStartMessage) -> Self {
        Self {
            id: message.id,
            thread_id: message.thread_id,
            message_id: message.message_id,
            account_id: message.account_id,
            account: message.account,
            folder: message.folder,
            sender: message.sender,
            address: message.address,
            domain: message.domain,
            initials: message.initials,
            subject: message.subject,
            preview: message.preview,
            date_ms: message.date_ms,
            to: String::new(),
            label: message.label,
            unread: message.unread,
            starred: message.starred,
            has_attachments: message.has_attachments,
            message_count: message.message_count.max(1),
            has_replied: message.has_replied,
            high_importance: message.high_importance,
            security: message.security,
            breaches: message.breaches,
            security_reason: message.security_reason,
            security_model: message.security_model,
            security_checked_at: message.security_checked_at,
            is_outgoing: false,
            labels: message.labels,
            html: None,
            text: None,
            attachments: Vec::new(),
            body_pending: true,
            sender_verification: message.sender_verification,
            // The banner waits for the message itself.
            can_unsubscribe: false,
            can_block: false,
            sender_blocked: false,
            invitation: None,
            receipt: None,
            returned_receipts: Vec::new(),
            verification_code: None,
            account_uses_labels: message.account_uses_labels,
            first_attachment: None,
            attachment_total: 0,
        }
    }
}

const fn single_message() -> i64 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WarmStartMailbox {
    pub(crate) account_id: i64,
    #[serde(default = "missing_folder_id")]
    pub(crate) folder_id: i64,
    #[serde(default = "missing_folder_id")]
    pub(crate) parent_folder_id: i64,
    #[serde(default)]
    pub(crate) depth: usize,
    #[serde(default)]
    pub(crate) has_children: bool,
    #[serde(default)]
    pub(crate) is_standard: bool,
    #[serde(default = "selectable_mailbox")]
    pub(crate) is_selectable: bool,
    #[serde(default)]
    pub(crate) can_create_children: bool,
    #[serde(default)]
    pub(crate) can_rename: bool,
    #[serde(default)]
    pub(crate) can_delete: bool,
    pub(crate) label: String,
    /// Absent for account headings, which are not navigation targets.
    #[serde(default)]
    pub(crate) scope: Option<Scope>,
    pub(crate) context: String,
    pub(crate) detail: String,
    pub(crate) avatar: String,
    pub(crate) is_account: bool,
    pub(crate) count: u32,
}

fn missing_folder_id() -> i64 {
    -1
}

fn selectable_mailbox() -> bool {
    true
}

impl From<&mail::MailboxEntry> for WarmStartMailbox {
    fn from(mailbox: &mail::MailboxEntry) -> Self {
        Self {
            account_id: mailbox.account_id,
            folder_id: mailbox.folder_id,
            parent_folder_id: mailbox.parent_folder_id,
            depth: mailbox.depth,
            has_children: mailbox.has_children,
            is_standard: mailbox.is_standard,
            is_selectable: mailbox.is_selectable,
            can_create_children: mailbox.can_create_children,
            can_rename: mailbox.can_rename,
            can_delete: mailbox.can_delete,
            label: mailbox.label.clone(),
            scope: mailbox.scope,
            context: mailbox.context.clone(),
            detail: mailbox.detail.clone(),
            avatar: mailbox.avatar.clone(),
            is_account: mailbox.is_account,
            count: mailbox.count,
        }
    }
}

impl From<WarmStartMailbox> for mail::MailboxEntry {
    fn from(mailbox: WarmStartMailbox) -> Self {
        Self {
            account_id: mailbox.account_id,
            folder_id: mailbox.folder_id,
            parent_folder_id: mailbox.parent_folder_id,
            depth: mailbox.depth,
            has_children: mailbox.has_children,
            is_standard: mailbox.is_standard,
            is_selectable: mailbox.is_selectable,
            can_create_children: mailbox.can_create_children,
            can_rename: mailbox.can_rename,
            can_delete: mailbox.can_delete,
            label: mailbox.label,
            scope: mailbox.scope,
            context: mailbox.context,
            detail: mailbox.detail,
            avatar: mailbox.avatar,
            is_account: mailbox.is_account,
            count: mailbox.count,
        }
    }
}

/// Where the saved rows' mailbox continues: after a conversation, or after a
/// message in lists without conversations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarmStartCursor {
    Thread(ThreadCursor),
    Message(MessageCursor),
}

impl WarmStartCursor {
    /// Search results are not saved, so neither is a search cursor.
    pub(crate) fn of(cursor: MailCursor) -> Option<Self> {
        match cursor {
            MailCursor::Thread(cursor) => Some(Self::Thread(cursor)),
            MailCursor::Message(cursor) => Some(Self::Message(cursor)),
            MailCursor::Search(_) | MailCursor::MessageSearch(_) => None,
        }
    }

    fn mail_cursor(self) -> MailCursor {
        match self {
            Self::Thread(cursor) => MailCursor::Thread(cursor),
            Self::Message(cursor) => MailCursor::Message(cursor),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WarmStartSnapshot {
    format_version: u32,
    pub(crate) saved_at_ms: i64,
    pub(crate) scope: Scope,
    pub(crate) inbox_count: usize,
    /// Whether the saved rows are conversations or single messages.
    pub(crate) grouping: MailGrouping,
    /// The order of the saved rows, which `next_cursor` continues.
    #[serde(default)]
    pub(crate) order: ListOrder,
    pub(crate) next_cursor: Option<WarmStartCursor>,
    pub(crate) accounts: Vec<Account>,
    /// The active profile's accounts, which the saved rows show.
    #[serde(default)]
    pub(crate) profile_accounts: Option<Vec<i64>>,
    pub(crate) messages: Vec<WarmStartMessage>,
    pub(crate) mailboxes: Vec<WarmStartMailbox>,
    pub(crate) unified_mailboxes: Vec<WarmStartMailbox>,
}

pub(crate) struct WarmStartProjection<'a> {
    pub(crate) scope: Scope,
    pub(crate) inbox_count: usize,
    pub(crate) grouping: MailGrouping,
    pub(crate) order: ListOrder,
    pub(crate) next_cursor: Option<WarmStartCursor>,
    pub(crate) accounts: &'a [Account],
    pub(crate) profile_accounts: Option<&'a [i64]>,
    pub(crate) messages: &'a [mail::MailMessage],
    pub(crate) mailboxes: &'a [mail::MailboxEntry],
    pub(crate) unified_mailboxes: &'a [mail::MailboxEntry],
}

impl WarmStartSnapshot {
    pub(crate) fn capture(projection: WarmStartProjection<'_>) -> Self {
        // Local mail is left out: until the core is open, nothing names it.
        let connected = |account_id: i64| {
            projection
                .accounts
                .iter()
                .any(|account| account.id == account_id)
        };
        Self {
            format_version: WARM_START_FORMAT_VERSION,
            saved_at_ms: chrono::Utc::now().timestamp_millis(),
            scope: projection.scope,
            inbox_count: projection.inbox_count,
            grouping: projection.grouping,
            order: projection.order,
            next_cursor: projection.next_cursor,
            accounts: projection
                .accounts
                .iter()
                .filter(|account| !account.calendar_only)
                .take(MAX_WARM_START_ACCOUNTS)
                .cloned()
                .collect(),
            profile_accounts: projection.profile_accounts.map(<[i64]>::to_vec),
            messages: projection
                .messages
                .iter()
                .filter(|message| connected(message.account_id))
                .take(MAX_WARM_START_MESSAGES)
                .map(WarmStartMessage::from)
                .collect(),
            mailboxes: projection
                .mailboxes
                .iter()
                .filter(|mailbox| connected(mailbox.account_id))
                .take(MAX_WARM_START_MAILBOXES)
                .map(WarmStartMailbox::from)
                .collect(),
            unified_mailboxes: projection
                .unified_mailboxes
                .iter()
                .take(MAX_WARM_START_MAILBOXES)
                .map(WarmStartMailbox::from)
                .collect(),
        }
    }

    fn is_valid(&self) -> bool {
        self.format_version == WARM_START_FORMAT_VERSION
            && !self.accounts.is_empty()
            && self.accounts.len() <= MAX_WARM_START_ACCOUNTS
            && self.messages.len() <= MAX_WARM_START_MESSAGES
            && self.mailboxes.len() <= MAX_WARM_START_MAILBOXES
            && self.unified_mailboxes.len() <= MAX_WARM_START_MAILBOXES
    }
}

pub(crate) fn apply_warm_start_snapshot(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Handle,
    snapshot: WarmStartSnapshot,
) {
    // The cache contains account summaries, not connection settings. Project
    // these directly: the full settings projection requires AccountConfig and
    // would filter every cached account out before the database has opened.
    let now = chrono::Local::now();
    let account_rows = snapshot
        .accounts
        .iter()
        .filter(|account| !account.calendar_only)
        .filter_map(|account| {
            let name = account
                .display_name
                .as_ref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&account.email);
            let (checked_day, checked_at) =
                crate::account_status::last_checked(account.caught_up_at, &now);
            Some(AccountRow {
                id: i32::try_from(account.id).ok()?,
                drag_key: account.id.to_string().into(),
                name: name.as_str().into(),
                email: account.email.as_str().into(),
                provider: account.provider.as_str().into(),
                mail_protocol: account.mail_protocol.as_str().into(),
                status: account.sync_state.as_str().into(),
                sync_error: account.sync_error.as_deref().unwrap_or_default().into(),
                checked_day,
                checked_at: checked_at.into(),
                initials: avatar_initials(name).into(),
                ..Default::default()
            })
        })
        .collect();
    {
        let mut state = state.borrow_mut();
        // Core remains unset: cached rows are only a presentation until local
        // storage is ready, and the startup guard keeps actions inert.
        state.using_core = true;
        // The saved rows show the profile's accounts; so do the sidebar and
        // scope until settings load.
        state.profile_accounts = snapshot.profile_accounts;
        let _ = state.set_connected_accounts(snapshot.accounts);
        state.set_scope(snapshot.scope);
        state.inbox_count = snapshot.inbox_count;
        state.grouping = snapshot.grouping;
        state.list_order = snapshot.order;
        state.next_cursor = snapshot.next_cursor.map(WarmStartCursor::mail_cursor);
        state.messages = snapshot
            .messages
            .into_iter()
            .map(MailMessage::from)
            .collect();
        state.mailboxes = snapshot
            .mailboxes
            .into_iter()
            .map(MailboxEntry::from)
            .collect();
        state.unified_mailboxes = snapshot
            .unified_mailboxes
            .into_iter()
            .map(MailboxEntry::from)
            .collect();
        state.preview_closed = true;
    }
    // The mailbox is a conditional element keyed on Slint's account model.
    // Publishing only the cached rows leaves that entire view uninstantiated.
    apply_connected_account_rows(app, account_rows);
    refresh_rows_only(app, state, runtime);
    refresh_list_metadata(app, state);
    app.set_startup_hydrated(true);
}

#[derive(Clone)]
enum WarmStartCacheCommand {
    Save(Arc<WarmStartSnapshot>),
    Clear,
}

#[derive(Clone)]
pub(crate) struct WarmStartCacheWriter {
    tx: tokio::sync::watch::Sender<Option<WarmStartCacheCommand>>,
}

impl WarmStartCacheWriter {
    pub(crate) fn spawn(runtime: &tokio::runtime::Handle, path: PathBuf) -> Self {
        // A cache file is a projection, not a journal. `watch` retains exactly
        // the newest command while a slow disk write is in flight, placing a
        // hard one-snapshot bound on queued memory.
        let (tx, mut rx) = tokio::sync::watch::channel(None);
        runtime.spawn(async move {
            while rx.changed().await.is_ok() {
                let Some(command) = rx.borrow_and_update().clone() else {
                    continue;
                };
                let result = match command {
                    WarmStartCacheCommand::Save(snapshot) => {
                        write_warm_start_snapshot(&path, &snapshot).await
                    }
                    WarmStartCacheCommand::Clear => clear_warm_start_snapshot(&path).await,
                };
                if let Err(error) = result {
                    tracing::warn!(error = %error, "warm-start mailbox cache update failed");
                }
            }
        });
        Self { tx }
    }

    pub(crate) fn save(&self, snapshot: WarmStartSnapshot) {
        self.tx
            .send_replace(Some(WarmStartCacheCommand::Save(Arc::new(snapshot))));
    }

    pub(crate) fn clear(&self) {
        self.tx.send_replace(Some(WarmStartCacheCommand::Clear));
    }
}

pub(crate) async fn load_warm_start_snapshot(path: &Path) -> Option<WarmStartSnapshot> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::with_capacity(MAX_WARM_START_BYTES as usize);
    if file
        .take(MAX_WARM_START_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() as u64 > MAX_WARM_START_BYTES
    {
        let _ = clear_warm_start_snapshot(path).await;
        return None;
    }
    let snapshot = serde_json::from_slice::<WarmStartSnapshot>(&bytes).ok();
    match snapshot.filter(WarmStartSnapshot::is_valid) {
        Some(snapshot) => Some(snapshot),
        None => {
            let _ = clear_warm_start_snapshot(path).await;
            None
        }
    }
}

async fn write_warm_start_snapshot(
    path: &Path,
    snapshot: &WarmStartSnapshot,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_WARM_START_BYTES {
        return Err("warm-start mailbox cache exceeded its size limit".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "warm-start mailbox cache has no parent directory".to_owned())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| error.to_string())?;
    }
    #[cfg(windows)]
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        tokio::fs::remove_file(path)
            .await
            .map_err(|error| error.to_string())?;
    }
    tokio::fs::rename(&temporary, path)
        .await
        .map_err(|error| error.to_string())
}

async fn clear_warm_start_snapshot(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) struct StartupSnapshot {
    pub(crate) core: CoreMailSource,
    pub(crate) scope: Scope,
    pub(crate) page: Option<mail::MailPage>,
    pub(crate) remote_images_enabled: bool,
    pub(crate) trusted_image_senders: Vec<String>,
    pub(crate) accounts: Vec<Account>,
    pub(crate) account_configs: Vec<AccountConfig>,
    pub(crate) local_mail: Option<flectar_mail_core::models::LocalMail>,
    pub(crate) settings: Option<Settings>,
    pub(crate) app_lock: Option<String>,
}

pub(crate) struct StartupCalendarSnapshot {
    pub(crate) calendar_connections: Vec<CalendarConnection>,
    pub(crate) carddav_connections: Vec<CardDavConnection>,
    pub(crate) calendar_events: Vec<LocalCalendarEvent>,
    pub(crate) calendar_accounts: Vec<LocalCalendarAccount>,
    pub(crate) calendar_sources: Vec<LocalCalendarSource>,
}

pub(crate) enum StartupUpdate {
    Warm {
        snapshot: Box<WarmStartSnapshot>,
        applied: tokio::sync::oneshot::Sender<()>,
    },
    Ready(Result<Box<StartupSnapshot>, String>),
    MailMetadata(Result<mail::MailMetadata, String>),
    Calendar {
        generation: u64,
        month: chrono::NaiveDate,
        snapshot: StartupCalendarSnapshot,
    },
    Connections {
        calendar: Vec<CalendarConnection>,
        contacts: Vec<CardDavConnection>,
    },
}

const MAX_PENDING_THREAD_UPDATES: usize = 2_048;

/// Cross-thread core events are collapsed to the latest meaningful state.
/// A massive backfill can therefore never grow an unbounded UI queue or make
/// one Slint callback drain thousands of historical notifications.
#[derive(Default)]
pub(crate) struct PendingCoreUpdates {
    pub(crate) changed_threads: HashSet<i64>,
    pub(crate) all_mail_changed: bool,
    pub(crate) account_states: HashMap<i64, (String, Option<String>)>,
    /// When each account's latest check caught its Inbox up.
    pub(crate) caught_up: HashMap<i64, i64>,
    pub(crate) action_error: Option<String>,
    /// What became of the latest mail copied or moved to other accounts.
    pub(crate) transfers: Option<TransferReport>,
    /// A sync counted older unread mail only on the server again.
    pub(crate) server_only_changed: bool,
    pub(crate) calendar_changed: bool,
    pub(crate) contacts_changed: bool,
}

impl PendingCoreUpdates {
    fn record_threads(&mut self, thread_ids: Vec<i64>) {
        if self.all_mail_changed {
            return;
        }
        if thread_ids.is_empty()
            || self.changed_threads.len().saturating_add(thread_ids.len())
                > MAX_PENDING_THREAD_UPDATES
        {
            self.changed_threads.clear();
            self.all_mail_changed = true;
            return;
        }
        self.changed_threads.extend(thread_ids);
    }
}

/// The text of the AI summary the model is still writing. The core streams the
/// structured summary as raw JSON chunks; the reader shows the parts it can
/// already use, refreshed as each chunk lands instead of once at the end.
#[derive(Default)]
pub(crate) struct SummaryStream {
    /// The conversation named by the request the panel is showing.
    thread_id: i64,
    /// Which request owns the buffer; a finished request must not paint over
    /// the answer of one the user started after it.
    generation: u64,
    /// The raw JSON written so far, in arrival order.
    raw: String,
    /// A chunk arrived that the panel has not read yet.
    fresh: bool,
}

impl SummaryStream {
    /// Take the buffer for a new request, dropping whatever an earlier one left
    /// behind, and return the generation that request owns.
    pub(crate) fn begin(&mut self, thread_id: i64) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.thread_id = thread_id;
        self.raw.clear();
        self.fresh = false;
        self.generation
    }

    /// Fold one model chunk in. A chunk for another conversation belongs to a
    /// request the user replaced, and mixing it in would show the wrong mail.
    pub(crate) fn append(&mut self, thread_id: i64, delta: &str) -> bool {
        if thread_id != self.thread_id {
            return false;
        }
        self.raw.push_str(delta);
        self.fresh = true;
        true
    }

    /// Whether `generation` is still the request the panel waits for.
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Read the chunks received since the last call, if any.
    pub(crate) fn take(&mut self) -> Option<String> {
        if !self.fresh {
            return None;
        }
        self.fresh = false;
        Some(summary_display_text(&self.raw))
    }

    /// Forget unread chunks, so text from a finished request cannot repaint a
    /// panel that has already shown its answer.
    pub(crate) fn discard(&mut self) {
        self.fresh = false;
    }
}

/// The readable sections the panel shows, recovered from the JSON written so
/// far. The model emits the object in field order, so the text fills in from
/// the top; a string still being written is shown up to its last whole escape.
pub(crate) fn summary_display_text(raw: &str) -> String {
    let sections = summary_sections(raw);
    let points = sections
        .key_points
        .iter()
        .map(|point| point.trim())
        .filter(|point| !point.is_empty())
        .collect::<Vec<_>>();
    let mut text = String::new();
    if !points.is_empty() {
        text.push_str("Key points\n");
        for point in points {
            text.push_str("• ");
            text.push_str(point);
            text.push('\n');
        }
        text.push('\n');
    }
    if let Some(next) = sections
        .next_action
        .as_deref()
        .map(str::trim)
        .filter(|next| !next.is_empty())
    {
        text.push_str("Next action\n");
        text.push_str(next);
        text.push_str("\n\n");
    }
    if let Some(reply) = sections
        .proposed_reply
        .as_deref()
        .map(str::trim)
        .filter(|reply| !reply.is_empty())
    {
        text.push_str("Suggested reply\n");
        text.push_str(reply);
        text.push('\n');
    }
    text
}

/// The parts of a streamed summary the panel renders.
#[derive(Default)]
struct SummarySections {
    key_points: Vec<String>,
    next_action: Option<String>,
    proposed_reply: Option<String>,
}

fn summary_sections(raw: &str) -> SummarySections {
    let mut sections = SummarySections::default();
    let bytes = raw.as_bytes();
    let Some(brace) = raw.find('{') else {
        return sections;
    };
    let mut index = brace + 1;
    loop {
        skip_json_separators(bytes, &mut index);
        if bytes.get(index) != Some(&b'"') {
            break;
        }
        let Some((key, Some(key_end))) = json_string_prefix(raw, index) else {
            break;
        };
        index = key_end;
        skip_json_whitespace(bytes, &mut index);
        if bytes.get(index) != Some(&b':') {
            break;
        }
        index += 1;
        skip_json_whitespace(bytes, &mut index);
        match key.as_str() {
            "keyPoints" => sections.key_points = json_string_array(raw, index),
            "nextAction" => sections.next_action = json_string_value(raw, index),
            "proposedReply" => sections.proposed_reply = json_string_value(raw, index),
            _ => {}
        }
        match json_value_end(raw, index) {
            Some(end) => index = end,
            None => break,
        }
    }
    sections
}

fn skip_json_whitespace(bytes: &[u8], index: &mut usize) {
    while bytes.get(*index).is_some_and(u8::is_ascii_whitespace) {
        *index += 1;
    }
}

fn skip_json_separators(bytes: &[u8], index: &mut usize) {
    while bytes
        .get(*index)
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b',')
    {
        *index += 1;
    }
}

/// A JSON string starting at `start`: its decoded text and the index past the
/// closing quote. A string cut off by the end of the input yields the text
/// written so far and `None` for the index.
fn json_string_prefix(raw: &str, start: usize) -> Option<(String, Option<usize>)> {
    let bytes = raw.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut text = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((text, Some(index + 1))),
            b'\\' => {
                index += 1;
                match bytes.get(index).copied() {
                    None => return Some((text, None)),
                    Some(b'"') => {
                        text.push('"');
                        index += 1;
                    }
                    Some(b'\\') => {
                        text.push('\\');
                        index += 1;
                    }
                    Some(b'/') => {
                        text.push('/');
                        index += 1;
                    }
                    Some(b'b') => {
                        text.push('\u{8}');
                        index += 1;
                    }
                    Some(b'f') => {
                        text.push('\u{c}');
                        index += 1;
                    }
                    Some(b'n') => {
                        text.push('\n');
                        index += 1;
                    }
                    Some(b'r') => {
                        text.push('\r');
                        index += 1;
                    }
                    Some(b't') => {
                        text.push('\t');
                        index += 1;
                    }
                    Some(b'u') => {
                        index += 1;
                        let Some(hex) = raw.get(index..index + 4) else {
                            return Some((text, None));
                        };
                        let Ok(code) = u32::from_str_radix(hex, 16) else {
                            return Some((text, None));
                        };
                        index += 4;
                        if let Some(character) = char::from_u32(code) {
                            text.push(character);
                        } else if (0xD800..0xDC00).contains(&code) {
                            // A surrogate pair spells one character, with the
                            // low half in the next escape when it is there.
                            let low = raw
                                .get(index..index + 2)
                                .filter(|escaped| *escaped == "\\u")
                                .and_then(|_| raw.get(index + 2..index + 6))
                                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                                .filter(|low| (0xDC00..0xE000).contains(low));
                            if let Some(low) = low
                                && let Some(character) = char::from_u32(
                                    0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00),
                                )
                            {
                                text.push(character);
                                index += 6;
                            }
                        }
                    }
                    // An unknown escape would be malformed JSON. Keep the
                    // character after it whole, so a multibyte one can never
                    // leave the scan inside a UTF-8 sequence.
                    Some(_) => {
                        let Some(character) = raw[index..].chars().next() else {
                            return Some((text, None));
                        };
                        text.push(character);
                        index += character.len_utf8();
                    }
                }
            }
            _ => {
                let Some(character) = raw[index..].chars().next() else {
                    return Some((text, None));
                };
                text.push(character);
                index += character.len_utf8();
            }
        }
    }
    Some((text, None))
}

/// A string value at `start`; `None` when the value is missing or is not a string.
fn json_string_value(raw: &str, start: usize) -> Option<String> {
    json_string_prefix(raw, start).map(|(text, _)| text)
}

/// The string elements of the array at `start`, including the one still being
/// written, so a key point appears as soon as its text has arrived.
fn json_string_array(raw: &str, start: usize) -> Vec<String> {
    let bytes = raw.as_bytes();
    let mut values = Vec::new();
    if bytes.get(start) != Some(&b'[') {
        return values;
    }
    let mut index = start + 1;
    loop {
        skip_json_separators(bytes, &mut index);
        match bytes.get(index).copied() {
            None | Some(b']') => break,
            Some(b'"') => {}
            Some(_) => break,
        }
        let Some((text, end)) = json_string_prefix(raw, index) else {
            break;
        };
        values.push(text);
        match end {
            Some(end) => index = end,
            None => break,
        }
    }
    values
}

/// The index just past the value at `start`, used to walk the object's fields.
fn json_value_end(raw: &str, start: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    match bytes.get(start).copied()? {
        b'"' => json_string_prefix(raw, start)?.1,
        b'[' | b'{' => {
            let mut depth = 0_usize;
            let mut index = start;
            while index < bytes.len() {
                match bytes[index] {
                    b'"' => {
                        index = json_string_prefix(raw, index)?.1?;
                    }
                    b'[' | b'{' => {
                        depth += 1;
                        index += 1;
                    }
                    b']' | b'}' => {
                        depth = depth.saturating_sub(1);
                        index += 1;
                        if depth == 0 {
                            return Some(index);
                        }
                    }
                    _ => index += 1,
                }
            }
            None
        }
        _ => {
            let mut index = start;
            while index < bytes.len() && !matches!(bytes[index], b',' | b'}' | b']') {
                index += 1;
            }
            (index > start).then_some(index)
        }
    }
}

pub(crate) async fn load_startup_snapshot(
    paths: Paths,
    credentials: flectar_mail_core::accounts::credentials::CredentialStoreHandle,
    oauth_redirects: flectar_mail_core::oauth::redirect::OAuthRedirectBrokerHandle,
    preferred_scope: Scope,
    metrics: &StartupMetrics,
) -> Result<StartupSnapshot, String> {
    let core = CoreMailSource::start(paths, credentials, oauth_redirects).await?;
    metrics.emit("core_opened", serde_json::Value::Null);
    crate::startup_metrics::phase("core_opened");
    // The first page shows only the active profile's accounts.
    let settings = core.load_settings().await.ok();
    crate::startup_metrics::phase("settings_loaded");
    let app_lock = core.load_app_lock().await.ok().flatten();
    crate::startup_metrics::phase("app_lock_loaded");
    // The first page is listed the way the saved preference groups mail.
    let _ = core.set_grouping(
        settings
            .as_ref()
            .map_or(MailGrouping::Conversations, Settings::mail_grouping),
    );
    let _ = core.set_profile_accounts(settings.as_ref().and_then(|settings| {
        AccountPresentationSettings::from_settings(settings).active_profile_accounts()
    }));
    let (mail, configs, local_mail, trusted_image_senders) = tokio::join!(
        // Counts walk every mailbox and can dominate startup on a large local
        // store. The first useful frame only needs one bounded page; sidebar
        // totals arrive as a separate progressive update.
        core.load_startup_page(preferred_scope, PAGE_SIZE as i64),
        core.load_account_configs(),
        core.load_local_mail(),
        core.trusted_image_senders(),
    );

    crate::startup_metrics::phase("startup_queries_loaded");
    let (accounts, scope, page) = mail.unwrap_or_else(|_| (Vec::new(), Scope::UNIFIED_INBOX, None));
    metrics.emit(
        "startup_queries_loaded",
        serde_json::json!({
            "accounts": accounts.len(),
            "has_page": page.is_some(),
        }),
    );

    Ok(StartupSnapshot {
        core,
        scope,
        page,
        remote_images_enabled: settings
            .as_ref()
            .is_some_and(|settings| settings.load_remote_images),
        trusted_image_senders: trusted_image_senders.unwrap_or_default(),
        accounts,
        account_configs: configs.unwrap_or_default(),
        local_mail: local_mail.ok().flatten(),
        settings,
        app_lock,
    })
}

pub(crate) async fn load_startup_mail_metadata(
    core: &CoreMailSource,
    scope: Scope,
) -> Result<mail::MailMetadata, String> {
    core.load_mail_metadata(scope).await
}

pub(crate) async fn load_startup_calendar_snapshot(
    core: &CoreMailSource,
    accounts: &[Account],
    visible_month: chrono::NaiveDate,
) -> StartupCalendarSnapshot {
    let calendar_range = calendar_range_millis(visible_month);
    let (connections, carddav_connections, events, calendars) = tokio::join!(
        core.load_calendar_connections(),
        core.load_carddav_connections(),
        async {
            match calendar_range {
                Ok((start_ms, end_ms)) => core.load_events(start_ms, end_ms).await,
                Err(error) => Err(error),
            }
        },
        core.load_calendars(None),
    );

    StartupCalendarSnapshot {
        calendar_connections: connections.unwrap_or_default(),
        carddav_connections: carddav_connections.unwrap_or_default(),
        calendar_events: events
            .unwrap_or_default()
            .into_iter()
            .map(core_calendar_event)
            .collect(),
        calendar_accounts: calendar_accounts(accounts),
        calendar_sources: calendar_sources(calendars.unwrap_or_default()),
    }
}

/// How long the core must stay quiet after a burst of work before its SQLite
/// caches and the allocator's free pages go back to the operating system.
const IDLE_RELEASE_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

pub(crate) fn spawn_core_event_listener(
    runtime: &tokio::runtime::Handle,
    core: CoreMailSource,
    pending: Arc<Mutex<PendingCoreUpdates>>,
    wake: UiWake,
    summary: Arc<Mutex<SummaryStream>>,
    summary_wake: UiWake,
    code_toasts: crate::verification_toast::CodeToasts,
) {
    let mut events = core.subscribe_events();
    // New mail is spoken from the core's own arrival event, not from the
    // notification host: the voice works whether or not the system shows
    // toasts, the window is on the inbox, or the user's notification scope
    // keeps the mail out of a notification.
    let announcements = crate::notifications::spawn_mail_announcer(runtime, core.file_core());
    // Weak, so the loop still ends when the core and its event bus are gone.
    let stores = Arc::downgrade(&core.file_core());
    runtime.spawn(async move {
        use tokio::sync::broadcast::error::RecvError;

        // Core events come from database work: sync steps, new and changed
        // mail, actions, calendar and contact pulls. A sync leaves the page
        // caches full and the freed pages with the allocator, so hand both
        // back once no event has arrived for a while.
        let mut release_at = None;
        loop {
            let event = tokio::select! {
                event = events.recv() => event,
                () = until(release_at) => {
                    release_at = None;
                    // On its own task: the release waits behind any job
                    // still queued on a database thread.
                    let stores = stores.clone();
                    tokio::spawn(async move {
                        if let Some(core) = stores.upgrade() {
                            let _ = core.release_cache_memory().await;
                        }
                        crate::memory::release_free_heap();
                    });
                    continue;
                }
            };
            release_at = Some(tokio::time::Instant::now() + IDLE_RELEASE_DELAY);
            let should_wake = match event {
                Ok(CoreEvent::MailUpdated { thread_ids }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .record_threads(thread_ids);
                    true
                }
                Ok(CoreEvent::MailNew { thread_ids, .. }) => {
                    // The mail list wakes from MailUpdated; only the voice and
                    // the copy-code toast need this event, so it does not wake
                    // the UI by itself.
                    announcements.mail_arrived(thread_ids.clone());
                    code_toasts.arrived(thread_ids.clone());
                    // Sender breach checks call HIBP, which must never hold
                    // mail up: a separate task owns the lookups and the cache
                    // write, and the MailUpdated it emits refreshes the rows
                    // that get a skull.
                    if let Some(core) = stores.upgrade() {
                        let thread_ids = thread_ids.clone();
                        tokio::spawn(async move {
                            match core.check_sender_breaches(thread_ids).await {
                                Ok(0) => {}
                                Ok(checked) => tracing::info!(
                                    checked,
                                    "sender breach check: looked up senders"
                                ),
                                Err(error) => tracing::warn!(
                                    %error,
                                    "sender breach check failed"
                                ),
                            }
                        });
                    }
                    false
                }
                Ok(CoreEvent::AccountState {
                    account_id,
                    sync_state,
                    sync_error,
                }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .account_states
                        .insert(account_id, (sync_state, sync_error));
                    true
                }
                Ok(CoreEvent::AccountCaughtUp { account_id, at }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .caught_up
                        .insert(account_id, at);
                    true
                }
                Ok(CoreEvent::ActionState {
                    state,
                    error: Some(error),
                    ..
                }) if state == "failed" => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .action_error = Some(error);
                    true
                }
                Ok(CoreEvent::TransfersFinished(report)) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .transfers = Some(report);
                    true
                }
                Ok(CoreEvent::CalendarUpdated { .. })
                | Ok(CoreEvent::CalendarEventsAdded { .. }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .calendar_changed = true;
                    true
                }
                Ok(CoreEvent::ContactsUpdated { .. }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .contacts_changed = true;
                    true
                }
                Ok(CoreEvent::ServerOnlyUnreadChanged { .. }) => {
                    pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .server_only_changed = true;
                    true
                }
                Err(RecvError::Lagged(_)) => {
                    let mut pending = pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    pending.changed_threads.clear();
                    pending.all_mail_changed = true;
                    pending.calendar_changed = true;
                    pending.contacts_changed = true;
                    true
                }
                Ok(CoreEvent::AiSummaryDelta { thread_id, delta }) => {
                    // The summary panel reads the text this buffer builds on
                    // the UI thread. Wake it only for the conversation the
                    // user asked about, so an answer to a replaced request
                    // cannot bring its panel back.
                    let fresh = summary
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .append(thread_id, &delta);
                    if fresh {
                        summary_wake.wake();
                    }
                    false
                }
                Ok(_) => false,
                Err(RecvError::Closed) => break,
            };
            if should_wake {
                wake.wake();
            }
        }
    });
}

pub(crate) fn refresh_oauth_availability(app: &AppWindow) {
    use flectar_mail_core::{models::Provider, oauth::providers::resolve_credentials};
    app.set_google_oauth_available(resolve_credentials(Provider::Gmail).is_ok());
    app.set_microsoft_oauth_available(resolve_credentials(Provider::Microsoft).is_ok());
}

/// Shows the read-aloud engine, its voices, and what it needs to reach a
/// service, and hands the speech layer the engine it should speak with.
pub(crate) fn apply_tts(app: &AppWindow, settings: &Settings, config: &tts::TtsConfig) {
    let engines = tts::TtsEngine::all();
    let selected = engines
        .iter()
        .position(|engine| *engine == config.engine)
        .unwrap_or(0);
    app.set_tts_engine(config.engine.id().into());
    app.set_tts_engine_index(selected as i32);
    app.set_tts_needs_key(config.engine.needs_key());
    app.set_tts_announce_mail(settings.tts_announce_mail);
    app.set_tts_endpoint(settings.tts_endpoint.clone().into());
    app.set_tts_model(settings.tts_model.clone().into());
    publish_tts_voices(app, config, tts::voices(config));
    crate::speech::set_config(config.clone());
}

/// Shows a voice list and selects the configured voice in it.
pub(crate) fn publish_tts_voices(
    app: &AppWindow,
    config: &tts::TtsConfig,
    voices: Vec<tts::TtsVoice>,
) {
    let labels: Vec<slint::SharedString> = voices
        .iter()
        .map(|voice| slint::SharedString::from(voice.label.as_str()))
        .collect();
    let ids: Vec<slint::SharedString> = voices
        .iter()
        .map(|voice| slint::SharedString::from(voice.id.as_str()))
        .collect();
    let selected = config.resolved_voice();
    let index = voices
        .iter()
        .position(|voice| voice.id == selected)
        .unwrap_or(0);
    crate::speech::set_voice_labels(
        voices
            .iter()
            .map(|voice| (voice.label.clone(), voice.id.clone())),
    );
    app.set_tts_voice_options(slint::ModelRc::new(slint::VecModel::from(labels)));
    app.set_tts_voice_ids(slint::ModelRc::new(slint::VecModel::from(ids)));
    app.set_tts_voice_index(index as i32);
    app.set_tts_voice(selected.into());
}

pub(crate) fn apply_settings(app: &AppWindow, settings: &Settings) {
    app.global::<MessageColors>()
        .set_preference(settings.message_appearance.as_str().into());
    // Without this the UI keeps the default System preference, so a desktop
    // that asks for reduced motion silences every animation even when the
    // user chose Off. The Space field's timer reads the same setting.
    app.global::<crate::MotionSettings>()
        .set_preference(settings.reduce_motion.as_str().into());
    crate::mail_rules::apply_cleanup(
        app,
        settings.empty_trash_after_days,
        settings.empty_junk_after_days,
    );
    app.global::<InvitationSettings>()
        .set_add(settings.add_invitations.as_str().into());
    refresh_oauth_availability(app);
    app.set_time_format(settings.time_format.as_str().into());
    let tts_config = crate::speech::config_from_settings(settings);
    apply_tts(app, settings, &tts_config);
    app.set_ai_phishing(settings.ai_phishing_enabled);
    app.set_breach_scan_enabled(settings.breach_scan_enabled);
    // The AI model pickers show the configured tier models until the probe
    // lands with the endpoint's own list.
    crate::services::publish_ai_tier_models(app, settings);
    app.set_ai_endpoint(settings.ai_base_url.clone().into());
    crate::apply_language(app, &settings.language);
    app.set_custom_oauth_configured(
        !settings.google_client_id.trim().is_empty() || !settings.ms_client_id.trim().is_empty(),
    );
    app.set_google_client_id(settings.google_client_id.clone().into());
    app.set_google_client_secret(settings.google_client_secret.clone().into());
    app.set_ms_client_id(settings.ms_client_id.clone().into());
    app.set_download_directory(
        crate::documents::download_directory_label(&settings.download_directory).into(),
    );
    app.set_download_directory_available(cfg!(desktop));
    app.set_notifications_enabled(settings.notifications_enabled);
    app.set_notification_sound_enabled(settings.sound_enabled);
    app.set_quiet_hours(settings.quiet_hours);
    app.set_quiet_start(settings.quiet_start.clone().into());
    app.set_quiet_end(settings.quiet_end.clone().into());
    // Tuta's own notifications: the saved choice, or detection for a machine
    // that never chose. The bridge itself reads the setting for each call.
    app.set_tuta_notifications(crate::notify_bridge::effective_enabled(settings));
    app.set_tuta_extra_apps(settings.tuta_extra_apps.clone().into());
    app.set_unread_badge_enabled(settings.unread_badge_enabled);
    app.global::<Updates>()
        .set_enabled(settings.check_for_updates);
    crate::updates::set_enabled(settings.check_for_updates);
    app.set_notification_scope(settings.notification_scope.clone().into());
    app.set_sync_interval_minutes(match settings.sync_interval_minutes {
        1 | 5 | 15 => settings.sync_interval_minutes as i32,
        _ => 5,
    });
    app.set_undo_send_seconds(i32::try_from(settings.undo_send_seconds.clamp(0, 30)).unwrap_or(10));
    app.set_compose_font(settings.compose_font.as_str().into());
    app.set_compose_text_size(settings.compose_text_size.as_str().into());
    app.set_mark_read(settings.mark_read.as_str().into());
    app.set_collect_outgoing_contacts(settings.collect_outgoing_contacts);
    app.set_collect_incoming_contacts(settings.collect_incoming_contacts);
    app.set_contact_suggest_all_accounts(settings.contact_suggest_all_accounts);
    crate::spelling::apply_settings(
        settings.spelling_enabled,
        settings.spelling_languages.clone(),
    );
    crate::spelling::apply_to_window(app);
    app.set_reacher_enabled(settings.reacher_enabled);
    app.set_reacher_url(settings.reacher_url.clone().into());
    crate::composer::set_address_checks_enabled(settings.reacher_enabled);
    app.set_suggest_learned_contacts(settings.suggest_learned_contacts);
    app.set_close_to_tray(settings.close_to_tray && !cfg!(mobile));
    crate::window_chrome::apply_preference(
        app,
        crate::window_chrome::WindowFrame::parse(&settings.window_frame),
    );
    app.set_monochrome_sidebar_icons(settings.monochrome_sidebar_icons);
    crate::app_icon::apply(app, settings.app_icon);
    app.set_show_avatars(settings.show_avatars);
    crate::apply_list_rows(app, settings);
    app.set_group_mail_by_date(settings.group_mail_by_date);
    app.set_show_tabs(settings.show_tabs);
    app.set_conversation_view(settings.conversation_view);
    app.set_show_account_markers(settings.show_account_badges);
    app.set_workspace_layout(
        match settings.workspace_layout.as_str() {
            "minimal" => "minimal",
            _ => "default",
        }
        .into(),
    );
    app.set_workspace_list_pane_width(normalized_workspace_list_pane_width(
        settings.workspace_list_pane_width,
    ) as f32);
    app.set_theme_mode(
        match settings.theme.as_str() {
            "carbon" | "dark" => "dark",
            "snow" | "light" => "light",
            _ => "system",
        }
        .into(),
    );

    let theme = app.global::<AppTheme>();
    theme.set_preset(
        match settings.theme_preset.as_str() {
            "teal" | "green" | "purple" | "oled" | "pixel" | "space" | "custom" => {
                settings.theme_preset.as_str()
            }
            _ => "default",
        }
        .into(),
    );
    let custom = &settings.custom_theme;
    theme.set_custom_light_primary(stored_color(&custom.light_primary, "#0969DA"));
    theme.set_custom_light_page_bg(stored_color(&custom.light_page_background, "#F2F2F0"));
    theme.set_custom_light_surface(stored_color(&custom.light_surface, "#FFFFFF"));
    theme.set_custom_light_text(stored_color(&custom.light_text, "#202120"));
    theme.set_custom_light_border(stored_color(&custom.light_border, "#D9D9D6"));
    theme.set_custom_dark_primary(stored_color(&custom.dark_primary, "#0969DA"));
    theme.set_custom_dark_page_bg(stored_color(&custom.dark_page_background, "#111213"));
    theme.set_custom_dark_surface(stored_color(&custom.dark_surface, "#18191A"));
    theme.set_custom_dark_text(stored_color(&custom.dark_text, "#F3F3F2"));
    theme.set_custom_dark_border(stored_color(&custom.dark_border, "#3A3B3C"));
}

#[cfg(test)]
mod warm_start_tests {
    use super::*;
    use flectar_mail_core::models::{AuthKind, MailProtocol, Provider};
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use slint::{Model, Rgb8Pixel};
    use std::{cell::Cell, time::Duration};

    struct Headless(Rc<MinimalSoftwareWindow>, Rc<Cell<Duration>>);

    impl Platform for Headless {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }

        fn duration_since_start(&self) -> Duration {
            self.1.get()
        }
    }

    const PERSON_INBOX: Scope = Scope::Account {
        account_id: 7,
        mailbox: crate::StandardMailbox::Inbox,
    };

    /// A headless window with a controlled clock and an empty inbox state.
    struct Harness {
        window: Rc<MinimalSoftwareWindow>,
        clock: Rc<Cell<Duration>>,
        app: AppWindow,
        runtime: tokio::runtime::Runtime,
        state: Rc<RefCell<InboxState>>,
        _cache: tempfile::TempDir,
    }

    impl Harness {
        fn new() -> Self {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            let clock = Rc::new(Cell::new(Duration::ZERO));
            slint::platform::set_platform(Box::new(Headless(window.clone(), clock.clone())))
                .unwrap();
            let app = AppWindow::new().unwrap();
            app.window().set_size(slint::PhysicalSize::new(1320, 800));
            app.show().unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let cache = tempfile::tempdir().unwrap();
            let (favicons, _) = crate::bounded_ui_channel();
            let (avatars, _) = crate::bounded_ui_channel();
            let state = InboxState::empty(
                None,
                crate::UiSender::new(favicons, UiWake::new(app.as_weak(), |_| {})),
                None,
                crate::UiSender::new(avatars, UiWake::new(app.as_weak(), |_| {})),
                false,
                false,
                WarmStartCacheWriter::spawn(runtime.handle(), cache.path().join("warm.json")),
            );
            app.set_emails(state.email_rows.clone().into());
            app.set_mail_list_entries(state.mail_list_entries.clone().into());
            app.set_sidebar_rows(state.sidebar_rows.clone().into());
            Self {
                window,
                clock,
                app,
                runtime,
                state: Rc::new(RefCell::new(state)),
                _cache: cache,
            }
        }

        fn apply_single_account_snapshot(&self) {
            apply_warm_start_snapshot(
                &self.app,
                &self.state,
                self.runtime.handle(),
                single_account_snapshot(),
            );
        }

        /// Paint a settled frame, saved as `<name>.png` when
        /// `FLECTAR_STARTUP_REVIEW_DIR` is set.
        fn render(&self, name: &str) -> Vec<Rgb8Pixel> {
            let size = self.app.window().size();
            let mut pixels = vec![Rgb8Pixel::default(); (size.width * size.height) as usize];
            // Evaluate bindings before advancing the clock: the compact drawer
            // starts its animation on the first frame. Clicks and snapshots
            // must see the settled layout even when PNG output is disabled.
            for elapsed in [Duration::ZERO, Duration::from_secs(1)] {
                self.clock.set(self.clock.get() + elapsed);
                slint::platform::update_timers_and_animations();
                self.app.window().request_redraw();
                self.window.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, size.width as usize);
                });
            }
            if let Some(directory) = std::env::var_os("FLECTAR_STARTUP_REVIEW_DIR") {
                let directory = PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let bytes: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
                image::save_buffer(
                    directory.join(format!("{name}.png")),
                    &bytes,
                    size.width,
                    size.height,
                    image::ColorType::Rgb8,
                )
                .unwrap();
            }
            pixels
        }

        fn click(&self, x: f32, y: f32) {
            let position = slint::LogicalPosition::new(x, y);
            let button = slint::platform::PointerEventButton::Left;
            self.app
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
            self.app
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        }

        fn shows_unified_inbox(&self) -> bool {
            self.state
                .borrow()
                .sidebar_rows
                .iter()
                .any(|row| row.kind == crate::SidebarRowKind::UnifiedInbox)
        }
    }

    /// One account whose cache was written while the unified inbox was
    /// selected, with every standard mailbox discovered.
    fn single_account_snapshot() -> WarmStartSnapshot {
        let mut mailboxes = vec![mail::MailboxEntry {
            is_account: true,
            is_standard: false,
            label: "Person".into(),
            scope: None,
            count: 0,
            ..mailbox()
        }];
        for (index, mailbox_kind) in crate::StandardMailbox::ALL.into_iter().enumerate() {
            mailboxes.push(mail::MailboxEntry {
                label: mailbox_kind.label().into(),
                scope: Some(Scope::Account {
                    account_id: 7,
                    mailbox: mailbox_kind,
                }),
                folder_id: index as i64 + 1,
                count: if index == 0 { 3 } else { 0 },
                ..mailbox()
            });
        }
        WarmStartSnapshot::capture(WarmStartProjection {
            scope: Scope::UNIFIED_INBOX,
            inbox_count: 3,
            grouping: MailGrouping::Conversations,
            order: ListOrder::NewestFirst,
            next_cursor: None,
            accounts: &[account()],
            profile_accounts: None,
            messages: &[message()],
            mailboxes: &mailboxes,
            unified_mailboxes: &[],
        })
    }

    #[test]
    fn warm_start_paints_accounts_and_rows_before_core_ready() {
        let harness = Harness::new();
        let app = &harness.app;
        let mut loading_frames = Vec::new();
        for theme in ["light", "dark"] {
            app.set_theme_mode(theme.into());
            let frame = harness.render(&format!("loading-{theme}"));
            assert!(
                frame.iter().any(|pixel| *pixel != frame[0]),
                "account discovery must display a loading view instead of a blank window"
            );
            loading_frames.push(frame);
        }

        harness.apply_single_account_snapshot();

        let state = &harness.state;
        assert!(app.get_startup_hydrated());
        assert!(!app.get_startup_ready());
        assert!(state.borrow().core.is_none());
        assert!(state.borrow().messages[0].html.is_none());
        assert_eq!(app.get_connected_accounts().row_count(), 1);
        assert_eq!(app.get_connected_accounts().row_data(0).unwrap().id, 7);
        assert_eq!(app.get_emails().row_count(), 1);
        assert_eq!(
            app.get_emails().row_data(0).unwrap().subject,
            "Cached subject"
        );
        // The cached unified selection resolves to the only account's inbox,
        // which the sidebar lists first, without a unified section.
        assert_eq!(state.borrow().scope, PERSON_INBOX);
        assert_eq!(app.get_selected_scope(), PERSON_INBOX.to_string());
        assert_eq!(app.get_selected_scope_title(), "Person / Inbox");
        let rows = state.borrow().sidebar_rows.iter().collect::<Vec<_>>();
        assert_eq!(rows[0].kind, crate::SidebarRowKind::Account);
        assert!(!harness.shows_unified_inbox());
        assert_eq!(
            rows.iter()
                .filter(|row| row.kind == crate::SidebarRowKind::Folder)
                .count(),
            crate::StandardMailbox::ALL.len()
        );
        for (index, theme) in ["light", "dark"].into_iter().enumerate() {
            app.set_theme_mode(theme.into());
            assert_ne!(
                harness.render(&format!("cached-{theme}")),
                loading_frames[index]
            );
        }
        app.window().set_size(slint::PhysicalSize::new(390, 844));
        harness.render("cached-compact");
        assert!(
            !app.get_startup_ready(),
            "rendering cached mail must not unlock storage actions"
        );
    }

    /// The projected account folders must be usable in the compact drawer as
    /// well as the desktop sidebar once startup unlocks the shell.
    #[test]
    fn single_account_inbox_is_selectable_in_compact_and_desktop_layouts() {
        let harness = Harness::new();
        let app = &harness.app;
        harness.apply_single_account_snapshot();
        app.set_startup_ready(true);
        app.global::<crate::MotionSettings>().set_enabled(false);
        let selected = Rc::new(RefCell::new(String::new()));
        let clicked = selected.clone();
        app.on_select_scope(move |scope| *clicked.borrow_mut() = scope.to_string());
        app.window().set_size(slint::PhysicalSize::new(390, 844));
        for theme in ["light", "dark"] {
            app.set_theme_mode(theme.into());
            harness.render(&format!("ready-compact-{theme}"));
            harness.click(31., 29.);
            harness.render(&format!("ready-compact-drawer-{theme}"));
            selected.borrow_mut().clear();
            harness.click(140., 165.);
            assert_eq!(selected.borrow().parse(), Ok(PERSON_INBOX));
            app.window().set_size(slint::PhysicalSize::new(1320, 800));
            harness.render(&format!("ready-desktop-{theme}"));
            selected.borrow_mut().clear();
            harness.click(140., 209.);
            assert_eq!(selected.borrow().parse(), Ok(PERSON_INBOX));
            app.window().set_size(slint::PhysicalSize::new(390, 844));
        }
    }

    #[test]
    fn account_inventory_controls_the_unified_section_and_visible_selection() {
        let harness = Harness::new();
        harness.apply_single_account_snapshot();
        let state = &harness.state;
        // A second account counts even before its folders arrive, and even
        // when offline. Adding it keeps the account's own selection.
        let mut offline = account();
        offline.id = 8;
        offline.sync_state = "error".into();
        offline.sync_error = Some("Offline".into());
        let both = vec![account(), offline];
        assert!(!state.borrow_mut().set_connected_accounts(both.clone()));
        // Searching folders must not redefine that inventory.
        for query in ["", "Inbox", ""] {
            state.borrow_mut().folder_filter = query.into();
            crate::refresh_sidebar(state);
            assert_eq!(harness.shows_unified_inbox(), query.is_empty());
            assert_eq!(state.borrow().scope, PERSON_INBOX);
        }

        // A unified selection that would disappear moves to the survivor.
        state
            .borrow_mut()
            .set_scope(Scope::Unified(crate::StandardMailbox::Sent));
        assert!(state.borrow_mut().set_connected_accounts(vec![account()]));
        assert_eq!(
            state.borrow().scope,
            Scope::Account {
                account_id: 7,
                mailbox: crate::StandardMailbox::Sent,
            }
        );
        crate::refresh_sidebar(state);
        assert!(!harness.shows_unified_inbox());
        assert!(!state.borrow_mut().set_connected_accounts(both));
        crate::refresh_sidebar(state);
        assert!(harness.shows_unified_inbox());
    }

    fn account() -> Account {
        Account {
            id: 7,
            email: "person@example.com".into(),
            display_name: Some("Person".into()),
            avatar_url: None,
            provider: Provider::Generic,
            auth_kind: AuthKind::Password,
            mail_protocol: MailProtocol::Imap,
            sync_state: "idle".into(),
            sync_error: None,
            can_create_top_level_mailbox: true,
            server_keeps_mail_days: None,
            caught_up_at: None,
            calendar_only: false,
        }
    }

    fn message() -> mail::MailMessage {
        mail::MailMessage {
            id: 9,
            thread_id: Some(19),
            message_id: None,
            account_id: 7,
            account: "Person".into(),
            folder: "Inbox".into(),
            sender: "Sender".into(),
            address: "sender@example.net".into(),
            domain: "example.net".into(),
            initials: "S".into(),
            subject: "Cached subject".into(),
            preview: "Cached preview".into(),
            date_ms: 1_700_000_000_000,
            to: "secret-recipient@example.com".into(),
            label: "UNREAD".into(),
            unread: true,
            starred: false,
            has_attachments: false,
            message_count: 2,
            has_replied: true,
            high_importance: false,
            security: String::new(),
            breaches: Vec::new(),
            security_reason: String::new(),
            security_model: String::new(),
            security_checked_at: None,
            is_outgoing: false,
            labels: Vec::new(),
            html: Some("<p>body must not enter warm cache</p>".into()),
            text: Some("Plain body must not enter warm cache".into()),
            attachments: Vec::new(),
            body_pending: false,
            sender_verification: "domain".into(),
            can_unsubscribe: false,
            can_block: false,
            sender_blocked: false,
            invitation: None,
            receipt: None,
            returned_receipts: Vec::new(),
            verification_code: Some("48291375".into()),
            account_uses_labels: true,
            first_attachment: Some(flectar_mail_core::models::AttachmentMeta {
                id: 3,
                filename: Some("cached-file-name.pdf".into()),
                mime_type: Some("application/pdf".into()),
                size: Some(42),
                is_inline: false,
            }),
            attachment_total: 1,
        }
    }

    fn mailbox() -> mail::MailboxEntry {
        mail::MailboxEntry {
            account_id: 7,
            folder_id: 1,
            parent_folder_id: -1,
            depth: 0,
            has_children: false,
            is_standard: true,
            is_selectable: true,
            can_create_children: true,
            can_rename: false,
            can_delete: false,
            label: "Inbox".into(),
            scope: Some(PERSON_INBOX),
            context: "Person".into(),
            detail: "person@example.com".into(),
            avatar: "P".into(),
            is_account: false,
            count: 3,
        }
    }

    #[tokio::test]
    async fn warm_start_round_trip_excludes_bodies_and_recipient_details() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("warm.json");
        let accounts = [account()];
        let messages = [message()];
        let mailboxes = [mailbox()];
        let snapshot = WarmStartSnapshot::capture(WarmStartProjection {
            scope: PERSON_INBOX,
            inbox_count: 3,
            grouping: MailGrouping::Messages,
            order: ListOrder::NewestFirst,
            next_cursor: Some(WarmStartCursor::Message(MessageCursor {
                date: 123,
                message_id: 9,
            })),
            accounts: &accounts,
            profile_accounts: Some(&[7]),
            messages: &messages,
            mailboxes: &mailboxes,
            unified_mailboxes: &[],
        });

        write_warm_start_snapshot(&path, &snapshot).await.unwrap();
        let bytes = tokio::fs::read(&path).await.unwrap();
        let json = String::from_utf8(bytes).unwrap();
        assert!(!json.contains("body must not enter"));
        assert!(!json.contains("secret-recipient"));
        assert!(!json.contains("48291375"));
        assert!(!json.contains("cached-file-name"));

        let restored = load_warm_start_snapshot(&path).await.unwrap();
        assert_eq!(restored.scope, PERSON_INBOX);
        assert_eq!(restored.grouping, MailGrouping::Messages);
        assert_eq!(
            restored.next_cursor,
            Some(WarmStartCursor::Message(MessageCursor {
                date: 123,
                message_id: 9,
            }))
        );
        assert_eq!(restored.profile_accounts, Some(vec![7]));
        assert_eq!(restored.messages.len(), 1);
        let restored_message = mail::MailMessage::from(restored.messages[0].clone());
        assert_eq!(restored_message.subject, "Cached subject");
        assert!(restored_message.has_replied);
        assert!(restored_message.html.is_none());
        assert!(restored_message.body_pending);
        assert!(restored_message.verification_code.is_none());
        assert!(restored_message.account_uses_labels);
    }

    #[tokio::test]
    async fn invalid_warm_start_is_ignored_and_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("warm.json");
        tokio::fs::write(&path, br#"{"format_version":999}"#)
            .await
            .unwrap();

        assert!(load_warm_start_snapshot(&path).await.is_none());
        assert!(!path.exists());
    }

    /// Version 3 keyed account mailboxes by display name; such a cache is a
    /// miss rather than a selection of the wrong mailbox.
    #[tokio::test]
    async fn previous_format_warm_start_is_ignored_and_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("warm.json");
        let mut json = serde_json::to_value(single_account_snapshot()).unwrap();
        json["format_version"] = 3.into();
        json["scope"] = "Person / Inbox".into();
        tokio::fs::write(&path, json.to_string()).await.unwrap();

        assert!(load_warm_start_snapshot(&path).await.is_none());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn warm_start_projection_is_bounded_to_the_visible_page() {
        let messages = (0..100)
            .map(|id| {
                let mut value = message();
                value.id = id;
                value
            })
            .collect::<Vec<_>>();
        let accounts = [account()];
        let mailboxes = [mailbox()];
        let snapshot = WarmStartSnapshot::capture(WarmStartProjection {
            scope: Scope::UNIFIED_INBOX,
            inbox_count: messages.len(),
            grouping: MailGrouping::Conversations,
            order: ListOrder::NewestFirst,
            next_cursor: None,
            accounts: &accounts,
            profile_accounts: None,
            messages: &messages,
            mailboxes: &mailboxes,
            unified_mailboxes: &[],
        });

        assert_eq!(snapshot.messages.len(), MAX_WARM_START_MESSAGES);
    }

    #[test]
    fn benchmark_warm_cache_fixture_matches_the_current_format() {
        let snapshot: WarmStartSnapshot = serde_json::from_str(include_str!(
            "../resources/benchmarks/warm-cache/warm-start-mailbox-v8.json"
        ))
        .unwrap();

        assert!(snapshot.is_valid());
        assert_eq!(snapshot.messages.len(), 3);
        // Caches written before profiles filtered mail show every account.
        assert_eq!(snapshot.profile_accounts, None);
        assert_eq!(snapshot.grouping, MailGrouping::Conversations);
    }
}

#[cfg(test)]
mod summary_stream_tests {
    use super::{SummaryStream, summary_display_text};

    #[test]
    fn streamed_summary_shows_fields_before_the_json_is_complete() {
        assert_eq!(
            summary_display_text(r#"{"timeline":[],"keyPoints":["Ask for the Q3 numbers","#),
            "Key points\n• Ask for the Q3 numbers\n\n"
        );
        // A field still being written keeps the text received so far.
        assert_eq!(
            summary_display_text(r#"{"keyPoints":["sent"],"nextAction":"reply tod"#),
            "Key points\n• sent\n\nNext action\nreply tod\n\n"
        );
    }

    #[test]
    fn streamed_summary_decodes_escapes_and_explains_sections() {
        let text = summary_display_text(
            r#"{"keyPoints":["a } b","quote \" inside"],"nextAction":"do it","proposedReply":"hi \u00e9"}"#,
        );
        assert_eq!(
            text,
            "Key points\n• a } b\n• quote \" inside\n\nNext action\ndo it\n\nSuggested reply\nhi é\n"
        );
    }

    #[test]
    fn summary_stream_ignores_replaced_requests() {
        let mut stream = SummaryStream::default();
        let generation = stream.begin(7);
        assert!(stream.append(7, r#"{"keyPoints":["first"]"#));
        assert!(stream.is_current(generation));
        assert_eq!(stream.take().as_deref(), Some("Key points\n• first\n\n"));
        assert!(stream.take().is_none());
        assert!(!stream.append(9, r#"{"keyPoints":["old"]}"#));
        let newer = stream.begin(9);
        assert!(!stream.is_current(generation));
        assert!(stream.is_current(newer));
    }
}
