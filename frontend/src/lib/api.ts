// Typed wrappers over the generated Wails bindings — the only place the
// frontend calls the backend. Command names and shapes are bound by
// docs/ARCHITECTURE.md ("Commands" + "Events"); the bindings themselves come
// from `wails3 generate bindings` and live in frontend/bindings.
//
// The generated signatures type every list as `T[] | null` (a nil Go slice).
// The backend guarantees non-null lists (models.NonNil), so results are
// narrowed to the app's types here rather than null-checked everywhere.
//
// IMPORTANT: never call the backend at module top level. These functions are
// only invoked from onMount/effects so that prerendering (npm run build) and
// plain-browser dev don't explode. Callers should still try/catch.

import { Browser, Events } from "@wailsio/runtime";
import * as App from "$bindings/cosmicmail/app";
import { isOpenableUrl } from "./message-html";
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
	SendMessageInput,
	Settings,
	Shipment,
	SyncStateEvent,
} from "./types";

/** Unsubscribes an event listener. */
export type UnlistenFn = () => void;

const list = <T>(p: Promise<T[] | null>): Promise<T[]> => p.then((v) => v ?? []);

// ---- Commands ----

export function getTheme(): Promise<OmarchyTheme> {
	return App.GetTheme() as Promise<OmarchyTheme>;
}

export function listAccounts(): Promise<Account[]> {
	return list(App.ListAccounts()) as Promise<Account[]>;
}

export function addImapAccount(input: ImapAccountInput): Promise<Account> {
	return App.AddImapAccount(input) as Promise<Account>;
}

export function startGmailOauth(): Promise<Account> {
	return App.StartGmailOauth() as Promise<Account>;
}

export function reauthGmailAccount(accountId: string): Promise<Account> {
	return App.ReauthGmailAccount(accountId) as Promise<Account>;
}

export function removeAccount(accountId: string): Promise<void> {
	return App.RemoveAccount(accountId);
}

export function listFolders(accountId: string): Promise<Folder[]> {
	return list(App.ListFolders(accountId)) as Promise<Folder[]>;
}

export function listMessages(
	folderId: number,
	offset: number,
	limit: number,
): Promise<MessageSummary[]> {
	return list(App.ListMessages(folderId, offset, limit));
}

export function listUnifiedMessages(
	offset: number,
	limit: number,
): Promise<MessageSummary[]> {
	return list(App.ListUnifiedMessages(offset, limit));
}

export function searchMessages(
	query: string,
	accountId: string | null,
	offset: number,
	limit: number,
): Promise<MessageSummary[]> {
	return list(App.SearchMessages(query, accountId, offset, limit));
}

export function getMessageBody(messageId: number): Promise<MessageBody> {
	return App.GetMessageBody(messageId).then((b) => ({
		...b,
		toAddrs: b.toAddrs ?? [],
		ccAddrs: b.ccAddrs ?? [],
		attachments: b.attachments ?? [],
	}));
}

export function listShipmentsForMessage(messageId: number): Promise<Shipment[]> {
	return list(App.ListShipmentsForMessage(messageId)) as Promise<Shipment[]>;
}

export function saveAttachment(attachmentId: number): Promise<string> {
	return App.SaveAttachment(attachmentId);
}

export function markRead(messageId: number, seen: boolean): Promise<void> {
	return App.MarkRead(messageId, seen);
}

export function markFlagged(messageId: number, flagged: boolean): Promise<void> {
	return App.MarkFlagged(messageId, flagged);
}

export function moveMessage(
	messageId: number,
	targetFolderId: number,
): Promise<void> {
	return App.MoveMessage(messageId, targetFolderId);
}

export function archiveMessage(messageId: number): Promise<void> {
	return App.ArchiveMessage(messageId);
}

export function deleteMessage(messageId: number): Promise<void> {
	return App.DeleteMessage(messageId);
}

export function sendMessage(input: SendMessageInput): Promise<void> {
	return App.SendMessage(input);
}

export function syncFolder(folderId: number): Promise<void> {
	return App.SyncFolder(folderId);
}

export function syncAccount(accountId: string): Promise<void> {
	return App.SyncAccount(accountId);
}

export function testNotification(): Promise<void> {
	return App.TestNotification();
}

export function discoverAccountConfig(
	email: string,
): Promise<DiscoveredConfig> {
	return App.DiscoverAccountConfig(email) as Promise<DiscoveredConfig>;
}

export function getSettings(): Promise<Settings> {
	return App.GetSettings();
}

export function updateSettings(settings: Settings): Promise<Settings> {
	return App.UpdateSettings(settings);
}

/**
 * Open a URL in the system browser. Only http(s) URLs are ever handed to
 * the OS — the Tauri build's opener capability was scoped to those, and the
 * Wails runtime call is unscoped, so the check lives here where every caller
 * (reader links, shipment tracking cards) funnels through.
 */
export function openUrl(url: string): Promise<void> {
	if (!isOpenableUrl(url)) {
		return Promise.reject(new Error("Only http(s) links can be opened"));
	}
	return Browser.OpenURL(url);
}

// ---- Events ----

function on<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
	return Promise.resolve(Events.On(name, (ev) => cb(ev.data as T)));
}

export function onThemeChanged(
	cb: (theme: OmarchyTheme) => void,
): Promise<UnlistenFn> {
	return on<OmarchyTheme>("omarchy:theme-changed", (t) =>
		cb({ ...t, palette: t.palette ?? [] }),
	);
}

export function onNewMessages(
	cb: (payload: NewMessagesEvent) => void,
): Promise<UnlistenFn> {
	return on<NewMessagesEvent>("mail:new-messages", (p) =>
		cb({ ...p, messages: p.messages ?? [] }),
	);
}

export function onMessagesUpdated(
	cb: (payload: MessagesUpdatedEvent) => void,
): Promise<UnlistenFn> {
	return on<MessagesUpdatedEvent>("mail:messages-updated", cb);
}

export function onSyncState(
	cb: (payload: SyncStateEvent) => void,
): Promise<UnlistenFn> {
	return on<SyncStateEvent>("mail:sync-state", cb);
}
