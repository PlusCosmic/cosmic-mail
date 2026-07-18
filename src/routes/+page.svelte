<script lang="ts">
	import { onMount } from "svelte";
	import { mail } from "$lib/stores/mail.svelte";
	import Rail from "$lib/components/Rail.svelte";
	import FolderColumn from "$lib/components/FolderColumn.svelte";
	import Header from "$lib/components/Header.svelte";
	import MessageList from "$lib/components/MessageList.svelte";
	import Reader from "$lib/components/Reader.svelte";
	import Footer from "$lib/components/Footer.svelte";
	import Toasts from "$lib/components/Toasts.svelte";
	import AddAccountModal from "$lib/components/AddAccountModal.svelte";
	import ComposeModal from "$lib/components/ComposeModal.svelte";
	import CommandPalette from "$lib/components/CommandPalette.svelte";
	import SettingsModal from "$lib/components/SettingsModal.svelte";
	import { replySeed, type ComposeSeed } from "$lib/compose";
	import { displayFolderName } from "$lib/format";
	import type { PaletteCommand } from "$lib/palette";
	import {
		DEFAULT_LIST_PANE_RATIO,
		PANE_DIVIDER_WIDTH,
		PANE_RATIO_STORAGE_KEY,
		isValidPaneRatio,
		paneWidthLimits,
		ratioForListPaneWidth,
		resolveListPaneWidth,
	} from "$lib/pane-layout";

	let showAddAccount = $state(false);
	let showSettings = $state(false);
	let composeSeed = $state<ComposeSeed | null>(null);
	let showPalette = $state(false);
	let paletteCommands = $state<PaletteCommand[]>([]);
	let palettePlaceholder = $state("Run a command…");
	let listEl = $state<HTMLElement>();
	let readerEl = $state<HTMLElement>();
	let searchEl = $state<HTMLInputElement>();
	let panesEl = $state<HTMLElement>();
	let listPaneRatio = $state(DEFAULT_LIST_PANE_RATIO);
	let listPaneWidth = $state(320);
	let minimumListPaneWidth = $state(320);
	let maximumListPaneWidth = $state(320);
	let draggingDivider = $state(false);

	// For the gg (top) chord.
	let lastG = 0;

	onMount(() => {
		void mail.init();
		return () => mail.dispose();
	});

	function availablePaneWidth(node: HTMLElement): number {
		return Math.max(0, node.clientWidth - PANE_DIVIDER_WIDTH);
	}

	function syncPaneWidth(node: HTMLElement) {
		const available = availablePaneWidth(node);
		const limits = paneWidthLimits(available);
		minimumListPaneWidth = limits.minimum;
		maximumListPaneWidth = limits.maximum;
		listPaneWidth = resolveListPaneWidth(available, listPaneRatio);
	}

	function loadPaneRatio() {
		try {
			const stored = localStorage.getItem(PANE_RATIO_STORAGE_KEY);
			if (stored === null) return;
			const parsed = Number(stored);
			if (isValidPaneRatio(parsed)) listPaneRatio = parsed;
		} catch {
			// Storage can be unavailable in hardened webviews; the default still works.
		}
	}

	function persistPaneRatio() {
		try {
			localStorage.setItem(PANE_RATIO_STORAGE_KEY, String(listPaneRatio));
		} catch {
			// Resizing remains functional when persistence is unavailable.
		}
	}

	function observePaneContainer(node: HTMLElement) {
		panesEl = node;
		loadPaneRatio();
		syncPaneWidth(node);

		const observer = new ResizeObserver(() => syncPaneWidth(node));
		observer.observe(node);

		return {
			destroy() {
				observer.disconnect();
				if (panesEl === node) panesEl = undefined;
			},
		};
	}

	function setListPaneWidth(requestedWidth: number, persist: boolean) {
		if (!panesEl) return;
		const available = availablePaneWidth(panesEl);
		const limits = paneWidthLimits(available);
		listPaneWidth = Math.min(Math.max(requestedWidth, limits.minimum), limits.maximum);
		minimumListPaneWidth = limits.minimum;
		maximumListPaneWidth = limits.maximum;
		listPaneRatio = ratioForListPaneWidth(available, listPaneWidth);
		if (persist) persistPaneRatio();
	}

	function resizeFromPointer(clientX: number) {
		if (!panesEl) return;
		const rect = panesEl.getBoundingClientRect();
		setListPaneWidth(clientX - rect.left - PANE_DIVIDER_WIDTH / 2, false);
	}

	function onDividerPointerDown(e: PointerEvent) {
		if (e.button !== 0) return;
		draggingDivider = true;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		resizeFromPointer(e.clientX);
		e.preventDefault();
	}

	function onDividerPointerMove(e: PointerEvent) {
		if (!draggingDivider) return;
		resizeFromPointer(e.clientX);
	}

	function finishDividerDrag() {
		if (!draggingDivider) return;
		draggingDivider = false;
		persistPaneRatio();
	}

	function onDividerKeydown(e: KeyboardEvent) {
		const step = e.shiftKey ? 80 : 24;
		let requestedWidth: number | null = null;

		switch (e.key) {
			case "ArrowLeft":
				requestedWidth = listPaneWidth - step;
				break;
			case "ArrowRight":
				requestedWidth = listPaneWidth + step;
				break;
			case "Home":
				requestedWidth = minimumListPaneWidth;
				break;
			case "End":
				requestedWidth = maximumListPaneWidth;
				break;
		}

		if (requestedWidth === null) return;
		e.preventDefault();
		setListPaneWidth(requestedWidth, true);
	}

	function resetPaneRatio() {
		listPaneRatio = DEFAULT_LIST_PANE_RATIO;
		if (panesEl) syncPaneWidth(panesEl);
		persistPaneRatio();
	}

	// Svelte's a11y checker does not yet recognize the interactive ARIA window-
	// splitter pattern, so listeners are attached as an action to keep the DOM
	// semantics correct without treating the separator as a button.
	function paneDivider(node: HTMLElement) {
		node.addEventListener("pointerdown", onDividerPointerDown);
		node.addEventListener("pointermove", onDividerPointerMove);
		node.addEventListener("pointerup", finishDividerDrag);
		node.addEventListener("pointercancel", finishDividerDrag);
		node.addEventListener("lostpointercapture", finishDividerDrag);
		node.addEventListener("keydown", onDividerKeydown);
		node.addEventListener("dblclick", resetPaneRatio);

		return {
			destroy() {
				node.removeEventListener("pointerdown", onDividerPointerDown);
				node.removeEventListener("pointermove", onDividerPointerMove);
				node.removeEventListener("pointerup", finishDividerDrag);
				node.removeEventListener("pointercancel", finishDividerDrag);
				node.removeEventListener("lostpointercapture", finishDividerDrag);
				node.removeEventListener("keydown", onDividerKeydown);
				node.removeEventListener("dblclick", resetPaneRatio);
			},
		};
	}

	// Index of the currently-selected message within visibleMessages (-1 if none).
	function currentIndex(): number {
		if (mail.selectedMessageId === null) return -1;
		return mail.visibleMessages.findIndex((m) => m.id === mail.selectedMessageId);
	}

	function selectIndex(i: number) {
		if (mail.visibleMessages.length === 0) return;
		const clamped = Math.max(0, Math.min(i, mail.visibleMessages.length - 1));
		const msg = mail.visibleMessages[clamped];
		if (msg) {
			void mail.selectMessage(msg);
			scrollRowIntoView(msg.id);
		}
	}

	// G jumps to the bottom of what's loaded so far, then keeps pulling more
	// pages (capped, so a very large mailbox can't loop indefinitely) until
	// the store reports no more are available, landing on the true last
	// message instead of an arbitrary load-page cutoff.
	const JUMP_TO_BOTTOM_LOAD_CAP = 20;

	async function jumpToBottom() {
		selectIndex(mail.visibleMessages.length - 1);
		let guard = 0;
		while (mail.hasMore && guard < JUMP_TO_BOTTOM_LOAD_CAP) {
			guard++;
			await mail.loadMore();
		}
		selectIndex(mail.visibleMessages.length - 1);
	}

	function scrollRowIntoView(id: number) {
		queueMicrotask(() => {
			const row = listEl?.querySelector<HTMLElement>(`[data-msg-id="${id}"]`);
			row?.scrollIntoView({ block: "nearest" });
		});
	}

	function isEditableTarget(t: EventTarget | null): boolean {
		if (!(t instanceof HTMLElement)) return false;
		const tag = t.tagName;
		return (
			tag === "INPUT" ||
			tag === "TEXTAREA" ||
			tag === "SELECT" ||
			t.isContentEditable
		);
	}

	function isInteractiveTarget(t: EventTarget | null): boolean {
		return (
			t instanceof HTMLElement &&
			(t.isContentEditable ||
				t.closest("button, a, input, textarea, select, [role='button'], [role='separator']") !== null)
		);
	}

	function defaultComposeAccountId(): string | null {
		return (
			mail.selectedAccountId ??
			mail.selectedMessage?.accountId ??
			mail.accounts[0]?.id ??
			null
		);
	}

	function openCompose() {
		const accountId = defaultComposeAccountId();
		if (!accountId) return;
		composeSeed = {
			accountId,
			toAddrs: [],
			ccAddrs: [],
			subject: "",
			bodyText: "",
			replyToMessageId: null,
		};
	}

	function openReply(replyAll = false) {
		const message = mail.selectedMessage;
		if (!message) return;
		const account = mail.accounts.find((candidate) => candidate.id === message.accountId);
		if (!account) return;
		const body = mail.body?.id === message.id ? mail.body : null;
		if (!body) return;
		composeSeed = replySeed(message, body, account, replyAll);
	}

	function closeCompose() {
		const wasReply = composeSeed?.replyToMessageId !== null;
		composeSeed = null;
		queueMicrotask(() => (wasReply ? readerEl : listEl)?.focus());
	}

	// Reuse the command palette as a folder picker for moving the selected message.
	// Commands are the message's account folders minus its current folder.
	function openMovePicker() {
		const message = mail.selectedMessage;
		if (!message) return;
		const folders = mail.foldersByAccount[message.accountId] ?? [];
		const cmds: PaletteCommand[] = folders
			.filter((folder) => folder.id !== message.folderId)
			.map((folder) => ({
				id: `move:${folder.id}`,
				title: `Move to — ${displayFolderName(folder.name)}`,
				keywords: ["move", folder.role, folder.name],
				run: () => void mail.moveMessage(message, folder.id),
			}));
		if (cmds.length === 0) return;
		paletteCommands = cmds;
		palettePlaceholder = "Move to folder…";
		showPalette = true;
	}

	// Build the currently-applicable command set for the palette. Evaluated when the
	// palette opens so it reflects the live selection, folders, and message state.
	function buildPaletteCommands(): PaletteCommand[] {
		const cmds: PaletteCommand[] = [];

		cmds.push({
			id: "unified",
			title: "Unified inbox",
			keywords: ["all", "inbox"],
			run: () => void mail.selectUnified(),
		});

		for (const acc of mail.accounts) {
			cmds.push({
				id: `account:${acc.id}`,
				title: `Open inbox — ${acc.email}`,
				keywords: ["account", "inbox", acc.displayName],
				run: () => void mail.selectAccount(acc.id),
			});
		}

		// Folders of the currently-selected account (per-account mode only).
		if (!mail.unified && mail.selectedAccountId) {
			const folders = mail.foldersByAccount[mail.selectedAccountId] ?? [];
			for (const folder of folders) {
				cmds.push({
					id: `folder:${folder.id}`,
					title: `Go to folder — ${displayFolderName(folder.name)}`,
					keywords: ["folder", folder.role],
					run: () => void mail.selectFolder(folder),
				});
			}
		}

		cmds.push({
			id: "compose",
			title: "Compose",
			hint: "c",
			keywords: ["new", "write", "mail"],
			run: () => openCompose(),
		});

		// Reply / reply all require a selected message whose body is loaded — the
		// same precondition openReply enforces.
		const selected = mail.selectedMessage;
		if (selected && mail.body?.id === selected.id) {
			cmds.push({
				id: "reply",
				title: "Reply",
				hint: "r",
				keywords: ["respond"],
				run: () => openReply(false),
			});
			cmds.push({
				id: "reply-all",
				title: "Reply all",
				keywords: ["respond", "everyone"],
				run: () => openReply(true),
			});
		}

		if (selected) {
			cmds.push({
				id: "toggle-read",
				title: "Toggle read",
				hint: "u",
				keywords: ["unread", "seen", "mark"],
				run: () => void mail.toggleRead(selected),
			});
			cmds.push({
				id: "archive-message",
				title: "Archive message",
				hint: "a",
				keywords: ["archive"],
				run: () => void mail.archiveMessage(selected),
			});
			cmds.push({
				id: "delete-message",
				title: "Delete message",
				hint: "d",
				keywords: ["trash", "remove", "delete"],
				run: () => void mail.deleteMessage(selected),
			});
			cmds.push({
				id: "toggle-flag",
				title: selected.flagged ? "Remove flag" : "Flag message",
				hint: "f",
				keywords: ["flag", "star", "important"],
				run: () => void mail.toggleFlagged(selected),
			});
			cmds.push({
				id: "move-message",
				title: "Move message…",
				hint: "m",
				keywords: ["move", "folder"],
				run: () => openMovePicker(),
			});
		}

		if (!mail.unified && mail.selectedFolderId !== null) {
			cmds.push({
				id: "sync-folder",
				title: "Sync current folder",
				keywords: ["refresh", "fetch"],
				run: () => void mail.syncSelectedFolder(),
			});
		}

		for (const acc of mail.accounts) {
			cmds.push({
				id: `sync-account:${acc.id}`,
				title: `Sync account — ${acc.email}`,
				keywords: ["refresh", "fetch"],
				run: () => void mail.syncAccount(acc.id),
			});
		}

		cmds.push({
			id: "add-account",
			title: "Add account",
			keywords: ["new", "imap", "gmail"],
			run: () => (showAddAccount = true),
		});

		cmds.push({
			id: "open-settings",
			title: "Open settings",
			keywords: ["preferences", "config", "options", "remote images"],
			run: () => (showSettings = true),
		});

		cmds.push({
			id: "focus-search",
			title: "Search all mail",
			hint: "/",
			keywords: ["filter", "find", "search", "query"],
			run: () => searchEl?.focus(),
		});

		return cmds;
	}

	function openPalette() {
		paletteCommands = buildPaletteCommands();
		palettePlaceholder = "Run a command…";
		showPalette = true;
	}

	function closePalette() {
		showPalette = false;
		listEl?.focus();
	}

	function closeSettings() {
		showSettings = false;
		queueMicrotask(() => listEl?.focus());
	}

	// Currently selected index for footer display.
	const selectedIndex = $derived(currentIndex());

	function onKeydown(e: KeyboardEvent) {
		// Ctrl/Cmd+K opens the palette from anywhere (including the search input),
		// but never over another modal. Handled before the modifier early-return.
		if ((e.ctrlKey || e.metaKey) && (e.key === "k" || e.key === "K")) {
			if (showAddAccount || showSettings || composeSeed) return;
			e.preventDefault();
			if (!showPalette) openPalette();
			return;
		}
		// Ignore when a modal is open — the palette handles its own keys.
		if (showAddAccount || showSettings || composeSeed || showPalette) return;
		// Let modifier combos through to the browser.
		if (e.metaKey || e.ctrlKey || e.altKey) return;
		// Native controls retain their normal Enter/Space/arrow behavior.
		if (isInteractiveTarget(e.target)) return;

		// '/' focuses search input from anywhere (even if target is body).
		if (e.key === "/" && !isEditableTarget(e.target)) {
			e.preventDefault();
			searchEl?.focus();
			return;
		}

		// Inside search input: Escape blurs, everything else passes through.
		if (isEditableTarget(e.target)) return;

		const idx = currentIndex();

		switch (e.key) {
			case "j":
				e.preventDefault();
				selectIndex(idx < 0 ? 0 : idx + 1);
				break;
			case "k":
				e.preventDefault();
				selectIndex(idx < 0 ? 0 : idx - 1);
				break;
			case "G":
				e.preventDefault();
				void jumpToBottom();
				break;
			case "g":
				e.preventDefault();
				// gg -> top
				if (Date.now() - lastG < 500) {
					selectIndex(0);
					lastG = 0;
				} else {
					lastG = Date.now();
				}
				break;
			case "Enter":
				e.preventDefault();
				if (mail.selectedMessage) readerEl?.focus();
				break;
			case "Escape":
				e.preventDefault();
				listEl?.focus();
				break;
			case "u":
				e.preventDefault();
				if (mail.selectedMessage) void mail.toggleRead(mail.selectedMessage);
				break;
			case "c":
				e.preventDefault();
				openCompose();
				break;
			case "r":
				e.preventDefault();
				openReply();
				break;
			case "a":
				e.preventDefault();
				if (mail.selectedMessage) void mail.archiveMessage(mail.selectedMessage);
				break;
			case "d":
				e.preventDefault();
				if (mail.selectedMessage) void mail.deleteMessage(mail.selectedMessage);
				break;
			case "f":
				e.preventDefault();
				if (mail.selectedMessage) void mail.toggleFlagged(mail.selectedMessage);
				break;
			case "m":
				e.preventDefault();
				openMovePicker();
				break;
			case "1":
				e.preventDefault();
				void mail.selectUnified();
				break;
			default:
				// Digit keys 2–9: select account by rail order index.
				if (/^[2-9]$/.test(e.key)) {
					e.preventDefault();
					const accIdx = parseInt(e.key, 10) - 2;
					const acc = mail.accounts[accIdx];
					if (acc) void mail.selectAccount(acc.id);
				}
				break;
		}

		if (e.key !== "g") lastG = 0;
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div class="app">
	{#if !mail.loadingAccounts && mail.accounts.length === 0}
		<div class="welcome">
			<div class="welcome-inner">
				<h1>Cosmic Mail</h1>
				<p>No accounts yet. Add one to get started.</p>
				<button class="primary" onclick={() => (showAddAccount = true)}>
					Add account
				</button>
			</div>
		</div>
	{:else}
		<div class="rail-col">
			<Rail
				onAddAccount={() => (showAddAccount = true)}
				onCompose={openCompose}
				onOpenSettings={() => (showSettings = true)}
			/>
		</div>

		<div class="main">
			<Header bind:searchEl />

			<div class="split">
				<FolderColumn />

				<div
					class="panes"
					use:observePaneContainer
					style:--list-pane-width={`${listPaneWidth}px`}
				>
					<div class="list-col">
						<MessageList bind:listEl />
					</div>

					<!-- The ARIA separator pattern is intentionally focusable and interactive. -->
					<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
					<div
						class="pane-divider"
						class:dragging={draggingDivider}
						use:paneDivider
						role="separator"
						aria-label="Resize message list and reader"
						aria-orientation="vertical"
						aria-valuemin={Math.round(minimumListPaneWidth)}
						aria-valuemax={Math.round(maximumListPaneWidth)}
						aria-valuenow={Math.round(listPaneWidth)}
						tabindex="0"
						title="Drag to resize · double-click to reset"
					></div>

					<div class="reader-col">
						<Reader
							bind:paneEl={readerEl}
							onReply={() => openReply()}
							onReplyAll={() => openReply(true)}
							onMove={openMovePicker}
						/>
					</div>
				</div>
			</div>

			<Footer {selectedIndex} />
		</div>
	{/if}
</div>

<Toasts />

{#if showAddAccount}
	<AddAccountModal onClose={() => (showAddAccount = false)} />
{/if}

{#if showSettings}
	<SettingsModal onClose={closeSettings} />
{/if}

{#if composeSeed}
	<ComposeModal seed={composeSeed} onClose={closeCompose} />
{/if}

{#if showPalette}
	<CommandPalette commands={paletteCommands} placeholder={palettePlaceholder} onClose={closePalette} />
{/if}

<style>
	.app {
		display: grid;
		grid-template-columns: 52px 1fr;
		grid-template-rows: minmax(0, 1fr);
		height: 100vh;
		width: 100vw;
		overflow: hidden;
	}

	.rail-col {
		grid-column: 1;
		grid-row: 1;
		min-height: 0;
		overflow: hidden;
	}

	/* Full-width welcome state (spans both columns) */
	.app > .welcome {
		grid-column: 1 / -1;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.main {
		grid-column: 2;
		grid-row: 1;
		display: grid;
		grid-template-rows: auto 1fr auto;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	/* The split row: optional fixed folder column + all remaining pane space. */
	.split {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.panes {
		/* Keep the panes in the flexible track when the optional folder column is absent. */
		grid-column: 2;
		display: grid;
		grid-template-columns: var(--list-pane-width) 9px minmax(0, 1fr);
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.list-col {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.pane-divider {
		position: relative;
		z-index: 3;
		width: 9px;
		min-height: 0;
		cursor: col-resize;
		touch-action: none;
		outline: none;
		background: transparent;
		border: 0;
		border-radius: 0;
		padding: 0;
	}

	.pane-divider:hover {
		background: transparent;
		border-color: transparent;
	}

	.pane-divider::before {
		content: "";
		position: absolute;
		inset: 0 4px;
		background: var(--border);
		transition: background 100ms ease, box-shadow 100ms ease;
	}

	.pane-divider:hover::before,
	.pane-divider:focus-visible::before,
	.pane-divider.dragging::before {
		background: var(--accent);
		box-shadow: 0 0 0 1px var(--accent-dim);
	}

	.reader-col {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.welcome-inner {
		text-align: center;
		display: flex;
		flex-direction: column;
		gap: 10px;
		align-items: center;
	}

	.welcome-inner h1 {
		margin: 0;
		font-size: 22px;
		letter-spacing: 0.5px;
	}

	.welcome-inner p {
		margin: 0;
		color: var(--muted);
	}

	.welcome-inner button {
		margin-top: 6px;
		padding: 8px 18px;
	}
</style>
