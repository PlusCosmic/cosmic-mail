//! Wire types shared between the backend and frontend.
//!
//! All types serialize with `camelCase` field names to match the TS contract in
//! `docs/ARCHITECTURE.md`.

use serde::{Deserialize, Serialize};

use crate::store::{AttachmentRow, FolderRow, MessageRow};

/// A mail folder projection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub role: String,
    pub unread_count: i64,
    pub total_count: i64,
}

impl From<FolderRow> for Folder {
    fn from(r: FolderRow) -> Self {
        Folder {
            id: r.id,
            account_id: r.account_id,
            name: r.name,
            role: r.role,
            unread_count: r.unread_count,
            total_count: r.total_count,
        }
    }
}

/// A message summary as shown in the message list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: i64,
    pub account_id: String,
    pub folder_id: i64,
    pub uid: u32,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub date: String,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
}

impl MessageSummary {
    /// Build from a DB row plus the owning account id.
    pub fn from_row(r: MessageRow, account_id: String) -> Self {
        MessageSummary {
            id: r.id,
            account_id,
            folder_id: r.folder_id,
            uid: r.uid as u32,
            subject: r.subject,
            from_name: r.from_name,
            from_addr: r.from_addr,
            date: r.date,
            snippet: r.snippet,
            seen: r.seen,
            flagged: r.flagged,
            has_attachments: r.has_attachments,
        }
    }
}

/// Attachment metadata shown in the reader (no bytes; download refetches).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub is_inline: bool,
}

impl From<AttachmentRow> for AttachmentInfo {
    fn from(r: AttachmentRow) -> Self {
        AttachmentInfo {
            id: r.id,
            filename: r.filename,
            mime_type: r.mime_type,
            size_bytes: r.size_bytes,
            is_inline: r.is_inline,
        }
    }
}

/// A message body plus recipient lists and attachment metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBody {
    pub id: i64,
    pub html: Option<String>,
    pub text: Option<String>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Input for adding a plain IMAP account.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImapAccountInput {
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
}

/// A plain-text message submitted through an account's SMTP server.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageInput {
    pub account_id: String,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub bcc_addrs: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub reply_to_message_id: Option<i64>,
}
