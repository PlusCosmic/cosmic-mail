export interface MessageFrameTheme {
	background: string;
	foreground: string;
	accent: string;
}

const OPENABLE_LINK_SCHEMES = new Set(["http:", "https:"]);

/**
 * Resolves an anchor `href` against a base URL and returns it only if the
 * resolved scheme is http: or https:. Used by the reader's delegated click
 * handler to decide whether a link may be forwarded to the system browser
 * via the opener plugin — everything else (javascript:, mailto:, data:,
 * vbscript:, cid:, and relative/fragment-only hrefs that cannot resolve
 * against an opaque `about:srcdoc` base) is rejected by returning null so
 * the caller silently ignores the click.
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
		"object-src 'none'",
		"frame-src 'none'",
		"form-action 'none'",
	].join("; ");
}

export function messageFrameDocument(
	cleanHtml: string,
	theme: MessageFrameTheme,
	allowRemoteContent: boolean,
): string {
	return `<!doctype html><html><head>
		<meta http-equiv="Content-Security-Policy" content="${remoteContentCsp(allowRemoteContent)}">
		<base target="_blank">
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
