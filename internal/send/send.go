// Package send builds plain-text messages and submits them over
// authenticated SMTP.
package send

import (
	"bytes"
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/mail"
	"strings"
	"time"

	"github.com/emersion/go-message"
	gomail "github.com/emersion/go-message/mail"
	"github.com/emersion/go-sasl"
	"github.com/emersion/go-smtp"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/models"
	"cosmicmail/internal/oauth"
)

// TLSConfig is the client TLS configuration used for SMTP; main sets it to
// the same one the IMAP connector uses so the debug extra-CA hook applies.
var TLSConfig = func() *tls.Config { return &tls.Config{MinVersion: tls.VersionTLS12} }

// Built is a fully built message: raw bytes plus the envelope recipients.
type Built struct {
	Raw        []byte
	From       string
	Recipients []string
}

// Send builds and submits a message. Credentials are resolved only after all
// user input validates.
func Send(ctx context.Context, account accounts.Account, input models.SendMessageInput, replyMessageID *string) error {
	built, err := Build(account, input, replyMessageID)
	if err != nil {
		return err
	}
	var auth sasl.Client
	switch account.Kind {
	case models.AccountKindImap:
		password, err := accounts.GetImapPassword(account.ID)
		if err != nil {
			return err
		}
		auth = sasl.NewPlainClient("", account.Username, password)
	case models.AccountKindGmail:
		token, err := oauth.AccessToken(ctx, account.ID)
		if err != nil {
			return fmt.Errorf("obtaining Gmail access token: %w", err)
		}
		auth = &oauth.XOAuth2Client{User: account.Username, Token: token}
	default:
		return fmt.Errorf("unsupported account kind %q", account.Kind)
	}

	addr := fmt.Sprintf("%s:%d", account.SmtpHost, account.SmtpPort)
	tlsCfg := TLSConfig()
	tlsCfg.ServerName = account.SmtpHost
	var client *smtp.Client
	// SMTP is always encrypted: port 465 uses implicit TLS; every other
	// configured submission port uses required STARTTLS.
	if account.SmtpPort == 465 {
		client, err = smtp.DialTLS(addr, tlsCfg)
	} else {
		client, err = smtp.DialStartTLS(addr, tlsCfg)
	}
	if err != nil {
		return fmt.Errorf("configuring SMTP server %s: %w", account.SmtpHost, err)
	}
	defer client.Close()
	if account.Kind == models.AccountKindImap && !client.SupportsAuth("PLAIN") && client.SupportsAuth("LOGIN") {
		password, _ := accounts.GetImapPassword(account.ID)
		auth = sasl.NewLoginClient(account.Username, password)
	}
	if err := client.Auth(auth); err != nil {
		return fmt.Errorf("authenticating with %s: %w", account.SmtpHost, err)
	}
	if err := client.SendMail(built.From, built.Recipients, bytes.NewReader(built.Raw)); err != nil {
		return fmt.Errorf("sending message through %s: %w", account.SmtpHost, err)
	}
	// SMTP acceptance of DATA completes the command: the message is committed
	// on the server. A server that drops the connection or answers QUIT
	// oddly afterwards must not turn into a "send failed" toast, or a retry
	// would deliver a duplicate.
	if err := client.Quit(); err != nil {
		slog.Debug("SMTP QUIT after a successful send failed", "host", account.SmtpHost, "error", err)
	}
	return nil
}

// Build constructs the outgoing message. All address and message headers are
// built here; the frontend never supplies raw headers.
func Build(account accounts.Account, input models.SendMessageInput, replyMessageID *string) (*Built, error) {
	fromAddr, err := mail.ParseAddress(account.Email)
	if err != nil {
		return nil, fmt.Errorf("parsing the sending account address: %w", err)
	}
	from := &gomail.Address{Name: strings.TrimSpace(account.DisplayName), Address: fromAddr.Address}

	var to, cc, bcc []*gomail.Address
	var recipients []string
	for _, raw := range input.ToAddrs {
		a, err := parseRecipient(raw)
		if err != nil {
			return nil, err
		}
		to = append(to, a)
		recipients = append(recipients, a.Address)
	}
	for _, raw := range input.CcAddrs {
		a, err := parseRecipient(raw)
		if err != nil {
			return nil, err
		}
		cc = append(cc, a)
		recipients = append(recipients, a.Address)
	}
	for _, raw := range input.BccAddrs {
		a, err := parseRecipient(raw)
		if err != nil {
			return nil, err
		}
		bcc = append(bcc, a)
		recipients = append(recipients, a.Address)
	}
	if len(recipients) == 0 {
		return nil, errors.New("add at least one recipient")
	}

	var h gomail.Header
	h.SetDate(time.Now())
	h.SetAddressList("From", []*gomail.Address{from})
	if len(to) > 0 {
		h.SetAddressList("To", to)
	}
	if len(cc) > 0 {
		h.SetAddressList("Cc", cc)
	}
	// Bcc recipients go on the envelope only, never in a header.
	_ = bcc
	h.SetSubject(input.Subject)
	if err := h.GenerateMessageIDWithHostname(hostnameFor(fromAddr.Address)); err != nil {
		return nil, fmt.Errorf("generating Message-ID: %w", err)
	}
	if replyMessageID != nil && isSafeMessageID(*replyMessageID) {
		id := strings.TrimSpace(*replyMessageID)
		h.Set("In-Reply-To", id)
		h.Set("References", id)
	}
	h.Set("MIME-Version", "1.0")
	h.SetContentType("text/plain", map[string]string{"charset": "utf-8"})
	h.Set("Content-Transfer-Encoding", "quoted-printable")

	var buf bytes.Buffer
	w, err := message.CreateWriter(&buf, h.Header)
	if err != nil {
		return nil, fmt.Errorf("building outgoing message: %w", err)
	}
	if _, err := io.WriteString(w, input.BodyText); err != nil {
		return nil, fmt.Errorf("building outgoing message: %w", err)
	}
	if err := w.Close(); err != nil {
		return nil, fmt.Errorf("building outgoing message: %w", err)
	}
	return &Built{Raw: buf.Bytes(), From: fromAddr.Address, Recipients: recipients}, nil
}

func hostnameFor(email string) string {
	if _, domain, ok := strings.Cut(email, "@"); ok && domain != "" {
		return domain
	}
	return "localhost"
}

func parseRecipient(raw string) (*gomail.Address, error) {
	value := strings.TrimSpace(raw)
	if value == "" {
		return nil, errors.New("recipient address cannot be empty")
	}
	a, err := mail.ParseAddress(value)
	if err != nil {
		return nil, fmt.Errorf("invalid recipient address: %s", value)
	}
	return &gomail.Address{Name: a.Name, Address: a.Address}, nil
}

func isSafeMessageID(value string) bool {
	trimmed := strings.TrimSpace(value)
	return trimmed != "" && !strings.ContainsAny(trimmed, "\r\n")
}
