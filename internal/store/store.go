// Package store is the SQLite persistence layer: schema, migrations, and
// query helpers.
//
// The database lives at `$XDG_DATA_HOME/cosmic-mail/mail.db`. A single
// connection is shared across goroutines behind a mutex (the Rust build used
// `Arc<Mutex<Connection>>`); modernc.org/sqlite is pure Go, so the only cgo
// in the binary stays WebKit's.
package store

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite"

	"cosmicmail/internal/xdg"
)

// FolderRole is derived from IMAP SPECIAL-USE attributes or name heuristics.
type FolderRole string

const (
	RoleInbox   FolderRole = "inbox"
	RoleSent    FolderRole = "sent"
	RoleDrafts  FolderRole = "drafts"
	RoleTrash   FolderRole = "trash"
	RoleArchive FolderRole = "archive"
	RoleSpam    FolderRole = "spam"
	RoleNormal  FolderRole = "normal"
)

// RFC3339 formats a time the way chrono's `to_rfc3339` did in the Rust build
// (always a numeric offset, so "+00:00" rather than "Z"), keeping new rows
// sortable next to the ones already on disk.
func RFC3339(t time.Time) string { return t.Format("2006-01-02T15:04:05-07:00") }

// Store owns the single connection. Every method takes the mutex, and the
// closure-based helpers below let callers run several statements under one
// hold, as the Rust `conn.lock()` scopes did.
type Store struct {
	mu sync.Mutex
	db *sql.DB
}

// DBPath is the SQLite database file.
func DBPath() (string, error) {
	dir, err := xdg.AppDataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "mail.db"), nil
}

// Open opens (creating if needed) the database and applies the schema.
func Open() (*Store, error) {
	path, err := DBPath()
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("creating %s: %w", filepath.Dir(path), err)
	}
	return OpenPath(path)
}

// DSN builds the SQLite URI for a filesystem path (":memory:" for tests).
// The path is percent-escaped so a `?` or `#` in $XDG_DATA_HOME cannot be
// read as the start of the query string.
func DSN(path string) string {
	if path == ":memory:" {
		return "file::memory:?_pragma=foreign_keys(ON)"
	}
	escaped := (&url.URL{Path: path}).EscapedPath()
	return "file:" + escaped + "?_pragma=journal_mode(WAL)&_pragma=foreign_keys(ON)&_pragma=busy_timeout(5000)"
}

// OpenPath opens the database at path (":memory:" for tests).
func OpenPath(path string) (*Store, error) {
	dsn := DSN(path)
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("opening %s: %w", path, err)
	}
	// One connection: pragmas are per-connection, and the mutex above
	// serialises access anyway.
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxLifetime(0)
	s := &Store{db: db}
	if err := s.initSchema(); err != nil {
		db.Close()
		return nil, err
	}
	return s, nil
}

// Close closes the connection.
func (s *Store) Close() error { return s.db.Close() }

// Lock takes the store mutex for a multi-statement critical section. Use it
// exactly where the Rust code held `conn.lock()` across several calls.
func (s *Store) Lock() func() {
	s.mu.Lock()
	return s.mu.Unlock
}

// The *Locked variants below are the raw operations; the exported wrappers
// take the mutex. Keeping both lets the sync loop hold the lock across a
// batch of upserts without re-entering it.

func (s *Store) initSchema() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := s.db.Exec(schemaSQL); err != nil {
		return fmt.Errorf("creating schema: %w", err)
	}
	if ok, err := s.messageColumnExists("rfc822_size"); err != nil {
		return err
	} else if !ok {
		if _, err := s.db.Exec("ALTER TABLE messages ADD COLUMN rfc822_size INTEGER NOT NULL DEFAULT 0"); err != nil {
			return fmt.Errorf("adding message size column: %w", err)
		}
	}
	if ok, err := s.messageColumnExists("body_cached"); err != nil {
		return err
	} else if !ok {
		if _, err := s.db.Exec("ALTER TABLE messages ADD COLUMN body_cached INTEGER NOT NULL DEFAULT 0"); err != nil {
			return fmt.Errorf("adding body cache marker: %w", err)
		}
		if _, err := s.db.Exec("UPDATE messages SET body_cached = 1 WHERE body_text IS NOT NULL OR body_html IS NOT NULL"); err != nil {
			return fmt.Errorf("marking existing cached bodies: %w", err)
		}
	}
	return s.initFTSSchema()
}

const schemaSQL = `
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
CREATE TABLE IF NOT EXISTS shipments (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  carrier TEXT NOT NULL,
  tracking_number TEXT,
  tracking_url TEXT,
  order_id TEXT,
  detected_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shipments_message ON shipments(message_id);
`

// initFTSSchema creates the external-content FTS5 index over `messages` and
// its sync triggers, guarded on the table's existence so it runs once; the
// first creation backfills existing rows with the FTS5 `rebuild` command.
func (s *Store) initFTSSchema() error {
	var count int
	if err := s.db.QueryRow("SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?", "messages_fts").Scan(&count); err != nil {
		return fmt.Errorf("checking table existence: %w", err)
	}
	if count > 0 {
		return nil
	}
	if _, err := s.db.Exec(`
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
`); err != nil {
		return fmt.Errorf("creating fts5 index: %w", err)
	}
	return nil
}

