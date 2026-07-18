//! Gmail OAuth2 (authorization-code + PKCE, loopback redirect) and token cache.
//!
//! Follows RFC 8252: we bind a loopback `TcpListener` on an ephemeral port,
//! open the consent URL in the user's browser, and capture the redirect. The
//! refresh token is stored in the keyring; access tokens are cached in memory
//! with their expiry and refreshed on demand.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};

use crate::accounts;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// In-memory access-token cache, keyed by account id.
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

static TOKEN_CACHE: LazyLock<Mutex<HashMap<String, CachedToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Marker error: the stored Gmail credentials can no longer mint access
/// tokens, and retrying cannot help — only a new interactive consent
/// (`reauth_gmail_account`) fixes it. Attached via `.context(AuthExpired)` so
/// the sync loop can `downcast_ref` for it anywhere in the chain.
#[derive(Debug, thiserror::Error)]
#[error("Gmail sign-in expired")]
pub struct AuthExpired;

/// Whether an error chain contains the [`AuthExpired`] marker.
pub fn is_auth_expired(err: &anyhow::Error) -> bool {
    err.downcast_ref::<AuthExpired>().is_some()
}

/// Result of a completed OAuth flow.
pub struct OAuthOutcome {
    pub email: String,
    pub refresh_token: String,
    pub access_token: String,
    pub expires_in: Duration,
}

/// Compile-time baked-in defaults for packaged builds: set
/// `COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID` (+ `..._SECRET`) when building a release
/// so installs work out of the box. Deliberately distinct from the runtime var
/// names so a dev shell never silently bakes its credentials into a binary.
/// Shipping these in the binary is the standard installed-app pattern (RFC 8252
/// §8.5 — desktop-app client credentials are non-confidential; PKCE secures the
/// flow).
const BAKED_CLIENT_ID: Option<&str> = option_env!("COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID");
const BAKED_CLIENT_SECRET: Option<&str> = option_env!("COSMIC_MAIL_BUILD_GOOGLE_CLIENT_SECRET");

/// On-disk client credentials: `$XDG_CONFIG_HOME/cosmic-mail/google-oauth.json`.
/// Google desktop-app client secrets are non-confidential by design, so a plain
/// config file is fine here (see ARCHITECTURE.md — the keyring rule covers real
/// secrets: passwords and refresh tokens).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleOAuthFile {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

fn credentials_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine config dir (XDG_CONFIG_HOME)")?
        .join("cosmic-mail");
    Ok(dir.join("google-oauth.json"))
}

fn env_nonempty(key: &str) -> Option<String> {
    nonempty(std::env::var(key).ok())
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// Resolve the Google OAuth client credentials, first hit wins:
/// runtime env vars (dev override) → `google-oauth.json` (user self-provisioning)
/// → compile-time baked defaults (packaged builds).
fn credentials() -> Result<(ClientId, Option<ClientSecret>)> {
    let env_secret = env_nonempty("COSMIC_MAIL_GOOGLE_CLIENT_SECRET");
    if let Some(id) = env_nonempty("COSMIC_MAIL_GOOGLE_CLIENT_ID") {
        return Ok((ClientId::new(id), env_secret.map(ClientSecret::new)));
    }

    let path = credentials_path()?;
    if path.exists() {
        let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let file: GoogleOAuthFile =
            serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))?;
        if file.client_id.trim().is_empty() {
            bail!("clientId in {} is empty", path.display());
        }
        let secret = env_secret.or(nonempty(file.client_secret));
        return Ok((ClientId::new(file.client_id), secret.map(ClientSecret::new)));
    }

    if let Some(id) = nonempty(BAKED_CLIENT_ID.map(String::from)) {
        let secret = env_secret.or(nonempty(BAKED_CLIENT_SECRET.map(String::from)));
        return Ok((ClientId::new(id), secret.map(ClientSecret::new)));
    }

    Err(anyhow!(
        "Gmail sign-in is not configured. Create an OAuth 2.0 \"Desktop app\" client in the \
         Google Cloud console, then save its credentials to {} as \
         {{\"clientId\": \"…\", \"clientSecret\": \"…\"}} (clientSecret optional). \
         The COSMIC_MAIL_GOOGLE_CLIENT_ID / COSMIC_MAIL_GOOGLE_CLIENT_SECRET \
         environment variables override the file.",
        path.display()
    ))
}

