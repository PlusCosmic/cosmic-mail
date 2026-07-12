<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";
	import { relativeDate, initials, avatarColor } from "$lib/format";
	import type { MessageSummary } from "$lib/types";

	// Bindable so the parent keydown handler can scroll the active row into view.
	let { listEl = $bindable() }: { listEl?: HTMLElement } = $props();

	const syncing = $derived(
		mail.unified
			? Object.values(mail.syncStates).some((state) => state === "syncing")
			: mail.selectedAccountId !== null &&
				mail.syncStates[mail.selectedAccountId] === "syncing",
	);

	function open(m: MessageSummary) {
		void mail.selectMessage(m);
	}

	function setFilter(f: "all" | "unread" | "flagged") {
		mail.filter = f;
	}

	// Short account label for the meta row (local part of email).
	function shortAccount(accountId: string): string {
		const acc = mail.accounts.find((a) => a.id === accountId);
		if (!acc) return accountId;
		const at = acc.email.indexOf("@");
		return at > 0 ? acc.email.slice(0, at) : acc.email;
	}

	function accountDotColor(accountId: string): string {
		const acc = mail.accounts.find((a) => a.id === accountId);
		return acc ? avatarColor(acc.email) : "var(--muted)";
	}
</script>

<div class="list-pane">
	<!-- Sticky header with filter chips -->
	<div class="list-head">
		<div class="filters">
			<button
				class="chip"
				class:on={mail.filter === "all"}
				onclick={() => setFilter("all")}
			>All</button>
			<button
				class="chip"
				class:on={mail.filter === "unread"}
				onclick={() => setFilter("unread")}
			>Unread</button>
			<button
				class="chip"
				class:on={mail.filter === "flagged"}
				onclick={() => setFilter("flagged")}
			>Flagged</button>
		</div>
		<span class="count">{mail.visibleMessages.length}{mail.hasMore ? "+" : ""} messages</span>
	</div>

	<div class="rows" bind:this={listEl} role="listbox" tabindex="-1" aria-label="Messages">
		{#if !mail.unified && mail.selectedFolderId === null}
			<div class="empty">Select a folder</div>
		{:else if mail.visibleMessages.length === 0 && (mail.loadingMessages || syncing)}
			<div class="empty">Syncing messages…</div>
		{:else if mail.visibleMessages.length === 0}
			{#if mail.messages.length === 0}
				<div class="empty">No messages</div>
			{:else}
				<div class="empty">No messages match the filter</div>
			{/if}
		{:else}
			{#each mail.visibleMessages as m (m.id)}
				{@const ini = initials(m.fromName, m.fromAddr)}
				{@const color = avatarColor(m.fromAddr || m.fromName)}
				<button
					class="msg"
					class:selected={mail.selectedMessageId === m.id}
					class:unread={!m.seen}
					role="option"
					aria-selected={mail.selectedMessageId === m.id}
					data-msg-id={m.id}
					onclick={() => open(m)}
				>
					<span class="dot" aria-hidden="true"></span>
					<span class="av" style="background:{color}">{ini}</span>
					<span class="from">{m.fromName || m.fromAddr || "(unknown)"}</span>
					<span class="when">{relativeDate(m.date)}</span>
					<span class="subject">{m.subject || "(no subject)"}</span>
					<span class="snippet">{m.snippet}</span>
					<!-- Meta row: account label in unified mode, flag indicator -->
					<span class="acctdot">
						{#if mail.unified}
							<span class="src">
								<i style="background:{accountDotColor(m.accountId)}"></i>
								{shortAccount(m.accountId)}
							</span>
						{/if}
						{#if m.flagged}<span class="flag" title="Flagged">★</span>{/if}
					</span>
				</button>
			{/each}

			{#if mail.hasMore}
				<button
					class="load-more"
					onclick={() => mail.loadMore()}
					disabled={mail.loadingMessages}
				>
					{mail.loadingMessages ? "Loading…" : "Load more"}
				</button>
			{/if}
		{/if}
	</div>
</div>

<style>
	.list-pane {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg);
		overflow: hidden;
	}

	.list-head {
		position: sticky;
		top: 0;
		z-index: 2;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 14px;
		background: var(--panel);
		border-bottom: 1px solid var(--border);
		font-size: 11px;
		color: var(--muted);
		flex-shrink: 0;
	}

	.filters {
		display: flex;
		gap: 4px;
	}

	.chip {
		padding: 2px 9px;
		border-radius: 10px;
		border: 1px solid var(--border);
		color: var(--muted);
		font-size: 11px;
		cursor: pointer;
		background: transparent;
		font-family: inherit;
	}

	.chip.on {
		background: var(--accent-soft);
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 40%, transparent);
	}

	.chip:hover:not(.on) {
		background: var(--hover);
	}

	.count {
		margin-left: auto;
		color: var(--muted);
		font-size: 11px;
	}

	.rows {
		flex: 1;
		overflow-y: auto;
		outline: none;
	}

	/* Message row grid: dot | avatar | from | time (row1), subject (row2), snippet (row3), meta (row4) */
	.msg {
		display: grid;
		grid-template-columns: 9px 26px 1fr auto;
		gap: 3px 10px;
		padding: 9px 14px 9px 12px;
		border-bottom: 1px solid var(--border-soft);
		cursor: pointer;
		align-items: start;
		width: 100%;
		text-align: left;
		background: transparent;
		border-radius: 0;
		font: inherit;
		color: var(--fg);
	}

	.msg:hover {
		background: var(--hover);
	}

	.msg.selected {
		background: var(--sel-bg);
		color: var(--sel-fg);
	}

	.dot {
		grid-row: 1 / span 2;
		align-self: center;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: transparent;
	}

	.msg.unread .dot {
		background: var(--accent);
	}

	.av {
		grid-row: 1 / span 2;
		align-self: center;
		width: 24px;
		height: 24px;
		border-radius: 4px;
		display: grid;
		place-items: center;
		color: var(--bg);
		font-weight: 700;
		font-size: 11px;
	}

	.from {
		font-weight: 500;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.msg.unread .from {
		font-weight: 700;
	}

	.when {
		color: var(--muted);
		font-size: 11px;
		white-space: nowrap;
	}

	.msg.selected .when {
		color: color-mix(in srgb, var(--sel-fg) 78%, transparent);
	}

	.subject {
		grid-column: 3 / span 2;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 12.5px;
	}

	.msg.unread .subject {
		font-weight: 600;
	}

	.snippet {
		grid-column: 3 / span 2;
		color: var(--muted);
		font-size: 11.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.msg.selected .snippet {
		color: color-mix(in srgb, var(--sel-fg) 78%, transparent);
	}

	.acctdot {
		grid-column: 3 / span 2;
		margin-top: 2px;
		display: flex;
		align-items: center;
		gap: 7px;
		font-size: 10px;
		color: var(--muted);
		min-height: 0;
	}

	.src {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.src i {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		font-style: normal;
		flex-shrink: 0;
	}

	.flag {
		color: var(--c3);
	}

	.empty {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 120px;
		color: var(--faint);
		font-style: italic;
		font-size: 12px;
	}

	.load-more {
		display: block;
		width: 100%;
		border: none;
		border-radius: 0;
		background: var(--panel);
		color: var(--muted);
		padding: 8px;
		font: inherit;
		cursor: pointer;
		border-top: 1px solid var(--border-soft);
	}

	.load-more:hover {
		background: var(--hover);
	}
</style>
