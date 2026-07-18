// Wire types — TS mirrors of the Rust serde structs.
// Contract source: docs/ARCHITECTURE.md ("Wire types"). Keep in exact sync.

export type AccountKind = "imap" | "gmail";

export interface Account {
	id: string; // uuid v4
	email: string;
	displayName: string;
	kind: AccountKind;
}

export type FolderRole =
	| "inbox"
	| "sent"
	| "drafts"
	| "trash"
	| "archive"
	| "spam"
	| "normal";

export interface Folder {
	id: number; // db rowid
	accountId: string;
	name: string; // full IMAP mailbox path, e.g. "Work/Receipts"
	role: FolderRole;
	unreadCount: number;
	totalCount: number;
}

export interface MessageSummary {
	id: number; // db rowid (stable local id)
	accountId: string;
	folderId: number;
	uid: number; // IMAP UID within folder
	subject: string;
	fromName: string;
	fromAddr: string;
	date: string; // RFC 3339
	snippet: string; // first ~160 chars of text body, single line
	seen: boolean;
	flagged: boolean;
	hasAttachments: boolean;
}

export interface AttachmentInfo {
	id: number; // attachments.id rowid
	filename: string; // decoded, sanitized display name
	mimeType: string; // e.g. "application/pdf"
	sizeBytes: number; // decoded byte length
	isInline: boolean; // inline cid part vs. a listed attachment
}

export interface MessageBody {
	id: number;
	html: string | null; // sanitization happens frontend-side before render
	text: string | null;
	toAddrs: string[];
	ccAddrs: string[];
	attachments: AttachmentInfo[]; // empty until the body is cached/parsed
}

export interface OmarchyTheme {
	name: string; // e.g. "kanagawa"
	accent: string;
	foreground: string;
	background: string;
	cursor: string;
	selectionForeground: string;
	selectionBackground: string;
	palette: string[]; // color0..color15 as "#rrggbb"
}

export type SyncState = "idle" | "syncing" | "error";

export type DiscoverySource = "autoconfig" | "ispdb" | "mx" | "srv" | "guess";

export interface DiscoveredConfig {
	kind: AccountKind; // "gmail" ⇒ frontend steers user to the Gmail OAuth tab
	imapHost: string;
	imapPort: number; // implicit-TLS entries only
	smtpHost: string;
	smtpPort: number;
	username: string; // %EMAILADDRESS% / %EMAILLOCALPART% already resolved
	source: DiscoverySource;
	confident: boolean; // false ⇒ heuristic guess, UI must show "unverified"
}

export interface ImapAccountInput {
	email: string;
	displayName: string;
	imapHost: string;
	imapPort: number; // implicit TLS (993)
	smtpHost: string;
	smtpPort: number; // STARTTLS (587) or implicit (465)
	username: string; // often == email
	password: string; // goes straight to keyring, never to db/json
}

export interface SendMessageInput {
	accountId: string;
	toAddrs: string[];
	ccAddrs: string[];
	bccAddrs: string[];
	subject: string;
	bodyText: string;
	replyToMessageId: number | null; // local db rowid; backend resolves RFC headers
}

export interface Settings {
	alwaysDownloadRemoteImages: boolean;
}

// ---- Event payloads (backend -> frontend) ----

export interface NewMessagesEvent {
	accountId: string;
	folderId: number;
	messages: MessageSummary[];
}

export interface MessagesUpdatedEvent {
	folderId: number;
}

export interface SyncStateEvent {
	accountId: string;
	state: SyncState;
	error: string | null;
	/** True only when the failure is dead Gmail credentials (retry can't help; offer Reconnect). */
	needsReauth: boolean;
}
