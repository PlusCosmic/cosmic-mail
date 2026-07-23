// Seed the GreenMail fixture: deliver every e2e/fixtures/mail/*.eml to the
// `test@localhost` mailbox over GreenMail's plaintext SMTP (localhost:3025).
// GreenMail files them into the IMAP INBOX the app then syncs.
//
// Zero dependencies — a tiny SMTP client over Node's `net`. Run after the
// container is up (see the `e2e:env:up` npm script).

import net from "node:net";
import http from "node:http";
import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HOST = "127.0.0.1";
const SMTP_PORT = 3025;
const API_PORT = 8080;
const RCPT = "test@localhost";
const MAIL_DIR = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "mail");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Read one SMTP reply (handles multi-line `250-...` continuations). */
function readReply(sock, buf) {
  return new Promise((resolve, reject) => {
    const onData = (chunk) => {
      buf.data += chunk;
      // A complete reply ends with a final line "<code><SP>...\r\n" — a space
      // after the code (a dash means more continuation lines follow).
      const last = buf.data.split("\r\n").filter((l) => l).pop() ?? "";
      if (/^\d{3} /.test(last)) {
        cleanup();
        const text = buf.data;
        buf.data = "";
        resolve({ code: Number(last.slice(0, 3)), text });
      }
    };
    const onErr = (err) => {
      cleanup();
      reject(err);
    };
    const cleanup = () => {
      sock.off("data", onData);
      sock.off("error", onErr);
    };
    sock.on("data", onData);
    sock.on("error", onErr);
  });
}

/** Send a command and assert the reply code is in `expect`. */
async function cmd(sock, buf, line, expect) {
  if (line != null) sock.write(line + "\r\n");
  const reply = await readReply(sock, buf);
  if (!expect.includes(reply.code)) {
    throw new Error(`SMTP: expected ${expect}, got ${reply.text.trim()}`);
  }
  return reply;
}

/** GET the readiness endpoint; resolves true on HTTP 200. */
function readinessOk() {
  return new Promise((resolve) => {
    const req = http.request(
      { host: HOST, port: API_PORT, path: "/api/service/readiness", method: "GET" },
      (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      },
    );
    req.on("error", () => resolve(false));
    req.end();
  });
}

/** Wait until GreenMail reports ready — a fresh container accepts an SMTP
 *  connection before it can actually file mail, so gate on the readiness API. */
async function waitForReady({ timeout = 30000, interval = 300 } = {}) {
  const deadline = Date.now() + timeout;
  for (;;) {
    if (await readinessOk()) return;
    if (Date.now() > deadline) throw new Error("GreenMail never became ready");
    await sleep(interval);
  }
}

/** Best-effort: purge any previously delivered mail so re-seeding is clean. */
function purgeAll() {
  return new Promise((resolve) => {
    const req = http.request(
      { host: HOST, port: API_PORT, path: "/api/mail/purge", method: "POST" },
      (res) => {
        res.resume();
        res.on("end", resolve);
      },
    );
    req.on("error", () => resolve()); // best-effort; ignore if unavailable
    req.end();
  });
}

/** Deliver one raw RFC822 message via SMTP (CRLF + dot-stuffing). */
async function deliver(raw) {
  const sock = net.createConnection(SMTP_PORT, HOST);
  const buf = { data: "" };
  await new Promise((resolve, reject) => {
    sock.once("connect", resolve);
    sock.once("error", reject);
  });
  await cmd(sock, buf, null, [220]);
  await cmd(sock, buf, "EHLO cosmic-mail-e2e", [250]);
  await cmd(sock, buf, "MAIL FROM:<fixture@example.com>", [250]);
  await cmd(sock, buf, `RCPT TO:<${RCPT}>`, [250]);
  await cmd(sock, buf, "DATA", [354]);
  const body = raw
    .replace(/\r?\n/g, "\r\n")
    .replace(/\r\n\./g, "\r\n..") // dot-stuff lines beginning with '.'
    .replace(/\r\n$/, "");
  sock.write(body + "\r\n.\r\n");
  await cmd(sock, buf, null, [250]);
  await cmd(sock, buf, "QUIT", [221]);
  sock.end();
}

async function main() {
  console.log("· waiting for GreenMail…");
  await waitForReady();
  await purgeAll();

  const files = (await readdir(MAIL_DIR)).filter((f) => f.endsWith(".eml")).sort();
  for (const file of files) {
    const raw = await readFile(join(MAIL_DIR, file), "utf8");
    await deliver(raw);
    console.log(`· delivered ${file}`);
  }
  console.log(`✓ seeded ${files.length} messages to ${RCPT}`);
}

main().catch((err) => {
  console.error("seed failed:", err.message);
  process.exit(1);
});
