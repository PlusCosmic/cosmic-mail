//! SQLite persistence: schema, migrations, and query helpers.
//!
//! The database lives at `$XDG_DATA_HOME/cosmic-mail/mail.db`. A single
//! [`rusqlite::Connection`] is shared across threads behind a mutex (see
//! [`crate::state`]); this is adequate for the current scale.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Folder role derived from IMAP SPECIAL-USE attributes or name heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Archive,
    Spam,
    Normal,
}

impl FolderRole {
    /// Wire/DB string form.
    pub fn as_str(self) -> &'static str {
        match self {
            FolderRole::Inbox => "inbox",
            FolderRole::Sent => "sent",
            FolderRole::Drafts => "drafts",
            FolderRole::Trash => "trash",
            FolderRole::Archive => "archive",
            FolderRole::Spam => "spam",
            FolderRole::Normal => "normal",
        }
    }
}

/// Path to the SQLite database file.
pub fn db_path() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .context("could not determine data dir (XDG_DATA_HOME)")?
        .join("cosmic-mail");
    Ok(dir.join("mail.db"))
}

/// Open (creating if needed) the database and apply the schema.
pub fn open() -> Result<Connection> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let conn = Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;",
    )
    .context("setting pragmas")?;
    init_schema(&conn)?;
    Ok(conn)
}

/// Create tables and indexes if they do not exist.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'normal',
  uidvalidity INTEGER NOT NULL DEFAULT 0,
  last_seen_uid INTEGER NOT NULL DEFAULT 0,
  unread_count INTEGER NOT NULL DEFAULT 0,
  total_count INTEGER NOT NULL DEFAULT 0,
  UNIQUE(account_id, name)
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  message_id TEXT,
  subject TEXT NOT NULL DEFAULT '',
  from_name TEXT NOT NULL DEFAULT '',
  from_addr TEXT NOT NULL DEFAULT '',
  to_addrs TEXT NOT NULL DEFAULT '[]',
  cc_addrs TEXT NOT NULL DEFAULT '[]',
  date TEXT NOT NULL,
  snippet TEXT NOT NULL DEFAULT '',
  seen INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  body_text TEXT, body_html TEXT,
  UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);
"#,
    )
    .context("creating schema")?;
    Ok(())
}

// --- Row projections ---------------------------------------------------------

/// A folder row as returned to the frontend.
#[derive(Debug, Clone)]
pub struct FolderRow {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub role: String,
    pub unread_count: i64,
    pub total_count: i64,
    /// Kept for completeness; reconciliation reads it directly in `upsert_folder`.
    #[allow(dead_code)]
    pub uidvalidity: i64,
    pub last_seen_uid: i64,
}

/// A message summary row.
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub folder_id: i64,
    pub uid: i64,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub date: String,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
}

/// Data required to upsert a message envelope.
#[derive(Debug, Clone)]
pub struct MessageUpsert {
    pub folder_id: i64,
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub date: String,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
}

// --- Folder queries ----------------------------------------------------------

