<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";
</script>

<div class="toasts" aria-live="polite">
	{#each mail.toasts as t (t.id)}
		<div class="toast {t.kind}" role="status">
			<span class="msg">{t.message}</span>
			<button
				class="x"
				aria-label="Dismiss"
				onclick={() => mail.dismissToast(t.id)}>×</button
			>
		</div>
	{/each}
</div>

<style>
	.toasts {
		position: fixed;
		bottom: 12px;
		right: 12px;
		display: flex;
		flex-direction: column;
		gap: 6px;
		z-index: 100;
		max-width: 360px;
	}
	.toast {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		background: var(--panel-2);
		border: 1px solid var(--border-strong);
		border-left: 3px solid var(--accent);
		border-radius: var(--radius);
		padding: 8px 10px;
		font-size: 12px;
	}
	.toast.error {
		border-left-color: var(--c1);
	}
	.msg {
		flex: 1;
		word-break: break-word;
	}
	.x {
		flex: 0 0 auto;
		border: none;
		background: transparent;
		padding: 0 2px;
		line-height: 1;
		color: var(--muted);
		font-size: 15px;
	}
	.x:hover {
		background: transparent;
		color: var(--fg);
	}
</style>
