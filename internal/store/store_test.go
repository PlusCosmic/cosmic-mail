package store_test

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"cosmicmail/internal/mailparse"
	"cosmicmail/internal/models"
	"cosmicmail/internal/store"
)

func open(t *testing.T) *store.Store {
	t.Helper()
	s, err := store.OpenPath(":memory:")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func folder(t *testing.T, s *store.Store, account, name string, role store.FolderRole) int64 {
	t.Helper()
	id, _, err := s.UpsertFolder(account, name, role, 1)
	if err != nil {
		t.Fatal(err)
	}
	return id
}

func upsert(t *testing.T, s *store.Store, m store.MessageUpsert) int64 {
	t.Helper()
	id, err := s.UpsertMessage(&m)
	if err != nil {
		t.Fatal(err)
	}
	return id
}

func msg(folderID int64, uid uint32) store.MessageUpsert {
	return store.MessageUpsert{FolderID: folderID, UID: uid, Subject: "s", FromAddr: "a@b.com", Date: fmt.Sprintf("2026-07-12T00:00:%02dZ", uid), Seen: true, RFC822Size: 100}
}

func insertSearchable(t *testing.T, s *store.Store, folderID int64, uid uint32, subject, fromName, fromAddr, snippet string) int64 {
	m := msg(folderID, uid)
	m.Subject, m.FromName, m.FromAddr, m.Snippet = subject, fromName, fromAddr, snippet
	return upsert(t, s, m)
}

func TestServerCountsAreIndependentOfCachedMessageCount(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "account", "INBOX", store.RoleInbox)
	if err := s.SetFolderCounts(fid, 1497, 12); err != nil {
		t.Fatal(err)
	}
	m := msg(fid, 1690)
	m.Subject, m.FromName = "test", "Sender"
	id := upsert(t, s, m)
	if changed, _ := s.MarkSeen(id, false); !changed {
		t.Fatal("mark unread")
	}
	s.AdjustFolderUnreadCount(fid, 1)
	f, _ := s.GetFolder(fid)
	if f.TotalCount != 1497 || f.UnreadCount != 13 {
		t.Fatalf("%+v", f)
	}
	if changed, _ := s.MarkSeen(id, true); !changed {
		t.Fatal("mark read")
	}
	s.AdjustFolderUnreadCount(fid, -1)
	if changed, _ := s.MarkSeen(id, true); changed {
		t.Fatal("repeat mark read changed")
	}
	f, _ = s.GetFolder(fid)
	if f.TotalCount != 1497 || f.UnreadCount != 12 {
		t.Fatalf("%+v", f)
	}
}

func TestReplyMetadataKeepsAccountAndMessageID(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "account", "INBOX", store.RoleInbox)
	m := msg(fid, 42)
	m.MessageID = models.Str("<parent@example.com>")
	id := upsert(t, s, m)
	md, err := s.GetReplyMetadata(id)
	if err != nil || md == nil || md.AccountID != "account" || md.MessageID == nil || *md.MessageID != "<parent@example.com>" {
		t.Fatalf("%+v %v", md, err)
	}
	if md, _ := s.GetReplyMetadata(999); md != nil {
		t.Fatal("missing row")
	}
}

func TestBodyPrefetchIsBoundedPrioritisedAndTracksEmptyBodies(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "account", "INBOX", store.RoleInbox)
	var ids []int64
	for uid := uint32(1); uid <= 6; uid++ {
		m := msg(fid, uid)
		m.Subject = fmt.Sprintf("message %d", uid)
		m.Seen = uid != 2
		if uid == 6 {
			m.RFC822Size = 2_000_000
		}
		ids = append(ids, upsert(t, s, m))
	}
	c, err := s.BodyPrefetchCandidates(fid, 5, 3, 1_048_576)
	if err != nil || len(c) != 3 || c[0].UID != 2 {
		t.Fatalf("%+v %v", c, err)
	}
	for _, x := range c {
		if x.UID == 6 {
			t.Fatal("oversize candidate")
		}
	}
	empty := ids[1]
	if err := s.SetBody(empty, nil, nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	body, _ := s.GetBody(empty)
	if !body.Cached || body.Text != nil || body.HTML != nil || body.ToAddrs == nil {
		t.Fatalf("%+v", body)
	}
	c, _ = s.BodyPrefetchCandidates(fid, 5, 5, 1_048_576)
	for _, x := range c {
		if x.ID == empty {
			t.Fatal("empty body re-candidate")
		}
	}
}

