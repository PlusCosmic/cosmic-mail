<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";
	import type { SyncState } from "$lib/types";

	let { selectedIndex = 0 }: { selectedIndex: number } = $props();

	function syncDotClass(s: SyncState): string {
		return s === "syncing" ? "syncing" : s === "error" ? "error" : "idle";
	}

	function shortEmail(email: string): string {
		const at = email.indexOf("@");
		return at > 0 ? email.slice(0, at) : email;
	}

	const metaLabel = $derived.by(() => {
		const total = mail.visibleMessages.length;
		const pos = total > 0 ? selectedIndex + 1 : 0;
		const label = mail.unified
			? "unified"
			: mail.accounts.find((a) => a.id === mail.selectedAccountId)?.email ?? "—";
		return `message ${pos} of ${total}${mail.hasMore ? "+" : ""} · ${label}`;
	});
</script>

<footer class="footer">
	<span class="fh"><kbd>j</kbd><kbd>k</kbd><span class="lbl">navigate</span></span>
	<span class="fh"><kbd>enter</kbd><span class="lbl">open</span></span>
	<span class="fh"><kbd>u</kbd><span class="lbl">toggle unread</span></span>
	<span class="fh"><kbd>c</kbd><span class="lbl">compose</span></span>
	<span class="fh"><kbd>r</kbd><span class="lbl">reply</span></span>
	<span class="fh"><kbd>/</kbd><span class="lbl">filter</span></span>
	<span class="fh"><kbd>g</kbd><kbd>g</kbd><span class="lbl">top</span></span>
	<span class="fh"><kbd>G</kbd><span class="lbl">bottom</span></span>
	<span class="fh"><kbd>1</kbd><span class="lbl">unified</span></span>
	<span class="fh"><kbd>2</kbd>…<span class="lbl">accounts</span></span>

	<span class="fspacer"></span>

	<!-- Per-account sync dots -->
	{#each mail.accounts as acc (acc.id)}
		{@const state = mail.syncStates[acc.id] ?? "idle"}
		<span class="acc-dot" title="{acc.email}: {state}">
			<span class="dot {syncDotClass(state)}"></span>
			<span class="acc-label">{shortEmail(acc.email)}</span>
		</span>
	{/each}

	<span class="fmeta">{metaLabel}</span>
</footer>

<style>
	.footer {
		display: flex;
		align-items: center;
		gap: 2px;
		flex-wrap: wrap;
		padding: 6px 16px;
		background: var(--panel);
		border-top: 1px solid var(--border);
		color: var(--muted);
		font-size: 11px;
		flex-shrink: 0;
	}

	.fh {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 1px 6px;
	}

	.fh .lbl {
		color: var(--muted);
	}

	.fspacer {
		flex: 1;
	}

	.fmeta {
		color: var(--muted-2);
		white-space: nowrap;
	}

	.acc-dot {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-right: 8px;
		font-size: 11px;
		color: var(--muted-2);
	}

	.dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--faint);
	}

	.dot.idle {
		background: color-mix(in srgb, var(--c2) 70%, var(--bg));
	}

	.dot.syncing {
		background: var(--accent);
		animation: pulse 1.2s ease-in-out infinite;
	}

	.dot.error {
		background: var(--c1);
	}

	.acc-label {
		font-size: 10px;
	}

	@keyframes pulse {
		50% {
			opacity: 0.3;
		}
	}
</style>
