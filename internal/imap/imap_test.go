package imap

import (
	"errors"
	"io"
	"net"
	"testing"

	goimap "github.com/emersion/go-imap/v2"

	"cosmicmail/internal/store"
)

func TestRecentSequenceRangeIsBoundedToExistingMessages(t *testing.T) {
	if s, ok := RecentSequenceStart(1497, 200); !ok || s != 1298 {
		t.Fatalf("%d %v", s, ok)
	}
	if s, ok := RecentSequenceStart(42, 200); !ok || s != 1 {
		t.Fatalf("%d %v", s, ok)
	}
	if _, ok := RecentSequenceStart(0, 200); ok {
		t.Fatal("exists=0")
	}
	if _, ok := RecentSequenceStart(42, 0); ok {
		t.Fatal("limit=0")
	}
}

func TestBodyFetchNeverSetsSeen(t *testing.T) {
	if len(bodyPeek.BodySection) != 1 || !bodyPeek.BodySection[0].Peek {
		t.Fatal("body fetch must use BODY.PEEK[]")
	}
}

func TestRoleFromName(t *testing.T) {
	cases := map[string]store.FolderRole{
		"INBOX": store.RoleInbox, "Sent Items": store.RoleSent, "Drafts": store.RoleDrafts, "Deleted Messages": store.RoleTrash,
		"Junk": store.RoleSpam, "[Gmail]/All Mail": store.RoleArchive, "Work/Receipts": store.RoleNormal, "Archive": store.RoleArchive,
	}
	for name, want := range cases {
		if got := RoleFromName(name); got != want {
			t.Errorf("%s: %s", name, got)
		}
	}
	if role, ok := roleFromAttributes([]goimap.MailboxAttr{goimap.MailboxAttrHasNoChildren, goimap.MailboxAttrAll}); !ok || role != store.RoleArchive {
		t.Fatal("\\All should map to archive")
	}
	if _, ok := roleFromAttributes([]goimap.MailboxAttr{goimap.MailboxAttrHasChildren}); ok {
		t.Fatal("no special-use")
	}
}

func single(typ, sub string, disp *goimap.BodyStructureDisposition, params map[string]string, id string) *goimap.BodyStructureSinglePart {
	p := &goimap.BodyStructureSinglePart{Type: typ, Subtype: sub, Params: params, ID: id, Encoding: "base64", Size: 100}
	if disp != nil {
		p.Extended = &goimap.BodyStructureSinglePartExt{Disposition: disp}
	}
	return p
}

func text(sub string) *goimap.BodyStructureSinglePart {
	return &goimap.BodyStructureSinglePart{Type: "text", Subtype: sub, Text: &goimap.BodyStructureText{NumLines: 10}}
}

func multi(sub string, children ...goimap.BodyStructure) *goimap.BodyStructureMultiPart {
	return &goimap.BodyStructureMultiPart{Subtype: sub, Children: children}
}

func TestBodyStructureHeuristic(t *testing.T) {
	att := &goimap.BodyStructureDisposition{Value: "attachment", Params: map[string]string{"filename": "report.pdf"}}
	if !BodyStructureHasRealAttachment(multi("mixed", text("plain"), single("application", "pdf", att, nil, ""))) {
		t.Fatal("real listed attachment")
	}
	if BodyStructureHasRealAttachment(multi("related", text("html"), single("image", "png", nil, nil, "img1@example.com"))) {
		t.Fatal("related inline image")
	}
	inline := &goimap.BodyStructureDisposition{Value: "inline", Params: map[string]string{"filename": "image001.png"}}
	if BodyStructureHasRealAttachment(multi("related", text("html"), single("image", "png", inline, nil, "img1@example.com"))) {
		t.Fatal("explicit inline")
	}
	sig := &goimap.BodyStructureDisposition{Value: "attachment", Params: map[string]string{"filename": "signature.asc"}}
	if BodyStructureHasRealAttachment(multi("signed", text("plain"), single("application", "pgp-signature", sig, nil, ""))) {
		t.Fatal("pgp signature")
	}
	if BodyStructureHasRealAttachment(multi("signed", text("plain"), single("application", "pkcs7-signature", nil, nil, ""))) {
		t.Fatal("smime signature")
	}
	if !BodyStructureHasRealAttachment(multi("mixed", text("plain"), single("application", "zip", nil, map[string]string{"name": "archive.zip"}, ""))) {
		t.Fatal("name param without disposition")
	}
	if BodyStructureHasRealAttachment(multi("alternative", text("plain"), text("html"))) {
		t.Fatal("plain bodies")
	}
}

func TestIdleCloseFailureClassification(t *testing.T) {
	if !IsBenignAfterRoutineWakeup(io.EOF) || !IsBenignAfterRoutineWakeup(net.ErrClosed) || !IsBenignAfterRoutineWakeup(&net.OpError{Op: "write", Err: errors.New("broken pipe")}) {
		t.Fatal("io/connection loss should be benign")
	}
	if !IsBenignAfterRoutineWakeup(&goimap.Error{Type: goimap.StatusResponseTypeNo, Text: "server said no"}) {
		t.Fatal("NO should be benign")
	}
	if IsBenignAfterRoutineWakeup(&goimap.Error{Type: goimap.StatusResponseTypeBad, Text: "DONE not understood"}) {
		t.Fatal("BAD must stay fatal")
	}
	if IsBenignAfterRoutineWakeup(errors.New("imapclient: unexpected token")) {
		t.Fatal("parse errors must stay fatal")
	}
	if IsBenignAfterRoutineWakeup(nil) {
		t.Fatal("nil")
	}
}

func TestDrainMailboxChangedIsEdgeTriggered(t *testing.T) {
	s := &Session{wake: make(chan struct{}, 1)}
	if s.DrainMailboxChanged() {
		t.Fatal("fresh session")
	}
	s.changed.Store(true)
	s.wake <- struct{}{}
	if !s.DrainMailboxChanged() {
		t.Fatal("should report the change")
	}
	if s.DrainMailboxChanged() {
		t.Fatal("second drain must be clean")
	}
	select {
	case <-s.wake:
		t.Fatal("wake should have been drained")
	default:
	}
}
