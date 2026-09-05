// Package sync is the sync engine: one background goroutine per account.
//
// Each goroutine connects, lists folders, performs an initial sync (newest
// 200 envelopes per folder), then holds the connection in an INBOX idle
// loop: re-sync INBOX → prefetch → drain-check → IDLE → wakeup → re-sync
// INBOX → … until the 25-minute full-resync deadline, when the cycle ends
// and the loop reconnects for a full folder sweep. Re-syncing on the live
// connection keeps new-mail notification latency at one STATUS+FETCH round
// trip.
//
// A single IDLE command is not held open for the full 25 minutes (issue
// #41): it is capped at the shorter IdleReissueInterval (~5 min), and
// hitting that cap alone loops back for another re-sync + drain-check +
// IDLE on the same connection rather than ending the cycle — only hitting
// the full FullResyncInterval deadline does that. See IdleWaitBudget and
// IdleTimeoutAction for the pure decision logic. This is on top of, and
// independent of, TCP keepalive on the socket itself (package imap).
//
// The drain-check closes a race that a re-sync-before-every-IDLE alone does
// not (issue #39): an IMAP server announces new mail by piggy-backing an
// untagged `* N EXISTS` onto whatever command response happens to be in
// flight, and never repeats it. go-imap hands any such announcement outside
// a SELECT to the unilateral-data handler, which package imap turns into a
// flag. Before entering IDLE we read-and-clear that flag
// (Session.DrainMailboxChanged) and, if anything landed, re-sync again
// instead of idling — bounded by MaxConsecutiveResyncsBeforeIdle. Because
// the drain is a local, non-blocking read with no network I/O between it and
// the IDLE call, no announcement can go unobserved between "we checked" and
// "we committed to IDLE".
//
// Body prefetch (issue #40) sits between the re-sync's notify and the
// drain-check: after the notify so up to five sequential body downloads can
// never delay a new-mail toast, and before the drain-check because that
// check has to be the last thing before IDLE. Errors trigger exponential
// backoff (30s → 5 min). Notifications fire only for new inbox messages
// above the folder's last_seen_uid, and never during the initial sweep.
package sync

import (
	"context"
	"log/slog"
	gosync "sync"
	"time"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/imap"
	"cosmicmail/internal/mailparse"
	"cosmicmail/internal/models"
	"cosmicmail/internal/oauth"
	"cosmicmail/internal/shipments"
	"cosmicmail/internal/store"
)

const (
	InitialSyncLimit       = 200
	BodyPrefetchWorkingSet = 20
	BodyPrefetchLimit      = 5
	BodyPrefetchMaxBytes   = 1024 * 1024
	FullResyncInterval     = 25 * time.Minute
	// IdleReissueInterval is how long a single IDLE command may wait before
	// we DONE and re-issue it on the same connection (issue #41). RFC 2177
	// anticipates clients re-issuing IDLE periodically, and doing so gives a
	// dead/dropped connection a chance to surface long before keepalive would
	// time it out anyway.
	IdleReissueInterval = 5 * time.Minute
	PollInterval        = 60 * time.Second
	BackoffMin          = 30 * time.Second
	BackoffMax          = 5 * time.Minute
	// MaxConsecutiveResyncsBeforeIdle caps back-to-back re-syncs without ever
	// entering IDLE when each drain keeps finding the mailbox changed, so a
	// pathological mailbox cannot spin on STATUS/SELECT/FETCH forever.
	MaxConsecutiveResyncsBeforeIdle = 5
)

// Emitter delivers events to the frontend. main wires it to the Wails event
// manager; tests use a recorder.
type Emitter interface {
	Emit(name string, data any)
}

// NewMail is one new-message notification's content.
type NewMail struct {
	FromName string
	Subject  string
}

// Notifier sends desktop notifications. main wires it to package notify.
type Notifier interface {
	NotifyNewMail(accountEmail string, mail []NewMail)
}

// StopTimeout bounds how long Stop waits for a cancelled sync goroutine to
// finish. Cancellation tears the IMAP connection down and aborts any dial, so
// the loop normally exits within milliseconds; the bound only guards against
// a stuck syscall, and a stop that hits it is logged.
const StopTimeout = 15 * time.Second