func TestReplaceAttachmentsReconcilesListsAndLocates(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := upsert(t, s, msg(fid, 5))
	metas := []store.AttachmentMeta{
		{PartIndex: 2, Filename: "report.pdf", MimeType: "application/pdf", SizeBytes: 1024},
		{PartIndex: 3, MimeType: "image/png", SizeBytes: 200, IsInline: true, ContentID: models.Str("logo@x")},
	}
	if err := s.ReplaceAttachments(id, metas); err != nil {
		t.Fatal(err)
	}
	rows, _ := s.PageMessages(fid, 0, 10)
	if len(rows) != 1 || !rows[0].HasAttachments {
		t.Fatalf("%+v", rows)
	}
	listed, _ := s.ListAttachments(id)
	if len(listed) != 2 || listed[0].Filename != "report.pdf" || listed[0].MimeType != "application/pdf" || listed[0].SizeBytes != 1024 || listed[0].IsInline || !listed[1].IsInline {
		t.Fatalf("%+v", listed)
	}
	loc, _ := s.GetAttachment(listed[0].ID)
	if loc == nil || loc.MessageID != id || loc.PartIndex != 2 || loc.UID != 5 || loc.AccountID != "acct" || loc.FolderName != "INBOX" {
		t.Fatalf("%+v", loc)
	}
	if err := s.ReplaceAttachments(id, []store.AttachmentMeta{{PartIndex: 1, MimeType: "image/gif", SizeBytes: 10, IsInline: true, ContentID: models.Str("only-inline@x")}}); err != nil {
		t.Fatal(err)
	}
	rows, _ = s.PageMessages(fid, 0, 10)
	if rows[0].HasAttachments {
		t.Fatal("inline only should clear has_attachments")
	}
	if l, _ := s.ListAttachments(id); len(l) != 1 {
		t.Fatal("list")
	}
	if err := s.RemoveMessage(id); err != nil {
		t.Fatal(err)
	}
	if l, _ := s.ListAttachments(id); len(l) != 0 {
		t.Fatal("cascade")
	}
}

func TestReplaceShipmentsDedupesAndCascades(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := upsert(t, s, msg(fid, 9))
	ships := []store.ShipmentInsert{
		{Carrier: "ups", TrackingNumber: models.Str("1Z999AA10123456784"), TrackingURL: models.Str("https://www.ups.com/track?tracknum=1Z999AA10123456784")},
		{Carrier: "amazon", OrderID: models.Str("123-4567890-1234567")},
	}
	if err := s.ReplaceShipments(id, ships); err != nil {
		t.Fatal(err)
	}
	if err := s.ReplaceShipments(id, ships); err != nil {
		t.Fatal(err)
	}
	listed, _ := s.ListShipments(id)
	if len(listed) != 2 || listed[0].Carrier != "ups" || *listed[0].TrackingNumber != "1Z999AA10123456784" || listed[1].Carrier != "amazon" || *listed[1].OrderID != "123-4567890-1234567" || listed[0].DetectedAt == "" {
		t.Fatalf("%+v", listed)
	}
	s.ReplaceShipments(id, ships[:1])
	if l, _ := s.ListShipments(id); len(l) != 1 {
		t.Fatal("fewer")
	}
	s.RemoveMessage(id)
	if l, _ := s.ListShipments(id); len(l) != 0 {
		t.Fatal("cascade")
	}
}

