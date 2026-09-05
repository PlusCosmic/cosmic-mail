package main

import (
	"os"
	"path/filepath"
	"testing"

	"cosmicmail/internal/models"
	"cosmicmail/internal/store"
	mailsync "cosmicmail/internal/sync"
)

type recorder struct{ events []string }

func (r *recorder) Emit(name string, _ any) { r.events = append(r.events, name) }

type noNotifier struct{}

func (noNotifier) NotifyNewMail(string, []mailsync.NewMail) {}
func (noNotifier) Test()                                    {}

// Drives the bound service end to end against a scratch profile, the way
// the frontend does — everything that needs no server or browser.
func TestServiceRoundTrip(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(root, "data"))
	t.Setenv("HOME", filepath.Join(root, "home"))

	st, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	if _, err := os.Stat(filepath.Join(root, "data", "cosmic-mail", "mail.db")); err != nil {
		t.Fatalf("db not created under XDG_DATA_HOME: %v", err)
	}
	rec := &recorder{}
	app := NewApp(st, mailsync.NewManager(st, rec, noNotifier{}), rec, noNotifier{})

	if theme := app.GetTheme(); theme.Name != "kanagawa" || len(theme.Palette) != 16 {
		t.Fatalf("GetTheme on a bare profile should fall back: %+v", theme)
	}
	if got, err := app.ListAccounts(); err != nil || got == nil || len(got) != 0 {
		t.Fatalf("ListAccounts: %#v %v", got, err)
	}
	if got := app.GetSettings(); got.AlwaysDownloadRemoteImages {
		t.Fatalf("GetSettings: %+v", got)
	}
	saved, err := app.UpdateSettings(models.Settings{AlwaysDownloadRemoteImages: true})
	if err != nil || !saved.AlwaysDownloadRemoteImages || !app.GetSettings().AlwaysDownloadRemoteImages {
		t.Fatalf("UpdateSettings: %+v %v", saved, err)
	}

	// Seed a folder and a message directly, then read them back through the
	// service the way the message list does.
	folderID, _, err := st.UpsertFolder("acct", "INBOX", store.RoleInbox, 1)
	if err != nil {
		t.Fatal(err)
	}
	msgID, err := st.UpsertMessage(&store.MessageUpsert{FolderID: folderID, UID: 1, Subject: "Welcome", FromName: "Cosmic", FromAddr: "hi@example.com", Date: "2026-07-10T00:00:00+00:00", Seen: false})
	if err != nil {
		t.Fatal(err)
	}
	folders, err := app.ListFolders("acct")
	if err != nil || len(folders) != 1 || folders[0].Role != "inbox" {
		t.Fatalf("ListFolders: %+v %v", folders, err)
	}
	if got, err := app.ListFolders("nobody"); err != nil || got == nil || len(got) != 0 {
		t.Fatalf("ListFolders unknown: %#v %v", got, err)
	}
	msgs, err := app.ListMessages(folderID, 0, 50)
	if err != nil || len(msgs) != 1 || msgs[0].AccountID != "acct" || msgs[0].Subject != "Welcome" {
		t.Fatalf("ListMessages: %+v %v", msgs, err)
	}
	if _, err := app.ListMessages(999, 0, 50); err == nil {
		t.Fatal("ListMessages on a missing folder should error")
	}
	unified, err := app.ListUnifiedMessages(0, 50)
	if err != nil || len(unified) != 1 || unified[0].ID != msgID {
		t.Fatalf("ListUnifiedMessages: %+v %v", unified, err)
	}
	hits, err := app.SearchMessages("welc", nil, 0, 50)
	if err != nil || len(hits) != 1 {
		t.Fatalf("SearchMessages: %+v %v", hits, err)
	}
	if empty, err := app.SearchMessages("   ", nil, 0, 50); err != nil || empty == nil || len(empty) != 0 {
		t.Fatalf("SearchMessages blank: %#v %v", empty, err)
	}
	if ships, err := app.ListShipmentsForMessage(msgID); err != nil || ships == nil || len(ships) != 0 {
		t.Fatalf("ListShipmentsForMessage: %#v %v", ships, err)
	}
	// A cached body is served without a server.
	html := "<p>Tracking: 1Z999AA10123456784</p>"
	if _, err := st.CacheParsedBody(msgID, models.Str("Tracking: 1Z999AA10123456784"), &html, []string{"a@b.com"}, nil, models.Str("Tracking"), nil, []store.ShipmentInsert{{Carrier: "ups", TrackingNumber: models.Str("1Z999AA10123456784")}}); err != nil {
		t.Fatal(err)
	}
	body, err := app.GetMessageBody(msgID)
	if err != nil || body.HTML == nil || *body.HTML != html || len(body.ToAddrs) != 1 || body.CcAddrs == nil || body.Attachments == nil {
		t.Fatalf("GetMessageBody: %+v %v", body, err)
	}
	ships, err := app.ListShipmentsForMessage(msgID)
	if err != nil || len(ships) != 1 || ships[0].Carrier != "ups" {
		t.Fatalf("ListShipmentsForMessage: %+v %v", ships, err)
	}
	if _, err := app.GetMessageBody(12345); err == nil {
		t.Fatal("GetMessageBody on a missing row should error")
	}
	if _, err := app.DiscoverAccountConfig("not-an-address"); err == nil {
		t.Fatal("DiscoverAccountConfig should reject an invalid address")
	}
	if err := app.SyncAccount("missing"); err == nil {
		t.Fatal("SyncAccount on an unknown account should error")
	}
	if err := app.RemoveAccount("missing"); err != nil {
		t.Fatalf("RemoveAccount on an unknown account is a no-op: %v", err)
	}
	if err := app.TestNotification(); err != nil {
		t.Fatal(err)
	}
}
