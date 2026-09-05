package autoconfig

import (
	"context"
	"os"
	"testing"

	"cosmicmail/internal/models"
)

const fastmailXML = `<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="fastmail.com">
    <domain>fastmail.com</domain>
    <displayName>Fastmail</displayName>
    <incomingServer type="imap">
      <hostname>imap.fastmail.com</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <authentication>password-cleartext</authentication>
      <username>%EMAILADDRESS%</username>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname>
      <port>465</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>`

const starttlsOnlyXML = `<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.com">
    <domain>example.com</domain>
    <incomingServer type="imap">
      <hostname>imap.example.com</hostname>
      <port>143</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILLOCALPART%</username>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.com</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
    </outgoingServer>
  </emailProvider>
</clientConfig>`

func TestParsesFastmailPrefersStartTLSSMTP(t *testing.T) {
	cfg, ok := parseAutoconfigXML(fastmailXML, "jane@fastmail.com")
	if !ok || cfg.imapHost != "imap.fastmail.com" || cfg.imapPort != 993 || cfg.smtpHost != "smtp.fastmail.com" || cfg.smtpPort != 587 || cfg.username != "jane@fastmail.com" {
		t.Fatalf("%+v %v", cfg, ok)
	}
}

func TestRejectsStartTLSOnlyIMAP(t *testing.T) {
	if _, ok := parseAutoconfigXML(starttlsOnlyXML, "bob@example.com"); ok {
		t.Fatal("should reject")
	}
}

func TestFallsBackToSSLSMTPWhenNoStartTLS(t *testing.T) {
	xml := `<clientConfig version="1.1"><emailProvider id="x">
          <incomingServer type="imap"><hostname>imap.x.com</hostname><port>993</port>
            <socketType>SSL</socketType><username>%EMAILLOCALPART%</username></incomingServer>
          <outgoingServer type="smtp"><hostname>smtp.x.com</hostname><port>465</port>
            <socketType>SSL</socketType><username>%EMAILADDRESS%</username></outgoingServer>
        </emailProvider></clientConfig>`
	cfg, ok := parseAutoconfigXML(xml, "sam@x.com")
	if !ok || cfg.smtpPort != 465 || cfg.username != "sam" {
		t.Fatalf("%+v", cfg)
	}
}

func TestHTMLCatchAllIsNotAConfig(t *testing.T) {
	if _, ok := parseAutoconfigXML("<!doctype html><html><body>Not found</body></html>", "a@b.com"); ok {
		t.Fatal("html page must not parse as a config")
	}
}

func TestPlaceholderResolution(t *testing.T) {
	if resolvePlaceholders("%EMAILADDRESS%", "a@b.com", "a") != "a@b.com" || resolvePlaceholders("%EMAILLOCALPART%", "a@b.com", "a") != "a" || resolvePlaceholders("literal", "a@b.com", "a") != "literal" {
		t.Fatal("placeholders")
	}
}

func TestRegistrableDomainHeuristic(t *testing.T) {
	cases := map[string]string{
		"mx.example.com":         "example.com",
		"example.com":            "example.com",
		"aspmx.l.google.com":     "google.com",
		"mx1.mail.example.co.uk": "example.co.uk",
		"example.co.uk":          "example.co.uk",
		"mail.example.com.au":    "example.com.au",
	}
	for in, want := range cases {
		if got := registrableDomain(in); got != want {
			t.Errorf("%s: %s", in, got)
		}
	}
}

func TestGmailDetection(t *testing.T) {
	s := func(v string) *string { return &v }
	if !isGmail("gmail.com", nil, nil) || !isGmail("googlemail.com", nil, nil) || !isGmail("mydomain.com", s("imap.gmail.com"), nil) ||
		!isGmail("mydomain.com", nil, s("aspmx.l.google.com.")) || !isGmail("mydomain.com", nil, s("alt1.gmail-smtp-in.l.google.com")) {
		t.Fatal("positive")
	}
	if isGmail("mydomain.com", s("imap.mydomain.com"), nil) || isGmail("mydomain.com", nil, s("mx.mydomain.com")) {
		t.Fatal("negative")
	}
}

func TestSplitEmailValidation(t *testing.T) {
	for _, ok := range []string{"a@b.com", "  a@b.com  "} {
		if _, _, err := splitEmail(ok); err != nil {
			t.Errorf("%q: %v", ok, err)
		}
	}
	for _, bad := range []string{"", "noatsign", "@b.com", "a@", "a@b@c"} {
		if _, _, err := splitEmail(bad); err == nil {
			t.Errorf("%q should fail", bad)
		}
	}
}

func TestGuessIsUnconfident(t *testing.T) {
	g := guess("example.com", "me@example.com")
	if g.Source != models.SourceGuess || g.Confident || g.ImapHost != "imap.example.com" || g.SmtpHost != "smtp.example.com" || g.ImapPort != 993 || g.SmtpPort != 587 {
		t.Fatalf("%+v", g)
	}
	gm := guess("gmail.com", "me@gmail.com")
	if gm.Kind != models.AccountKindGmail || gm.ImapHost != "imap.gmail.com" {
		t.Fatalf("%+v", gm)
	}
}

func TestDiscoverRejectsInvalidEmail(t *testing.T) {
	if _, err := Discover(context.Background(), "nope"); err == nil {
		t.Fatal("expected error")
	}
}

// Live network tests, run with COSMIC_MAIL_LIVE_TESTS=1.
func TestISPDBGmxLive(t *testing.T) {
	if os.Getenv("COSMIC_MAIL_LIVE_TESTS") == "" {
		t.Skip("set COSMIC_MAIL_LIVE_TESTS=1 to hit the live Thunderbird ISPDB")
	}
	cfg, ok := fetchISPDB(context.Background(), newClient(), "gmx.de", "jane@gmx.de")
	if !ok || cfg.imapHost != "imap.gmx.net" || cfg.imapPort != 993 {
		t.Fatalf("%+v %v", cfg, ok)
	}
}

func TestMxProviderAutoconfigPurelymailLive(t *testing.T) {
	if os.Getenv("COSMIC_MAIL_LIVE_TESTS") == "" {
		t.Skip("set COSMIC_MAIL_LIVE_TESTS=1 to hit live DNS and autoconfig.purelymail.com")
	}
	cfg, err := Discover(context.Background(), "harry@pluscosmic.dev")
	if err != nil || cfg.ImapHost != "imap.purelymail.com" || cfg.ImapPort != 993 || cfg.SmtpHost != "smtp.purelymail.com" || cfg.Username != "harry@pluscosmic.dev" || cfg.Source != models.SourceMx || !cfg.Confident {
		t.Fatalf("%+v %v", cfg, err)
	}
}

func TestProviderAutoconfigFastmailLive(t *testing.T) {
	if os.Getenv("COSMIC_MAIL_LIVE_TESTS") == "" {
		t.Skip("set COSMIC_MAIL_LIVE_TESTS=1 to hit Fastmail's live autoconfig endpoint")
	}
	cfg, ok := fetchAndParse(context.Background(), newClient(), "https://autoconfig.fastmail.com/mail/config-v1.1.xml?emailaddress=jane@fastmail.com", "jane@fastmail.com")
	if !ok || cfg.imapHost != "imap.fastmail.com" || cfg.imapPort != 993 {
		t.Fatalf("%+v %v", cfg, ok)
	}
}