func (s *Store) messageColumnExists(column string) (bool, error) {
	rows, err := s.db.Query("PRAGMA table_info(messages)")
	if err != nil {
		return false, err
	}
	defer rows.Close()
	for rows.Next() {
		var cid int
		var name, typ string
		var notnull int
		var dflt sql.NullString
		var pk int
		if err := rows.Scan(&cid, &name, &typ, &notnull, &dflt, &pk); err != nil {
			return false, err
		}
		if name == column {
			return true, nil
		}
	}
	return false, rows.Err()
}

// --- Row projections ---------------------------------------------------------

// FolderRow is a folder as stored.
type FolderRow struct {
	ID          int64
	AccountID   string
	Name        string
	Role        string
	UnreadCount int64
	TotalCount  int64
	UIDValidity int64
	LastSeenUID int64
}

// MessageRow is a message summary row.
type MessageRow struct {
	ID             int64
	FolderID       int64
	UID            int64
	Subject        string
	FromName       string
	FromAddr       string
	Date           string
	Snippet        string
	Seen           bool
	Flagged        bool
	HasAttachments bool
}

// MessageUpsert is the data required to upsert a message envelope.
type MessageUpsert struct {
	FolderID       int64
	UID            uint32
	MessageID      *string
	Subject        string
	FromName       string
	FromAddr       string
	ToAddrs        []string
	CcAddrs        []string
	Date           string
	Snippet        string
	Seen           bool
	Flagged        bool
	HasAttachments bool
	RFC822Size     uint32
}

// ReplyMetadata is trusted metadata used to thread a reply without accepting
// raw headers from the UI.
type ReplyMetadata struct {
	AccountID string
	MessageID *string
}

// BodyPrefetchCandidate is a body prefetch target from the bounded inbox
// working set.
type BodyPrefetchCandidate struct {
	ID         int64
	UID        uint32
	RFC822Size uint32
}

// AttachmentMeta is attachment metadata extracted from a parsed message body.
type AttachmentMeta struct {
	// Stable part index (position in deterministic parse order).
	PartIndex uint32
	Filename  string
	MimeType  string
	SizeBytes uint32
	IsInline  bool
	// Content-ID normalized without angle brackets, when present.
	ContentID *string
}

// ShipmentInsert is a shipment detected by local body parsing, ready to
// persist, with the carrier already reduced to its stable DB code.
type ShipmentInsert struct {
	Carrier        string
	TrackingNumber *string
	TrackingURL    *string
	OrderID        *string
}

// ShipmentRow is a shipment row as stored.
type ShipmentRow struct {
	ID             int64
	Carrier        string
	TrackingNumber *string
	TrackingURL    *string
	OrderID        *string
	DetectedAt     string
}

// AttachmentRow is an attachment row projected for the reader.
type AttachmentRow struct {
	ID        int64
	Filename  string
	MimeType  string
	SizeBytes int64
	IsInline  bool
}

// AttachmentLocation is everything needed to refetch and extract a single
// attachment from the server.
type AttachmentLocation struct {
	MessageID  int64
	PartIndex  uint32
	Filename   string
	MimeType   string
	UID        uint32
	AccountID  string
	FolderName string
}

// MessageLocation locates a message on its server.
type MessageLocation struct {
	FolderID   int64
	UID        uint32
	AccountID  string
	FolderName string
}

// MessageActionContext is everything a message action (flag/move/archive/
// delete) needs about one row.
type MessageActionContext struct {
	FolderID   int64
	UID        uint32
	AccountID  string
	FolderName string
	FolderRole string
	Seen       bool
}

// CachedBody is the cached body parts plus recipient lists for a message.
type CachedBody struct {
	Text    *string
	HTML    *string
	ToAddrs []string
	CcAddrs []string
	Cached  bool
}

// UnifiedMessageRow is a message row plus its owning account id.
type UnifiedMessageRow struct {
	Msg       MessageRow
	AccountID string
}

// --- Folder queries ----------------------------------------------------------

// UpsertFolder inserts or updates a folder for (accountID, name), reconciling
// UIDVALIDITY: on a mismatch the folder's cached messages are wiped (wiped is
// true) and counts reset so the next sync repopulates from scratch.
func (s *Store) UpsertFolder(accountID, name string, role FolderRole, uidvalidity uint32) (id int64, wiped bool, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.upsertFolderLocked(accountID, name, role, uidvalidity)
}

func (s *Store) upsertFolderLocked(accountID, name string, role FolderRole, uidvalidity uint32) (int64, bool, error) {
	var id, stored int64
	err := s.db.QueryRow("SELECT id, uidvalidity FROM folders WHERE account_id = ? AND name = ?", accountID, name).Scan(&id, &stored)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		res, err := s.db.Exec("INSERT INTO folders (account_id, name, role, uidvalidity) VALUES (?, ?, ?, ?)", accountID, name, string(role), uidvalidity)
		if err != nil {
			return 0, false, fmt.Errorf("inserting folder: %w", err)
		}
		id, err := res.LastInsertId()
		return id, false, err
	case err != nil:
		return 0, false, fmt.Errorf("looking up folder: %w", err)
	}
	if stored != int64(uidvalidity) && uidvalidity != 0 {
		if _, err := s.db.Exec("DELETE FROM messages WHERE folder_id = ?", id); err != nil {
			return 0, false, fmt.Errorf("wiping messages on uidvalidity change: %w", err)
		}
		if _, err := s.db.Exec("UPDATE folders SET uidvalidity = ?, last_seen_uid = 0, unread_count = 0, total_count = 0, role = ? WHERE id = ?", uidvalidity, string(role), id); err != nil {
			return 0, false, fmt.Errorf("resetting folder on uidvalidity change: %w", err)
		}
		return id, true, nil
	}
	if _, err := s.db.Exec("UPDATE folders SET role = ? WHERE id = ?", string(role), id); err != nil {
		return 0, false, fmt.Errorf("updating folder role: %w", err)
	}
	return id, false, nil
}

