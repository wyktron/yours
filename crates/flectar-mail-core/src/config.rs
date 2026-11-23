use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    /// Durable application state. Databases, raw mail, drafts, queues, and
    /// settings live here and must survive process death and cache eviction.
    pub data_dir: PathBuf,
    /// Reconstructable data such as downloaded attachment bodies and preview
    /// scratch files. Hosts may place this in an OS-managed cache directory.
    pub cache_dir: PathBuf,
}


/// The durable profile directory, and the one earlier releases used. The
/// rename copies the old profile across once; the old directory is never
/// removed, and stays in use if the copy cannot be verified.
#[cfg(desktop)]
const PROFILE_DIR: &str = "yours-mail";
#[cfg(desktop)]
const LEGACY_PROFILE_DIR: &str = "flectar-mail";
/// Reconstructable caches keep their earlier directory name, so the rename
/// does not pay to copy them.
#[cfg(desktop)]
const LEGACY_CACHE_DIR: &str = "flectar-mail";
#[cfg(desktop)]
const MIGRATION_MARKER: &str = ".migrated-from-flectar-mail";

#[cfg(desktop)]
#[derive(Debug, Default, PartialEq, Eq)]
struct ProfileSize {
    files: u64,
    bytes: u64,
}

#[cfg(desktop)]
#[derive(Debug, Default, PartialEq, Eq)]
struct ProfileRows {
    messages: i64,
    accounts: i64,
    folders: i64,
    calendar_events: i64,
}

/// Use the renamed profile directory, copying the old one across the first
/// time. Returns the old directory when the copy cannot be verified, so the
/// user keeps a working profile either way.
#[cfg(desktop)]
fn migrate_legacy_profile(root: &std::path::Path) -> crate::error::Result<PathBuf> {
    let current = root.join(PROFILE_DIR);
    let legacy = root.join(LEGACY_PROFILE_DIR);
    if current.exists() || !legacy.is_dir() {
        return Ok(current);
    }
    let staging = root.join(format!(".{PROFILE_DIR}.migrating"));
    match copy_legacy_profile(&legacy, &current, &staging) {
        Ok(()) => Ok(current),
        Err(error) => {
            // Never leave a half profile that a later start would mistake for
            // a finished migration.
            let _ = std::fs::remove_dir_all(&staging);
            eprintln!(
                "data: could not copy the profile from {} to {}: {error}; \
                 continuing with the old profile",
                legacy.display(),
                current.display()
            );
            Ok(legacy)
        }
    }
}

#[cfg(desktop)]
fn copy_legacy_profile(
    legacy: &std::path::Path,
    current: &std::path::Path,
    staging: &std::path::Path,
) -> std::io::Result<()> {
    // Creating it exclusively also refuses a second migration: a concurrent
    // bridge process falls back to the old profile instead of copying too.
    std::fs::create_dir(staging)?;
    eprintln!(
        "data: moving the profile from {} to {}; the old directory is kept",
        legacy.display(),
        current.display()
    );
    let source_size = profile_size(legacy)?;
    let copied = copy_profile_tree(legacy, staging)?;
    if copied != source_size {
        return Err(std::io::Error::other(format!(
            "copied {} files ({} bytes) but the source holds {} files ({} bytes)",
            copied.files, copied.bytes, source_size.files, source_size.bytes
        )));
    }
    let rows = verify_profile_databases(staging)?;
    write_migration_marker(staging, legacy, &copied, &rows)?;
    std::fs::rename(staging, current)?;
    eprintln!(
        "data: copied and verified {} files, {} bytes, messages={}, accounts={}, \
         folders={}, calendar events={}; using {}",
        copied.files,
        copied.bytes,
        rows.messages,
        rows.accounts,
        rows.folders,
        rows.calendar_events,
        current.display()
    );
    Ok(())
}

#[cfg(desktop)]
fn copy_profile_tree(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<ProfileSize> {
    let mut total = ProfileSize::default();
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            std::fs::create_dir(&to)?;
            let nested = copy_profile_tree(&from, &to)?;
            copy_permissions(&from, &to)?;
            total.files += nested.files;
            total.bytes += nested.bytes;
        } else if kind.is_file() {
            let bytes = std::fs::copy(&from, &to)?;
            total.files += 1;
            total.bytes += bytes;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
                total.files += 1;
            }
        }
        // Sockets, pipes and the like are runtime state, not profile data.
    }
    Ok(total)
}

