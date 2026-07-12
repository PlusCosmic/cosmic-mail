// Central runes-based application state for Cosmic Mail.
//
// Guards: every backend call is wrapped so plain-browser dev / prerender don't
// explode. Event subscriptions are only wired up from init(), called in onMount.

import * as api from "$lib/api";
import type {
	Account,
	Folder,
	MessageBody,
	MessageSummary,
	SyncState,
} from "$lib/types";
import type { UnlistenFn } from "$lib/api";

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

	toasts = $state<Toast[]>([]);

	/** Client-side filter chip selection. */
	filter = $state<"all" | "unread" | "flagged">("all");

	/** Client-side search query (case-insensitive, matches from/subject/snippet). */
	query = $state("");

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
		if (this.filter === "unread") msgs = msgs.filter((m) => !m.seen);
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

	async removeAccount(accountId: string): Promise<void> {
		try {
			await api.removeAccount(accountId);
			if (this.selectedAccountId === accountId) {
				this.selectedAccountId = null;
				this.selectedFolderId = null;
				this.messages = [];
				this.selectedMessageId = null;
				this.body = null;
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
		this.offset = 0;
		this.messages = [];
		this.filter = "all";
		this.query = "";
		await this.#fetchPage(false);
	}

	/** Select a specific folder (per-account mode). */
	async selectFolder(folder: Folder): Promise<void> {
		this.unified = false;
		this.selectedAccountId = folder.accountId;
		this.selectedFolderId = folder.id;
		this.selectedMessageId = null;
		this.body = null;
		this.offset = 0;
		this.messages = [];
		this.filter = "all";
		this.query = "";
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
			this.messages = [];
		}
	}

	async #fetchPage(append: boolean): Promise<void> {
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
		this.loadingBody = true;
		try {
			this.body = await api.getMessageBody(msg.id);
		} catch (e) {
			this.#error("Failed to load message", e);
		} finally {
			this.loadingBody = false;
		}
		// Opening a message marks it read.
		if (!msg.seen) await this.setSeen(msg, true);
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
		if (this.unified) {
			// In unified mode, prepend if the event's folder is an inbox.
			const folder = this.#folderById(p.folderId);
			if (folder?.role === "inbox") {
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

	#onSyncState(p: {
		accountId: string;
		state: SyncState;
		error: string | null;
	}): void {
		this.syncStates = { ...this.syncStates, [p.accountId]: p.state };
		if (p.state === "error" && p.error) {
			this.#error("Sync error", p.error);
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