// task is one running per-account sync goroutine.
type task struct {
	cancel context.CancelFunc
	done   chan struct{}
}

// Manager owns the per-account sync goroutines so they can be cancelled on
// removal or restarted on demand. opMu serialises Start/Stop/StopAll in
// their entirety — stop, join, and register happen under one hold — so two
// overlapping Start calls for the same account (a UI and a tray "sync now"
// at once) can never leave an untracked loop running.
type Manager struct {
	store    *store.Store
	emitter  Emitter
	notifier Notifier
	opMu     gosync.Mutex
	tasks    map[string]*task
}

// NewManager creates an empty manager.
func NewManager(st *store.Store, emitter Emitter, notifier Notifier) *Manager {
	return &Manager{store: st, emitter: emitter, notifier: notifier, tasks: map[string]*task{}}
}

// Start spawns (or respawns) the sync goroutine for an account. A running
// goroutine for the same account is stopped and joined first, so two loops
// never write the same account's rows at once.
func (m *Manager) Start(account accounts.Account) {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	m.stopLocked(account.ID)
	ctx, cancel := context.WithCancel(context.Background())
	t := &task{cancel: cancel, done: make(chan struct{})}
	m.tasks[account.ID] = t
	go func() {
		defer close(t.done)
		m.accountLoop(ctx, account)
	}()
}

// Stop cancels the sync goroutine for an account and waits for it to exit,
// so a caller that deletes the account's data afterwards (RemoveAccount)
// cannot race a store write the old loop was about to make.
func (m *Manager) Stop(accountID string) {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	m.stopLocked(accountID)
}

// StopAll cancels every sync goroutine and waits for them (application
// shutdown, before the store closes).
func (m *Manager) StopAll() {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	for id := range m.tasks {
		m.stopLocked(id)
	}
}

// Running reports how many sync goroutines are tracked.
func (m *Manager) Running() int {
	m.opMu.Lock()
	defer m.opMu.Unlock()
	return len(m.tasks)
}

func (m *Manager) stopLocked(accountID string) {
	t, ok := m.tasks[accountID]
	if !ok {
		return
	}
	delete(m.tasks, accountID)
	t.cancel()
	select {
	case <-t.done:
	case <-time.After(StopTimeout):
		slog.Warn("sync goroutine did not stop in time", "accountId", accountID)
	}
}

func (m *Manager) emitState(accountID string, state models.SyncState, err error, needsReauth bool) {
	payload := models.SyncStateEvent{AccountID: accountID, State: state, NeedsReauth: needsReauth}
	if err != nil {
		payload.Error = models.Str(err.Error())
	}
	m.emitter.Emit(models.EventSyncState, payload)
}

// accountLoop is the top-level per-account loop with error backoff.
func (m *Manager) accountLoop(ctx context.Context, account accounts.Account) {
	log := slog.With("account", account.Email)
	backoff := BackoffMin
	firstRun := true
	for ctx.Err() == nil {
		err := m.runOnce(ctx, log, account, firstRun)
		if ctx.Err() != nil {
			return
		}
		if err == nil {
			backoff = BackoffMin
			firstRun = false
			continue
		}
		log.Warn("sync cycle failed", "error", err)
		m.emitState(account.ID, models.SyncError, err, oauth.IsAuthExpired(err))
		select {
		case <-time.After(backoff):
		case <-ctx.Done():
			return
		}
		backoff *= 2
		if backoff > BackoffMax {
			backoff = BackoffMax
		}
	}
}

// sleep waits for d or cancellation.
func sleep(ctx context.Context, d time.Duration) {
	select {
	case <-time.After(d):
	case <-ctx.Done():
	}
}