#[cfg(desktop)]
fn profile_size(root: &std::path::Path) -> std::io::Result<ProfileSize> {
    let mut total = ProfileSize::default();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let nested = profile_size(&entry.path())?;
            total.files += nested.files;
            total.bytes += nested.bytes;
        } else if kind.is_file() {
            total.files += 1;
            total.bytes += entry.metadata()?.len();
        } else if kind.is_symlink() {
            total.files += 1;
        }
    }
    Ok(total)
}

#[cfg(desktop)]
fn copy_permissions(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let permissions = std::fs::metadata(from)?.permissions();
    std::fs::set_permissions(to, permissions)
}

/// Open the copied databases and count their rows, so a truncated or corrupt
/// copy fails the migration instead of replacing a working profile.
#[cfg(desktop)]
fn verify_profile_databases(dir: &std::path::Path) -> std::io::Result<ProfileRows> {
    let mail = dir.join("flectar-mail.db");
    let calendar = dir.join("flectar-calendar.db");
    let mut rows = ProfileRows::default();
    if mail.is_file() {
        let connection = rusqlite::Connection::open(&mail).map_err(sqlite_error)?;
        quick_check(&connection)?;
        rows.messages = count_rows(&connection, "messages")?;
        rows.accounts = count_rows(&connection, "accounts")?;
        rows.folders = count_rows(&connection, "folders")?;
    }
    if calendar.is_file() {
        let connection = rusqlite::Connection::open(&calendar).map_err(sqlite_error)?;
        quick_check(&connection)?;
        rows.calendar_events = count_rows(&connection, "calendar_events")?;
    }
    Ok(rows)
}

#[cfg(desktop)]
fn quick_check(connection: &rusqlite::Connection) -> std::io::Result<()> {
    let result: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    if result != "ok" {
        return Err(std::io::Error::other(format!(
            "SQLite quick_check reported {result}"
        )));
    }
    Ok(())
}

#[cfg(desktop)]
fn count_rows(connection: &rusqlite::Connection, table: &str) -> std::io::Result<i64> {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(sqlite_error)
}

