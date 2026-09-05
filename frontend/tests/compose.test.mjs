import assert from "node:assert/strict";
import test from "node:test";
import {
	mailboxKey,
	replySeed,
	replySubject,
	splitMailboxList,
} from "../src/lib/compose.ts";

const account = {
	id: "account",
	email: "me@example.com",
	displayName: "Me",
	kind: "imap",
};

const message = {
	id: 7,
	accountId: account.id,
	folderId: 3,
	uid: 42,
	subject: "Status",
	fromName: "Jane Doe",
	fromAddr: "jane@example.com",
	date: "2026-07-12T12:00:00Z",
	snippet: "",
	seen: true,
	flagged: false,
	hasAttachments: false,
};

const body = {
	id: message.id,
	html: null,
	text: "Original body",
	toAddrs: ['Me <me@example.com>', "other@example.com"],
	ccAddrs: ["OTHER@example.com", "copy@example.com"],
};

test("splits mailbox lists without breaking quoted display names", () => {
	assert.deepEqual(
		splitMailboxList('"Doe, Jane" <jane@example.com>, other@example.com; third@example.com'),
		['"Doe, Jane" <jane@example.com>', "other@example.com", "third@example.com"],
	);
});

test("normalizes formatted mailboxes for exclusion and deduplication", () => {
	assert.equal(mailboxKey('Me <ME@example.com>'), "me@example.com");
	const seed = replySeed(message, body, account, true);
	assert.deepEqual(seed.toAddrs, ["jane@example.com"]);
	assert.deepEqual(seed.ccAddrs, ["other@example.com", "copy@example.com"]);
});

test("replies from sent mail target the original recipients", () => {
	const sent = { ...message, fromName: "Me", fromAddr: account.email };
	const seed = replySeed(sent, body, account, true);
	assert.deepEqual(seed.toAddrs, ["other@example.com"]);
	assert.deepEqual(seed.ccAddrs, ["copy@example.com"]);
});

test("adds one reply prefix and quotes the source body", () => {
	assert.equal(replySubject("Re: Status"), "Re: Status");
	const seed = replySeed(message, body, account, false);
	assert.equal(seed.subject, "Re: Status");
	assert.match(seed.bodyText, /> Original body$/);
});
