//! Breach verdicts cached per sender address, so a sender is asked about
//! once a week rather than on every arrival, and the provider's rate limit is
//! never exceeded by a busy inbox.

use rusqlite::{Connection, OptionalExtension, params};

use crate::breaches::Breach;
use crate::error::Result;

/// How long a verdict stays usable before the sender is looked up again.
pub const TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// A cached verdict and when it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedBreaches {
    pub checked_at: i64,
    pub breaches: Vec<Breach>,
}

fn normalize(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

/// The verdict for this address, when one was stored within the TTL.
/// A stale entry reads as a miss so the caller asks the provider again.
pub fn get(conn: &Connection, address: &str, now: i64) -> Result<Option<CachedBreaches>> {
    let address = normalize(address);
    if address.is_empty() {
        return Ok(None);
    }
    let stored = conn
        .query_row(
            "SELECT checked_at, breach_json FROM sender_breaches WHERE address = ?1",
            params![address],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((checked_at, json)) = stored else {
        return Ok(None);
    };
    if now.saturating_sub(checked_at) > TTL_MS {
        return Ok(None);
    }
    let breaches = serde_json::from_str::<Vec<Breach>>(&json).unwrap_or_default();
    Ok(Some(CachedBreaches {
        checked_at,
        breaches,
    }))
}

/// Store a verdict, replacing an older one for the same address. An empty
/// list is stored too: a sender with no breaches is cached like any other,
/// so the 404 is not asked again for a week.
pub fn put(conn: &Connection, address: &str, checked_at: i64, breaches: &[Breach]) -> Result<()> {
    let address = normalize(address);
    if address.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string(breaches).map_err(|error| crate::error::CoreError::Other(error.to_string()))?;
    conn.execute(
        "INSERT INTO sender_breaches (address, checked_at, breach_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(address) DO UPDATE SET
             checked_at = excluded.checked_at,
             breach_json = excluded.breach_json",
        params![address, checked_at, json],
    )?;
    Ok(())
}

/// Every cached address with at least one breach, for painting the mail
/// list and the reader without a query per row.
pub fn all_known(conn: &Connection) -> Result<Vec<(String, Vec<Breach>)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT address, breach_json FROM sender_breaches
          WHERE breach_json <> '[]' ORDER BY address",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (address, json) = row?;
        let breaches = serde_json::from_str::<Vec<Breach>>(&json).unwrap_or_default();
        if !breaches.is_empty() {
            out.push((address, breaches));
        }
    }
    Ok(out)
}

/// The unique, checkable sender addresses of these conversations' messages.
pub fn addresses_for_threads(conn: &Connection, thread_ids: &[i64]) -> Result<Vec<String>> {
    let mut addresses: Vec<String> = Vec::new();
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT LOWER(TRIM(m.from_addr)) FROM messages m
          WHERE m.thread_id = ?1
            AND m.is_outgoing = 0
            AND TRIM(COALESCE(m.from_addr, '')) <> ''",
    )?;
    for thread_id in thread_ids {
        let rows = stmt.query_map(params![thread_id], |row| row.get::<_, String>(0))?;
        for row in rows {
            let address = row?;
            if crate::breaches::should_check_address(&address) && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breaches::Breach;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sender_breaches (
                 address TEXT PRIMARY KEY NOT NULL,
                 checked_at INTEGER NOT NULL,
                 breach_json TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn adobe() -> Breach {
        Breach {
            name: "Adobe".into(),
            title: "Adobe".into(),
            domain: "adobe.com".into(),
            breach_date: "2013-10-04".into(),
            pwn_count: 152_445_165,
            data_classes: vec!["Email addresses".into()],
        }
    }

    #[test]
    fn a_stored_verdict_reads_back_normalized() {
        let conn = connection();
        put(&conn, " Person@Example.COM ", 1_000, &[adobe()]).unwrap();
        let got = get(&conn, "person@example.com", 2_000).unwrap().unwrap();
        assert_eq!(got.checked_at, 1_000);
        assert_eq!(got.breaches, vec![adobe()]);
        assert_eq!(get(&conn, "PERSON@EXAMPLE.COM", 2_000).unwrap(), Some(got));
    }

    #[test]
    fn a_clean_sender_is_cached_as_an_empty_list() {
        let conn = connection();
        put(&conn, "clean@example.com", 1_000, &[]).unwrap();
        let got = get(&conn, "clean@example.com", 2_000).unwrap().unwrap();
        assert!(got.breaches.is_empty());
        assert!(all_known(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_verdict_goes_stale_after_seven_days() {
        let conn = connection();
        put(&conn, "person@example.com", 1_000, &[adobe()]).unwrap();
        assert!(get(&conn, "person@example.com", 1_000 + TTL_MS).unwrap().is_some());
        assert!(get(&conn, "person@example.com", 1_000 + TTL_MS + 1).unwrap().is_none());
        // Stale means "ask again", not "forget": the old verdict is still
        // there until the new one replaces it.
        assert_eq!(all_known(&conn).unwrap().len(), 1);
    }

    #[test]
    fn a_new_verdict_replaces_the_old_one() {
        let conn = connection();
        put(&conn, "person@example.com", 1_000, &[adobe()]).unwrap();
        put(&conn, "person@example.com", 2_000, &[]).unwrap();
        let got = get(&conn, "person@example.com", 2_100).unwrap().unwrap();
        assert_eq!(got.checked_at, 2_000);
        assert!(got.breaches.is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sender_breaches", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn only_breached_addresses_are_known() {
        let conn = connection();
        put(&conn, "bad@example.com", 1_000, &[adobe()]).unwrap();
        put(&conn, "good@example.com", 1_000, &[]).unwrap();
        let known = all_known(&conn).unwrap();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].0, "bad@example.com");
        assert_eq!(known[0].1, vec![adobe()]);
    }

    #[test]
    fn addresses_of_a_thread_are_unique_and_checkable() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE messages (
                 thread_id INTEGER,
                 from_addr TEXT,
                 is_outgoing INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO messages (thread_id, from_addr, is_outgoing) VALUES
                 (7, 'Person@Example.com', 0),
                 (7, 'person@example.com', 0),
                 (7, 'me@mine.com', 1),
                 (7, 'not-an-address', 0),
                 (8, 'other@example.org', 0);",
        )
        .unwrap();
        assert_eq!(
            addresses_for_threads(&conn, &[7]).unwrap(),
            vec!["person@example.com".to_owned()]
        );
        assert_eq!(
            addresses_for_threads(&conn, &[7, 8]).unwrap(),
            vec!["person@example.com".to_owned(), "other@example.org".to_owned()]
        );
    }
}
