//! Desktop process and window lifecycle.
//!
//! The application process owns background sync for the whole graphical
//! session. Its main window is only a view onto that process: closing the view
//! hides it, while launcher and notification activations show the same window.
//!
//! One process per user session: the first launch claims a Unix socket
//! ([`claim_instance`]); a later launch hands its request to that process over
//! the socket and exits, so a second `cosmic-mail` from the launcher focuses the
//! running window instead of starting a second sync engine.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

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

/// Open an `http(s)` URL in the user's browser via `xdg-open`.
///
/// Anything else (`javascript:`, `file:`, `mailto:`, relative hrefs) is
/// refused: URLs reach here from untrusted mail content.
pub fn open_url(url: &str) -> Result<()> {
    let Some(url) = openable_url(url) else {
        bail!("refusing to open a non-http(s) URL");
    };
    std::process::Command::new("xdg-open")
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("running xdg-open")?;
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

/// What a second launch asks of the running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceRequest {
    /// Show and focus the window, with the launcher's XDG activation token if any.
    Activate { token: Option<String> },
    /// A `--background` launch: the process already runs, nothing to show.
    Background,
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

    fn encode(&self) -> String {
        match self {
            InstanceRequest::Background => "background\n".to_string(),
            InstanceRequest::Activate { token: None } => "activate\n".to_string(),
            InstanceRequest::Activate { token: Some(t) } => {
                // Tokens are opaque ASCII; strip anything that would break framing.
                let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
                format!("activate {t}\n")
            }
        }
    }

    fn decode(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        match words.next()? {
            "background" => Some(InstanceRequest::Background),
            "activate" => Some(InstanceRequest::Activate {
                token: words.next().map(str::to_string),
            }),
            _ => None,
        }
    }
}

/// The per-user single-instance socket: `$XDG_RUNTIME_DIR/cosmic-mail.sock`.
pub fn instance_socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir);
    dir.join("cosmic-mail.sock")
}

/// The outcome of [`claim_instance`].
#[derive(Debug)]
pub enum Instance {
    /// This process is the only one: serve the listener.
    Primary(UnixListener),
    /// Another process runs and has been handed the request: exit.
    Secondary,
}

/// Become the session's only instance, or hand `request` to the one running.
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
    Ok(Instance::Primary(listener))
}

/// Serve later launches on a background thread for the process lifetime.
pub fn serve_instance(
    listener: UnixListener,
    on_request: impl Fn(InstanceRequest) + Send + 'static,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                if let Some(request) = InstanceRequest::decode(&line) {
                    on_request(request);
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
    fn second_launch_hands_its_request_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        let Instance::Primary(listener) =
            claim_instance(&path, &InstanceRequest::Background).unwrap()
        else {
            panic!("first claim must be primary");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        serve_instance(listener, move |r| tx.send(r).unwrap());

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

    #[test]
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
