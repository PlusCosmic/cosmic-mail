//go:build !production

package automation

import (
	"strings"
	"testing"
)

func TestWrapSnippetReturnsValueEnvelope(t *testing.T) {
	wrapped := WrapSnippet(7, "return 1 + 1;")
	if !strings.Contains(wrapped, "ok: true") || !strings.Contains(wrapped, "return 1 + 1;") || !strings.Contains(wrapped, `"cosmic-automation:7:"`) {
		t.Fatal(wrapped)
	}
}

func TestErrorEnvelopeEscapesQuotesAndNewlines(t *testing.T) {
	if got := ErrorEnvelope("bad \"thing\"\nhappened"); got != `{"error":"bad \"thing\"\nhappened","ok":false}` {
		t.Fatal(got)
	}
}

func TestParseReply(t *testing.T) {
	id, payload, ok := ParseReply(`cosmic-automation:42:{"ok":true,"value":"a:b"}`)
	if !ok || id != 42 || payload != `{"ok":true,"value":"a:b"}` {
		t.Fatalf("%d %q %v", id, payload, ok)
	}
	if _, _, ok := ParseReply("wails:drag"); ok {
		t.Fatal("foreign message")
	}
	if _, _, ok := ParseReply("cosmic-automation:x:{}"); ok {
		t.Fatal("bad id")
	}
}
