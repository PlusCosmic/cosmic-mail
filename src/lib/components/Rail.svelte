<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";
	import { initials, avatarColor } from "$lib/format";

	let { onAddAccount, onCompose }: { onAddAccount: () => void; onCompose: () => void } = $props();

	function capBadge(n: number): string {
		return n > 99 ? "99+" : n > 0 ? String(n) : "";
	}
</script>

<nav class="rail" aria-label="Account switcher">
	<!-- Logo -->
	<div class="logo" aria-hidden="true">C</div>

	<!-- Unified inbox button -->
	<div
		class="rail-btn"
		class:active={mail.unified}
		role="button"
		tabindex="0"
		title="Unified inbox"
		onclick={() => mail.selectUnified()}
		onkeydown={(e) => e.key === "Enter" && mail.selectUnified()}
		aria-label="Unified inbox"
		aria-pressed={mail.unified}
	>
		<span class="av" style="background:var(--c5)"></span>
		{#if mail.unifiedUnread > 0}
			<span class="count" aria-label="{mail.unifiedUnread} unread">{capBadge(mail.unifiedUnread)}</span>
		{/if}
		<span class="tip">Unified inbox</span>
	</div>

	<!-- Per-account buttons -->
	{#each mail.accounts as acc (acc.id)}
		{@const unread = mail.accountUnread(acc.id)}
		{@const ini = initials(acc.displayName, acc.email)}
		{@const color = avatarColor(acc.email)}
		<div
			class="rail-btn"
			class:active={!mail.unified && mail.selectedAccountId === acc.id}
			role="button"
			tabindex="0"
			title="{acc.email} — {acc.kind}"
			onclick={() => mail.selectAccount(acc.id)}
			onkeydown={(e) => e.key === "Enter" && mail.selectAccount(acc.id)}
			aria-label="{acc.email} — {acc.kind}"
			aria-pressed={!mail.unified && mail.selectedAccountId === acc.id}
		>
			<span class="av" style="background:{color}">{ini}</span>
			{#if unread > 0}
				<span class="count" aria-label="{unread} unread">{capBadge(unread)}</span>
			{/if}
			<span class="tip">{acc.email} — {acc.kind}</span>
		</div>
	{/each}

	<div class="spacer"></div>

	<!-- Compose -->
	<div
		class="rail-btn"
		role="button"
		tabindex="0"
		title="Compose (c)"
		onclick={onCompose}
		onkeydown={(e) => e.key === "Enter" && onCompose()}
		aria-label="Compose message"
	>
		<span class="av compose">✎</span>
		<span class="tip">Compose (c)</span>
	</div>

	<!-- Add account -->
	<div
		class="rail-btn"
		role="button"
		tabindex="0"
		title="Add account"
		onclick={onAddAccount}
		onkeydown={(e) => e.key === "Enter" && onAddAccount()}
		aria-label="Add account"
	>
		<span class="av add">+</span>
		<span class="tip">Add account</span>
	</div>
</nav>

<style>
	.rail {
		background: var(--rail);
		border-right: 1px solid var(--border);
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		padding: 10px 0;
		width: 52px;
		min-width: 52px;
		height: 100%;
		overflow: hidden;
	}

	.logo {
		width: 30px;
		height: 30px;
		border-radius: 6px;
		display: grid;
		place-items: center;
		margin-bottom: 8px;
		background: var(--accent);
		color: var(--bg);
		font-weight: 700;
		font-size: 15px;
	}

	.rail-btn {
		position: relative;
		width: 34px;
		height: 34px;
		border-radius: 6px;
		display: grid;
		place-items: center;
		color: var(--muted);
		font-size: 15px;
		cursor: pointer;
		border: 1px solid transparent;
		user-select: none;
	}

	.rail-btn .av {
		width: 26px;
		height: 26px;
		border-radius: 5px;
		display: grid;
		place-items: center;
		color: var(--bg);
		font-weight: 700;
		font-size: 12px;
	}

	.rail-btn .av.compose,
	.rail-btn .av.add {
		background: var(--panel-2);
		color: var(--muted);
		font-size: 14px;
	}

	.rail-btn:hover {
		background: var(--hover);
		color: var(--fg);
	}

	.rail-btn.active {
		background: var(--sel-bg);
	}

	.rail-btn.active::before {
		content: "";
		position: absolute;
		left: -10px;
		top: 7px;
		bottom: 7px;
		width: 3px;
		border-radius: 2px;
		background: var(--accent);
	}

	.rail-btn .count {
		position: absolute;
		top: -2px;
		right: -2px;
		min-width: 15px;
		height: 15px;
		padding: 0 3px;
		background: var(--c1);
		color: var(--bg);
		font-size: 9px;
		font-weight: 700;
		border-radius: 8px;
		display: grid;
		place-items: center;
		border: 2px solid var(--rail);
	}

	.rail-btn .tip {
		position: absolute;
		left: 44px;
		top: 50%;
		transform: translateY(-50%);
		white-space: nowrap;
		background: var(--panel-2);
		color: var(--fg);
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 3px 9px;
		font-size: 11px;
		opacity: 0;
		pointer-events: none;
		transition: opacity 0.12s;
		z-index: 20;
	}

	.rail-btn:hover .tip {
		opacity: 1;
	}

	.spacer {
		flex: 1;
	}
</style>