const folderColumns = "id, account_id, name, role, unread_count, total_count, uidvalidity, last_seen_uid"

func scanFolder(r interface{ Scan(...any) error }) (FolderRow, error) {
	var f FolderRow
	err := r.Scan(&f.ID, &f.AccountID, &f.Name, &f.Role, &f.UnreadCount, &f.TotalCount, &f.UIDValidity, &f.LastSeenUID)
	return f, err
}

// ListFolders lists all folders for an account in display order.
func (s *Store) ListFolders(accountID string) ([]FolderRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query("SELECT "+folderColumns+" FROM folders WHERE account_id = ? ORDER BY "+
		"CASE role WHEN 'inbox' THEN 0 WHEN 'sent' THEN 1 WHEN 'drafts' THEN 2 "+
		"WHEN 'archive' THEN 3 WHEN 'spam' THEN 4 WHEN 'trash' THEN 5 ELSE 6 END, name", accountID)
	if err != nil {
		return nil, fmt.Errorf("listing folders: %w", err)
	}
	defer rows.Close()
	out := []FolderRow{}
	for rows.Next() {
		f, err := scanFolder(rows)
		if err != nil {
			return nil, fmt.Errorf("collecting folders: %w", err)
		}
		out = append(out, f)
	}
	return out, rows.Err()
}

// GetFolder fetches a single folder by rowid (nil when absent).
func (s *Store) GetFolder(folderID int64) (*FolderRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.getFolderLocked(folderID)
}

func (s *Store) getFolderLocked(folderID int64) (*FolderRow, error) {
	f, err := scanFolder(s.db.QueryRow("SELECT "+folderColumns+" FROM folders WHERE id = ?", folderID))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("fetching folder: %w", err)
	}
	return &f, nil
}

// SetLastSeenUID raises the notification high-water mark for a folder.
func (s *Store) SetLastSeenUID(folderID int64, uid uint32) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.setLastSeenUIDLocked(folderID, uid)
}

func (s *Store) setLastSeenUIDLocked(folderID int64, uid uint32) error {
	if _, err := s.db.Exec("UPDATE folders SET last_seen_uid = ? WHERE id = ? AND last_seen_uid < ?", uid, folderID, uid); err != nil {
		return fmt.Errorf("updating last_seen_uid: %w", err)
	}
	return nil
}

// SetFolderCounts persists the authoritative server counts from IMAP STATUS.
func (s *Store) SetFolderCounts(folderID int64, total, unread uint32) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.setFolderCountsLocked(folderID, total, unread)
}

func (s *Store) setFolderCountsLocked(folderID int64, total, unread uint32) error {
	if _, err := s.db.Exec("UPDATE folders SET total_count = ?, unread_count = ? WHERE id = ?", total, unread, folderID); err != nil {
		return fmt.Errorf("updating server folder counts: %w", err)
	}
	return nil
}

// AdjustFolderUnreadCount adjusts the unread count after a local flag change.
func (s *Store) AdjustFolderUnreadCount(folderID int64, delta int64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.adjustFolderUnreadCountLocked(folderID, delta)
}

func (s *Store) adjustFolderUnreadCountLocked(folderID int64, delta int64) error {
	if _, err := s.db.Exec("UPDATE folders SET unread_count = MAX(0, unread_count + ?) WHERE id = ?", delta, folderID); err != nil {
		return fmt.Errorf("adjusting folder unread count: %w", err)
	}
	return nil
}

// IncrementFolderCounts bumps a target folder's counts after a message lands
// in it (move/archive): total +1 and, when the message was unseen, unread +1.
func (s *Store) IncrementFolderCounts(folderID int64, unseen bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := s.db.Exec("UPDATE folders SET total_count = total_count + 1 WHERE id = ?", folderID); err != nil {
		return fmt.Errorf("incrementing folder total count: %w", err)
	}
	if unseen {
		if _, err := s.db.Exec("UPDATE folders SET unread_count = unread_count + 1 WHERE id = ?", folderID); err != nil {
			return fmt.Errorf("incrementing folder unread count: %w", err)
		}
	}
	return nil
}

// FindFolderByRole returns the first folder of an account carrying role.
func (s *Store) FindFolderByRole(accountID string, role FolderRole) (*FolderRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	f, err := scanFolder(s.db.QueryRow("SELECT "+folderColumns+" FROM folders WHERE account_id = ? AND role = ? ORDER BY id LIMIT 1", accountID, string(role)))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("finding folder by role: %w", err)
	}
	return &f, nil
}

// --- Message queries ---------------------------------------------------------

func jsonList(list []string) string {
	b, err := json.Marshal(list)
	if err != nil || list == nil {
		return "[]"
	}
	return string(b)
}

