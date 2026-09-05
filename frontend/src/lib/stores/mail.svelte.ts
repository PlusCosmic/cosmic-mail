// Central runes-based application state for Cosmic Mail.
//
// Guards: every backend call is wrapped so plain-browser dev / prerender don't
// explode. Event subscriptions are only wired up from init(), called in onMount.

import * as api from "$lib/api";
import type {
	Account,
	AttachmentInfo,
	Folder,
	MessageBody,
	MessageSummary,
	SendMessageInput,
	Settings,
	Shipment,
	SyncState,
	SyncStateEvent,
} from "$lib/types";
import type { UnlistenFn } from "$lib/api";
import { nextSelectionId } from "$lib/selection";

export const PAGE_LIMIT = 50;

export interface Toast {
	id: number;
	kind: "info" | "error";
	message: string;
}

let toastSeq = 0;

export class MailStore {
	// Data
	accounts = $state<Account[]>([]);
	/** folderId lists keyed by accountId. */
	foldersByAccount = $state<Record<string, Folder[]>>({});
	syncStates = $state<Record<string, SyncState>>({});
	/** Accounts whose Gmail credentials are dead — Reconnect (not retry) is the fix. */
	reauthRequired = $state<Record<string, boolean>>({});

	/** When true the list shows all inbox folders across all accounts. */
	unified = $state(true);

	selectedAccountId = $state<string | null>(null);
	selectedFolderId = $state<number | null>(null);

	messages = $state<MessageSummary[]>([]);
	offset = $state(0);
	hasMore = $state(false);
	loadingMessages = $state(false);

	selectedMessageId = $state<number | null>(null);
	body = $state<MessageBody | null>(null);
	loadingBody = $state(false);
	/** Shipments detected in the currently-loaded body, if any. */
	shipments = $state<Shipment[]>([]);

	toasts = $state<Toast[]>([]);

	/** Client-side filter chip selection. */
	filter = $state<"all" | "unread" | "flagged">("all");

	/** Client-side search query (case-insensitive, matches from/subject/snippet). */
	query = $state("");

	/** True while the message list shows backend full-cache search results. */
	searchActive = $state(false);

	/** The submitted backend search term (distinct from the live `query` filter). */
	searchQuery = $state("");

	/** Persisted global preferences (defaults until loaded from the backend). */
	settings = $state<Settings>({ alwaysDownloadRemoteImages: false });

	// Lifecycle
	loadingAccounts = $state(true);

	#unlisteners: UnlistenFn[] = [];

	// ---- Derived ----

	get selectedFolder(): Folder | null {
		if (this.selectedFolderId === null) return null;
		for (const list of Object.values(this.foldersByAccount)) {
			const f = list.find((f) => f.id === this.selectedFolderId);
			if (f) return f;
		}
		return null;
	}

	get selectedMessage(): MessageSummary | null {
		if (this.selectedMessageId === null) return null;
		return this.visibleMessages.find((m) => m.id === this.selectedMessageId) ?? null;
	}

	get totalMessageCount(): number {
		return this.messages.length;
	}

	/** Messages after filter + query applied — all keyboard nav operates on this. */
	get visibleMessages(): MessageSummary[] {
		let msgs = this.messages;
		// Keep the currently selected message visible even once it's read —
		// opening a message under the Unread filter shouldn't yank it (and the
		// reader) out from under the user. It naturally drops out once
		// selection moves elsewhere.
		if (this.filter === "unread")
			msgs = msgs.filter((m) => !m.seen || m.id === this.selectedMessageId);
		else if (this.filter === "flagged") msgs = msgs.filter((m) => m.flagged);
		if (this.query.trim()) {
			const q = this.query.trim().toLowerCase();
			msgs = msgs.filter(
				(m) =>
					m.fromName.toLowerCase().includes(q) ||
					m.fromAddr.toLowerCase().includes(q) ||
					m.subject.toLowerCase().includes(q) ||
					m.snippet.toLowerCase().includes(q),
			);
		}
		return msgs;
	}

	/** Sum of unreadCount over all role='inbox' folders (all accounts). */
	get unifiedUnread(): number {
		let total = 0;
		for (const list of Object.values(this.foldersByAccount)) {
			for (const f of list) {
				if (f.role === "inbox") total += f.unreadCount;
			}
		}
		return total;
	}

