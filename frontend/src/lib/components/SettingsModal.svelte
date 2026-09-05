<script lang="ts">
	import { onDestroy } from "svelte";
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

	// Removing an account is destructive (wipes its cached mail on disk), so it
	// takes two clicks: the first arms a "Confirm remove" state for a given
	// account id, the second within the window actually removes it. Clicking
	// elsewhere, or letting it time out, disarms without side effects.
	let pendingRemovalId = $state<string | null>(null);
	let pendingRemovalTimer: ReturnType<typeof setTimeout> | undefined;

	function disarmRemoval() {
		pendingRemovalId = null;
		if (pendingRemovalTimer !== undefined) {
			clearTimeout(pendingRemovalTimer);
			pendingRemovalTimer = undefined;
		}
	}

	function requestRemoveAccount(accountId: string) {
		if (pendingRemovalId === accountId) {
			disarmRemoval();
			void mail.removeAccount(accountId);
			return;
		}
		pendingRemovalId = accountId;
		if (pendingRemovalTimer !== undefined) clearTimeout(pendingRemovalTimer);
		pendingRemovalTimer = setTimeout(disarmRemoval, 4000);
	}

	onDestroy(disarmRemoval);

	// Reconnect blocks on the interactive browser consent (up to 5 minutes
	// backend-side), so only one flow runs at a time.
	let reauthingId = $state<string | null>(null);

	async function reconnectAccount(accountId: string) {
		if (reauthingId !== null) return;
		reauthingId = accountId;
		try {
			await mail.reauthAccount(accountId);
		} finally {
			reauthingId = null;
		}
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

			<div class="section">
				<span class="section-title">Accounts</span>
				<ul class="accounts">
					{#each mail.accounts as acc (acc.id)}
						<li class="account-row">
							<span class="account-info">
								<span class="account-email">{acc.email}</span>
								<span class="account-kind">
									{acc.kind}{#if mail.reauthRequired[acc.id]}<span class="expired-hint">
											· sign-in expired</span
										>{/if}
								</span>
							</span>
							{#if pendingRemovalId === acc.id}
								<span class="confirm-group">
									<button
										type="button"
										class="confirm-remove"
										onclick={() => requestRemoveAccount(acc.id)}
									>
										Confirm remove
									</button>
									<button type="button" class="cancel-remove" onclick={disarmRemoval}>
										Cancel
									</button>
								</span>
							{:else}
								<span class="account-actions">
									{#if acc.kind === "gmail"}
										<button
											type="button"
											class="reconnect-account"
											class:attention={mail.reauthRequired[acc.id]}
											disabled={reauthingId !== null}
											onclick={() => reconnectAccount(acc.id)}
										>
											{reauthingId === acc.id ? "Waiting for Google…" : "Reconnect"}
										</button>
									{/if}
									<button
										type="button"
										class="remove-account"
										onclick={() => requestRemoveAccount(acc.id)}
									>
										Remove
									</button>
								</span>
							{/if}
						</li>
					{:else}
						<li class="account-empty">No accounts configured.</li>
					{/each}
				</ul>
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
	.section + .section {
		margin-top: 18px;
		padding-top: 14px;
		border-top: 1px solid var(--border);
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

	.accounts {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.account-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 6px 8px;
		border: 1px solid var(--border);
		border-radius: 6px;
		background: var(--panel-2);
	}
	.account-info {
		display: flex;
		flex-direction: column;
		gap: 1px;
		overflow: hidden;
	}
	.account-email {
		font-size: 12.5px;
		color: var(--fg);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.account-kind {
		font-size: 10.5px;
		color: var(--muted);
		text-transform: uppercase;
		letter-spacing: 0.3px;
	}
	.account-empty {
		font-size: 11.5px;
		color: var(--muted);
		font-style: italic;
	}
	.expired-hint {
		color: var(--c1);
		text-transform: none;
	}
	.account-actions {
		display: flex;
		gap: 6px;
		flex-shrink: 0;
	}
	.reconnect-account,
	.remove-account,
	.confirm-remove,
	.cancel-remove {
		flex-shrink: 0;
		font: inherit;
		font-size: 11px;
		padding: 4px 10px;
		border-radius: 4px;
		border: 1px solid var(--border);
		background: transparent;
		color: var(--muted);
		cursor: pointer;
	}
	.remove-account:hover {
		color: var(--c1);
		border-color: color-mix(in srgb, var(--c1) 45%, transparent);
	}
	.reconnect-account:hover:not(:disabled) {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 45%, transparent);
	}
	.reconnect-account.attention {
		color: var(--accent);
		border-color: var(--accent);
		font-weight: 600;
	}
	.reconnect-account:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.confirm-group {
		display: flex;
		gap: 6px;
		flex-shrink: 0;
	}
	.confirm-remove {
		color: var(--bg);
		background: var(--c1);
		border-color: var(--c1);
		font-weight: 600;
	}
	.confirm-remove:hover {
		background: color-mix(in srgb, var(--c1) 85%, var(--fg));
	}
	.cancel-remove:hover {
		color: var(--fg);
	}
</style>
