export interface MessageFrameTheme {
	background: string;
	foreground: string;
	accent: string;
}

const OPENABLE_LINK_SCHEMES = new Set(["http:", "https:"]);

/** postMessage type for link clicks forwarded out of the reader frame. */
export const OPEN_LINK_MESSAGE_TYPE = "cosmic-mail:open-link";

/**
 * Resolves an anchor `href` against a base URL and returns it only if the
 * resolved scheme is http: or https:. Used by the reader when a link click
 * is forwarded out of the message frame, to decide whether it may be passed
 * to the system browser via the opener plugin — everything else
 * (javascript:, mailto:, data:, vbscript:, cid:, and relative/fragment-only
 * hrefs that cannot resolve against an opaque `about:srcdoc` base) is
 * rejected by returning null so the caller silently ignores the click.
 */
export function resolveOpenableLinkUrl(href: string, baseUrl: string): string | null {
	const trimmed = href.trim();
	if (!trimmed) return null;
	let resolved: URL;
	try {
		resolved = new URL(trimmed, baseUrl);
	} catch {
		return null;
	}
	return OPENABLE_LINK_SCHEMES.has(resolved.protocol) ? resolved.href : null;
}

export function remoteContentCsp(allowRemoteContent: boolean): string {
	const remote = allowRemoteContent ? " http: https:" : "";
	return [
		"default-src 'none'",
		`img-src data: cid:${remote}`,
		"media-src data:",
		"style-src 'unsafe-inline'",
		// Permits only the click-forwarder below: DOMPurify has already
		// stripped every sender <script> and on* handler, and the frame runs
		// with sandbox="allow-scripts" alone, so its origin is opaque — no
		// parent DOM, no Tauri IPC, and default-src 'none' blocks the network.
		"script-src 'unsafe-inline'",
		"object-src 'none'",
		"frame-src 'none'",
		"form-action 'none'",
	].join("; ");
}

/**
 * Script injected into the message frame's <head>, ahead of any sender
 * content. WebKitGTK never exposes a sandboxed srcdoc frame's document to
 * the parent (contentDocument is null once the real document loads), so
 * click handling must live inside the frame: swallow every anchor
 * activation and forward the raw href to the parent, which validates it and
 * hands http(s) URLs to the opener plugin (system browser).
 */
export const LINK_FORWARDER_SCRIPT = `(() => {
	const forward = (event) => {
		const target = event.target;
		const anchor = target && target.closest ? target.closest("a[href]") : null;
		if (!anchor) return;
		event.preventDefault();
		window.parent.postMessage(
			{ type: "${OPEN_LINK_MESSAGE_TYPE}", href: anchor.getAttribute("href") || "" },
			"*",
		);
	};
	document.addEventListener("click", forward);
	document.addEventListener("auxclick", forward);
})();`;

export function messageFrameDocument(
	cleanHtml: string,
	theme: MessageFrameTheme,
	allowRemoteContent: boolean,
): string {
	return `<!doctype html><html><head>
		<meta http-equiv="Content-Security-Policy" content="${remoteContentCsp(allowRemoteContent)}">
		<base target="_blank">
		<script>${LINK_FORWARDER_SCRIPT}</script>
		<style>
			html,body{background:${theme.background};color:${theme.foreground};margin:0;padding:12px;
				font-family:'CaskaydiaMono Nerd Font','JetBrainsMono Nerd Font',ui-sans-serif,system-ui,sans-serif;
				font-size:14px;line-height:1.5;word-wrap:break-word;}
			a{color:${theme.accent};}
			img{max-width:100%;height:auto;}
			table{max-width:100%;}
			blockquote{border-left:2px solid ${theme.accent};margin:0 0 0 4px;padding-left:10px;opacity:.85;}
		</style>
	</head><body>${cleanHtml}</body></html>`;
}
