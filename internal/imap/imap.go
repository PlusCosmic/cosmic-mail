// Package imap is the IMAP connector and per-session operations built on
// go-imap v2 over crypto/tls.
//
// Supports two auth mechanisms: LOGIN with a keyring password (plain IMAP
// accounts) and SASL AUTHENTICATE XOAUTH2 (Gmail).
package imap

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	goimap "github.com/emersion/go-imap/v2"
	"github.com/emersion/go-imap/v2/imapclient"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/buildinfo"
	"cosmicmail/internal/mailparse"
	"cosmicmail/internal/models"
	"cosmicmail/internal/oauth"
	"cosmicmail/internal/store"
)

// TCP keepalive settings for the IMAP socket (issue #41).
//
// Without these, a socket that dies silently — laptop suspend/resume, wifi
// roam, VPN flap, NAT idle-timeout, a server that drops the connection
// without a FIN/RST — leaves the app sitting in a dead IDLE receiving
// nothing, byte-for-byte indistinguishable from "no mail has arrived" until
// the (much longer) full-resync deadline in package sync fires. With
// keepalive probes the kernel detects the dead peer and the next socket
// read/write fails in roughly idle + interval*count (~90s), which propagates
// up through go-imap into the ordinary reconnect-with-backoff path.
const (
	tcpKeepaliveIdle     = 60 * time.Second
	tcpKeepaliveInterval = 10 * time.Second
	tcpKeepaliveRetries  = 3
	dialTimeout          = 30 * time.Second
)

// PollInterval is the sleep used when a server does not support IDLE.
const PollInterval = 60 * time.Second

// Session is an authenticated IMAP session over TLS.
type Session struct {
	c *imapclient.Client
	// changed is set by the unilateral-data handler whenever the server
	// announces new mail (`* N EXISTS`) outside a command that reads it; the
	// sync loop's drain-check reads and clears it (see package sync).
	changed atomic.Bool
	// wake is signalled (non-blocking) on the same announcement so an IDLE
	// wait can end immediately.
	wake     chan struct{}
	capsOnce sync.Once
	caps     goimap.CapSet
}

// TLSConfig builds the client TLS config: the OS root store, plus — in
// development builds only — any certificates in the PEM named by
// COSMIC_MAIL_EXTRA_CA, so E2E tests can point the app at a local IMAPS
// fixture (GreenMail) serving a self-signed `localhost` cert. Compiled out
// of production builds; the strict path is unchanged there.
func TLSConfig() *tls.Config {
	cfg := &tls.Config{MinVersion: tls.VersionTLS12}
	if buildinfo.Debug {
		addExtraCA(cfg)
	}
	return cfg
}

func addExtraCA(cfg *tls.Config) {
	path := os.Getenv("COSMIC_MAIL_EXTRA_CA")
	if path == "" {
		return
	}
	pem, err := os.ReadFile(path)
	if err != nil {
		slog.Warn("COSMIC_MAIL_EXTRA_CA unreadable", "error", err)
		return
	}
	roots, err := x509.SystemCertPool()
	if err != nil || roots == nil {
		roots = x509.NewCertPool()
	}
	if !roots.AppendCertsFromPEM(pem) {
		slog.Warn("COSMIC_MAIL_EXTRA_CA contained no usable certificates")
		return
	}
	cfg.RootCAs = roots
	slog.Warn("trusting extra CA certificates (development build)", "path", path)
}