func TestSetFlaggedReportsChanges(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := insertSearchable(t, s, fid, 1, "s", "f", "a@b.com", "")
	if ch, _ := s.SetFlagged(id, true); !ch {
		t.Fatal("flag")
	}
	if ch, _ := s.SetFlagged(id, true); ch {
		t.Fatal("flag again")
	}
	rows, _ := s.PageMessages(fid, 0, 10)
	if !rows[0].Flagged {
		t.Fatal("persisted")
	}
	if ch, _ := s.SetFlagged(id, false); !ch {
		t.Fatal("unflag")
	}
}

func TestRemoveMessageAdjustsCountsDropsFTSAndCascades(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	s.SetFolderCounts(fid, 2, 1)
	seen := insertSearchable(t, s, fid, 1, "alpha", "f", "a@b.com", "")
	m := msg(fid, 2)
	m.Subject, m.FromAddr, m.Seen = "beta unique", "c@d.com", false
	unseen := upsert(t, s, m)
	s.ReplaceAttachments(seen, []store.AttachmentMeta{{PartIndex: 2, Filename: "a.pdf", MimeType: "application/pdf", SizeBytes: 10}})

	if err := s.RemoveMessage(seen); err != nil {
		t.Fatal(err)
	}
	f, _ := s.GetFolder(fid)
	if f.TotalCount != 1 || f.UnreadCount != 1 {
		t.Fatalf("%+v", f)
	}
	if hits, _ := s.SearchMessages("alpha", nil, 0, 50); len(hits) != 0 {
		t.Fatal("fts row not gone")
	}
	if l, _ := s.ListAttachments(seen); len(l) != 0 {
		t.Fatal("cascade")
	}
	s.RemoveMessage(unseen)
	f, _ = s.GetFolder(fid)
	if f.TotalCount != 0 || f.UnreadCount != 0 {
		t.Fatalf("%+v", f)
	}
	if err := s.RemoveMessage(99999); err != nil {
		t.Fatal(err)
	}
	f, _ = s.GetFolder(fid)
	if f.TotalCount != 0 || f.UnreadCount != 0 {
		t.Fatalf("floor: %+v", f)
	}
}

func TestMarkSeenForMessageIDSiblingsUpdatesOtherFoldersOnly(t *testing.T) {
	s := open(t)
	inbox := folder(t, s, "acct", "INBOX", store.RoleInbox)
	archive := folder(t, s, "acct", "All Mail", store.RoleArchive)
	other := folder(t, s, "other-acct", "INBOX", store.RoleInbox)
	for _, f := range []int64{inbox, archive, other} {
		s.SetFolderCounts(f, 5, 2)
	}
	shared := "<gmail-shared@example.com>"
	make := func(folderID int64, uid uint32) int64 {
		m := msg(folderID, uid)
		m.MessageID, m.Seen = models.Str(shared), false
		return upsert(t, s, m)
	}
	inboxRow := make(inbox, 1)
	archiveRow := make(archive, 1)
	otherRow := make(other, 1)
	um := msg(archive, 2)
	um.MessageID, um.Seen = models.Str("<unrelated@example.com>"), false
	unrelatedRow := upsert(t, s, um)

	if ch, _ := s.MarkSeen(inboxRow, true); !ch {
		t.Fatal("mark inbox copy")
	}
	changed, err := s.MarkSeenForMessageIDSiblings("acct", shared, inboxRow, true)
	if err != nil || len(changed) != 1 || changed[0] != archive {
		t.Fatalf("%v %v", changed, err)
	}
	readSeen := func(id int64) bool {
		for _, f := range []int64{inbox, archive, other} {
			rows, _ := s.PageMessages(f, 0, 10)
			for _, r := range rows {
				if r.ID == id {
					return r.Seen
				}
			}
		}
		t.Fatalf("row %d not found", id)
		return false
	}
	if !readSeen(archiveRow) || readSeen(otherRow) || readSeen(unrelatedRow) {
		t.Fatal("sibling propagation wrong")
	}
	af, _ := s.GetFolder(archive)
	of, _ := s.GetFolder(other)
	if af.UnreadCount != 1 || of.UnreadCount != 2 {
		t.Fatalf("%+v %+v", af, of)
	}
	if again, _ := s.MarkSeenForMessageIDSiblings("acct", shared, inboxRow, true); len(again) != 0 {
		t.Fatal("second call should be a no-op")
	}
}

