<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";

	let { searchEl = $bindable() }: { searchEl?: HTMLInputElement } = $props();

	const title = $derived.by(() => {
		if (mail.unified) return "Unified Inbox";
		if (mail.selectedFolder) return mail.selectedFolder.name;
		if (mail.selectedAccountId) {
			const acc = mail.accounts.find((a) => a.id === mail.selectedAccountId);
			return acc ? acc.email : "No folder";
		}
		return "No folder selected";
	});

	const subtitle = $derived.by(() => {
		const n = mail.accounts.length;
		const u = mail.unifiedUnread;
		return `${n} account${n !== 1 ? "s" : ""} · ${u} unread`;
	});

	// Sync indicator: error if any, syncing if any, else synced.
	const syncStatus = $derived.by((): "idle" | "syncing" | "error" => {
		const states = Object.values(mail.syncStates);
		if (states.some((s) => s === "error")) return "error";
		if (states.some((s) => s === "syncing")) return "syncing";
		return "idle";
	});

	function onSearchKeydown(e: KeyboardEvent) {
		if (e.key === "Enter") {
			e.preventDefault();
			// Run a backend full-cache search over the current scope, then blur so
			// j/k navigation works on the results.
			void mail.runSearch(mail.query);
			searchEl?.blur();
		} else if (e.key === "Escape") {
			e.preventDefault();
			// Escape clears an active search back to the normal view; otherwise it
			// just clears the live filter. Either way, blur.
			if (mail.searchActive) void mail.clearSearch();
			mail.query = "";
			searchEl?.blur();
		}
	}
</script>

<div class="header">
	<div class="h-title">
		{#if mail.unified}
			<span class="u"></span>
		{/if}
		{title}
	</div>
	<div class="h-sub">{subtitle}</div>

	{#if mail.searchActive}
		<span class="search-status">
			results for “{mail.searchQuery}” — {mail.visibleMessages.length}{mail.hasMore
				? "+"
				: ""}
			<button type="button" class="clear-search" onclick={() => mail.clearSearch()}>
				clear
			</button>
		</span>
	{/if}

	<div class="search">
		<span class="search-icon"></span>
		<input
			bind:this={searchEl}
			bind:value={mail.query}
			type="text"
			placeholder={mail.searchActive
				? "Search all mail…"
				: "Filter loaded messages…"}
			aria-label="Search messages"
			onkeydown={onSearchKeydown}
		/>
		<span class="kbd">{mail.searchActive ? "⏎" : "/"}</span>
	</div>

	<span class="h-sync">
		<span class="sync-dot" class:syncing={syncStatus === "syncing"} class:error={syncStatus === "error"}></span>
		{#if syncStatus === "syncing"}
			syncing
		{:else if syncStatus === "error"}
			sync error
		{:else}
			synced
		{/if}
	</span>
</div>

<style>
	.header {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 9px 16px;
		background: var(--panel);
		border-bottom: 1px solid var(--border);
		flex-shrink: 0;
	}

	.h-title {
		font-weight: 700;
		letter-spacing: 0.3px;
		white-space: nowrap;
	}

	.u {
		color: var(--accent);
		margin-right: 4px;
	}

	.h-sub {
		color: var(--muted);
		font-size: 11px;
		white-space: nowrap;
	}

	.search-status {
		margin-left: auto;
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font-size: 11px;
		color: var(--accent);
		white-space: nowrap;
	}

	.clear-search {
		background: transparent;
		border: 1px solid var(--border);
		border-radius: 3px;
		color: var(--muted);
		font: inherit;
		font-size: 10px;
		padding: 1px 7px;
		cursor: pointer;
	}

	.clear-search:hover {
		color: var(--fg);
		border-color: var(--accent);
	}

	/* When the search-status element is present it owns the left auto-margin. */
	.search-status + .search {
		margin-left: 0;
	}

	.search {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 260px;
		max-width: 400px;
		flex: 1;
		padding: 6px 11px;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--muted);
	}

	.search-icon {
		font-size: 12px;
		color: var(--muted-2);
	}

	.search input {
		flex: 1;
		background: transparent;
		border: none;
		outline: none;
		color: var(--fg);
		font: inherit;
		font-size: 12px;
		width: 0;
		min-width: 0;
		padding: 0;
	}

	.search input::placeholder {
		color: var(--muted-2);
	}

	.search .kbd {
		font-size: 10px;
		color: var(--muted-2);
		border: 1px solid var(--border);
		border-radius: 3px;
		padding: 0 5px;
		flex-shrink: 0;
	}

	.h-sync {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--muted);
		font-size: 11px;
		white-space: nowrap;
	}

	.sync-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--c2);
		box-shadow: 0 0 5px var(--c2);
	}

	.sync-dot.syncing {
		background: var(--accent);
		box-shadow: 0 0 5px var(--accent);
		animation: pulse 1.2s ease-in-out infinite;
	}

	.sync-dot.error {
		background: var(--c1);
		box-shadow: 0 0 5px var(--c1);
	}

	@keyframes pulse {
		50% {
			opacity: 0.3;
		}
	}
</style>