// Connect connects and authenticates an IMAP session for the account. For
// imap accounts the password comes from the keyring; for gmail accounts an
// OAuth access token is obtained (and refreshed) via package oauth.
func Connect(ctx context.Context, account accounts.Account) (*Session, error) {
	s := &Session{wake: make(chan struct{}, 1)}
	handler := &imapclient.UnilateralDataHandler{
		Mailbox: func(data *imapclient.UnilateralDataMailbox) {
			if data.NumMessages != nil {
				s.changed.Store(true)
				select {
				case s.wake <- struct{}{}:
				default:
				}
			}
		},
	}
	dialer := &net.Dialer{
		Timeout: dialTimeout,
		// Set on the socket before the TLS handshake so it covers the whole
		// connection lifetime, including the handshake itself.
		KeepAliveConfig: net.KeepAliveConfig{Enable: true, Idle: tcpKeepaliveIdle, Interval: tcpKeepaliveInterval, Count: tcpKeepaliveRetries},
	}
	opts := &imapclient.Options{
		TLSConfig:             TLSConfig(),
		Dialer:                dialer,
		WordDecoder:           mailparse.WordDecoder,
		UnilateralDataHandler: handler,
	}
	addr := fmt.Sprintf("%s:%d", account.ImapHost, account.ImapPort)
	c, err := imapclient.DialTLS(addr, opts)
	if err != nil {
		return nil, fmt.Errorf("connecting to %s: %w", addr, err)
	}
	s.c = c
	if err := c.WaitGreeting(); err != nil {
		c.Close()
		return nil, fmt.Errorf("reading IMAP greeting: %w", err)
	}
	switch account.Kind {
	case models.AccountKindImap:
		password, err := accounts.GetImapPassword(account.ID)
		if err != nil {
			c.Close()
			return nil, fmt.Errorf("no stored IMAP password: %w", err)
		}
		if err := c.Login(account.Username, password).Wait(); err != nil {
			c.Close()
			return nil, fmt.Errorf("IMAP LOGIN failed: %w", err)
		}
	case models.AccountKindGmail:
		token, err := oauth.AccessToken(ctx, account.ID)
		if err != nil {
			c.Close()
			return nil, fmt.Errorf("obtaining Gmail access token: %w", err)
		}
		if err := c.Authenticate(&oauth.XOAuth2Client{User: account.Username, Token: token}); err != nil {
			c.Close()
			return nil, fmt.Errorf("IMAP XOAUTH2 auth failed: %w", err)
		}
	default:
		c.Close()
		return nil, fmt.Errorf("unsupported account kind %q", account.Kind)
	}
	return s, nil
}

// Logout ends the session, best effort.
func (s *Session) Logout() {
	if s == nil || s.c == nil {
		return
	}
	_ = s.c.Logout().Wait()
	_ = s.c.Close()
}

// Closed is closed when the connection is gone.
func (s *Session) Closed() <-chan struct{} { return s.c.Closed() }

// DrainMailboxChanged reports whether the server announced a mailbox change
// (an untagged EXISTS) since the last drain, and clears the flag. It is a
// local, non-blocking read — nothing can land between it and a following
// Idle call that it would not already have observed, which is what closes
// the race described in package sync (issue #39).
func (s *Session) DrainMailboxChanged() bool {
	changed := s.changed.Swap(false)
	select {
	case <-s.wake:
	default:
	}
	return changed
}

// RemoteFolder is a folder discovered on the server.
type RemoteFolder struct {
	Name string
	Role store.FolderRole
}

// ListFoldersWithRoles lists all selectable folders and assigns roles from
// SPECIAL-USE attributes, falling back to name heuristics.
func (s *Session) ListFoldersWithRoles() ([]RemoteFolder, error) {
	opts := &goimap.ListOptions{}
	if s.Caps().Has(goimap.CapSpecialUse) {
		opts.ReturnSpecialUse = true
	}
	list, err := s.c.List("", "*", opts).Collect()
	if err != nil {
		return nil, fmt.Errorf("collecting LIST results: %w", err)
	}
	folders := []RemoteFolder{}
	for _, item := range list {
		noSelect := false
		for _, attr := range item.Attrs {
			if strings.EqualFold(string(attr), string(goimap.MailboxAttrNoSelect)) {
				noSelect = true
			}
		}
		if noSelect {
			continue
		}
		role, ok := roleFromAttributes(item.Attrs)
		if !ok {
			role = RoleFromName(item.Mailbox)
		}
		folders = append(folders, RemoteFolder{Name: item.Mailbox, Role: role})
	}
	return folders, nil
}

func roleFromAttributes(attrs []goimap.MailboxAttr) (store.FolderRole, bool) {
	for _, attr := range attrs {
		switch strings.ToLower(string(attr)) {
		case `\all`, `\archive`:
			return store.RoleArchive, true
		case `\drafts`:
			return store.RoleDrafts, true
		case `\junk`:
			return store.RoleSpam, true
		case `\sent`:
			return store.RoleSent, true
		case `\trash`:
			return store.RoleTrash, true
		}
	}
	return "", false
}

// RoleFromName guesses a folder role from its name.
func RoleFromName(name string) store.FolderRole {
	lower := strings.ToLower(name)
	leaf := lower
	if i := strings.LastIndex(lower, "/"); i >= 0 {
		leaf = lower[i+1:]
	}
	switch {
	case leaf == "inbox":
		return store.RoleInbox
	case strings.Contains(leaf, "sent"):
		return store.RoleSent
	case strings.Contains(leaf, "draft"):
		return store.RoleDrafts
	case strings.Contains(leaf, "trash"), strings.Contains(leaf, "deleted"):
		return store.RoleTrash
	case strings.Contains(leaf, "spam"), strings.Contains(leaf, "junk"):
		return store.RoleSpam
	case strings.Contains(leaf, "archive"), strings.Contains(leaf, "all mail"):
		return store.RoleArchive
	}
	return store.RoleNormal
}

