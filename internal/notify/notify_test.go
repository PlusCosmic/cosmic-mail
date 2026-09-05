package notify

import (
	"testing"

	mailsync "cosmicmail/internal/sync"
)

func TestBatchCollapsesMoreThanThree(t *testing.T) {
	if Batch("me@example.com", nil) != nil {
		t.Fatal("empty")
	}
	three := []mailsync.NewMail{{FromName: "A", Subject: "a"}, {FromName: "B", Subject: "b"}, {FromName: "C", Subject: "c"}}
	got := Batch("me@example.com", three)
	if len(got) != 3 || got[0][0] != "New mail — A" || got[0][1] != "a" {
		t.Fatalf("%v", got)
	}
	four := append(three, mailsync.NewMail{FromName: "D", Subject: "d"})
	got = Batch("me@example.com", four)
	if len(got) != 1 || got[0][0] != "4 new messages" || got[0][1] != "in me@example.com" {
		t.Fatalf("%v", got)
	}
}
