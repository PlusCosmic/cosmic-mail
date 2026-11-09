//! Desktop process and window lifecycle.
//!
//! The application process owns background sync for the whole graphical
//! session. Its main window is only a view onto that process: closing the view
//! hides it, while launcher and notification activations show the same window.
//!
//! One process per user session: the first launch claims a Unix socket
//! ([`claim_instance`]); a later launch hands its request to that process over
//! the socket and exits, so a second `cosmic-mail` from the launcher focuses the
//! running window instead of starting a second sync engine. On Windows, Rune's
//! `single_instance` (a per-user named pipe) does the same job; see `app.rs`.

use std::ffi::OsStr;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::wire::ImapAccountInput;

pub const BACKGROUND_ARG: &str = "--background";

/// Whether argv requests a silent, background-only startup.
pub fn is_background_launch<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(BACKGROUND_ARG))
}

/// Open an `http(s)` URL in the user's browser: `xdg-open` on Linux, `open`
/// on macOS, the shell's URL handler on Windows.
///
/// Anything else (`javascript:`, `file:`, `mailto:`, relative hrefs) is
/// refused: URLs reach here from untrusted mail content.
pub fn open_url(url: &str) -> Result<()> {
    let Some(url) = openable_url(url) else {
        bail!("refusing to open a non-http(s) URL");
    };
    #[cfg(windows)]
    let mut command = {
        // url.dll's handler takes the URL as one argument, with no cmd.exe
        // parsing in between (`cmd /c start` would interpret & and ^).
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("launching the URL handler")?;
    Ok(())
}

/// The URL as an absolute `http`/`https` URL, or `None` for anything else.
pub fn openable_url(href: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(href.trim()).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

/// Ask Hyprland to focus the window after a user activation.
///
/// Wayland compositors may reject focus requests from a background client.
/// Cosmic Mail targets Omarchy/Hyprland, whose IPC can honor an explicit user
/// activation. Fixed arguments, no shell.
pub fn hyprland_focus() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    tracing::debug!("requesting application focus through hyprctl");
    match std::process::Command::new("hyprctl")
        .args(["dispatch", "focuswindow", "class:^(cosmic-mail)$"])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(%status, "hyprctl application focus failed"),
        Err(err) => tracing::warn!(error = %err, "could not run hyprctl application focus"),
    }
}

/// What a second launch, or another app, asks of the running process.
#[derive(Clone, PartialEq, Eq)]
pub enum InstanceRequest {
    /// Show and focus the window, with the launcher's XDG activation token if any.
    Activate { token: Option<String> },
    /// A `--background` launch: the process already runs, nothing to show.
    Background,
    /// Add an IMAP account, as Add account does (it's validated by connecting first). Sent by
    /// Postmaster after it makes the mailbox an app password; replied to with the account id.
    AddAccount(Box<ImapAccountInput>),
    /// Remove the account with this address, as Settings' Remove does. Sent by Postmaster after
    /// it deletes the mailbox.
    RemoveAccount { email: String },
}

/// The password in an `AddAccount` must never reach a log through `{:?}`.
impl std::fmt::Debug for InstanceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceRequest::Activate { token } => f
                .debug_struct("Activate")
                .field("token", &token.as_ref().map(|_| "<token>"))
                .finish(),
            InstanceRequest::Background => f.write_str("Background"),
            InstanceRequest::AddAccount(input) => {
                f.debug_tuple("AddAccount").field(&input.email).finish()
            }
            InstanceRequest::RemoveAccount { email } => f
                .debug_struct("RemoveAccount")
                .field("email", email)
                .finish(),
        }
    }
}

impl InstanceRequest {
    /// The request a launch with these args and environment makes.
    pub fn for_launch<I, S>(args: I, token: Option<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if is_background_launch(args) {
            InstanceRequest::Background
        } else {
            InstanceRequest::Activate { token }
        }
    }

    /// One line, ending in a newline. Account JSON is compact, so it never contains one.
    #[cfg(unix)]
    pub fn encode(&self) -> String {
        match self {
            InstanceRequest::Background => "background\n".to_string(),
            InstanceRequest::Activate { token: None } => "activate\n".to_string(),
            InstanceRequest::Activate { token: Some(t) } => {
                // Tokens are opaque ASCII; strip anything that would break framing.
                let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
                format!("activate {t}\n")
            }
            InstanceRequest::AddAccount(input) => {
                // Serializing plain strings and numbers can't fail.
                let json = serde_json::to_string(input).unwrap_or_default();
                format!("add-account {json}\n")
            }
            InstanceRequest::RemoveAccount { email } => {
                let email: String = email.chars().filter(|c| !c.is_whitespace()).collect();
                format!("remove-account {email}\n")
            }
        }
    }