// FolderStatus holds authoritative mailbox counters from IMAP STATUS.
type FolderStatus struct {
	UIDValidity uint32
	Exists      uint32
	Unseen      uint32
}

// Status reads mailbox counters without selecting it.
func (s *Session) Status(mailbox string) (FolderStatus, error) {
	data, err := s.c.Status(mailbox, &goimap.StatusOptions{NumMessages: true, NumUnseen: true, UIDNext: true, UIDValidity: true}).Wait()
	if err != nil {
		return FolderStatus{}, fmt.Errorf("reading status for %s: %w", mailbox, err)
	}
	st := FolderStatus{UIDValidity: data.UIDValidity}
	if data.NumMessages != nil {
		st.Exists = *data.NumMessages
	}
	if data.NumUnseen != nil {
		st.Unseen = *data.NumUnseen
	}
	return st, nil
}

// Select selects (read-write) a mailbox.
func (s *Session) Select(mailbox string) error {
	if _, err := s.c.Select(mailbox, nil).Wait(); err != nil {
		return fmt.Errorf("selecting %s: %w", mailbox, err)
	}
	return nil
}

// EnvelopeSummary is a decoded envelope summary for one message.
type EnvelopeSummary struct {
	UID            uint32
	MessageID      *string
	Subject        string
	FromName       string
	FromAddr       string
	ToAddrs        []string
	CcAddrs        []string
	Date           string
	Seen           bool
	Flagged        bool
	HasAttachments bool
	RFC822Size     uint32
}

var envelopeItems = &goimap.FetchOptions{UID: true, Flags: true, InternalDate: true, Envelope: true, BodyStructure: &goimap.FetchItemBodyStructure{Extended: true}, RFC822Size: true}

// FetchEnvelopesFrom fetches envelope summaries for UIDs `from:*`.
func (s *Session) FetchEnvelopesFrom(from uint32) ([]EnvelopeSummary, error) {
	set := goimap.UIDSet{goimap.UIDRange{Start: goimap.UID(from), Stop: 0}}
	msgs, err := s.c.Fetch(set, envelopeItems).Collect()
	if err != nil {
		return nil, fmt.Errorf("issuing UID FETCH for envelopes: %w", err)
	}
	return envelopes(msgs), nil
}

// FetchRecentEnvelopes fetches at most the newest limit existing messages
// using sequence numbers, which are dense within the selected mailbox
// (unlike UIDs), so the server returns exactly the desired tail even when
// the mailbox has UID gaps.
func (s *Session) FetchRecentEnvelopes(exists, limit uint32) ([]EnvelopeSummary, error) {
	start, ok := RecentSequenceStart(exists, limit)
	if !ok {
		return []EnvelopeSummary{}, nil
	}
	set := goimap.SeqSet{goimap.SeqRange{Start: start, Stop: 0}}
	msgs, err := s.c.Fetch(set, envelopeItems).Collect()
	if err != nil {
		return nil, fmt.Errorf("issuing FETCH for recent envelopes: %w", err)
	}
	return envelopes(msgs), nil
}

// RecentSequenceStart is the first sequence number of the `start:*` range
// covering the newest limit of exists messages; false when there is nothing
// to fetch.
func RecentSequenceStart(exists, limit uint32) (uint32, bool) {
	if exists == 0 || limit == 0 {
		return 0, false
	}
	if exists <= limit {
		return 1, true
	}
	return exists - limit + 1, true
}

func envelopes(msgs []*imapclient.FetchMessageBuffer) []EnvelopeSummary {
	out := []EnvelopeSummary{}
	for _, m := range msgs {
		if e, ok := envelopeFromFetch(m); ok {
			out = append(out, e)
		}
	}
	return out
}

