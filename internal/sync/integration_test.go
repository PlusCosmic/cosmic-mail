package sync_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	gosync "sync"
	"testing"
	"time"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/models"
	"cosmicmail/internal/store"
	mailsync "cosmicmail/internal/sync"
	"cosmicmail/internal/testimap"
)

// recorder collects emitted events and notifications.
type recorder struct {
	mu      gosync.Mutex
	events  []string
	payload map[string][]any
	notes   [][]mailsync.NewMail
}

func (r *recorder) Emit(name string, data any) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, name)
	if r.payload == nil {
		r.payload = map[string][]any{}
	}
	r.payload[name] = append(r.payload[name], data)
}

func (r *recorder) NotifyNewMail(_ string, mail []mailsync.NewMail) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.notes = append(r.notes, mail)
}

func (r *recorder) count(name string) int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.payload[name])
}

func (r *recorder) states() []models.SyncState {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []models.SyncState
	for _, p := range r.payload[models.EventSyncState] {
		out = append(out, p.(models.SyncStateEvent).State)
	}
	return out
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func fixtures(t *testing.T) [][]byte {
	t.Helper()
	dir := filepath.Join("..", "..", "e2e", "fixtures", "mail")
	names, err := filepath.Glob(filepath.Join(dir, "*.eml"))
	if err != nil || len(names) == 0 {
		t.Fatalf("no fixtures: %v", err)
	}
	var out [][]byte
	for _, n := range names {
		b, err := os.ReadFile(n)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, b)
	}
	return out
}

// Drives the whole engine against a real IMAP server (go-imap's in-memory
// server over TLS): the initial sweep, events, body prefetch with shipment
// detection, then a message arriving during IDLE, the way the GreenMail
// fixture does in e2e/.
func TestSyncAgainstInMemoryIMAPServer(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(root, "data"))
	server, err := testimap.Start("127.0.0.1:0", map[string]string{"test": "test-pass"})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	caPath := filepath.Join(root, "ca.pem")
	if err := os.WriteFile(caPath, server.CAPEM, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("COSMIC_MAIL_EXTRA_CA", caPath)
	t.Setenv("COSMIC_MAIL_TEST_IMAP_PASSWORD", "test-pass")
	for _, raw := range fixtures(t) {
		if err := server.Seed("test", "INBOX", raw); err != nil {
			t.Fatal(err)
		}
	}

	st, err := store.OpenPath(":memory:")
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	rec := &recorder{}
	manager := mailsync.NewManager(st, rec, rec)
	account := accounts.Account{ID: "acct", Email: "test@localhost", DisplayName: "E2E", Kind: models.AccountKindImap,
		ImapHost: "localhost", ImapPort: server.Port(), SmtpHost: "localhost", SmtpPort: 3025, Username: "test"}
	manager.Start(account)
	defer manager.StopAll()

	// The initial sweep lands the three fixture messages and settles to idle.
	waitFor(t, "initial sync", func() bool {
		rows, _ := st.PageUnifiedMessages(0, 50)
		states := rec.states()
		return len(rows) == 3 && len(states) > 0 && states[len(states)-1] == models.SyncIdle
	})
	folders, _ := st.ListFolders("acct")
	roles := map[string]string{}
	for _, f := range folders {
		roles[f.Name] = f.Role
	}
	if roles["INBOX"] != "inbox" || roles["Trash"] != "trash" || roles["Archive"] != "archive" {
		t.Fatalf("folder roles: %v", roles)
	}
	inbox, _ := st.FindFolderByRole("acct", store.RoleInbox)
	if inbox.TotalCount != 3 || inbox.UnreadCount != 3 {
		t.Fatalf("inbox counts: %+v", inbox)
	}
	rows, _ := st.PageMessages(inbox.ID, 0, 50)
	subjects := map[string]bool{}
	for _, r := range rows {
		subjects[r.Subject] = true
	}
	if !subjects["Welcome to Cosmic Mail"] || !subjects["Your order has shipped"] {
		t.Fatalf("subjects: %v", subjects)
	}
	if rec.count(models.EventNewMessages) == 0 || rec.count(models.EventMessagesUpdated) == 0 {
		t.Fatalf("events: %v", rec.events)
	}
	if len(rec.notes) != 0 {
		t.Fatalf("the initial sweep must not notify: %v", rec.notes)
	}
	// The new-messages payload is the wire shape the frontend reads.
	b, _ := json.Marshal(rec.payload[models.EventNewMessages][0])
	if !strings.Contains(string(b), `"accountId":"acct"`) || !strings.Contains(string(b), `"messages":[`) {
		t.Fatalf("payload: %s", b)
	}

	// Prefetch cached the bodies (all well under 1 MiB) and detected the UPS
	// shipment from the fixture.
	waitFor(t, "body prefetch", func() bool {
		for _, r := range rows {
			body, _ := st.GetBody(r.ID)
			if body == nil || !body.Cached {
				return false
			}
		}
		return true
	})
	var shipped int64
	for _, r := range rows {
		if r.Subject == "Your order has shipped" {
			shipped = r.ID
		}
	}
	ships, _ := st.ListShipments(shipped)
	if len(ships) != 1 || ships[0].Carrier != "ups" || *ships[0].TrackingNumber != "1Z999AA10123456784" {
		t.Fatalf("shipments: %+v", ships)
	}
	rows, _ = st.PageMessages(inbox.ID, 0, 50)
	for _, r := range rows {
		if r.Snippet == "" {
			t.Fatalf("snippet not derived for %q", r.Subject)
		}
	}

	// New mail while the loop is idling: the IDLE wakeup re-syncs, emits, and
	// notifies (the UID is above the sweep's last_seen_uid baseline).
	before := rec.count(models.EventNewMessages)
	raw := "From: Later <later@example.com>\r\nTo: test@localhost\r\nSubject: Arrived during idle\r\nDate: Wed, 14 Jan 2026 09:00:00 +0000\r\nMessage-ID: <idle-1@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello from IDLE.\r\n"
	if err := server.Seed("test", "INBOX", []byte(raw)); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "idle wakeup", func() bool {
		rows, _ := st.PageMessages(inbox.ID, 0, 50)
		return len(rows) == 4 && rec.count(models.EventNewMessages) > before
	})
	rec.mu.Lock()
	notes := rec.notes
	rec.mu.Unlock()
	if len(notes) != 1 || len(notes[0]) != 1 || notes[0][0].Subject != "Arrived during idle" || notes[0][0].FromName != "Later" {
		t.Fatalf("notifications: %+v", notes)
	}
}