// runOnce is one connect → full folder sweep → INBOX idle-loop cycle.
func (m *Manager) runOnce(ctx context.Context, log *slog.Logger, account accounts.Account, initial bool) error {
	// Real work is about to start; report Syncing for exactly this span, not
	// for the IDLE waits that follow.
	m.emitState(account.ID, models.SyncSyncing, nil, false)

	session, err := imap.Connect(ctx, account)
	if err != nil {
		return err
	}
	defer session.Logout()
	// A cancelled context (account removed, sync restarted) drops the
	// connection so a blocking IDLE returns promptly; the deferred Logout
	// then finds it closed and is a no-op.
	stop := context.AfterFunc(ctx, session.Close)
	defer stop()

	remote, err := session.ListFoldersWithRoles()
	if err != nil {
		return err
	}
	var inboxName string
	for _, folder := range remote {
		status, err := session.Status(folder.Name)
		if err != nil {
			return err
		}
		folderID, err := m.store.UpsertFolderWithCounts(account.ID, folder.Name, folder.Role, status.UIDValidity, status.Exists, status.Unseen)
		if err != nil {
			return err
		}
		if folder.Role == store.RoleInbox {
			inboxName = folder.Name
		}
		if err := m.syncFolderUIDs(session, account, folderID, folder.Name, folder.Role, initial, status.Exists); err != nil {
			return err
		}
		if ctx.Err() != nil {
			return nil
		}
	}

	if inboxName == "" {
		// No inbox: nothing left to do this cycle; settle to Idle before
		// waiting for the next one.
		m.emitState(account.ID, models.SyncIdle, nil, false)
		sleep(ctx, PollInterval)
		return nil
	}

	// Idle loop: re-sync INBOX on the live connection, then IDLE until the
	// server announces changes, the re-issue cadence elapses, or the
	// full-resync deadline passes. The first re-sync runs straight after the
	// sweep and catches mail that arrived while other folders were syncing.
	deadline := time.Now().Add(FullResyncInterval)
	consecutiveResyncs := 0
	for ctx.Err() == nil {
		status, err := session.Status(inboxName)
		if err != nil {
			return err
		}
		inboxID, err := m.store.UpsertFolderWithCounts(account.ID, inboxName, store.RoleInbox, status.UIDValidity, status.Exists, status.Unseen)
		if err != nil {
			return err
		}
		// Never `initial` here: the sweep already established the
		// last_seen_uid baseline, so anything newer deserves a notification
		// even during the account's first connection.
		if err := m.syncFolderUIDs(session, account, inboxID, inboxName, store.RoleInbox, false, status.Exists); err != nil {
			return err
		}
		if ctx.Err() != nil {
			return nil
		}

		// Body prefetch (issue #40): after the re-sync above has emitted and
		// notified, and before the drain-check below. INBOX is guaranteed to
		// be the selected mailbox here because syncFolderUIDs selected it.
		m.prefetchMessageBodies(log, inboxID, session)

		now := time.Now()
		if !now.Before(deadline) {
			break
		}
		remaining := deadline.Sub(now)

		// Atomicity requirement (issue #39): the decision to IDLE is made
		// from a local, synchronous check nothing can go stale between.
		switch DrainAction(session.DrainMailboxChanged(), consecutiveResyncs) {
		case ActionResync:
			consecutiveResyncs++
			continue
		case ActionEndCycle:
			// The drain consumed an announcement the server will not repeat,
			// and the cap says stop re-syncing — idling here would strand it.
			return nil
		case ActionIdle:
			consecutiveResyncs = 0
		}

		budget := IdleWaitBudget(remaining, IdleReissueInterval)
		m.emitState(account.ID, models.SyncIdle, nil, false)
		outcome, err := session.IdleWait(budget)
		if err != nil {
			return err
		}
		if ctx.Err() != nil {
			return nil
		}
		switch outcome {
		case imap.IdleNewData:
			m.emitState(account.ID, models.SyncSyncing, nil, false)
		case imap.IdleTimeout:
			switch IdleTimeoutAction(remaining, budget) {
			case TimeoutReissue:
				// Looping back runs a real STATUS/SELECT/FETCH pass and
				// prefetch, so report Syncing for it as the NewData arm does.
				m.emitState(account.ID, models.SyncSyncing, nil, false)
				continue
			case TimeoutEndCycle:
				return nil
			}
		case imap.IdleSessionGone:
			return nil
		}
	}
	return nil
}

