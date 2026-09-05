// Package oauth implements Gmail OAuth2 (authorization-code + PKCE, loopback
// redirect) and the access-token cache.
//
// Follows RFC 8252: a loopback listener on an ephemeral port, the consent
// URL opened in the user's browser, the redirect captured. The refresh token
// is stored in the keyring; access tokens are cached in memory with their
// expiry and refreshed on demand.
package oauth

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/oauth2"

	"cosmicmail/internal/accounts"
	"cosmicmail/internal/xdg"
)

const (
	authURL     = "https://accounts.google.com/o/oauth2/v2/auth"
	tokenURL    = "https://oauth2.googleapis.com/token"
	userinfoURL = "https://openidconnect.googleapis.com/v1/userinfo"
	// redirectTimeout bounds the wait for Google to redirect back.
	redirectTimeout = 5 * time.Minute
)

// BakedClientID / BakedClientSecret are the compile-time defaults for
// packaged builds, set with
//
//	-ldflags "-X cosmicmail/internal/oauth.BakedClientID=… -X cosmicmail/internal/oauth.BakedClientSecret=…"
//
// (the Rust build read COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID at compile time).
// Deliberately distinct from the runtime variable names so a dev shell never
// silently bakes its credentials into a binary. Shipping these is the
// standard installed-app pattern (RFC 8252 §8.5): desktop-app client
// credentials are non-confidential; PKCE secures the flow.
var (
	BakedClientID     string
	BakedClientSecret string
)

// ErrAuthExpired marks the stored Gmail credentials as unable to mint access
// tokens: retrying cannot help; only a new interactive consent
// (ReauthGmailAccount) fixes it. It is wrapped into the error chain so the
// sync loop can errors.Is for it anywhere in the chain.
var ErrAuthExpired = errors.New("Gmail sign-in expired")

// IsAuthExpired reports whether an error chain carries ErrAuthExpired.
func IsAuthExpired(err error) bool { return errors.Is(err, ErrAuthExpired) }

// Outcome is the result of a completed OAuth flow.
type Outcome struct {
	Email        string
	RefreshToken string
	AccessToken  string
	ExpiresIn    time.Duration
}

type cachedToken struct {
	accessToken string
	expiresAt   time.Time
}

var (
	cacheMu    sync.Mutex
	tokenCache = map[string]cachedToken{}
)

// OpenURL opens the consent URL in the user's browser. main wires it to the
// Wails browser manager; tests and headless builds can replace it.
var OpenURL = func(url string) error { return errors.New("no browser opener configured") }

type googleOAuthFile struct {
	ClientID     string `json:"clientId"`
	ClientSecret string `json:"clientSecret"`
}

func credentialsPath() (string, error) {
	dir, err := xdg.AppConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "google-oauth.json"), nil
}

func envNonEmpty(key string) string { return strings.TrimSpace(os.Getenv(key)) }

// credentials resolves the Google OAuth client credentials, first hit wins:
// runtime env vars (dev override) → google-oauth.json (user self-provisioning)
// → compile-time baked defaults (packaged builds).
func credentials() (clientID, clientSecret string, err error) {
	envSecret := envNonEmpty("COSMIC_MAIL_GOOGLE_CLIENT_SECRET")
	if id := envNonEmpty("COSMIC_MAIL_GOOGLE_CLIENT_ID"); id != "" {
		return id, envSecret, nil
	}
	path, err := credentialsPath()
	if err != nil {
		return "", "", err
	}
	if data, readErr := os.ReadFile(path); readErr == nil {
		var f googleOAuthFile
		if err := json.Unmarshal(data, &f); err != nil {
			return "", "", fmt.Errorf("parsing %s: %w", path, err)
		}
		if strings.TrimSpace(f.ClientID) == "" {
			return "", "", fmt.Errorf("clientId in %s is empty", path)
		}
		secret := envSecret
		if secret == "" {
			secret = strings.TrimSpace(f.ClientSecret)
		}
		return f.ClientID, secret, nil
	} else if !errors.Is(readErr, os.ErrNotExist) {
		return "", "", fmt.Errorf("reading %s: %w", path, readErr)
	}
	if id := strings.TrimSpace(BakedClientID); id != "" {
		secret := envSecret
		if secret == "" {
			secret = strings.TrimSpace(BakedClientSecret)
		}
		return id, secret, nil
	}
	return "", "", fmt.Errorf("Gmail sign-in is not configured. Create an OAuth 2.0 \"Desktop app\" client in the "+
		"Google Cloud console, then save its credentials to %s as "+
		"{\"clientId\": \"…\", \"clientSecret\": \"…\"} (clientSecret optional). "+
		"The COSMIC_MAIL_GOOGLE_CLIENT_ID / COSMIC_MAIL_GOOGLE_CLIENT_SECRET "+
		"environment variables override the file.", path)
}