	/** Sum of unreadCount over all role='inbox' folders for a specific account. */
	accountUnread(accountId: string): number {
		const list = this.foldersByAccount[accountId] ?? [];
		return list
			.filter((f) => f.role === "inbox")
			.reduce((s, f) => s + f.unreadCount, 0);
	}

	/** Total unread across every folder of every account. */
	get totalUnread(): number {
		let total = 0;
		for (const list of Object.values(this.foldersByAccount)) {
			for (const f of list) total += f.unreadCount;
		}
		return total;
	}

	// ---- Toasts ----

	toast(message: string, kind: Toast["kind"] = "info"): void {
		const id = ++toastSeq;
		this.toasts = [...this.toasts, { id, kind, message }];
		setTimeout(() => this.dismissToast(id), kind === "error" ? 8000 : 4000);
	}

	dismissToast(id: number): void {
		this.toasts = this.toasts.filter((t) => t.id !== id);
	}

	#error(prefix: string, e: unknown): void {
		const msg = e instanceof Error ? e.message : String(e);
		this.toast(`${prefix}: ${msg}`, "error");
	}

	// ---- Init / teardown ----

	async init(): Promise<void> {
		try {
			this.settings = await api.getSettings();
		} catch {
			// Backend unavailable (plain browser) — keep defaults.
		}
		await this.loadAccounts();
		try {
			this.#unlisteners.push(
				await api.onNewMessages((p) => this.#onNewMessages(p)),
				await api.onMessagesUpdated((p) => this.#onMessagesUpdated(p)),
				await api.onSyncState((p) => this.#onSyncState(p)),
			);
		} catch {
			// Events unavailable (plain browser) — app still functions read-only.
		}
	}

	dispose(): void {
		for (const u of this.#unlisteners) {
			try {
				u();
			} catch {
				/* ignore */
			}
		}
		this.#unlisteners = [];
	}

	// ---- Accounts ----

	async loadAccounts(): Promise<void> {
		this.loadingAccounts = true;
		try {
			this.accounts = await api.listAccounts();
			for (const acc of this.accounts) {
				if (!(acc.id in this.syncStates)) this.syncStates[acc.id] = "idle";
				await this.loadFolders(acc.id);
			}
			// On startup with accounts and nothing selected, default to unified view.
			if (this.accounts.length > 0 && this.selectedFolderId === null && this.unified) {
				await this.selectUnified();
			}
		} catch {
			// In plain-browser dev this simply yields zero accounts.
			this.accounts = [];
		} finally {
			this.loadingAccounts = false;
		}
	}

	async loadFolders(accountId: string): Promise<void> {
		try {
			const folders = await api.listFolders(accountId);
			this.foldersByAccount = { ...this.foldersByAccount, [accountId]: folders };
		} catch {
			this.foldersByAccount = { ...this.foldersByAccount, [accountId]: [] };
		}
	}

	async addImapAccount(
		input: Parameters<typeof api.addImapAccount>[0],
	): Promise<Account> {
		const acc = await api.addImapAccount(input);
		await this.loadAccounts();
		this.toast(`Added ${acc.email}`);
		return acc;
	}

	async startGmailOauth(): Promise<Account> {
		const acc = await api.startGmailOauth();
		await this.loadAccounts();
		this.toast(`Added ${acc.email}`);
		return acc;
	}

	/**
	 * Re-run the Gmail OAuth consent for an existing account in place.
	 * Blocks until the user completes (or abandons) the browser consent.
	 */
	async reauthAccount(accountId: string): Promise<void> {
		try {
			const acc = await api.reauthGmailAccount(accountId);
			this.reauthRequired = { ...this.reauthRequired, [accountId]: false };
			this.toast(`Reconnected ${acc.email}`);
		} catch (e) {
			this.#error("Failed to reconnect account", e);
		}
	}

	async removeAccount(accountId: string): Promise<void> {
		try {
			await api.removeAccount(accountId);
			if (this.selectedAccountId === accountId) {
				this.selectedAccountId = null;
				this.selectedFolderId = null;
				this.messages = [];
				this.selectedMessageId = null;
				this.body = null;
				this.shipments = [];
				this.unified = true;
			}
			await this.loadAccounts();
			this.toast("Account removed");
		} catch (e) {
			this.#error("Failed to remove account", e);
		}
	}

	// ---- Unified / folder selection ----

	/** Switch to unified inbox view. */
	async selectUnified(): Promise<void> {
		this.unified = true;
		this.selectedAccountId = null;
		this.selectedFolderId = null;
		this.selectedMessageId = null;
		this.body = null;
		this.shipments = [];
		this.offset = 0;
		this.messages = [];
		this.filter = "all";
		this.query = "";
		this.searchActive = false;
		this.searchQuery = "";
		await this.#fetchPage(false);
	}

	/** Select a specific folder (per-account mode). */
	async selectFolder(folder: Folder): Promise<void> {
		this.unified = false;
		this.selectedAccountId = folder.accountId;
		this.selectedFolderId = folder.id;
		this.selectedMessageId = null;
		this.body = null;
		this.shipments = [];
		this.offset = 0;
		this.messages = [];
		this.filter = "all";
		this.query = "";
		this.searchActive = false;
		this.searchQuery = "";
		await this.#fetchPage(false);
	}

	/** Select an account in the rail — opens its inbox folder by default. */
	async selectAccount(accountId: string): Promise<void> {
		const folders = this.foldersByAccount[accountId] ?? [];
		const inbox = folders.find((f) => f.role === "inbox") ?? folders[0];
		if (inbox) {
			await this.selectFolder(inbox);
		} else {
			// No folders yet; show the account in single-account mode without messages.
			this.unified = false;
			this.selectedAccountId = accountId;
			this.selectedFolderId = null;
			this.selectedMessageId = null;
			this.body = null;
			this.shipments = [];
			this.messages = [];
			this.searchActive = false;
			this.searchQuery = "";
		}
	}

	/** Run a backend full-cache search for `q`, replacing the message list. */
	async runSearch(q: string): Promise<void> {
		const query = q.trim();
		if (!query) {
			await this.clearSearch();
			return;
		}
		this.searchActive = true;
		this.searchQuery = query;
		// Clear the live filter so it doesn't further constrain the ranked
		// results (a multi-word live filter requires a contiguous substring the
		// FTS match does not); the active-search indicator carries the term.
		this.query = "";
		this.selectedMessageId = null;
		this.body = null;
		this.shipments = [];
		this.offset = 0;
		this.messages = [];
		await this.#fetchPage(false);
	}

	/** Exit search mode and refetch the underlying (unified or folder) view. */
	async clearSearch(): Promise<void> {
		if (!this.searchActive) return;
		this.searchActive = false;
		this.searchQuery = "";
		this.selectedMessageId = null;
		this.body = null;
		this.shipments = [];
		this.offset = 0;
		this.messages = [];
		await this.#fetchPage(false);
	}

	async #fetchPage(append: boolean): Promise<void> {
		if (this.searchActive) {
			this.loadingMessages = true;
			const query = this.searchQuery;
			const accountId = this.unified ? null : this.selectedAccountId;
			try {
				const page = await api.searchMessages(query, accountId, this.offset, PAGE_LIMIT);
				// Guard against races: still searching the same term?
				if (!this.searchActive || this.searchQuery !== query) return;
				this.messages = append ? [...this.messages, ...page] : page;
				this.hasMore = page.length === PAGE_LIMIT;
			} catch (e) {
				if (append) this.#error("Failed to load more results", e);
				else this.messages = [];
			} finally {
				this.loadingMessages = false;
			}
			return;
		}
		this.loadingMessages = true;
		if (this.unified) {
			const snap = true; // we're in unified mode at call time
			try {
				const page = await api.listUnifiedMessages(this.offset, PAGE_LIMIT);
				// Guard: still in unified mode?
				if (!this.unified) return;
				this.messages = append ? [...this.messages, ...page] : page;
				this.hasMore = page.length === PAGE_LIMIT;
			} catch (e) {
				if (append) this.#error("Failed to load more messages", e);
				else this.messages = [];
			} finally {
				this.loadingMessages = false;
			}
			void snap; // suppress unused warning
		} else {
			if (this.selectedFolderId === null) {
				this.loadingMessages = false;
				return;
			}
			const folderId = this.selectedFolderId;
			try {
				const page = await api.listMessages(folderId, this.offset, PAGE_LIMIT);
				// Guard against races: the folder may have changed during the await.
				if (this.selectedFolderId !== folderId) return;
				this.messages = append ? [...this.messages, ...page] : page;
				this.hasMore = page.length === PAGE_LIMIT;
			} catch (e) {
				if (append) this.#error("Failed to load more messages", e);
				else this.messages = [];
			} finally {
				this.loadingMessages = false;
			}
		}
	}

	async loadMore(): Promise<void> {
		if (!this.hasMore || this.loadingMessages) return;
		this.offset += PAGE_LIMIT;
		await this.#fetchPage(true);
	}

	/** Refetch the currently-loaded range (offset 0 .. current length). */
	async refreshCurrentPage(): Promise<void> {
		const limit = Math.max(PAGE_LIMIT, this.messages.length);
		if (this.unified) {
			const wasUnified = true;
			try {
				const page = await api.listUnifiedMessages(0, limit);
				if (!this.unified) return;
				this.messages = page;
				this.hasMore = page.length === limit;
			} catch {
				/* silent — keep existing view */
			}
			void wasUnified;
		} else {
			if (this.selectedFolderId === null) return;
			const folderId = this.selectedFolderId;
			try {
				const page = await api.listMessages(folderId, 0, limit);
				if (this.selectedFolderId !== folderId) return;
				this.messages = page;
				this.hasMore = page.length === limit;
			} catch {
				/* silent — keep existing view */
			}
		}
	}

	async selectMessage(msg: MessageSummary): Promise<void> {
		this.selectedMessageId = msg.id;
		this.body = null;
		this.shipments = [];
		this.loadingBody = true;
		try {
			const body = await api.getMessageBody(msg.id);
			if (this.selectedMessageId === msg.id) this.body = body;
		} catch (e) {
			if (this.selectedMessageId === msg.id) this.#error("Failed to load message", e);
		} finally {
			if (this.selectedMessageId === msg.id) this.loadingBody = false;
		}
		// Shipment detection runs as part of body caching backend-side, so this
		// is safe to fetch right after the body call resolves either way.
		try {
			const shipments = await api.listShipmentsForMessage(msg.id);
			if (this.selectedMessageId === msg.id) this.shipments = shipments;
		} catch {
			/* silent — the shipment card is a non-essential enhancement */
		}
		// Opening a message marks it read.
		if (!msg.seen) await this.setSeen(msg, true);
	}

	async sendMessage(input: SendMessageInput): Promise<void> {
		await api.sendMessage(input);
		this.toast("Message sent");
	}

	/** Download an attachment, reporting the saved path (or an error) via toast. */
	async saveAttachment(attachment: AttachmentInfo): Promise<void> {
		try {
			const path = await api.saveAttachment(attachment.id);
			this.toast(`Saved to ${path}`);
		} catch (e) {
			this.#error(`Failed to save ${attachment.filename || "attachment"}`, e);
		}
	}

	/** Persist updated settings optimistically, rolling back on failure. */
	async updateSettings(next: Settings): Promise<void> {
		const prev = this.settings;
		this.settings = next;
		try {
			this.settings = await api.updateSettings(next);
		} catch (e) {
			this.settings = prev;
			this.#error("Failed to save settings", e);
		}
	}

	selectByIndex(index: number): void {
		const msg = this.visibleMessages[index];
		if (msg) void this.selectMessage(msg);
	}

	/** Toggle read state of the given message (optimistic). */
	async toggleRead(msg: MessageSummary): Promise<void> {
		await this.setSeen(msg, !msg.seen);
	}

	async setSeen(msg: MessageSummary, seen: boolean): Promise<void> {
		if (msg.seen === seen) return;
		const prev = msg.seen;
		this.#patchMessage(msg.id, { seen });
		this.#bumpUnread(msg.folderId, seen ? -1 : +1);
		try {
			await api.markRead(msg.id, seen);
		} catch (e) {
			// Roll back.
			this.#patchMessage(msg.id, { seen: prev });
			this.#bumpUnread(msg.folderId, seen ? +1 : -1);
			this.#error("Failed to update flag", e);
		}
	}

	/** Toggle the flagged state of a message (optimistic, no unread change). */
	async toggleFlagged(msg: MessageSummary): Promise<void> {
		const flagged = !msg.flagged;
		this.#patchMessage(msg.id, { flagged });
		try {
			await api.markFlagged(msg.id, flagged);
		} catch (e) {
			this.#patchMessage(msg.id, { flagged: !flagged });
			this.#error("Failed to update flag", e);
		}
	}

	/** Archive a message into its account's archive folder (optimistic). */
	async archiveMessage(msg: MessageSummary): Promise<void> {
		await this.#removeOptimistically(
			msg,
			() => api.archiveMessage(msg.id),
			null,
			"Failed to archive message",
		);
	}

	/** Delete a message: to trash, or permanently when already in trash (optimistic). */
	async deleteMessage(msg: MessageSummary): Promise<void> {
		await this.#removeOptimistically(
			msg,
			() => api.deleteMessage(msg.id),
			null,
			"Failed to delete message",
		);
	}

	/** Move a message to another folder of the same account (optimistic). */
	async moveMessage(msg: MessageSummary, targetFolderId: number): Promise<void> {
		if (targetFolderId === msg.folderId) return;
		await this.#removeOptimistically(
			msg,
			() => api.moveMessage(msg.id, targetFolderId),
			targetFolderId,
			"Failed to move message",
		);
	}

	/**
	 * Shared optimistic removal for archive/delete/move: pick the next selection,
	 * drop the row, adjust source (and, for a move, target) folder counts, select
	 * the next message, then run `action`. On failure, refetch the current page +
	 * affected folders and toast. `targetFolderId` is the move destination (whose
	 * counts get bumped up) or `null` for archive/delete.
	 */
	async #removeOptimistically(
		msg: MessageSummary,
		action: () => Promise<void>,
		targetFolderId: number | null,
		errorPrefix: string,
	): Promise<void> {
		const nextId = nextSelectionId(this.visibleMessages, msg.id);
		const wasSelected = this.selectedMessageId === msg.id;

		this.messages = this.messages.filter((m) => m.id !== msg.id);
		this.#bumpFolderCounts(msg.folderId, -1, msg.seen ? 0 : -1);
		if (targetFolderId !== null) {
			this.#bumpFolderCounts(targetFolderId, +1, msg.seen ? 0 : +1);
		}

		if (wasSelected) {
			this.selectedMessageId = nextId;
			this.body = null;
			this.shipments = [];
			const next = nextId === null ? null : this.messages.find((m) => m.id === nextId);
			if (next) void this.selectMessage(next);
		}

		try {
			await action();
		} catch (e) {
			this.#error(errorPrefix, e);
			// Full-refetch rollback: restore the list and the affected folder counts.
			await this.refreshCurrentPage();
			await this.loadFolders(msg.accountId);
			if (this.messages.some((m) => m.id === msg.id)) {
				this.selectedMessageId = msg.id;
			}
		}
	}

	async syncSelectedFolder(): Promise<void> {
		if (this.selectedFolderId === null) return;
		try {
			await api.syncFolder(this.selectedFolderId);
		} catch (e) {
			this.#error("Sync failed", e);
		}
	}

	async syncAccount(accountId: string): Promise<void> {
		try {
			await api.syncAccount(accountId);
		} catch (e) {
			this.#error("Sync failed", e);
		}
	}

	// ---- Internal mutation helpers ----

	#patchMessage(id: number, patch: Partial<MessageSummary>): void {
		this.messages = this.messages.map((m) =>
			m.id === id ? { ...m, ...patch } : m,
		);
	}

	#bumpUnread(folderId: number, delta: number): void {
		const next: Record<string, Folder[]> = {};
		for (const [accId, list] of Object.entries(this.foldersByAccount)) {
			next[accId] = list.map((f) =>
				f.id === folderId
					? { ...f, unreadCount: Math.max(0, f.unreadCount + delta) }
					: f,
			);
		}
		this.foldersByAccount = next;
	}

	/** Adjust a folder's total and unread counts locally (both floored at 0). */
	#bumpFolderCounts(folderId: number, totalDelta: number, unreadDelta: number): void {
		const next: Record<string, Folder[]> = {};
		for (const [accId, list] of Object.entries(this.foldersByAccount)) {
			next[accId] = list.map((f) =>
				f.id === folderId
					? {
							...f,
							totalCount: Math.max(0, f.totalCount + totalDelta),
							unreadCount: Math.max(0, f.unreadCount + unreadDelta),
						}
					: f,
			);
		}
		this.foldersByAccount = next;
	}

	/** Find a folder by its id, scanning all accounts. */
	#folderById(folderId: number): Folder | null {
		for (const list of Object.values(this.foldersByAccount)) {
			const f = list.find((f) => f.id === folderId);
			if (f) return f;
		}
		return null;
	}

	// ---- Event handlers ----

	#onNewMessages(p: {
		accountId: string;
		folderId: number;
		messages: MessageSummary[];
	}): void {
		// While a search is active the list holds ranked results, not a folder
		// view — don't prepend into it. The underlying view refreshes on clear.
		// The unread badge still updates below.
		if (this.searchActive) {
			const newUnread = p.messages.filter((m) => !m.seen).length;
			if (newUnread) this.#bumpUnread(p.folderId, newUnread);
			return;
		}
		if (this.unified) {
			// In unified mode, prepend if the event's folder is an inbox.
			const folder = this.#folderById(p.folderId);
			if (folder === null) {
				// Folder not classified yet — this is the first sync of a freshly
				// loaded account, where new-message events can arrive before
				// loadFolders has recorded the INBOX. Learn the folders, then
				// refresh so inbox mail appears live instead of only after a
				// manual reselect.
				void this.loadFolders(p.accountId).then(() => {
					if (this.unified && this.#folderById(p.folderId)?.role === "inbox") {
						void this.refreshCurrentPage();
					}
				});
			} else if (folder.role === "inbox") {
				const existing = new Set(this.messages.map((m) => m.id));
				const fresh = p.messages.filter((m) => !existing.has(m.id));
				if (fresh.length) this.messages = [...fresh, ...this.messages];
			}
		} else {
			// Per-folder mode: prepend if this folder is currently open.
			if (this.selectedFolderId === p.folderId) {
				const existing = new Set(this.messages.map((m) => m.id));
				const fresh = p.messages.filter((m) => !existing.has(m.id));
				if (fresh.length) this.messages = [...fresh, ...this.messages];
			}
		}
		// Bump the unread badge for the folder.
		const newUnread = p.messages.filter((m) => !m.seen).length;
		if (newUnread) this.#bumpUnread(p.folderId, newUnread);
	}

	#onMessagesUpdated(p: { folderId: number }): void {
		// Don't refresh the search result list into a folder view; still update
		// the affected account's folder badges below.
		if (this.searchActive) {
			const acc = this.#accountForFolder(p.folderId);
			if (acc) void this.loadFolders(acc);
			return;
		}
		if (this.unified) {
			// In unified mode, refresh if the updated folder is an inbox.
			const folder = this.#folderById(p.folderId);
			if (folder?.role === "inbox") void this.refreshCurrentPage();
		} else {
			if (this.selectedFolderId === p.folderId) void this.refreshCurrentPage();
		}
		// Refresh folder badges for the affected account too.
		const acc = this.#accountForFolder(p.folderId);
		if (acc) void this.loadFolders(acc);
	}

	#onSyncState(p: SyncStateEvent): void {
		this.syncStates = { ...this.syncStates, [p.accountId]: p.state };
		if (p.state === "error" && p.needsReauth) {
			// Dead credentials fail every backoff retry identically — flag the
			// account and toast once on the transition, not per retry.
			if (!this.reauthRequired[p.accountId]) {
				this.reauthRequired = { ...this.reauthRequired, [p.accountId]: true };
				const email = this.accounts.find((a) => a.id === p.accountId)?.email;
				this.toast(
					`Gmail sign-in expired for ${email ?? "an account"} — reconnect it from Settings`,
					"error",
				);
			}
		} else if (p.state === "error" && p.error) {
			this.#error("Sync error", p.error);
		} else if (p.state !== "error" && this.reauthRequired[p.accountId]) {
			// Sync is running again (e.g. after a reconnect) — clear the flag.
			this.reauthRequired = { ...this.reauthRequired, [p.accountId]: false };
		}
		// When a sync settles, refresh folder counts.
		if (p.state === "idle") void this.loadFolders(p.accountId);
	}

	#accountForFolder(folderId: number): string | null {
		for (const [accId, list] of Object.entries(this.foldersByAccount)) {
			if (list.some((f) => f.id === folderId)) return accId;
		}
		return null;
	}
}

export const mail = new MailStore();