#[cfg(desktop)]
fn sqlite_error(error: rusqlite::Error) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(desktop)]
fn write_migration_marker(
    dir: &std::path::Path,
    legacy: &std::path::Path,
    size: &ProfileSize,
    rows: &ProfileRows,
) -> std::io::Result<()> {
    use std::io::Write;
    let copied_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut marker = std::fs::File::create(dir.join(MIGRATION_MARKER))?;
    writeln!(marker, "source={}", legacy.display())?;
    writeln!(marker, "copied_at_unix={copied_at}")?;
    writeln!(marker, "files={}", size.files)?;
    writeln!(marker, "bytes={}", size.bytes)?;
    writeln!(marker, "messages={}", rows.messages)?;
    writeln!(marker, "accounts={}", rows.accounts)?;
    writeln!(marker, "folders={}", rows.folders)?;
    writeln!(marker, "calendar_events={}", rows.calendar_events)?;
    writeln!(marker, "verified=sizes,sqlite quick_check,row counts")?;
    marker.sync_all()?;
    Ok(())
}
impl Paths {
    /// Resolve desktop directories. Mobile hosts inject their sandbox paths
    /// through `Paths::new`; silently using the process working directory is
    /// never safe there.
    #[cfg(desktop)]
    pub fn default_dirs() -> crate::error::Result<Self> {
        let data_root = dirs::data_dir().ok_or_else(|| {
            crate::error::CoreError::Other(
                "the operating system has no application data directory".into(),
            )
        })?;
        let cache_root = dirs::cache_dir().ok_or_else(|| {
            crate::error::CoreError::Other(
                "the operating system has no application cache directory".into(),
            )
        })?;
        let data_dir = migrate_legacy_profile(&data_root)?;
        Ok(Self::new(data_dir, cache_root.join(LEGACY_CACHE_DIR)))
    }

    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            data_dir,
            cache_dir,
        }
    }

    pub fn for_tests(root: &std::path::Path) -> Self {
        Self::new(root.to_path_buf(), root.join("cache"))
    }

    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("flectar-mail.db")
    }

    /// Calendar data lives in its own SQLite database so large mailbox writes,
    /// FTS maintenance, and mail backfills never contend with calendar reads or
    /// sync bookkeeping.
    pub fn calendar_db_file(&self) -> PathBuf {
        self.data_dir.join("flectar-calendar.db")
    }

    pub fn files_db_file(&self) -> PathBuf {
        self.data_dir.join("flectar-files.db")
    }
    pub fn files_cache_dir(&self, account_id: i64) -> PathBuf {
        self.cache_dir.join("files").join(account_id.to_string())
    }
    pub fn files_staging_dir(&self, account_id: i64) -> PathBuf {
        self.data_dir
            .join("file_transfers")
            .join(account_id.to_string())
    }

    /// Small, reconstructable projection of the last visible mailbox page.
    /// The native shell may paint this before the databases finish opening;
    /// SQLite remains authoritative and replaces it during normal startup.
    pub fn warm_start_file(&self) -> PathBuf {
        self.cache_dir.join("warm-start-mailbox.json")
    }

    /// Superseded reconstructable caches that can be removed without reading.
    ///
    /// TODO(warm-start-v2-cleanup): After the first public release using the
    /// stable `warm-start-mailbox.json` filename has been superseded by a later
    /// release, remove this method, its startup cleanup loop, and the matching
    /// path test. Keeping it longer is harmless; removing it sooner only leaves
    /// the old, unread v2 cache orphaned on direct upgrades from older builds.
    pub fn obsolete_warm_start_files(&self) -> [PathBuf; 1] {
        [self.cache_dir.join("warm-start-mailbox-v2.json")]
    }

    /// Directory holding every account's [`Self::mail_dir`].
    pub fn mail_root(&self) -> PathBuf {
        self.data_dir.join("mail")
    }

    /// Directory for raw .eml files of one account.
    pub fn mail_dir(&self, account_id: i64) -> PathBuf {
        self.mail_root().join(account_id.to_string())
    }

    /// Directory for the raw messages being copied or moved to another
    /// account.
    pub fn transfers_dir(&self) -> PathBuf {
        self.data_dir.join("transfers")
    }

    /// Directory for extracted attachment files.
    pub fn attachments_dir(&self, account_id: i64) -> PathBuf {
        self.cache_dir
            .join("attachments")
            .join(account_id.to_string())
    }

    /// Root for app-managed copies of outgoing (draft) attachments. Files
    /// picked in the composer are staged here so dispatch only ever reads
    /// files the app itself wrote, never an arbitrary caller-supplied path.
    pub fn draft_attachments_dir(&self) -> PathBuf {
        self.data_dir.join("draft_attachments")
    }

    /// Directory holding local embedding-model weights (`<models_dir>/<key>`).
    pub fn models_dir(&self) -> PathBuf {
        self.cache_dir.join("models")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.cache_dir.join("tmp")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for directory in [
            &self.data_dir,
            &self.cache_dir,
            &self.mail_root(),
            &self.draft_attachments_dir(),
            &self.temp_dir(),
        ] {
            std::fs::create_dir_all(directory)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(&self.cache_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_paths_separate_durable_and_reconstructable_state() {
        let root = std::path::Path::new("/private/app");
        let paths = Paths::new(root.join("data"), root.join("cache"));

        assert_eq!(paths.db_file(), root.join("data/flectar-mail.db"));
        assert_eq!(
            paths.calendar_db_file(),
            root.join("data/flectar-calendar.db")
        );
        assert_eq!(
            paths.warm_start_file(),
            root.join("cache/warm-start-mailbox.json")
        );
        assert_eq!(
            paths.obsolete_warm_start_files(),
            [root.join("cache/warm-start-mailbox-v2.json")]
        );
        assert_eq!(paths.mail_dir(7), root.join("data/mail/7"));
        assert_eq!(
            paths.draft_attachments_dir(),
            root.join("data/draft_attachments")
        );
        assert_eq!(paths.attachments_dir(7), root.join("cache/attachments/7"));
        assert_eq!(paths.temp_dir(), root.join("cache/tmp"));
    }
}
