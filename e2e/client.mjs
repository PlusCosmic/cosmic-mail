// Client for the debug-only in-app automation bridge (see
// src-tauri/src/automation.rs). Talks to the loopback listener the app opens
// in debug builds: POST a JS snippet, get its completion value back as JSON.
//
// Zero dependencies — Node's built-in http only. Async UI waits are done here,
// on the client, by polling: WebKitGTK evaluates each snippet synchronously and
// does not await a returned Promise.

import http from "node:http";

const PORT = Number(process.env.COSMIC_MAIL_AUTOMATION_PORT ?? 4127);
const HOST = "127.0.0.1";

/** POST `body` to `path` and resolve with the response text. */
function request(path, method, body) {
  return new Promise((resolve, reject) => {
    const payload = body ?? "";
    const req = http.request(
      { host: HOST, port: PORT, path, method, headers: { "Content-Length": Buffer.byteLength(payload) } },
      (res) => {
        let data = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => resolve(data));
      },
    );
    req.on("error", reject);
    req.end(payload);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export class Bridge {
  /** Wait until the bridge answers /health (app up + webview reachable). */
  async waitForReady({ timeout = 30000, interval = 250 } = {}) {
    const deadline = Date.now() + timeout;
    for (;;) {
      try {
        const res = JSON.parse(await request("/health", "GET"));
        if (res.ok) return;
      } catch {
        // not listening yet
      }
      if (Date.now() > deadline) throw new Error("automation bridge never became ready");
      await sleep(interval);
    }
  }

  /**
   * Evaluate a JS snippet in the webview. The snippet is a function body, so
   * `return <expr>;` yields the value. Throws if the snippet throws.
   */
  async eval(js) {
    const raw = await request("/eval", "POST", js);
    let envelope;
    try {
      envelope = JSON.parse(raw);
    } catch {
      throw new Error(`bridge returned non-JSON: ${raw.slice(0, 200)}`);
    }
    if (!envelope.ok) throw new Error(`eval failed: ${envelope.error}`);
    return envelope.value;
  }

  /** Poll `eval(js)` until it returns a truthy value or the timeout elapses. */
  async waitFor(js, { timeout = 10000, interval = 150 } = {}) {
    const deadline = Date.now() + timeout;
    for (;;) {
      const value = await this.eval(js);
      if (value) return value;
      if (Date.now() > deadline) throw new Error(`waitFor timed out: ${js}`);
      await sleep(interval);
    }
  }

  // --- Convenience DOM helpers (all take a CSS selector) ---

  /** Number of elements matching `selector`. */
  count(selector) {
    return this.eval(`return document.querySelectorAll(${JSON.stringify(selector)}).length;`);
  }

  /** trimmed textContent of the first match, or null if none. */
  text(selector) {
    return this.eval(
      `const el = document.querySelector(${JSON.stringify(selector)});
       return el ? el.textContent.trim() : null;`,
    );
  }

  /** Click the first match. Throws if nothing matches. */
  click(selector) {
    return this.eval(
      `const el = document.querySelector(${JSON.stringify(selector)});
       if (!el) throw new Error("no element for selector: " + ${JSON.stringify(selector)});
       el.click();
       return true;`,
    );
  }

  /** Wait until at least one element matches `selector`, then return its text. */
  async waitForText(selector, opts) {
    await this.waitFor(
      `return document.querySelector(${JSON.stringify(selector)}) != null;`,
      opts,
    );
    return this.text(selector);
  }
}