// UpsertMessage inserts or updates a message by (folder_id, uid), preserving
// already-cached body columns, and returns the rowid.
func (s *Store) UpsertMessage(m *MessageUpsert) (int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.upsertMessageLocked(m)
}

func (s *Store) upsertMessageLocked(m *MessageUpsert) (int64, error) {
	_, err := s.db.Exec(`INSERT INTO messages
           (folder_id, uid, message_id, subject, from_name, from_addr, to_addrs, cc_addrs,
             date, snippet, seen, flagged, has_attachments, rfc822_size)
          VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)
          ON CONFLICT(folder_id, uid) DO UPDATE SET
           message_id=excluded.message_id, subject=excluded.subject,
           from_name=excluded.from_name, from_addr=excluded.from_addr,
           to_addrs=excluded.to_addrs, cc_addrs=excluded.cc_addrs, date=excluded.date,
           snippet=CASE WHEN excluded.snippet='' THEN messages.snippet ELSE excluded.snippet END,
           seen=excluded.seen, flagged=excluded.flagged,
           has_attachments=excluded.has_attachments, rfc822_size=excluded.rfc822_size`,
		m.FolderID, m.UID, m.MessageID, m.Subject, m.FromName, m.FromAddr, jsonList(m.ToAddrs), jsonList(m.CcAddrs),
		m.Date, m.Snippet, b2i(m.Seen), b2i(m.Flagged), b2i(m.HasAttachments), m.RFC822Size)
	if err != nil {
		return 0, fmt.Errorf("upserting message: %w", err)
	}
	var id int64
	if err := s.db.QueryRow("SELECT id FROM messages WHERE folder_id = ? AND uid = ?", m.FolderID, m.UID).Scan(&id); err != nil {
		return 0, fmt.Errorf("reading message rowid: %w", err)
	}
	return id, nil
}

func b2i(b bool) int64 {
	if b {
		return 1
	}
	return 0
}

const messageColumns = "id, folder_id, uid, subject, from_name, from_addr, date, snippet, seen, flagged, has_attachments"

func scanMessage(r interface{ Scan(...any) error }, extra ...any) (MessageRow, error) {
	var m MessageRow
	var seen, flagged, has int64
	dest := []any{&m.ID, &m.FolderID, &m.UID, &m.Subject, &m.FromName, &m.FromAddr, &m.Date, &m.Snippet, &seen, &flagged, &has}
	dest = append(dest, extra...)
	if err := r.Scan(dest...); err != nil {
		return m, err
	}
	m.Seen, m.Flagged, m.HasAttachments = seen != 0, flagged != 0, has != 0
	return m, nil
}

func (s *Store) queryUnified(query string, args ...any) ([]UnifiedMessageRow, error) {
	rows, err := s.db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []UnifiedMessageRow{}
	for rows.Next() {
		var accountID string
		m, err := scanMessage(rows, &accountID)
		if err != nil {
			return nil, err
		}
		out = append(out, UnifiedMessageRow{Msg: m, AccountID: accountID})
	}
	return out, rows.Err()
}

// PageUnifiedMessages pages messages across all inbox folders of all
// accounts, newest first.
func (s *Store) PageUnifiedMessages(offset, limit int64) ([]UnifiedMessageRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.queryUnified("SELECT m.id, m.folder_id, m.uid, m.subject, m.from_name, m.from_addr, m.date, "+
		"m.snippet, m.seen, m.flagged, m.has_attachments, f.account_id FROM messages m "+
		"JOIN folders f ON f.id = m.folder_id WHERE f.role = 'inbox' ORDER BY m.date DESC LIMIT ? OFFSET ?", limit, offset)
	if err != nil {
		return nil, fmt.Errorf("collecting unified messages: %w", err)
	}
	return rows, nil
}

// BuildFTSMatch builds a safe FTS5 MATCH expression from raw user input.
//
// Never pass untrusted text to MATCH directly: bareword operators (AND, OR,
// NEAR) and punctuation are FTS5 query syntax. Each whitespace-separated
// token has its embedded `"` doubled, is wrapped in double quotes so it is
// literal text, and gets a `*` suffix for prefix matching; tokens are joined
// by spaces (implicit AND). Returns "" when the query has no tokens.
func BuildFTSMatch(query string) string {
	fields := strings.Fields(query)
	parts := make([]string, 0, len(fields))
	for _, tok := range fields {
		parts = append(parts, `"`+strings.ReplaceAll(tok, `"`, `""`)+`"*`)
	}
	return strings.Join(parts, " ")
}

