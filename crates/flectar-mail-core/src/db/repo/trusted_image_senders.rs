//! Senders whose remote images load without asking. The reader's
//! "Always load images" choice remembers the From address here, and the
//! remote-image policy consults it before blocking a message's images.
use crate::error::Result;
use crate::models::TrustedImageSender;
use rusqlite::{Connection, params};

/// The form an address is kept and compared in. None for something that
/// isn't an address.
pub fn normalized(address: &str) -> Option<String> {
    super::blocked::normalized(address)
}

/// Trust the remote images of mail from `address`. False when it already did.
pub fn add(conn: &Connection, address: &str, now: i64) -> Result<bool> {
    Ok(conn.execute(
        "INSERT INTO trusted_image_senders (address, created_at) VALUES (?1, ?2)
         ON CONFLICT(address) DO NOTHING",
        params![address, now],
    )? == 1)
}

/// Stop trusting `address`. False when it wasn't trusted.
pub fn remove(conn: &Connection, address: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM trusted_image_senders WHERE address = ?1",
        params![address],
    )? == 1)
}

/// Whether mail from `address` may load its remote images without asking.
pub fn is_trusted(conn: &Connection, address: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM trusted_image_senders WHERE address = ?1)",
        params![address],
        |r| r.get(0),
    )?)
}

/// Every trusted address, alphabetically.
pub fn list(conn: &Connection) -> Result<Vec<TrustedImageSender>> {
    let mut stmt = conn
        .prepare("SELECT address, created_at FROM trusted_image_senders ORDER BY address")?;
    let rows = stmt.query_map([], |r| {
        Ok(TrustedImageSender {
            address: r.get(0)?,
            created_at: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil;

    #[test]
    fn addresses_are_kept_lowercase_and_once() {
        let conn = testutil::conn();
        assert_eq!(
            normalized("  News@Example.COM ").as_deref(),
            Some("news@example.com")
        );
        assert_eq!(normalized("not an address"), None);
        assert_eq!(normalized("nobody"), None);

        assert!(add(&conn, "news@example.com", 10).unwrap());
        assert!(!add(&conn, "news@example.com", 20).unwrap());
        assert!(add(&conn, "ads@example.com", 30).unwrap());
        assert!(is_trusted(&conn, "news@example.com").unwrap());
        assert!(!is_trusted(&conn, "friend@example.com").unwrap());
        assert_eq!(
            list(&conn).unwrap(),
            vec![
                TrustedImageSender {
                    address: "ads@example.com".into(),
                    created_at: 30
                },
                TrustedImageSender {
                    address: "news@example.com".into(),
                    created_at: 10
                },
            ]
        );
        assert!(remove(&conn, "news@example.com").unwrap());
        assert!(!remove(&conn, "news@example.com").unwrap());
        assert!(!is_trusted(&conn, "news@example.com").unwrap());
        assert_eq!(
            list(&conn).unwrap(),
            vec![TrustedImageSender {
                address: "ads@example.com".into(),
                created_at: 30
            }]
        );
    }

    #[test]
    fn another_message_from_the_same_sender_is_still_trusted() {
        let conn = testutil::conn();
        testutil::seed_account(&conn);
        let (_, first) = testutil::seed_message(&conn, "News@Example.com", "One", false);
        let (_, second) = testutil::seed_message(&conn, "news@example.com", "Two", false);
        assert!(add(&conn, "news@example.com", 10).unwrap());

        let from = |id: i64| {
            conn.query_row(
                "SELECT LOWER(from_addr) FROM messages WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        };
        for id in [first, second] {
            assert!(is_trusted(&conn, &from(id)).unwrap());
        }
    }
}
