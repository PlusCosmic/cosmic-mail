package accounts

import (
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/zalando/go-keyring"

	"cosmicmail/internal/models"
)

func useMockKeyring(t *testing.T) map[string]string {
	t.Helper()
	store := map[string]string{}
	keyringGet = func(service, user string) (string, error) {
		v, ok := store[service+"/"+user]
		if !ok {
			return "", keyring.ErrNotFound
		}
		return v, nil
	}
	keyringSet = func(service, user, pass string) error { store[service+"/"+user] = pass; return nil }
	keyringDelete = func(service, user string) error { delete(store, service+"/"+user); return nil }
	t.Cleanup(func() { keyringGet, keyringSet, keyringDelete = keyring.Get, keyring.Set, keyring.Delete })
	return store
}

func TestLoadIsEmptyWithoutFileAndRoundTrips(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	got, err := Load()
	if err != nil || got == nil || len(got) != 0 {
		t.Fatalf("Load on fresh profile: %#v %v", got, err)
	}
	a := Account{ID: "id-1", Email: "a@b.com", DisplayName: "A", Kind: models.AccountKindImap, ImapHost: "imap.b.com", ImapPort: 993, SmtpHost: "smtp.b.com", SmtpPort: 587, Username: "a@b.com"}
	if err := Add(a); err != nil {
		t.Fatal(err)
	}
	found, err := Find("id-1")
	if err != nil || found != a {
		t.Fatalf("Find: %+v %v", found, err)
	}
	if _, err := Find("missing"); err == nil {
		t.Fatal("missing account should error")
	}
	// The on-disk shape is the camelCase one the Rust build wrote.
	path, _ := Path()
	raw, _ := os.ReadFile(path)
	for _, key := range []string{`"displayName"`, `"imapHost"`, `"smtpPort"`, `"kind": "imap"`} {
		if !contains(string(raw), key) {
			t.Fatalf("accounts.json missing %s: %s", key, raw)
		}
	}
	removed, err := Remove("id-1")
	if err != nil || removed == nil || removed.ID != "id-1" {
		t.Fatalf("Remove: %+v %v", removed, err)
	}
	if removed, err := Remove("id-1"); err != nil || removed != nil {
		t.Fatalf("second Remove: %+v %v", removed, err)
	}
	if got, _ := Load(); len(got) != 0 {
		t.Fatalf("still has accounts: %+v", got)
	}
}

func TestReadsTheRustBuildsFixtureShape(t *testing.T) {
	cfg := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", cfg)
	src, err := os.ReadFile(filepath.Join("..", "..", "e2e", "profile", "accounts.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(cfg, "cosmic-mail"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(cfg, "cosmic-mail", "accounts.json"), src, 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := Load()
	if err != nil || len(got) != 1 {
		t.Fatalf("%+v %v", got, err)
	}
	if got[0].Email != "test@localhost" || got[0].ImapPort != 3993 || got[0].Kind != models.AccountKindImap {
		t.Fatalf("%+v", got[0])
	}
}

func TestKeyringHelpers(t *testing.T) {
	store := useMockKeyring(t)
	t.Setenv("COSMIC_MAIL_TEST_IMAP_PASSWORD", "")
	os.Unsetenv("COSMIC_MAIL_TEST_IMAP_PASSWORD")

	if _, err := GetImapPassword("x"); !errors.Is(err, ErrNoSecret) {
		t.Fatalf("missing entry should be ErrNoSecret, got %v", err)
	}
	if err := SetImapPassword("x", "hunter2"); err != nil {
		t.Fatal(err)
	}
	if pw, err := GetImapPassword("x"); err != nil || pw != "hunter2" {
		t.Fatalf("%q %v", pw, err)
	}
	if _, ok := store[KeyringService+"/imap-password:x"]; !ok {
		t.Fatalf("key layout changed: %v", store)
	}
	if err := SetOAuthRefreshToken("g", "tok"); err != nil {
		t.Fatal(err)
	}
	if tok, err := GetOAuthRefreshToken("g"); err != nil || tok != "tok" {
		t.Fatalf("%q %v", tok, err)
	}
	DeleteSecrets("x", models.AccountKindImap)
	DeleteSecrets("g", models.AccountKindGmail)
	if len(store) != 0 {
		t.Fatalf("secrets left behind: %v", store)
	}
}

func TestTestPasswordHookBypassesKeyring(t *testing.T) {
	useMockKeyring(t)
	t.Setenv("COSMIC_MAIL_TEST_IMAP_PASSWORD", "test-pass")
	if pw, err := GetImapPassword("anything"); err != nil || pw != "test-pass" {
		t.Fatalf("%q %v", pw, err)
	}
}

func contains(s, sub string) bool {
	return len(sub) == 0 || (len(s) >= len(sub) && indexOf(s, sub) >= 0)
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}
