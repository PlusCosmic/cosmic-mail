// Package accounts holds the account model, `accounts.json` persistence, and
// the keyring secret helpers.
//
// Non-secret account configuration is stored in
// `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`. Secrets (IMAP passwords, Gmail
// OAuth refresh tokens) live only in the Secret Service keyring and are never
// written to disk or logged.
package accounts

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/zalando/go-keyring"

	"cosmicmail/internal/buildinfo"
	"cosmicmail/internal/models"
	"cosmicmail/internal/settings"
	"cosmicmail/internal/xdg"
)

// KeyringService is the Secret Service `service` attribute shared by every
// secret this app stores. It matches the Rust build (keyring v4 also used the
// `service` + `username` attributes), so existing entries stay readable.
const KeyringService = "dev.pluscosmic.mail"

// Account is the full record persisted to accounts.json (contains no secrets).
type Account struct {
	ID          string             `json:"id"`
	Email       string             `json:"email"`
	DisplayName string             `json:"displayName"`
	Kind        models.AccountKind `json:"kind"`
	// Connection details. For Gmail these are the fixed Gmail servers.
	ImapHost string `json:"imapHost"`
	ImapPort int    `json:"imapPort"`
	SmtpHost string `json:"smtpHost"`
	SmtpPort int    `json:"smtpPort"`
	Username string `json:"username"`
}

// Public projects the account onto the wire type (no connection details).
func (a Account) Public() models.Account {
	return models.Account{ID: a.ID, Email: a.Email, DisplayName: a.DisplayName, Kind: a.Kind}
}

// Path is `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`.
func Path() (string, error) {
	dir, err := xdg.AppConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "accounts.json"), nil
}

// Load reads every account from disk; a missing file is an empty list.
func Load() ([]Account, error) {
	path, err := Path()
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return []Account{}, nil
	}
	if err != nil {
		return nil, fmt.Errorf("reading %s: %w", path, err)
	}
	var accounts []Account
	if err := json.Unmarshal(data, &accounts); err != nil {
		return nil, fmt.Errorf("parsing accounts.json: %w", err)
	}
	if accounts == nil {
		accounts = []Account{}
	}
	return accounts, nil
}

// Save atomically persists the account list to accounts.json.
func Save(accounts []Account) error {
	path, err := Path()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("creating %s: %w", filepath.Dir(path), err)
	}
	data, err := json.MarshalIndent(models.NonNil(accounts), "", "  ")
	if err != nil {
		return fmt.Errorf("serializing accounts: %w", err)
	}
	return settings.WriteFileAtomic(path, data)
}

// Find returns the account with the given id.
func Find(accountID string) (Account, error) {
	accounts, err := Load()
	if err != nil {
		return Account{}, err
	}
	for _, a := range accounts {
		if a.ID == accountID {
			return a, nil
		}
	}
	return Account{}, errors.New("account not found")
}

// Add appends an account to the on-disk list and persists it.
func Add(account Account) error {
	accounts, err := Load()
	if err != nil {
		return err
	}
	return Save(append(accounts, account))
}

// Remove deletes an account from the on-disk list, returning it if present.
func Remove(accountID string) (*Account, error) {
	accounts, err := Load()
	if err != nil {
		return nil, err
	}
	for i, a := range accounts {
		if a.ID == accountID {
			removed := a
			accounts = append(accounts[:i], accounts[i+1:]...)
			if err := Save(accounts); err != nil {
				return nil, err
			}
			return &removed, nil
		}
	}
	return nil, nil
}

// --- keyring helpers ---------------------------------------------------------

// ErrNoSecret reports a keyring entry that does not exist (as opposed to a
// keyring that is locked or unavailable). The OAuth layer classifies it as
// "sign in again", never as a transient failure.
var ErrNoSecret = errors.New("no such keyring entry")

func imapPasswordKey(accountID string) string      { return "imap-password:" + accountID }
func oauthRefreshTokenKey(accountID string) string { return "oauth-refresh-token:" + accountID }

// keyringGet/Set/Delete are swappable so tests never touch the real Secret
// Service.
var (
	keyringGet    = keyring.Get
	keyringSet    = keyring.Set
	keyringDelete = keyring.Delete
)

func getSecret(key string, what string) (string, error) {
	v, err := keyringGet(KeyringService, key)
	if errors.Is(err, keyring.ErrNotFound) {
		return "", fmt.Errorf("reading %s from keyring: %w", what, ErrNoSecret)
	}
	if err != nil {
		return "", fmt.Errorf("reading %s from keyring: %w", what, err)
	}
	return v, nil
}

// SetImapPassword stores an IMAP password for the account.
func SetImapPassword(accountID, password string) error {
	if err := keyringSet(KeyringService, imapPasswordKey(accountID), password); err != nil {
		return fmt.Errorf("storing IMAP password in keyring: %w", err)
	}
	return nil
}

// GetImapPassword fetches the IMAP password for the account.
//
// Development builds only: if COSMIC_MAIL_TEST_IMAP_PASSWORD is set, it is
// returned without touching the keyring, so E2E tests can run against a
// fixture IMAP server on a headless machine with no Secret Service. Compiled
// out of production builds; the keyring path is unchanged for real accounts.
func GetImapPassword(accountID string) (string, error) {
	if buildinfo.Debug {
		if v, ok := os.LookupEnv("COSMIC_MAIL_TEST_IMAP_PASSWORD"); ok {
			return v, nil
		}
	}
	return getSecret(imapPasswordKey(accountID), "IMAP password")
}

// SetOAuthRefreshToken stores the Gmail OAuth refresh token for the account.
func SetOAuthRefreshToken(accountID, token string) error {
	if err := keyringSet(KeyringService, oauthRefreshTokenKey(accountID), token); err != nil {
		return fmt.Errorf("storing OAuth refresh token in keyring: %w", err)
	}
	return nil
}

// GetOAuthRefreshToken fetches the Gmail OAuth refresh token for the account.
func GetOAuthRefreshToken(accountID string) (string, error) {
	return getSecret(oauthRefreshTokenKey(accountID), "OAuth refresh token")
}

// DeleteSecrets removes the account's secrets, best effort (missing entries
// and deletion errors are ignored).
func DeleteSecrets(accountID string, kind models.AccountKind) {
	key := imapPasswordKey(accountID)
	if kind == models.AccountKindGmail {
		key = oauthRefreshTokenKey(accountID)
	}
	_ = keyringDelete(KeyringService, key)
}