func config(redirectURL string) (*oauth2.Config, error) {
	id, secret, err := credentials()
	if err != nil {
		return nil, err
	}
	return &oauth2.Config{
		ClientID:     id,
		ClientSecret: secret,
		Endpoint:     oauth2.Endpoint{AuthURL: authURL, TokenURL: tokenURL, AuthStyle: oauth2.AuthStyleInParams},
		RedirectURL:  redirectURL,
		Scopes:       []string{"https://mail.google.com/", "openid", "email"},
	}, nil
}

// httpContext returns a context whose oauth2 HTTP client does not follow
// redirects (SSRF hardening).
func httpContext(ctx context.Context) context.Context {
	client := &http.Client{
		Timeout: 30 * time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
	return context.WithValue(ctx, oauth2.HTTPClient, client)
}

// RunFlow runs the full interactive OAuth flow: open the browser, capture
// the redirect, exchange the code, and learn the account email. Times out
// after 5 minutes.
func RunFlow(ctx context.Context) (*Outcome, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, fmt.Errorf("binding loopback redirect listener: %w", err)
	}
	defer listener.Close()
	port := listener.Addr().(*net.TCPAddr).Port
	cfg, err := config(fmt.Sprintf("http://127.0.0.1:%d", port))
	if err != nil {
		return nil, err
	}

	verifier := oauth2.GenerateVerifier()
	state := oauth2.GenerateVerifier()
	consentURL := cfg.AuthCodeURL(state, oauth2.AccessTypeOffline, oauth2.S256ChallengeOption(verifier), oauth2.SetAuthURLParam("prompt", "consent"))
	if err := OpenURL(consentURL); err != nil {
		return nil, fmt.Errorf("opening browser for consent: %w", err)
	}

	code, gotState, err := acceptRedirect(ctx, listener)
	if err != nil {
		return nil, fmt.Errorf("capturing OAuth redirect: %w", err)
	}
	if gotState != state {
		return nil, errors.New("OAuth state mismatch — possible CSRF; aborting")
	}

	token, err := cfg.Exchange(httpContext(ctx), code, oauth2.VerifierOption(verifier))
	if err != nil {
		return nil, fmt.Errorf("exchanging authorization code: %w", err)
	}
	if token.RefreshToken == "" {
		return nil, errors.New("Google did not return a refresh token")
	}
	expiresIn := time.Until(token.Expiry)
	if token.Expiry.IsZero() || expiresIn <= 0 {
		expiresIn = time.Hour
	}
	email, err := fetchEmail(ctx, token.AccessToken)
	if err != nil {
		return nil, err
	}
	return &Outcome{Email: email, RefreshToken: token.RefreshToken, AccessToken: token.AccessToken, ExpiresIn: expiresIn}, nil
}

// acceptRedirect accepts exactly one loopback HTTP request, extracts `code`
// and `state`, and responds with a small "you can close this tab" page.
func acceptRedirect(ctx context.Context, listener net.Listener) (code, state string, err error) {
	type result struct {
		code, state string
		err         error
	}
	done := make(chan result, 1)
	server := &http.Server{ReadHeaderTimeout: 10 * time.Second}
	var once sync.Once
	server.Handler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		fmt.Fprint(w, `<!doctype html><html><head><meta charset="utf-8"><title>Cosmic Mail</title></head><body style="font-family:sans-serif;padding:2rem;">You can close this tab and return to Cosmic Mail.</body></html>`)
		once.Do(func() {
			if e := q.Get("error"); e != "" {
				done <- result{err: fmt.Errorf("Google returned an OAuth error: %s", e)}
				return
			}
			c := q.Get("code")
			if c == "" {
				done <- result{err: errors.New("redirect did not include an authorization code")}
				return
			}
			done <- result{code: c, state: q.Get("state")}
		})
	})
	go func() { _ = server.Serve(listener) }()
	defer server.Close()

	timer := time.NewTimer(redirectTimeout)
	defer timer.Stop()
	select {
	case r := <-done:
		return r.code, r.state, r.err
	case <-timer.C:
		return "", "", errors.New("timed out waiting for Google to redirect (5 minutes)")
	case <-ctx.Done():
		return "", "", ctx.Err()
	}
}

