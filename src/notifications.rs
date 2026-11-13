//! Desktop notifications via `notify-rust` on Windows, macOS and Linux.
//!
//! On Linux, notifications carry the app-name "Cosmic Mail" (so users can
//! theme/script per app-name in their notification daemon) and a
//! `dev.pluscosmic.mail` desktop-entry hint. Each
//! notification exposes a single `default` action; activating it emits
//! `AppEvent::Activate`, which shows and focuses the main window. Action
//! handling is spawned so waiting on the user does not block the sync engine.
//! On macOS and Windows a click on the notification does the same (macOS:
//! `NSUserNotificationCenter` reports it to the posting process, and Rune's loop
//! pumps the main run loop it arrives on; Windows: the toast's `Activated` event).
//! Actions and hints are otherwise freedesktop (D-Bus) features.
//!
//! Sound follows `Settings.notificationSound`: on macOS the system alert sound
//! (`NSUserNotificationDefaultSoundName`), on Windows the "Mail" toast sound, and
//! on Linux the freedesktop `message-new-email` sound-name hint (daemons that
//! play sounds honour it; mako plays none itself, but an `on-notify` rule can). Only the first
//! notification of a batch makes a sound.
//!
//! On macOS notifications are posted as the app bundle (`dev.pluscosmic.mail`,
//! registered with Launch Services by installing `Cosmic Mail.app`), so they carry
//! its name and icon and are allowed or muted under its entry in System Settings →
//! Notifications. Without that, `mac-notification-sys` posts them as Finder.
//!
//! On Windows toasts are posted under the AppUserModelID `dev.pluscosmic.mail`
//! (without one, `notify-rust` uses PowerShell's, and the toast reads "Windows
//! PowerShell"). The installer puts that id on its Start-menu shortcut, and the
//! app itself registers it under `HKCU\Software\Classes\AppUserModelId` before
//! its first toast, so the portable zip (no shortcut) is named Cosmic Mail too.

use crate::events::{AppEvent, Events};
#[cfg(all(unix, not(target_os = "macos")))]
use notify_rust::Hint;
use notify_rust::{Notification, NotificationResponse};

const APP_NAME: &str = "Cosmic Mail";
const DESKTOP_ENTRY: &str = "dev.pluscosmic.mail";

/// A single new-message notification's content.
pub struct NewMail {
    pub from_name: String,
    pub subject: String,
}

/// Send notifications for a batch of new messages that arrived in an inbox
/// during one sync pass. Per the contract: 1..=3 messages produce one
/// notification each; more than 3 collapse into a single summary notification.
/// With `sound`, the first notification of the batch plays the new-mail sound.
pub fn notify_new_mail(events: &Events, account_email: &str, new: &[NewMail], sound: bool) {
    for (i, (summary, body)) in batch(account_email, new).into_iter().enumerate() {
        show(events, &summary, &body, sound && i == 0);
    }
}

/// The (summary, body) of each notification for one batch of new mail.
fn batch(account_email: &str, new: &[NewMail]) -> Vec<(String, String)> {
    if new.len() > 3 {
        vec![(
            format!("{} new messages", new.len()),
            format!("in {account_email}"),
        )]
    } else {
        new.iter()
            .map(|m| (format!("New mail — {}", m.from_name), m.subject.clone()))
            .collect()
    }
}

/// Send a sample notification (used by the `test_notification` command), with
/// the new-mail sound when `sound` is on.
pub fn test(events: &Events, sound: bool) {
    show(
        events,
        "Cosmic Mail",
        "Notifications are working. Click to focus the window.",
        sound,
    );
}

/// Post macOS notifications as the app bundle rather than Finder. Must run
/// before the first notification; `mac-notification-sys` takes the identity
/// once per process.
#[cfg(target_os = "macos")]
fn set_macos_identity() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Err(err) = notify_rust::set_application(DESKTOP_ENTRY) {
            // Not registered with Launch Services (Cosmic Mail.app was never
            // installed): notifications still show, under another app's name.
            tracing::warn!(error = %err, "could not post notifications as {DESKTOP_ENTRY}");
        }
    });
}