func readSnippet(t *testing.T, s *store.Store, folderID, id int64) string {
	t.Helper()
	rows, err := s.PageMessages(folderID, 0, 100)
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range rows {
		if r.ID == id {
			return r.Snippet
		}
	}
	t.Fatalf("row %d missing", id)
	return ""
}

func TestHealCachedSnippetsRecomputesStaleRowsAndSkipsCurrentOnes(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	entityText := "A Message From Our Team &zwnj; &zwnj; &zwnj; Real update here"
	stale := insertSearchable(t, s, fid, 1, "Wilson", "Team", "team@example.com", "")
	s.SetBody(stale, models.Str(entityText), nil, nil, nil, models.Str("staletoken A Message From Our Team &zwnj; &zwnj; Real update here"))
	freshText := "Hi team, quick update on the roadmap for next week."
	fresh := insertSearchable(t, s, fid, 2, "Roadmap", "PM", "pm@example.com", "")
	s.SetBody(fresh, models.Str(freshText), nil, nil, nil, mailparse.SnippetForBody(models.Str(freshText), nil))
	uncached := insertSearchable(t, s, fid, 3, "Envelope", "S", "s@example.com", "envelope snippet &zwnj; stays")

	changed, err := s.HealCachedSnippets(mailparse.SnippetForBody)
	if err != nil || changed != 1 {
		t.Fatalf("%d %v", changed, err)
	}
	if got := readSnippet(t, s, fid, stale); got != "A Message From Our Team Real update here" {
		t.Fatal(got)
	}
	if got := readSnippet(t, s, fid, fresh); got != freshText {
		t.Fatal(got)
	}
	if got := readSnippet(t, s, fid, uncached); got != "envelope snippet &zwnj; stays" {
		t.Fatal(got)
	}
	if hits, _ := s.SearchMessages("staletoken", nil, 0, 50); len(hits) != 0 {
		t.Fatal("stale snippet still indexed")
	}
	if again, _ := s.HealCachedSnippets(mailparse.SnippetForBody); again != 0 {
		t.Fatal("second run should be a no-op")
	}
}

func TestMessageIDHeader(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	m := msg(fid, 1)
	m.MessageID = models.Str("<a@b.com>")
	with := upsert(t, s, m)
	without := upsert(t, s, msg(fid, 2))
	if v, _ := s.MessageIDHeader(with); v == nil || *v != "<a@b.com>" {
		t.Fatal("with")
	}
	if v, _ := s.MessageIDHeader(without); v != nil {
		t.Fatal("without")
	}
	if v, _ := s.MessageIDHeader(999999); v != nil {
		t.Fatal("missing")
	}
}

func TestIncrementFolderCountsTracksUnseen(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "Archive", store.RoleArchive)
	s.SetFolderCounts(fid, 5, 0)
	s.IncrementFolderCounts(fid, false)
	f, _ := s.GetFolder(fid)
	if f.TotalCount != 6 || f.UnreadCount != 0 {
		t.Fatalf("%+v", f)
	}
	s.IncrementFolderCounts(fid, true)
	f, _ = s.GetFolder(fid)
	if f.TotalCount != 7 || f.UnreadCount != 1 {
		t.Fatalf("%+v", f)
	}
}

