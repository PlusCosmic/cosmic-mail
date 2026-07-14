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
  rfc822_size INTEGER NOT NULL DEFAULT 0,
  body_text TEXT, body_html TEXT,
  body_cached INTEGER NOT NULL DEFAULT 0,
  UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);
CREATE TABLE IF NOT EXISTS attachments (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_index INTEGER NOT NULL,
  filename TEXT NOT NULL DEFAULT '',
  mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
  size_bytes INTEGER NOT NULL DEFAULT 0,
  is_inline INTEGER NOT NULL DEFAULT 0,
  content_id TEXT,
  UNIQUE(message_id, part_index)
);
CREATE INDEX IF NOT EXISTS idx_attachments_message ON attachments(message_id);
"#,
    )
    .context("creating schema")?;

    if !message_column_exists(conn, "rfc822_size")? {
        conn.execute(
            "ALTER TABLE messages ADD COLUMN rfc822_size INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .context("adding message size column")?;
    }
    if !message_column_exists(conn, "body_cached")? {
        conn.execute(
            "ALTER TABLE messages ADD COLUMN body_cached INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .context("adding body cache marker")?;
        conn.execute(
            "UPDATE messages SET body_cached = 1 \
             WHERE body_text IS NOT NULL OR body_html IS NOT NULL",
            [],
        )
        .context("marking existing cached bodies")?;
    }

    init_fts_schema(conn)?;
    Ok(())
}

/// Create the external-content FTS5 index over `messages` and its sync triggers.
///
/// Guarded on the table's existence so this runs once. The index tracks the
/// searchable text columns; `body_text` is NULL until a body is cached, which
/// FTS5 indexes as empty until the `AFTER UPDATE` trigger fires from `set_body`.
/// On first creation, existing rows are backfilled with the FTS5 `rebuild`
/// command. See <https://sqlite.org/fts5.html#external_content_tables>.
fn init_fts_schema(conn: &Connection) -> Result<()> {
    if table_exists(conn, "messages_fts")? {
        return Ok(());
    }
    conn.execute_batch(
        r#"
CREATE VIRTUAL TABLE messages_fts USING fts5(
  subject, from_name, from_addr, snippet, body_text,
  content='messages', content_rowid='id'
);
CREATE TRIGGER messages_fts_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_name, from_addr, snippet, body_text)
  VALUES (new.id, new.subject, new.from_name, new.from_addr, new.snippet, new.body_text);
END;
CREATE TRIGGER messages_fts_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_addr, snippet, body_text)
  VALUES ('delete', old.id, old.subject, old.from_name, old.from_addr, old.snippet, old.body_text);
END;
CREATE TRIGGER messages_fts_au
AFTER UPDATE OF subject, from_name, from_addr, snippet, body_text ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_addr, snippet, body_text)
  VALUES ('delete', old.id, old.subject, old.from_name, old.from_addr, old.snippet, old.body_text);
  INSERT INTO messages_fts(rowid, subject, from_name, from_addr, snippet, body_text)
  VALUES (new.id, new.subject, new.from_name, new.from_addr, new.snippet, new.body_text);
END;
INSERT INTO messages_fts(messages_fts) VALUES ('rebuild');
"#,
    )
    .context("creating fts5 index")?;
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?1",
            params![name],
            |r| r.get(0),
        )
        .context("checking table existence")?;
    Ok(count > 0)
}

fn message_column_exists(conn: &Connection, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(messages)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
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
    pub rfc822_size: u32,
}

/// Trusted metadata used to thread a reply without accepting raw headers from the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyMetadata {
    pub account_id: String,
    pub message_id: Option<String>,
}

/// A body prefetch target from the bounded inbox working set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyPrefetchCandidate {
    pub id: i64,
    pub uid: u32,
    pub rfc822_size: u32,
}

/// Attachment metadata extracted from a parsed message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMeta {
    /// Stable mail-parser part id (position in deterministic parse order).
    pub part_index: u32,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u32,
    pub is_inline: bool,
    /// Content-ID normalized without angle brackets, when present.
    pub content_id: Option<String>,
}

/// An attachment row projected for the reader (non-secret, no bytes).
#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub is_inline: bool,
}