func envelopeFromFetch(m *imapclient.FetchMessageBuffer) (EnvelopeSummary, bool) {
	if m.UID == 0 || m.Envelope == nil {
		return EnvelopeSummary{}, false
	}
	env := m.Envelope
	e := EnvelopeSummary{UID: uint32(m.UID), Subject: strings.TrimSpace(env.Subject), RFC822Size: uint32(m.RFC822Size)}
	if len(env.From) > 0 {
		e.FromName, e.FromAddr = addressParts(env.From[0])
	}
	e.ToAddrs = addressEmails(env.To)
	e.CcAddrs = addressEmails(env.Cc)
	if mid := strings.TrimSpace(env.MessageID); mid != "" {
		e.MessageID = &mid
	}
	// Prefer INTERNALDATE for stable ordering; fall back to the envelope date.
	switch {
	case !m.InternalDate.IsZero():
		e.Date = store.RFC3339(m.InternalDate)
	case !env.Date.IsZero():
		e.Date = store.RFC3339(env.Date)
	default:
		e.Date = store.RFC3339(time.Now().UTC())
	}
	for _, f := range m.Flags {
		switch strings.ToLower(string(f)) {
		case `\seen`:
			e.Seen = true
		case `\flagged`:
			e.Flagged = true
		}
	}
	if m.BodyStructure != nil {
		e.HasAttachments = BodyStructureHasRealAttachment(m.BodyStructure)
	}
	return e, true
}

// addressParts splits an IMAP address into (display-name, email), with the
// email standing in for a missing name.
func addressParts(a goimap.Address) (string, string) {
	name := mailparse.UnquoteDisplayName(strings.TrimSpace(a.Name))
	email := addressEmail(a)
	if name == "" {
		name = email
	}
	return name, email
}

func addressEmail(a goimap.Address) string {
	switch {
	case a.Mailbox != "" && a.Host != "":
		return a.Mailbox + "@" + a.Host
	case a.Mailbox != "":
		return a.Mailbox
	}
	return ""
}

func addressEmails(list []goimap.Address) []string {
	out := []string{}
	for _, a := range list {
		if e := addressEmail(a); e != "" {
			out = append(out, e)
		}
	}
	return out
}

// BodyStructureHasRealAttachment is the attachment heuristic used for
// envelope-only rows before a body has been fetched and parsed for real
// (store.ReplaceAttachments corrects `has_attachments` once the body is
// cached). A part counts as a plausible real attachment when it carries
// Content-Disposition: attachment, or — absent an explicit inline
// disposition and a Content-ID — a filename/name param (some senders omit
// Content-Disposition entirely on real attachments).
//
// Deliberately excludes: multipart/related inline images (a Content-ID with
// no explicit attachment disposition, regardless of any filename param), the
// text/plain and text/html body parts themselves, and PGP/S-MIME signature
// parts, which ride along on every signed message.
func BodyStructureHasRealAttachment(bs goimap.BodyStructure) bool {
	found := false
	bs.Walk(func(_ []int, part goimap.BodyStructure) bool {
		if found {
			return false
		}
		single, ok := part.(*goimap.BodyStructureSinglePart)
		if !ok {
			return true
		}
		if isRealAttachment(single) {
			found = true
			return false
		}
		return true
	})
	return found
}

func isSignatureMime(typ, subtype string) bool {
	if !strings.EqualFold(typ, "application") {
		return false
	}
	return strings.EqualFold(subtype, "pgp-signature") || strings.EqualFold(subtype, "pkcs7-signature") || strings.EqualFold(subtype, "x-pkcs7-signature")
}

func hasFilenameParam(params map[string]string) bool {
	for k := range params {
		if strings.EqualFold(k, "filename") || strings.EqualFold(k, "name") {
			return true
		}
	}
	return false
}

func isRealAttachment(p *goimap.BodyStructureSinglePart) bool {
	if isSignatureMime(p.Type, p.Subtype) {
		return false
	}
	disp := p.Disposition()
	if disp != nil {
		switch strings.ToLower(disp.Value) {
		case "attachment":
			return true
		case "inline":
			return false
		}
	}
	if p.ID != "" {
		// A Content-ID with no explicit "attachment" disposition is an inline
		// cid:-referenced part (multipart/related image), not a listed
		// attachment, even if it also carries a filename param.
		return false
	}
	if hasFilenameParam(p.Params) {
		return true
	}
	return disp != nil && hasFilenameParam(disp.Params)
}

var bodyPeek = &goimap.FetchOptions{UID: true, BodySection: []*goimap.FetchItemBodySection{{Peek: true}}}