// SearchMessages runs a relevance-ranked full-text search over cached
// envelopes and bodies. accountID nil searches every account; an empty or
// whitespace-only query returns no rows without touching FTS.
func (s *Store) SearchMessages(query string, accountID *string, offset, limit int64) ([]UnifiedMessageRow, error) {
	match := BuildFTSMatch(query)
	if match == "" {
		return []UnifiedMessageRow{}, nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.queryUnified("SELECT m.id, m.folder_id, m.uid, m.subject, m.from_name, m.from_addr, m.date, "+
		"m.snippet, m.seen, m.flagged, m.has_attachments, f.account_id FROM messages_fts "+
		"JOIN messages m ON m.id = messages_fts.rowid JOIN folders f ON f.id = m.folder_id "+
		"WHERE messages_fts MATCH ? AND (? IS NULL OR f.account_id = ?) ORDER BY rank LIMIT ? OFFSET ?",
		match, accountID, accountID, limit, offset)
	if err != nil {
		return nil, fmt.Errorf("collecting search results: %w", err)
	}
	return rows, nil
}

// PageMessages pages a folder's messages, newest first.
func (s *Store) PageMessages(folderID, offset, limit int64) ([]MessageRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query("SELECT "+messageColumns+" FROM messages WHERE folder_id = ? ORDER BY date DESC LIMIT ? OFFSET ?", folderID, limit, offset)
	if err != nil {
		return nil, fmt.Errorf("paging messages: %w", err)
	}
	defer rows.Close()
	out := []MessageRow{}
	for rows.Next() {
		m, err := scanMessage(rows)
		if err != nil {
			return nil, fmt.Errorf("collecting messages: %w", err)
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

// LocateMessage resolves a message rowid to its server location.
func (s *Store) LocateMessage(messageID int64) (*MessageLocation, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var loc MessageLocation
	err := s.db.QueryRow("SELECT m.folder_id, m.uid, f.account_id, f.name FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?", messageID).
		Scan(&loc.FolderID, &loc.UID, &loc.AccountID, &loc.FolderName)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("locating message: %w", err)
	}
	return &loc, nil
}

// MessageActionContext resolves the action context for a message rowid.
func (s *Store) MessageActionContext(messageID int64) (*MessageActionContext, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var ctx MessageActionContext
	var seen int64
	err := s.db.QueryRow("SELECT m.folder_id, m.uid, f.account_id, f.name, f.role, m.seen FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?", messageID).
		Scan(&ctx.FolderID, &ctx.UID, &ctx.AccountID, &ctx.FolderName, &ctx.FolderRole, &seen)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("reading message action context: %w", err)
	}
	ctx.Seen = seen != 0
	return &ctx, nil
}

// GetReplyMetadata resolves the account and RFC Message-ID for a local row.
func (s *Store) GetReplyMetadata(localMessageID int64) (*ReplyMetadata, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var md ReplyMetadata
	var mid sql.NullString
	err := s.db.QueryRow("SELECT f.account_id, m.message_id FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?", localMessageID).Scan(&md.AccountID, &mid)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("reading reply metadata: %w", err)
	}
	if mid.Valid {
		md.MessageID = &mid.String
	}
	return &md, nil
}

// GetBody reads the cached body columns for a message (nil when the row is
// absent; body parts may be nil).
func (s *Store) GetBody(messageID int64) (*CachedBody, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var text, html sql.NullString
	var toJSON, ccJSON string
	var cached int64
	err := s.db.QueryRow("SELECT body_text, body_html, to_addrs, cc_addrs, body_cached FROM messages WHERE id = ?", messageID).
		Scan(&text, &html, &toJSON, &ccJSON, &cached)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("reading cached body: %w", err)
	}
	body := &CachedBody{Cached: cached != 0, ToAddrs: parseJSONList(toJSON), CcAddrs: parseJSONList(ccJSON)}
	if text.Valid {
		body.Text = &text.String
	}
	if html.Valid {
		body.HTML = &html.String
	}
	return body, nil
}

func parseJSONList(raw string) []string {
	var list []string
	if err := json.Unmarshal([]byte(raw), &list); err != nil || list == nil {
		return []string{}
	}
	return list
}

// SetBody persists fetched body parts for a message. A nil snippet leaves
// the stored snippet untouched (COALESCE).
func (s *Store) SetBody(messageID int64, text, html *string, toAddrs, ccAddrs []string, snippet *string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.setBodyLocked(messageID, text, html, toAddrs, ccAddrs, snippet)
}

func (s *Store) setBodyLocked(messageID int64, text, html *string, toAddrs, ccAddrs []string, snippet *string) error {
	if _, err := s.db.Exec("UPDATE messages SET body_text = ?, body_html = ?, to_addrs = ?, cc_addrs = ?, snippet = COALESCE(?, snippet), body_cached = 1 WHERE id = ?",
		text, html, jsonList(toAddrs), jsonList(ccAddrs), snippet, messageID); err != nil {
		return fmt.Errorf("storing body: %w", err)
	}
	return nil
}

// ReplaceAttachments replaces all attachment rows for a message and
// reconciles `has_attachments` from the real parse (true iff at least one
// non-inline attachment exists), in one transaction.
func (s *Store) ReplaceAttachments(messageID int64, attachments []AttachmentMeta) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.replaceAttachmentsLocked(messageID, attachments)
}

func (s *Store) replaceAttachmentsLocked(messageID int64, attachments []AttachmentMeta) error {
	tx, err := s.db.Begin()
	if err != nil {
		return fmt.Errorf("beginning attachment transaction: %w", err)
	}
	defer tx.Rollback()
	if _, err := tx.Exec("DELETE FROM attachments WHERE message_id = ?", messageID); err != nil {
		return fmt.Errorf("clearing old attachments: %w", err)
	}
	nonInline := 0
	for _, a := range attachments {
		if _, err := tx.Exec("INSERT INTO attachments (message_id, part_index, filename, mime_type, size_bytes, is_inline, content_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
			messageID, a.PartIndex, a.Filename, a.MimeType, a.SizeBytes, b2i(a.IsInline), a.ContentID); err != nil {
			return fmt.Errorf("inserting attachment: %w", err)
		}
		if !a.IsInline {
			nonInline++
		}
	}
	if _, err := tx.Exec("UPDATE messages SET has_attachments = ? WHERE id = ?", b2i(nonInline > 0), messageID); err != nil {
		return fmt.Errorf("updating has_attachments from parse: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("committing attachment transaction: %w", err)
	}
	return nil
}

// ReplaceShipments replaces all shipment rows detected for a message,
// wholesale in one transaction, so re-parsing cannot accumulate duplicates.
func (s *Store) ReplaceShipments(messageID int64, shipments []ShipmentInsert) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.replaceShipmentsLocked(messageID, shipments)
}