func TestFindFolderByRoleAndActionContext(t *testing.T) {
	s := open(t)
	inbox := folder(t, s, "acct", "INBOX", store.RoleInbox)
	trash := folder(t, s, "acct", "Trash", store.RoleTrash)
	folder(t, s, "other", "All Mail", store.RoleArchive)
	if f, _ := s.FindFolderByRole("acct", store.RoleTrash); f == nil || f.ID != trash {
		t.Fatal("trash")
	}
	if f, _ := s.FindFolderByRole("acct", store.RoleArchive); f != nil {
		t.Fatal("archive")
	}
	m := msg(inbox, 7)
	m.Seen = false
	id := upsert(t, s, m)
	ctx, _ := s.MessageActionContext(id)
	want := store.MessageActionContext{FolderID: inbox, UID: 7, AccountID: "acct", FolderName: "INBOX", FolderRole: "inbox", Seen: false}
	if ctx == nil || *ctx != want {
		t.Fatalf("%+v", ctx)
	}
	if ctx, _ := s.MessageActionContext(12345); ctx != nil {
		t.Fatal("missing")
	}
	loc, _ := s.LocateMessage(id)
	if loc == nil || loc.FolderID != inbox || loc.UID != 7 || loc.AccountID != "acct" || loc.FolderName != "INBOX" {
		t.Fatalf("%+v", loc)
	}
}

func TestFTSQueryBuilder(t *testing.T) {
	if store.BuildFTSMatch("   ") != "" || store.BuildFTSMatch("") != "" {
		t.Fatal("empty")
	}
	if got := store.BuildFTSMatch("hello"); got != `"hello"*` {
		t.Fatal(got)
	}
	if got := store.BuildFTSMatch("hello world"); got != `"hello"* "world"*` {
		t.Fatal(got)
	}
	if got := store.BuildFTSMatch(`a"b`); got != `"a""b"*` {
		t.Fatal(got)
	}
}

func TestAdversarialQueriesNeverRaise(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	insertSearchable(t, s, fid, 1, "Quarterly report", "Finance", "finance@example.com", "numbers inside")
	for _, q := range []string{"AND", "OR", "NEAR", "report AND finance", "foo-bar", "(unbalanced", `a"b`, "*", "col:val", "^caret", "報告"} {
		if _, err := s.SearchMessages(q, nil, 0, 50); err != nil {
			t.Fatalf("query %q: %v", q, err)
		}
	}
}

func TestSearchMatchesEachIndexedColumnAndPrefixes(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := insertSearchable(t, s, fid, 1, "Invoice attached", "Alice Merchant", "billing@shop.example", "your receipt is ready")
	for _, q := range []string{"invoice", "merchant", "shop.example", "receipt", "invo"} {
		hits, err := s.SearchMessages(q, nil, 0, 50)
		if err != nil || len(hits) != 1 || hits[0].Msg.ID != id {
			t.Fatalf("query %q: %+v %v", q, hits, err)
		}
	}
	if hits, _ := s.SearchMessages("zzznomatch", nil, 0, 50); len(hits) != 0 {
		t.Fatal("no match")
	}
}

func TestSearchIndexesBodyTextAfterSetBody(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := insertSearchable(t, s, fid, 1, "Meeting", "Bob", "bob@example.com", "")
	if hits, _ := s.SearchMessages("asparagus", nil, 0, 50); len(hits) != 0 {
		t.Fatal("before body")
	}
	s.SetBody(id, models.Str("the asparagus harvest was excellent"), nil, nil, nil, models.Str("the asparagus harvest"))
	hits, _ := s.SearchMessages("asparagus", nil, 0, 50)
	if len(hits) != 1 || hits[0].Msg.ID != id {
		t.Fatalf("%+v", hits)
	}
}

