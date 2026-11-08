import assert from "node:assert/strict";
import test from "node:test";
import { formatBytes } from "../src/lib/format.ts";

test("formats byte sizes with sensible units and precision", () => {
	assert.equal(formatBytes(0), "0 B");
	assert.equal(formatBytes(512), "512 B");
	assert.equal(formatBytes(1024), "1 KB");
	assert.equal(formatBytes(1536), "1.5 KB");
	assert.equal(formatBytes(1024 * 1024), "1 MB");
	assert.equal(formatBytes(1024 * 1024 * 1024), "1 GB");
	// Rounds to one decimal place.
	assert.equal(formatBytes(1024 * 1024 * 2.55), "2.6 MB");
});

test("handles invalid or non-positive inputs safely", () => {
	assert.equal(formatBytes(-1), "0 B");
	assert.equal(formatBytes(Number.NaN), "0 B");
	assert.equal(formatBytes(Number.POSITIVE_INFINITY), "0 B");
});
