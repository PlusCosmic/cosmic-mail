import assert from "node:assert/strict";
import test from "node:test";
import {
	messageFrameDocument,
	remoteContentCsp,
	resolveOpenableLinkUrl,
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

const srcdocBase = "about:srcdoc";
const httpBase = "https://mail.example.com/inbox/message";

test("accepts absolute http and https links", () => {
	assert.equal(
		resolveOpenableLinkUrl("http://example.com/a", srcdocBase),
		"http://example.com/a",
	);
	assert.equal(
		resolveOpenableLinkUrl("https://example.com/a?x=1", srcdocBase),
		"https://example.com/a?x=1",
	);
});

test("rejects javascript:, mailto:, data:, and vbscript: links", () => {
	assert.equal(resolveOpenableLinkUrl("javascript:alert(1)", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("JavaScript:alert(1)", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("mailto:a@example.com", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("data:text/html,hi", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("vbscript:msgbox(1)", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("cid:abc123", srcdocBase), null);
});

test("rejects relative and fragment-only hrefs against the opaque srcdoc base", () => {
	// `about:srcdoc` is an opaque-path URL and cannot resolve relative
	// references, which is exactly what makes bare in-page anchors and
	// server-relative paths safely inert in the reader iframe.
	assert.equal(resolveOpenableLinkUrl("/relative/path", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("relative/path", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("#section", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("//example.com/x", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("", srcdocBase), null);
	assert.equal(resolveOpenableLinkUrl("   ", srcdocBase), null);
});

test("resolves protocol-relative and relative hrefs against a real http(s) base", () => {
	assert.equal(
		resolveOpenableLinkUrl("//example.com/x", httpBase),
		"https://example.com/x",
	);
	assert.equal(
		resolveOpenableLinkUrl("/other", httpBase),
		"https://mail.example.com/other",
	);
	assert.equal(
		resolveOpenableLinkUrl("page.html", httpBase),
		"https://mail.example.com/inbox/page.html",
	);
});
