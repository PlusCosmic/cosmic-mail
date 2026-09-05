package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"strings"

	"github.com/google/uuid"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/attachments"
	"cosmicmail/internal/autoconfig"
	"cosmicmail/internal/imap"
	"cosmicmail/internal/models"
	"cosmicmail/internal/oauth"
	"cosmicmail/internal/omarchy"
	"cosmicmail/internal/send"
	"cosmicmail/internal/settings"
	"cosmicmail/internal/shipments"
	"cosmicmail/internal/store"
	mailsync "cosmicmail/internal/sync"
)

// Notifier is the desktop-notification side of the service.
type Notifier interface {
	mailsync.Notifier
	Test()
}

// App is the service bound to the frontend. Every method here is a command
// in docs/ARCHITECTURE.md; `wails3 generate bindings` turns them into the
// typed TypeScript in frontend/bindings. Methods are intentionally thin:
// they validate inputs, delegate to the internal packages, and return plain
// errors, which reach the frontend as rejected promises carrying the message.
type App struct {
	store    *store.Store
	sync     *mailsync.Manager
	emitter  mailsync.Emitter
	notifier Notifier
}

// NewApp wires the service to its dependencies.
func NewApp(st *store.Store, sync *mailsync.Manager, emitter mailsync.Emitter, notifier Notifier) *App {
	return &App{store: st, sync: sync, emitter: emitter, notifier: notifier}
}

// GetTheme reads the active omarchy theme.
func (a *App) GetTheme() models.OmarchyTheme { return omarchy.ReadTheme() }

// ListAccounts lists all configured accounts (public projection).
func (a *App) ListAccounts() ([]models.Account, error) {
	list, err := accounts.Load()
	if err != nil {
		return nil, err
	}
	out := make([]models.Account, 0, len(list))
	for _, acct := range list {
		out = append(out, acct.Public())
	}
	return out, nil
}

// AddImapAccount adds a plain IMAP account, validating credentials by
// connecting first.
func (a *App) AddImapAccount(input models.ImapAccountInput) (models.Account, error) {
	account := accounts.Account{
		ID: uuid.NewString(), Email: input.Email, DisplayName: input.DisplayName, Kind: models.AccountKindImap,
		ImapHost: input.ImapHost, ImapPort: input.ImapPort, SmtpHost: input.SmtpHost, SmtpPort: input.SmtpPort, Username: input.Username,
	}
	// Store the password in the keyring, then validate by connecting.
	if err := accounts.SetImapPassword(account.ID, input.Password); err != nil {
		return models.Account{}, err
	}
	session, err := imap.Connect(context.Background(), account)
	if err != nil {
		// Roll back the secret we just stored.
		accounts.DeleteSecrets(account.ID, models.AccountKindImap)
		return models.Account{}, fmt.Errorf("Could not connect to the IMAP server: %w", err)
	}
	session.Logout()
	if err := accounts.Add(account); err != nil {
		return models.Account{}, err
	}
	a.sync.Start(account)
	return account.Public(), nil
}

// StartGmailOauth runs the interactive Gmail OAuth flow and registers the
// account.
func (a *App) StartGmailOauth() (models.Account, error) {
	outcome, err := oauth.RunFlow(context.Background())
	if err != nil {
		return models.Account{}, err
	}
	list, err := accounts.Load()
	if err != nil {
		return models.Account{}, err
	}
	// Reuse an existing account for this email if present.
	var account *accounts.Account
	for i := range list {
		if list[i].Email == outcome.Email && list[i].Kind == models.AccountKindGmail {
			account = &list[i]
			break
		}
	}
	isNew := account == nil
	if isNew {
		account = &accounts.Account{
			ID: uuid.NewString(), Email: outcome.Email, DisplayName: outcome.Email, Kind: models.AccountKindGmail,
			ImapHost: "imap.gmail.com", ImapPort: 993, SmtpHost: "smtp.gmail.com", SmtpPort: 587, Username: outcome.Email,
		}
	}
	if err := accounts.SetOAuthRefreshToken(account.ID, outcome.RefreshToken); err != nil {
		return models.Account{}, err
	}
	oauth.CacheToken(account.ID, outcome.AccessToken, outcome.ExpiresIn)
	if isNew {
		if err := accounts.Add(*account); err != nil {
			return models.Account{}, err
		}
	}
	a.sync.Start(*account)
	return account.Public(), nil
}