/// The oauth2 HTTP client (its own reqwest), configured to not follow
/// redirects (SSRF hardening, per the oauth2 crate guidance).
fn http_client() -> Result<oauth2::reqwest::Client> {
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .context("building oauth http client")
}

/// Run the full interactive OAuth flow: open the browser, capture the redirect,
/// exchange the code, and learn the account email. Times out after 5 minutes.
pub async fn run_oauth_flow(app: &tauri::AppHandle) -> Result<OAuthOutcome> {
    let _ = app; // reserved for future UI feedback

    let (id, secret) = credentials()?;

    // Bind a loopback listener on an ephemeral port for the redirect.
    let listener =
        TcpListener::bind("127.0.0.1:0").context("binding loopback redirect listener")?;
    let port = listener
        .local_addr()
        .context("reading loopback addr")?
        .port();
    let redirect = format!("http://127.0.0.1:{port}");

    let mut client = BasicClient::new(id)
        .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).context("auth url")?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).context("token url")?)
        .set_redirect_uri(RedirectUrl::new(redirect).context("redirect url")?);
    if let Some(secret) = secret {
        client = client.set_client_secret(secret);
    }

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("https://mail.google.com/".to_string()))
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    tauri_plugin_opener::open_url(auth_url.as_str(), None::<&str>)
        .context("opening browser for consent")?;

    // Capture the redirect on a blocking thread with a 5-minute deadline.
    let (code, state) = tokio::task::spawn_blocking(move || accept_redirect(listener))
        .await
        .context("redirect task panicked")?
        .context("capturing OAuth redirect")?;

    if state != *csrf.secret() {
        bail!("OAuth state mismatch — possible CSRF; aborting");
    }

    let http = http_client()?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.secret().to_string()))
        .request_async(&http)
        .await
        .context("exchanging authorization code")?;

    let access_token = token.access_token().secret().to_string();
    let refresh_token = token
        .refresh_token()
        .map(|t| t.secret().to_string())
        .context("Google did not return a refresh token")?;
    let expires_in = token.expires_in().unwrap_or(Duration::from_secs(3600));

    let email = fetch_email(&access_token).await?;

    Ok(OAuthOutcome {
        email,
        refresh_token,
        access_token,
        expires_in,
    })
}

/// Accept exactly one loopback HTTP request, extract `code` and `state`, and
/// respond with a small "you can close this tab" page. 5-minute timeout.
fn accept_redirect(listener: TcpListener) -> Result<(String, String)> {
    listener
        .set_nonblocking(false)
        .context("configuring listener")?;
    let deadline = Instant::now() + Duration::from_secs(5 * 60);

    // Use a background accept with a timeout by polling the listener.
    listener
        .set_nonblocking(true)
        .context("configuring listener nonblocking")?;

    loop {
        if Instant::now() > deadline {
            bail!("timed out waiting for Google to redirect (5 minutes)");
        }
        match listener.accept() {
            Ok((mut stream, _addr)) => {
                stream.set_nonblocking(false).ok();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).context("reading redirect request")?;
                let request = String::from_utf8_lossy(&buf[..n]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                let params = parse_query(target);
                let body = "<!doctype html><html><head><meta charset=\"utf-8\">\
                    <title>Cosmic Mail</title></head><body style=\"font-family:sans-serif;\
                    padding:2rem;\">You can close this tab and return to Cosmic Mail.</body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();

                if let Some(err) = params.get("error") {
                    bail!("Google returned an OAuth error: {err}");
                }
                let code = params
                    .get("code")
                    .cloned()
                    .context("redirect did not include an authorization code")?;
                let state = params.get("state").cloned().unwrap_or_default();
                return Ok((code, state));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e).context("accepting redirect connection"),
        }
    }
}