func TestEnvelopeResyncWithEmptySnippetKeepsBodyDerivedSnippet(t *testing.T) {
	s := open(t)
	fid := folder(t, s, "acct", "INBOX", store.RoleInbox)
	id := insertSearchable(t, s, fid, 7, "Sale", "Shop", "shop@example.com", "")
	if got := readSnippet(t, s, fid, id); got != "" {
		t.Fatal(got)
	}
	s.SetBody(id, models.Str("Big Sale! View in browser"), nil, nil, nil, models.Str("Big Sale! View in browser"))
	if got := readSnippet(t, s, fid, id); got != "Big Sale! View in browser" {
		t.Fatal(got)
	}
	if again := insertSearchable(t, s, fid, 7, "Sale", "Shop", "shop@example.com", ""); again != id {
		t.Fatal("upsert should hit the same row")
	}
	if got := readSnippet(t, s, fid, id); got != "Big Sale! View in browser" {
		t.Fatal(got)
	}
	insertSearchable(t, s, fid, 7, "Sale", "Shop", "shop@example.com", "fresher snippet")
	if got := readSnippet(t, s, fid, id); got != "fresher snippet" {
		t.Fatal(got)
	}
}

func TestSearchScopesToAccountAndDropsDeletedRows(t *testing.T) {
	s := open(t)
	fa := folder(t, s, "acct-a", "INBOX", store.RoleInbox)
	fb := folder(t, s, "acct-b", "INBOX", store.RoleInbox)
	insertSearchable(t, s, fa, 1, "shared keyword", "A", "a@a.example", "")
	insertSearchable(t, s, fb, 1, "shared keyword", "B", "b@b.example", "")
	if hits, _ := s.SearchMessages("shared", nil, 0, 50); len(hits) != 2 {
		t.Fatal("unscoped")
	}
	scoped, _ := s.SearchMessages("shared", models.Str("acct-a"), 0, 50)
	if len(scoped) != 1 || scoped[0].AccountID != "acct-a" {
		t.Fatalf("%+v", scoped)
	}
	s.DeleteAccountData("acct-a")
	after, _ := s.SearchMessages("shared", nil, 0, 50)
	if len(after) != 1 || after[0].AccountID != "acct-b" {
		t.Fatalf("%+v", after)
	}
}

func TestUnifiedPagingFolderOrderAndUIDValidityWipe(t *testing.T) {
	s := open(t)
	inbox := folder(t, s, "acct", "INBOX", store.RoleInbox)
	sent := folder(t, s, "acct", "Sent", store.RoleSent)
	upsert(t, s, msg(inbox, 1))
	upsert(t, s, msg(sent, 1))
	if rows, _ := s.PageUnifiedMessages(0, 10); len(rows) != 1 || rows[0].AccountID != "acct" {
		t.Fatalf("unified: %+v", rows)
	}
	folders, _ := s.ListFolders("acct")
	if len(folders) != 2 || folders[0].Role != "inbox" || folders[1].Role != "sent" {
		t.Fatalf("%+v", folders)
	}
	if maxUID, _ := s.MaxUID(inbox); maxUID != 1 {
		t.Fatal("max uid")
	}
	id, wiped, err := s.UpsertFolder("acct", "INBOX", store.RoleInbox, 2)
	if err != nil || !wiped || id != inbox {
		t.Fatalf("%d %v %v", id, wiped, err)
	}
	if maxUID, _ := s.MaxUID(inbox); maxUID != 0 {
		t.Fatal("wipe")
	}
	s.SetLastSeenUID(inbox, 10)
	s.SetLastSeenUID(inbox, 5)
	if _, last, _ := s.FolderSyncState(inbox); last != 10 {
		t.Fatalf("last seen %d", last)
	}
}

func TestDSNEscapesURIDelimitersInThePath(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "odd?dir#1 two")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "mail.db")
	if dsn := store.DSN(path); strings.Contains(dsn, "?dir") || !strings.Contains(dsn, "odd%3Fdir%231%20two/mail.db?_pragma=") {
		t.Fatal(dsn)
	}
	s, err := store.OpenPath(path)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if _, _, err := s.UpsertFolder("acct", "INBOX", store.RoleInbox, 1); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("database not created at the configured path: %v", err)
	}
}

func TestRFC3339MatchesChronoStyle(t *testing.T) {
	if got := store.RFC3339(mustTime()); got != "2026-07-10T00:00:00+00:00" {
		t.Fatal(got)
	}
}