// fetchRawBody fetches the raw RFC822 bytes for a single UID with
// BODY.PEEK[] (never setting \Seen).
func (s *Session) fetchRawBody(uid uint32) ([]byte, error) {
	msgs, err := s.c.Fetch(goimap.UIDSetNum(goimap.UID(uid)), bodyPeek).Collect()
	if err != nil {
		return nil, fmt.Errorf("issuing UID FETCH for body: %w", err)
	}
	for _, m := range msgs {
		for _, section := range m.BodySection {
			if section.Bytes != nil {
				return section.Bytes, nil
			}
		}
	}
	return nil, errors.New("server returned no body for message")
}

// FetchBody fetches and parses the full body for a single UID.
func (s *Session) FetchBody(uid uint32) (mailparse.FetchedBody, error) {
	raw, err := s.fetchRawBody(uid)
	if err != nil {
		return mailparse.FetchedBody{}, err
	}
	return mailparse.ParseBody(raw), nil
}

// FetchAttachmentBytes fetches the full message (BODY.PEEK[], non-marking)
// and returns one part's decoded bytes by its stable parse index.
func (s *Session) FetchAttachmentBytes(uid uint32, partIndex uint32) ([]byte, error) {
	raw, err := s.fetchRawBody(uid)
	if err != nil {
		return nil, err
	}
	return mailparse.AttachmentBytes(raw, partIndex)
}

// FetchMessageSize reads the server-reported full-message size without
// fetching body content.
func (s *Session) FetchMessageSize(uid uint32) (uint32, error) {
	msgs, err := s.c.Fetch(goimap.UIDSetNum(goimap.UID(uid)), &goimap.FetchOptions{UID: true, RFC822Size: true}).Collect()
	if err != nil {
		return 0, fmt.Errorf("issuing UID FETCH for message size: %w", err)
	}
	for _, m := range msgs {
		if m.RFC822Size > 0 {
			return uint32(m.RFC822Size), nil
		}
	}
	return 0, errors.New("server returned no size for message")
}

func (s *Session) storeFlag(uid uint32, flag goimap.Flag, add bool, what string) error {
	op := goimap.StoreFlagsDel
	if add {
		op = goimap.StoreFlagsAdd
	}
	cmd := s.c.Store(goimap.UIDSetNum(goimap.UID(uid)), &goimap.StoreFlags{Op: op, Flags: []goimap.Flag{flag}}, nil)
	if _, err := cmd.Collect(); err != nil {
		return fmt.Errorf("issuing UID STORE for %s: %w", what, err)
	}
	return nil
}

// SetSeenFlag sets or clears \Seen on a message by UID.
func (s *Session) SetSeenFlag(uid uint32, seen bool) error {
	return s.storeFlag(uid, goimap.FlagSeen, seen, `\Seen`)
}

// SetFlaggedFlag sets or clears \Flagged on a message by UID.
func (s *Session) SetFlaggedFlag(uid uint32, flagged bool) error {
	return s.storeFlag(uid, goimap.FlagFlagged, flagged, `\Flagged`)
}

// Caps returns the server capabilities (cached per session).
func (s *Session) Caps() goimap.CapSet {
	s.capsOnce.Do(func() {
		caps := s.c.Caps()
		if caps == nil {
			caps, _ = s.c.Capability().Wait()
		}
		s.caps = caps
	})
	return s.caps
}

// MoveMessage moves a message by UID from the selected mailbox to target.
//
// Prefers UID MOVE (RFC 6851) when the server advertises MOVE; otherwise
// go-imap falls back to UID COPY + \Deleted + expunge, preferring UID EXPUNGE
// (RFC 4315 UIDPLUS) so only the copied message is expunged, and dropping to
// a plain EXPUNGE when UIDPLUS is absent.
func (s *Session) MoveMessage(uid uint32, target string) error {
	if _, err := s.c.Move(goimap.UIDSetNum(goimap.UID(uid)), target).Wait(); err != nil {
		return fmt.Errorf("UID MOVE to %s: %w", target, err)
	}
	return nil
}

// DeletePermanently sets \Deleted then expunges, preferring UID EXPUNGE when
// UIDPLUS is available so only this UID is removed.
func (s *Session) DeletePermanently(uid uint32) error {
	set := goimap.UIDSetNum(goimap.UID(uid))
	if _, err := s.c.Store(set, &goimap.StoreFlags{Op: goimap.StoreFlagsAdd, Silent: true, Flags: []goimap.Flag{goimap.FlagDeleted}}, nil).Collect(); err != nil {
		return fmt.Errorf("issuing UID STORE +FLAGS (\\Deleted): %w", err)
	}
	var err error
	if s.Caps().Has(goimap.CapUIDPlus) {
		_, err = s.c.UIDExpunge(set).Collect()
	} else {
		_, err = s.c.Expunge().Collect()
	}
	if err != nil {
		return fmt.Errorf("issuing EXPUNGE: %w", err)
	}
	return nil
}

