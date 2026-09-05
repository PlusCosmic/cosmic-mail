// Package models holds every struct that crosses the Go/frontend boundary
// (the "Wire types" and event payloads in docs/ARCHITECTURE.md).
//
// Field names are camelCased in JSON, matching the TypeScript mirrors that
// `wails3 generate bindings` writes to frontend/bindings. Optional values are
// pointers (JSON null); list fields must never be nil, because the frontend
// calls array methods on them without checking — use NonNil when building.
package models

// AccountKind selects the auth mechanism used for IMAP/SMTP.
type AccountKind string

const (
	AccountKindImap  AccountKind = "imap"
	AccountKindGmail AccountKind = "gmail"
)

// Account is the public projection of a configured account (no secrets, no
// connection details).
type Account struct {
	ID          string      `json:"id"`
	Email       string      `json:"email"`
	DisplayName string      `json:"displayName"`
	Kind        AccountKind `json:"kind"`
}

// Folder is a mail folder projection.
type Folder struct {
	ID          int64  `json:"id"`
	AccountID   string `json:"accountId"`
	Name        string `json:"name"`
	Role        string `json:"role"`
	UnreadCount int64  `json:"unreadCount"`
	TotalCount  int64  `json:"totalCount"`
}

// MessageSummary is one row of the message list.
type MessageSummary struct {
	ID             int64  `json:"id"`
	AccountID      string `json:"accountId"`
	FolderID       int64  `json:"folderId"`
	UID            uint32 `json:"uid"`
	Subject        string `json:"subject"`
	FromName       string `json:"fromName"`
	FromAddr       string `json:"fromAddr"`
	Date           string `json:"date"`
	Snippet        string `json:"snippet"`
	Seen           bool   `json:"seen"`
	Flagged        bool   `json:"flagged"`
	HasAttachments bool   `json:"hasAttachments"`
}

// AttachmentInfo is attachment metadata shown in the reader (no bytes).
type AttachmentInfo struct {
	ID        int64  `json:"id"`
	Filename  string `json:"filename"`
	MimeType  string `json:"mimeType"`
	SizeBytes int64  `json:"sizeBytes"`
	IsInline  bool   `json:"isInline"`
}

// MessageBody is a message body plus recipient lists and attachment metadata.
type MessageBody struct {
	ID          int64            `json:"id"`
	HTML        *string          `json:"html"`
	Text        *string          `json:"text"`
	ToAddrs     []string         `json:"toAddrs"`
	CcAddrs     []string         `json:"ccAddrs"`
	Attachments []AttachmentInfo `json:"attachments"`
}

// Shipment is a shipment detected in a message body by local heuristics.
type Shipment struct {
	ID             int64   `json:"id"`
	Carrier        string  `json:"carrier"`
	TrackingNumber *string `json:"trackingNumber"`
	TrackingURL    *string `json:"trackingUrl"`
	OrderID        *string `json:"orderId"`
	DetectedAt     string  `json:"detectedAt"`
}

// OmarchyTheme is the active omarchy theme's colors.
type OmarchyTheme struct {
	Name                string   `json:"name"`
	Accent              string   `json:"accent"`
	Foreground          string   `json:"foreground"`
	Background          string   `json:"background"`
	Cursor              string   `json:"cursor"`
	SelectionForeground string   `json:"selectionForeground"`
	SelectionBackground string   `json:"selectionBackground"`
	Palette             []string `json:"palette"`
}

// DiscoverySource says where a DiscoveredConfig came from.
type DiscoverySource string

const (
	SourceAutoconfig DiscoverySource = "autoconfig"
	SourceIspdb      DiscoverySource = "ispdb"
	SourceMx         DiscoverySource = "mx"
	SourceSrv        DiscoverySource = "srv"
	SourceGuess      DiscoverySource = "guess"
)

// DiscoveredConfig is the settings-discovery result.
type DiscoveredConfig struct {
	Kind      AccountKind     `json:"kind"`
	ImapHost  string          `json:"imapHost"`
	ImapPort  int             `json:"imapPort"`
	SmtpHost  string          `json:"smtpHost"`
	SmtpPort  int             `json:"smtpPort"`
	Username  string          `json:"username"`
	Source    DiscoverySource `json:"source"`
	Confident bool            `json:"confident"`
}

// ImapAccountInput is the input for adding a plain IMAP account.
type ImapAccountInput struct {
	Email       string `json:"email"`
	DisplayName string `json:"displayName"`
	ImapHost    string `json:"imapHost"`
	ImapPort    int    `json:"imapPort"`
	SmtpHost    string `json:"smtpHost"`
	SmtpPort    int    `json:"smtpPort"`
	Username    string `json:"username"`
	Password    string `json:"password"`
}

// SendMessageInput is a plain-text message submitted through an account's
// SMTP server.
type SendMessageInput struct {
	AccountID        string   `json:"accountId"`
	ToAddrs          []string `json:"toAddrs"`
	CcAddrs          []string `json:"ccAddrs"`
	BccAddrs         []string `json:"bccAddrs"`
	Subject          string   `json:"subject"`
	BodyText         string   `json:"bodyText"`
	ReplyToMessageID *int64   `json:"replyToMessageId"`
}

// Settings holds the global, non-secret preferences.
type Settings struct {
	AlwaysDownloadRemoteImages bool `json:"alwaysDownloadRemoteImages"`
}

// --- Event payloads (backend -> frontend) -----------------------------------

// Event names, as registered in main.
const (
	EventThemeChanged    = "omarchy:theme-changed"
	EventNewMessages     = "mail:new-messages"
	EventMessagesUpdated = "mail:messages-updated"
	EventSyncState       = "mail:sync-state"
)

// NewMessagesEvent is the payload of `mail:new-messages`.
type NewMessagesEvent struct {
	AccountID string           `json:"accountId"`
	FolderID  int64            `json:"folderId"`
	Messages  []MessageSummary `json:"messages"`
}

// MessagesUpdatedEvent is the payload of `mail:messages-updated`.
type MessagesUpdatedEvent struct {
	FolderID int64 `json:"folderId"`
}

// SyncState is the per-account sync state reported to the frontend.
type SyncState string

const (
	SyncIdle    SyncState = "idle"
	SyncSyncing SyncState = "syncing"
	SyncError   SyncState = "error"
)

// SyncStateEvent is the payload of `mail:sync-state`.
type SyncStateEvent struct {
	AccountID string    `json:"accountId"`
	State     SyncState `json:"state"`
	Error     *string   `json:"error"`
	// True only when the failure is classified AuthExpired (dead Gmail
	// credentials): retrying cannot help and the UI should offer Reconnect.
	NeedsReauth bool `json:"needsReauth"`
}

// NonNil returns s, or an empty (non-nil) slice when s is nil, so the JSON is
// `[]` rather than `null`.
func NonNil[T any](s []T) []T {
	if s == nil {
		return []T{}
	}
	return s
}

// Str returns a pointer to s (an optional string field).
func Str(s string) *string { return &s }

// StrOrNil returns nil for "" and a pointer otherwise.
func StrOrNil(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}