/// Everything needed to refetch and extract a single attachment from the server.
#[derive(Debug, Clone)]
pub struct AttachmentLocation {
    /// Kept for completeness and tests; the save path locates by uid/folder.
    #[allow(dead_code)]
    pub message_id: i64,
    pub part_index: u32,
    pub filename: String,
    pub mime_type: String,
    pub uid: u32,
    pub account_id: String,
    pub folder_name: String,
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

/// Best-effort bump of a target folder's server counts after a message lands in
/// it (e.g. a move/archive). Increments `total_count` by one and, when the
/// message was unseen, `unread_count` by one. The next IMAP STATUS reconciles
/// these authoritative counts.
pub fn increment_folder_counts(conn: &Connection, folder_id: i64, unseen: bool) -> Result<()> {
    conn.execute(
        "UPDATE folders SET total_count = total_count + 1 WHERE id = ?1",
        params![folder_id],
    )
    .context("incrementing folder total count")?;
    if unseen {
        conn.execute(
            "UPDATE folders SET unread_count = unread_count + 1 WHERE id = ?1",
            params![folder_id],
        )
        .context("incrementing folder unread count")?;
    }
    Ok(())
}

/// Find the first folder for an account carrying the given role, if any.
pub fn find_folder_by_role(
    conn: &Connection,
    account_id: &str,
    role: FolderRole,
) -> Result<Option<FolderRow>> {
    conn.query_row(
        "SELECT id, account_id, name, role, unread_count, total_count, uidvalidity, last_seen_uid \
         FROM folders WHERE account_id = ?1 AND role = ?2 ORDER BY id LIMIT 1",
        params![account_id, role.as_str()],
        row_to_folder,
    )
    .optional()
    .context("finding folder by role")
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
             date, snippet, seen, flagged, has_attachments, rfc822_size) \
          VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) \
          ON CONFLICT(folder_id, uid) DO UPDATE SET \
           message_id=excluded.message_id, subject=excluded.subject, \
           from_name=excluded.from_name, from_addr=excluded.from_addr, \
           to_addrs=excluded.to_addrs, cc_addrs=excluded.cc_addrs, date=excluded.date, \
           snippet=excluded.snippet, seen=excluded.seen, flagged=excluded.flagged, \
            has_attachments=excluded.has_attachments, rfc822_size=excluded.rfc822_size",
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
            m.rfc822_size,
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

/// Build a safe FTS5 MATCH expression from raw user input.
///
/// Never pass untrusted text to MATCH directly: bareword operators (`AND`,
/// `OR`, `NEAR`) and punctuation (`-`, `(`, `)`, `"`, `*`, `:`) are FTS5 query
/// syntax and would either change the meaning or raise a syntax error. We split
/// on whitespace, double any embedded `"` to escape it, wrap each token in
/// double quotes so its contents are treated as literal text, and append `*`
/// for prefix matching. Tokens are joined by spaces (implicit AND).
///
/// Returns `None` when the query contains no tokens, so callers can skip the
/// query entirely rather than running an empty MATCH.
fn build_fts_match(query: &str) -> Option<String> {
    let expr = query
        .split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    if expr.is_empty() {
        None
    } else {
        Some(expr)
    }
}