// syncFolderUIDs fetches new UIDs for a folder, upserts them, emits events,
// and (for inbox, non-initial) notifies. It deliberately does not prefetch
// bodies: prefetch is inbox-only and its timing relative to IDLE matters.
func (m *Manager) syncFolderUIDs(session *imap.Session, account accounts.Account, folderID int64, folderName string, role store.FolderRole, initial bool, serverExists uint32) error {
	if err := session.Select(folderName); err != nil {
		return err
	}
	lastUID, lastSeenUID, err := m.store.FolderSyncState(folderID)
	if err != nil {
		return err
	}
	var envelopes []imap.EnvelopeSummary
	if lastUID == 0 {
		envelopes, err = session.FetchRecentEnvelopes(serverExists, InitialSyncLimit)
	} else {
		envelopes, err = session.FetchEnvelopesFrom(lastUID + 1)
	}
	if err != nil {
		return err
	}
	// Drop any envelope we already have (the `<last+1>:*` form can return
	// the last UID on some servers) and keep only genuinely new ones.
	fresh := envelopes[:0]
	for _, e := range envelopes {
		if e.UID > lastUID {
			fresh = append(fresh, e)
		}
	}
	envelopes = fresh
	if len(envelopes) == 0 {
		return nil
	}

	upserts := make([]*store.MessageUpsert, 0, len(envelopes))
	highestUID := lastUID
	for _, e := range envelopes {
		// No body has been fetched yet at envelope-sync time, so there is
		// nothing to summarise; leave the snippet empty rather than seeding
		// it from the subject.
		upserts = append(upserts, &store.MessageUpsert{
			FolderID: folderID, UID: e.UID, MessageID: e.MessageID, Subject: e.Subject, FromName: e.FromName, FromAddr: e.FromAddr,
			ToAddrs: e.ToAddrs, CcAddrs: e.CcAddrs, Date: e.Date, Seen: e.Seen, Flagged: e.Flagged, HasAttachments: e.HasAttachments,
			RFC822Size: e.RFC822Size,
		})
		if e.UID > highestUID {
			highestUID = e.UID
		}
	}
	ids, err := m.store.UpsertEnvelopes(upserts)
	if err != nil {
		return err
	}
	if role == store.RoleInbox {
		if err := m.store.SetLastSeenUID(folderID, highestUID); err != nil {
			return err
		}
	}

	summaries := make([]models.MessageSummary, 0, len(envelopes))
	var notify []NewMail
	for i, e := range envelopes {
		summaries = append(summaries, models.MessageSummary{
			ID: ids[i], AccountID: account.ID, FolderID: folderID, UID: e.UID, Subject: e.Subject, FromName: e.FromName, FromAddr: e.FromAddr,
			Date: e.Date, Seen: e.Seen, Flagged: e.Flagged, HasAttachments: e.HasAttachments,
		})
		if !initial && role == store.RoleInbox && e.UID > lastSeenUID && !e.Seen {
			notify = append(notify, NewMail{FromName: e.FromName, Subject: e.Subject})
		}
	}
	// Emit and notify immediately; body prefetch must never sit between new
	// mail landing in the DB and this notification.
	m.emitter.Emit(models.EventNewMessages, models.NewMessagesEvent{AccountID: account.ID, FolderID: folderID, Messages: summaries})
	m.emitter.Emit(models.EventMessagesUpdated, models.MessagesUpdatedEvent{FolderID: folderID})
	if len(notify) > 0 && m.notifier != nil {
		m.notifier.NotifyNewMail(account.Email, notify)
	}
	return nil
}

