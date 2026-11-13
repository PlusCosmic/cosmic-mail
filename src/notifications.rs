//! Desktop notifications via `notify-rust` (rendered by mako on omarchy).
//!
//! Notifications carry the app-name "Cosmic Mail" (so users can theme/script
//! per app-name in mako) and a `dev.pluscosmic.mail` desktop-entry hint. Each
//! notification exposes a single `default` action; activating it emits
//! `AppEvent::Activate`, which shows and focuses the main window. Action
//! handling is spawned so waiting on the user does not block the sync engine.
//! Actions and hints are freedesktop (D-Bus) features: on Windows and macOS a
//! notification just shows.
//!
//! Sound follows `Settings.notificationSound`: on macOS the system alert sound
//! (`NSUserNotificationDefaultSoundName`), on Windows the "Mail" toast sound, and
//! on Linux the freedesktop `message-new-email` sound-name hint (mako plays
//! nothing itself; an `on-notify` rule in mako's config can). Only the first
//! notification of a batch makes a sound.
//!
//! On macOS notifications are posted as the app bundle (`dev.pluscosmic.mail`,
//! registered with Launch Services by installing `Cosmic Mail.app`), so they carry
//! its name and icon and are allowed or muted under its entry in System Settings →
//! Notifications. Without that, `mac-notification-sys` posts them as Finder.

#[cfg(all(unix, not(target_os = "macos")))]
use crate::events::AppEvent;
use crate::events::Events;
use notify_rust::Notification;
#[cfg(all(unix, not(target_os = "macos")))]
use notify_rust::{Hint, NotificationResponse};

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

    // Actions and the desktop-entry hint are the freedesktop (D-Bus) spec;
    // elsewhere the notification just shows (Windows toasts open the app on
    // click by themselves).
    #[cfg(target_os = "macos")]
    {
        let _ = events;
        set_macos_identity();
        // `show` only builds a handle; dropping it posts the notification and
        // waits (up to a couple of seconds) for delivery. Keep that wait off the
        // sync engine.
        std::thread::spawn(move || {
            if let Err(err) = notification.show() {
                tracing::warn!(error = %err, "failed to show notification");
            }
        });
    }
    #[cfg(target_os = "windows")]
    {
        let _ = events;
        if let Err(err) = notification.show() {
            tracing::warn!(error = %err, "failed to show notification");
        }
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
                if let Err(err) =
                    handle.wait_for_response(|response: &NotificationResponse| match response {
                        NotificationResponse::Default | NotificationResponse::Action(_) => {
                            tracing::debug!(?response, "notification action received");
                            events.emit(AppEvent::Activate);
                        }
                        NotificationResponse::Closed(reason) => {
                            tracing::debug!(?reason, "notification closed");
                        }
                        NotificationResponse::Reply(_) => {}
                    })
                {
                    tracing::warn!(error = %err, "notification action listener failed");
                }
            });
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to show notification");
        }
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
    fn nothing_new_notifies_nothing() {
        assert!(batch("me@example.com", &[]).is_empty());
    }
}