// ReauthGmailAccount re-runs the interactive Gmail OAuth consent for an
// existing account, in place: the account, its folders and cached mail are
// untouched — only the keyring refresh token is replaced. Errors without
// storing anything if the completed consent belongs to a different Google
// account than the one being reconnected.
func (a *App) ReauthGmailAccount(accountID string) (models.Account, error) {
	account, err := accounts.Find(accountID)
	if err != nil {
		return models.Account{}, err
	}
	if account.Kind != models.AccountKindGmail {
		return models.Account{}, errors.New("Only Gmail accounts use OAuth re-authentication")
	}
	outcome, err := oauth.RunFlow(context.Background())
	if err != nil {
		return models.Account{}, err
	}
	if !strings.EqualFold(outcome.Email, account.Email) {
		return models.Account{}, fmt.Errorf("You signed in as %s, but this account is %s. Sign in with the matching Google account.", outcome.Email, account.Email)
	}
	if err := accounts.SetOAuthRefreshToken(account.ID, outcome.RefreshToken); err != nil {
		return models.Account{}, err
	}
	oauth.CacheToken(account.ID, outcome.AccessToken, outcome.ExpiresIn)
	a.sync.Start(account)
	return account.Public(), nil
}

// RemoveAccount removes an account, its cached data, secrets, and running
// sync task.
func (a *App) RemoveAccount(accountID string) error {
	a.sync.Stop(accountID)
	removed, err := accounts.Remove(accountID)
	if err != nil {
		return err
	}
	if removed != nil {
		accounts.DeleteSecrets(removed.ID, removed.Kind)
		if removed.Kind == models.AccountKindGmail {
			oauth.Forget(removed.ID)
		}
	}
	return a.store.DeleteAccountData(accountID)
}

// ListFolders lists an account's folders.
func (a *App) ListFolders(accountID string) ([]models.Folder, error) {
	rows, err := a.store.ListFolders(accountID)
	if err != nil {
		return nil, err
	}
	out := make([]models.Folder, 0, len(rows))
	for _, r := range rows {
		out = append(out, folderFromRow(r))
	}
	return out, nil
}

func folderFromRow(r store.FolderRow) models.Folder {
	return models.Folder{ID: r.ID, AccountID: r.AccountID, Name: r.Name, Role: r.Role, UnreadCount: r.UnreadCount, TotalCount: r.TotalCount}
}

func summaryFromRow(r store.MessageRow, accountID string) models.MessageSummary {
	return models.MessageSummary{
		ID: r.ID, AccountID: accountID, FolderID: r.FolderID, UID: uint32(r.UID), Subject: r.Subject, FromName: r.FromName, FromAddr: r.FromAddr,
		Date: r.Date, Snippet: r.Snippet, Seen: r.Seen, Flagged: r.Flagged, HasAttachments: r.HasAttachments,
	}
}

// ListMessages pages messages for a folder, newest first.
func (a *App) ListMessages(folderID int64, offset int64, limit int64) ([]models.MessageSummary, error) {
	folder, err := a.store.GetFolder(folderID)
	if err != nil {
		return nil, err
	}
	if folder == nil {
		return nil, errors.New("folder not found")
	}
	rows, err := a.store.PageMessages(folderID, offset, limit)
	if err != nil {
		return nil, err
	}
	out := make([]models.MessageSummary, 0, len(rows))
	for _, r := range rows {
		out = append(out, summaryFromRow(r, folder.AccountID))
	}
	return out, nil
}

// ListUnifiedMessages pages messages across all inbox folders of all
// accounts, newest first.
func (a *App) ListUnifiedMessages(offset int64, limit int64) ([]models.MessageSummary, error) {
	rows, err := a.store.PageUnifiedMessages(offset, limit)
	if err != nil {
		return nil, err
	}
	return unifiedSummaries(rows), nil
}

func unifiedSummaries(rows []store.UnifiedMessageRow) []models.MessageSummary {
	out := make([]models.MessageSummary, 0, len(rows))
	for _, r := range rows {
		out = append(out, summaryFromRow(r.Msg, r.AccountID))
	}
	return out
}

// SearchMessages is a relevance-ranked full-text search over the local
// cache. A nil accountID spans every account; otherwise it is scoped to that
// account. Server-side IMAP SEARCH is not involved.
func (a *App) SearchMessages(query string, accountID *string, offset int64, limit int64) ([]models.MessageSummary, error) {
	rows, err := a.store.SearchMessages(query, accountID, offset, limit)
	if err != nil {
		return nil, err
	}
	return unifiedSummaries(rows), nil
}

