// Small formatting helpers (no deps).

import type { FolderRole } from "./types";

/** 1–2 uppercase chars from display name, fallback to addr local part. */
export function initials(name: string, addr: string): string {
	const src = name.trim() || addr.split("@")[0] || "?";
	const parts = src.split(/[\s._\-+]+/).filter(Boolean);
	if (parts.length >= 2) {
		return (parts[0][0] + parts[1][0]).toUpperCase();
	}
	return (parts[0]?.slice(0, 2) ?? "?").toUpperCase();
}

const AVATAR_COLORS = [
	"var(--c2)",
	"var(--c3)",
	"var(--c4)",
	"var(--c5)",
	"var(--c6)",
	"var(--c12)",
	"var(--c13)",
	"var(--c14)",
];

/** Deterministic avatar background color derived from a seed string. */
export function avatarColor(seed: string): string {
	let h = 0;
	for (let i = 0; i < seed.length; i++) {
		h = (Math.imul(31, h) + seed.charCodeAt(i)) >>> 0;
	}
	return AVATAR_COLORS[h % AVATAR_COLORS.length];
}

/** Unicode glyph per folder role. Deliberately no icon library. */
export function roleGlyph(role: FolderRole): string {
	switch (role) {
		case "inbox":
			return "\u{1F4E5}"; // 📥
		case "sent":
			return "\u{1F4E4}"; // 📤
		case "drafts":
			return "✎"; // ✎
		case "trash":
			return "\u{1F5D1}"; // 🗑
		case "archive":
			return "\u{1F4E6}"; // 📦
		case "spam":
			return "⚠"; // ⚠
		default:
			return "\u{1F4C1}"; // 📁
	}
}

/** Compact relative date for list rows; falls back to a locale date. */
export function relativeDate(iso: string): string {
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return "";
	const now = new Date();
	const diffMs = now.getTime() - d.getTime();
	const min = 60_000;
	const hour = 60 * min;
	const day = 24 * hour;

	if (diffMs < min) return "now";
	if (diffMs < hour) return `${Math.floor(diffMs / min)}m`;
	if (diffMs < day) return `${Math.floor(diffMs / hour)}h`;

	const sameYear = d.getFullYear() === now.getFullYear();
	if (diffMs < 7 * day) {
		return d.toLocaleDateString(undefined, { weekday: "short" });
	}
	return d.toLocaleDateString(
		undefined,
		sameYear
			? { month: "short", day: "numeric" }
			: { year: "2-digit", month: "short", day: "numeric" },
	);
}

/** Human-readable byte size, e.g. 1536 → "1.5 KB", 0 → "0 B". */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
	const units = ["B", "KB", "MB", "GB", "TB"];
	const exp = Math.min(
		Math.floor(Math.log(bytes) / Math.log(1024)),
		units.length - 1,
	);
	const value = bytes / 1024 ** exp;
	// Whole bytes and round values read better without a trailing ".0".
	const rounded = exp === 0 ? value : Math.round(value * 10) / 10;
	const text = Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
	return `${text} ${units[exp]}`;
}

/**
 * Gmail nests its system mailboxes under a literal "[Gmail]/" IMAP path
 * (e.g. "[Gmail]/Sent Mail", "[Gmail]/Drafts"). Strip that prefix for
 * display only — every command, key, and lookup keeps using the real
 * `Folder.name` path returned by the backend.
 */
export function displayFolderName(name: string): string {
	const prefix = "[Gmail]/";
	return name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

/** Full timestamp for the reader header. */
export function fullDate(iso: string): string {
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return iso;
	return d.toLocaleString(undefined, {
		weekday: "short",
		year: "numeric",
		month: "short",
		day: "numeric",
		hour: "2-digit",
		minute: "2-digit",
	});
}