func (s *Store) replaceShipmentsLocked(messageID int64, shipments []ShipmentInsert) error {
	tx, err := s.db.Begin()
	if err != nil {
		return fmt.Errorf("beginning shipment transaction: %w", err)
	}
	defer tx.Rollback()
	if _, err := tx.Exec("DELETE FROM shipments WHERE message_id = ?", messageID); err != nil {
		return fmt.Errorf("clearing old shipments: %w", err)
	}
	if len(shipments) > 0 {
		detectedAt := RFC3339(time.Now().UTC())
		for _, sh := range shipments {
			if _, err := tx.Exec("INSERT INTO shipments (message_id, carrier, tracking_number, tracking_url, order_id, detected_at) VALUES (?, ?, ?, ?, ?, ?)",
				messageID, sh.Carrier, sh.TrackingNumber, sh.TrackingURL, sh.OrderID, detectedAt); err != nil {
				return fmt.Errorf("inserting shipment: %w", err)
			}
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("committing shipment transaction: %w", err)
	}
	return nil
}

// ListShipments lists the shipments detected for a message, in detection order.
func (s *Store) ListShipments(messageID int64) ([]ShipmentRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query("SELECT id, carrier, tracking_number, tracking_url, order_id, detected_at FROM shipments WHERE message_id = ? ORDER BY id", messageID)
	if err != nil {
		return nil, fmt.Errorf("listing shipments: %w", err)
	}
	defer rows.Close()
	out := []ShipmentRow{}
	for rows.Next() {
		var r ShipmentRow
		var tn, tu, oid sql.NullString
		if err := rows.Scan(&r.ID, &r.Carrier, &tn, &tu, &oid, &r.DetectedAt); err != nil {
			return nil, fmt.Errorf("collecting shipments: %w", err)
		}
		r.TrackingNumber, r.TrackingURL, r.OrderID = nullStr(tn), nullStr(tu), nullStr(oid)
		out = append(out, r)
	}
	return out, rows.Err()
}

func nullStr(v sql.NullString) *string {
	if !v.Valid {
		return nil
	}
	return &v.String
}

// SnippetFunc recomputes a snippet from cached body parts; nil means nothing
// usable.
type SnippetFunc func(text, html *string) *string

// HealCachedSnippets recomputes the preview snippet of every message with a
// cached body and updates only the rows whose snippet actually changes,
// returning how many changed. Cached bodies are never re-fetched, so this
// startup pass is how rows snippeted by an older cleanup version pick up
// improvements; a nil recompute leaves the stored snippet untouched.
func (s *Store) HealCachedSnippets(recompute SnippetFunc) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query("SELECT id, snippet, body_text, body_html FROM messages WHERE body_cached = 1")
	if err != nil {
		return 0, fmt.Errorf("querying cached bodies for snippet healing: %w", err)
	}
	type row struct {
		id         int64
		current    string
		text, html *string
	}
	var cached []row
	for rows.Next() {
		var r row
		var text, html sql.NullString
		if err := rows.Scan(&r.id, &r.current, &text, &html); err != nil {
			rows.Close()
			return 0, fmt.Errorf("collecting cached bodies for snippet healing: %w", err)
		}
		r.text, r.html = nullStr(text), nullStr(html)
		cached = append(cached, r)
	}
	rows.Close()
	changed := 0
	for _, r := range cached {
		fresh := recompute(r.text, r.html)
		if fresh == nil || *fresh == r.current {
			continue
		}
		if _, err := s.db.Exec("UPDATE messages SET snippet = ? WHERE id = ?", *fresh, r.id); err != nil {
			return changed, fmt.Errorf("updating healed snippet: %w", err)
		}
		changed++
	}
	return changed, nil
}

// ListAttachments lists a message's attachment rows, ordered by parse position.
func (s *Store) ListAttachments(messageID int64) ([]AttachmentRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.listAttachmentsLocked(messageID)
}

func (s *Store) listAttachmentsLocked(messageID int64) ([]AttachmentRow, error) {
	rows, err := s.db.Query("SELECT id, filename, mime_type, size_bytes, is_inline FROM attachments WHERE message_id = ? ORDER BY part_index", messageID)
	if err != nil {
		return nil, fmt.Errorf("listing attachments: %w", err)
	}
	defer rows.Close()
	out := []AttachmentRow{}
	for rows.Next() {
		var a AttachmentRow
		var inline int64
		if err := rows.Scan(&a.ID, &a.Filename, &a.MimeType, &a.SizeBytes, &inline); err != nil {
			return nil, fmt.Errorf("collecting attachments: %w", err)
		}
		a.IsInline = inline != 0
		out = append(out, a)
	}
	return out, rows.Err()
}

