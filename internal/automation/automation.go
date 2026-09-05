//go:build !production

// Package automation is the development-only in-app bridge for scripted
// end-to-end testing.
//
// Arch's webkit2gtk-4.1 ships no WebKitWebDriver, so in non-production
// builds only, a tiny loopback HTTP listener accepts a JavaScript snippet,
// evaluates it in the main webview, and returns the script's completion
// value as JSON. An external test client (see e2e/) drives the real UI over
// this socket. The package is compiled out of production builds (see the
// build tags here and in stub.go). It binds loopback only and never touches
// secrets.
//
// Contract:
//   - GET  /health → {"ok":true} once an eval round-trip through the webview
//     succeeds, i.e. the page has committed and scripts get results back.
//   - POST /eval, body = raw JS → {"ok":true,"value":<json>} on success, or
//     {"ok":false,"error":"<message>"} if the snippet throws. The snippet is
//     wrapped in an IIFE, so `return <expr>;` yields the assertion value.
//
// WebKitGTK evaluates the snippet synchronously and does not await a
// returned Promise: waiting for async UI is the client's job, done by
// polling /eval until a condition holds. Wails' ExecJS has no return path,
// so the wrapper posts its result back through the webview's raw message
// channel, which main routes to HandleRawMessage.
package automation

import (
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"

	"cosmicmail/internal/desktop"
)

const (
	defaultPort   = 4127
	evalTimeout   = 15 * time.Second
	maxBody       = 1 << 20
	messagePrefix = "cosmic-automation:"
)

// Enabled reports whether the bridge is compiled in.
const Enabled = true

var (
	nextID   atomic.Uint64
	pendingM sync.Mutex
	pending  = map[uint64]chan string{}
)

// Spawn starts the automation listener. Bind failures are logged and
// otherwise ignored — the bridge is a testing convenience, never a
// requirement for the app to run.
func Spawn(app *application.App) {
	port := defaultPort
	if raw := os.Getenv("COSMIC_MAIL_AUTOMATION_PORT"); raw != "" {
		if p, err := strconv.Atoi(raw); err == nil && p > 0 && p < 65536 {
			port = p
		}
	}
	listener, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		slog.Warn("automation bridge failed to bind", "port", port, "error", err)
		return
	}
	slog.Info("automation bridge listening (development build)", "port", port)
	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Connection", "close")
		// Ready means an eval round-trip succeeds, not merely that the window
		// exists: before the first page commit ExecJS is queued.
		if r.Method == http.MethodGet && strings.Contains(eval(app, "return true;"), `"ok":true`) {
			writeJSON(w, http.StatusOK, `{"ok":true}`)
			return
		}
		writeJSON(w, http.StatusServiceUnavailable, ErrorEnvelope("webview not scriptable yet"))
	})
	mux.HandleFunc("/eval", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Connection", "close")
		if r.Method != http.MethodPost {
			writeJSON(w, http.StatusNotFound, ErrorEnvelope("unknown route"))
			return
		}
		body, err := io.ReadAll(io.LimitReader(r.Body, maxBody+1))
		if err != nil || len(body) > maxBody {
			writeJSON(w, http.StatusInternalServerError, ErrorEnvelope("request body too large"))
			return
		}
		writeJSON(w, http.StatusOK, eval(app, string(body)))
	})
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusNotFound, ErrorEnvelope("unknown route"))
	})
	server := &http.Server{Handler: mux, ReadHeaderTimeout: 10 * time.Second}
	go func() {
		if err := server.Serve(listener); err != nil && err != http.ErrServerClosed {
			slog.Warn("automation bridge stopped", "error", err)
		}
	}()
}

func writeJSON(w http.ResponseWriter, status int, body string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = io.WriteString(w, body)
}

// eval evaluates js in the main webview and returns the JSON envelope.
func eval(app *application.App, js string) string {
	window, ok := app.Window.GetByName(desktop.MainWindow)
	if !ok || window == nil {
		return ErrorEnvelope("main window unavailable")
	}
	id := nextID.Add(1)
	ch := make(chan string, 1)
	pendingM.Lock()
	pending[id] = ch
	pendingM.Unlock()
	defer func() {
		pendingM.Lock()
		delete(pending, id)
		pendingM.Unlock()
	}()
	window.ExecJS(WrapSnippet(id, js))
	select {
	case result := <-ch:
		if result == "" {
			return ErrorEnvelope("evaluation produced no result")
		}
		return result
	case <-time.After(evalTimeout):
		return ErrorEnvelope("evaluation timed out")
	}
}

// HandleRawMessage is the application's RawMessageHandler: it routes the
// wrapper's `cosmic-automation:<id>:<json>` reply to the waiting eval.
func HandleRawMessage(_ application.Window, message string, _ *application.OriginInfo) {
	id, payload, ok := ParseReply(message)
	if !ok {
		return
	}
	pendingM.Lock()
	ch := pending[id]
	pendingM.Unlock()
	if ch != nil {
		select {
		case ch <- payload:
		default:
		}
	}
}

// ParseReply splits a bridge reply message into its id and JSON payload.
func ParseReply(message string) (uint64, string, bool) {
	if !strings.HasPrefix(message, messagePrefix) {
		return 0, "", false
	}
	rest := strings.TrimPrefix(message, messagePrefix)
	idText, payload, ok := strings.Cut(rest, ":")
	if !ok {
		return 0, "", false
	}
	id, err := strconv.ParseUint(idText, 10, 64)
	if err != nil {
		return 0, "", false
	}
	return id, payload, true
}

// WrapSnippet wraps the caller's snippet so its completion value is posted
// back as a JSON envelope and any throw is reported rather than swallowed.
func WrapSnippet(id uint64, js string) string {
	return fmt.Sprintf(`(() => {
  const __send = (r) => {
    let json;
    try { json = JSON.stringify(r); } catch (e) { json = JSON.stringify({ ok: false, error: "unserializable result: " + String(e) }); }
    window.webkit.messageHandlers.external.postMessage(%q + json);
  };
  try {
    const __v = (() => { %s })();
    __send({ ok: true, value: __v === undefined ? null : __v });
  } catch (e) {
    __send({ ok: false, error: String((e && e.stack) || e) });
  }
})();`, fmt.Sprintf("%s%d:", messagePrefix, id), js)
}

// ErrorEnvelope builds a {"ok":false,"error":…} envelope.
func ErrorEnvelope(message string) string {
	b, _ := json.Marshal(map[string]any{"ok": false, "error": message})
	return string(b)
}
