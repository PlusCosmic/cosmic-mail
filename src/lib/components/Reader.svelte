<script lang="ts">
	import DOMPurify from "dompurify";
	import { mail } from "$lib/stores/mail.svelte";
	import { fullDate, initials, avatarColor } from "$lib/format";

	// The reader focuses this element when a message is opened (Enter).
	let {
		paneEl = $bindable(),
		onReply,
		onReplyAll,
	}: { paneEl?: HTMLElement; onReply: () => void; onReplyAll: () => void } = $props();

	const msg = $derived(mail.selectedMessage);
	const body = $derived(mail.body);

	// Base style injected into the iframe so mail with no colors of its own
	// adopts the omarchy theme. Uses the resolved values, not var() (the iframe
	// is a separate document and can't see our custom properties).
	function iframeSrcdoc(rawHtml: string): string {
		const clean = DOMPurify.sanitize(rawHtml, {
			WHOLE_DOCUMENT: true,
			// Block anything that could phone home or run script.
			FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form"],
			FORBID_ATTR: ["srcset"],
		});
		const css = getComputedStyle(document.documentElement);
		const bg = css.getPropertyValue("--bg").trim() || "#1f1f28";
		const fg = css.getPropertyValue("--fg").trim() || "#dcd7ba";
		const accent = css.getPropertyValue("--accent").trim() || "#7e9cd8";
		const base = `<base target="_blank"><style>
			html,body{background:${bg};color:${fg};margin:0;padding:12px;
				font-family:'CaskaydiaMono Nerd Font','JetBrainsMono Nerd Font',ui-sans-serif,system-ui,sans-serif;
				font-size:14px;line-height:1.5;word-wrap:break-word;}
			a{color:${accent};}
			img{max-width:100%;height:auto;}
			table{max-width:100%;}
			blockquote{border-left:2px solid ${accent};margin:0 0 0 4px;padding-left:10px;opacity:.85;}
		</style>`;
		return base + clean;
	}

	const srcdoc = $derived(body?.html ? iframeSrcdoc(body.html) : null);

	const senderIni = $derived(msg ? initials(msg.fromName, msg.fromAddr) : "");
	const senderColor = $derived(msg ? avatarColor(msg.fromAddr || msg.fromName) : "var(--muted)");

	// First toAddr or the account email as fallback.
	const toDisplay = $derived.by(() => {
		if (!msg) return "";
		if (body?.toAddrs.length) return body.toAddrs[0];
		const acc = mail.accounts.find((a) => a.id === msg.accountId);
		return acc?.email ?? "";
	});
</script>

<section class="reader" bind:this={paneEl} tabindex="-1" aria-label="Message">
	{#if !msg}
		<div class="empty">No message selected</div>
	{:else}
		<div class="r-head">
			<h1 class="r-subj">{msg.subject || "(no subject)"}</h1>

			<div class="r-row">
				<div class="r-av" style="background:{senderColor}">{senderIni}</div>
				<div class="r-who">
					<div class="n">{msg.fromName || msg.fromAddr}</div>
					<div class="a">
						{msg.fromAddr}{toDisplay ? ` → ${toDisplay}` : ""}
					</div>
				</div>
				<div class="r-when">{fullDate(msg.date)}</div>
			</div>

			<div class="r-actions">
				<button class="r-act pri" onclick={onReply} disabled={body?.id !== msg.id} title="Reply (r)">
					<span class="ic">↩</span> Reply
				</button>
				<button class="r-act" onclick={onReplyAll} disabled={body?.id !== msg.id} title="Reply all">
					<span class="ic">↩↩</span> Reply all
				</button>
				<button class="r-act" disabled title="Coming soon">
					<span class="ic">→</span> Forward
				</button>
				<button class="r-act" disabled title="Coming soon">
					<span class="ic">📦</span> Archive
				</button>
				<button class="r-act" disabled title="Coming soon">
					<span class="ic">🗑</span> Delete
				</button>
				<!-- Mark read/unread: the one working action -->
				<button
					class="r-act"
					title={msg.seen ? "Mark unread" : "Mark read"}
					onclick={() => mail.toggleRead(msg)}
				>
					<span class="ic">{msg.seen ? "◎" : "●"}</span>
					{msg.seen ? "Mark unread" : "Mark read"}
				</button>
			</div>
		</div>

		<div class="body">
			{#if mail.loadingBody}
				<div class="empty">Loading…</div>
			{:else if srcdoc}
				<iframe
					class="html-body"
					title="Message body"
					sandbox="allow-same-origin"
					srcdoc={srcdoc}
				></iframe>
			{:else if body?.text}
				<pre class="wrap">{body.text}</pre>
			{:else}
				<div class="empty">(empty message)</div>
			{/if}
		</div>
	{/if}
</section>

<style>
	.reader {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg);
		overflow: hidden;
		outline: none;
	}

	.reader:focus-visible {
		box-shadow: inset 0 0 0 1px var(--accent);
	}

	.r-head {
		padding: 20px 26px 14px;
		border-bottom: 1px solid var(--border);
		flex-shrink: 0;
	}

	.r-subj {
		font-size: 17px;
		font-weight: 700;
		margin: 0 0 14px;
		line-height: 1.4;
		word-break: break-word;
	}

	.r-row {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.r-av {
		width: 38px;
		height: 38px;
		border-radius: 5px;
		display: grid;
		place-items: center;
		color: var(--bg);
		font-weight: 700;
		font-size: 14px;
		flex-shrink: 0;
	}

	.r-who .n {
		font-weight: 700;
	}

	.r-who .a {
		color: var(--muted);
		font-size: 11.5px;
	}

	.r-when {
		margin-left: auto;
		color: var(--muted);
		font-size: 11.5px;
		text-align: right;
		white-space: nowrap;
	}

	.r-actions {
		display: flex;
		gap: 6px;
		margin-top: 14px;
		flex-wrap: wrap;
	}

	.r-act {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 5px 11px;
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--fg);
		background: transparent;
		font: inherit;
		cursor: pointer;
		font-size: 12px;
	}

	.r-act:hover:not(:disabled) {
		background: var(--hover);
	}

	.r-act .ic {
		color: var(--muted);
	}

	.r-act.pri {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 40%, transparent);
	}

	.r-act.pri .ic {
		color: var(--accent);
	}

	.r-act:disabled {
		opacity: 0.45;
		cursor: default;
	}

	.body {
		flex: 1;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.html-body {
		flex: 1;
		width: 100%;
		border: none;
		background: var(--bg);
	}

	.wrap {
		flex: 1;
		margin: 0;
		padding: 20px 26px;
		overflow: auto;
		white-space: pre-wrap;
		word-break: break-word;
		font-family: var(--mono);
		font-size: 13px;
		line-height: 1.7;
	}

	.empty {
		display: flex;
		align-items: center;
		justify-content: center;
		flex: 1;
		width: 100%;
		color: var(--faint);
		font-style: italic;
	}
</style>