// GetAttachment resolves an attachment rowid to everything needed to refetch
// its bytes (nil when absent).
func (s *Store) GetAttachment(attachmentID int64) (*AttachmentLocation, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var loc AttachmentLocation
	err := s.db.QueryRow("SELECT a.message_id, a.part_index, a.filename, a.mime_type, m.uid, f.account_id, f.name FROM attachments a "+
		"JOIN messages m ON m.id = a.message_id JOIN folders f ON f.id = m.folder_id WHERE a.id = ?", attachmentID).
		Scan(&loc.MessageID, &loc.PartIndex, &loc.Filename, &loc.MimeType, &loc.UID, &loc.AccountID, &loc.FolderName)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("locating attachment: %w", err)
	}
	return &loc, nil
}

// BodyPrefetchCandidates selects a bounded batch of uncached bodies from the
// recent and unread inbox messages.
func (s *Store) BodyPrefetchCandidates(folderID int64, workingSetLimit, fetchLimit, maximumSize uint32) ([]BodyPrefetchCandidate, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query(`WITH unread AS (
           SELECT id FROM messages WHERE folder_id = ?1 AND seen = 0 ORDER BY date DESC LIMIT ?2
         ), recent AS (
           SELECT id FROM messages WHERE folder_id = ?1 ORDER BY date DESC LIMIT ?2
         ), working AS (SELECT id FROM unread UNION SELECT id FROM recent)
         SELECT m.id, m.uid, m.rfc822_size FROM messages m
         JOIN working w ON w.id = m.id
         WHERE m.body_cached = 0 AND (m.rfc822_size = 0 OR m.rfc822_size <= ?3)
         ORDER BY m.seen ASC, m.date DESC LIMIT ?4`, folderID, workingSetLimit, maximumSize, fetchLimit)
	if err != nil {
		return nil, fmt.Errorf("selecting body prefetch candidates: %w", err)
	}
	defer rows.Close()
	out := []BodyPrefetchCandidate{}
	for rows.Next() {
		var c BodyPrefetchCandidate
		if err := rows.Scan(&c.ID, &c.UID, &c.RFC822Size); err != nil {
			return nil, fmt.Errorf("collecting body prefetch candidates: %w", err)
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

// SetMessageSize stores a size discovered separately for a pre-migration
// envelope.
func (s *Store) SetMessageSize(messageID int64, size uint32) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := s.db.Exec("UPDATE messages SET rfc822_size = ? WHERE id = ?", size, messageID); err != nil {
		return fmt.Errorf("storing message size: %w", err)
	}
	return nil
}

// MarkSeen updates the seen flag in the local cache, reporting whether the
// value actually changed.
func (s *Store) MarkSeen(messageID int64, seen bool) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	res, err := s.db.Exec("UPDATE messages SET seen = ? WHERE id = ? AND seen != ?", b2i(seen), messageID, b2i(seen))
	if err != nil {
		return false, fmt.Errorf("marking seen: %w", err)
	}
	n, _ := res.RowsAffected()
	return n > 0, nil
}

// SetFlagged updates the flagged state in the local cache, reporting whether
// the value actually changed.
func (s *Store) SetFlagged(messageID int64, flagged bool) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	res, err := s.db.Exec("UPDATE messages SET flagged = ? WHERE id = ? AND flagged != ?", b2i(flagged), messageID, b2i(flagged))
	if err != nil {
		return false, fmt.Errorf("setting flagged: %w", err)
	}
	n, _ := res.RowsAffected()
	return n > 0, nil
}

// MessageIDHeader reads the RFC 5322 Message-ID cached for a row, if any.
func (s *Store) MessageIDHeader(messageID int64) (*string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var v sql.NullString
	err := s.db.QueryRow("SELECT message_id FROM messages WHERE id = ?", messageID).Scan(&v)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("reading message-id header: %w", err)
	}
	return nullStr(v), nil
}

// MarkSeenForMessageIDSiblings updates the seen flag on every other cached
// copy of a message sharing the same RFC 5322 Message-ID within an account
// (Gmail labels), adjusting each affected folder's unread count, and returns
// the distinct folder ids that changed.
func (s *Store) MarkSeenForMessageIDSiblings(accountID, messageIDHeader string, excludeRowID int64, seen bool) ([]int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rows, err := s.db.Query("SELECT m.id, m.folder_id, m.seen FROM messages m JOIN folders f ON f.id = m.folder_id WHERE f.account_id = ? AND m.message_id = ? AND m.id != ?",
		accountID, messageIDHeader, excludeRowID)
	if err != nil {
		return nil, fmt.Errorf("querying sibling messages: %w", err)
	}
	type sib struct {
		id, folder int64
		seen       bool
	}
	var sibs []sib
	for rows.Next() {
		var sb sib
		var wasSeen int64
		if err := rows.Scan(&sb.id, &sb.folder, &wasSeen); err != nil {
			rows.Close()
			return nil, fmt.Errorf("collecting sibling messages: %w", err)
		}
		sb.seen = wasSeen != 0
		sibs = append(sibs, sb)
	}
	rows.Close()
	changed := []int64{}
	for _, sb := range sibs {
		if sb.seen == seen {
			continue
		}
		if _, err := s.db.Exec("UPDATE messages SET seen = ? WHERE id = ?", b2i(seen), sb.id); err != nil {
			return nil, fmt.Errorf("updating sibling seen state: %w", err)
		}
		delta := int64(1)
		if seen {
			delta = -1
		}
		if err := s.adjustFolderUnreadCountLocked(sb.folder, delta); err != nil {
			return nil, err
		}
		if !containsInt(changed, sb.folder) {
			changed = append(changed, sb.folder)
		}
	}
	return changed, nil
}

