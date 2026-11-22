//! The AI's judgement of a message for phishing and junk.
//!
//! The scanner judges recent mail once, stores the verdict on the message,
//! and the mail list paints those rows. Nothing here acts on the mail: the
//! user decides whether to block, move or delete.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

/// How far back the scanner looks, so turning it on judges recent mail rather
/// than the whole archive.
pub const SCAN_DAYS: i64 = 7;
/// How many messages one pass judges, keeping a local model responsive.
pub const SCAN_BATCH: usize = 12;
/// Longest body excerpt sent to the model.
const BODY_LIMIT: usize = 3000;

/// A message waiting for a judgement.
#[derive(Debug, Clone)]
pub struct PendingScan {
    pub id: i64,
    pub thread_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub body: String,
    pub received: i64,
    /// Authentication and routing data captured when the message was parsed.
    pub evidence: String,
}

/// Recent mail that has not been judged yet, newest first.
pub fn pending(conn: &Connection, since: i64, limit: i64) -> Result<Vec<PendingScan>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.id, COALESCE(m.thread_id, 0), COALESCE(m.from_name, ''),
                COALESCE(m.from_addr, ''), COALESCE(m.subject, ''),
                COALESCE(m.snippet, ''),
                COALESCE(NULLIF(SUBSTR(mb.text_body, 1, ?3), ''), COALESCE(m.snippet, '')),
                m.date,
                COALESCE(m.security_evidence, '')
           FROM messages m
           LEFT JOIN message_bodies mb ON mb.message_id = m.id
          WHERE m.security_checked_at IS NULL
            AND m.is_draft = 0
            AND m.is_outgoing = 0
            AND m.date >= ?1
            AND COALESCE(m.from_addr, '') <> ''
          ORDER BY m.date DESC
          LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![since, limit, BODY_LIMIT as i64], |row| {
        Ok(PendingScan {
            id: row.get(0)?,
            thread_id: row.get(1)?,
            from_name: row.get(2)?,
            from_addr: row.get(3)?,
            subject: row.get(4)?,
            snippet: row.get(5)?,
            body: row.get(6)?,
            received: row.get(7)?,
            evidence: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Records the judgement on a message and returns its thread, for the list to
/// refresh. The reason is clipped so a chatty model cannot fill the database.
pub fn store(
    conn: &Connection,
    message_id: i64,
    verdict: &str,
    reason: &str,
    model: &str,
    checked_at: i64,
) -> Result<Option<i64>> {
    let reason: String = reason.trim().chars().take(300).collect();
    conn.execute(
        "UPDATE messages
            SET security_verdict = ?2, security_reason = ?3,
                security_checked_at = ?4, security_model = ?5
          WHERE id = ?1",
        params![message_id, verdict, reason, checked_at, model],
    )?;
    Ok(conn
        .query_row(
            "SELECT thread_id FROM messages WHERE id = ?1",
            [message_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten())
}

/// Forget every judgement, for a re-run after the model changed.
pub fn clear(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE messages
            SET security_verdict = NULL, security_reason = NULL,
                security_checked_at = NULL, security_model = NULL
          WHERE security_checked_at IS NOT NULL",
        [],
    )?)
}