func (m *Manager) prefetchMessageBodies(log *slog.Logger, folderID int64, session *imap.Session) {
	candidates, err := m.store.BodyPrefetchCandidates(folderID, BodyPrefetchWorkingSet, BodyPrefetchLimit, BodyPrefetchMaxBytes)
	if err != nil {
		log.Warn("could not select body prefetch candidates", "folderId", folderID, "error", err)
		return
	}
	for _, c := range candidates {
		size := c.RFC822Size
		if size == 0 {
			size, err = session.FetchMessageSize(c.UID)
			if err != nil {
				log.Warn("body prefetch size lookup failed", "messageId", c.ID, "error", err)
				continue
			}
			if err := m.store.SetMessageSize(c.ID, size); err != nil {
				log.Warn("could not cache message size", "messageId", c.ID, "error", err)
			}
		}
		if size > BodyPrefetchMaxBytes {
			continue
		}
		body, err := session.FetchBody(c.UID)
		if err != nil {
			log.Warn("body prefetch failed", "messageId", c.ID, "error", err)
			continue
		}
		if _, err := CacheBody(m.store, c.ID, body); err != nil {
			log.Warn("could not cache prefetched body", "messageId", c.ID, "error", err)
		}
	}
}

// CacheBody is the body-parse hook shared by the background prefetch and the
// foreground cache miss in GetMessageBody: it stores the parts, the snippet,
// the attachment metadata and the detected shipments in one step.
func CacheBody(st *store.Store, messageID int64, body mailparse.FetchedBody) ([]store.AttachmentRow, error) {
	snippet := mailparse.SnippetForBody(body.Text, body.HTML)
	detected := shipments.Extract(body.Text, body.HTML)
	inserts := make([]store.ShipmentInsert, 0, len(detected))
	for _, s := range detected {
		inserts = append(inserts, store.ShipmentInsert{Carrier: string(s.Carrier), TrackingNumber: s.TrackingNumber, TrackingURL: s.TrackingURL, OrderID: s.OrderID})
	}
	return st.CacheParsedBody(messageID, body.Text, body.HTML, body.ToAddrs, body.CcAddrs, snippet, body.Attachments, inserts)
}

// Action is what the idle loop should do once the drain-check has run.
type Action int

const (
	// ActionIdle: nothing pending; enter IDLE.
	ActionIdle Action = iota
	// ActionResync: the mailbox changed; re-sync INBOX again before idling.
	ActionResync
	// ActionEndCycle: the mailbox changed but the re-sync cap is exhausted;
	// end the cycle so the caller reconnects and sweeps rather than idling on
	// a known-stale view.
	ActionEndCycle
)

// DrainAction decides what to do after a drain-check. Once a drain has
// returned "changed", the announcement is out of the channel and the server
// will never repeat it, so the cap must end the cycle rather than idle; only
// a clean drain may enter IDLE.
func DrainAction(mailboxChanged bool, consecutiveResyncs int) Action {
	if !mailboxChanged {
		return ActionIdle
	}
	if consecutiveResyncs < MaxConsecutiveResyncsBeforeIdle {
		return ActionResync
	}
	return ActionEndCycle
}

// IdleWaitBudget is the timeout to hand the next IdleWait: whichever is
// shorter, the IDLE re-issue cadence or the time left until the full-resync
// deadline. Capping at the deadline is what guarantees the idle loop never
// overruns FullResyncInterval.
func IdleWaitBudget(remainingUntilDeadline, idleReissueInterval time.Duration) time.Duration {
	if remainingUntilDeadline < idleReissueInterval {
		return remainingUntilDeadline
	}
	return idleReissueInterval
}

// TimeoutAction is what the idle loop should do after an IdleTimeout.
type TimeoutAction int

const (
	// TimeoutReissue: the re-issue cadence elapsed but the deadline has not
	// been reached; loop back on the same connection.
	TimeoutReissue TimeoutAction = iota
	// TimeoutEndCycle: the wait was capped by the full-resync deadline, now
	// reached; end the cycle so the caller reconnects and re-sweeps.
	TimeoutEndCycle
)

// IdleTimeoutAction decides from the same two durations that produced the
// wait budget: if the budget was not capped by the deadline the timeout was
// the re-issue cadence firing early; otherwise the deadline itself arrived.
func IdleTimeoutAction(remainingUntilDeadline, waitBudget time.Duration) TimeoutAction {
	if waitBudget >= remainingUntilDeadline {
		return TimeoutEndCycle
	}
	return TimeoutReissue
}
