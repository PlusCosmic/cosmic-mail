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