func attachmentInfos(rows []store.AttachmentRow) []models.AttachmentInfo {
	out := make([]models.AttachmentInfo, 0, len(rows))
	for _, r := range rows {
		out = append(out, models.AttachmentInfo{ID: r.ID, Filename: r.Filename, MimeType: r.MimeType, SizeBytes: r.SizeBytes, IsInline: r.IsInline})
	}
	return out
}

// GetMessageBody returns a message body, fetching from the server (and
// caching) if not present.
func (a *App) GetMessageBody(messageID int64) (models.MessageBody, error) {
	cached, err := a.store.GetBody(messageID)
	if err != nil {
		return models.MessageBody{}, err
	}
	if cached != nil && cached.Cached {
		rows, err := a.store.ListAttachments(messageID)
		if err != nil {
			return models.MessageBody{}, err
		}
		return models.MessageBody{ID: messageID, HTML: cached.HTML, Text: cached.Text, ToAddrs: models.NonNil(cached.ToAddrs), CcAddrs: models.NonNil(cached.CcAddrs), Attachments: attachmentInfos(rows)}, nil
	}
	loc, err := a.store.LocateMessage(messageID)
	if err != nil {
		return models.MessageBody{}, err
	}
	if loc == nil {
		return models.MessageBody{}, errors.New("message not found")
	}
	account, err := accounts.Find(loc.AccountID)
	if err != nil {
		return models.MessageBody{}, err
	}
	session, err := imap.Connect(context.Background(), account)
	if err != nil {
		return models.MessageBody{}, err
	}
	defer session.Logout()
	if err := session.Select(loc.FolderName); err != nil {
		return models.MessageBody{}, err
	}
	body, err := session.FetchBody(loc.UID)
	if err != nil {
		return models.MessageBody{}, err
	}
	rows, err := mailsync.CacheBody(a.store, messageID, body)
	if err != nil {
		return models.MessageBody{}, err
	}
	return models.MessageBody{ID: messageID, HTML: body.HTML, Text: body.Text, ToAddrs: models.NonNil(body.ToAddrs), CcAddrs: models.NonNil(body.CcAddrs), Attachments: attachmentInfos(rows)}, nil
}

// ListShipmentsForMessage lists shipments detected in a message's cached
// body (empty until the body has been fetched, or if none were detected).
func (a *App) ListShipmentsForMessage(messageID int64) ([]models.Shipment, error) {
	rows, err := a.store.ListShipments(messageID)
	if err != nil {
		return nil, err
	}
	out := make([]models.Shipment, 0, len(rows))
	for _, r := range rows {
		carrier := r.Carrier
		if c, ok := shipments.FromDB(r.Carrier); ok {
			carrier = string(c)
		}
		out = append(out, models.Shipment{ID: r.ID, Carrier: carrier, TrackingNumber: r.TrackingNumber, TrackingURL: r.TrackingURL, OrderID: r.OrderID, DetectedAt: r.DetectedAt})
	}
	return out, nil
}

// SaveAttachment saves an attachment to the user's downloads directory and
// returns its path. Raw RFC822 is not cached, so this refetches the message
// (BODY.PEEK[], non-marking), re-parses, extracts the part by its stable
// index, and writes it under a sanitised, collision-suffixed filename.
func (a *App) SaveAttachment(attachmentID int64) (string, error) {
	loc, err := a.store.GetAttachment(attachmentID)
	if err != nil {
		return "", err
	}
	if loc == nil {
		return "", errors.New("attachment not found")
	}
	account, err := accounts.Find(loc.AccountID)
	if err != nil {
		return "", err
	}
	session, err := imap.Connect(context.Background(), account)
	if err != nil {
		return "", err
	}
	defer session.Logout()
	if err := session.Select(loc.FolderName); err != nil {
		return "", err
	}
	data, err := session.FetchAttachmentBytes(loc.UID, loc.PartIndex)
	if err != nil {
		return "", err
	}
	dir, err := attachments.DownloadsDir()
	if err != nil {
		return "", err
	}
	path := attachments.UniquePath(dir, attachments.SafeFilename(loc.Filename, loc.MimeType, attachmentID))
	if err := os.WriteFile(path, data, 0o644); err != nil {
		return "", fmt.Errorf("Could not save attachment: %w", err)
	}
	return path, nil
}

func (a *App) emitMessagesUpdated(folderID int64) {
	a.emitter.Emit(models.EventMessagesUpdated, models.MessagesUpdatedEvent{FolderID: folderID})
}

