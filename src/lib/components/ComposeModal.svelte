<script lang="ts">
	import { onMount, untrack } from "svelte";
	import { mail } from "$lib/stores/mail.svelte";
	import { splitMailboxList, type ComposeSeed } from "$lib/compose";
	import type { SendMessageInput } from "$lib/types";

	let { seed, onClose }: { seed: ComposeSeed; onClose: () => void } = $props();

	let accountId = $state(untrack(() => seed.accountId));
	let to = $state(untrack(() => seed.toAddrs.join(", ")));
	let cc = $state(untrack(() => seed.ccAddrs.join(", ")));
	let bcc = $state("");
	let subject = $state(untrack(() => seed.subject));
	let bodyText = $state(untrack(() => seed.bodyText));
	let busy = $state(false);
	let error = $state<string | null>(null);
	let toInput = $state<HTMLInputElement>();

	onMount(() => toInput?.focus());

	function addresses(value: string): string[] {
		return splitMailboxList(value);
	}

	async function submit(e?: Event) {
		e?.preventDefault();
		if (busy) return;
		error = null;
		const input: SendMessageInput = {
			accountId,
			toAddrs: addresses(to),
			ccAddrs: addresses(cc),
			bccAddrs: addresses(bcc),
			subject,
			bodyText,
			replyToMessageId: seed.replyToMessageId,
		};
		if (input.toAddrs.length + input.ccAddrs.length + input.bccAddrs.length === 0) {
			error = "Add at least one recipient";
			return;
		}

		busy = true;
		try {
			await mail.sendMessage(input);
			onClose();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === "Escape" && !busy) {
			e.preventDefault();
			e.stopPropagation();
			onClose();
		} else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
			e.preventDefault();
			void submit();
		}
	}

	function closeFromBackdrop(e: MouseEvent) {
		if (!busy && e.target === e.currentTarget) onClose();
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div
	class="backdrop"
	role="button"
	tabindex="-1"
	onclick={closeFromBackdrop}
	onkeydown={() => {}}
>
	<div
		class="modal"
		role="dialog"
		aria-modal="true"
		aria-label="Compose message"
	>
		<form class="compose-form" onsubmit={submit}>
		<div class="modal-head">
			<span class="title">{seed.replyToMessageId === null ? "New message" : "Reply"}</span>
			<button type="button" class="x" aria-label="Close" onclick={onClose} disabled={busy}>×</button>
		</div>

		<div class="fields">
			<label>
				<span>From</span>
				<select bind:value={accountId} disabled={busy || seed.replyToMessageId !== null}>
					{#each mail.accounts as account (account.id)}
						<option value={account.id}>{account.displayName || account.email} &lt;{account.email}&gt;</option>
					{/each}
				</select>
			</label>
			<label>
				<span>To</span>
				<input bind:this={toInput} bind:value={to} autocomplete="off" disabled={busy} />
			</label>
			<div class="recipient-row">
				<label>
					<span>Cc</span>
					<input bind:value={cc} autocomplete="off" disabled={busy} />
				</label>
				<label>
					<span>Bcc</span>
					<input bind:value={bcc} autocomplete="off" disabled={busy} />
				</label>
			</div>
			<label>
				<span>Subject</span>
				<input bind:value={subject} autocomplete="off" disabled={busy} />
			</label>
			<label class="body-field">
				<span>Message</span>
				<textarea bind:value={bodyText} disabled={busy} aria-label="Message body"></textarea>
			</label>
		</div>

		{#if error}<div class="error">{error}</div>{/if}

		<div class="actions">
			<span class="hint"><kbd>ctrl</kbd>+<kbd>enter</kbd> send</span>
			<button type="button" onclick={onClose} disabled={busy}>Cancel</button>
			<button type="submit" class="primary" disabled={busy}>
				{busy ? "Sending…" : "Send"}
			</button>
		</div>
		</form>
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
		width: 680px;
		max-width: calc(100vw - 24px);
		height: min(720px, calc(100vh - 32px));
		display: flex;
		flex-direction: column;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
	}
	.compose-form { display: contents; }
	.modal-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 12px;
		border-bottom: 1px solid var(--border);
	}
	.title { font-weight: 600; }
	.x {
		border: none;
		background: transparent;
		font-size: 18px;
		line-height: 1;
		padding: 0 4px;
		color: var(--muted);
	}
	.x:hover { background: transparent; color: var(--fg); }
	.fields {
		padding: 14px 16px 0;
		display: flex;
		flex: 1;
		min-height: 0;
		flex-direction: column;
		gap: 9px;
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 3px;
		font-size: 11px;
		color: var(--muted);
	}
	label > span {
		text-transform: uppercase;
		letter-spacing: 0.4px;
	}
	.recipient-row { display: grid; grid-template-columns: 1fr 1fr; gap: 9px; }
	.body-field { flex: 1; min-height: 140px; }
	textarea {
		flex: 1;
		width: 100%;
		resize: none;
		font: inherit;
		line-height: 1.55;
		color: var(--fg);
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 9px;
	}
	textarea:focus { outline: none; border-color: var(--accent); }
	.error {
		margin: 10px 16px 0;
		padding: 8px 10px;
		border: 1px solid var(--c1);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--c1) 15%, var(--bg));
		font-size: 12px;
		word-break: break-word;
	}
	.actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
		padding: 12px 16px 14px;
	}
	.hint { margin-right: auto; color: var(--muted); font-size: 10px; }
	@media (max-width: 600px) {
		.backdrop { align-items: stretch; }
		.modal { width: 100%; height: 100%; max-width: none; border-radius: 0; }
		.recipient-row { grid-template-columns: 1fr; }
	}
</style>