/// Build, show, and wire up the `default` action for a notification.
fn show(events: &Events, summary: &str, body: &str, sound: bool) {
    let mut notification = Notification::new();
    notification
        .appname(APP_NAME)
        .summary(summary)
        .body(body)
        .icon(DESKTOP_ENTRY);

    #[cfg(target_os = "macos")]
    if sound {
        notification.sound_name("NSUserNotificationDefaultSoundName");
    }
    #[cfg(target_os = "windows")]
    if sound {
        notification.sound_name("Mail");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    notification.hint(if sound {
        Hint::SoundName("message-new-email".to_string())
    } else {
        Hint::SuppressSound(true)
    });

    // Actions and the desktop-entry hint are the freedesktop (D-Bus) spec; macOS
    // and Windows report a click on the notification itself.
    #[cfg(target_os = "windows")]
    notification.app_id(DESKTOP_ENTRY);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let events = events.clone();
        // On macOS `show` only builds a handle; posting happens on the handle's
        // thread, either waiting for the click (`listen`) or, past the cap,
        // fire-and-forget on drop (which still waits up to a couple of seconds for
        // delivery). On Windows `show` posts the toast. Either way the identity is
        // set up here, off the sync engine's thread.
        std::thread::spawn(move || {
            #[cfg(target_os = "macos")]
            set_macos_identity();
            #[cfg(target_os = "windows")]
            register_windows_app_id();
            match notification.show() {
                Ok(handle) => match ClickListener::claim() {
                    Some(_slot) => listen(&events, |respond| handle.wait_for_response(respond)),
                    None => drop(handle),
                },
                Err(err) => tracing::warn!(error = %err, "failed to show notification"),
            }
        });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    notification
        .hint(Hint::DesktopEntry(DESKTOP_ENTRY.to_string()))
        .action("default", "Open");

    #[cfg(all(unix, not(target_os = "macos")))]
    match notification.show() {
        Ok(handle) => {
            let events = events.clone();
            // wait_for_response blocks until the user acts or the notification
            // closes; run it off the sync/async runtime on a dedicated thread.
            std::thread::spawn(move || {
                listen(&events, |respond| handle.wait_for_response(respond))
            });
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to show notification");
        }
    }
}

/// Block until the user clicks the notification (emit `AppEvent::Activate`) or
/// it goes away. Runs on its own thread. `wait` is the handle's
/// `wait_for_response`, passed as a closure because `notify-rust` does not
/// export its Windows `NotificationHandle` type.
fn listen(
    events: &Events,
    wait: impl FnOnce(&dyn Fn(&NotificationResponse)) -> notify_rust::error::Result<()>,
) {
    let result = wait(&|response: &NotificationResponse| {
        if is_click(response) {
            tracing::debug!(?response, "notification action received");
            events.emit(AppEvent::Activate);
        } else {
            tracing::debug!(?response, "notification closed");
        }
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, "notification action listener failed");
    }
}

/// Whether a response means "open the app": the notification's body or its
/// action, not a dismissal or a reply.
fn is_click(response: &NotificationResponse) -> bool {
    matches!(
        response,
        NotificationResponse::Default | NotificationResponse::Action(_)
    )
}

/// Register the AppUserModelID toasts are posted under, with Cosmic Mail's name
/// and (when `icon.png` sits beside the executable, as the installer and the
/// portable zip put it) icon, so Windows labels the toast as Cosmic Mail and not
/// PowerShell. Per user (HKCU), once per process; rewriting the same values is
/// harmless, and keeps `IconUri` right if a portable copy moves. The installer's
/// shortcut carries the id too, and its uninstaller removes the key.
#[cfg(target_os = "windows")]
fn register_windows_app_id() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let key = format!(r"HKCU\Software\Classes\AppUserModelId\{DESKTOP_ENTRY}");
        let mut values = vec![("DisplayName", APP_NAME.to_string())];
        let icon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("icon.png")))
            .filter(|icon| icon.is_file());
        if let Some(icon) = icon {
            values.push(("IconUri", icon.display().to_string()));
        }
        for (name, data) in values {
            if let Err(err) = reg_add(&key, name, &data) {
                // Toasts still show; Windows may label them with the bare id.
                tracing::warn!(error = %err, "could not register {DESKTOP_ENTRY} ({name})");
            }
        }
    });
}

/// Set one string value under an HKCU key with `reg.exe` (fixed arguments, no
/// shell; no console window flashes up).
#[cfg(target_os = "windows")]
fn reg_add(key: &str, name: &str, data: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = Command::new("reg")
        .args(["add", key, "/v", name, "/t", "REG_SZ", "/d", data, "/f"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    anyhow::ensure!(status.success(), "reg add exited with {status}");
    Ok(())
}

/// A click listener's slot (macOS and Windows). A listener thread waits until its
/// notification is clicked or goes away: on macOS a notification left in
/// Notification Center keeps it waiting until clicked or cleared; on Windows the
/// toast's `Dismissed` event ends it when the toast leaves the screen. So the
/// number of listeners is capped; past the cap a notification still shows, and
/// a click on it still brings the app forward on macOS (AppKit activates the
/// bundle), it just can't reopen a closed window.
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ClickListener;

#[cfg(any(target_os = "macos", target_os = "windows"))]
static CLICK_LISTENERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl ClickListener {
    const MAX: usize = 32;

    fn claim() -> Option<ClickListener> {
        use std::sync::atomic::Ordering;
        CLICK_LISTENERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < Self::MAX).then_some(n + 1)
            })
            .ok()
            .map(|_| ClickListener)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for ClickListener {
    fn drop(&mut self) {
        CLICK_LISTENERS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(n: usize) -> Vec<NewMail> {
        (0..n)
            .map(|i| NewMail {
                from_name: format!("Sender {i}"),
                subject: format!("Subject {i}"),
            })
            .collect()
    }

    #[test]
    fn up_to_three_messages_notify_one_each() {
        let out = batch("me@example.com", &mail(3));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ("New mail — Sender 0".into(), "Subject 0".into()));
    }

    #[test]
    fn more_than_three_collapse_into_one() {
        let out = batch("me@example.com", &mail(4));
        assert_eq!(
            out,
            vec![("4 new messages".into(), "in me@example.com".into())]
        );
    }

    #[test]
    fn only_a_click_opens_the_app() {
        use notify_rust::CloseReason;
        assert!(is_click(&NotificationResponse::Default));
        assert!(is_click(&NotificationResponse::Action("default".into())));
        assert!(!is_click(&NotificationResponse::Closed(
            CloseReason::Dismissed
        )));
        assert!(!is_click(&NotificationResponse::Reply("hi".into())));
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn click_listeners_are_capped() {
        let held: Vec<_> = std::iter::from_fn(ClickListener::claim)
            .take(ClickListener::MAX + 1)
            .collect();
        assert_eq!(held.len(), ClickListener::MAX);
        drop(held);
        assert!(ClickListener::claim().is_some(), "slots come back on drop");
    }

    #[test]
    fn nothing_new_notifies_nothing() {
        assert!(batch("me@example.com", &[]).is_empty());
    }
}
