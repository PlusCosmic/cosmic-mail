// Command palette ranking — pure, deterministic, unit-tested (tests/palette.test.mjs).
//
// A command is anything the user can fuzzy-search and run from the Ctrl+K overlay.
// Ranking is a small subsequence fuzzy matcher: it never depends on Svelte, the DOM,
// or the mail store, so it can be exercised directly from node:test.

export interface PaletteCommand {
	id: string;
	title: string;
	hint?: string;
	keywords?: string[];
	run: () => void;
}

// Per-character scoring weights. Word-start matches dominate so that a query like
// "ci" prefers "Compose inbox" (two word starts) over an incidental subsequence.
const MATCH_BASE = 1;
const WORD_START_BONUS = 10;
const CONSECUTIVE_BONUS = 5;
// Small penalty per character before the first match, rewarding earlier positions
// without ever overturning a word-start advantage.
const LEADING_PENALTY = 0.5;

function isAlphaNumeric(ch: string): boolean {
	return /[a-zA-Z0-9]/.test(ch);
}

/** A text index is a "word start" at position 0, after punctuation/space, or on a camelCase hump. */
function isWordStart(text: string, index: number): boolean {
	if (index === 0) return true;
	const prev = text[index - 1];
	const cur = text[index];
	if (!isAlphaNumeric(prev)) return true;
	// lower -> upper transition (camelCase).
	if (prev >= "a" && prev <= "z" && cur >= "A" && cur <= "Z") return true;
	return false;
}

/**
 * Case-insensitive subsequence score of `query` against `text`.
 * Returns `null` when `text` does not contain `query` as a subsequence.
 * An empty query matches everything with a score of 0.
 * Higher is better; scores reward word-start matches, consecutive runs, and earlier placement.
 */
export function fuzzyScore(query: string, text: string): number | null {
	const q = query.toLowerCase();
	if (q.length === 0) return 0;
	const lower = text.toLowerCase();

	let score = 0;
	let qi = 0;
	let prevMatch = -2;
	let firstMatch = -1;

	for (let ti = 0; ti < lower.length && qi < q.length; ti++) {
		if (lower[ti] !== q[qi]) continue;
		if (firstMatch < 0) firstMatch = ti;
		let charScore = MATCH_BASE;
		if (isWordStart(text, ti)) charScore += WORD_START_BONUS;
		if (ti === prevMatch + 1) charScore += CONSECUTIVE_BONUS;
		score += charScore;
		prevMatch = ti;
		qi++;
	}

	if (qi < q.length) return null; // not every query character matched
	return score - firstMatch * LEADING_PENALTY;
}

/**
 * Rank `commands` against `query`, scoring each against its title and keywords
 * (best of the two wins). Non-matches are dropped; results sort by score descending
 * with a stable tie-break on original order. An empty/whitespace query returns all
 * commands in their original order.
 */
export function rankCommands(query: string, commands: PaletteCommand[]): PaletteCommand[] {
	const q = query.trim();
	if (q === "") return commands.slice();

	const scored: { cmd: PaletteCommand; score: number; order: number }[] = [];
	commands.forEach((cmd, order) => {
		let best = fuzzyScore(q, cmd.title);
		for (const keyword of cmd.keywords ?? []) {
			const s = fuzzyScore(q, keyword);
			if (s !== null && (best === null || s > best)) best = s;
		}
		if (best !== null) scored.push({ cmd, score: best, order });
	});

	scored.sort((a, b) => b.score - a.score || a.order - b.order);
	return scored.map((entry) => entry.cmd);
}
