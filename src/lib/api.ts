// Typed wrappers around Tauri's invoke() and event listen().
// Contract source: docs/ARCHITECTURE.md ("Tauri commands" + "Events").
//
// IMPORTANT: never call invoke()/listen() at module top level. These functions
// are only invoked from onMount/effects so that prerendering (npm run build)
// and plain-browser dev don't explode. Callers should still try/catch.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
	Account,
	DiscoveredConfig,
	Folder,
	ImapAccountInput,
	MessageBody,
	MessageSummary,
	MessagesUpdatedEvent,
	NewMessagesEvent,
	OmarchyTheme,
	SyncStateEvent,
} from "./types";

// ---- Commands ----

export function getTheme(): Promise<OmarchyTheme> {
	return invoke("get_theme");
}

export function listAccounts(): Promise<Account[]> {
	return invoke("list_accounts");
}

export function addImapAccount(input: ImapAccountInput): Promise<Account> {
	return invoke("add_imap_account", { input });
}

export function startGmailOauth(): Promise<Account> {
	return invoke("start_gmail_oauth");
}

export function removeAccount(accountId: string): Promise<void> {
	return invoke("remove_account", { accountId });
}

export function listFolders(accountId: string): Promise<Folder[]> {
	return invoke("list_folders", { accountId });
}

export function listMessages(
	folderId: number,
	offset: number,
	limit: number,
): Promise<MessageSummary[]> {
	return invoke("list_messages", { folderId, offset, limit });
}

export function listUnifiedMessages(
	offset: number,
	limit: number,
): Promise<MessageSummary[]> {
	return invoke("list_unified_messages", { offset, limit });
}

export function getMessageBody(messageId: number): Promise<MessageBody> {
	return invoke("get_message_body", { messageId });
}

export function markRead(messageId: number, seen: boolean): Promise<void> {
	return invoke("mark_read", { messageId, seen });
}

export function syncFolder(folderId: number): Promise<void> {
	return invoke("sync_folder", { folderId });
}

export function syncAccount(accountId: string): Promise<void> {
	return invoke("sync_account", { accountId });
}

export function testNotification(): Promise<void> {
	return invoke("test_notification");
}

export function discoverAccountConfig(
	email: string,
): Promise<DiscoveredConfig> {
	return invoke("discover_account_config", { email });
}

// ---- Events ----

export function onThemeChanged(
	cb: (theme: OmarchyTheme) => void,
): Promise<UnlistenFn> {
	return listen<OmarchyTheme>("omarchy:theme-changed", (e) => cb(e.payload));
}

export function onNewMessages(
	cb: (payload: NewMessagesEvent) => void,
): Promise<UnlistenFn> {
	return listen<NewMessagesEvent>("mail:new-messages", (e) => cb(e.payload));
}

export function onMessagesUpdated(
	cb: (payload: MessagesUpdatedEvent) => void,
): Promise<UnlistenFn> {
	return listen<MessagesUpdatedEvent>("mail:messages-updated", (e) =>
		cb(e.payload),
	);
}

export function onSyncState(
	cb: (payload: SyncStateEvent) => void,
): Promise<UnlistenFn> {
	return listen<SyncStateEvent>("mail:sync-state", (e) => cb(e.payload));
}

export type { UnlistenFn };