/// Insert or update a folder for `(account_id, name)`, reconciling UIDVALIDITY.
///
/// If the stored UIDVALIDITY differs from the server's, the folder's cached
/// messages are wiped (returns `true` in that case) and counts are reset so the
/// next sync repopulates from scratch. Returns the folder rowid.
pub fn upsert_folder(
    conn: &Connection,
    account_id: &str,
    name: &str,
    role: FolderRole,
    uidvalidity: u32,
) -> Result<(i64, bool)> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, uidvalidity FROM folders WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .context("looking up folder")?;

    match existing {
        Some((id, stored_uidvalidity)) => {
            let mut wiped = false;
            if stored_uidvalidity != uidvalidity as i64 && uidvalidity != 0 {
                // UIDVALIDITY changed: invalidate the cache for this folder.
                conn.execute("DELETE FROM messages WHERE folder_id = ?1", params![id])
                    .context("wiping messages on uidvalidity change")?;
                conn.execute(
                    "UPDATE folders SET uidvalidity = ?1, last_seen_uid = 0, \
                     unread_count = 0, total_count = 0, role = ?2 WHERE id = ?3",
                    params![uidvalidity, role.as_str(), id],
                )
                .context("resetting folder on uidvalidity change")?;
                wiped = true;
            } else {
                // Keep role up to date.
                conn.execute(
                    "UPDATE folders SET role = ?1 WHERE id = ?2",
                    params![role.as_str(), id],
                )
                .context("updating folder role")?;
            }
            Ok((id, wiped))
        }
        None => {
            conn.execute(
                "INSERT INTO folders (account_id, name, role, uidvalidity) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![account_id, name, role.as_str(), uidvalidity],
            )
            .context("inserting folder")?;
            Ok((conn.last_insert_rowid(), false))
        }
    }
}

/// List all folders for an account.
pub fn list_folders(conn: &Connection, account_id: &str) -> Result<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, name, role, unread_count, total_count, uidvalidity, last_seen_uid \
         FROM folders WHERE account_id = ?1 ORDER BY \
         CASE role WHEN 'inbox' THEN 0 WHEN 'sent' THEN 1 WHEN 'drafts' THEN 2 \
         WHEN 'archive' THEN 3 WHEN 'spam' THEN 4 WHEN 'trash' THEN 5 ELSE 6 END, name",
    )?;
    let rows = stmt
        .query_map(params![account_id], row_to_folder)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting folders")?;
    Ok(rows)
}

/// Fetch a single folder by rowid.
pub fn get_folder(conn: &Connection, folder_id: i64) -> Result<Option<FolderRow>> {
    conn.query_row(
        "SELECT id, account_id, name, role, unread_count, total_count, uidvalidity, last_seen_uid \
         FROM folders WHERE id = ?1",
        params![folder_id],
        row_to_folder,
    )
    .optional()
    .context("fetching folder")
}

fn row_to_folder(r: &rusqlite::Row<'_>) -> rusqlite::Result<FolderRow> {
    Ok(FolderRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        name: r.get(2)?,
        role: r.get(3)?,
        unread_count: r.get(4)?,
        total_count: r.get(5)?,
        uidvalidity: r.get(6)?,
        last_seen_uid: r.get(7)?,
    })
}

/// Update the notification high-water mark for a folder.
pub fn set_last_seen_uid(conn: &Connection, folder_id: i64, uid: u32) -> Result<()> {
    conn.execute(
        "UPDATE folders SET last_seen_uid = ?1 WHERE id = ?2 AND last_seen_uid < ?1",
        params![uid, folder_id],
    )
    .context("updating last_seen_uid")?;
    Ok(())
}

/// Persist authoritative server counts returned by IMAP STATUS.
pub fn set_folder_counts(
    conn: &Connection,
    folder_id: i64,
    total_count: u32,
    unread_count: u32,
) -> Result<()> {
    conn.execute(
        "UPDATE folders SET total_count = ?1, unread_count = ?2 WHERE id = ?3",
        params![total_count, unread_count, folder_id],
    )
    .context("updating server folder counts")?;
    Ok(())
}

/// Adjust the authoritative unread count after a successful local flag change.
pub fn adjust_folder_unread_count(conn: &Connection, folder_id: i64, delta: i64) -> Result<()> {
    conn.execute(
        "UPDATE folders SET unread_count = MAX(0, unread_count + ?1) WHERE id = ?2",
        params![delta, folder_id],
    )
    .context("adjusting folder unread count")?;
    Ok(())
}

// --- Message queries ---------------------------------------------------------