    #[cfg(unix)]
    pub fn decode(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\r', '\n']);
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "background" => Some(InstanceRequest::Background),
            "activate" => Some(InstanceRequest::Activate {
                token: rest.split_whitespace().next().map(str::to_string),
            }),
            "add-account" => serde_json::from_str(rest)
                .ok()
                .map(|input| InstanceRequest::AddAccount(Box::new(input))),
            "remove-account" => {
                rest.split_whitespace()
                    .next()
                    .map(|email| InstanceRequest::RemoveAccount {
                        email: email.to_string(),
                    })
            }
            _ => None,
        }
    }

    /// Whether the sender waits for a reply on the same connection.
    pub fn wants_reply(&self) -> bool {
        matches!(
            self,
            InstanceRequest::AddAccount(_) | InstanceRequest::RemoveAccount { .. }
        )
    }
}

/// The answer to a request that [wants one](InstanceRequest::wants_reply): `ok [detail]` or
/// `error <message>`, one line on the same connection. Dropping it unanswered closes the
/// connection, which the sender reads as "this Cosmic Mail doesn't do that".
#[cfg(unix)]
pub struct Reply(Option<UnixStream>);

#[cfg(unix)]
impl Reply {
    pub fn send(mut self, result: std::result::Result<String, String>) {
        let Some(mut stream) = self.0.take() else {
            return;
        };
        let line = match result {
            Ok(detail) if detail.is_empty() => "ok\n".to_string(),
            Ok(detail) => format!("ok {}\n", one_line(&detail)),
            Err(message) => format!("error {}\n", one_line(&message)),
        };
        if let Err(err) = stream.write_all(line.as_bytes()) {
            tracing::warn!(error = %err, "could not answer an instance request");
        }
    }

    /// A reply nobody reads, for tests and requests that don't want one.
    pub fn none() -> Reply {
        Reply(None)
    }
}

#[cfg(unix)]
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The per-user single-instance socket: `$XDG_RUNTIME_DIR/cosmic-mail.sock`.
#[cfg(unix)]
pub fn instance_socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir);
    dir.join("cosmic-mail.sock")
}

/// The outcome of [`claim_instance`].
#[cfg(unix)]
#[derive(Debug)]
pub enum Instance {
    /// This process is the only one: serve the listener.
    Primary(UnixListener),
    /// Another process runs and has been handed the request: exit.
    Secondary,
}

/// Become the session's only instance, or hand `request` to the one running.
#[cfg(unix)]
pub fn claim_instance(path: &Path, request: &InstanceRequest) -> Result<Instance> {
    match UnixStream::connect(path) {
        Ok(mut stream) => {
            stream
                .write_all(request.encode().as_bytes())
                .context("handing the launch to the running instance")?;
            return Ok(Instance::Secondary);
        }
        // Nobody listening: a stale socket from a crashed process, or none.
        Err(_) if path.exists() => {
            std::fs::remove_file(path)
                .with_context(|| format!("removing stale socket {}", path.display()))?;
        }
        Err(_) => {}
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    // Requests can carry a password, so only this user may connect, even when the socket falls
    // back to a shared temp folder.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
    }
    Ok(Instance::Primary(listener))
}

/// The longest request line read; an account is well under 2 KiB.
#[cfg(unix)]
const MAX_REQUEST: u64 = 16 * 1024;