// withSelected opens a per-command IMAP session on the message's folder.
func (a *App) withSelected(messageID int64, fn func(session *imap.Session, loc *store.MessageLocation, account accounts.Account) error) (*store.MessageLocation, accounts.Account, error) {
	loc, err := a.store.LocateMessage(messageID)
	if err != nil {
		return nil, accounts.Account{}, err
	}
	if loc == nil {
		return nil, accounts.Account{}, errors.New("message not found")
	}
	account, err := accounts.Find(loc.AccountID)
	if err != nil {
		return nil, accounts.Account{}, err
	}
	session, err := imap.Connect(context.Background(), account)
	if err != nil {
		return nil, accounts.Account{}, err
	}
	defer session.Logout()
	if err := session.Select(loc.FolderName); err != nil {
		return nil, accounts.Account{}, err
	}
	return loc, account, fn(session, loc, account)
}

// MarkRead sets/clears the seen flag on the server and in the local cache.
//
// On a Gmail account, the same physical message is exposed under multiple
// folders (labels); after the primary update, other cached copies sharing
// the same RFC 5322 Message-ID within the account are updated locally too.
func (a *App) MarkRead(messageID int64, seen bool) error {
	loc, account, err := a.withSelected(messageID, func(s *imap.Session, loc *store.MessageLocation, _ accounts.Account) error {
		return s.SetSeenFlag(loc.UID, seen)
	})
	if err != nil {
		return err
	}
	changed, err := a.store.MarkSeen(messageID, seen)
	if err != nil {
		return err
	}
	if changed {
		delta := int64(1)
		if seen {
			delta = -1
		}
		if err := a.store.AdjustFolderUnreadCount(loc.FolderID, delta); err != nil {
			return err
		}
	}
	var siblingFolders []int64
	if account.Kind == models.AccountKindGmail {
		header, err := a.store.MessageIDHeader(messageID)
		if err != nil {
			return err
		}
		if header != nil && *header != "" {
			siblingFolders, err = a.store.MarkSeenForMessageIDSiblings(loc.AccountID, *header, messageID, seen)
			if err != nil {
				return err
			}
		}
	}
	a.emitMessagesUpdated(loc.FolderID)
	for _, f := range siblingFolders {
		a.emitMessagesUpdated(f)
	}
	return nil
}

// MarkFlagged sets/clears \Flagged on the server and in the local cache.
func (a *App) MarkFlagged(messageID int64, flagged bool) error {
	loc, _, err := a.withSelected(messageID, func(s *imap.Session, loc *store.MessageLocation, _ accounts.Account) error {
		return s.SetFlaggedFlag(loc.UID, flagged)
	})
	if err != nil {
		return err
	}
	if _, err := a.store.SetFlagged(messageID, flagged); err != nil {
		return err
	}
	a.emitMessagesUpdated(loc.FolderID)
	return nil
}

// MoveMessage moves a message to another folder of the same account.
func (a *App) MoveMessage(messageID int64, targetFolderID int64) error {
	return a.performMove(messageID, targetFolderID)
}

// ArchiveMessage moves a message to the account's archive-role folder.
func (a *App) ArchiveMessage(messageID int64) error {
	ctx, err := a.store.MessageActionContext(messageID)
	if err != nil {
		return err
	}
	if ctx == nil {
		return errors.New("message not found")
	}
	target, err := a.store.FindFolderByRole(ctx.AccountID, store.RoleArchive)
	if err != nil {
		return err
	}
	if target == nil {
		return errors.New("This account has no archive folder")
	}
	return a.performMove(messageID, target.ID)
}

// DeleteMessage deletes a message: permanently when already in trash,
// otherwise it moves it to trash.
func (a *App) DeleteMessage(messageID int64) error {
	ctx, err := a.store.MessageActionContext(messageID)
	if err != nil {
		return err
	}
	if ctx == nil {
		return errors.New("message not found")
	}
	if ctx.FolderRole == string(store.RoleTrash) {
		if _, _, err := a.withSelected(messageID, func(s *imap.Session, loc *store.MessageLocation, _ accounts.Account) error {
			return s.DeletePermanently(loc.UID)
		}); err != nil {
			return err
		}
		if err := a.store.RemoveMessage(messageID); err != nil {
			return err
		}
		a.emitMessagesUpdated(ctx.FolderID)
		return nil
	}
	target, err := a.store.FindFolderByRole(ctx.AccountID, store.RoleTrash)
	if err != nil {
		return err
	}
	if target == nil {
		return errors.New("This account has no trash folder")
	}
	return a.performMove(messageID, target.ID)
}