/// Parse a `?a=b&c=d` query string from an HTTP request target.
fn parse_query(target: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = target.split('?').nth(1) {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                let key = percent_decode(k);
                let val = percent_decode(v);
                map.insert(key, val);
            }
        }
    }
    map
}

/// Minimal percent-decoding for query values (handles `%XX` and `+`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(serde::Deserialize)]
struct UserInfo {
    email: Option<String>,
}

/// Fetch the account's email address from the OpenID userinfo endpoint.
async fn fetch_email(access_token: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let info: UserInfo = client
        .get(USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .context("requesting userinfo")?
        .error_for_status()
        .context("userinfo returned an error status")?
        .json()
        .await
        .context("parsing userinfo response")?;
    info.email.context("userinfo did not include an email")
}

/// Return a valid access token for the account, refreshing via the stored
/// refresh token if the cached one is missing or near expiry.
pub async fn access_token(account_id: &str) -> Result<String> {
    // Fast path: cached and not near expiry.
    {
        let cache = TOKEN_CACHE.lock().expect("token cache poisoned");
        if let Some(entry) = cache.get(account_id) {
            if entry.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(entry.access_token.clone());
            }
        }
    }

    let refresh = match accounts::get_oauth_refresh_token(account_id) {
        Ok(token) => token,
        Err(err) => {
            // A vanished keyring entry can only be fixed by a new consent;
            // other keyring failures (locked/unavailable Secret Service) are
            // transient and stay plain retryable errors.
            let missing = err
                .downcast_ref::<keyring::Error>()
                .is_some_and(|e| matches!(e, keyring::Error::NoEntry));
            let err = err.context("no stored refresh token for account");
            return Err(if missing {
                err.context(AuthExpired)
            } else {
                err
            });
        }
    };

    let (id, secret) = credentials()?;
    let mut client = BasicClient::new(id)
        .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).context("auth url")?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).context("token url")?);
    if let Some(secret) = secret {
        client = client.set_client_secret(secret);
    }

    let http = http_client()?;
    let token = match client
        .exchange_refresh_token(&RefreshToken::new(refresh))
        .request_async(&http)
        .await
    {
        Ok(token) => token,
        Err(err) => {
            // `invalid_grant` means the refresh token itself is dead (expired
            // 7-day "Testing"-status token, or revoked access) — retrying is
            // pointless. Network/server failures stay plain retryable errors.
            let dead_grant = matches!(&err, oauth2::RequestTokenError::ServerResponse(resp)
                if matches!(resp.error(), oauth2::basic::BasicErrorResponseType::InvalidGrant));
            let err = anyhow::Error::new(err).context("refreshing access token");
            return Err(if dead_grant {
                err.context(AuthExpired)
            } else {
                err
            });
        }
    };

    let access = token.access_token().secret().to_string();
    let expires_in = token.expires_in().unwrap_or(Duration::from_secs(3600));

    // Google may issue a rotated refresh token; persist it if so.
    if let Some(new_refresh) = token.refresh_token() {
        let _ = accounts::set_oauth_refresh_token(account_id, new_refresh.secret());
    }

    cache_token(account_id, &access, expires_in);
    Ok(access)
}

/// Seed the in-memory cache (used right after the initial flow).
pub fn cache_token(account_id: &str, access_token: &str, expires_in: Duration) {
    let mut cache = TOKEN_CACHE.lock().expect("token cache poisoned");
    cache.insert(
        account_id.to_string(),
        CachedToken {
            access_token: access_token.to_string(),
            expires_at: Instant::now() + expires_in,
        },
    );
}

/// Drop any cached token for an account (used on removal).
pub fn forget(account_id: &str) {
    if let Ok(mut cache) = TOKEN_CACHE.lock() {
        cache.remove(account_id);
    }
}