// fetchEmail reads the account's email from the OpenID userinfo endpoint.
func fetchEmail(ctx context.Context, accessToken string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, userinfoURL, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)
	resp, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		return "", fmt.Errorf("requesting userinfo: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", fmt.Errorf("userinfo returned an error status: %s", resp.Status)
	}
	var info struct {
		Email string `json:"email"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&info); err != nil {
		return "", fmt.Errorf("parsing userinfo response: %w", err)
	}
	if info.Email == "" {
		return "", errors.New("userinfo did not include an email")
	}
	return info.Email, nil
}

// AccessToken returns a valid access token for the account, refreshing via
// the stored refresh token if the cached one is missing or near expiry.
//
// Dead credentials are classified, not just reported: exactly two failures
// carry ErrAuthExpired — a missing keyring entry, and a refresh exchange
// rejected with OAuth `invalid_grant` (a lapsed "Testing"-status refresh
// token, or revoked access). Every other failure (network, keyring outage,
// server 5xx) stays a plain retryable error.
func AccessToken(ctx context.Context, accountID string) (string, error) {
	cacheMu.Lock()
	if entry, ok := tokenCache[accountID]; ok && entry.expiresAt.After(time.Now().Add(time.Minute)) {
		cacheMu.Unlock()
		return entry.accessToken, nil
	}
	cacheMu.Unlock()

	refresh, err := accounts.GetOAuthRefreshToken(accountID)
	if err != nil {
		if errors.Is(err, accounts.ErrNoSecret) {
			return "", fmt.Errorf("no stored refresh token for account: %w: %w", ErrAuthExpired, err)
		}
		return "", fmt.Errorf("no stored refresh token for account: %w", err)
	}
	cfg, err := config("")
	if err != nil {
		return "", err
	}
	token, err := cfg.TokenSource(httpContext(ctx), &oauth2.Token{RefreshToken: refresh, Expiry: time.Now().Add(-time.Hour)}).Token()
	if err != nil {
		var re *oauth2.RetrieveError
		if errors.As(err, &re) && re.ErrorCode == "invalid_grant" {
			return "", fmt.Errorf("refreshing access token: %w: %w", ErrAuthExpired, err)
		}
		return "", fmt.Errorf("refreshing access token: %w", err)
	}
	expiresIn := time.Until(token.Expiry)
	if token.Expiry.IsZero() || expiresIn <= 0 {
		expiresIn = time.Hour
	}
	// Google may issue a rotated refresh token; persist it if so.
	if token.RefreshToken != "" && token.RefreshToken != refresh {
		_ = accounts.SetOAuthRefreshToken(accountID, token.RefreshToken)
	}
	CacheToken(accountID, token.AccessToken, expiresIn)
	return token.AccessToken, nil
}

// CacheToken seeds the in-memory cache (used right after the initial flow).
func CacheToken(accountID, accessToken string, expiresIn time.Duration) {
	cacheMu.Lock()
	defer cacheMu.Unlock()
	tokenCache[accountID] = cachedToken{accessToken: accessToken, expiresAt: time.Now().Add(expiresIn)}
}

// Forget drops any cached token for an account (used on removal).
func Forget(accountID string) {
	cacheMu.Lock()
	defer cacheMu.Unlock()
	delete(tokenCache, accountID)
}

// XOAuth2Client is the SASL XOAUTH2 mechanism shared by IMAP and SMTP. The
// initial response is the raw `user=…\x01auth=Bearer …\x01\x01` string; the
// transports base64-encode it themselves (pre-encoding double-encodes and
// Gmail rejects it with "Invalid SASL argument").
type XOAuth2Client struct {
	User  string
	Token string
}

// Start implements sasl.Client.
func (c *XOAuth2Client) Start() (string, []byte, error) {
	return "XOAUTH2", []byte("user=" + c.User + "\x01auth=Bearer " + c.Token + "\x01\x01"), nil
}

// Next implements sasl.Client: a server challenge after the initial response
// is an error document; answering with an empty line makes the server report
// the failure as the tagged NO.
func (c *XOAuth2Client) Next(challenge []byte) ([]byte, error) {
	return []byte{}, nil
}