/// Insert or update a message by `(folder_id, uid)`. Preserves already-cached
/// body columns. Returns the message rowid.
pub fn upsert_message(conn: &Connection, m: &MessageUpsert) -> Result<i64> {
    let to_json = serde_json::to_string(&m.to_addrs).unwrap_or_else(|_| "[]".into());
    let cc_json = serde_json::to_string(&m.cc_addrs).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO messages \
           (folder_id, uid, message_id, subject, from_name, from_addr, to_addrs, cc_addrs, \
            date, snippet, seen, flagged, has_attachments) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) \
         ON CONFLICT(folder_id, uid) DO UPDATE SET \
           message_id=excluded.message_id, subject=excluded.subject, \
           from_name=excluded.from_name, from_addr=excluded.from_addr, \
           to_addrs=excluded.to_addrs, cc_addrs=excluded.cc_addrs, date=excluded.date, \
           snippet=excluded.snippet, seen=excluded.seen, flagged=excluded.flagged, \
           has_attachments=excluded.has_attachments",
        params![
            m.folder_id,
            m.uid,
            m.message_id,
            m.subject,
            m.from_name,
            m.from_addr,
            to_json,
            cc_json,
            m.date,
            m.snippet,
            m.seen as i64,
            m.flagged as i64,
            m.has_attachments as i64,
        ],
    )
    .context("upserting message")?;

    let id: i64 = conn
        .query_row(
            "SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2",
            params![m.folder_id, m.uid],
            |r| r.get(0),
        )
        .context("reading message rowid")?;
    Ok(id)
}

/// A message row from the unified inbox query (includes account_id from folders join).
#[derive(Debug, Clone)]
pub struct UnifiedMessageRow {
    pub msg: MessageRow,
    pub account_id: String,
}

/// Page messages across all inbox folders for all accounts, newest first.
pub fn page_unified_messages(
    conn: &Connection,
    offset: i64,
    limit: i64,
) -> Result<Vec<UnifiedMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.folder_id, m.uid, m.subject, m.from_name, m.from_addr, m.date, \
                m.snippet, m.seen, m.flagged, m.has_attachments, f.account_id \
         FROM messages m \
         JOIN folders f ON f.id = m.folder_id \
         WHERE f.role = 'inbox' \
         ORDER BY m.date DESC LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit, offset], |r| {
            Ok(UnifiedMessageRow {
                msg: MessageRow {
                    id: r.get(0)?,
                    folder_id: r.get(1)?,
                    uid: r.get(2)?,
                    subject: r.get(3)?,
                    from_name: r.get(4)?,
                    from_addr: r.get(5)?,
                    date: r.get(6)?,
                    snippet: r.get(7)?,
                    seen: r.get::<_, i64>(8)? != 0,
                    flagged: r.get::<_, i64>(9)? != 0,
                    has_attachments: r.get::<_, i64>(10)? != 0,
                },
                account_id: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting unified messages")?;
    Ok(rows)
}

/// Page messages for a folder, newest first.
pub fn page_messages(
    conn: &Connection,
    folder_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<MessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, folder_id, uid, subject, from_name, from_addr, date, snippet, \
                seen, flagged, has_attachments \
         FROM messages WHERE folder_id = ?1 ORDER BY date DESC LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt
        .query_map(params![folder_id, limit, offset], row_to_message)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting messages")?;
    Ok(rows)
}

fn row_to_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        uid: r.get(2)?,
        subject: r.get(3)?,
        from_name: r.get(4)?,
        from_addr: r.get(5)?,
        date: r.get(6)?,
        snippet: r.get(7)?,
        seen: r.get::<_, i64>(8)? != 0,
        flagged: r.get::<_, i64>(9)? != 0,
        has_attachments: r.get::<_, i64>(10)? != 0,
    })
}

/// Locate a message by rowid, returning `(folder_id, uid, account_id, folder_name)`.
pub fn locate_message(
    conn: &Connection,
    message_id: i64,
) -> Result<Option<(i64, u32, String, String)>> {
    conn.query_row(
        "SELECT m.folder_id, m.uid, f.account_id, f.name \
         FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
        params![message_id],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)? as u32,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .context("locating message")
}

