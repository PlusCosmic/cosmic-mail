// End-to-end tests driving the real UI through the debug automation bridge
// (src-tauri/src/automation.rs) against the hermetic GreenMail fixture. The app
// syncs the seeded `test@localhost` INBOX (see e2e/fixtures/mail/), so subjects
// and content are fixed and assertions are deterministic. No mailbox-restore
// dance — the fixture inbox is disposable (`npm run e2e:env:down` wipes it).
//
// Run with the fixture up and the app pointed at it — see e2e/README.md.

import { test, before } from "node:test";
import assert from "node:assert/strict";
import { Bridge } from "./client.mjs";

const bridge = new Bridge();
const ROW = '.rows [role="option"]';

/** Find the message row whose subject matches and click it. */
async function openRow(subject) {
  await bridge.eval(
    `const rows = [...document.querySelectorAll('${ROW}')];
     const row = rows.find((r) => r.querySelector(".subject")?.textContent.trim() === ${JSON.stringify(subject)});
     if (!row) throw new Error("row not found: " + ${JSON.stringify(subject)});
     row.click();
     return true;`,
  );
}

before(async () => {
  await bridge.waitForReady();
});

test("app shell renders", async () => {
  assert.equal(await bridge.eval("return document.title;"), "Cosmic Mail");
});

test("fixture inbox syncs into the message list", async () => {
  await bridge.waitFor(`return document.querySelectorAll('${ROW}').length >= 3;`, {
    timeout: 30000,
  });
  const subjects = await bridge.eval(
    `return [...document.querySelectorAll('${ROW} .subject')].map((e) => e.textContent.trim());`,
  );
  assert.ok(subjects.includes("Welcome to Cosmic Mail"), `got: ${subjects.join(", ")}`);
  assert.ok(subjects.includes("Your order has shipped"), `got: ${subjects.join(", ")}`);
});

test("clicking a message opens it in the reader", async () => {
  await openRow("Welcome to Cosmic Mail");
  const subject = await bridge.waitForText("h1.r-subj");
  assert.equal(subject, "Welcome to Cosmic Mail");
});

test("a shipment email surfaces its tracking number", async () => {
  await openRow("Your order has shipped");
  await bridge.waitFor(
    `return document.querySelector("h1.r-subj")?.textContent.trim() === "Your order has shipped";`,
  );
  // Shipments render after the body loads and extraction runs — poll for it.
  const shipText = await bridge.waitFor(
    `const el = document.querySelector(".r-ships");
     return el && el.textContent.includes("1Z999AA10123456784") ? el.textContent.trim() : null;`,
    { timeout: 15000 },
  );
  assert.match(shipText, /1Z999AA10123456784/);
  assert.match(await bridge.text(".r-ships .carrier"), /UPS/i);
});
