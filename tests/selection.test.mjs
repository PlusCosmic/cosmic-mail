import assert from "node:assert/strict";
import test from "node:test";
import { nextSelectionId } from "../src/lib/selection.ts";

const list = (...ids) => ids.map((id) => ({ id }));

test("selects the following message when one exists", () => {
	assert.equal(nextSelectionId(list(1, 2, 3), 2), 3);
	assert.equal(nextSelectionId(list(1, 2, 3), 1), 2);
});

test("falls back to the previous message when removing the last", () => {
	assert.equal(nextSelectionId(list(1, 2, 3), 3), 2);
});

test("returns null when removing the only message", () => {
	assert.equal(nextSelectionId(list(7), 7), null);
});

test("returns null when the id is not present", () => {
	assert.equal(nextSelectionId(list(1, 2), 99), null);
	assert.equal(nextSelectionId([], 1), null);
});
