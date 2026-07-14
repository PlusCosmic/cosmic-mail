<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";

	let { onClose }: { onClose: () => void } = $props();

	let firstControlEl = $state<HTMLInputElement>();

	// Toggle persists immediately (optimistic; the store rolls back on failure).
	function toggleRemoteImages(value: boolean) {
		void mail.updateSettings({
			...mail.settings,
			alwaysDownloadRemoteImages: value,
		});
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === "Escape") {
			e.stopPropagation();
			onClose();
		}
	}

	$effect(() => {
		firstControlEl?.focus();
	});
</script>

<svelte:window onkeydown={onKeydown} />

<div
	class="backdrop"
	role="button"
	tabindex="-1"
	onclick={onClose}
	onkeydown={() => {}}
>
	<div
		class="modal"
		role="dialog"
		aria-modal="true"
		aria-label="Settings"
		tabindex="-1"
		onclick={(e) => e.stopPropagation()}
		onkeydown={() => {}}
	>
		<div class="modal-head">
			<span class="title">Settings</span>
			<button class="x" aria-label="Close" onclick={onClose}>×</button>
		</div>

		<div class="body">
			<div class="section">
				<span class="section-title">Privacy</span>
				<label class="row">
					<input
						bind:this={firstControlEl}
						type="checkbox"
						checked={mail.settings.alwaysDownloadRemoteImages}
						onchange={(e) => toggleRemoteImages(e.currentTarget.checked)}
					/>
					<span class="row-text">
						<span class="row-label">Always download remote images</span>
						<span class="row-desc">
							Load HTTP(S) images in HTML messages without asking each time.
							Message sanitization and iframe sandboxing stay on; only images
							are affected — scripts, forms, and other remote resources remain
							blocked.
						</span>
					</span>
				</label>
			</div>
		</div>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: color-mix(in srgb, var(--c0) 65%, transparent);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 200;
	}
	.modal {
		width: 460px;
		max-width: calc(100vw - 32px);
		max-height: calc(100vh - 48px);
		overflow: auto;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
	}
	.modal-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 12px;
		border-bottom: 1px solid var(--border);
	}
	.title {
		font-weight: 600;
	}
	.x {
		border: none;
		background: transparent;
		font-size: 18px;
		line-height: 1;
		padding: 0 4px;
		color: var(--muted);
	}
	.x:hover {
		background: transparent;
		color: var(--fg);
	}
	.body {
		padding: 14px 16px;
	}
	.section {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.section-title {
		text-transform: uppercase;
		letter-spacing: 0.4px;
		font-size: 11px;
		color: var(--muted);
	}
	.row {
		display: flex;
		align-items: flex-start;
		gap: 10px;
		cursor: pointer;
	}
	.row input[type="checkbox"] {
		margin-top: 2px;
		width: 15px;
		height: 15px;
		flex-shrink: 0;
		accent-color: var(--accent);
		cursor: pointer;
	}
	.row-text {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}
	.row-label {
		font-size: 13px;
		color: var(--fg);
	}
	.row-desc {
		font-size: 11.5px;
		color: var(--muted);
		line-height: 1.5;
	}
</style>
