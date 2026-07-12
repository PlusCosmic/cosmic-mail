import assert from "node:assert/strict";
import test from "node:test";
import {
	messageFrameDocument,
	remoteContentCsp,
} from "../src/lib/message-html.ts";

const theme = {
	background: "#111111",
	foreground: "#eeeeee",
	accent: "#88aaff",
};

test("blocks network resources before sender content by default", () => {
	const html = messageFrameDocument('<img src="https://tracker.test/pixel">', theme, false);
	const policy = remoteContentCsp(false);
	assert.match(policy, /default-src 'none'/);
	assert.match(policy, /img-src data: cid:/);
	assert.doesNotMatch(policy, /https?:/);
	assert.ok(html.indexOf("Content-Security-Policy") < html.indexOf("tracker.test"));
});

test("remote opt-in only expands image sources", () => {
	const policy = remoteContentCsp(true);
	assert.match(policy, /img-src data: cid: http: https:/);
	assert.match(policy, /media-src data:/);
	assert.doesNotMatch(policy, /media-src data: http/);
	assert.match(policy, /default-src 'none'/);
	assert.match(policy, /object-src 'none'/);
	assert.match(policy, /frame-src 'none'/);
});

test("builds one deliberate iframe document", () => {
	const html = messageFrameDocument("<p>Hello</p>", theme, false);
	assert.match(html, /^<!doctype html><html><head>/);
	assert.match(html, /<\/head><body><p>Hello<\/p><\/body><\/html>$/);
});
