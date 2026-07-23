<script lang="ts">
	import DOMPurify from "dompurify";
	import { openUrl } from "@tauri-apps/plugin-opener";
	import { mail } from "$lib/stores/mail.svelte";
	import { fullDate, initials, avatarColor, formatBytes, carrierLabel, carrierGlyph } from "$lib/format";
	import {
		messageFrameDocument,
		resolveOpenableLinkUrl,
		OPEN_LINK_MESSAGE_TYPE,
	} from "$lib/message-html";

	// The reader focuses this element when a message is opened (Enter).
	let {
		paneEl = $bindable(),
		onReply,
		onReplyAll,
		onMove,
	}: {
		paneEl?: HTMLElement;
		onReply: () => void;
		onReplyAll: () => void;
		onMove: () => void;
	} = $props();

	const msg = $derived(mail.selectedMessage);
	const body = $derived(mail.body);
	let remoteContentMessageIds = $state<number[]>([]);
	// The global preference grants remote images to every message; otherwise
	// consent is per-message and session-only.
	const alwaysRemote = $derived(mail.settings.alwaysDownloadRemoteImages);
	const remoteContentAllowed = $derived(
		alwaysRemote || (msg ? remoteContentMessageIds.includes(msg.id) : false),
	);

	// Base style injected into the iframe so mail with no colors of its own
	// adopts the omarchy theme. Uses the resolved values, not var() (the iframe
	// is a separate document and can't see our custom properties).
	function iframeSrcdoc(rawHtml: string, allowRemoteContent: boolean): string {
		const clean = DOMPurify.sanitize(rawHtml, {
			// The iframe CSP controls resource loading; sanitization stays strict after opt-in.
			FORBID_TAGS: [
				"script",
				"style",
				"iframe",
				"object",
				"embed",
				"form",
				"base",
				"link",
				"meta",
			],
			FORBID_ATTR: ["srcset"],
		});
		const css = getComputedStyle(document.documentElement);
		const bg = css.getPropertyValue("--bg").trim() || "#1f1f28";
		const fg = css.getPropertyValue("--fg").trim() || "#dcd7ba";
		const accent = css.getPropertyValue("--accent").trim() || "#7e9cd8";
		return messageFrameDocument(
			clean,
			{ background: bg, foreground: fg, accent },
			allowRemoteContent,
		);
	}

	const srcdoc = $derived(
		body?.html ? iframeSrcdoc(body.html, remoteContentAllowed) : null,
	);

	let iframeEl = $state<HTMLIFrameElement | undefined>();

	// Sender HTML must never navigate the frame or reach the app. WebKitGTK
	// keeps a sandboxed srcdoc frame's contentDocument null from the parent's
	// side, so clicks cannot be observed from out here — instead the frame
	// document itself carries a click-forwarder script (see
	// LINK_FORWARDER_SCRIPT) that swallows every anchor activation and
	// postMessages the raw href to this window. The frame runs with
	// sandbox="allow-scripts" only, so its origin is opaque: no parent DOM
	// access, no Tauri IPC, and the frame CSP denies the network. Validation
	// stays on this side — only http(s) hrefs reach the opener plugin.
	$effect(() => {
		const onMessage = (event: MessageEvent) => {
			if (!iframeEl || event.source !== iframeEl.contentWindow) return;
			const data = event.data as { type?: unknown; href?: unknown } | null;
			if (!data || data.type !== OPEN_LINK_MESSAGE_TYPE) return;
			if (typeof data.href !== "string") return;
			const resolved = resolveOpenableLinkUrl(data.href, "about:srcdoc");
			if (resolved) void openUrl(resolved);
		};
		window.addEventListener("message", onMessage);
		return () => window.removeEventListener("message", onMessage);
	});

	function toggleRemoteContent() {
		if (!msg) return;
		remoteContentMessageIds = remoteContentAllowed
			? remoteContentMessageIds.filter((id) => id !== msg.id)
			: [...remoteContentMessageIds, msg.id];
	}

	// Listed (non-inline) attachments for the currently-loaded body. Inline cid
	// images already render in the body, so they are not shown as chips.
	const attachments = $derived(
		body && msg && body.id === msg.id
			? body.attachments.filter((a) => !a.isInline)
			: [],
	);

	// Shipments detected for the currently-loaded body; `mail.shipments` is
	// reset to [] on every message change so no id-matching guard is needed.
	const shipments = $derived(mail.shipments);

	function openShipmentLink(url: string) {
		void openUrl(url);
	}

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
				<button class="r-act" title="Archive (a)" onclick={() => mail.archiveMessage(msg)}>
					<span class="ic">📦</span> Archive
				</button>
				<button class="r-act" title="Delete (d)" onclick={() => mail.deleteMessage(msg)}>
					<span class="ic">🗑</span> Delete
				</button>
				<button
					class="r-act"
					class:pri={msg.flagged}
					title={msg.flagged ? "Remove flag (f)" : "Flag (f)"}
					aria-pressed={msg.flagged}
					onclick={() => mail.toggleFlagged(msg)}
				>
					<span class="ic">★</span>
					{msg.flagged ? "Unflag" : "Flag"}
				</button>
				<button class="r-act" title="Move to folder (m)" onclick={onMove}>
					<span class="ic">↦</span> Move
				</button>
				<!-- Mark read/unread -->
				<button
					class="r-act"
					title={msg.seen ? "Mark unread" : "Mark read"}
					onclick={() => mail.toggleRead(msg)}
				>
					<span class="ic">{msg.seen ? "◎" : "●"}</span>
					{msg.seen ? "Mark unread" : "Mark read"}
				</button>
			</div>

			{#if attachments.length}
				<div class="r-atts" aria-label="Attachments">
					{#each attachments as att (att.id)}
						<button
							type="button"
							class="r-att"
							title={`Save ${att.filename || "attachment"} (${formatBytes(att.sizeBytes)})`}
							onclick={() => mail.saveAttachment(att)}
						>
							<span class="ic" aria-hidden="true">📎</span>
							<span class="name">{att.filename || "attachment"}</span>
							<span class="size">{formatBytes(att.sizeBytes)}</span>
						</button>
					{/each}
				</div>
			{/if}

			{#if shipments.length}
				<div class="r-ships" aria-label="Shipments">
					{#each shipments as shipment (shipment.id)}
						<button
							type="button"
							class="r-ship"
							disabled={!shipment.trackingUrl}
							title={shipment.trackingUrl ? `Track on ${carrierLabel(shipment.carrier)}` : undefined}
							onclick={() => shipment.trackingUrl && openShipmentLink(shipment.trackingUrl)}
						>
							<span class="ic" aria-hidden="true">{carrierGlyph(shipment.carrier)}</span>
							<span class="ship-info">
								<span class="carrier">{carrierLabel(shipment.carrier)}</span>
								{#if shipment.trackingNumber}
									<span class="num">{shipment.trackingNumber}</span>
								{/if}
								{#if shipment.orderId}
									<span class="num">Order {shipment.orderId}</span>
								{/if}
							</span>
							{#if shipment.trackingUrl}
								<span class="go" aria-hidden="true">↗</span>
							{/if}
						</button>
					{/each}
				</div>
			{/if}
		</div>

		<div class="body">
			{#if mail.loadingBody}
				<div class="empty">Loading…</div>
			{:else if srcdoc}
				{#if !alwaysRemote}
					<div class="remote-content" role="status">
						<span>
							{remoteContentAllowed
								? "Remote images are allowed for this message."
								: "Remote images are blocked to protect your privacy."}
						</span>
						<button
							type="button"
							aria-pressed={remoteContentAllowed}
							onclick={toggleRemoteContent}
						>
							{remoteContentAllowed ? "Block remote images" : "Load remote images"}
						</button>
					</div>
				{/if}
				<iframe
					class="html-body"
					title="Message body"
					sandbox="allow-scripts"
					srcdoc={srcdoc}
					bind:this={iframeEl}
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

	.r-atts {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 12px;
	}

	.r-att {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		max-width: 100%;
		padding: 5px 11px;
		border: 1px solid var(--border);
		border-radius: 4px;
		background: var(--surface);
		color: var(--fg);
		font: inherit;
		font-size: 12px;
		cursor: pointer;
	}

	.r-att:hover {
		background: var(--hover);
		border-color: color-mix(in srgb, var(--accent) 40%, transparent);
	}

	.r-att:focus-visible {
		outline: none;
		box-shadow: 0 0 0 1px var(--accent);
	}

	.r-att .ic {
		color: var(--muted);
		flex-shrink: 0;
	}

	.r-att .name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 240px;
	}

	.r-att .size {
		color: var(--muted);
		font-size: 11px;
		flex-shrink: 0;
	}

	.r-ships {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 12px;
	}

	.r-ship {
		display: inline-flex;
		align-items: center;
		gap: 9px;
		max-width: 100%;
		padding: 6px 12px;
		border: 1px solid var(--border);
		border-radius: 4px;
		background: var(--surface);
		color: var(--fg);
		font: inherit;
		font-size: 12px;
		cursor: pointer;
		text-align: left;
	}

	.r-ship:not(:disabled):hover {
		background: var(--hover);
		border-color: color-mix(in srgb, var(--accent) 40%, transparent);
	}

	.r-ship:disabled {
		cursor: default;
	}

	.r-ship:focus-visible {
		outline: none;
		box-shadow: 0 0 0 1px var(--accent);
	}

	.r-ship .ship-info {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
	}

	.r-ship .carrier {
		font-weight: 700;
	}

	.r-ship .num {
		color: var(--muted);
		font-family: var(--mono);
		font-size: 11px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 260px;
	}

	.r-ship .go {
		color: var(--accent);
		flex-shrink: 0;
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

	.remote-content {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 7px 14px;
		border-bottom: 1px solid var(--border);
		color: var(--muted);
		background: var(--surface);
		font-size: 11.5px;
	}

	.remote-content button {
		flex-shrink: 0;
		border: 0;
		padding: 2px 0;
		color: var(--accent);
		background: transparent;
		font: inherit;
		font-weight: 700;
		cursor: pointer;
	}

	.remote-content button:hover {
		text-decoration: underline;
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
