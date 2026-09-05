package send

import (
	"strings"
	"testing"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/models"
)

func account() accounts.Account {
	return accounts.Account{ID: "account", Email: "me@example.com", DisplayName: "Cosmic User", Kind: models.AccountKindImap,
		ImapHost: "imap.example.com", ImapPort: 993, SmtpHost: "smtp.example.com", SmtpPort: 587, Username: "me@example.com"}
}

func input() models.SendMessageInput {
	return models.SendMessageInput{AccountID: "account", ToAddrs: []string{"Recipient <to@example.com>"}, CcAddrs: []string{"cc@example.com"},
		BccAddrs: []string{"hidden@example.com"}, Subject: "Hello", BodyText: "Plain body"}
}

func TestBuildsPlainMessageAndHidesBccHeader(t *testing.T) {
	b, err := Build(account(), input(), nil)
	if err != nil {
		t.Fatal(err)
	}
	raw := string(b.Raw)
	for _, want := range []string{"From:", "Cosmic User", "me@example.com", "To:", "Recipient", "to@example.com", "Cc: <cc@example.com>", "Subject: Hello", "Content-Type: text/plain", "Message-Id: <", "Plain body"} {
		if !strings.Contains(raw, want) {
			t.Errorf("missing %q in:\n%s", want, raw)
		}
	}
	if strings.Contains(raw, "Bcc:") {
		t.Fatal("bcc leaked into headers")
	}
	if len(b.Recipients) != 3 || b.From != "me@example.com" {
		t.Fatalf("%+v", b)
	}
}

func TestAddsDirectParentReplyHeaders(t *testing.T) {
	id := "<parent@example.com>"
	b, err := Build(account(), input(), &id)
	if err != nil {
		t.Fatal(err)
	}
	raw := string(b.Raw)
	if !strings.Contains(raw, "In-Reply-To: <parent@example.com>") || !strings.Contains(raw, "References: <parent@example.com>") {
		t.Fatal(raw)
	}
}

func TestRejectsMissingOrInvalidRecipients(t *testing.T) {
	none := input()
	none.ToAddrs, none.CcAddrs, none.BccAddrs = nil, nil, nil
	if _, err := Build(account(), none, nil); err == nil {
		t.Fatal("no recipients")
	}
	invalid := input()
	invalid.ToAddrs = []string{"not an address"}
	if _, err := Build(account(), invalid, nil); err == nil {
		t.Fatal("invalid recipient")
	}
}

func TestOmitsUnsafeReplyHeader(t *testing.T) {
	id := "<parent@example.com>\r\nBcc: attacker@example.com"
	b, err := Build(account(), input(), &id)
	if err != nil {
		t.Fatal(err)
	}
	raw := string(b.Raw)
	if strings.Contains(raw, "In-Reply-To:") || strings.Contains(raw, "attacker@example.com") {
		t.Fatal(raw)
	}
}