func containsInt(list []int64, v int64) bool {
	for _, x := range list {
		if x == v {
			return true
		}
	}
	return false
}

// RemoveMessage removes a row from the local cache and adjusts its source
// folder's counts (total -1, and unread -1 when unseen, floored at 0). The
// FTS5 delete trigger and the foreign-key cascades clean up derived rows. A
// no-op if the row is gone.
func (s *Store) RemoveMessage(messageID int64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	var folderID, seen int64
	err := s.db.QueryRow("SELECT folder_id, seen FROM messages WHERE id = ?", messageID).Scan(&folderID, &seen)
	if errors.Is(err, sql.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("looking up message before removal: %w", err)
	}
	if _, err := s.db.Exec("DELETE FROM messages WHERE id = ?", messageID); err != nil {
		return fmt.Errorf("removing message: %w", err)
	}
	if _, err := s.db.Exec("UPDATE folders SET total_count = MAX(0, total_count - 1) WHERE id = ?", folderID); err != nil {
		return fmt.Errorf("decrementing folder total count: %w", err)
	}
	if seen == 0 {
		if _, err := s.db.Exec("UPDATE folders SET unread_count = MAX(0, unread_count - 1) WHERE id = ?", folderID); err != nil {
			return fmt.Errorf("decrementing folder unread count: %w", err)
		}
	}
	return nil
}

// MaxUID is the highest UID cached for a folder (0 if empty).
func (s *Store) MaxUID(folderID int64) (uint32, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.maxUIDLocked(folderID)
}

func (s *Store) maxUIDLocked(folderID int64) (uint32, error) {
	var v int64
	if err := s.db.QueryRow("SELECT COALESCE(MAX(uid), 0) FROM messages WHERE folder_id = ?", folderID).Scan(&v); err != nil {
		return 0, fmt.Errorf("computing max uid: %w", err)
	}
	return uint32(v), nil
}

// DeleteAccountData deletes cached messages and folders for an account.
func (s *Store) DeleteAccountData(accountID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := s.db.Exec("DELETE FROM messages WHERE folder_id IN (SELECT id FROM folders WHERE account_id = ?)", accountID); err != nil {
		return fmt.Errorf("deleting account messages: %w", err)
	}
	if _, err := s.db.Exec("DELETE FROM folders WHERE account_id = ?", accountID); err != nil {
		return fmt.Errorf("deleting account folders: %w", err)
	}
	return nil
}

// --- batch helpers for the sync loop ----------------------------------------

// SyncedEnvelope is one upserted envelope's rowid.
type SyncedEnvelope struct {
	RowID int64
}

// FolderSyncBatch upserts a batch of envelopes under one lock hold and, for
// an inbox, raises the last-seen UID, mirroring the Rust sync loop's single
// `conn.lock()` scope. It returns the rowids in input order.
func (s *Store) UpsertEnvelopes(envelopes []*MessageUpsert) ([]int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ids := make([]int64, 0, len(envelopes))
	for _, e := range envelopes {
		id, err := s.upsertMessageLocked(e)
		if err != nil {
			return nil, err
		}
		ids = append(ids, id)
	}
	return ids, nil
}

// CacheParsedBody stores a fetched body, its attachment metadata and its
// detected shipments in one lock hold — the "body-parse hook" both the
// foreground cache miss and the background prefetch go through.
func (s *Store) CacheParsedBody(messageID int64, text, html *string, toAddrs, ccAddrs []string, snippet *string, attachments []AttachmentMeta, shipments []ShipmentInsert) ([]AttachmentRow, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := s.setBodyLocked(messageID, text, html, toAddrs, ccAddrs, snippet); err != nil {
		return nil, err
	}
	if err := s.replaceAttachmentsLocked(messageID, attachments); err != nil {
		return nil, err
	}
	if err := s.replaceShipmentsLocked(messageID, shipments); err != nil {
		return nil, err
	}
	return s.listAttachmentsLocked(messageID)
}

// FolderSyncState reads the cached max UID and last-seen UID for a folder.
func (s *Store) FolderSyncState(folderID int64) (maxUID, lastSeen uint32, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	maxUID, err = s.maxUIDLocked(folderID)
	if err != nil {
		return 0, 0, err
	}
	f, err := s.getFolderLocked(folderID)
	if err != nil {
		return 0, 0, err
	}
	if f != nil {
		lastSeen = uint32(f.LastSeenUID)
	}
	return maxUID, lastSeen, nil
}

// UpsertFolderWithCounts upserts a folder and stores its STATUS counts in one
// lock hold.
func (s *Store) UpsertFolderWithCounts(accountID, name string, role FolderRole, uidvalidity, exists, unseen uint32) (int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	id, _, err := s.upsertFolderLocked(accountID, name, role, uidvalidity)
	if err != nil {
		return 0, err
	}
	if err := s.setFolderCountsLocked(id, exists, unseen); err != nil {
		return 0, err
	}
	return id, nil
}
