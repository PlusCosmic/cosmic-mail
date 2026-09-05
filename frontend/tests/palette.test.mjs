import assert from "node:assert/strict";
import test from "node:test";
import { fuzzyScore, rankCommands } from "../src/lib/palette.ts";

function cmd(id, title, keywords) {
	return { id, title, keywords, run: () => {} };
}

test("exact prefix match scores and beats a scattered subsequence", () => {
	const exact = fuzzyScore("arch", "Archive message");
	const scattered = fuzzyScore("arch", "Search all mail");
	assert.notEqual(exact, null);
	assert.notEqual(scattered, null);
	assert.ok(exact > scattered, "consecutive word-start match should outrank a scattered one");
});

test("substring match anywhere is found", () => {
	assert.notEqual(fuzzyScore("inbox", "Open inbox — harry@pluscosmic.dev"), null);
});

test("case-insensitive subsequence match", () => {
	assert.notEqual(fuzzyScore("SYNC", "sync current folder"), null);
	assert.notEqual(fuzzyScore("gtf", "Go to folder — Archive"), null);
});

test("no match returns null", () => {
	assert.equal(fuzzyScore("zzz", "Compose"), null);
	// Missing a required character breaks the subsequence.
	assert.equal(fuzzyScore("cx", "Compose"), null);
});

test("empty query matches with a neutral score", () => {
	assert.equal(fuzzyScore("", "anything"), 0);
});

test("word-start preference: 'ci' ranks 'Compose inbox' above a mid-word match", () => {
	const commands = [
		cmd("music", "Music library"),
		cmd("compose", "Compose inbox"),
	];
	const ranked = rankCommands("ci", commands);
	assert.equal(ranked[0].id, "compose");
});

test("rankCommands filters non-matches", () => {
	const commands = [
		cmd("a", "Compose"),
		cmd("b", "Add account"),
		cmd("c", "Sync current folder"),
	];
	const ranked = rankCommands("sync", commands);
	assert.deepEqual(
		ranked.map((c) => c.id),
		["c"],
	);
});

test("empty query returns all commands in original order", () => {
	const commands = [cmd("a", "Alpha"), cmd("b", "Beta"), cmd("c", "Gamma")];
	assert.deepEqual(
		rankCommands("", commands).map((c) => c.id),
		["a", "b", "c"],
	);
	assert.deepEqual(
		rankCommands("   ", commands).map((c) => c.id),
		["a", "b", "c"],
	);
});

test("keyword matching finds commands whose title does not match", () => {
	const commands = [
		cmd("toggle", "Toggle read", ["unread", "seen", "mark"]),
		cmd("compose", "Compose"),
	];
	const ranked = rankCommands("unread", commands);
	assert.equal(ranked[0].id, "toggle");
	assert.equal(ranked.length, 1);
});

test("stable tie-break preserves original order for equal scores", () => {
	// Identical titles score identically; original order must be preserved.
	const commands = [
		cmd("first", "Refresh"),
		cmd("second", "Refresh"),
		cmd("third", "Refresh"),
	];
	assert.deepEqual(
		rankCommands("refresh", commands).map((c) => c.id),
		["first", "second", "third"],
	);
});
