//! Debug-only in-app automation bridge for scripted end-to-end testing.
//!
//! Arch/CachyOS `webkit2gtk-4.1` ships no `WebKitWebDriver` binary, so the
//! standard `tauri-driver` route is unavailable on this platform. Instead, in
//! `debug_assertions` builds only, we run a tiny localhost HTTP listener that
//! accepts a JavaScript snippet, evaluates it in the main webview via
//! [`WebviewWindow::eval_with_callback`], and returns the script's completion
//! value serialized as JSON. An external test client (see `e2e/`) drives the
//! real UI over this socket: find elements, click, and assert on text.
//!
//! This module is compiled out of release builds entirely (`cfg` at the call
//! site in `lib.rs`). It binds loopback only and never touches secrets.
//!
//! ## Contract
//!
//! - `GET  /health` → `{"ok":true}` once an eval round-trip through the webview
//!   succeeds, i.e. the page has committed and scripts get results back.
//! - `POST /eval`, body = raw JS → `{"ok":true,"value":<json>}` on success, or
//!   `{"ok":false,"error":"<message>"}` if the snippet throws. The snippet is
//!   wrapped in an IIFE, so `return <expr>;` yields the assertion value.
//!
//! WebKitGTK evaluates the snippet synchronously and does **not** await a
//! returned Promise: waiting for async UI (a click that renders a pane) is the
//! client's job, done by polling `/eval` until a condition holds.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::desktop::MAIN_WINDOW;

/// Default loopback port; override with `COSMIC_MAIL_AUTOMATION_PORT`.
const DEFAULT_PORT: u16 = 4127;

/// How long to wait for the webview to return an evaluation result.
const EVAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Largest request body we will read (guards against a runaway client).
const MAX_BODY: usize = 1 << 20; // 1 MiB

/// Spawn the automation listener on a background task. Failures to bind are
/// logged and otherwise ignored — the bridge is a testing convenience, never a
/// requirement for the app to run.
pub fn spawn(app: AppHandle) {
    let port = std::env::var("COSMIC_MAIL_AUTOMATION_PORT")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);

    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!(error = %err, port, "automation bridge failed to bind");
                return;
            }
        };
        tracing::info!(port, "automation bridge listening (debug build)");

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(err) = handle_connection(stream, &app).await {
                            tracing::debug!(error = %err, "automation connection error");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, "automation bridge accept failed");
                }
            }
        }
    });
}

/// A minimal parsed HTTP request: just what the bridge routes on.
struct Request {
    method: String,
    path: String,
    body: String,
}

/// Read and dispatch a single request, then write one response and close.
async fn handle_connection(mut stream: TcpStream, app: &AppHandle) -> anyhow::Result<()> {
    let request = match read_request(&mut stream).await? {
        Some(request) => request,
        // Empty/short read (e.g. a health probe that closed early): nothing to do.
        None => return Ok(()),
    };

    let (status, body) = match (request.method.as_str(), request.path.as_str()) {
        // Ready means an eval round-trip succeeds, not merely that the window
        // exists: before the first page commit, wry queues scripts and drops
        // their callbacks (webkitgtk `pending_scripts`), so a client that
        // started on window-exists would see every eval fail.
        ("GET", "/health") => {
            if eval(app, "return true;".to_string())
                .await
                .is_ok_and(|json| json.contains(r#""ok":true"#))
            {
                ("200 OK", r#"{"ok":true}"#.to_string())
            } else {
                (
                    "503 Service Unavailable",
                    error_envelope("webview not scriptable yet"),
                )
            }
        }
        ("POST", "/eval") => match eval(app, request.body).await {
            Ok(json) => ("200 OK", json),
            Err(err) => (
                "500 Internal Server Error",
                error_envelope(&err.to_string()),
            ),
        },
        _ => ("404 Not Found", error_envelope("unknown route")),
    };

    write_response(&mut stream, status, &body).await
}

/// Evaluate `js` in the main webview and return the callback's JSON string.
async fn eval(app: &AppHandle, js: String) -> anyhow::Result<String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| anyhow::anyhow!("main window unavailable"))?;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    // eval_with_callback wants an `Fn`, but the result arrives exactly once;
    // hold the oneshot sender in a Mutex<Option<_>> and take it on first call.
    let tx = Mutex::new(Some(tx));
    window.eval_with_callback(wrap_snippet(&js), move |result| {
        if let Ok(mut guard) = tx.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(result);
            }
        }
    })?;

    match tokio::time::timeout(EVAL_TIMEOUT, rx).await {
        Ok(Ok(json)) if json.is_empty() => {
            // WebKitGTK yields an empty string when the script itself fails to
            // produce a value (syntax error, or a throw the wrapper missed).
            Ok(error_envelope("evaluation produced no result"))
        }
        Ok(Ok(json)) => Ok(json),
        Ok(Err(_)) => Ok(error_envelope("evaluation callback dropped")),
        Err(_) => Ok(error_envelope("evaluation timed out")),
    }
}

/// Wrap the caller's snippet so its completion value is a JSON envelope and any
/// throw is reported rather than swallowed into an empty result.
fn wrap_snippet(js: &str) -> String {
    format!(
        "(() => {{ try {{ const __v = (() => {{ {js} }})(); \
         return {{ ok: true, value: __v === undefined ? null : __v }}; }} \
         catch (e) {{ return {{ ok: false, error: String((e && e.stack) || e) }}; }} }})()"
    )
}

/// Build a `{"ok":false,"error":...}` envelope with a JSON-escaped message.
fn error_envelope(message: &str) -> String {
    let escaped = message
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!(r#"{{"ok":false,"error":"{escaped}"}}"#)
}

/// Read one HTTP request: request line, headers, and a `Content-Length` body.
async fn read_request(stream: &mut TcpStream) -> anyhow::Result<Option<Request>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    // Read until we have the full header block (terminated by CRLF CRLF).
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None); // client closed before sending headers
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_BODY {
            anyhow::bail!("request headers too large");
        }
    };

    let head = std::str::from_utf8(&buf[..header_end])?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    if content_length > MAX_BODY {
        anyhow::bail!("request body too large");
    }

    // Body bytes already buffered past the header terminator, plus any more.
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Some(Request {
        method,
        path,
        body: String::from_utf8(body)?,
    }))
}

/// Write a single `Connection: close` JSON response.
async fn write_response(stream: &mut TcpStream, status: &str, body: &str) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n\
         {body}",
        len = body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Index of the first occurrence of `needle` within `haystack`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_snippet_returns_value_envelope() {
        let wrapped = wrap_snippet("return 1 + 1;");
        assert!(wrapped.contains("ok: true"));
        assert!(wrapped.contains("return 1 + 1;"));
    }

    #[test]
    fn error_envelope_escapes_quotes_and_newlines() {
        let json = error_envelope("bad \"thing\"\nhappened");
        assert_eq!(json, r#"{"ok":false,"error":"bad \"thing\"\nhappened"}"#);
    }

    #[test]
    fn find_subsequence_locates_header_terminator() {
        assert_eq!(find_subsequence(b"ab\r\n\r\ncd", b"\r\n\r\n"), Some(2));
        assert_eq!(find_subsequence(b"no terminator", b"\r\n\r\n"), None);
    }
}