/// Full-text search over cached message envelopes and bodies, relevance-ranked.
///
/// Searches the local cache only (all folders in scope, any role). When
/// `account_id` is `Some`, results are limited to that account; `None` searches
/// every account. An empty/whitespace-only query returns no rows without
/// touching FTS. Rows map to the same shape as [`page_unified_messages`].
pub fn search_messages(
    conn: &Connection,
    query: &str,
    account_id: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<UnifiedMessageRow>> {
    let Some(match_expr) = build_fts_match(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT m.id, m.folder_id, m.uid, m.subject, m.from_name, m.from_addr, m.date, \
                m.snippet, m.seen, m.flagged, m.has_attachments, f.account_id \
         FROM messages_fts \
         JOIN messages m ON m.id = messages_fts.rowid \
         JOIN folders f ON f.id = m.folder_id \
         WHERE messages_fts MATCH ?1 AND (?2 IS NULL OR f.account_id = ?2) \
         ORDER BY rank LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt
        .query_map(params![match_expr, account_id, limit, offset], |r| {
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
        .context("collecting search results")?;
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

/// Everything a message action (flag/move/archive/delete) needs about one row:
/// its source folder, UID, owning account, folder name + role, and seen state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageActionContext {
    pub folder_id: i64,
    pub uid: u32,
    pub account_id: String,
    pub folder_name: String,
    pub folder_role: String,
    pub seen: bool,
}

/// Resolve the action context for a message rowid.
pub fn message_action_context(
    conn: &Connection,
    message_id: i64,
) -> Result<Option<MessageActionContext>> {
    conn.query_row(
        "SELECT m.folder_id, m.uid, f.account_id, f.name, f.role, m.seen \
         FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
        params![message_id],
        |r| {
            Ok(MessageActionContext {
                folder_id: r.get(0)?,
                uid: r.get::<_, i64>(1)? as u32,
                account_id: r.get(2)?,
                folder_name: r.get(3)?,
                folder_role: r.get(4)?,
                seen: r.get::<_, i64>(5)? != 0,
            })
        },
    )
    .optional()
    .context("reading message action context")
}

/// Resolve the account and RFC Message-ID for a local message row.
pub fn get_reply_metadata(
    conn: &Connection,
    local_message_id: i64,
) -> Result<Option<ReplyMetadata>> {
    conn.query_row(
        "SELECT f.account_id, m.message_id \
         FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
        params![local_message_id],
        |r| {
            Ok(ReplyMetadata {
                account_id: r.get(0)?,
                message_id: r.get(1)?,
            })
        },
    )
    .optional()
    .context("reading reply metadata")
}

/// Cached body parts plus recipient lists for a message.
#[derive(Debug, Clone)]
pub struct CachedBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub cached: bool,
}

/// Read cached body columns for a message (body parts may be `None`).
pub fn get_body(conn: &Connection, message_id: i64) -> Result<Option<CachedBody>> {
    conn.query_row(
        "SELECT body_text, body_html, to_addrs, cc_addrs, body_cached \
         FROM messages WHERE id = ?1",
        params![message_id],
        |r| {
            let to_json: String = r.get(2)?;
            let cc_json: String = r.get(3)?;
            Ok(CachedBody {
                text: r.get(0)?,
                html: r.get(1)?,
                to_addrs: serde_json::from_str(&to_json).unwrap_or_default(),
                cc_addrs: serde_json::from_str(&cc_json).unwrap_or_default(),
                cached: r.get::<_, i64>(4)? != 0,
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
    to_addrs: &[String],
    cc_addrs: &[String],
    snippet: Option<&str>,
) -> Result<()> {
    let to_json = serde_json::to_string(to_addrs).unwrap_or_else(|_| "[]".into());
    let cc_json = serde_json::to_string(cc_addrs).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "UPDATE messages SET body_text = ?1, body_html = ?2, to_addrs = ?3, cc_addrs = ?4, \
         snippet = COALESCE(?5, snippet), body_cached = 1 WHERE id = ?6",
        params![text, html, to_json, cc_json, snippet, message_id],
    )
    .context("storing body")?;
    Ok(())
}

/// Replace all attachment rows for a message and reconcile `has_attachments`.
///
/// Runs in one transaction: the previous rows are deleted, the new metadata is
/// inserted, and `messages.has_attachments` is set from the real parse (true iff
/// at least one non-inline attachment exists). Called whenever a body is cached
/// from a full parse, so the envelope BODYSTRUCTURE heuristic is corrected once
/// the body is known. `has_attachments` is not an FTS-indexed column, so this
/// does not touch the search index.
pub fn replace_attachments(
    conn: &Connection,
    message_id: i64,
    attachments: &[AttachmentMeta],
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("beginning attachment transaction")?;
    tx.execute(
        "DELETE FROM attachments WHERE message_id = ?1",
        params![message_id],
    )
    .context("clearing old attachments")?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO attachments \
                   (message_id, part_index, filename, mime_type, size_bytes, is_inline, content_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .context("preparing attachment insert")?;
        for att in attachments {
            stmt.execute(params![
                message_id,
                att.part_index,
                att.filename,
                att.mime_type,
                att.size_bytes,
                att.is_inline as i64,
                att.content_id,
            ])
            .context("inserting attachment")?;
        }
    }
    let non_inline = attachments.iter().filter(|a| !a.is_inline).count();
    tx.execute(
        "UPDATE messages SET has_attachments = ?1 WHERE id = ?2",
        params![(non_inline > 0) as i64, message_id],
    )
    .context("updating has_attachments from parse")?;
    tx.commit().context("committing attachment transaction")?;
    Ok(())
}

/// List an attachment metadata rows for a message, ordered by parse position.
pub fn list_attachments(conn: &Connection, message_id: i64) -> Result<Vec<AttachmentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, filename, mime_type, size_bytes, is_inline \
         FROM attachments WHERE message_id = ?1 ORDER BY part_index",
    )?;
    let rows = stmt
        .query_map(params![message_id], |r| {
            Ok(AttachmentRow {
                id: r.get(0)?,
                filename: r.get(1)?,
                mime_type: r.get(2)?,
                size_bytes: r.get(3)?,
                is_inline: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting attachments")?;
    Ok(rows)
}

/// Resolve an attachment rowid to everything needed to refetch its bytes.
pub fn get_attachment(conn: &Connection, attachment_id: i64) -> Result<Option<AttachmentLocation>> {
    conn.query_row(
        "SELECT a.message_id, a.part_index, a.filename, a.mime_type, m.uid, f.account_id, f.name \
         FROM attachments a \
         JOIN messages m ON m.id = a.message_id \
         JOIN folders f ON f.id = m.folder_id \
         WHERE a.id = ?1",
        params![attachment_id],
        |r| {
            Ok(AttachmentLocation {
                message_id: r.get(0)?,
                part_index: r.get::<_, i64>(1)? as u32,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                uid: r.get::<_, i64>(4)? as u32,
                account_id: r.get(5)?,
                folder_name: r.get(6)?,
            })
        },
    )
    .optional()
    .context("locating attachment")
}

/// Select a bounded batch of uncached bodies from recent and unread inbox messages.
pub fn body_prefetch_candidates(
    conn: &Connection,
    folder_id: i64,
    working_set_limit: u32,
    fetch_limit: u32,
    maximum_size: u32,
) -> Result<Vec<BodyPrefetchCandidate>> {
    let mut stmt = conn.prepare(
        "WITH unread AS ( \
           SELECT id FROM messages WHERE folder_id = ?1 AND seen = 0 \
           ORDER BY date DESC LIMIT ?2 \
         ), recent AS ( \
           SELECT id FROM messages WHERE folder_id = ?1 ORDER BY date DESC LIMIT ?2 \
         ), working AS (SELECT id FROM unread UNION SELECT id FROM recent) \
         SELECT m.id, m.uid, m.rfc822_size FROM messages m \
         JOIN working w ON w.id = m.id \
         WHERE m.body_cached = 0 AND (m.rfc822_size = 0 OR m.rfc822_size <= ?3) \
         ORDER BY m.seen ASC, m.date DESC LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            params![folder_id, working_set_limit, maximum_size, fetch_limit],
            |r| {
                Ok(BodyPrefetchCandidate {
                    id: r.get(0)?,
                    uid: r.get::<_, i64>(1)? as u32,
                    rfc822_size: r.get::<_, i64>(2)? as u32,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting body prefetch candidates")?;
    Ok(rows)
}

/// Store a size discovered separately for a pre-migration envelope.
pub fn set_message_size(conn: &Connection, message_id: i64, size: u32) -> Result<()> {
    conn.execute(
        "UPDATE messages SET rfc822_size = ?1 WHERE id = ?2",
        params![size, message_id],
    )
    .context("storing message size")?;
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

/// Update the flagged (`\Flagged`) state for a message in the local cache.
///
/// Returns whether the cached value actually changed.
pub fn set_flagged(conn: &Connection, message_id: i64, flagged: bool) -> Result<bool> {
    let changed = conn
        .execute(
            "UPDATE messages SET flagged = ?1 WHERE id = ?2 AND flagged != ?1",
            params![flagged as i64, message_id],
        )
        .context("setting flagged")?;
    Ok(changed > 0)
}

/// Remove a message row from the local cache and adjust its source folder's
/// server-authoritative counts (`total_count -= 1`, and `unread_count -= 1`
/// when the message was unseen, both floored at 0).
///
/// The FTS5 delete trigger and the `attachments` foreign-key cascade clean up
/// the derived rows. The counts are best-effort until the next IMAP STATUS, in
/// the same spirit as [`adjust_folder_unread_count`]. A no-op if the row is gone.
pub fn remove_message(conn: &Connection, message_id: i64) -> Result<()> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT folder_id, seen FROM messages WHERE id = ?1",
            params![message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .context("looking up message before removal")?;
    let Some((folder_id, seen)) = existing else {
        return Ok(());
    };
    conn.execute("DELETE FROM messages WHERE id = ?1", params![message_id])
        .context("removing message")?;
    conn.execute(
        "UPDATE folders SET total_count = MAX(0, total_count - 1) WHERE id = ?1",
        params![folder_id],
    )
    .context("decrementing folder total count")?;
    if seen == 0 {
        conn.execute(
            "UPDATE folders SET unread_count = MAX(0, unread_count - 1) WHERE id = ?1",
            params![folder_id],
        )
        .context("decrementing folder unread count")?;
    }
    Ok(())
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
                rfc822_size: 100,
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

    #[test]
    fn reply_metadata_keeps_account_and_rfc_message_id() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "account", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        let local_id = upsert_message(
            &conn,
            &MessageUpsert {
                folder_id,
                uid: 42,
                message_id: Some("<parent@example.com>".into()),
                subject: String::new(),
                from_name: String::new(),
                from_addr: "sender@example.com".into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: "2026-07-12T00:00:00Z".into(),
                snippet: String::new(),
                seen: true,
                flagged: false,
                has_attachments: false,
                rfc822_size: 100,
            },
        )
        .expect("insert message");

        assert_eq!(
            get_reply_metadata(&conn, local_id).expect("read metadata"),
            Some(ReplyMetadata {
                account_id: "account".into(),
                message_id: Some("<parent@example.com>".into()),
            })
        );
    }

    #[test]
    fn body_prefetch_is_bounded_prioritized_and_tracks_empty_bodies() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "account", "INBOX", FolderRole::Inbox, 1).expect("insert folder");

        let mut ids = Vec::new();
        for uid in 1..=6 {
            let id = upsert_message(
                &conn,
                &MessageUpsert {
                    folder_id,
                    uid,
                    message_id: None,
                    subject: format!("message {uid}"),
                    from_name: String::new(),
                    from_addr: "sender@example.com".into(),
                    to_addrs: Vec::new(),
                    cc_addrs: Vec::new(),
                    date: format!("2026-07-12T00:00:0{uid}Z"),
                    snippet: String::new(),
                    seen: uid != 2,
                    flagged: false,
                    has_attachments: false,
                    rfc822_size: if uid == 6 { 2_000_000 } else { 100 },
                },
            )
            .expect("insert message");
            ids.push(id);
        }

        let candidates =
            body_prefetch_candidates(&conn, folder_id, 5, 3, 1_048_576).expect("select candidates");
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].uid, 2, "unread messages come first");
        assert!(candidates.iter().all(|candidate| candidate.uid != 6));

        let empty_id = ids[1];
        set_body(&conn, empty_id, None, None, &[], &[], None).expect("cache empty body");
        let cached = get_body(&conn, empty_id)
            .expect("read body")
            .expect("message exists");
        assert!(cached.cached);
        assert!(cached.text.is_none());
        assert!(cached.html.is_none());
        let candidates = body_prefetch_candidates(&conn, folder_id, 5, 5, 1_048_576)
            .expect("select candidates again");
        assert!(candidates.iter().all(|candidate| candidate.id != empty_id));
    }

    #[test]
    fn replace_attachments_reconciles_has_attachments_and_lists_and_locates() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        let message_id = upsert_message(
            &conn,
            &MessageUpsert {
                folder_id,
                uid: 5,
                message_id: None,
                subject: "s".into(),
                from_name: String::new(),
                from_addr: "a@b.com".into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: "2026-07-14T00:00:00Z".into(),
                snippet: String::new(),
                seen: true,
                flagged: false,
                // Envelope heuristic guessed no attachment.
                has_attachments: false,
                rfc822_size: 100,
            },
        )
        .expect("insert message");

        let metas = vec![
            AttachmentMeta {
                part_index: 2,
                filename: "report.pdf".into(),
                mime_type: "application/pdf".into(),
                size_bytes: 1024,
                is_inline: false,
                content_id: None,
            },
            AttachmentMeta {
                part_index: 3,
                filename: String::new(),
                mime_type: "image/png".into(),
                size_bytes: 200,
                is_inline: true,
                content_id: Some("logo@x".into()),
            },
        ];
        replace_attachments(&conn, message_id, &metas).expect("store attachments");

        // has_attachments becomes true from the real parse (one non-inline part).
        let has: i64 = conn
            .query_row(
                "SELECT has_attachments FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .expect("read has_attachments");
        assert_eq!(has, 1);

        let listed = list_attachments(&conn, message_id).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].filename, "report.pdf");
        assert_eq!(listed[0].mime_type, "application/pdf");
        assert_eq!(listed[0].size_bytes, 1024);
        assert!(!listed[0].is_inline);
        assert!(listed[1].is_inline);

        // get_attachment joins through to the owning message/folder/account.
        let loc = get_attachment(&conn, listed[0].id)
            .expect("get")
            .expect("attachment exists");
        assert_eq!(loc.message_id, message_id);
        assert_eq!(loc.part_index, 2);
        assert_eq!(loc.uid, 5);
        assert_eq!(loc.account_id, "acct");
        assert_eq!(loc.folder_name, "INBOX");

        // Replacing with only inline attachments flips has_attachments back off.
        replace_attachments(
            &conn,
            message_id,
            &[AttachmentMeta {
                part_index: 1,
                filename: String::new(),
                mime_type: "image/gif".into(),
                size_bytes: 10,
                is_inline: true,
                content_id: Some("only-inline@x".into()),
            }],
        )
        .expect("replace attachments");
        let has: i64 = conn
            .query_row(
                "SELECT has_attachments FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .expect("read has_attachments");
        assert_eq!(has, 0);
        assert_eq!(list_attachments(&conn, message_id).expect("list").len(), 1);

        // Deleting the message cascades to its attachment rows.
        conn.execute("DELETE FROM messages WHERE id = ?1", params![message_id])
            .expect("delete message");
        assert!(list_attachments(&conn, message_id)
            .expect("list")
            .is_empty());
    }

    /// Pins the capability the search feature depends on: the bundled SQLite must
    /// be compiled with `SQLITE_ENABLE_FTS5`. If this fails, the bundled build no
    /// longer enables FTS5 and search needs a rusqlite feature flag.
    #[test]
    fn fts5_is_compiled_into_bundled_sqlite() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        conn.execute_batch("CREATE VIRTUAL TABLE probe USING fts5(body);")
            .expect("bundled sqlite must expose FTS5");
    }

    #[test]
    fn set_flagged_reports_changes_and_persists() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        let id = insert_searchable(&conn, folder_id, 1, "s", "f", "a@b.com", "");

        assert!(set_flagged(&conn, id, true).expect("flag"));
        // Idempotent: no change on a repeat.
        assert!(!set_flagged(&conn, id, true).expect("flag again"));
        let flagged: i64 = conn
            .query_row(
                "SELECT flagged FROM messages WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .expect("read flagged");
        assert_eq!(flagged, 1);
        assert!(set_flagged(&conn, id, false).expect("unflag"));
    }

    #[test]
    fn remove_message_adjusts_counts_drops_fts_and_cascades_attachments() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .expect("enable foreign keys");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        set_folder_counts(&conn, folder_id, 2, 1).expect("set server counts");

        // One seen message and one unseen message.
        let seen_id = insert_searchable(&conn, folder_id, 1, "alpha", "f", "a@b.com", "");
        let unseen_id = upsert_message(
            &conn,
            &MessageUpsert {
                folder_id,
                uid: 2,
                message_id: None,
                subject: "beta unique".into(),
                from_name: String::new(),
                from_addr: "c@d.com".into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: "2026-07-14T00:00:02Z".into(),
                snippet: String::new(),
                seen: false,
                flagged: false,
                has_attachments: false,
                rfc822_size: 100,
            },
        )
        .expect("insert unseen message");

        // Attach a row so we can verify the cascade.
        replace_attachments(
            &conn,
            seen_id,
            &[AttachmentMeta {
                part_index: 2,
                filename: "a.pdf".into(),
                mime_type: "application/pdf".into(),
                size_bytes: 10,
                is_inline: false,
                content_id: None,
            }],
        )
        .expect("attach");

        // Removing the seen message: total down by one, unread unchanged.
        remove_message(&conn, seen_id).expect("remove seen");
        let folder = get_folder(&conn, folder_id)
            .expect("read folder")
            .expect("folder exists");
        assert_eq!(folder.total_count, 1);
        assert_eq!(folder.unread_count, 1);
        // FTS row gone: the subject no longer matches.
        assert!(search_messages(&conn, "alpha", None, 0, 50)
            .expect("search")
            .is_empty());
        // Attachment cascade fired.
        assert!(list_attachments(&conn, seen_id).expect("list").is_empty());

        // Removing the unseen message: both counts drop by one.
        remove_message(&conn, unseen_id).expect("remove unseen");
        let folder = get_folder(&conn, folder_id)
            .expect("read folder")
            .expect("folder exists");
        assert_eq!(folder.total_count, 0);
        assert_eq!(folder.unread_count, 0);

        // Removing a nonexistent row is a no-op and floors at zero.
        remove_message(&conn, 99_999).expect("remove missing");
        let folder = get_folder(&conn, folder_id)
            .expect("read folder")
            .expect("folder exists");
        assert_eq!(folder.total_count, 0);
        assert_eq!(folder.unread_count, 0);
    }

    #[test]
    fn increment_folder_counts_tracks_unseen() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "Archive", FolderRole::Archive, 1).expect("insert folder");
        set_folder_counts(&conn, folder_id, 5, 0).expect("set counts");

        increment_folder_counts(&conn, folder_id, false).expect("add seen");
        let folder = get_folder(&conn, folder_id).unwrap().unwrap();
        assert_eq!(folder.total_count, 6);
        assert_eq!(folder.unread_count, 0);

        increment_folder_counts(&conn, folder_id, true).expect("add unseen");
        let folder = get_folder(&conn, folder_id).unwrap().unwrap();
        assert_eq!(folder.total_count, 7);
        assert_eq!(folder.unread_count, 1);
    }

    #[test]
    fn find_folder_by_role_and_action_context_resolve() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (inbox_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("inbox");
        let (trash_id, _) =
            upsert_folder(&conn, "acct", "Trash", FolderRole::Trash, 1).expect("trash");
        // A different account's archive must not be resolved for "acct".
        upsert_folder(&conn, "other", "All Mail", FolderRole::Archive, 1).expect("other archive");

        assert_eq!(
            find_folder_by_role(&conn, "acct", FolderRole::Trash)
                .expect("find trash")
                .map(|f| f.id),
            Some(trash_id)
        );
        assert!(find_folder_by_role(&conn, "acct", FolderRole::Archive)
            .expect("find archive")
            .is_none());

        let msg_id = upsert_message(
            &conn,
            &MessageUpsert {
                folder_id: inbox_id,
                uid: 7,
                message_id: None,
                subject: "s".into(),
                from_name: String::new(),
                from_addr: "a@b.com".into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: "2026-07-14T00:00:00Z".into(),
                snippet: String::new(),
                seen: false,
                flagged: false,
                has_attachments: false,
                rfc822_size: 100,
            },
        )
        .expect("insert message");

        let ctx = message_action_context(&conn, msg_id)
            .expect("context")
            .expect("message exists");
        assert_eq!(
            ctx,
            MessageActionContext {
                folder_id: inbox_id,
                uid: 7,
                account_id: "acct".into(),
                folder_name: "INBOX".into(),
                folder_role: "inbox".into(),
                seen: false,
            }
        );
        assert!(message_action_context(&conn, 12_345)
            .expect("missing context")
            .is_none());
    }

    #[test]
    fn fts_query_builder_escapes_and_prefixes() {
        assert_eq!(build_fts_match("   "), None);
        assert_eq!(build_fts_match(""), None);
        assert_eq!(build_fts_match("hello"), Some("\"hello\"*".into()));
        assert_eq!(
            build_fts_match("hello world"),
            Some("\"hello\"* \"world\"*".into()),
        );
        // Embedded double quotes are doubled so they stay literal inside the token.
        assert_eq!(build_fts_match("a\"b"), Some("\"a\"\"b\"*".into()));
    }

    fn insert_searchable(
        conn: &Connection,
        folder_id: i64,
        uid: u32,
        subject: &str,
        from_name: &str,
        from_addr: &str,
        snippet: &str,
    ) -> i64 {
        upsert_message(
            conn,
            &MessageUpsert {
                folder_id,
                uid,
                message_id: None,
                subject: subject.into(),
                from_name: from_name.into(),
                from_addr: from_addr.into(),
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                date: format!("2026-07-12T00:00:{uid:02}Z"),
                snippet: snippet.into(),
                seen: true,
                flagged: false,
                has_attachments: false,
                rfc822_size: 100,
            },
        )
        .expect("insert message")
    }

    #[test]
    fn adversarial_queries_never_raise_fts_syntax_errors() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        insert_searchable(
            &conn,
            folder_id,
            1,
            "Quarterly report",
            "Finance",
            "finance@example.com",
            "numbers inside",
        );

        // Each of these embeds FTS5 query operators / punctuation as literal text.
        for q in [
            "AND",
            "OR",
            "NEAR",
            "report AND finance",
            "foo-bar",
            "(unbalanced",
            "a\"b",
            "*",
            "col:val",
            "^caret",
            "報告",
        ] {
            let hits = search_messages(&conn, q, None, 0, 50)
                .unwrap_or_else(|e| panic!("query {q:?} must not error: {e}"));
            // We only assert it does not raise a syntax error; matches are optional.
            let _ = hits;
        }
    }

    #[test]
    fn search_matches_each_indexed_column_and_prefixes() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        let id = insert_searchable(
            &conn,
            folder_id,
            1,
            "Invoice attached",
            "Alice Merchant",
            "billing@shop.example",
            "your receipt is ready",
        );

        // Subject, from_name, from_addr, and snippet are all searchable.
        for q in ["invoice", "merchant", "shop.example", "receipt"] {
            let hits = search_messages(&conn, q, None, 0, 50).expect("search");
            assert_eq!(hits.len(), 1, "query {q:?} should match");
            assert_eq!(hits[0].msg.id, id);
        }

        // Prefix matching: a partial token still matches.
        let hits = search_messages(&conn, "invo", None, 0, 50).expect("prefix search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].msg.id, id);

        // A token that appears nowhere returns nothing.
        assert!(search_messages(&conn, "zzznomatch", None, 0, 50)
            .expect("search")
            .is_empty());
    }

    #[test]
    fn search_indexes_body_text_after_set_body() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_id, _) =
            upsert_folder(&conn, "acct", "INBOX", FolderRole::Inbox, 1).expect("insert folder");
        let id = insert_searchable(&conn, folder_id, 1, "Meeting", "Bob", "bob@example.com", "");

        // Body is not cached yet — a body-only term does not match.
        assert!(search_messages(&conn, "asparagus", None, 0, 50)
            .expect("search")
            .is_empty());

        // Caching the body fires the AFTER UPDATE trigger and indexes body_text.
        set_body(
            &conn,
            id,
            Some("the asparagus harvest was excellent"),
            None,
            &[],
            &[],
            Some("the asparagus harvest"),
        )
        .expect("set body");

        let hits = search_messages(&conn, "asparagus", None, 0, 50).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].msg.id, id);
    }

    #[test]
    fn search_scopes_to_account_and_drops_deleted_rows() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize schema");
        let (folder_a, _) =
            upsert_folder(&conn, "acct-a", "INBOX", FolderRole::Inbox, 1).expect("folder a");
        let (folder_b, _) =
            upsert_folder(&conn, "acct-b", "INBOX", FolderRole::Inbox, 1).expect("folder b");
        insert_searchable(&conn, folder_a, 1, "shared keyword", "A", "a@a.example", "");
        insert_searchable(&conn, folder_b, 1, "shared keyword", "B", "b@b.example", "");

        // Unscoped search sees both accounts.
        assert_eq!(
            search_messages(&conn, "shared", None, 0, 50)
                .expect("search")
                .len(),
            2
        );
        // Scoped search sees only the requested account.
        let scoped = search_messages(&conn, "shared", Some("acct-a"), 0, 50).expect("search");
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].account_id, "acct-a");

        // Deleting an account's cached rows removes them from the index (AFTER DELETE).
        delete_account_data(&conn, "acct-a").expect("delete account data");
        let after = search_messages(&conn, "shared", None, 0, 50).expect("search");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].account_id, "acct-b");
    }
}
