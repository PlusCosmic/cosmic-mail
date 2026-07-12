import assert from "node:assert/strict";
import test from "node:test";
import {
	DEFAULT_LIST_PANE_RATIO,
	MAX_LIST_PANE_WIDTH,
	MIN_LIST_PANE_WIDTH,
	isValidPaneRatio,
	paneWidthLimits,
	ratioForListPaneWidth,
	resolveListPaneWidth,
} from "../src/lib/pane-layout.ts";

test("uses the preferred ratio when both panes remain usable", () => {
	assert.equal(resolveListPaneWidth(1000, 0.42), 420);
});

test("clamps list width at both its readable bounds", () => {
	assert.equal(resolveListPaneWidth(1000, 0.1), MIN_LIST_PANE_WIDTH);
	assert.equal(resolveListPaneWidth(3000, 0.9), MAX_LIST_PANE_WIDTH);
});

test("preserves reader space when the available width is unusually small", () => {
	assert.deepEqual(paneWidthLimits(500), { minimum: 180, maximum: 180 });
	assert.equal(resolveListPaneWidth(500, DEFAULT_LIST_PANE_RATIO), 180);
});

test("converts an adjusted pixel width back into a responsive preference", () => {
	assert.equal(ratioForListPaneWidth(1200, 480), 0.4);
	assert.equal(ratioForListPaneWidth(0, 480), DEFAULT_LIST_PANE_RATIO);
});

test("rejects malformed persisted ratios", () => {
	assert.equal(isValidPaneRatio(0.42), true);
	assert.equal(isValidPaneRatio(Number.NaN), false);
	assert.equal(isValidPaneRatio(0), false);
	assert.equal(isValidPaneRatio(1), false);
	assert.equal(isValidPaneRatio("0.42"), false);
});
