package oauth

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestAuthExpiredMarkerIsDetectableThroughWrapChain(t *testing.T) {
	err := fmt.Errorf("connecting IMAP: %w", fmt.Errorf("obtaining Gmail access token: %w", fmt.Errorf("refreshing access token: %w: %w", ErrAuthExpired, errors.New("invalid_grant"))))
	if !IsAuthExpired(err) {
		t.Fatal("marker lost through the chain")
	}
	if IsAuthExpired(fmt.Errorf("connecting IMAP: %w", errors.New("connection refused"))) {
		t.Fatal("plain error classified as expired")
	}
}

func TestCredentialsPrecedence(t *testing.T) {
	cfg := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", cfg)
	t.Setenv("COSMIC_MAIL_GOOGLE_CLIENT_ID", "")
	t.Setenv("COSMIC_MAIL_GOOGLE_CLIENT_SECRET", "")
	BakedClientID, BakedClientSecret = "", ""
	t.Cleanup(func() { BakedClientID, BakedClientSecret = "", "" })

	if _, _, err := credentials(); err == nil {
		t.Fatal("unconfigured should error")
	}
	BakedClientID, BakedClientSecret = "baked-id", "baked-secret"
	if id, secret, err := credentials(); err != nil || id != "baked-id" || secret != "baked-secret" {
		t.Fatalf("baked: %s %s %v", id, secret, err)
	}
	dir := filepath.Join(cfg, "cosmic-mail")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(filepath.Join(dir, "google-oauth.json"), []byte(`{"clientId": "file-id"}`), 0o644)
	if id, secret, err := credentials(); err != nil || id != "file-id" || secret != "" {
		t.Fatalf("file: %s %s %v", id, secret, err)
	}
	t.Setenv("COSMIC_MAIL_GOOGLE_CLIENT_ID", "env-id")
	t.Setenv("COSMIC_MAIL_GOOGLE_CLIENT_SECRET", "env-secret")
	if id, secret, err := credentials(); err != nil || id != "env-id" || secret != "env-secret" {
		t.Fatalf("env: %s %s %v", id, secret, err)
	}
}

func TestXOAuth2InitialResponseIsRaw(t *testing.T) {
	c := &XOAuth2Client{User: "me@gmail.com", Token: "tok"}
	mech, ir, err := c.Start()
	if err != nil || mech != "XOAUTH2" || string(ir) != "user=me@gmail.com\x01auth=Bearer tok\x01\x01" {
		t.Fatalf("%s %q %v", mech, ir, err)
	}
	if next, err := c.Next([]byte(`{"status":"400"}`)); err != nil || next == nil || len(next) != 0 {
		t.Fatalf("%q %v", next, err)
	}
}

func TestTokenCache(t *testing.T) {
	CacheToken("acct", "tok", 0)
	Forget("acct")
	cacheMu.Lock()
	_, ok := tokenCache["acct"]
	cacheMu.Unlock()
	if ok {
		t.Fatal("not forgotten")
	}
}