/// Cached body parts plus recipient lists for a message.
#[derive(Debug, Clone)]
pub struct CachedBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
}

/// Read cached body columns for a message (body parts may be `None`).
pub fn get_body(conn: &Connection, message_id: i64) -> Result<Option<CachedBody>> {
    conn.query_row(
        "SELECT body_text, body_html, to_addrs, cc_addrs FROM messages WHERE id = ?1",
        params![message_id],
        |r| {
            let to_json: String = r.get(2)?;
            let cc_json: String = r.get(3)?;
            Ok(CachedBody {
                text: r.get(0)?,
                html: r.get(1)?,
                to_addrs: serde_json::from_str(&to_json).unwrap_or_default(),
                cc_addrs: serde_json::from_str(&cc_json).unwrap_or_default(),
            })
        },
    )
    .optional()
    .context("reading cached body")
}

/// Persist fetched body parts for a message.
pub fn set_body(
    conn: &Connection,
    message_id: i64,
    text: Option<&str>,
    html: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE messages SET body_text = ?1, body_html = ?2 WHERE id = ?3",
        params![text, html, message_id],
    )
    .context("storing body")?;
    Ok(())
}

/// Update the seen flag for a message in the local cache.
///
/// Returns whether the cached value actually changed.
pub fn mark_seen(conn: &Connection, message_id: i64, seen: bool) -> Result<bool> {
    let changed = conn
        .execute(
            "UPDATE messages SET seen = ?1 WHERE id = ?2 AND seen != ?1",
            params![seen as i64, message_id],
        )
        .context("marking seen")?;
    Ok(changed > 0)
}

/// Highest UID currently cached for a folder (0 if empty).
pub fn max_uid(conn: &Connection, folder_id: i64) -> Result<u32> {
    let v: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(uid), 0) FROM messages WHERE folder_id = ?1",
            params![folder_id],
            |r| r.get(0),
        )
        .context("computing max uid")?;
    Ok(v as u32)
}

/// Delete cached messages and folders for an account (used on account removal).
pub fn delete_account_data(conn: &Connection, account_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM messages WHERE folder_id IN \
         (SELECT id FROM folders WHERE account_id = ?1)",
        params![account_id],
    )
    .context("deleting account messages")?;
    conn.execute(
        "DELETE FROM folders WHERE account_id = ?1",
        params![account_id],
    )
    .context("deleting account folders")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_counts_are_independent_of_cached_message_count() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "account", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        set_folder_counts(&conn, folder_id, 1_497, 12).expect("set server counts");

        let message_id = upsert_message(
            &conn,
            &MessageUpsert {
                folder_id,
                uid: 1_690,
                message_id: None,
                subject: "test".into(),
                from_name: "Sender".into(),
                from_addr: "sender@example.com".into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: "2026-07-10T00:00:00Z".into(),
                snippet: String::new(),
                seen: true,
                flagged: false,
                has_attachments: false,
            },
        )
        .expect("cache one message");

        assert!(mark_seen(&conn, message_id, false).expect("mark unread"));
        adjust_folder_unread_count(&conn, folder_id, 1).expect("increment unread count");
        let folder = get_folder(&conn, folder_id)
            .expect("read folder")
            .expect("folder exists");
        assert_eq!(folder.total_count, 1_497);
        assert_eq!(folder.unread_count, 13);

        assert!(mark_seen(&conn, message_id, true).expect("mark read"));
        adjust_folder_unread_count(&conn, folder_id, -1).expect("decrement unread count");
        assert!(!mark_seen(&conn, message_id, true).expect("repeat mark read"));
        let folder = get_folder(&conn, folder_id)
            .expect("read folder")
            .expect("folder exists");
        assert_eq!(folder.total_count, 1_497);
        assert_eq!(folder.unread_count, 12);
    }
}