// IdleOutcome is the outcome of one IDLE wait on the selected inbox.
type IdleOutcome int

const (
	// IdleNewData: the server announced changes; the session is live for an
	// immediate re-sync on the same connection.
	IdleNewData IdleOutcome = iota
	// IdleTimeout: the wait timed out (the re-issue cadence, the full-resync
	// deadline, or an IDLE-less server's poll-fallback sleep); the session is
	// live either way.
	IdleTimeout
	// IdleSessionGone: the wakeup was routine but the connection died while
	// closing IDLE; the caller should end the cycle and reconnect.
	IdleSessionGone
)

// IdleWait enters IDLE on the selected mailbox and waits for a server
// wakeup or timeout, keeping the session usable afterwards. If the server
// does not support IDLE it falls back to a single poll sleep.
func (s *Session) IdleWait(timeout time.Duration) (IdleOutcome, error) {
	if !s.Caps().Has(goimap.CapIdle) {
		return s.pollFallback(timeout)
	}
	idle, err := s.c.Idle()
	if err != nil {
		slog.Info("IDLE unavailable; falling back to polling", "error", err)
		return s.pollFallback(timeout)
	}
	// An EXISTS piggy-backed on the IDLE command's own response (before the
	// "+ idling" continuation) has already set the flag by now; end IDLE
	// immediately and report it like a real wakeup rather than waiting out
	// the timeout on a mailbox we already know has moved on.
	if s.DrainMailboxChanged() {
		return s.closeIdle(idle, IdleNewData)
	}
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	outcome := IdleTimeout
	select {
	case <-s.wake:
		outcome = IdleNewData
	case <-timer.C:
	case <-s.c.Closed():
		_ = idle.Close()
		return IdleSessionGone, nil
	}
	return s.closeIdle(idle, outcome)
}

// closeIdle sends DONE and recovers the session. The wakeup itself already
// succeeded and every cycle reconnects fresh regardless, so a close failure
// caused by a vanished connection is reported as IdleSessionGone rather
// than as a sync failure; a protocol-level failure stays an error.
func (s *Session) closeIdle(idle *imapclient.IdleCommand, outcome IdleOutcome) (IdleOutcome, error) {
	err := idle.Close()
	if err == nil {
		err = idle.Wait()
	}
	// Hygiene: the flag is decided by the wakeup, not by whatever else landed
	// during the wait or DONE's own response read; clear it so the next
	// drain-check starts clean (the caller re-syncs regardless on NewData).
	if outcome == IdleNewData {
		s.DrainMailboxChanged()
	}
	if err == nil {
		return outcome, nil
	}
	if IsBenignAfterRoutineWakeup(err) {
		slog.Debug("IDLE session close failed after routine wakeup; reconnecting next cycle", "error", err)
		return IdleSessionGone, nil
	}
	return outcome, fmt.Errorf("closing IDLE handle: %w", err)
}

func (s *Session) pollFallback(timeout time.Duration) (IdleOutcome, error) {
	sleep := PollInterval
	if timeout < sleep {
		sleep = timeout
	}
	timer := time.NewTimer(sleep)
	defer timer.Stop()
	select {
	case <-s.wake:
	case <-timer.C:
	case <-s.c.Closed():
		return IdleSessionGone, nil
	}
	_ = s.c.Noop().Wait()
	return IdleTimeout, nil
}

// IsBenignAfterRoutineWakeup reports whether a failure to close IDLE right
// after a routine wakeup should be swallowed: IO errors and a vanished
// connection are the routine downside of holding a socket idle, and a
// server NO is not a protocol problem either. A BAD or unparsable response
// to DONE itself stays fatal.
func IsBenignAfterRoutineWakeup(err error) bool {
	if err == nil {
		return false
	}
	var netErr net.Error
	if errors.As(err, &netErr) {
		return true
	}
	var imapErr *goimap.Error
	if errors.As(err, &imapErr) {
		return imapErr.Type == goimap.StatusResponseTypeNo
	}
	msg := strings.ToLower(err.Error())
	return strings.Contains(msg, "closed") || strings.Contains(msg, "eof") || strings.Contains(msg, "broken pipe") || strings.Contains(msg, "connection reset") || strings.Contains(msg, "use of closed network connection")
}
