<script lang="ts">
	import * as api from "$lib/api";
	import { mail } from "$lib/stores/mail.svelte";
	import type { DiscoveredConfig, ImapAccountInput } from "$lib/types";

	let { onClose }: { onClose: () => void } = $props();

	let tab = $state<"imap" | "gmail">("imap");
	let busy = $state(false);
	let error = $state<string | null>(null);

	// IMAP form with sensible defaults.
	let form = $state<ImapAccountInput>({
		email: "",
		displayName: "",
		imapHost: "",
		imapPort: 993,
		smtpHost: "",
		smtpPort: 587,
		username: "",
		password: "",
	});

	// --- Autodiscovery state ---
	let detecting = $state(false);
	// Result of the last discovery, drives the inline status line.
	let discovered = $state<DiscoveredConfig | null>(null);
	// The email we last ran discovery for (avoids redundant re-runs).
	let discoveredFor = $state<string | null>(null);
	// True once the user has typed into any host/port/username field, so a later
	// email edit won't clobber their manual input. Re-detect is allowed while
	// this is false (untouched) or immediately after we autofilled the fields.
	let hostFieldsTouched = $state(false);

	function looksLikeEmail(v: string): boolean {
		const s = v.trim();
		const at = s.indexOf("@");
		return at > 0 && at < s.length - 1 && !s.includes(" ");
	}

	function markHostFieldsTouched() {
		hostFieldsTouched = true;
	}

	// Fill host/port/username fields from a discovery result.
	function applyDiscovery(cfg: DiscoveredConfig) {
		form.imapHost = cfg.imapHost;
		form.imapPort = cfg.imapPort;
		form.smtpHost = cfg.smtpHost;
		form.smtpPort = cfg.smtpPort;
		if (cfg.username) form.username = cfg.username;
		// These values came from us, not the user.
		hostFieldsTouched = false;
	}

	async function runDiscovery() {
		const email = form.email.trim();
		if (!looksLikeEmail(email)) return;
		// Don't clobber fields the user has hand-edited.
		if (hostFieldsTouched) return;
		// Skip if we already have a result for this exact address.
		if (discoveredFor === email && discovered) return;

		detecting = true;
		try {
			const cfg = await api.discoverAccountConfig(email);
			discovered = cfg;
			discoveredFor = email;
			if (cfg.kind === "gmail") {
				// Steer to Gmail; don't autofill IMAP fields.
			} else {
				applyDiscovery(cfg);
			}
		} catch {
			// Discovery failure is silent — keep manual entry.
			discovered = null;
			discoveredFor = null;
		} finally {
			detecting = false;
		}
	}

	function onEmailBlur() {
		void runDiscovery();
	}

	function onEmailKeydown(e: KeyboardEvent) {
		if (e.key === "Enter") {
			e.preventDefault();
			void runDiscovery();
		}
	}

	// If the user changes the email after a result, drop the stale status
	// (unless they've hand-edited hosts, in which case we stay out of the way).
	function onEmailInput() {
		if (discoveredFor !== null && form.email.trim() !== discoveredFor) {
			discovered = null;
			discoveredFor = null;
		}
	}

	function switchToGmailTab() {
		tab = "gmail";
	}

	// Human-readable status line for a non-gmail discovery result.
	let statusText = $derived.by(() => {
		if (!discovered || discovered.kind === "gmail") return null;
		switch (discovered.source) {
			case "ispdb":
				return { tone: "ok", text: "Settings found (Thunderbird ISPDB)" };
			case "autoconfig":
				return { tone: "ok", text: "Settings found (provider autoconfig)" };
			case "mx":
				return { tone: "ok", text: "Settings found (via MX lookup)" };
			case "srv":
				return { tone: "ok", text: "Settings found (DNS SRV)" };
			case "guess":
				return {
					tone: "warn",
					text: "Could not verify — guessed common defaults, please check",
				};
		}
	});

	async function submitImap(e: Event) {
		e.preventDefault();
		error = null;
		busy = true;
		try {
			const input: ImapAccountInput = {
				...form,
				// Username often equals the email if left blank.
				username: form.username.trim() || form.email.trim(),
			};
			await mail.addImapAccount(input);
			onClose();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	async function gmail() {
		error = null;
		busy = true;
		try {
			await mail.startGmailOauth();
			onClose();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === "Escape" && !busy) {
			e.stopPropagation();
			onClose();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div
	class="backdrop"
	role="button"
	tabindex="-1"
	onclick={() => !busy && onClose()}
	onkeydown={() => {}}
>
	<div
		class="modal"
		role="dialog"
		aria-modal="true"
		aria-label="Add account"
		tabindex="-1"
		onclick={(e) => e.stopPropagation()}
		onkeydown={() => {}}
	>
		<div class="modal-head">
			<span class="title">Add account</span>
			<button class="x" aria-label="Close" onclick={onClose} disabled={busy}>×</button>
		</div>

		<div class="tabs">
			<button class="tab" class:active={tab === "imap"} onclick={() => (tab = "imap")}>
				IMAP
			</button>
			<button class="tab" class:active={tab === "gmail"} onclick={() => (tab = "gmail")}>
				Gmail
			</button>
		</div>

		<div class="tab-body">
			{#if tab === "imap"}
				<form onsubmit={submitImap}>
					<div class="grid">
						<label>
							<span>Email</span>
							<input
								type="email"
								bind:value={form.email}
								required
								autocomplete="off"
								oninput={onEmailInput}
								onblur={onEmailBlur}
								onkeydown={onEmailKeydown}
							/>
						</label>
						<label>
							<span>Display name</span>
							<input bind:value={form.displayName} autocomplete="off" />
						</label>

						{#if detecting}
							<div class="discovery detecting">
								<span class="spinner" aria-hidden="true"></span>
								Detecting settings…
							</div>
						{:else if discovered && discovered.kind === "gmail"}
							<div class="discovery gmail-notice">
								<span>This address is hosted by Google — use Google sign-in instead</span>
								<button type="button" class="link" onclick={switchToGmailTab}>
									Use Gmail
								</button>
							</div>
						{:else if statusText}
							<div class="discovery status" class:warn={statusText.tone === "warn"}>
								{statusText.text}
							</div>
						{/if}

						<label>
							<span>IMAP host</span>
							<input bind:value={form.imapHost} required autocomplete="off" placeholder="imap.example.com" oninput={markHostFieldsTouched} />
						</label>
						<label class="narrow">
							<span>IMAP port</span>
							<input type="number" bind:value={form.imapPort} required oninput={markHostFieldsTouched} />
						</label>
						<label>
							<span>SMTP host</span>
							<input bind:value={form.smtpHost} required autocomplete="off" placeholder="smtp.example.com" oninput={markHostFieldsTouched} />
						</label>
						<label class="narrow">
							<span>SMTP port</span>
							<input type="number" bind:value={form.smtpPort} required oninput={markHostFieldsTouched} />
						</label>
						<label>
							<span>Username</span>
							<input bind:value={form.username} autocomplete="off" placeholder="defaults to email" oninput={markHostFieldsTouched} />
						</label>
						<label>
							<span>Password</span>
							<input type="password" bind:value={form.password} required autocomplete="off" />
						</label>
					</div>

					{#if error}<div class="error">{error}</div>{/if}

					<div class="actions">
						<button type="button" onclick={onClose} disabled={busy}>Cancel</button>
						<button type="submit" class="primary" disabled={busy}>
							{busy ? "Connecting…" : "Add account"}
						</button>
					</div>
				</form>
			{:else}
				<div class="gmail">
					<p class="hint">
						Sign in with Google to add a Gmail account. A browser window will open
						for consent; this window waits until you finish.
					</p>
					{#if error}<div class="error">{error}</div>{/if}
					<button class="primary google" onclick={gmail} disabled={busy}>
						{busy ? "Waiting for Google…" : "Sign in with Google"}
					</button>
				</div>
			{/if}
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
	.tabs {
		display: flex;
		border-bottom: 1px solid var(--border);
	}
	.tab {
		flex: 1;
		border: none;
		border-radius: 0;
		background: transparent;
		border-bottom: 2px solid transparent;
		padding: 8px;
		color: var(--muted);
	}
	.tab:hover {
		background: var(--hover);
	}
	.tab.active {
		color: var(--fg);
		border-bottom-color: var(--accent);
	}
	.tab-body {
		padding: 14px 16px;
	}
	.grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 10px;
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 3px;
		font-size: 11px;
		color: var(--muted);
		grid-column: span 2;
	}
	label.narrow {
		grid-column: span 1;
	}
	label span {
		text-transform: uppercase;
		letter-spacing: 0.4px;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		margin-top: 14px;
	}
	.error {
		margin-top: 12px;
		padding: 8px 10px;
		border: 1px solid var(--c1);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--c1) 15%, var(--bg));
		color: var(--fg);
		font-size: 12px;
		word-break: break-word;
	}
	.gmail {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.hint {
		margin: 0;
		color: var(--muted);
		font-size: 12px;
	}
	.google {
		align-self: flex-start;
	}

	.discovery {
		grid-column: span 2;
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 11px;
		padding: 6px 8px;
		border-radius: var(--radius);
		border: 1px solid var(--border);
		background: color-mix(in srgb, var(--accent) 8%, var(--bg));
		color: var(--muted);
	}
	.discovery.detecting {
		color: var(--muted);
	}
	.discovery.status {
		color: color-mix(in srgb, var(--accent) 80%, var(--fg));
		border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
	}
	.discovery.status.warn {
		color: var(--fg);
		border-color: var(--c3);
		background: color-mix(in srgb, var(--c3) 14%, var(--bg));
	}
	.discovery.gmail-notice {
		justify-content: space-between;
		color: var(--fg);
	}
	.discovery .link {
		border: none;
		background: transparent;
		color: var(--accent);
		padding: 0;
		font-size: 11px;
		text-decoration: underline;
		flex: none;
		cursor: pointer;
	}
	.discovery .link:hover {
		background: transparent;
		color: color-mix(in srgb, var(--accent) 80%, var(--fg));
	}
	.spinner {
		width: 12px;
		height: 12px;
		border: 2px solid color-mix(in srgb, var(--accent) 35%, transparent);
		border-top-color: var(--accent);
		border-radius: 50%;
		display: inline-block;
		animation: spin 0.7s linear infinite;
		flex: none;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
