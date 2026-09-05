// Maps an OmarchyTheme onto CSS custom properties on <html>.
// See docs/ARCHITECTURE.md ("Omarchy integration").

import { getTheme, onThemeChanged, type UnlistenFn } from "./api";
import type { OmarchyTheme } from "./types";

// Built-in kanagawa fallback so the UI renders even when invoke() fails
// (plain-browser dev, backend not ready, missing theme files, etc.).
export const KANAGAWA_FALLBACK: OmarchyTheme = {
	name: "kanagawa",
	background: "#1f1f28",
	foreground: "#dcd7ba",
	accent: "#7e9cd8",
	cursor: "#c8c093",
	selectionBackground: "#2d4f67",
	selectionForeground: "#c8c093",
	// Standard kanagawa ANSI 16 (color0..color15).
	palette: [
		"#090618", // 0  black
		"#c34043", // 1  red
		"#76946a", // 2  green
		"#c0a36e", // 3  yellow
		"#7e9cd8", // 4  blue
		"#957fb8", // 5  magenta
		"#6a9589", // 6  cyan
		"#c8c093", // 7  white
		"#727169", // 8  bright black
		"#e82424", // 9  bright red
		"#98bb6c", // 10 bright green
		"#e6c384", // 11 bright yellow
		"#7fb4ca", // 12 bright blue
		"#938aa9", // 13 bright magenta
		"#7aa89f", // 14 bright cyan
		"#dcd7ba", // 15 bright white
	],
};

export function applyTheme(t: OmarchyTheme): void {
	if (typeof document === "undefined") return;
	const root = document.documentElement;
	root.style.setProperty("--bg", t.background);
	root.style.setProperty("--fg", t.foreground);
	root.style.setProperty("--accent", t.accent);
	root.style.setProperty("--cursor", t.cursor);
	root.style.setProperty("--sel-bg", t.selectionBackground);
	root.style.setProperty("--sel-fg", t.selectionForeground);
	for (let i = 0; i < 16; i++) {
		const c = t.palette[i] ?? KANAGAWA_FALLBACK.palette[i];
		root.style.setProperty(`--c${i}`, c);
	}
}

/**
 * Fetch the current theme, apply it, and subscribe to live changes.
 * Always applies the kanagawa fallback first so there is never an unstyled
 * flash, then upgrades to the real theme if the backend answers.
 * Returns an unlisten function (no-op if the subscription failed).
 */
export async function initTheme(): Promise<UnlistenFn> {
	applyTheme(KANAGAWA_FALLBACK);

	try {
		const theme = await getTheme();
		applyTheme(theme);
	} catch {
		// Backend unavailable — keep the fallback.
	}

	try {
		return await onThemeChanged(applyTheme);
	} catch {
		return () => {};
	}
}