/// Serve later launches, and Postmaster, on a background thread for the process lifetime.
/// `on_request` must not block: a request that wants a reply carries a [`Reply`] to answer
/// whenever its work is done.
#[cfg(unix)]
pub fn serve_instance(
    listener: UnixListener,
    on_request: impl Fn(InstanceRequest, Reply) + Send + 'static,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Ok(reader) = stream.try_clone() else {
                continue;
            };
            let mut line = String::new();
            if BufReader::new(reader.take(MAX_REQUEST))
                .read_line(&mut line)
                .is_ok()
            {
                if let Some(request) = InstanceRequest::decode(&line) {
                    let reply = if request.wants_reply() {
                        Reply(Some(stream))
                    } else {
                        Reply(None)
                    };
                    on_request(request, reply);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_background_argument() {
        assert!(is_background_launch(["cosmic-mail", BACKGROUND_ARG]));
        assert!(is_background_launch([
            "cosmic-mail",
            "--unrelated",
            BACKGROUND_ARG,
        ]));
    }

    #[test]
    fn normal_launch_is_not_background() {
        assert!(!is_background_launch(["cosmic-mail"]));
        assert!(!is_background_launch(["cosmic-mail", "--background-task",]));
    }

    #[test]
    fn only_http_and_https_urls_open() {
        assert!(openable_url("https://example.com/track?id=1").is_some());
        assert!(openable_url("http://example.com").is_some());
        assert!(openable_url("javascript:alert(1)").is_none());
        assert!(openable_url("file:///etc/passwd").is_none());
        assert!(openable_url("mailto:a@b.c").is_none());
        assert!(openable_url("/relative").is_none());
        assert!(openable_url("").is_none());
    }

    #[test]
    #[cfg(unix)]
    fn requests_round_trip() {
        for request in [
            InstanceRequest::Background,
            InstanceRequest::Activate { token: None },
            InstanceRequest::Activate {
                token: Some("abc_123".into()),
            },
        ] {
            assert_eq!(InstanceRequest::decode(&request.encode()), Some(request));
        }
        assert_eq!(InstanceRequest::decode("bogus"), None);
        assert_eq!(
            InstanceRequest::for_launch(["cosmic-mail"], Some("t".into())),
            InstanceRequest::Activate {
                token: Some("t".into())
            }
        );
    }

    #[test]
    #[cfg(unix)]
    fn second_launch_hands_its_request_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        let Instance::Primary(listener) =
            claim_instance(&path, &InstanceRequest::Background).unwrap()
        else {
            panic!("first claim must be primary");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        serve_instance(listener, move |r, _| tx.send(r).unwrap());

        let request = InstanceRequest::Activate {
            token: Some("tok".into()),
        };
        assert!(matches!(
            claim_instance(&path, &request).unwrap(),
            Instance::Secondary
        ));
        let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(got, request);
    }

    fn account() -> ImapAccountInput {
        ImapAccountInput {
            email: "lucy@leach.family".into(),
            display_name: "Lucy".into(),
            imap_host: "imap.purelymail.com".into(),
            imap_port: 993,
            smtp_host: "smtp.purelymail.com".into(),
            smtp_port: 465,
            username: "lucy@leach.family".into(),
            password: "hqvt 8rmw\nzx2p".into(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn account_requests_round_trip_on_one_line() {
        for request in [
            InstanceRequest::AddAccount(Box::new(account())),
            InstanceRequest::RemoveAccount {
                email: "lucy@leach.family".into(),
            },
        ] {
            let line = request.encode();
            assert_eq!(line.matches('\n').count(), 1, "{line:?}");
            assert!(request.wants_reply());
            assert_eq!(InstanceRequest::decode(&line), Some(request));
        }
        assert_eq!(InstanceRequest::decode("add-account {not json"), None);
        assert_eq!(InstanceRequest::decode("remove-account"), None);
        assert!(!InstanceRequest::Background.wants_reply());
    }

    #[test]
    fn debug_never_shows_a_password_or_token() {
        let printed = format!(
            "{:?} {:?} {:?}",
            InstanceRequest::AddAccount(Box::new(account())),
            account(),
            InstanceRequest::Activate {
                token: Some("secret-token".into())
            }
        );
        assert!(!printed.contains("hqvt"), "{printed}");
        assert!(!printed.contains("secret-token"), "{printed}");
        assert!(printed.contains("lucy@leach.family"));
    }

    /// Plays Postmaster: sends one request and reads the one-line reply, if any.
    #[cfg(unix)]
    fn ask(path: &Path, request: &str) -> String {
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        reply
    }

    #[test]
    #[cfg(unix)]
    fn account_requests_are_answered_on_the_same_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        let Instance::Primary(listener) =
            claim_instance(&path, &InstanceRequest::Background).unwrap()
        else {
            panic!("first claim must be primary");
        };
        serve_instance(listener, |request, reply| match request {
            InstanceRequest::AddAccount(input) => {
                // Answered later, from another thread, as the app does.
                std::thread::spawn(move || reply.send(Ok(format!("id-for {}", input.email))));
            }
            InstanceRequest::RemoveAccount { email } => {
                reply.send(Err(format!("{email} isn't\nin Cosmic Mail")))
            }
            _ => {}
        });

        let added = ask(
            &path,
            &InstanceRequest::AddAccount(Box::new(account())).encode(),
        );
        assert_eq!(added, "ok id-for lucy@leach.family\n");
        let removed = ask(&path, "remove-account sam@leach.family\n");
        assert_eq!(removed, "error sam@leach.family isn't in Cosmic Mail\n");
        // A verb this version doesn't know gets no answer: the connection just closes.
        assert_eq!(ask(&path, "rename-account a@b.c\n"), "");
    }

    #[test]
    #[cfg(unix)]
    fn only_this_user_can_connect() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        let _instance = claim_instance(&path, &InstanceRequest::Background).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn stale_socket_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        drop(UnixListener::bind(&path).unwrap()); // leaves the file, nobody listening
        assert!(matches!(
            claim_instance(&path, &InstanceRequest::Background).unwrap(),
            Instance::Primary(_)
        ));
    }
}