// performMove is shared by move/archive/delete-to-trash: it validates the
// target (exists, same account, not the source folder) before any network
// work, performs the server move, removes the local row, bumps the target
// folder's counts, and emits mail:messages-updated for both folders.
func (a *App) performMove(messageID int64, targetFolderID int64) error {
	ctx, err := a.store.MessageActionContext(messageID)
	if err != nil {
		return err
	}
	if ctx == nil {
		return errors.New("message not found")
	}
	target, err := a.store.GetFolder(targetFolderID)
	if err != nil {
		return err
	}
	if target == nil {
		return errors.New("Target folder not found")
	}
	if target.AccountID != ctx.AccountID {
		return errors.New("Moving messages between accounts is not supported")
	}
	if target.ID == ctx.FolderID {
		return errors.New("The message is already in that folder")
	}
	if _, _, err := a.withSelected(messageID, func(s *imap.Session, loc *store.MessageLocation, _ accounts.Account) error {
		return s.MoveMessage(loc.UID, target.Name)
	}); err != nil {
		return err
	}
	if err := a.store.RemoveMessage(messageID); err != nil {
		return err
	}
	if err := a.store.IncrementFolderCounts(targetFolderID, !ctx.Seen); err != nil {
		return err
	}
	a.emitMessagesUpdated(ctx.FolderID)
	a.emitMessagesUpdated(targetFolderID)
	return nil
}

// SendMessage submits a plain-text message through the selected account's
// SMTP server.
func (a *App) SendMessage(input models.SendMessageInput) error {
	account, err := accounts.Find(input.AccountID)
	if err != nil {
		return err
	}
	var replyMessageID *string
	if input.ReplyToMessageID != nil {
		md, err := a.store.GetReplyMetadata(*input.ReplyToMessageID)
		if err != nil {
			return err
		}
		if md == nil {
			return errors.New("reply message not found")
		}
		if md.AccountID != account.ID {
			return errors.New("reply message belongs to another account")
		}
		replyMessageID = md.MessageID
	}
	return send.Send(context.Background(), account, input, replyMessageID)
}

// SyncFolder triggers a background re-sync for the account that owns a
// folder.
func (a *App) SyncFolder(folderID int64) error {
	folder, err := a.store.GetFolder(folderID)
	if err != nil {
		return err
	}
	if folder == nil {
		return errors.New("folder not found")
	}
	return a.SyncAccount(folder.AccountID)
}

// SyncAccount triggers a background re-sync for an account.
func (a *App) SyncAccount(accountID string) error {
	account, err := accounts.Find(accountID)
	if err != nil {
		return err
	}
	a.sync.Start(account)
	return nil
}

// SyncAllAccounts restarts every configured account's background sync task
// (tray "Sync now"; backend-only, not part of the wire surface).
func (a *App) syncAllAccounts() (int, error) {
	list, err := accounts.Load()
	if err != nil {
		return 0, err
	}
	for _, account := range list {
		a.sync.Start(account)
	}
	return len(list), nil
}

// TestNotification sends a sample notification.
func (a *App) TestNotification() error {
	if a.notifier != nil {
		a.notifier.Test()
	}
	return nil
}

// DiscoverAccountConfig discovers IMAP/SMTP settings for an email address.
// It errors only on an invalid address; a "not found" result falls through to
// a heuristic guess with confident: false.
func (a *App) DiscoverAccountConfig(email string) (models.DiscoveredConfig, error) {
	cfg, err := autoconfig.Discover(context.Background(), email)
	if err != nil {
		return models.DiscoveredConfig{}, errors.New("Please enter a valid email address")
	}
	return cfg, nil
}

// GetSettings reads the global application settings. It never errors: a
// missing or malformed settings file yields defaults.
func (a *App) GetSettings() models.Settings { return settings.Load() }

// UpdateSettings persists the global application settings and returns the
// stored value.
func (a *App) UpdateSettings(s models.Settings) (models.Settings, error) {
	if err := settings.Save(s); err != nil {
		return models.Settings{}, err
	}
	return s, nil
}

// startConfiguredAccounts spawns sync tasks for every account on disk.
func (a *App) startConfiguredAccounts() {
	list, err := accounts.Load()
	if err != nil {
		slog.Warn("failed to load accounts at startup", "error", err)
		return
	}
	for _, account := range list {
		a.sync.Start(account)
	}
}
