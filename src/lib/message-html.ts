export interface MessageFrameTheme {
	background: string;
	foreground: string;
	accent: string;
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
