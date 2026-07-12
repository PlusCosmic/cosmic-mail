import type { Account, MessageBody, MessageSummary } from "$lib/types";

export interface ComposeSeed {
	accountId: string;
	toAddrs: string[];
	ccAddrs: string[];
	subject: string;
	bodyText: string;
	replyToMessageId: number | null;
}

export function replySubject(subject: string): string {
	const trimmed = subject.trim();
	if (/^re\s*:/i.test(trimmed)) return trimmed;
	return `Re: ${trimmed || "(no subject)"}`;
}

export function mailboxKey(value: string): string {
	const trimmed = value.trim();
	const bracketed = /<\s*([^<>]+)\s*>\s*$/.exec(trimmed);
	return (bracketed?.[1] ?? trimmed).trim().toLowerCase();
}

export function splitMailboxList(value: string): string[] {
	const result: string[] = [];
	let current = "";
	let quoted = false;
	let escaped = false;
	let angleDepth = 0;

	for (const char of value) {
		if (escaped) {
			current += char;
			escaped = false;
			continue;
		}
		if (char === "\\" && quoted) {
			current += char;
			escaped = true;
			continue;
		}
		if (char === '"') quoted = !quoted;
		else if (!quoted && char === "<") angleDepth += 1;
		else if (!quoted && char === ">") angleDepth = Math.max(0, angleDepth - 1);

		if (!quoted && angleDepth === 0 && (char === "," || char === ";" || char === "\n")) {
			if (current.trim()) result.push(current.trim());
			current = "";
		} else {
			current += char;
		}
	}
	if (current.trim()) result.push(current.trim());
	return result;
}

function uniqueAddresses(addresses: string[], excluded: Set<string>): string[] {
	const result: string[] = [];
	const seen = new Set(excluded);
	for (const value of addresses) {
		const address = value.trim();
		const key = mailboxKey(address);
		if (address && !seen.has(key)) {
			seen.add(key);
			result.push(address);
		}
	}
	return result;
}

export function replySeed(
	message: MessageSummary,
	body: MessageBody,
	account: Account,
	replyAll: boolean,
): ComposeSeed {
	const sender = message.fromAddr.trim();
	const ownAddress = mailboxKey(account.email);
	const excluded = new Set([ownAddress]);
	const senderIsOwn = mailboxKey(sender) === ownAddress;
	const originalTo = uniqueAddresses(body.toAddrs, excluded);
	const toAddrs = senderIsOwn
		? originalTo
		: uniqueAddresses(sender ? [sender] : [], excluded);
	const ccAddrs = replyAll
		? uniqueAddresses(
				[...(senderIsOwn ? [] : body.toAddrs), ...body.ccAddrs],
				new Set([...excluded, ...toAddrs.map(mailboxKey)]),
			)
		: [];
	const quoted = body.text?.trim()
		? `\n\nOn ${new Date(message.date).toLocaleString()}, ${message.fromName || sender} wrote:\n${body.text
				.split("\n")
				.map((line) => `> ${line}`)
				.join("\n")}`
		: "";

	return {
		accountId: account.id,
		toAddrs,
		ccAddrs,
		subject: replySubject(message.subject),
		bodyText: quoted,
		replyToMessageId: message.id,
	};
}
